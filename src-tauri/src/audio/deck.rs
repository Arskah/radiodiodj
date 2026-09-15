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
use super::output::Output;
use super::player::{
    append_at, clamp_start, decode_bytes, read_file, read_with_retry, Bytes, Cmd, PlayerTuning,
    Topics,
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
/// `Arm` is loaded and waiting to take over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeckRole {
    Main,
    Arm,
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
    start_at: f64,
    autoplay: bool,
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
    start_at: f64,
    autoplay: bool,
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
    /// Bytes of the currently loaded track, kept so seeks re-decode from RAM.
    current_bytes: Option<Bytes>,
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
            current_bytes: None,
            seek_offset: 0.0,
            active: false,
            loading: false,
            load_start: None,
            generation: 0,
            volume: 1.0,
            pending_load: None,
            last_time_emit: Instant::now()
                .checked_sub(TIME_EMIT_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Connect to the mixer if this deck has no live sink. A sink built against
    /// an older output generation belongs to a dropped stream and is discarded.
    fn ensure_sink(&mut self, mixer: &Mixer, generation: u64) {
        if self.sink_generation != generation {
            self.sink = None;
        }
        if self.sink.is_none() {
            let sink = Sink::connect_new(mixer);
            sink.set_volume(self.volume);
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
        sink.set_volume(self.volume);
        self.sink = Some(sink);
        self.sink_generation = generation;
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
        self.current_bytes = None;
        self.seek_offset = 0.0;
        self.pending_load = None;
    }
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
}

/// Emit a pause-state change and record it on the role's `playing` mirror.
///
/// Every pause-state transition goes through here, so the flag the handle
/// exposes cannot drift from what the renderer was told.
fn set_pause_state(app: &AppHandle, events: &DeckEvents, paused: bool) {
    events.playing.store(!paused, Ordering::SeqCst);
    let _ = app.emit(&events.topics.pause_state, paused);
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
    start_at: f64,
    autoplay: bool,
) {
    let Some((mixer, generation)) = ensure_output(output, app, events) else {
        // No device yet: remember the intent and let the idle loop retry the
        // open. `ensure_output` already emitted the error; keep the buffering
        // indicator up while we wait for the device.
        deck.pending_load = Some(PendingLoad {
            id,
            path,
            duration,
            start_at,
            autoplay,
        });
        output.mark_retry_now();
        deck.current_id = Some(id);
        let _ = app.emit(&events.topics.buffering, true);
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
    deck.current_bytes = None;
    deck.seek_offset = 0.0;
    deck.active = false;
    deck.loading = true;
    deck.load_start = Some(Instant::now());
    let _ = app.emit(&events.topics.buffering, true);

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
            start_at,
            autoplay,
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
                start_at,
                autoplay,
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
    match cmd {
        Cmd::Load {
            id,
            path,
            duration,
            start_at,
            autoplay,
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
                start_at,
                autoplay,
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
        Cmd::Stop => {
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
        Cmd::Seek(s) => {
            let target = s.max(0.0);
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
                    append_at(sink, source, Duration::from_secs_f64(target));
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
            let clamped = v.clamp(0.0, 1.0);
            deck.volume = clamped;
            // Remembered on the deck and applied when its sink next connects.
            if let Some(sink) = deck.sink.as_ref() {
                sink.set_volume(clamped);
            }
        }
    }
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
            let final_duration = msg.duration.or(decoded_duration);
            // The start position travels with the load rather than arriving as
            // a separate Seek: the bytes only exist here, so a Seek issued
            // alongside the Load would have found none and been dropped.
            let start_at = clamp_start(msg.start_at, final_duration);
            append_at(sink, source, Duration::from_secs_f64(start_at));
            if msg.autoplay {
                sink.play();
            } else {
                sink.pause();
            }
            deck.current_bytes = Some(bytes);
            deck.current_duration = final_duration;
            deck.seek_offset = start_at;
            deck.active = true;
            if let Some(d) = final_duration {
                let _ = app.emit(&events.topics.duration, d);
            }
            let _ = app.emit(&events.topics.time, start_at);
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
                    let Some(i) = decks.iter().position(|d| d.role == role) else {
                        continue;
                    };
                    let role_index = decks[i].role as usize;
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
                        p.start_at,
                        p.autoplay,
                    );
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
                let _ = app.emit(&events.topics.time, deck.seek_offset + pos);
            }

            if deck.active && !deck.loading && empty {
                deck.active = false;
                set_pause_state(&app, events, true);
                let _ = app.emit(&events.topics.ended, ());
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
            start_at: 0.0,
            autoplay: true,
            bytes: Ok(Arc::from(Vec::new().into_boxed_slice())),
        };
        let fresh = LoadMsg {
            deck: 0,
            generation: 5,
            id: 1,
            duration: None,
            start_at: 0.0,
            autoplay: true,
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
            start_at: 0.0,
            autoplay: true,
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
