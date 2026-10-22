//! Wiring for the playlist engine.
//!
//! Owns the [`Playlist`] and turns its [`Effect`]s into real work: deck loads,
//! play-count increments, prefetch-window updates, outage retry timers. Every
//! transition ends with a [`Snapshot`] on `program:playlist-state`, which is the
//! renderer's only source of playlist truth.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Listener};

use super::engine::{Effect, LoadFailure, Playlist, Refiller, Transition};
use super::generate;
use super::model::{
    Destination, HoldingStop, PlaylistItem, QueueChild, QueueNode, Snapshot, SourceInfo,
};
use super::timing::{
    ClockSample, CurrentTiming, PlaylistTiming, Schedule, TimingItem, TimingKind, TimingMode,
    TimingTransport,
};
use crate::audio::bus::{ProgramBus, FADED_OUT_EVENT, HANDOVER_EVENT};
use crate::audio::cache::Cache;
use crate::audio::cue_points::CuePoints;
use crate::audio::levelling;
use crate::audio::player::{Cmd, LoadFailed};
use crate::audio::timing::{self as audio_timing, Observation};
use crate::broadcast::BroadcastService;
use crate::library::db::{
    Db, SavedEntryKind, SavedPlaylist, SavedPlaylistKind, SavedQueueEntry, Track, TrackLoadInfo,
};
use crate::library::health::HEALTH_EVENT;
use crate::library::scanner::now_ms;
use crate::library::waveform_scan::{
    CuePointsReady, DurationReady, CUE_POINTS_READY_EVENT, DURATION_READY_EVENT,
};
use crate::persist::config::Config;
use crate::persist::session::{SessionPlaylistItem, SessionQueueNode, SessionState};

/// The part of `program:handover` the playlist needs: the incoming track. The
/// outgoing one is already on record as `current`.
#[derive(serde::Deserialize)]
struct HandoverEvent {
    to: i64,
    #[serde(default)]
    manual: bool,
}

#[derive(serde::Deserialize)]
struct EndEvent {
    #[serde(default)]
    manual: bool,
}

/// The part of the library health report the playlist needs: the tracks it
/// must not air, whether their file is gone or an admin hid them.
#[derive(serde::Deserialize)]
struct MissingIds {
    missing: Vec<IdOnly>,
    hidden: Vec<IdOnly>,
}

#[derive(serde::Deserialize)]
struct IdOnly {
    id: i64,
}

/// Topic the whole-playlist snapshot is emitted on.
pub const PLAYLIST_STATE_EVENT: &str = "program:playlist-state";

/// Fallback retry delay, used only if the configured schedule is empty. The
/// stored one is clamped non-empty on write, so this is a belt-and-braces value
/// rather than a tunable.
const DEFAULT_BACKOFF_MS: u64 = 1000;

/// How long [`PlaylistService::drain`] waits for the command thread at shutdown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Refill material drawn from the library, sized by the stored tuning. Both the
/// cadence and the sizes are read per call, so a settings change takes effect on
/// the next refill without a restart.
struct DbRefiller<'a> {
    db: &'a Db,
    interleave: generate::Interleave,
    rotation: generate::Rotation,
    /// The saved playlist music is drawn from, or the music library.
    pool: Option<i64>,
    buffer: i64,
    threshold: i64,
    history_cap: usize,
}

fn authored_items(entries: Vec<SavedQueueEntry>, tracks: Vec<Track>) -> Vec<PlaylistItem> {
    let by_id: HashMap<i64, Track> = tracks.into_iter().map(|track| (track.id, track)).collect();
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            SavedQueueEntry::Track { id, .. } => by_id.get(&id).cloned().map(PlaylistItem::track),
            SavedQueueEntry::Stop {
                planned_duration_seconds,
            } => Some(PlaylistItem::Stop {
                planned_duration_seconds,
            }),
        })
        .collect()
}

/// Copy one saved list's authored sequence, retaining Stops and counting only
/// unmatched tracks as unavailable. The saved original is never mutated.
pub(crate) fn saved_copy(
    list: SavedPlaylist,
) -> (String, SavedPlaylistKind, Vec<SavedQueueEntry>, usize) {
    let mut entries = Vec::with_capacity(list.entries.len());
    let mut unmatched = 0;
    for entry in list.entries {
        match entry.kind {
            SavedEntryKind::Stop => entries.push(SavedQueueEntry::Stop {
                planned_duration_seconds: entry.planned_duration_seconds,
            }),
            SavedEntryKind::Track => match entry.track {
                Some(track) => entries.push(SavedQueueEntry::Track {
                    id: track.id,
                    content_type: entry.content_type.unwrap_or_default(),
                }),
                None => unmatched += 1,
            },
        }
    }
    (list.name, list.kind, entries, unmatched)
}

impl Refiller for DbRefiller<'_> {
    fn generate(&self, count: i64, queued: &[&Track]) -> Vec<Track> {
        generate::generate(
            self.db,
            count,
            queued,
            &self.interleave,
            &self.rotation,
            self.pool,
            now_ms(),
        )
        .unwrap_or_else(|e| {
            log::error!("auto-playlist refill failed: {}", e);
            vec![]
        })
    }

    fn buffer(&self) -> i64 {
        self.buffer
    }

    fn threshold(&self) -> i64 {
        self.threshold
    }

    fn history_cap(&self) -> usize {
        self.history_cap
    }
}

/// The auto-playlist source the operator chose, and what the service last had
/// to say about it. See `docs/saved-playlists.md`.
#[derive(Default)]
struct Source {
    /// The saved playlist's id, and its name as last read — kept so a revert
    /// can still name a source that has been deleted.
    chosen: Option<(i64, String)>,
    /// Set when a source ran out of playable music; cleared by the next choice.
    reverted_from: Option<String>,
}

#[derive(Default)]
struct TimingState {
    schedule: Schedule,
    wire: PlaylistTiming,
    observation: Option<Observation>,
    fresh_observation: bool,
    revision: u64,
    force_reanchor: bool,
    hold: Option<(String, f64, f64)>,
}

impl TimingState {
    fn manual_transition(&mut self, manual: bool) {
        self.force_reanchor |= manual;
    }

    fn prepare_start(
        &mut self,
        playlist: &Playlist,
        target: Option<&str>,
        captured: audio_timing::Capture,
    ) {
        let Some(target) = target else {
            return;
        };
        let snapshot = playlist.snapshot();
        let upcoming = snapshot.playlist.iter().any(|node| match node {
            QueueNode::Standalone { id, .. } => id == target,
            QueueNode::Block { entries, .. } => entries.iter().any(|entry| entry.id == target),
        });
        if !upcoming {
            return;
        }
        let resumes_next = playlist.next_occurrence_id() == Some(target);
        let finished_track = self.observation.as_ref().is_some_and(|observation| {
            observation.occurrence_id.as_deref() == playlist.current_occurrence_id()
                && !observation.playing
                && !observation.buffering
                && observation
                    .duration_seconds
                    .is_some_and(|duration| observation.position_seconds >= duration)
        });
        let early_plan = snapshot.holding_stop.as_ref().is_some_and(|hold| {
            hold.planned_duration_seconds.is_some_and(|plan| {
                let elapsed = self
                    .hold
                    .as_ref()
                    .filter(|(id, _, _)| id == &hold.occurrence_id)
                    .map_or(hold.elapsed_seconds, |(_, elapsed, started)| {
                        elapsed + (captured.monotonic_seconds - started).max(0.0)
                    });
                elapsed < f64::from(plan)
            })
        });
        self.force_reanchor |=
            !resumes_next || (snapshot.current.is_some() && !finished_track) || early_plan;
    }

    fn accept(&mut self, playlist: &Playlist, observation: Observation) -> bool {
        if observation.occurrence_id.as_deref() != playlist.current_occurrence_id()
            || observation.track_id != playlist.snapshot().current.as_ref().map(|track| track.id)
            || self
                .observation
                .as_ref()
                .is_some_and(|previous| previous.sequence >= observation.sequence)
        {
            return false;
        }
        if let Some(previous) = self.observation.as_ref() {
            let seek = previous.instance_id == observation.instance_id
                && previous.seek_revision != observation.seek_revision;
            let interruption = observation.interrupted && !previous.interrupted;
            self.force_reanchor |= seek || interruption;
        }
        self.observation = Some(observation);
        self.fresh_observation = true;
        true
    }

