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
    append_span, clamp_start, decode_bytes, fresh_cancel, fresh_progress, Bytes, Cancel, Cmd,
    PlayerTuning, Progress, RampDone, Seen, Topics,
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
    bound_dead_air: bool,
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
    /// What the in-flight read has delivered so far, published by the reading
    /// thread. `None` whenever no read of our own is pending — a cache hit
    /// routes resident bytes with nothing to read.
    read_progress: Option<Progress>,
    /// Tells the in-flight read's thread to stop *waiting* for another reader's
    /// copy. Set whenever the load it was issued for is given up on, because a
    /// waiter has no other way to hear about it.
    read_cancel: Option<Cancel>,
    /// The last progress the worker loop saw of the in-flight read; drives the
    /// watchdog, which fires on a stall rather than on the read's age. `None`
    /// whenever no read is pending.
    load_progress: Option<Seen>,
    /// When the pending load was issued. The watchdog does not use it — a read
    /// still delivering is never too old — but the dead-air limit does, because
    /// what it bounds is how long air has been silent.
    load_issued: Option<Instant>,
    /// Whether the pending load puts the dead-air limit on the clock: it asked
    /// to play *and* the playlist issued it. A load parked silent (a session
    /// restore) is not dead air however long it takes, and one an operator
    /// started by hand is their call to make — see
    /// [`Cmd::Load::bound_dead_air`].
    load_bounds_air: bool,
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
            read_progress: None,
            read_cancel: None,
            load_progress: None,
            load_issued: None,
            load_bounds_air: false,
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

    /// Fold the in-flight read's published byte count into `load_progress`, so
    /// the watchdog measures the current stall rather than the read's age.
    ///
    /// A read with no publisher (a cache hit) keeps the stamp it was issued
    /// with: there is nothing to deliver, and a hit that never completes should
    /// still time out.
    fn note_read_progress(&mut self, now: Instant) {
        let Some(progress) = self.read_progress.as_ref() else {
            return;
        };
        let bytes = progress.load(Ordering::Relaxed);
        if let Some(seen) = self.load_progress {
            self.load_progress = Some(seen.observe(bytes, now));
        }
    }

    /// Whether this deck is putting sound on the bus *right now*. `active`
    /// alone is not that question: a deck preloaded for a handover is active
    /// with its sink paused, and counting it as sound would switch the dead-air
    /// limit off for the whole steady state.
    fn audible(&self) -> bool {
        self.active && self.sink.as_ref().is_some_and(|s| !s.is_paused())
    }

    /// Give up on the in-flight read, as far as a detached thread can be given
    /// up on: a waiter stops waiting, while a read of our own runs to the end
    /// and leaves its bytes in the cache.
    fn cancel_read(&mut self) {
        if let Some(cancel) = self.read_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the in-flight read has delivered anything at all — the
    /// difference between a share that is slow and one that is not answering.
    fn read_delivered(&self) -> bool {
        self.load_progress.is_some_and(|seen| seen.bytes > 0)
    }

    /// Drop everything about the loaded track. Shared by `Stop` and by every
    /// failure path, which want exactly the same end state: nothing loaded,
    /// nothing in flight, no deferred load waiting on a device.
    fn reset(&mut self) {
        self.cancel_read();
        self.active = false;
        self.loading = false;
        self.read_progress = None;
        self.load_progress = None;
        self.load_issued = None;
        self.load_bounds_air = false;
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
    /// Whether this set is program audio. Only then is a silent deck dead air,
    /// which is what the dead-air limit is measured against; the cue deck
    /// monitors and may take as long as the share needs.
    pub on_air: bool,
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
    tuning: &PlayerTuning,
    deck_index: usize,
    id: i64,
    path: PathBuf,
    duration: Option<f64>,
    cue_points: CuePoints,
    start_at: f64,
    autoplay: bool,
    bound_dead_air: bool,
    replay_gain: f32,
) {
    let Some((mixer, generation)) = ensure_output(output, app, events) else {
        // Abandon whatever the *previous* track's read is doing: it is for a
        // track nothing will play now, and while the watchdog timed every read
        // out it was abandoned within the budget anyway. Left at the current
        // generation, a read that keeps delivering would land in `apply_load`
        // under this deck's new `current_id` and report the wrong track — and
        // the reset there would drop the load we are about to defer.
        deck.generation = deck.generation.wrapping_add(1);
        deck.loading = false;
        deck.cancel_read();
        deck.read_progress = None;
        deck.load_progress = None;
        deck.load_issued = None;

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
            bound_dead_air,
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
    // Whatever the last read is still waiting for, it is waiting for a track
    // this deck is no longer going to play.
    deck.cancel_read();
    deck.current_id = Some(id);
    deck.current_path = Some(path.clone());
    deck.current_duration = duration;
    deck.cue = Resolved::default();
    deck.current_bytes = None;
    deck.seek_offset = 0.0;
    deck.active = false;
    deck.loading = true;
    let issued = Instant::now();
    deck.load_progress = Some(Seen::issued(issued));
    deck.load_issued = Some(issued);
    deck.load_bounds_air = autoplay && bound_dead_air;
    let _ = app.emit(&events.topics.buffering, true);
    report_load_start(app, events, start_at, autoplay);

    let generation = deck.generation;
    let tx = load_tx.clone();
    if let Some(bytes) = cache.get(id) {
        // Nothing reads, so nothing publishes progress — and the watchdog still
        // holds here, on a `Seen` that never moves.
        deck.read_progress = None;
        deck.read_cancel = None;
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
        //
        // The cache does the reading, so the bytes become resident and a
        // file the prefetch worker is already pulling is waited for rather
        // than fetched a second time. Offering them here rather than in
        // `apply_load` is deliberate: that runs on the worker thread, which
        // has no business taking a cache lock, and it discards a superseded
        // result — whose bytes are still the right bytes for this track.
        let backoffs = tuning.read_retry_backoffs.clone();
        let stall_budget = tuning.read_watchdog_timeout;
        let cache = Arc::clone(cache);
        let progress = fresh_progress();
        deck.read_progress = Some(Arc::clone(&progress));
        let cancel = fresh_cancel();
        deck.read_cancel = Some(Arc::clone(&cancel));
        thread::spawn(move || {
            // Retry transient failures with backoff; hangs are the
            // watchdog's job (handled in the worker loop, not here). The
            // budget bounds the wait for another reader's copy, on the same
            // stall rule the watchdog applies here.
            let bytes = cache.read_for_deck(id, &path, &backoffs, stall_budget, &progress, &cancel);
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
    tuning: &PlayerTuning,
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
            bound_dead_air,
            gain,
        } => {
            start_load(
                app,
                output,
                deck,
                events,
                load_tx,
                cache,
                tuning,
                deck_index,
                id,
                path,
                duration,
                cue_points,
                start_at,
                autoplay,
                bound_dead_air,
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
    deck.read_progress = None;
    deck.load_progress = None;
    deck.load_issued = None;
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
            // Cue points anchored to the file end need a length, so resolution
            // happens here rather than at the caller. The row's duration comes
            // first because once the analysis pass has reached the track it is
            // that pass's sample count — exact, where this decoder's own answer
            // is whatever the container claims. The fallback is for a row the
            // pass has not reached *and* whose tags carried no length at all;
            // a row that holds the tag's guess still wins here, because nothing
            // on the row says which of the two it is.
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
            // The bytes reached the deck: the counterpart of `:load-failed`,
            // and what tells the playlist a track actually went on rather than
            // merely having been asked for.
            let _ = app.emit(&events.topics.loaded, msg.id);
        }
        Err(e) => {
            log::error!("player: decode failed: {}", e);
            let _ = app.emit(&events.topics.error, format!("decode failed: {}", e));
            deck.reset();
            set_pause_state(app, events, true);
        }
    }
}

/// Why a load was given up on. Every answer is the same to everything
/// downstream — a `:load-failed` the playlist turns into skip-to-cached — and
/// they differ only in what the operator is told, which is the difference
/// between "the share is gone" and "the share is too slow to open a show with".
///
/// Which bound expired is not that difference on its own: the dead-air limit is
/// the shorter of the two, so on a silent on-air deck it is what fires even
/// when the mount is dead. What the operator is told therefore follows what the
/// read has *delivered*, which is the question they would ask next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Abandoned {
    /// Nothing arrived for the watchdog budget: a wedged mount.
    Stalled,
    /// Air has been silent for the dead-air limit with a read still pending.
    /// `delivered` is whether any bytes had arrived by then.
    DeadAir { delivered: bool },
}

/// Give up on an in-flight load: abandon the detached read, emit the human
/// error plus a `:load-failed` carrying the track id, and reset load state. The
/// generation is bumped so a late `LoadMsg` from the abandoned thread is
/// discarded rather than played.
///
/// The read itself runs on — a blocked `read()` cannot be cancelled, and one
/// that is merely slow is worth finishing: `Cache::read_for_deck` offers its
/// bytes to the cache, so the track the playlist just skipped is resident for
/// whoever plays it next.
fn abandon_load(
    app: &AppHandle,
    deck: &mut Deck,
    events: &DeckEvents,
    why: Abandoned,
    after: Duration,
) {
    let id = deck.current_id;
    let path = deck
        .current_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let message = match why {
        Abandoned::Stalled => {
            log::error!(
                "player: read {} delivered nothing for {:?}; abandoning read",
                path,
                after
            );
            "network down: read timed out"
        }
        Abandoned::DeadAir { delivered: true } => {
            log::warn!(
                "player: read {} still arriving after {:?} of silence; skipping ahead",
                path,
                after
            );
            "network slow: gave up the load to keep air"
        }
        // Three seconds is too early to call the mount dead — the share may
        // simply be slow to open a file — so the wording stops at what is
        // known. The watchdog says the rest if the read never starts arriving.
        Abandoned::DeadAir { delivered: false } => {
            log::error!(
                "player: read {} delivered nothing in {:?} of silence; skipping ahead",
                path,
                after
            );
            "network not responding: gave up the load to keep air"
        }
    };
    deck.generation = deck.generation.wrapping_add(1);
    deck.reset();
    let _ = app.emit(&events.topics.buffering, false);
    let _ = app.emit(&events.topics.error, message.to_string());
    if let Some(id) = id {
        let _ = app.emit(&events.topics.load_failed, id);
    }
    set_pause_state(app, events, true);
}

/// Decide whether a load has kept air silent past the dead-air limit.
///
/// Pure (given the clock via `now`), like [`watchdog_timed_out`], and a
/// different question from it: this one does not care whether bytes are
/// arriving, only that nothing is audible while they do. It asks for four
/// things at once, because each one of them is a reason there is no dead air to
/// bound — the deck is not program audio, nobody is owed air by this load
/// (`bounds_air`: it was parked rather than played, or an operator chose the
/// track themselves), some deck is still audible, or no load is pending at all.
fn dead_air_expired(
    on_air: bool,
    bounds_air: bool,
    anything_audible: bool,
    load_issued: Option<Instant>,
    now: Instant,
    limit: Duration,
) -> bool {
    if !on_air || !bounds_air || anything_audible {
        return false;
    }
    load_issued.is_some_and(|at| now.saturating_duration_since(at) >= limit)
}

/// Decide whether an in-flight read has **stalled** past the watchdog budget.
/// Pure (given the clock via `now`) so it is unit-testable without threads or
/// sleeps.
///
/// A read is timed out only while `loading` is true, a `load_progress` is
/// recorded, and nothing has arrived for at least `READ_WATCHDOG_TIMEOUT`. When
/// a result has arrived the worker sets `loading = false`, so this returns
/// false. Why the budget bounds the stall and never the read:
/// `docs/audio.md`, *Whole-file reads*.
fn watchdog_timed_out(
    loading: bool,
    load_progress: Option<Seen>,
    now: Instant,
    timeout: Duration,
) -> bool {
    match load_progress {
        Some(seen) if loading => seen.stalled(now, timeout),
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
        on_air,
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
                        &tuning,
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

        // Watchdog: a read that has delivered nothing for the budget is a
        // wedged mount. Declare a timeout and abandon the detached read thread
        // — the worker never blocks waiting on it. Note the reader's progress
        // first, so a read that is merely slow keeps its budget refreshed.
        //
        // The dead-air limit is the second, much shorter bound, and the reason
        // the first one can afford to be patient: a read that keeps arriving is
        // never a wedged mount, but air that keeps being silent is still a show
        // with nothing on it.
        let now = Instant::now();
        let anything_audible = decks.iter().any(Deck::audible);
        for deck in decks.iter_mut() {
            deck.note_read_progress(now);
            let abandon = if watchdog_timed_out(
                deck.loading,
                deck.load_progress,
                now,
                tuning.read_watchdog_timeout,
            ) {
                Some((Abandoned::Stalled, tuning.read_watchdog_timeout))
            } else if deck.loading
                && dead_air_expired(
                    on_air && deck.role == DeckRole::Main,
                    deck.load_bounds_air,
                    anything_audible,
                    deck.load_issued,
                    now,
                    tuning.dead_air_limit,
                )
            {
                Some((
                    Abandoned::DeadAir {
                        delivered: deck.read_delivered(),
                    },
                    tuning.dead_air_limit,
                ))
            } else {
                None
            };
            if let Some((why, after)) = abandon {
                let role_index = deck.role as usize;
                abandon_load(&app, deck, &events[role_index], why, after);
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
                        &tuning,
                        i,
                        p.id,
                        p.path,
                        p.duration,
                        p.cue_points,
                        p.start_at,
                        p.autoplay,
                        p.bound_dead_air,
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
    fn watchdog_times_out_only_after_a_stall_while_loading() {
        let issued = Instant::now();
        let before = issued
            .checked_add(READ_WATCHDOG_TIMEOUT - Duration::from_millis(1))
            .unwrap();
        let after = issued
            .checked_add(READ_WATCHDOG_TIMEOUT + Duration::from_millis(1))
            .unwrap();
        let seen = Seen::issued(issued);

        // Nothing has arrived, but the budget has not run out.
        assert!(!watchdog_timed_out(
            true,
            Some(seen),
            before,
            READ_WATCHDOG_TIMEOUT
        ));
        // Nothing has arrived for the whole budget: a wedged mount.
        assert!(watchdog_timed_out(
            true,
            Some(seen),
            after,
            READ_WATCHDOG_TIMEOUT
        ));
        // A result arrived (loading == false): never a timeout.
        assert!(!watchdog_timed_out(
            false,
            Some(seen),
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

    /// The other half of #504: a read that keeps arriving keeps its load, but
    /// not while that means an empty transmitter. The dead-air limit is the
    /// short bound, and it does not care about progress at all.
    #[test]
    fn dead_air_is_bounded_only_where_there_is_air_to_lose() {
        let issued = Instant::now();
        let limit = Duration::from_secs(3);
        let after = issued.checked_add(limit).unwrap();
        let before = issued
            .checked_add(limit - Duration::from_millis(1))
            .unwrap();

        // On air, asked to play, nothing audible: silence has a bound.
        assert!(dead_air_expired(
            true,
            true,
            false,
            Some(issued),
            after,
            limit
        ));
        assert!(!dead_air_expired(
            true,
            true,
            false,
            Some(issued),
            before,
            limit
        ));
        // A deck that is audible, a load nobody is owed air by (parked by a
        // session restore, or a track an operator chose themselves) and the cue
        // deck's audition are each a reason there is no dead air.
        assert!(!dead_air_expired(
            true,
            true,
            true,
            Some(issued),
            after,
            limit
        ));
        assert!(!dead_air_expired(
            true,
            false,
            false,
            Some(issued),
            after,
            limit
        ));
        assert!(!dead_air_expired(
            false,
            true,
            false,
            Some(issued),
            after,
            limit
        ));
        // Nothing pending.
        assert!(!dead_air_expired(true, true, false, None, after, limit));
    }

    /// What `anything_audible` is derived from. A deck preloaded for a handover
    /// is loaded and silent, and counting it as sound would switch the dead-air
    /// limit off through the whole steady state.
    #[test]
    fn a_loaded_deck_is_only_audible_while_its_sink_runs() {
        let mut deck = Deck::new(DeckSlot::A, DeckRole::Main);
        assert!(!deck.audible(), "no sink, no sound");

        let (sink, _output) = Sink::new();
        deck.sink = Some(sink);
        deck.active = true;
        assert!(deck.audible());

        deck.sink.as_ref().expect("sink").pause();
        assert!(!deck.audible(), "a deck preloaded for a handover is silent");

        deck.sink.as_ref().expect("sink").play();
        deck.active = false;
        assert!(!deck.audible(), "a running sink with nothing in it");
    }

    /// Which bound expired does not say what to tell the operator: on a silent
    /// on-air deck the dead-air limit is always the one that fires, dead mount
    /// or not. What arrived is the question they would ask next.
    #[test]
    fn what_the_operator_is_told_follows_what_arrived() {
        let mut deck = Deck::new(DeckSlot::A, DeckRole::Main);
        assert!(!deck.read_delivered(), "no load, nothing delivered");

        let issued = Instant::now();
        deck.load_progress = Some(Seen::issued(issued));
        assert!(!deck.read_delivered(), "a share that has sent nothing");

        deck.load_progress = Some(Seen::issued(issued).observe(64 * 1024, issued));
        assert!(deck.read_delivered(), "a share that is merely slow");
    }

    /// A waiter cannot see the deck's generation, so this is the only way it
    /// hears that the load it is waiting for is not wanted any more.
    #[test]
    fn giving_up_on_a_load_tells_its_waiter_to_stop() {
        let mut deck = Deck::new(DeckSlot::A, DeckRole::Main);
        let cancel = fresh_cancel();
        deck.read_cancel = Some(Arc::clone(&cancel));

        deck.reset();
        assert!(cancel.load(Ordering::Relaxed), "the waiter was told");
        assert!(deck.read_cancel.is_none(), "and the handle was let go");
        // Idempotent: a deck with nothing in flight has nobody to tell.
        deck.cancel_read();
    }

    /// The two bounds answer different questions, and the short one comes
    /// first: a read still delivering has not stalled, yet air is still empty.
    #[test]
    fn a_delivering_read_is_still_dead_air() {
        let issued = Instant::now();
        let dead_air = Duration::from_secs(3);
        let now = issued.checked_add(Duration::from_secs(5)).unwrap();
        let seen = Seen::issued(issued).observe(512 * 1024, now);

        assert!(!watchdog_timed_out(
            true,
            Some(seen),
            now,
            READ_WATCHDOG_TIMEOUT
        ));
        assert!(dead_air_expired(
            true,
            true,
            false,
            Some(issued),
            now,
            dead_air
        ));
    }

    /// The bug in #504: a large file on a slow share kept arriving and was
    /// failed anyway, because the budget was measured from the read's start.
    /// Bytes landing at a tenth of the budget carry the read as far as it has
    /// to go.
    #[test]
    fn a_read_that_keeps_delivering_is_never_timed_out() {
        let mut now = Instant::now();
        let mut seen = Seen::issued(now);
        let step = READ_WATCHDOG_TIMEOUT / 10;
        let mut delivered = 0u64;

        // Ten budgets' worth of elapsed time, a chunk every tenth of one.
        for _ in 0..100 {
            now = now.checked_add(step).unwrap();
            delivered += 256 * 1024;
            seen = seen.observe(delivered, now);
            assert!(
                !watchdog_timed_out(true, Some(seen), now, READ_WATCHDOG_TIMEOUT),
                "a read still delivering bytes is not a wedged mount"
            );
        }

        // The share then goes quiet with the file half read.
        let stalled = now.checked_add(READ_WATCHDOG_TIMEOUT).unwrap();
        seen = seen.observe(delivered, stalled);
        assert!(watchdog_timed_out(
            true,
            Some(seen),
            stalled,
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
                bound_dead_air: true,
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
            bound_dead_air: true,
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
