//! A playback deck and the worker loop that drives a set of them.
//!
//! A [`Deck`] is one rodio `Sink` fed from an in-RAM copy of the track, plus the
//! load/seek/ended bookkeeping around it. [`run`] ticks any number of decks
//! against a single shared [`Output`], so handover between decks needs no
//! cross-thread coordination: the loop that tracks every playhead is the loop
//! that would start the next one.
//!
//! Decks emit on **role-mapped** topics: whichever deck holds [`DeckRole::Main`]
//! emits `main-deck:*`. Roles move between decks; decks do not move between
//! roles.

use anyhow::Result;
use rodio::mixer::Mixer;
use rodio::Sink;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::cache::Cache;
use super::cue_points::{CuePoints, Resolved};
use super::output::Output;
use super::player::{
    append_span, clamp_start, decode_bytes, read_file, read_with_retry, Bytes, Cmd, PlayerTuning,
    RampDone, Topics,
};

const TICK_INTERVAL: Duration = Duration::from_millis(50);
const TIME_EMIT_INTERVAL: Duration = Duration::from_millis(100);

/// A physical deck. The slot is an identity: it never changes what it is, only
/// what it is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeckSlot {
    A,
    B,
}

/// What a deck is doing right now. `Main` is on air and defines Now playing;
/// `Arm` is loaded and waiting to take over; `Tail` has handed over and is
/// playing the outgoing track out — audible, but no longer Now playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeckRole {
    Main,
    Arm,
    Tail,
}

/// What a command aimed at the `main` role does to a deck playing a tail under
/// it: any explicit change to what is on air cuts the tail, and only reaching
/// its own cue out lets one finish. Seek and volume leave it alone — they act
/// on the incoming track, which is what `main` means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailAction {
    Cut,
    Pause,
    Resume,
}

fn tail_companion(cmd: &Cmd) -> Option<TailAction> {
    match cmd {
        // Next, prev, play-index and play-now all reach the deck as a Load.
        Cmd::Load { .. } | Cmd::Stop => Some(TailAction::Cut),
        Cmd::Pause => Some(TailAction::Pause),
        Cmd::Play => Some(TailAction::Resume),
        Cmd::Seek(_) | Cmd::SetVolume(_) => None,
        // A fade is an explicit change to what is on air, but not yet: cutting
        // the tail here would leave it silent for the length of the ramp while
        // `main` was still audible. The tail is faded alongside and cut when
        // the ramp completes — see [`fade_main`].
        Cmd::Fade { .. } => None,
        // Handled before dispatch; it never reaches a single deck.
        Cmd::HandOverNow { .. } => None,
    }
}

/// A live gain ramp on one deck: "from here, to `to`, over `ms`".
///
/// Deliberately distinct from a track's stored fades, which are keyed on file
/// position and applied at the source (`audio/envelope.rs`). This is a deck
/// control, so the two compose by multiplication instead of fighting over one
/// value.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    from: f32,
    to: f32,
    started: Instant,
    ms: u64,
    on_complete: Option<RampDone>,
}

/// Gain part-way through a ramp, and whether it has finished.
///
/// Linear, matching the stored envelope's own curve (`envelope::gain_at`): the
/// live ramp and a track's stored fade-in have unrelated durations, so there is
/// no symmetric crossfade for an equal-power law to preserve — one curve
/// convention across the codebase is worth more.
///
/// Pure so the interpolation is testable without an audio device, the way
/// [`handover_due`] and [`watchdog_timed_out`] are.
fn ramp_gain(from: f32, to: f32, elapsed: Duration, ms: u64) -> (f32, bool) {
    if ms == 0 {
        return (to, true);
    }
    let t = elapsed.as_secs_f32() / (ms as f32 / 1000.0);
    if t >= 1.0 {
        return (to, true);
    }
    (from + (to - from) * t, false)
}

/// Whether the deck holding `main` should hand over on this tick: it has
/// reached the outgoing track's next start, and there is a deck armed and
/// decoded to hand over to.
///
/// Pure so the trigger is testable without an audio device, the way
/// [`watchdog_timed_out`] is.
fn handover_due(pos: f64, next_start: Option<f64>, playing: bool, arm_ready: bool) -> bool {
    match next_start {
        Some(at) => playing && arm_ready && pos >= at,
        None => false,
    }
}

/// Where a deck's events go while it holds its current role, and the
/// pause-state mirror that goes with them.
///
/// A renderer that attaches after an event was emitted — a reload, or a session
/// restored before the window existed — reads `playing` instead of assuming.
pub(super) struct DeckEvents {
    pub topics: Topics,
    pub playing: Arc<AtomicBool>,
}