    fn project(
        &mut self,
        playlist: &Playlist,
        captured: audio_timing::Capture,
        correction: f64,
    ) -> Snapshot {
        let mut snapshot = playlist.snapshot();
        let timing = self;
        if correction != 0.0 {
            timing.schedule.shift_wall(correction);
        }
        let mut clock = clock_sample(captured);
        if std::mem::take(&mut timing.fresh_observation) {
            if let Some(observation) = &timing.observation {
                clock.unix_ms -=
                    (clock.monotonic_seconds - observation.captured.monotonic_seconds) * 1000.0;
                clock.monotonic_seconds = observation.captured.monotonic_seconds;
            }
        }
        let mut items = Vec::new();
        for node in &snapshot.playlist {
            match node {
                QueueNode::Standalone { id, item } => items.push(timing_item(
                    id,
                    item,
                    item.as_track()
                        .is_some_and(|track| playlist.is_unavailable(track.id)),
                )),
                QueueNode::Block { entries, .. } => items.extend(entries.iter().map(|entry| {
                    timing_item(
                        &entry.id,
                        &entry.item,
                        entry
                            .item
                            .as_track()
                            .is_some_and(|track| playlist.is_unavailable(track.id)),
                    )
                })),
            }
        }
        let next_is_stop = items
            .iter()
            .find(|item| {
                !matches!(
                    item.kind,
                    TimingKind::Track {
                        unavailable: true,
                        ..
                    }
                )
            })
            .is_some_and(|item| matches!(item.kind, TimingKind::Stop { .. }));
        let mut pending = false;
        let transport = if let Some(hold) = &mut snapshot.holding_stop {
            if timing
                .hold
                .as_ref()
                .is_none_or(|(id, _, _)| id != &hold.occurrence_id)
            {
                timing.hold = Some((
                    hold.occurrence_id.clone(),
                    hold.elapsed_seconds,
                    clock.monotonic_seconds,
                ));
            }
            let (_, elapsed, started) = timing.hold.as_ref().expect("hold clock established");
            hold.elapsed_seconds = elapsed + (clock.monotonic_seconds - started).max(0.0);
            hold.captured_at_unix_ms = clock.unix_ms;
            TimingTransport::Holding {
                remaining_plan: hold.planned_duration_seconds.map_or(0.0, |plan| {
                    (f64::from(plan) - hold.elapsed_seconds).max(0.0)
                }),
            }
        } else if let Some(track) = &snapshot.current {
            timing.hold = None;
            let current_id = playlist.current_occurrence_id().unwrap_or_default();
            let observation = timing
                .observation
                .as_ref()
                .filter(|observation| observation.occurrence_id.as_deref() == Some(current_id));
            let cue = snapshot
                .current_override
                .unwrap_or(track.cue_points)
                .resolve((track.duration > 0.0).then_some(track.duration));
            let position = observation.map_or(0.0, |observation| {
                observation.position_seconds
                    + if observation.playing {
                        (clock.monotonic_seconds - observation.captured.monotonic_seconds).max(0.0)
                    } else {
                        0.0
                    }
            });
            let duration = observation
                .and_then(|observation| observation.duration_seconds)
                .or(cue.air_duration());
            let next_start = observation
                .and_then(|observation| observation.next_start_seconds)
                .or(cue.next_start.map(|at| cue.air_time(at)));
            let tail = observation.map_or(0.0, |observation| {
                (observation.tail_remaining_seconds
                    - if observation.playing {
                        (clock.monotonic_seconds - observation.captured.monotonic_seconds).max(0.0)
                    } else {
                        0.0
                    })
                .max(0.0)
            });
            let span = if snapshot.auto_advance && !next_is_stop {
                next_start.or(duration)
            } else {
                duration
            };
            if observation.is_some_and(|observation| observation.playing) {
                TimingTransport::Playing(CurrentTiming {
                    occurrence_id: current_id.to_owned(),
                    position,
                    air_duration: duration,
                    next_start,
                    tail_remaining: tail,
                })
            } else {
                pending = !snapshot.awaiting_network
                    && observation.is_none_or(|observation| {
                        observation.buffering && !observation.interrupted
                    })
                    && timing
                        .wire
                        .entries
                        .iter()
                        .any(|entry| entry.occurrence_id == current_id);
                TimingTransport::Waiting {
                    remaining: span.zip(duration).map(|(span, duration)| {
                        (span - position)
                            .max(0.0)
                            .max(tail)
                            .min((duration - position).max(0.0))
                    }),
                    tail_remaining: 0.0,
                }
            }
        } else {
            timing.hold = None;
            TimingTransport::Waiting {
                remaining: Some(0.0),
                tail_remaining: 0.0,
            }
        };
        let force = timing.force_reanchor;
        if !pending || force {
            timing.force_reanchor = false;
            timing.wire = timing.schedule.update(
                &items,
                if snapshot.auto_advance {
                    TimingMode::Auto
                } else {
                    TimingMode::Manual
                },
                &transport,
                clock,
                force,
            );
        }
        let upcoming: HashSet<&str> = items
            .iter()
            .map(|item| item.occurrence_id.as_str())
            .collect();
        snapshot.timing = timing.wire.clone();
        snapshot
            .timing
            .entries
            .retain(|entry| upcoming.contains(entry.occurrence_id.as_str()));
        timing.revision = timing.revision.wrapping_add(1);
        snapshot.revision = timing.revision;
        snapshot
    }
}

fn clock_sample(capture: audio_timing::Capture) -> ClockSample {
    ClockSample {
        monotonic_seconds: capture.monotonic_seconds,
        unix_ms: capture.unix_ms,
    }
}

fn timing_item(id: &str, item: &PlaylistItem, unavailable: bool) -> TimingItem {
    let kind = match item {
        PlaylistItem::Stop {
            planned_duration_seconds,
        } => TimingKind::Stop {
            planned_duration_seconds: planned_duration_seconds.map(f64::from),
        },
        PlaylistItem::Track {
            track,
            cue_override,
        } => {
            let cue = cue_override
                .unwrap_or(track.cue_points)
                .resolve((track.duration > 0.0).then_some(track.duration));
            TimingKind::Track {
                air_duration: cue.air_duration(),
                next_start: cue.next_start.map(|at| cue.air_time(at)),
                unavailable,
            }
        }
    };
    TimingItem {
        occurrence_id: id.to_owned(),
        kind,
    }
}

struct Inner {
    playlist: Mutex<Playlist>,
    timing: Mutex<TimingState>,
    source: Mutex<Source>,
    db: Arc<Db>,
    config: Arc<Config>,
    bus: Arc<ProgramBus>,
    cache: Arc<Cache>,
    broadcast: Arc<BroadcastService>,
    app: AppHandle,
    /// Bumped whenever a retry is armed or cancelled. A sleeping timer whose
    /// generation no longer matches has been superseded and does nothing —
    /// which is how an explicit track change cancels a pending outage retry.
    retry_generation: AtomicU64,
}

/// One unit of queued work: it is handed what the owner holds and answers
/// nothing — a transition's answer is the snapshot it emits.
type Job<T> = Box<dyn FnOnce(&T) + Send>;

/// Jobs applied one at a time, on a thread of their own, in the order they were
/// posted.
struct Serial<T> {
    jobs: Sender<Job<T>>,
}

impl<T: Send + 'static> Serial<T> {
    /// Start the thread. It owns `context` and lives as long as the sender does.
    ///
    /// A panicking job is caught and logged rather than taken as the thread's
    /// death: the alternative is a station whose transport buttons silently stop
    /// working mid-show. The playlist's lock does not poison, so the next command
    /// is applied to whatever the failed transition left behind.
    fn spawn(context: T) -> Self {
        let (jobs, rx) = channel::<Job<T>>();
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let run = AssertUnwindSafe(|| job(&context));
                if catch_unwind(run).is_err() {
                    log::error!("playlist: a queued command panicked");
                }
            }
        });
        Self { jobs }
    }

    /// Post a job and return, without waiting for it to run.
    fn post<F>(&self, job: F)
    where
        F: FnOnce(&T) + Send + 'static,
    {
        if self.jobs.send(Box::new(job)).is_err() {
            log::error!("playlist: the command thread is gone");
        }
    }

    /// Wait for the jobs posted so far to have run. Reports whether they did
    /// within `timeout`.
    fn drain(&self, timeout: Duration) -> bool {
        let (tx, applied) = channel();
        self.post(move |_| {
            let _ = tx.send(());
        });
        applied.recv_timeout(timeout).is_ok()
    }
}

pub struct PlaylistService {
    inner: Arc<Inner>,
    /// Hands work to the command thread. See [`PlaylistService::queue`].
    commands: Serial<Arc<Inner>>,
}

impl PlaylistService {
    pub fn new(
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        bus: Arc<ProgramBus>,
        cache: Arc<Cache>,
        broadcast: Arc<BroadcastService>,
    ) -> Self {
        let inner = Arc::new(Inner {
            playlist: Mutex::new(Playlist::new()),
            timing: Mutex::new(TimingState::default()),
            source: Mutex::new(Source::default()),
            db,
            config,
            bus,
            cache,
            broadcast,
            app,
            retry_generation: AtomicU64::new(0),
        });
        let commands = Serial::spawn(Arc::clone(&inner));
        Self { inner, commands }
    }

    /// Run a transition on the command thread.
    ///
    /// A Tauri command handler without `async` runs on the process main thread,
    /// which is the thread the window is drawn from — so a transition that
    /// queries the library there stalls the UI for as long as the query takes.
    /// A refill under the queue's threshold is tens of milliseconds of SQL on a
    /// large library, and the lock the library keeps is shared with the analysis
    /// pass, so the wait is not bounded by this query alone. Every command
    /// therefore posts its work here and returns.
    ///
    /// One thread rather than the async runtime's pool, because the order
    /// commands are applied in is part of what they mean: `remove` and `move`
    /// carry queue indices, and a pool would let the second of two clicks
    /// overtake the first and act on positions that no longer exist.
    ///
    /// Deck events and the outage retry timer keep running transitions on their
    /// own threads; the playlist lock is what makes each one atomic, and that
    /// has never been this thread's job.
    fn queue<F>(&self, job: F)
    where
        F: FnOnce(&Arc<Inner>) + Send + 'static,
    {
        self.commands.post(job);
    }

    /// Wait for what is already queued to have been applied.
    ///
    /// Called on the way out. A queued transition holds work that outlives the
    /// process otherwise: `Effect::TrackPlayed` is what writes the airing log,
    /// and rotation reads that log back. Quitting a beat after the last skip
    /// would lose the play.
    ///
    /// Bounded, because shutdown is not the place to wait on a database lock the
    /// analysis pass happens to hold; a queue that has not drained by then is
    /// logged and abandoned.
    pub fn drain(&self) {
        if !self.commands.drain(DRAIN_TIMEOUT) {
            log::warn!("playlist: queued commands did not finish before shutdown");
        }
    }

    /// Subscribe to the main deck's lifecycle. Advancement is driven from here
    /// rather than from the renderer: the deck that notices the track ended is
    /// in the same process as the playlist that knows what follows it.
    pub fn attach_to_app(&self, app: &AppHandle) {
        let ended = Arc::clone(&self.inner);
        app.listen("main-deck:ended", move |event| {
            ended.timing.lock().manual_transition(
                serde_json::from_str::<EndEvent>(event.payload()).is_ok_and(|event| event.manual),
            );
            Inner::apply(&ended, |p, r| p.on_ended(r));
        });

        let handover = Arc::clone(&self.inner);
        app.listen(HANDOVER_EVENT, move |event| {
            let Ok(moved) = serde_json::from_str::<HandoverEvent>(event.payload()) else {
                return;
            };
            handover.timing.lock().manual_transition(moved.manual);
            Inner::apply(&handover, move |p, r| p.on_handover(moved.to, r));
        });

        // A fade to silence finished on air. It is a Stop the operator asked
        // for slowly, so it takes the track off air exactly as Stop does —
        // otherwise the engine keeps believing a silent deck is playing and the
        // next Play resumes nothing.
        let faded = Arc::clone(&self.inner);
        app.listen(FADED_OUT_EVENT, move |_| {
            faded.timing.lock().manual_transition(true);
            Inner::apply(&faded, |p, _| p.stop());
        });

        // The tail is gone, so the deck it held can be armed again.
        let tail_ended = Arc::clone(&self.inner);
        app.listen("tail-deck:ended", move |_| {
            Inner::apply(&tail_ended, |p, _| p.on_tail_ended());
        });

        // The airing is counted here, not when the load was issued: a read
        // that fails over a dead share never put anything on air.
        let loaded = Arc::clone(&self.inner);
        app.listen("main-deck:loaded", move |event| {
            let Ok(id) = serde_json::from_str::<i64>(event.payload()) else {
                return;
            };
            Inner::apply(&loaded, move |p, _| p.on_loaded(id));
        });

        let failed = Arc::clone(&self.inner);
        app.listen("main-deck:load-failed", move |event| {
            let Ok(load) = serde_json::from_str::<LoadFailed>(event.payload()) else {
                return;
            };
            let why = if load.abandoned {
                LoadFailure::GivenUp
            } else {
                LoadFailure::Unplayable
            };
            Inner::apply(&failed, move |p, r| p.on_load_failed(load.id, why, r));
        });

        let missing = Arc::clone(&self.inner);
        app.listen(HEALTH_EVENT, move |event| {
            let Ok(report) = serde_json::from_str::<MissingIds>(event.payload()) else {
                return;
            };
            let ids = report
                .missing
                .into_iter()
                .chain(report.hidden)
                .map(|t| t.id)
                .collect();
            Inner::apply(&missing, |p, r| p.on_missing_state(ids, r));
        });

        // A queued item holds a copy of its track, so an automatic cue result
        // landing mid-queue has to reach it — otherwise the row keeps the full
        // file length while the deck airs the trimmed one, and the stale copy
        // is what a session restore brings back.
        let auto_cue = Arc::clone(&self.inner);
        app.listen(CUE_POINTS_READY_EVENT, move |event| {
            let Ok(ready) = serde_json::from_str::<CuePointsReady>(event.payload()) else {
                return;
            };
            Inner::adopt_cue_points(&auto_cue, ready.id, ready.cue_points);
        });

        // The same copy carries the track's length, which is the tag's guess
        // until the analysis pass has measured it.
        let measured = Arc::clone(&self.inner);
        app.listen(DURATION_READY_EVENT, move |event| {
            let Ok(ready) = serde_json::from_str::<DurationReady>(event.payload()) else {
                return;
            };
            Inner::adopt_duration(&measured, ready.id, ready.duration);
        });

        let transport = Arc::clone(&self.inner);
        let commands = self.commands.jobs.clone();
        app.listen(audio_timing::TRANSPORT_EVENT, move |event| {
            let Ok(observation) = serde_json::from_str::<Observation>(event.payload()) else {
                return;
            };
            let transport = Arc::clone(&transport);
            let _ = commands.send(Box::new(move |_| Inner::observe(&transport, observation)));
        });

        let cache_state = Arc::clone(&self.inner);
        app.listen("main-deck:cache-state", move |event| {
            let ids: Vec<i64> = serde_json::from_str(event.payload()).unwrap_or_default();
            Inner::apply(&cache_state, |p, r| p.on_cache_state(ids, r));
        });
    }

    /// Current state, for a renderer that has just (re)connected.
    pub fn snapshot(&self) -> Snapshot {
        let playlist = self.inner.playlist.lock();
        self.inner.project_snapshot(&playlist)
    }

    /// Read Upcoming after earlier commands have run, so Save as captures the
    /// sequence the operator just edited rather than a snapshot still waiting
    /// behind those commands.
    pub fn queued_items(&self) -> Result<Vec<PlaylistItem>, String> {
        let (send, receive) = channel();
        self.queue(move |inner| {
            let _ = send.send(inner.playlist.lock().remaining_items());
        });
        receive
            .recv()
            .map_err(|_| "playlist command thread stopped".to_owned())
    }

    /// Restore the persisted playlist and put the saved track back on the deck,
    /// paused at its saved position. History comes from the airing log rather
    /// than the session file, so a restart shows what actually went out even if
    /// the session was lost.
    ///
    /// Applied directly rather than queued. It runs during setup, before the
    /// window exists, and [`Self::snapshot`] is what the renderer reads once it
    /// is listening — a queued restore could still be resolving its rows when
    /// that call arrives and would answer it with an empty playlist.
    pub fn hydrate(&self, state: &SessionState) {
        let queue = state
            .queue
            .as_ref()
            .map(|nodes| self.inner.resolve_queue(nodes));
        let items = queue.is_none().then(|| self.inner.resolve_items(state));
        let current_block_id = state.current_block_id.clone();
        let current = state
            .current_track_id
            .and_then(|id| self.inner.lookup(id).ok().flatten());
        let current_override = state.current_cue_override;
        let seconds = state.current_time;
        let auto_playlist = state.auto_playlist_active;
        let auto_advance = state.auto_advance;
        // Its name is read back by the first transition, which also drops it
        // if the saved playlist did not survive the restart.
        self.inner.source.lock().chosen = state.auto_playlist_source.map(|id| (id, String::new()));
        let cap = self.inner.config.get_tuning().auto_playlist.history_cap;
        let history = self
            .inner
            .db
            .recent_airings(cap as i64)
            .unwrap_or_else(|e| {
                log::error!("history restore failed: {}", e);
                vec![]
            });
        let restored_hold = state.holding_stop.clone();
        let held_block = restored_hold.as_ref().and_then(|hold| queue.as_ref().and_then(|nodes| nodes.iter().find(|node| matches!(node, QueueNode::Block { id, .. } if Some(id) == hold.block_id.as_ref())).cloned()));
        Inner::apply(&self.inner, move |p, _| {
            let transition = match queue {
                Some(queue) => p.hydrate_queue(
                    queue,
                    current_block_id,
                    current,
                    current_override,
                    seconds,
                    auto_playlist,
                    auto_advance,
                    history,
                ),
                None => p.hydrate(
                    items.unwrap_or_default(),
                    current,
                    current_override,
                    seconds,
                    auto_playlist,
                    auto_advance,
                    history,
                ),
            };
            if let Some(hold) = restored_hold {
                p.restore_hold(
                    HoldingStop {
                        occurrence_id: if hold.occurrence_id.is_empty() {
                            uuid::Uuid::new_v4().to_string()
                        } else {
                            hold.occurrence_id
                        },
                        planned_duration_seconds: hold.planned_duration_seconds,
                        elapsed_seconds: if hold.elapsed_seconds.is_finite() {
                            hold.elapsed_seconds.max(0.0)
                        } else {
                            0.0
                        },
                        captured_at_unix_ms: 0.0,
                        block_id: hold.block_id,
                    },
                    held_block,
                );
                Transition::default()
            } else {
                transition
            }
        });
    }