impl DeckEvents {
    pub(super) fn new(prefix: &str) -> Self {
        Self {
            topics: Topics::new(prefix),
            playing: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// A load that could not be started because no audio output was openable.
/// Retained so the idle auto-retry loop can replay it once the device returns.
#[derive(Clone)]
struct PendingLoad {
    id: i64,
    path: PathBuf,
    duration: Option<f64>,
    cue_points: CuePoints,
    start_at: f64,
    autoplay: bool,
    replay_gain: f32,
}

/// Result of a background file read, routed back to the worker thread.
/// `generation` lets the worker discard reads that a newer `Load`/`Stop` has
/// superseded (e.g. the user skipped again before a slow read finished).
struct LoadMsg {
    /// Index of the deck this read was issued for. One worker serves several
    /// decks, so a completed read has to find its way back to the right one.
    deck: usize,
    generation: u64,
    /// Track id this read was issued for; reported in `:load-failed` on failure.
    id: i64,
    duration: Option<f64>,
    cue_points: CuePoints,
    start_at: f64,
    autoplay: bool,
    replay_gain: f32,
    bytes: Result<Bytes>,
}

pub(super) struct Deck {
    slot: DeckSlot,
    role: DeckRole,
    /// This deck's connection to the shared mixer. `None` until the output has
    /// opened; rebuilt whenever the output re-opens (`sink_generation`).
    sink: Option<Sink>,
    sink_generation: u64,
    /// Track id of the most recent `Load`, reported in `:load-failed`.
    current_id: Option<i64>,
    current_path: Option<PathBuf>,
    current_duration: Option<f64>,
    /// The loaded track's cue points, resolved against the duration the decoder
    /// reported. Absolute file positions: everything the worker emits or
    /// receives is air time, and this is what converts between the two.
    cue: Resolved,
    /// Bytes of the currently loaded track, kept so seeks re-decode from RAM.
    current_bytes: Option<Bytes>,
    /// Absolute file position the current source starts at.
    seek_offset: f64,
    active: bool,
    /// A background read is in flight; suppresses ended-detection and time
    /// emits until the source is ready.
    loading: bool,
    /// When the in-flight read started; drives the watchdog timeout. `None`
    /// whenever no read is pending.
    load_start: Option<Instant>,
    /// Monotonic token identifying the most recent load intent. Bumped on every
    /// `Load` and `Stop`; background reads carry the token they were issued for.
    generation: u64,
    volume: f32,
    /// Live ramp gain, multiplied into `volume` on its way to the sink. `1.0`
    /// whenever no fade is running, which every transport command restores —
    /// so a deck can never be left quiet for the next track.
    gain: f32,
    /// The ramp currently stepping `gain`, if any.
    ramp: Option<Ramp>,
    /// The loaded track's ReplayGain factor, kept so a re-decode on seek
    /// levels it the same way the original load did. Distinct from `gain`,
    /// which is the live ramp: this one is a property of the track, set once
    /// per load and never stepped.
    replay_gain: f32,
    /// A load deferred because no audio output could be opened. The idle loop
    /// retries the open and replays this load once a device is available (#259).
    pending_load: Option<PendingLoad>,
    last_time_emit: Instant,
}

impl Deck {
    pub(super) fn new(slot: DeckSlot, role: DeckRole) -> Self {
        Self {
            slot,
            role,
            sink: None,
            sink_generation: 0,
            current_id: None,
            current_path: None,
            current_duration: None,
            cue: Resolved::default(),
            current_bytes: None,
            seek_offset: 0.0,
            active: false,
            loading: false,
            load_start: None,
            generation: 0,
            volume: 1.0,
            gain: 1.0,
            ramp: None,
            replay_gain: 1.0,
            pending_load: None,
            last_time_emit: Instant::now()
                .checked_sub(TIME_EMIT_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    /// What the sink is actually set to: the operator's deck volume scaled by
    /// any live ramp. Every `set_volume` goes through here, so a sink rebuilt
    /// mid-fade (an output reopen, a seek) resumes at the ramp's level instead
    /// of jumping back to full.
    fn effective_volume(&self) -> f32 {
        (self.volume * self.gain).clamp(0.0, 1.0)
    }

    /// Connect to the mixer if this deck has no live sink. A sink built against
    /// an older output generation belongs to a dropped stream and is discarded.
    fn ensure_sink(&mut self, mixer: &Mixer, generation: u64) {
        if self.sink_generation != generation {
            self.sink = None;
        }
        if self.sink.is_none() {
            let sink = Sink::connect_new(mixer);
            sink.set_volume(self.effective_volume());
            self.sink = Some(sink);
            self.sink_generation = generation;
        }
    }

    /// Stop whatever is playing and start from a brand-new sink. Used wherever
    /// the queued source has to go away immediately (load, stop, seek).
    fn replace_sink(&mut self, mixer: &Mixer, generation: u64) {
        if let Some(sink) = self.sink.as_ref() {
            sink.stop();
        }
        let sink = Sink::connect_new(mixer);
        sink.set_volume(self.effective_volume());
        self.sink = Some(sink);
        self.sink_generation = generation;
    }

    /// Start a ramp from wherever the gain is now.
    fn start_ramp(&mut self, to: f32, ms: u64, on_complete: Option<RampDone>) {
        self.ramp = Some(Ramp {
            from: self.gain,
            to: to.clamp(0.0, 1.0),
            started: Instant::now(),
            ms,
            on_complete,
        });
    }

    /// Step a running ramp and apply the new gain. Returns what the ramp asked
    /// to happen once it finished, which the caller runs — completion needs the
    /// whole deck set (a stop cuts the tail too), not just this deck.
    fn step_ramp(&mut self, now: Instant) -> Option<Option<RampDone>> {
        let ramp = self.ramp?;
        let (gain, done) = ramp_gain(ramp.from, ramp.to, now - ramp.started, ramp.ms);
        self.gain = gain;
        if let Some(sink) = self.sink.as_ref() {
            sink.set_volume(self.effective_volume());
        }
        if !done {
            return None;
        }
        self.ramp = None;
        Some(ramp.on_complete)
    }

    /// Abandon any ramp and restore full gain. Every transport command does
    /// this before acting: a fade is only ever the most recent intent.
    fn cancel_ramp(&mut self) {
        if self.ramp.take().is_none() && self.gain == 1.0 {
            return;
        }
        self.gain = 1.0;
        if let Some(sink) = self.sink.as_ref() {
            sink.set_volume(self.effective_volume());
        }
    }

    /// Drop everything about the loaded track. Shared by `Stop` and by every
    /// failure path, which want exactly the same end state: nothing loaded,
    /// nothing in flight, no deferred load waiting on a device.
    fn reset(&mut self) {
        self.active = false;
        self.loading = false;
        self.load_start = None;
        self.current_id = None;
        self.current_path = None;
        self.current_duration = None;
        self.cue = Resolved::default();
        self.current_bytes = None;
        self.seek_offset = 0.0;
        self.pending_load = None;
        self.cancel_ramp();
    }
}

/// The `program:handover` payload: the `main` role moved from one deck to
/// another. What the playlist engine reconciles against.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Handover {
    from: Option<i64>,
    to: i64,
}

/// One entry of the `program:roles` snapshot: which slot holds which role, and
/// what is on it.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoleEntry {
    slot: DeckSlot,
    role: DeckRole,
    track_id: Option<i64>,
}

/// The decks one worker drives, and how their events are addressed.
pub(super) struct DeckSet {
    pub decks: Vec<Deck>,
    /// Indexed by `DeckRole as usize`: the topics and pause-state mirror for
    /// whichever deck currently holds that role.
    pub events: Vec<DeckEvents>,
    /// Topic for the slot→role snapshot, or `None` for a worker whose decks
    /// have no roles to speak of (the cue deck).
    pub roles_topic: Option<&'static str>,
    /// Topic a move of the `main` role is announced on, or `None` for a worker
    /// that never hands over.
    pub handover_topic: Option<&'static str>,
    /// Topic a completed fade to silence on `main` is announced on, so the
    /// playlist can take the track off air. `None` for a worker whose decks
    /// nothing else owns (the cue deck).
    pub faded_out_topic: Option<&'static str>,
}

/// Emit a pause-state change and record it on the role's `playing` mirror.
///
/// Every pause-state transition goes through here, so the flag the handle
/// exposes cannot drift from what the renderer was told.
fn set_pause_state(app: &AppHandle, events: &DeckEvents, paused: bool) {
    events.playing.store(!paused, Ordering::SeqCst);
    let _ = app.emit(&events.topics.pause_state, paused);
}

/// Report where a load leaves the deck, at the moment the `Load` is taken
/// rather than when the read lands. The outgoing track's audio is already gone
/// by then, so a renderer told nothing keeps drawing its playhead — and, for a
/// parked load, its transport state — over the incoming track for as long as
/// the file takes to arrive. A load that will autoplay stays "playing": it is
/// buffering into playback, which is what the buffering event says.
fn report_load_start(app: &AppHandle, events: &DeckEvents, start_at: f64, autoplay: bool) {
    let _ = app.emit(&events.topics.time, start_at.max(0.0));
    if !autoplay {
        set_pause_state(app, events, true);
    }
}

/// Emit an `output-unavailable` event only when the availability actually
/// changes, so the idle retry loop's repeated failures don't spam the UI and a
/// recovery reliably clears the banner.
fn report_output(output: &mut Output, app: &AppHandle, events: &DeckEvents, ok: bool) {
    if output.set_ok(ok) {
        let _ = app.emit(&events.topics.output_unavailable, !ok);
    }
}

/// Ensure the shared output is open and hand back the mixer to connect to.
/// Returns `None` — after emitting an `error` event — when no audio device can
/// be opened at all, so the caller drops the current command instead of the
/// whole worker thread exiting. A dead worker silently discards every later
/// command, disabling playback for the rest of the session; see issue #259.
fn ensure_output(
    output: &mut Output,
    app: &AppHandle,
    events: &DeckEvents,
) -> Option<(Mixer, u64)> {
    if let Err(e) = output.open() {
        log::error!("player: no audio output available: {}", e);
        let _ = app.emit(
            &events.topics.error,
            format!("audio output unavailable: {}", e),
        );
        report_output(output, app, events, false);
        return None;
    }
    report_output(output, app, events, true);
    output.current()
}

/// Start loading a track: (re)open the output, stop current audio, and kick off
/// the background read (or route a cache hit). If no audio device can be opened,
/// the load is *deferred* — stored in the deck's `pending_load` for the idle
/// loop to replay once a device returns — rather than dropped, so playback
/// self-heals without user action (#259).
#[allow(clippy::too_many_arguments)]
fn start_load(
    app: &AppHandle,
    output: &mut Output,
    deck: &mut Deck,
    events: &DeckEvents,
    load_tx: &Sender<LoadMsg>,
    cache: &Arc<Cache>,
    read_retry_backoffs: &[Duration],
    deck_index: usize,
    id: i64,
    path: PathBuf,
    duration: Option<f64>,
    cue_points: CuePoints,
    start_at: f64,
    autoplay: bool,
    replay_gain: f32,
) {
    let Some((mixer, generation)) = ensure_output(output, app, events) else {
        // No device yet: remember the intent and let the idle loop retry the
        // open. `ensure_output` already emitted the error; keep the buffering
        // indicator up while we wait for the device.
        deck.pending_load = Some(PendingLoad {
            id,
            path,
            duration,
            cue_points,
            start_at,
            autoplay,
            replay_gain,
        });
        output.mark_retry_now();
        deck.current_id = Some(id);
        let _ = app.emit(&events.topics.buffering, true);
        report_load_start(app, events, start_at, autoplay);
        return;
    };
    deck.pending_load = None;

    // Stop current audio immediately; the new source arrives once the
    // background read completes.
    deck.replace_sink(&mixer, generation);

    deck.generation = deck.generation.wrapping_add(1);
    deck.current_id = Some(id);
    deck.current_path = Some(path.clone());
    deck.current_duration = duration;
    deck.cue = Resolved::default();
    deck.current_bytes = None;
    deck.seek_offset = 0.0;
    deck.active = false;
    deck.loading = true;
    deck.load_start = Some(Instant::now());
    let _ = app.emit(&events.topics.buffering, true);
    report_load_start(app, events, start_at, autoplay);

    let generation = deck.generation;
    let tx = load_tx.clone();
    if let Some(bytes) = cache.get(id) {
        // Cache hit: route the resident bytes through the same
        // completion path as a background read — no filesystem access.
        let _ = tx.send(LoadMsg {
            deck: deck_index,
            generation,
            id,
            duration,
            cue_points,
            start_at,
            autoplay,
            replay_gain,
            bytes: Ok(bytes),
        });
    } else {
        // Miss: read the whole file off the worker thread so a
        // slow/networked read never blocks transport commands. One read
        // is in flight per deck at a time — a newer `Load` bumps
        // `generation`, so a stale read's result is discarded rather
        // than another thread being blocked on.
        let backoffs = read_retry_backoffs.to_vec();
        thread::spawn(move || {
            // Retry transient failures with backoff; hangs are the
            // watchdog's job (handled in the worker loop, not here).
            let bytes = read_with_retry(|| read_file(&path), thread::sleep, &backoffs);
            let _ = tx.send(LoadMsg {
                deck: deck_index,
                generation,
                id,
                duration,
                cue_points,
                start_at,
                autoplay,
                replay_gain,
                bytes,
            });
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    app: &AppHandle,
    output: &mut Output,
    deck: &mut Deck,
    events: &DeckEvents,
    load_tx: &Sender<LoadMsg>,
    cache: &Arc<Cache>,
    read_retry_backoffs: &[Duration],
    deck_index: usize,
    cmd: Cmd,
) {
    if cancels_ramp(&cmd) {
        deck.cancel_ramp();
    }
    match cmd {
        Cmd::Load {
            id,
            path,
            duration,
            cue_points,
            start_at,
            autoplay,
            gain,
        } => {
            start_load(
                app,
                output,
                deck,
                events,
                load_tx,
                cache,
                read_retry_backoffs,
                deck_index,
                id,
                path,
                duration,
                cue_points,
                start_at,
                autoplay,
                gain,
            );
        }
        Cmd::Play => {
            // Play is also a self-heal trigger: re-open the output if a launch
            // failure left it closed. Drop silently if no device is available.
            let Some((mixer, generation)) = ensure_output(output, app, events) else {
                return;
            };
            deck.ensure_sink(&mixer, generation);
            if let Some(sink) = deck.sink.as_ref() {
                sink.play();
            }
            // Only report playing if there is (or will be) something to play.
            if deck.active || deck.loading {
                set_pause_state(app, events, false);
            }
        }
        Cmd::Pause => {
            // Nothing to pause without an open output; never open one just to pause.
            if let Some(sink) = deck.sink.as_ref() {
                sink.pause();
            }
            set_pause_state(app, events, true);
        }
        Cmd::Stop => stop_deck(app, output, deck, events),
        Cmd::Seek(s) => {
            // Air seconds in, absolute file position out — the caller neither
            // knows nor needs to know where the track's audio really starts.
            let target = deck.cue.file_pos(s);
            // Seek decodes from the in-RAM bytes — never re-reads the file.
            let Some(bytes) = deck.current_bytes.clone() else {
                return;
            };
            let Some((mixer, generation)) = ensure_output(output, app, events) else {
                return;
            };
            let was_paused = deck.sink.as_ref().is_some_and(|s| s.is_paused());
            deck.replace_sink(&mixer, generation);
            let Some(sink) = deck.sink.as_ref() else {
                return;
            };
            match decode_bytes(bytes) {
                Ok((source, _)) => {
                    append_span(
                        sink,
                        source,
                        Duration::from_secs_f64(target),
                        &deck.cue,
                        deck.replay_gain,
                    );
                    deck.seek_offset = target;
                    deck.active = true;
                    if was_paused {
                        sink.pause();
                    } else {
                        sink.play();
                    }
                }
                Err(e) => {
                    log::error!("player: seek decode failed: {}", e);
                    let _ = app.emit(&events.topics.error, format!("seek failed: {}", e));
                    deck.active = false;
                }
            }
        }
        Cmd::SetVolume(v) => {
            deck.volume = v.clamp(0.0, 1.0);
            // Remembered on the deck and applied when its sink next connects.
            // A running ramp is left alone: it scales whatever the operator
            // sets, so the two compose instead of racing.
            if let Some(sink) = deck.sink.as_ref() {
                sink.set_volume(deck.effective_volume());
            }
        }
        Cmd::Fade {
            to,
            ms,
            on_complete,
        } => {
            // A second fade restarts the ramp from the level reached so far,
            // so repeated presses never step back up.
            deck.start_ramp(to, ms, on_complete);
        }
        // Intercepted by the worker before dispatch; it acts on two decks.
        Cmd::HandOverNow { .. } => {}
    }
}

/// Which commands abandon a running ramp. Everything that changes what the deck
/// is doing does; the two that only re-aim it — a further fade, and the
/// operator's own volume, which a ramp multiplies — do not.
fn cancels_ramp(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Load { .. } | Cmd::Play | Cmd::Pause | Cmd::Stop | Cmd::Seek(_) => true,
        Cmd::SetVolume(_) | Cmd::Fade { .. } | Cmd::HandOverNow { .. } => false,
    }
}

/// Tear the deck down to nothing loaded. Shared by [`Cmd::Stop`] and by a ramp
/// that completes with [`RampDone::Stop`], which must land in exactly the same
/// state — the fade is only how the deck got quiet, not what happened to it.
fn stop_deck(app: &AppHandle, output: &mut Output, deck: &mut Deck, events: &DeckEvents) {
    if let Some((mixer, generation)) = output.current() {
        deck.replace_sink(&mixer, generation);
    }
    // Invalidate any in-flight read.
    deck.generation = deck.generation.wrapping_add(1);
    deck.reset();
    // Stop cancels a deferred load too — the user no longer wants it.
    output.clear_retry();
    let _ = app.emit(&events.topics.buffering, false);
    set_pause_state(app, events, true);
}

/// Handle a completed background read. Stale results (superseded by a newer
/// `Load`/`Stop`) are dropped.
fn apply_load(
    app: &AppHandle,
    output: &mut Output,
    deck: &mut Deck,
    events: &DeckEvents,
    msg: LoadMsg,
) {
    if msg.generation != deck.generation {
        return; // superseded
    }
    deck.loading = false;
    deck.load_start = None;
    let _ = app.emit(&events.topics.buffering, false);

    let bytes = match msg.bytes {
        Ok(b) => b,
        Err(e) => {
            let path = deck
                .current_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            // Retries were already exhausted inside the read thread.
            log::error!("player: read {} failed after retries: {}", path, e);
            let _ = app.emit(&events.topics.error, format!("read failed: {}", e));
            deck.reset();
            // Programmatic signal carrying the track id (human message above).
            let _ = app.emit(&events.topics.load_failed, msg.id);
            set_pause_state(app, events, true);
            return;
        }
    };

    match decode_bytes(bytes.clone()) {
        Ok((source, decoded_duration)) => {
            // The output should already be open (Load opened it), but re-open
            // defensively in case the device dropped while the read was in
            // flight. Treat an unopenable output as a load failure.
            let Some((mixer, generation)) = ensure_output(output, app, events) else {
                deck.reset();
                let _ = app.emit(&events.topics.load_failed, msg.id);
                set_pause_state(app, events, true);
                return;
            };
            deck.ensure_sink(&mixer, generation);
            let Some(sink) = deck.sink.as_ref() else {
                return;
            };
            // The decoded duration is the only trustworthy one — the tag value
            // is wrong on VBR MP3 — and cue points anchored to the file end
            // need it, so resolution happens here rather than at the caller.
            let final_duration = msg.duration.or(decoded_duration);
            let cue = msg.cue_points.resolve(final_duration);
            let air_duration = cue.air_duration();
            // The start position travels with the load rather than arriving as
            // a separate Seek: the bytes only exist here, so a Seek issued
            // alongside the Load would have found none and been dropped.
            let air_start = clamp_start(msg.start_at, air_duration);
            let start_at = cue.file_pos(air_start);
            append_span(
                sink,
                source,
                Duration::from_secs_f64(start_at),
                &cue,
                msg.replay_gain,
            );
            if msg.autoplay {
                sink.play();
            } else {
                sink.pause();
            }
            deck.current_bytes = Some(bytes);
            deck.current_duration = final_duration;
            deck.cue = cue;
            deck.replay_gain = msg.replay_gain;
            deck.seek_offset = start_at;
            deck.active = true;
            // Air time, so a trimmed track is simply a shorter track to every
            // listener of these events. `None` only when the file end is
            // unknown, which is the same case that emitted nothing before.
            if let Some(d) = air_duration {
                let _ = app.emit(&events.topics.duration, d);
            }
            let _ = app.emit(&events.topics.time, air_start);
            set_pause_state(app, events, !msg.autoplay);
        }
        Err(e) => {
            log::error!("player: decode failed: {}", e);
            let _ = app.emit(&events.topics.error, format!("decode failed: {}", e));
            deck.reset();
            set_pause_state(app, events, true);
        }
    }
}

/// Handle a watchdog timeout: abandon the detached read, emit the human error
/// plus a `:load-failed` carrying the track id, and reset load state. The
/// generation is bumped so a late `LoadMsg` from the abandoned thread is
/// discarded rather than played.
fn handle_load_timeout(app: &AppHandle, deck: &mut Deck, events: &DeckEvents, timeout: Duration) {
    let id = deck.current_id;
    let path = deck
        .current_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    log::error!(
        "player: read {} timed out after {:?}; abandoning read",
        path,
        timeout
    );
    deck.generation = deck.generation.wrapping_add(1);
    deck.reset();
    let _ = app.emit(&events.topics.buffering, false);
    let _ = app.emit(
        &events.topics.error,
        "network down: read timed out".to_string(),
    );
    if let Some(id) = id {
        let _ = app.emit(&events.topics.load_failed, id);
    }
    set_pause_state(app, events, true);
}

/// Decide whether an in-flight read has exceeded the watchdog budget. Pure
/// (given the clock via `now`) so it is unit-testable without threads or sleeps.
/// A read is timed out only while `loading` is true, a `load_start` is recorded,
/// and at least `READ_WATCHDOG_TIMEOUT` has elapsed. When a result has arrived
/// the worker sets `loading = false`, so this returns false.
fn watchdog_timed_out(
    loading: bool,
    load_start: Option<Instant>,
    now: Instant,
    timeout: Duration,
) -> bool {
    match load_start {
        Some(start) if loading => now.saturating_duration_since(start) >= timeout,
        _ => false,
    }
}

/// Move the `main` role from `main` to the armed deck `arm`, which starts
/// playing. The outgoing deck becomes the tail: still audible, no longer Now
/// playing.
///
/// `main-deck:duration` and `:time` are re-emitted for the incoming track right
/// here rather than left to the next tick, so the renderer flips in one go
/// instead of drawing the outgoing playhead over the incoming track.
fn hand_over(
    app: &AppHandle,
    decks: &mut [Deck],
    events: &[DeckEvents],
    topic: Option<&'static str>,
    main: usize,
    arm: usize,
) {
    let from = decks[main].current_id;
    let Some(to) = decks[arm].current_id else {
        return;
    };
    decks[main].role = DeckRole::Tail;
    decks[arm].role = DeckRole::Main;

    if let Some(sink) = decks[arm].sink.as_ref() {
        sink.play();
    }
    set_pause_state(app, &events[DeckRole::Tail as usize], false);

    let incoming = &decks[arm];
    let main_events = &events[DeckRole::Main as usize];
    if let Some(d) = incoming.cue.air_duration() {
        let _ = app.emit(&main_events.topics.duration, d);
    }
    let pos = incoming
        .sink
        .as_ref()
        .map(|s| s.get_pos().as_secs_f64())
        .unwrap_or(0.0);
    let _ = app.emit(
        &main_events.topics.time,
        incoming.cue.air_time(incoming.seek_offset + pos),
    );
    set_pause_state(app, main_events, false);

    log::info!("program bus: handover {:?} -> {}", from, to);
    if let Some(topic) = topic {
        let _ = app.emit(topic, Handover { from, to });
    }
}

/// Whether a completed ramp announces the track as ended, so the playlist
/// advances the way it does at the end of any track. Only ever on air: an
/// `EndTrack` ramp is the *Fade to next* fallback, and a tail ending would tell
/// the playlist a track finished that it already accounted for.
fn ends_the_track(done: RampDone, role: DeckRole) -> bool {
    done == RampDone::EndTrack && role == DeckRole::Main
}

/// Whether a completed ramp announces `program:faded-out`, which the playlist
/// answers by taking the track off air exactly as Stop does.
///
/// Only a fade to silence **on air** is that. A tail ramping down under an
/// incoming track is the second half of a handover the playlist has already
/// reconciled — announcing there would stop the track that just started.
///
/// Pure because the alternative is an untestable seam: the emit itself needs a
/// running Tauri app, while the rule about which completions announce is the
/// part that can actually be got wrong.
fn announces_faded_out(done: RampDone, role: DeckRole) -> bool {
    done == RampDone::Stop && role == DeckRole::Main
}

/// Whether a handover can be forced right now: something armed and decoded to
/// hand over *to*, and no tail already playing — a third audible track is not
/// something the bus is built to mix, and the engine never arms during an
/// overlap anyway, so this only ever declines what it should.
fn hand_over_now_allowed(arm_ready: bool, tail_present: bool) -> bool {
    arm_ready && !tail_present
}

/// Start the next item now and fade the outgoing track out underneath it: the
/// same role swap [`hand_over`] performs at `next_start`, fired on operator
/// command, with a ramp on the deck it leaves behind.
///
/// When it cannot overlap — nothing armed and decoded, or a tail already
/// playing — the outgoing track is faded out and *ended* instead, so the
/// playlist advances under the rules it applies at any other end of track. The
/// caller checks the same condition before sending, but it can go stale between
/// the check and this tick, and a transport button that silently does nothing
/// is worse than one that fades.
///
/// The engine needs no special case either way: it reconciles against
/// `program:handover`, or against the track ending.
fn hand_over_now(
    app: &AppHandle,
    decks: &mut [Deck],
    events: &[DeckEvents],
    topic: Option<&'static str>,
    fade_ms: u64,
) {
    let Some(m) = decks.iter().position(|d| d.role == DeckRole::Main) else {
        return;
    };
    let armed = decks.iter().position(|d| d.role == DeckRole::Arm);
    let tail_present = decks.iter().any(|d| d.role == DeckRole::Tail);
    let arm_ready = armed.is_some_and(|a| decks[a].active && !decks[a].loading);
    match armed.filter(|_| hand_over_now_allowed(arm_ready, tail_present)) {
        Some(arm) => {
            hand_over(app, decks, events, topic, m, arm);
            // `hand_over` has just made this deck the tail.
            decks[m].start_ramp(0.0, fade_ms, Some(RampDone::Stop));
        }
        None => decks[m].start_ramp(0.0, fade_ms, Some(RampDone::EndTrack)),
    }
}

/// Silence a tail deck and hand the slot back as the armed one. Used by every
/// explicit operator action that changes what is on air, and by the rule that a
/// tail dies with the track that displaced it.
fn cut_tail(app: &AppHandle, output: &Output, deck: &mut Deck, events: &DeckEvents) {
    if let Some((mixer, generation)) = output.current() {
        deck.replace_sink(&mixer, generation);
    }
    deck.generation = deck.generation.wrapping_add(1);
    deck.reset();
    deck.role = DeckRole::Arm;
    set_pause_state(app, events, true);
}

fn role_snapshot(decks: &[Deck]) -> Vec<RoleEntry> {
    decks
        .iter()
        .map(|d| RoleEntry {
            slot: d.slot,
            role: d.role,
            track_id: d.current_id,
        })
        .collect()
}

/// Drive every deck in `set` against one shared output until the command
/// channel closes.
pub(super) fn run(
    app: AppHandle,
    rx: Receiver<(DeckRole, Cmd)>,
    mut output: Output,
    set: DeckSet,
    cache: Arc<Cache>,
    tuning: PlayerTuning,
) -> Result<()> {
    let DeckSet {
        mut decks,
        events,
        roles_topic,
        handover_topic,
        faded_out_topic,
    } = set;
    // Completed background reads arrive here; `load_tx` is cloned per read.
    let (load_tx, load_rx) = channel::<LoadMsg>();
    let mut roles = role_snapshot(&decks);
    if let Some(topic) = roles_topic {
        let _ = app.emit(topic, &roles);
    }

    loop {
        loop {
            match rx.try_recv() {
                Ok((role, cmd)) => {
                    // The one command that acts on two decks, so it never goes
                    // through `apply`. Declined when nothing is armed and
                    // decoded or a tail is already playing — the caller falls
                    // back to a fade-out plus a plain next.
                    if let Cmd::HandOverNow { fade_ms } = cmd {
                        hand_over_now(&app, &mut decks, &events, handover_topic, fade_ms);
                        continue;
                    }
                    let Some(i) = decks.iter().position(|d| d.role == role) else {
                        continue;
                    };
                    let role_index = decks[i].role as usize;
                    let companion = (role == DeckRole::Main)
                        .then(|| tail_companion(&cmd))
                        .flatten();
                    // A fade aimed at `main` takes any tail with it: fading one
                    // of two audible tracks to silence is not what the operator
                    // asked for. The tail's own ramp ends in a stop, and the
                    // vacate rule below hands the slot back as the armed one.
                    let fade_tail = match (role, &cmd) {
                        (DeckRole::Main, Cmd::Fade { to, ms, .. }) => Some((*to, *ms)),
                        _ => None,
                    };
                    apply(
                        &app,
                        &mut output,
                        &mut decks[i],
                        &events[role_index],
                        &load_tx,
                        &cache,
                        &tuning.read_retry_backoffs,
                        i,
                        cmd,
                    );
                    if let Some(action) = companion {
                        if let Some(t) = decks.iter().position(|d| d.role == DeckRole::Tail) {
                            let tail_events = &events[DeckRole::Tail as usize];
                            match action {
                                TailAction::Cut => {
                                    cut_tail(&app, &output, &mut decks[t], tail_events)
                                }
                                TailAction::Pause => {
                                    if let Some(sink) = decks[t].sink.as_ref() {
                                        sink.pause();
                                    }
                                    set_pause_state(&app, tail_events, true);
                                }
                                TailAction::Resume => {
                                    if let Some(sink) = decks[t].sink.as_ref() {
                                        sink.play();
                                    }
                                    set_pause_state(&app, tail_events, false);
                                }
                            }
                        }
                    }
                    if let Some((to, ms)) = fade_tail {
                        if let Some(t) = decks.iter().position(|d| d.role == DeckRole::Tail) {
                            decks[t].start_ramp(to, ms, Some(RampDone::Stop));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Drain any completed background reads, back to the deck that issued them.
        while let Ok(msg) = load_rx.try_recv() {
            let Some(deck) = decks.get_mut(msg.deck) else {
                continue;
            };
            let role_index = deck.role as usize;
            apply_load(&app, &mut output, deck, &events[role_index], msg);
        }

        // Watchdog: a read that neither completed nor errored within the budget
        // is a wedged mount. Declare a timeout and abandon the detached read
        // thread — the worker never blocks waiting on it.
        let now = Instant::now();
        for deck in decks.iter_mut() {
            if watchdog_timed_out(
                deck.loading,
                deck.load_start,
                now,
                tuning.read_watchdog_timeout,
            ) {
                let role_index = deck.role as usize;
                handle_load_timeout(
                    &app,
                    deck,
                    &events[role_index],
                    tuning.read_watchdog_timeout,
                );
            }
        }

        // Idle auto-retry: a load deferred because the audio device could not be
        // opened waits here. Retry the open on a timer (quietly — the initial
        // failure already surfaced an error), and replay the loads the moment an
        // output becomes available, whether reopened here or by a command above
        // (#259). No user action required.
        if let Some(waiting) = decks.iter().position(|d| d.pending_load.is_some()) {
            if !output.is_open() && output.retry_due(Instant::now()) {
                output.mark_retry_now();
                let role_index = decks[waiting].role as usize;
                match output.open() {
                    Ok(()) => report_output(&mut output, &app, &events[role_index], true),
                    // Quiet: the initial failure already reported unavailable.
                    Err(e) => log::debug!("player: output reopen retry failed: {}", e),
                }
            }
            if output.is_open() {
                for (i, deck) in decks.iter_mut().enumerate() {
                    let Some(p) = deck.pending_load.take() else {
                        continue;
                    };
                    log::info!("player: audio output available; resuming deferred load");
                    let role_index = deck.role as usize;
                    start_load(
                        &app,
                        &mut output,
                        deck,
                        &events[role_index],
                        &load_tx,
                        &cache,
                        &tuning.read_retry_backoffs,
                        i,
                        p.id,
                        p.path,
                        p.duration,
                        p.cue_points,
                        p.start_at,
                        p.autoplay,
                        p.replay_gain,
                    );
                }
            }
        }

        // Step any live fade. A ramp that ends in a stop tears its deck down
        // here rather than in the dispatch loop, so a fade-out lands in exactly
        // the state an immediate `Stop` would have left.
        let mut main_ended = false;
        for deck in decks.iter_mut() {
            let Some(on_complete) = deck.step_ramp(now) else {
                continue;
            };
            let Some(done) = on_complete else { continue };
            let role_index = deck.role as usize;
            let announce = announces_faded_out(done, deck.role);
            if ends_the_track(done, deck.role) {
                let _ = app.emit(&events[role_index].topics.ended, ());
                main_ended = true;
            }
            stop_deck(&app, &mut output, deck, &events[role_index]);
            if announce {
                if let Some(topic) = faded_out_topic {
                    let _ = app.emit(topic, ());
                }
            }
        }

        // Handover, checked before ended detection so a track whose next start
        // is its cue out segues rather than hard-cutting: the two paths must
        // never both advance the playlist.
        if let Some(m) = decks.iter().position(|d| d.role == DeckRole::Main) {
            let pos = decks[m]
                .sink
                .as_ref()
                .map(|s| decks[m].seek_offset + s.get_pos().as_secs_f64());
            let armed = decks.iter().position(|d| d.role == DeckRole::Arm);
            if let (Some(pos), Some(arm)) = (pos, armed) {
                if handover_due(
                    pos,
                    decks[m].cue.next_start,
                    decks[m].active && !decks[m].loading,
                    decks[arm].active && !decks[arm].loading,
                ) {
                    hand_over(&app, &mut decks, &events, handover_topic, m, arm);
                }
            }
        }

        // Time + ended detection are only meaningful with a live sink.
        for deck in decks.iter_mut() {
            let Some((pos, empty)) = deck
                .sink
                .as_ref()
                .map(|s| (s.get_pos().as_secs_f64(), s.empty()))
            else {
                continue;
            };
            let role_index = deck.role as usize;
            let events = &events[role_index];

            if deck.active && deck.last_time_emit.elapsed() >= TIME_EMIT_INTERVAL {
                deck.last_time_emit = Instant::now();
                // Air time: `0` is the first audible sample, not the first
                // sample in the file.
                let air = deck.cue.air_time(deck.seek_offset + pos);
                let _ = app.emit(&events.topics.time, air);
            }

            if deck.active && !deck.loading && empty {
                deck.active = false;
                set_pause_state(&app, events, true);
                let _ = app.emit(&events.topics.ended, ());
                main_ended |= deck.role == DeckRole::Main;
            }
        }

        // A tail is vacated at its own cue out, or when the deck that took over
        // from it ends, whichever comes first: a tail belongs to the track that
        // displaced it and dies with it, so at most two tracks are ever
        // audible. The freed slot becomes the armed one, which is what lets the
        // playlist load the following item onto it.
        if let Some(t) = decks.iter().position(|d| d.role == DeckRole::Tail) {
            if main_ended {
                cut_tail(
                    &app,
                    &output,
                    &mut decks[t],
                    &events[DeckRole::Tail as usize],
                );
            } else if !decks[t].active && !decks[t].loading {
                decks[t].reset();
                decks[t].role = DeckRole::Arm;
            }
        }

        if let Some(topic) = roles_topic {
            let next = role_snapshot(&decks);
            if next != roles {
                roles = next;
                let _ = app.emit(topic, &roles);
            }
        }

        thread::sleep(TICK_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::player::READ_WATCHDOG_TIMEOUT;

    /// The worker only applies a background read if its generation still matches
    /// the latest intent. This mirrors the guard in `apply_load`.
    #[test]
    fn stale_generation_is_discarded() {
        let latest = 5u64;
        let stale = LoadMsg {
            deck: 0,
            generation: 4,
            id: 1,
            duration: None,
            cue_points: CuePoints::default(),
            start_at: 0.0,
            autoplay: true,
            replay_gain: 1.0,
            bytes: Ok(Arc::from(Vec::new().into_boxed_slice())),
        };
        let fresh = LoadMsg {
            deck: 0,
            generation: 5,
            id: 1,
            duration: None,
            cue_points: CuePoints::default(),
            start_at: 0.0,
            autoplay: true,
            replay_gain: 1.0,
            bytes: Ok(Arc::from(Vec::new().into_boxed_slice())),
        };
        assert_ne!(stale.generation, latest);
        assert_eq!(fresh.generation, latest);
    }

    #[test]
    fn watchdog_times_out_only_after_budget_while_loading() {
        let start = Instant::now();
        let before = start
            .checked_add(READ_WATCHDOG_TIMEOUT - Duration::from_millis(1))
            .unwrap();
        let after = start
            .checked_add(READ_WATCHDOG_TIMEOUT + Duration::from_millis(1))
            .unwrap();

        // Under budget: not timed out.
        assert!(!watchdog_timed_out(
            true,
            Some(start),
            before,
            READ_WATCHDOG_TIMEOUT
        ));
        // Over budget while loading: timed out.
        assert!(watchdog_timed_out(
            true,
            Some(start),
            after,
            READ_WATCHDOG_TIMEOUT
        ));
        // A result arrived (loading == false): never a timeout.
        assert!(!watchdog_timed_out(
            false,
            Some(start),
            after,
            READ_WATCHDOG_TIMEOUT
        ));
        // No read in flight: never a timeout.
        assert!(!watchdog_timed_out(
            true,
            None,
            after,
            READ_WATCHDOG_TIMEOUT
        ));
    }

    /// A read that completes routes back to the deck that issued it, not to
    /// whichever deck the worker happens to look at first.
    #[test]
    fn a_completed_read_is_addressed_to_its_own_deck() {
        let msg = LoadMsg {
            deck: 1,
            generation: 1,
            id: 7,
            duration: None,
            cue_points: CuePoints::default(),
            start_at: 0.0,
            autoplay: true,
            replay_gain: 1.0,
            bytes: Ok(Arc::from(Vec::new().into_boxed_slice())),
        };
        let decks = [
            Deck::new(DeckSlot::A, DeckRole::Main),
            Deck::new(DeckSlot::B, DeckRole::Arm),
        ];
        assert_eq!(decks[msg.deck].slot, DeckSlot::B);
    }

    /// v1 holds roles static: A is main, B is armed and idle. The snapshot is
    /// what `program:roles` carries.
    #[test]
    fn the_role_snapshot_names_both_slots() {
        let decks = [
            Deck::new(DeckSlot::A, DeckRole::Main),
            Deck::new(DeckSlot::B, DeckRole::Arm),
        ];
        let roles = role_snapshot(&decks);
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].slot, DeckSlot::A);
        assert_eq!(roles[0].role, DeckRole::Main);
        assert_eq!(roles[0].track_id, None);
        assert_eq!(roles[1].slot, DeckSlot::B);
        assert_eq!(roles[1].role, DeckRole::Arm);
    }

    /// The trigger: the main deck reaching the outgoing track's next start,
    /// with a deck armed and decoded to hand over to.
    #[test]
    fn handover_is_due_at_next_start_and_not_before() {
        assert!(!handover_due(9.9, Some(10.0), true, true));
        assert!(handover_due(10.0, Some(10.0), true, true));
        assert!(handover_due(10.1, Some(10.0), true, true));
    }

    /// A track nobody prepped has no next start of its own: it resolves to cue
    /// out, so the tick fires as the sink empties and the result is the hard cut
    /// it has always been. `None` reaches the worker only before a load has
    /// resolved, and there is nothing to hand over then.
    #[test]
    fn nothing_is_due_without_a_resolved_next_start() {
        assert!(!handover_due(500.0, None, true, true));
    }

    /// Never mid-load or off air: an arm deck still reading its file, or a main
    /// deck that is not playing, has nothing to hand over with.
    #[test]
    fn handover_waits_for_both_decks_to_be_ready() {
        assert!(!handover_due(10.0, Some(10.0), true, false));
        assert!(!handover_due(10.0, Some(10.0), false, true));
    }

    /// Any explicit change to what is on air cuts the tail with it; seek and
    /// volume act on the incoming track alone.
    #[test]
    fn a_command_to_main_carries_the_tail_with_it() {
        assert_eq!(tail_companion(&Cmd::Stop), Some(TailAction::Cut));
        assert_eq!(
            tail_companion(&Cmd::Load {
                id: 1,
                path: PathBuf::new(),
                duration: None,
                cue_points: CuePoints::default(),
                start_at: 0.0,
                autoplay: true,
                gain: 1.0,
            }),
            Some(TailAction::Cut)
        );
        assert_eq!(tail_companion(&Cmd::Pause), Some(TailAction::Pause));
        assert_eq!(tail_companion(&Cmd::Play), Some(TailAction::Resume));
        assert_eq!(tail_companion(&Cmd::Seek(12.0)), None);
        assert_eq!(tail_companion(&Cmd::SetVolume(0.5)), None);
    }

    fn fade(ms: u64) -> Cmd {
        Cmd::Fade {
            to: 0.0,
            ms,
            on_complete: Some(RampDone::Stop),
        }
    }

    /// A fade must not cut the tail when it starts: the tail is ramped
    /// alongside and torn down when the ramp completes, so the two stay
    /// audible together for the length of the fade.
    #[test]
    fn a_fade_does_not_cut_the_tail_up_front() {
        assert_eq!(tail_companion(&fade(3000)), None);
        assert_eq!(tail_companion(&Cmd::HandOverNow { fade_ms: 3000 }), None);
    }

    #[test]
    fn ramp_interpolates_linearly_between_its_endpoints() {
        assert_eq!(ramp_gain(1.0, 0.0, Duration::ZERO, 1000).0, 1.0);
        assert_eq!(
            ramp_gain(1.0, 0.0, Duration::from_millis(250), 1000).0,
            0.75
        );
        assert_eq!(ramp_gain(1.0, 0.0, Duration::from_millis(500), 1000).0, 0.5);
        // A ramp starting part-way down — a second press mid-fade — carries on
        // from where it was rather than stepping back up.
        assert_eq!(
            ramp_gain(0.5, 0.0, Duration::from_millis(500), 1000).0,
            0.25
        );
    }

    #[test]
    fn ramp_completes_exactly_on_its_target() {
        let (gain, done) = ramp_gain(1.0, 0.0, Duration::from_millis(1000), 1000);
        assert_eq!(gain, 0.0);
        assert!(done);
        // Overshoot — a tick that lands late — never runs past the target.
        let (gain, done) = ramp_gain(1.0, 0.0, Duration::from_secs(30), 1000);
        assert_eq!(gain, 0.0);
        assert!(done);
    }

    /// A zero-length fade is a cut, not a division by zero.
    #[test]
    fn a_zero_length_ramp_lands_immediately() {
        let (gain, done) = ramp_gain(1.0, 0.0, Duration::ZERO, 0);
        assert_eq!(gain, 0.0);
        assert!(done);
    }

    /// Anything that changes what the deck is doing abandons the ramp; the two
    /// that only re-aim it do not. Without this a faded-out deck could be left
    /// quiet for the track that follows.
    #[test]
    fn transport_commands_cancel_a_ramp() {
        assert!(cancels_ramp(&Cmd::Play));
        assert!(cancels_ramp(&Cmd::Pause));
        assert!(cancels_ramp(&Cmd::Stop));
        assert!(cancels_ramp(&Cmd::Seek(3.0)));
        assert!(cancels_ramp(&Cmd::Load {
            id: 1,
            path: PathBuf::new(),
            duration: None,
            cue_points: CuePoints::default(),
            start_at: 0.0,
            autoplay: true,
            gain: 1.0,
        }));
        assert!(!cancels_ramp(&Cmd::SetVolume(0.5)));
        assert!(!cancels_ramp(&fade(3000)));
    }

    /// The ramp scales the operator's volume rather than replacing it, so a
    /// sink rebuilt mid-fade resumes at the faded level.
    #[test]
    fn the_ramp_multiplies_the_deck_volume() {
        let mut deck = Deck::new(DeckSlot::A, DeckRole::Main);
        deck.volume = 0.5;
        assert_eq!(deck.effective_volume(), 0.5);
        deck.gain = 0.5;
        assert_eq!(deck.effective_volume(), 0.25);
        deck.cancel_ramp();
        assert_eq!(deck.effective_volume(), 0.5, "volume survives the fade");
    }

    #[test]
    fn stepping_a_ramp_reports_completion_once() {
        let mut deck = Deck::new(DeckSlot::A, DeckRole::Main);
        deck.start_ramp(0.0, 0, Some(RampDone::Stop));
        assert_eq!(deck.step_ramp(Instant::now()), Some(Some(RampDone::Stop)));
        assert_eq!(deck.gain, 0.0);
        assert!(deck.step_ramp(Instant::now()).is_none(), "ramp is spent");
    }

    /// The rule behind `program:faded-out`. A fade to silence on air is a Stop
    /// the operator asked for slowly, and the playlist has to hear about it or
    /// it goes on believing a silent deck is playing — which is what left the
    /// Play button dead after a fade-out.
    #[test]
    fn a_fade_to_silence_on_air_announces_itself() {
        assert!(announces_faded_out(RampDone::Stop, DeckRole::Main));
    }

    /// The tail of a *Fade to next* ends in a stop too, but the playlist has
    /// already reconciled that handover: announcing would stop the track that
    /// just started.
    #[test]
    fn a_tail_fading_out_under_the_next_track_announces_nothing() {
        assert!(!announces_faded_out(RampDone::Stop, DeckRole::Tail));
        assert!(!announces_faded_out(RampDone::Stop, DeckRole::Arm));
    }

    /// The two completions are answered differently and must not be confused:
    /// one takes the track off air, the other ends it so the playlist advances.
    #[test]
    fn ending_a_track_and_fading_it_out_are_distinct() {
        assert!(!announces_faded_out(RampDone::EndTrack, DeckRole::Main));
        assert!(ends_the_track(RampDone::EndTrack, DeckRole::Main));
        assert!(!ends_the_track(RampDone::Stop, DeckRole::Main));
        assert!(!ends_the_track(RampDone::EndTrack, DeckRole::Tail));
    }

    /// Forcing a handover needs something decoded to hand over *to*, and must
    /// refuse while a tail is playing: a third audible track is not something
    /// the bus is built to mix.
    #[test]
    fn a_forced_handover_needs_an_armed_deck_and_no_tail() {
        assert!(hand_over_now_allowed(true, false));
        assert!(!hand_over_now_allowed(false, false));
        assert!(!hand_over_now_allowed(true, true));
    }

    /// Roles address events, so a third `DeckEvents` has to exist for the
    /// outgoing deck — `events[DeckRole::Tail as usize]` is indexed directly.
    #[test]
    fn every_role_addresses_its_own_topics() {
        let events = [
            DeckEvents::new("main-deck"),
            DeckEvents::new("arm-deck"),
            DeckEvents::new("tail-deck"),
        ];
        assert_eq!(
            events[DeckRole::Main as usize].topics.ended,
            "main-deck:ended"
        );
        assert_eq!(
            events[DeckRole::Arm as usize].topics.ended,
            "arm-deck:ended"
        );
        assert_eq!(
            events[DeckRole::Tail as usize].topics.ended,
            "tail-deck:ended"
        );
    }

    /// Commands are routed by role, so `main_deck_*` reaches whichever slot
    /// holds `main` rather than a fixed deck.
    #[test]
    fn a_command_routes_to_the_deck_holding_the_role() {
        let decks = [
            Deck::new(DeckSlot::A, DeckRole::Arm),
            Deck::new(DeckSlot::B, DeckRole::Main),
        ];
        let main = decks
            .iter()
            .position(|d| d.role == DeckRole::Main)
            .expect("a deck holds main");
        assert_eq!(decks[main].slot, DeckSlot::B);
    }
}