    /// Queue a track at the end, optionally under an override for one airing.
    pub fn add(&self, id: i64, cue_override: Option<CuePoints>) {
        self.queue(move |inner| {
            Inner::with_track(inner, id, move |p, _, track| p.add(track, cue_override))
        });
    }

    /// Queue a track as next-up, optionally under an override the operator
    /// auditioned on the cue deck.
    pub fn add_front(&self, id: i64, cue_override: Option<CuePoints>) {
        self.queue(move |inner| {
            Inner::with_track(inner, id, move |p, _, track| {
                p.add_front(track, cue_override)
            })
        });
    }

    /// Queue a track ahead of the item at `index`.
    pub fn insert(&self, id: i64, index: usize) {
        self.queue(move |inner| {
            Inner::with_track(inner, id, move |p, _, track| p.insert(index, track))
        });
    }

    /// Queue several tracks as one run ahead of the item at `index`, or at the
    /// end without one: one transition and one snapshot, however many there
    /// are. An id with no row is skipped, as [`Inner::with_track`] skips one.
    pub fn add_many(&self, ids: Vec<i64>, index: Option<usize>) {
        self.queue(move |inner| {
            let tracks = match inner.db.get_tracks_by_ids(&ids) {
                Ok(tracks) => tracks,
                Err(e) => {
                    log::error!("playlist: lookup of {} tracks failed: {}", ids.len(), e);
                    return;
                }
            };
            if tracks.len() < ids.len() {
                log::error!(
                    "playlist: {} of {} tracks are not in the library",
                    ids.len() - tracks.len(),
                    ids.len()
                );
            }
            if tracks.is_empty() {
                return;
            }
            Inner::apply(inner, move |p, _| p.insert_many(index, tracks));
        });
    }

    /// Copy library tracks into an ID-addressed top-level or block gap.
    pub fn copy_tracks(&self, ids: Vec<i64>, destination: Destination) {
        self.queue(move |inner| match inner.db.get_tracks_by_ids(&ids) {
            Ok(tracks) if !tracks.is_empty() => {
                Inner::apply(inner, move |p, _| p.copy_tracks(tracks, destination));
            }
            Ok(_) => {}
            Err(e) => log::error!("playlist: looking up tracks for copy failed: {e}"),
        });
    }

    /// Insert a selected run ahead of an active block's remaining children,
    /// or at the main queue's front when no block is active.
    pub fn add_as_next(&self, ids: Vec<i64>) {
        self.queue(move |inner| match inner.db.get_tracks_by_ids(&ids) {
            Ok(tracks) if !tracks.is_empty() => {
                Inner::apply(inner, move |p, _| p.add_as_next(tracks));
            }
            Ok(_) => {}
            Err(e) => log::error!("playlist: looking up next tracks failed: {e}"),
        });
    }

    /// Choose where the auto-playlist draws its music from: a saved playlist,
    /// by id and name, or `None` for the music library. Sets the source and
    /// nothing else — it neither switches the auto-playlist on nor touches what
    /// is queued. Any choice clears a pending revert notice.
    pub fn set_source(&self, chosen: Option<(i64, String)>) {
        self.queue(move |inner| {
            {
                let mut source = inner.source.lock();
                source.chosen = chosen;
                source.reverted_from = None;
            }
            Inner::apply(inner, |p, r| p.on_source_changed(r));
        });
    }

    /// Queue an independent named block copied from a saved list. Ordinary
    /// playlists may first be brought up to the interleave cadence; prepared
    /// shows keep their authored track and Stop sequence.
    pub fn add_saved(
        &self,
        title: String,
        kind: SavedPlaylistKind,
        entries: Vec<SavedQueueEntry>,
        weave: bool,
        before_id: Option<String>,
    ) {
        self.queue(move |inner| {
            let ids: Vec<i64> = entries
                .iter()
                .filter_map(|entry| match entry {
                    SavedQueueEntry::Track { id, .. } => Some(*id),
                    SavedQueueEntry::Stop { .. } => None,
                })
                .collect();
            let tracks = match inner.db.get_tracks_by_ids(&ids) {
                Ok(tracks) => tracks,
                Err(e) => {
                    log::error!("playlist: lookup of {} tracks failed: {}", ids.len(), e);
                    return;
                }
            };
            if tracks.len() < ids.len() {
                log::error!(
                    "playlist: {} of {} tracks are not in the library",
                    ids.len() - tracks.len(),
                    ids.len()
                );
            }
            let items = if weave {
                let types: HashMap<i64, String> = entries
                    .into_iter()
                    .filter_map(|entry| match entry {
                        SavedQueueEntry::Track { id, content_type } => Some((id, content_type)),
                        SavedQueueEntry::Stop { .. } => None,
                    })
                    .collect();
                let list = tracks
                    .iter()
                    .map(|t| {
                        let content_type = types.get(&t.id).cloned().unwrap_or_default();
                        (t.clone(), content_type)
                    })
                    .collect();
                let queued: Vec<i64> = inner
                    .playlist
                    .lock()
                    .remaining_items()
                    .iter()
                    .filter_map(|item| item.as_track().map(|t| t.id))
                    .collect();
                let interleave = generate::Interleave::from_config(&inner.config.get_tuning());
                let woven = match generate::weave(&inner.db, list, &queued, &interleave) {
                    Ok(woven) => woven,
                    // The operator named the list; it goes on as written.
                    Err(e) => {
                        log::error!("playlist: weaving a saved playlist failed: {}", e);
                        tracks
                    }
                };
                woven.into_iter().map(PlaylistItem::track).collect()
            } else {
                authored_items(entries, tracks)
            };
            if items.is_empty() {
                return;
            }
            Inner::apply(inner, move |p, _| {
                p.insert_block(title, kind, items, before_id.as_deref())
            });
        });
    }

    /// The automatic-cue policy changed, so every copy held here is stale at
    /// once. Re-reads them from the library, which applies the policy, in one
    /// transition rather than one per track.
    pub fn reload_cue_points(&self) {
        self.queue(|inner| {
            let ids = inner.playlist.lock().held_ids();
            if ids.is_empty() {
                return;
            }
            let fresh: HashMap<i64, CuePoints> = match inner.db.get_tracks_by_ids(&ids) {
                Ok(tracks) => tracks.into_iter().map(|t| (t.id, t.cue_points)).collect(),
                Err(e) => {
                    log::error!("playlist: re-reading cue points failed: {}", e);
                    return;
                }
            };
            Inner::apply(inner, move |p, _| p.refresh_cue_points(&fresh));
        });
    }

    /// A radio edit was stored for `id`; refresh the queued copies of it so the
    /// operator's next snapshot shows what was just saved.
    pub fn on_cue_points_saved(&self, id: i64, points: CuePoints) {
        self.queue(move |inner| Inner::adopt_cue_points(inner, id, points));
    }

    pub fn set_item_cue_points(&self, index: usize, cue_override: Option<CuePoints>) {
        self.queue(move |inner| {
            Inner::apply(inner, move |p, _| {
                p.set_item_cue_points(index, cue_override)
            })
        });
    }

    /// Change one queued occurrence's cue points without relying on a visible index.
    pub fn set_occurrence_cue_points(&self, id: String, cue_override: Option<CuePoints>) {
        self.queue(move |inner| {
            Inner::apply(inner, move |p, _| {
                p.set_occurrence_cue_points(&id, cue_override)
            });
        });
    }

    /// Set only an upcoming Stop’s plan, leaving its saved source untouched.
    pub fn set_stop_duration(&self, occurrence_id: String, planned_duration_seconds: Option<u32>) {
        self.queue(move |inner| {
            Inner::apply(inner, move |playlist, _| {
                playlist.set_stop_duration(&occurrence_id, planned_duration_seconds)
            })
        });
    }

    pub fn add_stop(&self) {
        self.queue(|inner| Inner::apply(inner, |p, _| p.add_stop()));
    }

    /// Append one jingle or commercial picked the same way the auto-playlist
    /// would have. A no-op when the typed library is empty.
    pub fn add_filler(&self, content_type: generate::ContentType) {
        self.queue(move |inner| {
            let interleave = generate::Interleave::from_config(&inner.config.get_tuning());
            match generate::pick_filler(&inner.db, content_type, &interleave) {
                Ok(Some(track)) => Inner::apply(inner, move |p, _| p.add(track, None)),
                Ok(None) => {}
                Err(e) => log::error!("playlist: picking a {:?} failed: {}", content_type, e),
            }
        });
    }

    pub fn remove(&self, index: usize) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.remove(index)));
    }

    /// Remove one standalone item or block child by occurrence id.
    pub fn remove_occurrence(&self, id: String) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.remove_occurrence(&id)));
    }

    /// Move one queued occurrence to a stable top-level or block gap.
    pub fn move_occurrence(&self, id: String, destination: Destination) {
        self.queue(move |inner| {
            Inner::apply(inner, move |p, _| p.move_occurrence(&id, destination));
        });
    }

    /// Move an unstarted block as one top-level occurrence.
    pub fn move_block(&self, block_id: String, before_id: Option<String>) {
        self.queue(move |inner| {
            Inner::apply(inner, move |p, _| {
                p.move_block(&block_id, before_id.as_deref())
            });
        });
    }

    /// Remove a block's remaining children without cutting its current track.
    pub fn remove_block(&self, block_id: String) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.remove_block(&block_id)));
    }

    pub fn remove_tracks(&self, ids: HashSet<i64>) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.remove_tracks(&ids)));
    }

    pub fn move_item(&self, from: usize, to: usize) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.move_item(from, to)));
    }

    pub fn clear(&self) {
        self.queue(|inner| Inner::apply(inner, |p, _| p.clear()));
    }

    pub fn play_index(&self, index: usize) {
        self.queue(move |inner| {
            if inner.playlist.lock().is_missing_at(index) {
                log::warn!(
                    "playlist: item {} cannot air — its track is missing or hidden",
                    index
                );
                return;
            }
            {
                let playlist = inner.playlist.lock();
                inner.timing.lock().prepare_start(
                    &playlist,
                    playlist.occurrence_id_at(index),
                    audio_timing::capture(),
                );
            }
            Inner::apply(inner, move |p, r| p.play_index(index, r));
        });
    }

    /// Put a particular queued child or standalone track on air.
    pub fn play_occurrence(&self, id: String) {
        self.queue(move |inner| {
            {
                let playlist = inner.playlist.lock();
                inner
                    .timing
                    .lock()
                    .prepare_start(&playlist, Some(&id), audio_timing::capture());
            }
            Inner::apply(inner, move |p, r| p.play_occurrence(&id, r));
        });
    }

    pub fn play_now(&self, id: i64) {
        self.queue(move |inner| {
            inner.timing.lock().force_reanchor = true;
            Inner::with_track(inner, id, |p, r, track| p.play_now(track, r));
        });
    }

    pub fn next(&self) {
        self.queue(|inner| {
            {
                let playlist = inner.playlist.lock();
                inner.timing.lock().prepare_start(
                    &playlist,
                    playlist
                        .next_occurrence_id()
                        .or_else(|| playlist.occurrence_id_at(0)),
                    audio_timing::capture(),
                );
            }
            Inner::apply(inner, |p, r| p.next(r));
        });
    }

    /// Step back to the last track that aired, which the playlist reads off its
    /// own history.
    pub fn prev(&self) {
        self.queue(|inner| {
            inner.timing.lock().force_reanchor = true;
            Inner::apply(inner, |p, r| p.prev(r));
        });
    }

    pub fn stop(&self) {
        self.queue(|inner| {
            inner.timing.lock().force_reanchor = true;
            Inner::apply(inner, |p, _| p.stop());
        });
    }

    pub fn set_auto_advance(&self, active: bool) {
        self.queue(move |inner| Inner::apply(inner, move |p, _| p.set_auto_advance(active)));
    }

    pub fn set_auto_playlist(&self, active: bool) {
        self.queue(move |inner| Inner::apply(inner, move |p, r| p.set_auto_playlist(active, r)));
    }
}

/// Delay for retry number `attempt`, saturating on the schedule's last entry so
/// a long outage keeps retrying at a steady pace instead of running off the end.
fn backoff_ms(schedule: &[u64], attempt: usize) -> u64 {
    schedule
        .get(attempt.min(schedule.len().saturating_sub(1)))
        .copied()
        .unwrap_or(DEFAULT_BACKOFF_MS)
}

impl Inner {
    fn lookup(&self, id: i64) -> anyhow::Result<Option<Track>> {
        self.db.get_track(id)
    }

    /// Resolve a track and run a transition that needs its row.
    ///
    /// A row that cannot be read is logged rather than reported: the caller is
    /// the command thread, which has no one left to answer. The renderer only
    /// ever logged these too, and a track that has left the library between the
    /// click and the lookup is not something the operator can act on.
    fn with_track<F>(inner: &Arc<Inner>, id: i64, f: F)
    where
        F: FnOnce(&mut Playlist, &dyn Refiller, Track) -> Transition,
    {
        match inner.lookup(id) {
            Ok(Some(track)) => Inner::apply(inner, move |p, r| f(p, r, track)),
            Ok(None) => log::error!("playlist: track {} is not in the library", id),
            Err(e) => log::error!("playlist: track {} lookup failed: {}", id, e),
        }
    }

    fn resolve_items(&self, state: &SessionState) -> Vec<PlaylistItem> {
        PlaylistItem::from_session(&state.playlist_items, &state.playlist_ids, |id| {
            self.lookup(id).ok().flatten()
        })
    }

    fn resolve_session_item(&self, item: &SessionPlaylistItem) -> Option<PlaylistItem> {
        match item {
            SessionPlaylistItem::Stop {
                planned_duration_seconds,
            } => Some(PlaylistItem::Stop {
                planned_duration_seconds: *planned_duration_seconds,
            }),
            SessionPlaylistItem::Track { id, cue_override } => self
                .lookup(*id)
                .ok()
                .flatten()
                .map(|track| PlaylistItem::with_override(track, *cue_override)),
        }
    }

    fn resolve_queue(&self, nodes: &[SessionQueueNode]) -> Vec<QueueNode> {
        nodes
            .iter()
            .filter_map(|node| match node {
                SessionQueueNode::Standalone { id, item } => {
                    self.resolve_session_item(item)
                        .map(|item| QueueNode::Standalone {
                            id: id.clone(),
                            item,
                        })
                }
                SessionQueueNode::Block {
                    id,
                    title,
                    saved_kind,
                    entries,
                } => {
                    let entries = entries
                        .iter()
                        .filter_map(|child| {
                            self.resolve_session_item(&child.item)
                                .map(|item| QueueChild {
                                    id: child.id.clone(),
                                    item,
                                })
                        })
                        .collect();
                    Some(QueueNode::Block {
                        id: id.clone(),
                        title: title.clone(),
                        saved_kind: *saved_kind,
                        entries,
                    })
                }
            })
            .collect()
    }

    /// Where the auto-playlist draws from for this transition. A source with
    /// no playable music left — deleted, or every music entry missing or
    /// unmatched — is dropped for the music library here, before a refill can
    /// ask it for tracks and get none.
    fn resolve_source(&self) -> (Option<SourceInfo>, Option<String>) {
        let mut source = self.source.lock();
        if let Some((id, known_name)) = source.chosen.clone() {
            match self.db.saved_playlist_pool(id) {
                Ok(Some((name, tracks))) if tracks > 0 => {
                    source.chosen = Some((id, name.clone()));
                    let info = SourceInfo { id, name, tracks };
                    return (Some(info), source.reverted_from.clone());
                }
                Ok(found) => {
                    let name = found.map_or(known_name, |(name, _)| name);
                    log::warn!(
                        "auto-playlist: source \"{name}\" has no playable music; \
                         back to the music library"
                    );
                    source.chosen = None;
                    source.reverted_from = (!name.is_empty()).then_some(name);
                }
                // A failed read says nothing about the pool; keep the choice.
                Err(e) => {
                    log::error!("auto-playlist: reading the source failed: {e:#}");
                    let info = SourceInfo {
                        id,
                        name: known_name,
                        tracks: 0,
                    };
                    return (Some(info), source.reverted_from.clone());
                }
            }
        }
        (None, source.reverted_from.clone())
    }

    fn refiller(&self, pool: Option<i64>) -> DbRefiller<'_> {
        let tuning = self.config.get_tuning();
        DbRefiller {
            db: &self.db,
            pool,
            interleave: generate::Interleave::from_config(&tuning),
            rotation: generate::Rotation::from_config(&tuning),
            buffer: tuning.auto_playlist.auto_playlist_buffer as i64,
            threshold: tuning.auto_playlist.auto_playlist_threshold as i64,
            history_cap: tuning.auto_playlist.history_cap,
        }
    }

    /// Take new markers into the copies of a track held here, if any are. The
    /// analysis pass reports every result it commits — one per track in the
    /// library on a backfill — and [`Inner::apply`] is not cheap: it takes the
    /// playlist lock, reconciles the arm deck, serializes a whole snapshot to
    /// the renderer and re-pushes the prefetch window. A track nobody queued
    /// is dropped before any of that.
    fn adopt_cue_points(inner: &Arc<Inner>, id: i64, points: CuePoints) {
        if !inner.playlist.lock().holds(id) {
            return;
        }
        Inner::apply(inner, move |p, _| p.on_cue_points_saved(id, points));
    }

    /// Take a measured length into the copies of a track held here, under the
    /// same screen as [`Inner::adopt_cue_points`] and for the same reason.
    fn adopt_duration(inner: &Arc<Inner>, id: i64, duration: f64) {
        if !inner.playlist.lock().holds(id) {
            return;
        }
        Inner::apply(inner, move |p, _| p.on_duration_measured(id, duration));
    }

    fn observe(inner: &Arc<Inner>, observation: Observation) {
        let playlist = inner.playlist.lock();
        if !inner.timing.lock().accept(&playlist, observation) {
            return;
        }
        let snapshot = inner.project_snapshot(&playlist);
        let _ = inner.app.emit(PLAYLIST_STATE_EVENT, &snapshot);
    }

    fn project_snapshot(&self, playlist: &Playlist) -> Snapshot {
        let correction = audio_timing::correct_host_clock();
        self.timing
            .lock()
            .project(playlist, audio_timing::capture(), correction)
    }

    /// Run one transition and settle its consequences.
    ///
    /// Two ordering rules, both about re-entrancy. Tauri dispatches to backend
    /// listeners inline, and `set_window` emits `main-deck:cache-state`, which
    /// this service listens to — so a transition can re-enter `apply` before it
    /// returns.
    ///
    /// The playlist lock is therefore released before any effect runs, or the
    /// nested call would deadlock on it. And the snapshot is emitted before the
    /// effects, so the nested transition's snapshot lands *after* this one:
    /// emitting last would have the outer call overwrite the renderer with the
    /// state as it was before the nested advance.
    fn apply<F>(inner: &Arc<Inner>, f: F)
    where
        F: FnOnce(&mut Playlist, &dyn Refiller) -> Transition,
    {
        let (transition, snapshot, window) = {
            let (source, reverted_from) = inner.resolve_source();
            let refiller = inner.refiller(source.as_ref().map(|s| s.id));
            let mut playlist = inner.playlist.lock();
            playlist.set_history_cap(refiller.history_cap());
            playlist.set_source_view(source, reverted_from);
            let mut transition = f(&mut playlist, &refiller);
            if let Some(reason) = transition.rejection {
                log::warn!("playlist: command rejected: {reason}");
            }
            // Bringing the arm deck in line follows every transition rather
            // than each one remembering to ask, for the same reason the
            // prefetch window does: a queue mutation, a track change and a
            // refill all move what comes next.
            transition.effects.extend(playlist.reconcile_arm());
            let snapshot = inner.project_snapshot(&playlist);
            let window = playlist.prefetch_window();
            (transition, snapshot, window)
        };

        let _ = inner.app.emit(PLAYLIST_STATE_EVENT, &snapshot);
        for effect in &transition.effects {
            Inner::run(inner, effect);
        }
        // Re-pushed on every transition, not only when the window changed: the
        // cache worker reads on window updates, so this is also what retries a
        // failed prefetch once the share is back.
        inner.set_window(window);
    }

    fn run(inner: &Arc<Inner>, effect: &Effect) {
        match effect {
            Effect::Play {
                id,
                cue_override,
                bound_dead_air,
            } => {
                if inner.load_deck(*id, *cue_override, 0.0, true, *bound_dead_air) {
                    // Redundant for the audio — the load plays itself once the
                    // bytes are decoded — but it reports "playing" immediately
                    // instead of after a read that may be crossing a network.
                    inner.bus.send_main(Cmd::Play);
                } else {
                    // A purged row can never load; treat it as a failed read
                    // so advancement moves on.
                    Inner::apply(inner, |p, r| {
                        p.on_load_failed(*id, LoadFailure::Unplayable, r)
                    });
                }
            }
            Effect::Resume {
                id,
                seconds,
                cue_override,
            } => {
                // Parked and silent, so nothing is owed air by it.
                inner.load_deck(*id, *cue_override, *seconds, false, false);
            }
            Effect::Stop => inner.bus.send_main(Cmd::Stop),
            Effect::Arm { id, cue_override } => inner.arm_deck(*id, *cue_override),
            Effect::Disarm => inner.bus.send_arm(Cmd::Stop),
            Effect::NowPlaying { id, cue_override } => {
                if let Some(info) = inner.load_info(*id, *cue_override) {
                    inner.broadcast.went_on_air(info.into());
                }
            }
            Effect::TrackPlayed(id) => {
                if let Err(e) = inner.db.record_airing(*id, now_ms()) {
                    log::error!("airing record failed for track {}: {}", id, e);
                }
            }
            Effect::ArmRetry(attempt) => Inner::arm_retry(inner, *attempt),
            Effect::CancelRetry => {
                inner.retry_generation.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// Park the next playlist item on the arm deck, so a handover has something
    /// to hand over to.
    ///
    /// Deliberately does **not** set the broadcast's pending track. An arm-load
    /// happens minutes before the track goes on air, and now-playing fires on
    /// `main-deck:pause-state`, so setting it here would broadcast the incoming
    /// track the moment the operator paused and resumed the *outgoing* one.
    fn arm_deck(&self, id: i64, cue_override: Option<CuePoints>) {
        let Some(info) = self.load_info(id, cue_override) else {
            return;
        };
        let gain = self.replay_gain(&info);
        // Arming is not air: the outgoing track is still playing, which is
        // exactly the slack the stall watchdog is there to use.
        let occurrence_id = self.playlist.lock().next_occurrence_id().map(str::to_owned);
        self.bus.send_arm(Self::load_cmd(
            &info,
            0.0,
            false,
            false,
            gain,
            occurrence_id,
        ));
    }

    /// Resolve a track's row and fold in the markers this airing plays under.
    ///
    /// The item's override if it carries one, the track's radio edit otherwise.
    /// Resolution happens on the thread that starts the load — the worker is
    /// handed the markers to apply and never consults the library itself.
    /// Writing the effective points back onto the load info is also what keeps
    /// the broadcast's `durationSec` reporting the airing rather than the radio
    /// edit.
    fn load_info(&self, id: i64, cue_override: Option<CuePoints>) -> Option<TrackLoadInfo> {
        let mut info = match self.db.get_track_load_info(id) {
            Ok(Some(info)) => info,
            Ok(None) => {
                log::error!("playlist: track {} is no longer in the library", id);
                return None;
            }
            Err(e) => {
                log::error!("playlist: track {} lookup failed: {}", id, e);
                return None;
            }
        };
        if let Some(points) = cue_override {
            info.cue_points = points;
        }
        Some(info)
    }

    /// `start_at` and `autoplay` travel with the load rather than following it
    /// as separate commands: the deck reads the file on a background thread, so
    /// a Seek sent straight after a Load arrives before there is anything to
    /// seek in, and a Play would override a restore that is meant to stay
    /// parked.
    fn load_cmd(
        info: &TrackLoadInfo,
        start_at: f64,
        autoplay: bool,
        bound_dead_air: bool,
        gain: f32,
        occurrence_id: Option<String>,
    ) -> Cmd {
        Cmd::Load {
            id: info.id,
            occurrence_id,
            path: PathBuf::from(info.path.clone()),
            duration: (info.duration > 0.0).then_some(info.duration),
            cue_points: info.cue_points,
            start_at,
            autoplay,
            bound_dead_air,
            gain,
        }
    }

    /// What levelling this track loads at, under the current setting.
    fn replay_gain(&self, info: &TrackLoadInfo) -> f32 {
        levelling::factor(
            self.config.get_tuning().player.replay_gain,
            info.loudness.gain_db,
            info.loudness.peak,
        )
    }

    /// Hand a track to the main deck. Reports whether the load was actually
    /// issued, so a missing row does not leave the caller sending Play at
    /// nothing.
    fn load_deck(
        &self,
        id: i64,
        cue_override: Option<CuePoints>,
        start_at: f64,
        autoplay: bool,
        bound_dead_air: bool,
    ) -> bool {
        let Some(info) = self.load_info(id, cue_override) else {
            return false;
        };
        let cmd = Self::load_cmd(
            &info,
            start_at,
            autoplay,
            bound_dead_air,
            self.replay_gain(&info),
            self.playlist
                .lock()
                .current_occurrence_id()
                .map(str::to_owned),
        );
        self.broadcast.set_pending_track(info.into());
        self.bus.send_main(cmd);
        true
    }

    fn set_window(&self, ids: Vec<i64>) {
        match self.db.get_paths_by_ids(&ids) {
            Ok(paths) => {
                let window = paths
                    .into_iter()
                    .map(|(id, path)| (id, PathBuf::from(path)))
                    .collect();
                self.cache.set_window(window);
            }
            Err(e) => log::error!("prefetch window lookup failed: {}", e),
        }
    }

    /// Sleep out the backoff on a throwaway thread, then re-plan. The schedule's
    /// last entry repeats, so a long outage keeps retrying at a steady pace.
    fn arm_retry(inner: &Arc<Inner>, attempt: usize) {
        let delay = backoff_ms(
            &inner
                .config
                .get_tuning()
                .auto_playlist
                .net_retry_backoffs_ms,
            attempt,
        );
        let generation = inner.retry_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let inner = Arc::clone(inner);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay));
            if inner.retry_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            Inner::apply(&inner, |p, r| p.on_retry_tick(r));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::{SavedEntryInput, TrackInsert};

    struct NoTimingRefill;

    impl Refiller for NoTimingRefill {
        fn generate(&self, _: i64, _: &[&Track]) -> Vec<Track> {
            vec![]
        }
        fn buffer(&self) -> i64 {
            0
        }
        fn threshold(&self) -> i64 {
            0
        }
        fn history_cap(&self) -> usize {
            100
        }
    }

    fn timing_track(id: i64, duration: f64) -> Track {
        Track {
            id,
            duration,
            ..Default::default()
        }
    }

    fn capture(seconds: f64) -> audio_timing::Capture {
        audio_timing::Capture {
            monotonic_seconds: seconds,
            unix_ms: 1_000_000.0 + seconds * 1000.0,
        }
    }

    fn observation(
        playlist: &Playlist,
        sequence: u64,
        seconds: f64,
        position: f64,
        playing: bool,
    ) -> Observation {
        Observation {
            sequence,
            instance_id: format!(
                "instance-{}",
                playlist.current_occurrence_id().unwrap_or_default()
            ),
            occurrence_id: playlist.current_occurrence_id().map(str::to_owned),
            track_id: playlist.snapshot().current.as_ref().map(|track| track.id),
            captured: capture(seconds),
            position_seconds: position,
            duration_seconds: Some(10.4),
            next_start_seconds: Some(10.4),
            playing,
            buffering: false,
            interrupted: false,
            tail_remaining_seconds: 0.0,
            seek_revision: 0,
        }
    }

    fn displayed(snapshot: &Snapshot, seconds: f64) -> Vec<Option<i64>> {
        let delay = (snapshot.timing.delay_seconds
            + snapshot.timing.delay_started_at_unix_ms.map_or(0.0, |at| {
                ((capture(seconds).unix_ms - at) / 1000.0).max(0.0)
            }))
        .floor() as i64;
        snapshot
            .timing
            .entries
            .iter()
            .map(|entry| entry.base_second.map(|base| base + delay))
            .collect()
    }

    #[test]
    fn operator_fade_completions_revise_the_schedule_at_the_actual_transition() {
        for action in ["handover", "end", "stop"] {
            let mut playlist = Playlist::new();
            let mut current = timing_track(1, 100.0);
            current.cue_points.next_start_ms = Some(90_000);
            playlist.add(current, None);
            playlist.add(timing_track(2, 10.0), None);
            playlist.add(timing_track(3, 10.0), None);
            playlist.next(&NoTimingRefill);
            playlist.on_loaded(1);
            let mut timing = TimingState::default();
            let mut started = observation(&playlist, 1, 0.0, 0.0, true);
            started.duration_seconds = Some(100.0);
            started.next_start_seconds = Some(90.0);
            timing.accept(&playlist, started);
            let initial = timing.project(&playlist, capture(0.0), 0.0);
            assert_eq!(displayed(&initial, 0.0), vec![Some(1090), Some(1100)]);
            match action {
                "handover" => {
                    let event: HandoverEvent =
                        serde_json::from_str(r#"{"to":2,"manual":true}"#).unwrap();
                    timing.manual_transition(event.manual);
                    playlist.on_handover(event.to, &NoTimingRefill);
                }
                "end" => {
                    let event: EndEvent = serde_json::from_str(r#"{"manual":true}"#).unwrap();
                    timing.manual_transition(event.manual);
                    playlist.on_ended(&NoTimingRefill);
                }
                _ => {
                    timing.manual_transition(true);
                    playlist.stop();
                }
            }
            let revised = timing.project(&playlist, capture(10.0), 0.0);
            let expected = if action == "stop" {
                vec![Some(1010), Some(1020)]
            } else {
                vec![Some(1020)]
            };
            assert_eq!(displayed(&revised, 10.0), expected, "{action}");
        }
    }

    #[test]
    fn transport_start_after_an_unplanned_hold_freezes_fractional_delay_without_a_jump() {
        let mut playlist = Playlist::new();
        playlist.restore_hold(
            HoldingStop {
                occurrence_id: "hold".into(),
                planned_duration_seconds: None,
                elapsed_seconds: 0.0,
                captured_at_unix_ms: 0.0,
                block_id: None,
            },
            None,
        );
        playlist.add(timing_track(1, 2.6), None);
        playlist.add(timing_track(2, 2.6), None);
        let mut timing = TimingState::default();
        timing.project(&playlist, capture(0.6), 0.0);
        let before = timing.project(&playlist, capture(4.4), 0.0);
        let target = playlist.next_occurrence_id().unwrap().to_owned();
        timing.prepare_start(&playlist, Some(&target), capture(4.4));
        assert!(!timing.force_reanchor);
        playlist.next(&NoTimingRefill);
        timing.project(&playlist, capture(4.4), 0.0);
        let mut started = observation(&playlist, 1, 4.45, 0.0, true);
        started.duration_seconds = Some(2.6);
        started.next_start_seconds = Some(2.6);
        timing.accept(&playlist, started);
        let after = timing.project(&playlist, capture(8.0), 0.0);
        assert_eq!(displayed(&before, 4.4)[1..], displayed(&after, 8.0));
        assert_eq!(displayed(&after, 8.0), vec![Some(1006)]);
    }

    #[test]
    fn starting_before_a_planned_hold_deadline_revises_the_schedule() {
        let mut playlist = Playlist::new();
        playlist.restore_hold(
            HoldingStop {
                occurrence_id: "hold".into(),
                planned_duration_seconds: Some(120),
                elapsed_seconds: 0.0,
                captured_at_unix_ms: 0.0,
                block_id: None,
            },
            None,
        );
        playlist.add(timing_track(1, 2.6), None);
        let mut timing = TimingState::default();
        timing.project(&playlist, capture(0.0), 0.0);
        timing.prepare_start(&playlist, playlist.next_occurrence_id(), capture(5.0));
        assert!(timing.force_reanchor);
    }

    #[test]
    fn coherent_parked_restore_revises_the_preload_estimate_to_its_saved_position() {
        let mut playlist = Playlist::new();
        playlist.hydrate(
            vec![PlaylistItem::track(timing_track(2, 10.0))],
            Some(timing_track(1, 100.0)),
            None,
            50.0,
            false,
            true,
            vec![],
        );
        let mut timing = TimingState::default();
        timing.project(&playlist, capture(0.0), 0.0);
        let mut parked = observation(&playlist, 1, 0.05, 50.0, false);
        parked.duration_seconds = Some(100.0);
        parked.next_start_seconds = Some(100.0);
        timing.accept(&playlist, parked);
        let restored = timing.project(&playlist, capture(0.5), 0.0);
        assert_eq!(displayed(&restored, 0.5), vec![Some(1050)]);
    }

    #[test]
    fn captured_pause_and_resume_ignore_delivery_latency_and_stale_observations() {
        let mut playlist = Playlist::new();
        playlist.add(timing_track(1, 10.4), None);
        playlist.add(timing_track(2, 10.4), None);
        playlist.next(&NoTimingRefill);
        let mut timing = TimingState::default();
        assert!(timing.accept(&playlist, observation(&playlist, 1, 0.0, 0.0, true)));
        let initial = timing.project(&playlist, capture(3.0), 0.0);
        assert_eq!(displayed(&initial, 3.0), vec![Some(1010)]);

        assert!(timing.accept(&playlist, observation(&playlist, 2, 2.0, 2.0, false)));
        let paused = timing.project(&playlist, capture(5.0), 0.0);
        assert_eq!(displayed(&paused, 5.0), vec![Some(1013)]);
        assert!(!timing.accept(&playlist, observation(&playlist, 1, 1.0, 1.0, true)));
        assert!(timing.accept(&playlist, observation(&playlist, 3, 5.4, 2.0, true)));
        let resumed = timing.project(&playlist, capture(8.0), 0.0);
        assert_eq!(displayed(&resumed, 8.0), vec![Some(1013)]);
        assert_eq!(
            displayed(&timing.project(&playlist, capture(9.0), 0.0), 9.0),
            vec![Some(1013)]
        );
    }

    #[test]
    fn normal_consumption_and_late_actual_start_keep_the_same_schedule() {
        let mut playlist = Playlist::new();
        for id in 1..=4 {
            playlist.add(timing_track(id, 10.4), None);
        }
        playlist.next(&NoTimingRefill);
        let mut timing = TimingState::default();
        timing.accept(&playlist, observation(&playlist, 1, 0.0, 0.0, true));
        let initial = timing.project(&playlist, capture(0.0), 0.0);
        playlist.on_loaded(1);
        playlist.on_ended(&NoTimingRefill);
        let pending = timing.project(&playlist, capture(10.45), 0.0);
        assert_eq!(displayed(&pending, 10.45), displayed(&initial, 10.45)[1..]);
        let mut loading = observation(&playlist, 2, 10.45, 0.0, false);
        loading.buffering = true;
        timing.accept(&playlist, loading);
        timing.project(&playlist, capture(13.0), 0.0);
        timing.accept(&playlist, observation(&playlist, 3, 10.5, 0.0, true));
        let started = timing.project(&playlist, capture(15.0), 0.0);
        assert_eq!(displayed(&started, 15.0), displayed(&initial, 15.0)[1..]);
        assert!(!timing.force_reanchor);
    }

    #[test]
    fn a_late_observation_for_a_previous_airing_of_the_same_track_is_rejected() {
        let mut playlist = Playlist::new();
        playlist.add(timing_track(1, 10.4), None);
        playlist.add(timing_track(1, 10.4), None);
        playlist.next(&NoTimingRefill);
        let old = observation(&playlist, 5, 0.0, 0.0, true);
        playlist.next(&NoTimingRefill);
        let mut timing = TimingState::default();
        assert!(!timing.accept(&playlist, old));
        assert!(timing.accept(&playlist, observation(&playlist, 6, 1.0, 0.0, true)));
    }

    #[test]
    fn held_stop_restores_saved_elapsed_without_counting_time_closed() {
        let mut playlist = Playlist::new();
        playlist.add(timing_track(2, 10.4), None);
        playlist.restore_hold(
            HoldingStop {
                occurrence_id: "hold".into(),
                planned_duration_seconds: Some(120),
                elapsed_seconds: 53.0,
                captured_at_unix_ms: 0.0,
                block_id: None,
            },
            None,
        );
        let mut timing = TimingState::default();
        let opened = timing.project(&playlist, capture(1800.0), 0.0);
        assert_eq!(opened.holding_stop.unwrap().elapsed_seconds, 53.0);
        let later = timing.project(&playlist, capture(1802.5), 0.0);
        assert_eq!(later.holding_stop.as_ref().unwrap().elapsed_seconds, 55.5);
        assert_eq!(displayed(&later, 1802.5), vec![Some(2867)]);
    }

    #[test]
    fn planned_hold_deadline_does_not_move_when_upcoming_is_edited() {
        let mut playlist = Playlist::new();
        playlist.restore_hold(
            HoldingStop {
                occurrence_id: "hold".into(),
                planned_duration_seconds: Some(120),
                elapsed_seconds: 0.0,
                captured_at_unix_ms: 0.0,
                block_id: None,
            },
            None,
        );
        playlist.add(timing_track(1, 10.4), None);
        let mut timing = TimingState::default();
        let initial = timing.project(&playlist, capture(0.0), 0.0);
        playlist.add(timing_track(2, 10.4), None);
        let edited = timing.project(&playlist, capture(60.5), 0.0);
        assert_eq!(displayed(&initial, 60.5)[0], displayed(&edited, 60.5)[0]);
        assert_eq!(displayed(&edited, 125.5), vec![Some(1125), Some(1135)]);
        assert_eq!(edited.holding_stop.unwrap().elapsed_seconds, 60.5);
    }

    #[test]
    fn a_queued_copy_keeps_its_title_kind_and_entries_after_the_saved_original_changes() {
        let db = Db::open_in_memory().unwrap();
        for (id, title) in [(1, "A"), (2, "B")] {
            db.insert_track(&TrackInsert {
                path: format!("/{id}.mp3"),
                content_type: "music".into(),
                title: Some(title.into()),
                ..TrackInsert::default()
            })
            .unwrap();
        }
        let made = db
            .create_saved_playlist_with_entries(
                "Friday",
                SavedPlaylistKind::PreparedShow,
                &[
                    SavedEntryInput::Track(1),
                    SavedEntryInput::Stop {
                        planned_duration_seconds: None,
                    },
                    SavedEntryInput::Track(2),
                ],
                1,
            )
            .unwrap();
        let copy = saved_copy(db.saved_playlist(made.id).unwrap().unwrap());
        db.rename_saved_playlist(made.id, "Saturday", 2).unwrap();
        db.delete_saved_playlist(made.id).unwrap();

        assert_eq!(copy.0, "Friday");
        assert_eq!(copy.1, SavedPlaylistKind::PreparedShow);
        assert_eq!(
            copy.2,
            [
                SavedQueueEntry::Track {
                    id: 1,
                    content_type: "music".into()
                },
                SavedQueueEntry::Stop {
                    planned_duration_seconds: None
                },
                SavedQueueEntry::Track {
                    id: 2,
                    content_type: "music".into()
                },
            ]
        );
        assert_eq!(copy.3, 0);
    }

    #[test]
    fn authored_show_keeps_stops_and_repeated_tracks_when_an_adjacent_file_is_gone() {
        let entries = vec![
            SavedQueueEntry::Track {
                id: 1,
                content_type: "music".into(),
            },
            SavedQueueEntry::Track {
                id: 99,
                content_type: "music".into(),
            },
            SavedQueueEntry::Stop {
                planned_duration_seconds: None,
            },
            SavedQueueEntry::Track {
                id: 1,
                content_type: "music".into(),
            },
        ];
        let items = authored_items(
            entries,
            vec![Track {
                id: 1,
                cue_points: CuePoints {
                    cue_in_ms: Some(100),
                    ..Default::default()
                },
                ..Default::default()
            }],
        );

        assert_eq!(
            items
                .iter()
                .map(|item| item.as_track().map(|track| track.id))
                .collect::<Vec<_>>(),
            vec![Some(1), None, Some(1)]
        );
        for item in items.iter().filter(|item| item.as_track().is_some()) {
            let PlaylistItem::Track {
                track,
                cue_override,
            } = item
            else {
                unreachable!()
            };
            assert_eq!(track.cue_points.cue_in_ms, Some(100));
            assert_eq!(*cue_override, None);
        }
    }

    /// Posted jobs record themselves here, so a test can read the order they ran
    /// in. Stands in for the playlist the real jobs mutate.
    type Log = Arc<Mutex<Vec<usize>>>;

    #[test]
    fn jobs_run_in_the_order_they_were_posted() {
        let log: Log = Arc::new(Mutex::new(vec![]));
        let serial = Serial::spawn(Arc::clone(&log));
        let (done_tx, done) = channel();
        for i in 0..200 {
            serial.post(move |log: &Log| log.lock().push(i));
        }
        serial.post(move |_: &Log| done_tx.send(()).unwrap());

        done.recv().unwrap();
        assert_eq!(*log.lock(), (0..200).collect::<Vec<_>>());
    }

    /// The property the transport depends on: posting is what a Tauri command
    /// does on the main thread, and it may not wait for the work.
    #[test]
    fn posting_does_not_wait_for_the_job() {
        let log: Log = Arc::new(Mutex::new(vec![]));
        let serial = Serial::spawn(Arc::clone(&log));
        let (release_tx, release) = channel::<()>();
        let (done_tx, done) = channel::<()>();

        serial.post(move |log: &Log| {
            release.recv().unwrap();
            log.lock().push(1);
            done_tx.send(()).unwrap();
        });
        // The first job is still parked on `release`, so this call proves
        // posting returned without it having run.
        assert!(log.lock().is_empty());

        release_tx.send(()).unwrap();
        done.recv().unwrap();
        assert_eq!(*log.lock(), vec![1]);
    }

    /// What shutdown relies on: everything posted before the drain has run by the
    /// time it returns, so a transition still owing the airing log a play is not
    /// lost with the process.
    #[test]
    fn draining_waits_for_what_was_posted() {
        let log: Log = Arc::new(Mutex::new(vec![]));
        let serial = Serial::spawn(Arc::clone(&log));
        for i in 0..50 {
            serial.post(move |log: &Log| log.lock().push(i));
        }

        assert!(serial.drain(Duration::from_secs(5)));
        assert_eq!(*log.lock(), (0..50).collect::<Vec<_>>());
    }

    /// A job that never finishes must not hold the window open.
    #[test]
    fn draining_gives_up_on_a_wedged_job() {
        let log: Log = Arc::new(Mutex::new(vec![]));
        let serial = Serial::spawn(Arc::clone(&log));
        let (release_tx, release) = channel::<()>();
        serial.post(move |_: &Log| {
            let _ = release.recv();
        });

        assert!(!serial.drain(Duration::from_millis(50)));
        release_tx.send(()).unwrap();
    }

    /// A panicking transition must not take the transport with it.
    #[test]
    fn a_panicking_job_leaves_the_queue_running() {
        let log: Log = Arc::new(Mutex::new(vec![]));
        let serial = Serial::spawn(Arc::clone(&log));
        let (done_tx, done) = channel();

        serial.post(|_: &Log| panic!("as a queued command might"));
        serial.post(move |log: &Log| {
            log.lock().push(1);
            done_tx.send(()).unwrap();
        });

        done.recv().unwrap();
        assert_eq!(*log.lock(), vec![1]);
    }

    #[test]
    fn backoff_walks_the_schedule() {
        let schedule = [1000, 2000, 5000];
        assert_eq!(backoff_ms(&schedule, 0), 1000);
        assert_eq!(backoff_ms(&schedule, 1), 2000);
        assert_eq!(backoff_ms(&schedule, 2), 5000);
    }

    #[test]
    fn backoff_saturates_on_the_last_entry() {
        let schedule = [1000, 2000, 5000];
        assert_eq!(backoff_ms(&schedule, 3), 5000);
        assert_eq!(backoff_ms(&schedule, 99), 5000);
    }

    #[test]
    fn backoff_falls_back_when_the_schedule_is_empty() {
        assert_eq!(backoff_ms(&[], 0), DEFAULT_BACKOFF_MS);
        assert_eq!(backoff_ms(&[], 7), DEFAULT_BACKOFF_MS);
    }
}
