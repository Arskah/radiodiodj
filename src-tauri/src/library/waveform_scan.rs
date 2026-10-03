//! Background waveform and fingerprint computation, decoupled from the
//! metadata scan.
//!
//! The metadata scan (tag reads) is fast and finishes quickly. Computing a
//! track's amplitude curve requires a full audio decode, which is far heavier —
//! so it runs here, on its own threads, after the scan. The same pass
//! backfills [fingerprints](crate::audio_measure::fingerprint): from the bytes
//! already read when a waveform is due, from the head of the file otherwise.
//! The same decode also yields the
//! [automatic cue points](crate::library::auto_cue), so an unprepped track
//! airs trimmed without a second pass over the file. Waveforms land in the DB
//! one at a time and a `waveform-ready` event is emitted per track so the
//! renderer can refresh a curve for the deck that is currently showing it.
//!
//! **One thread reads and several decode.** The library is usually a network
//! share, and the rule `audio/cache.rs` follows for prefetch is the share's
//! rule rather than that worker's: concurrent reads are how a share that was
//! merely slow becomes a share that is down. So the pass pulls one file at a
//! time and hands it to a pool of decoders over a one-slot channel — the CPU
//! fan-out costs the share nothing. Every file access the pass makes happens on
//! the reader, including the head read behind a fingerprint-only job.
//!
//! Progress is surfaced separately from the metadata scan via
//! `waveform-progress` / `waveform-state-changed` so the UI can show a second
//! bar under the tag-scan bar. Like the metadata scan, the `processed`/`total`
//! counts are cumulative across all libraries (the worker drains one flat
//! missing-waveform list spanning every library).
//!
//! The job is single-flight (only one worker at a time) and cancelable. A
//! cancel stops the reader after the file it is on and each decoder after the
//! file it holds, leaving anything read but not yet started undecoded; it is
//! consumed by that pass: it means "not right now", so the next kick — a scan, a
//! reclassification, a launch — starts a fresh one. It drains
//! [`Db::tracks_needing_analysis`] in a loop so tracks added while it runs
//! are still picked up. A file that cannot be decoded is recorded in the DB and
//! left alone until a scan sees it change; one that merely could not be read
//! (a share dropping out) is skipped for the rest of the run and tried again on
//! the next.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::db::{AnalysisJob, Db};
use super::scanner::now_ms;
use crate::audio::cue_points::CuePoints;
use crate::audio_measure::fingerprint;
use crate::audio_measure::{level_envelope, loudness, waveform};
use crate::library::auto_cue;
use crate::persist::config::Config;

type Bytes = Arc<[u8]>;

/// Event emitted after a track's waveform is stored. Payload is the track id.
const WAVEFORM_READY_EVENT: &str = "waveform-ready";
/// Event emitted after an automatic cue result is committed, carrying the whole
/// stored set. Every copy of a track held outside the DB — the playlist's queued
/// items, the renderer's rows — derives its duration from these markers, and a
/// backfill changes them under all of them.
pub const CUE_POINTS_READY_EVENT: &str = "cue-points-ready";
/// Throttled `{processed, total}` progress updates.
const WAVEFORM_PROGRESS_EVENT: &str = "waveform-progress";
/// Running/idle transitions.
const WAVEFORM_STATE_EVENT: &str = "waveform-state-changed";
const PROGRESS_THROTTLE: Duration = Duration::from_millis(200);
/// Whole files the pass may hold in RAM at once: one per decode worker, one
/// queued ahead of them, and one in the reader's hand. A single track can be
/// 100 MB, so this — not the worker count — is the number that bounds the pass.
const MAX_RESIDENT_FILES: usize = 8;
/// Files the reader may run ahead of the decoders. One slot keeps a decoder
/// from waiting on the share for bytes the share has already sent; every
/// further slot is another whole file resident for no extra throughput.
const READAHEAD: usize = 1;
/// Hard ceiling on parallel decode workers. Decoding is CPU-heavy and each file
/// is independent, so we fan out across cores; the actual worker count is
/// `cores - 2` (reserving headroom for playback/UI) clamped into `2..=MAX`. The
/// share is not what this protects — the reader does that by being one thread —
/// so what bounds it is [`MAX_RESIDENT_FILES`].
const MAX_CONCURRENCY: usize = MAX_RESIDENT_FILES - READAHEAD - 1;
/// Cores held back from the decode pool so audio playback and the UI stay
/// responsive when a backfill runs mid-set.
const RESERVED_CORES: usize = 2;
/// How long the reader waits for a free decoder before looking at the cancel
/// flag again. A blocking hand-off would hold a cancel for a whole decode.
const HANDOFF_POLL: Duration = Duration::from_millis(25);

/// Progress of the background waveform pass, mirrored to the UI. Cumulative
/// across all libraries.
#[derive(Serialize, Clone, Default, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum WaveformStatus {
    #[default]
    Idle,
    #[serde(rename_all = "camelCase")]
    Running { processed: usize, total: usize },
}

#[derive(Serialize, Clone)]
struct WaveformProgress {
    processed: usize,
    total: usize,
}

/// Payload of [`CUE_POINTS_READY_EVENT`].
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CuePointsReady {
    pub id: i64,
    pub cue_points: CuePoints,
}

#[derive(Default)]
pub struct WaveformJob {
    running: AtomicBool,
    /// Work arrived while a worker was running. See [`WaveformJob::start`].
    kicked: AtomicBool,
    cancel: AtomicBool,
    status: Mutex<WaveformStatus>,
}

impl WaveformJob {
    /// Current progress, for hydration when the UI mounts mid-run.
    pub fn status(&self) -> WaveformStatus {
        self.status.lock().clone()
    }

    /// Request the running worker (if any) to stop after the current file.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Kick the worker. No-op if one is already running (single-flight): the
    /// running worker re-drains the work list on each pass, so it will observe
    /// any tracks a concurrent scan just added.
    ///
    /// A kick that arrives after the running worker's last drain found nothing
    /// is recorded rather than dropped — otherwise work queued in that window
    /// (a reclassification, say) would wait for the next scan or restart.
    pub fn start(self: Arc<Self>, app: AppHandle, db: Arc<Db>, config: Arc<Config>) {
        // Claim the single-flight slot, or leave a note for the worker holding
        // it, which may be past the point of noticing on its own.
        if self.running.swap(true, Ordering::SeqCst) {
            self.kicked.store(true, Ordering::SeqCst);
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            let emit = |next: WaveformStatus| self.set_status(&app, next);
            self.drain_loop(|| run(&self, &app, &db, &config), emit);
        });
    }

    /// Drain until nothing asks for another pass, releasing the single-flight
    /// slot however each one ends — a panic included.
    ///
    /// Nothing else on this thread catches, so a panic would otherwise unwind
    /// past [`WaveformJob::finish`] and leave `running` claimed for the life of
    /// the process: every later kick answers "one is already running" and the
    /// bar sits where it stopped. The panic itself is already logged by the hook
    /// `lib.rs` installs, backtrace and all.
    ///
    /// A caught panic therefore has to do what the pass no longer can: put the
    /// bar back down if it had put one up, and answer for the cancel it would
    /// have consumed. A panic under a cancel is reported as [`Stop::Cancelled`],
    /// so the flag is consumed exactly where a stopped pass consumes it and the
    /// stop is not overridden by a kick that was waiting behind it.
    ///
    /// `pass` and `emit` are injected so the loop can be driven without a
    /// filesystem or an `AppHandle`, in the same spirit as [`read_loop`].
    fn drain_loop(&self, mut pass: impl FnMut() -> Stop, mut emit: impl FnMut(WaveformStatus)) {
        loop {
            // Cleared before the drain, so a kick during it is never lost.
            self.kicked.store(false, Ordering::SeqCst);
            let stop = match catch_unwind(AssertUnwindSafe(&mut pass)) {
                Ok(stop) => stop,
                Err(_) => {
                    log::error!("waveform: the analysis pass panicked; releasing the slot");
                    // Only if the pass got as far as announcing work: an Idle
                    // that no Running preceded is a transition the topic does
                    // not have.
                    if matches!(self.status(), WaveformStatus::Running { .. }) {
                        emit(WaveformStatus::Idle);
                    }
                    if self.cancel.load(Ordering::SeqCst) {
                        Stop::Cancelled
                    } else {
                        Stop::Drained
                    }
                }
            };
            if !self.finish(stop) {
                break;
            }
        }
    }

    /// Release the single-flight slot after a drain and say whether to take it
    /// back and drain again.
    ///
    /// A cancel is consumed here, by the pass it stopped, while the slot is
    /// still held — so no other pass can be in flight to have it taken from
    /// underneath. A cancel therefore means "not right now": the next kick
    /// starts a fresh pass instead of being swallowed until the next launch.
    fn finish(&self, stop: Stop) -> bool {
        let cancelled = matches!(stop, Stop::Cancelled);
        if cancelled {
            self.cancel.store(false, Ordering::SeqCst);
        }
        self.running.store(false, Ordering::SeqCst);
        !cancelled && self.claim_rerun()
    }

    /// Whether the worker that has just released the slot should take it back
    /// and drain again: only for a kick it has not already served, and only if
    /// it is not cancelled and no other kick claimed the free slot first —
    /// that one owns the work from here.
    fn claim_rerun(&self) -> bool {
        // Cancel is read first so a kick is not consumed and then thrown away:
        // it stands, and the next start() serves it.
        !self.cancel.load(Ordering::SeqCst)
            && self.kicked.swap(false, Ordering::SeqCst)
            && !self.running.swap(true, Ordering::SeqCst)
    }

    fn set_status(&self, app: &AppHandle, next: WaveformStatus) {
        *self.status.lock() = next.clone();
        let _ = app.emit(WAVEFORM_STATE_EVENT, &next);
    }
}

/// Why a drain loop ended.
enum Stop {
    /// Nothing left to analyse, or the work list could not be read.
    Drained,
    /// The operator stopped the pass.
    Cancelled,
}

fn run(job: &WaveformJob, app: &AppHandle, db: &Db, config: &Config) -> Stop {
    // Ids that could not be read or stored this run — skipped on subsequent
    // passes so the drain loop cannot spin on them. Written by the reader (a
    // file it could not read) and by the decoders (a store that failed).
    let failed: Mutex<HashSet<i64>> = Mutex::new(HashSet::new());
    let progress = Progress::new();
    // Balanced pool: use the cores that exist minus a reserve for playback/UI,
    // never fewer than 2, never more than the MAX_CONCURRENCY ceiling.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let decoders = cores
        .saturating_sub(RESERVED_CORES)
        .clamp(2, MAX_CONCURRENCY);
    let mut started = false;

    let stop = loop {
        if job.cancel.load(Ordering::SeqCst) {
            break Stop::Cancelled;
        }
        let missing = match db.tracks_needing_analysis() {
            Ok(m) => m,
            Err(e) => {
                log::error!("waveform: query missing failed: {}", e);
                break Stop::Drained;
            }
        };
        let pending: Vec<AnalysisJob> = {
            let f = failed.lock();
            missing
                .into_iter()
                .filter(|job| !f.contains(&job.id))
                .collect()
        };
        if pending.is_empty() {
            break Stop::Drained;
        }

        // Announce Running only once real work exists — avoids a bar flash when
        // every track already has a waveform.
        if !started {
            started = true;
            job.set_status(
                app,
                WaveformStatus::Running {
                    processed: 0,
                    total: pending.len(),
                },
            );
        }

        // Total for this pass. It can grow across passes if a concurrent scan
        // adds tracks; `processed` only ever climbs.
        progress.begin_pass(pending.len());
        run_pass(job, app, db, config, pending, &progress, &failed, decoders);
    };

    if started {
        job.set_status(app, WaveformStatus::Idle);
    }
    stop
}

/// One drain's worth of work: read on this thread, decode on `decoders`.
///
/// The reader is deliberately singular. `docs/audio.md` states the rule for the
/// prefetch cache — hammering a network share with concurrent reads is how a
/// share that was merely slow becomes a share that is down — and the rule
/// belongs to the share, not to that worker. Decoding is what fans out.
#[allow(clippy::too_many_arguments)]
fn run_pass(
    job: &WaveformJob,
    app: &AppHandle,
    db: &Db,
    config: &Config,
    pending: Vec<AnalysisJob>,
    progress: &Progress,
    failed: &Mutex<HashSet<i64>>,
    decoders: usize,
) {
    let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
    let rx = Mutex::new(rx);
    std::thread::scope(|scope| {
        for _ in 0..decoders {
            scope.spawn(|| {
                decode_loop(
                    &rx,
                    &job.cancel,
                    |fetched| {
                        let Fetched { track, payload } = fetched;
                        match analyse(&track, payload, db, app, config) {
                            Outcome::Done => {}
                            Outcome::Retry => {
                                failed.lock().insert(track.id);
                            }
                            Outcome::Unreadable(error) => {
                                unreadable(db, failed, track.id, &error);
                            }
                        }
                        tick(progress, job, app);
                    },
                    |track| {
                        unreadable(db, failed, track.id, PANICKED_DECODE);
                        tick(progress, job, app);
                    },
                );
            });
        }
        // The reader runs on this thread, and dropping `tx` when it returns is
        // what tells the decoders nothing more is coming.
        read_loop(
            pending,
            tx,
            &job.cancel,
            |track| {
                failed.lock().insert(track.id);
                tick(progress, job, app);
            },
            |track| {
                unreadable(db, failed, track.id, PANICKED_READ);
                tick(progress, job, app);
            },
            fetch,
        );
    });
}

/// Count one disposed job: mirror it into the job's status, and emit progress
/// if the throttle window has elapsed.
///
/// Status is written directly rather than through `set_status`, which would
/// also emit `waveform-state-changed` — that topic is for running/idle
/// transitions, not for every file.
fn tick(progress: &Progress, job: &WaveformJob, app: &AppHandle) {
    let (processed, total, due) = progress.record(Instant::now());
    *job.status.lock() = WaveformStatus::Running { processed, total };
    if due {
        let _ = app.emit(
            WAVEFORM_PROGRESS_EVENT,
            WaveformProgress { processed, total },
        );
    }
}

/// Cumulative progress across every pass of one run.
struct Progress {
    processed: AtomicUsize,
    total: AtomicUsize,
    last_emit: Mutex<Instant>,
}

impl Progress {
    fn new() -> Self {
        Self {
            processed: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            last_emit: Mutex::new(
                Instant::now()
                    .checked_sub(PROGRESS_THROTTLE)
                    .unwrap_or_else(Instant::now),
            ),
        }
    }

    /// Start a pass: the denominator is what has been done plus what is queued,
    /// so it grows when a concurrent scan adds work and never shrinks.
    fn begin_pass(&self, pending: usize) -> usize {
        let total = self.processed.load(Ordering::Relaxed) + pending;
        self.total.store(total, Ordering::Relaxed);
        total
    }

    /// Count one disposed job. Returns the snapshot to publish and whether the
    /// throttle window has elapsed; `now` is injected so it is testable.
    fn record(&self, now: Instant) -> (usize, usize, bool) {
        let processed = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
        let total = self.total.load(Ordering::Relaxed);
        let mut last = self.last_emit.lock();
        let due = now.saturating_duration_since(*last) >= PROGRESS_THROTTLE;
        if due {
            *last = now;
        }
        (processed, total, due)
    }
}

/// What the reader has to pull off the share for one job.
enum Fetch {
    /// The whole file: something here needs a decode.
    Whole,
    /// The head only, for a job that wants nothing but a fingerprint.
    Head,
}

/// Which read one job needs. A job with no decode work left is a fingerprint
/// backfill — every track in the library becomes one after a
/// [`fingerprint::VERSION`] bump, which is the case this pass most has to stay
/// polite for.
fn fetch_kind(job: &AnalysisJob) -> Fetch {
    if job.needs_waveform
        || job.needs_loudness
        || job.needs_auto_cue
        || job.needs_auto_cue_levels
        || job.needs_bpm
        || job.needs_key
        || job.needs_duration
    {
        Fetch::Whole
    } else {
        Fetch::Head
    }
}

/// What the reader pulled off the share for one job.
enum Payload {
    /// The whole file, for a job that needs a decode.
    Whole(Bytes),
    /// A job that needed a fingerprint and nothing else. The head read and the
    /// hash both happen on the reader thread — a sha256 over a megabyte is
    /// nothing beside the read it is attached to, and keeping it there is what
    /// makes "one analysis read at a time" true rather than nearly true. `Err`
    /// is the file's fault: a read failure never gets this far.
    Fingerprint(Result<String, String>),
}

/// One job's bytes, off the share and on the way to a decode worker.
struct Fetched {
    track: AnalysisJob,
    payload: Payload,
}

/// What a decode that brought its thread down records, where one that returned
/// an error would have recorded the error.
const PANICKED_DECODE: &str = "panicked while decoding";
/// The same, for a read that brought the reader down.
const PANICKED_READ: &str = "panicked while reading";

/// Record that a file was read but cannot be turned into an analysis, keeping it
/// out of this run if the row refuses the mark.
///
/// A panicking file is marked rather than only added to `failed`, which is
/// per-run: without the mark the pass would pull the same file across the share
/// on every launch and panic on it again. It is the disposition a decode that
/// returned an error already gets, and it comes back the same way — when a scan
/// sees the file change.
fn unreadable(db: &Db, failed: &Mutex<HashSet<i64>>, id: i64, error: &str) {
    if let Err(e) = db.set_analysis_failed(id, error, now_ms()) {
        log::error!("analysis: store failure {} failed: {}", id, e);
        failed.lock().insert(id);
    }
}

/// Pull one job off the share. `Err` is a read failure — the share's fault, and
/// worth another try on the next run.
fn fetch(track: &AnalysisJob) -> Result<Payload, String> {
    match fetch_kind(track) {
        Fetch::Whole => std::fs::read(&track.path)
            .map(|v| Payload::Whole(Arc::from(v.into_boxed_slice())))
            .map_err(|e| e.to_string()),
        Fetch::Head => match fingerprint::of_file(Path::new(&track.path)) {
            Ok(fp) => Ok(Payload::Fingerprint(Ok(fp))),
            Err(e) if fingerprint::is_read_error(&e) => Err(format!("{e:#}")),
            Err(e) => Ok(Payload::Fingerprint(Err(format!("fingerprint: {e:#}")))),
        },
    }
}

/// Read `jobs` off the share one at a time and hand each to a decoder.
///
/// `read` is injected so the loop can be driven without a filesystem, in the
/// same spirit as `player::read_with_retry`. `dropped` disposes of a job whose
/// read failed and `panicked` one whose read panicked; neither reaches a
/// decoder.
///
/// A panic is caught per file because the reader runs on the scope's parent
/// thread: uncaught it takes `run_pass` down directly, without a join to be
/// re-raised at. A fingerprint-only job is the realistic way in — it hands the
/// file to symphonia here, before any decoder sees it.
fn read_loop<R, D, P>(
    jobs: Vec<AnalysisJob>,
    tx: SyncSender<Fetched>,
    cancel: &AtomicBool,
    mut dropped: D,
    mut panicked: P,
    mut read: R,
) where
    R: FnMut(&AnalysisJob) -> Result<Payload, String>,
    D: FnMut(&AnalysisJob),
    P: FnMut(&AnalysisJob),
{
    for track in jobs {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let read = catch_unwind(AssertUnwindSafe(|| read(&track)));
        let payload = match read {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                log::warn!("analysis: read {} failed: {}", track.path, e);
                dropped(&track);
                continue;
            }
            Err(_) => {
                log::error!("analysis: reading {} panicked", track.path);
                panicked(&track);
                continue;
            }
        };
        let mut fetched = Fetched { track, payload };
        loop {
            match tx.try_send(fetched) {
                Ok(()) => break,
                Err(TrySendError::Full(f)) => {
                    // Every decoder is busy. Poll rather than block: the
                    // receiver belongs to the scope, not to the workers, so a
                    // blocking send would not see them leave on a cancel and
                    // would wait for a receiver that is never coming.
                    if cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    fetched = f;
                    thread::sleep(HANDOFF_POLL);
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

/// Take fetched jobs until the reader is done or the pass is cancelled. The
/// file a worker already holds is finished; anything merely queued is dropped
/// unread, which is what keeps a cancel to one file per thread.
///
/// One file's analysis panicking costs that file and not the pass: `panicked` is
/// handed the job, and this worker takes the next one. Uncaught it would be
/// re-raised when `thread::scope` joins, unwinding every other decode in flight
/// and the reader with them.
fn decode_loop(
    rx: &Mutex<Receiver<Fetched>>,
    cancel: &AtomicBool,
    mut analyse: impl FnMut(Fetched),
    mut panicked: impl FnMut(&AnalysisJob),
) {
    loop {
        // The guard is held across `recv`, so exactly one worker waits on the
        // reader at a time and the rest wait on the lock.
        let Ok(fetched) = ({ rx.lock().recv() }) else {
            return;
        };
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let track = fetched.track.clone();
        if catch_unwind(AssertUnwindSafe(|| analyse(fetched))).is_err() {
            log::error!("analysis: decoding {} panicked", track.path);
            panicked(&track);
        }
    }
}

/// What analysing one track came to.
///
/// A store the row refused — the file moved on while the decode ran — is none
/// of these. It is `Done`: the rescan that moved it left the row queued, so
/// the drain loop takes it again against the file that is there now.
enum Outcome {
    Done,
    /// A result could not be stored. Worth another try on the next run.
    ///
    /// A file that could not be *read* never reaches here: the reader disposes
    /// of it, so by the time a decoder has a job the share has already given up
    /// its bytes.
    Retry,
    /// The file was read but cannot be decoded.
    Unreadable(String),
}

/// Fill whatever `job` is missing from what the reader pulled, and store it.
fn analyse(
    job: &AnalysisJob,
    payload: Payload,
    db: &Db,
    app: &AppHandle,
    config: &Config,
) -> Outcome {
    let mut retry = false;
    let mut errors: Vec<String> = Vec::new();
    let fingerprint = match payload {
        Payload::Whole(bytes) => {
            let start = Instant::now();
            // Read per track for the same reason `store_auto_cue` does: a threshold
            // changed mid-backfill applies from the next file on.
            let silence_dbfs = config.get_tuning().auto_cue.silence_dbfs;
            match waveform::analyze(Arc::clone(&bytes), silence_dbfs) {
                Ok(analysis) => {
                    let decode_ms = start.elapsed().as_millis();
                    let store_start = Instant::now();
                    // Unscreened, unlike everything below: the decode counted
                    // the samples whatever this job came here for.
                    let measured_ms = analysis.windows.duration_ms;
                    match db.set_measured_duration(job.id, measured_ms, now_ms(), job.mtime) {
                        Err(e) => {
                            log::error!("duration: store {} failed: {}", job.id, e);
                            retry = true;
                        }
                        Ok(false) => log::debug!("duration: {} moved on", job.path),
                        Ok(true) => log::debug!("duration: {} {}ms", job.path, measured_ms),
                    }
                    if job.needs_waveform {
                        match db.set_waveform(job.id, &analysis.curve, job.mtime) {
                            Err(e) => {
                                log::error!("waveform: store {} failed: {}", job.id, e);
                                retry = true;
                            }
                            Ok(false) => log::debug!("waveform: {} moved on", job.path),
                            Ok(true) => {
                                log::debug!(
                                    "waveform: {} decode {}ms write {}ms",
                                    job.path,
                                    decode_ms,
                                    store_start.elapsed().as_millis()
                                );
                                let _ = app.emit(WAVEFORM_READY_EVENT, job.id);
                            }
                        }
                    }
                    if job.needs_loudness {
                        let m = analysis.loudness;
                        let gain = m.map(|l| loudness::gain_db(l.lufs));
                        let peak = m.map(|l| f64::from(l.peak));
                        match db.set_loudness(job.id, gain, peak, now_ms(), job.mtime) {
                            Err(e) => {
                                log::error!("loudness: store {} failed: {}", job.id, e);
                                retry = true;
                            }
                            Ok(false) => log::debug!("loudness: {} moved on", job.path),
                            Ok(true) => log::debug!(
                                "loudness: {} {:?} LUFS gain {:?} dB",
                                job.path,
                                m.map(|l| l.lufs),
                                gain
                            ),
                        }
                    }
                    if job.needs_bpm {
                        match db.set_bpm(job.id, analysis.bpm, now_ms(), job.mtime) {
                            Err(e) => {
                                log::error!("bpm: store {} failed: {}", job.id, e);
                                retry = true;
                            }
                            Ok(false) => log::debug!("bpm: {} moved on", job.path),
                            Ok(true) => log::debug!("bpm: {} {:?}", job.path, analysis.bpm),
                        }
                    }
                    if job.needs_key {
                        match db.set_key(job.id, analysis.key, now_ms(), job.mtime) {
                            Err(e) => {
                                log::error!("key: store {} failed: {}", job.id, e);
                                retry = true;
                            }
                            Ok(false) => log::debug!("key: {} moved on", job.path),
                            Ok(true) => {
                                log::debug!(
                                    "key: {} {:?}",
                                    job.path,
                                    analysis.key.map(|k| k.name())
                                )
                            }
                        }
                    }
                    let levels = (job.needs_auto_cue || job.needs_auto_cue_levels)
                        .then(|| analysis.windows.envelope());
                    if let Some(levels) = &levels {
                        if job.needs_auto_cue {
                            store_auto_cue(job, db, app, config, levels, &mut retry);
                        } else {
                            // The trio is already derived and belongs to whatever
                            // thresholds produced it. Only the table is missing.
                            store_auto_cue_levels(job, db, levels, &mut retry);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("waveform: decode {} failed: {:#}", job.path, e);
                    errors.push(format!("waveform: {e:#}"));
                }
            }
            // Hashed from the bytes already in hand, never a second read. An error
            // out of an in-memory source is the file's fault by construction: the
            // only I/O error a `Cursor` can raise is an early end of stream, which
            // `fingerprint::is_read_error` already excludes.
            job.needs_fingerprint.then(|| {
                let ext = Path::new(&job.path).extension().and_then(|e| e.to_str());
                fingerprint::of_source(Box::new(Cursor::new(bytes)), ext)
                    .map_err(|e| format!("fingerprint: {e:#}"))
            })
        }
        // The reader hashed the head itself; a read failure never gets here.
        Payload::Fingerprint(result) => Some(result),
    };
    match fingerprint {
        Some(Ok(fp)) => {
            if let Err(e) = db.set_fingerprint(job.id, &fp, job.mtime) {
                log::error!("fingerprint: store {} failed: {}", job.id, e);
                retry = true;
            }
        }
        Some(Err(e)) => {
            log::warn!("fingerprint: {} failed: {}", job.path, e);
            errors.push(e);
        }
        None => {}
    }
    if retry {
        Outcome::Retry
    } else if errors.is_empty() {
        Outcome::Done
    } else {
        Outcome::Unreadable(errors.join("; "))
    }
}

/// Derive and commit this track's automatic cue points. The commit is refused
/// outright if the row moved on while the decode ran — the operator took
/// ownership, or a reclassification changed what should have been derived —
/// which is the whole race the ownership state exists for. A discarded result
/// is not a failure and leaves nothing to retry: a reclassified track is still
/// `pending`, so the drain loop takes it again under its new class.
fn store_auto_cue(
    job: &AnalysisJob,
    db: &Db,
    app: &AppHandle,
    config: &Config,
    levels: &level_envelope::Envelope,
    retry: &mut bool,
) {
    // Read per track rather than once per run: a threshold changed mid-backfill
    // then applies from the next file, matching "future analyses only".
    let thresholds = config.get_tuning().auto_cue.thresholds();
    let music = job.content_type == "music";
    let analysed = auto_cue::Analysed {
        cue: auto_cue::detect(levels, music, thresholds),
        levels: levels.clone(),
        thresholds,
        at_ms: now_ms(),
    };
    match db.set_auto_cue(job.id, &analysed, &job.content_type, job.mtime) {
        Ok(Some(cue_points)) => {
            log::debug!("auto cue: {} {:?}", job.path, analysed.cue);
            let _ = app.emit(
                CUE_POINTS_READY_EVENT,
                CuePointsReady {
                    id: job.id,
                    cue_points,
                },
            );
        }
        Ok(None) => log::debug!("auto cue: {} moved on under us", job.path),
        Err(e) => {
            log::error!("auto cue: store {} failed: {}", job.id, e);
            *retry = true;
        }
    }
}

/// Store the level table for a track that already has a derived trio, leaving
/// the trio alone.
///
/// The backfill path. Re-deriving here would apply today's thresholds to a
/// track analysed under yesterday's, which is exactly the implicit mass
/// re-analysis `docs/cue-auto-analysis.md` rules out; the operator asks for that
/// explicitly or not at all.
fn store_auto_cue_levels(
    job: &AnalysisJob,
    db: &Db,
    levels: &level_envelope::Envelope,
    retry: &mut bool,
) {
    match db.set_auto_cue_levels(job.id, levels, &job.content_type, job.mtime) {
        Ok(true) => log::debug!("auto cue levels: {}", job.path),
        Ok(false) => log::debug!("auto cue levels: {} moved on under us", job.path),
        Err(e) => {
            log::error!("auto cue levels: store {} failed: {}", job.id, e);
            *retry = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lost wakeup: a track queued after the worker's last drain found
    /// nothing, while it is still tidying up, must not wait for the next scan.
    #[test]
    fn a_kick_during_the_last_drain_runs_the_worker_again() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.kicked.store(true, Ordering::SeqCst);
        job.running.store(false, Ordering::SeqCst);

        assert!(job.claim_rerun());
        assert!(job.running.load(Ordering::SeqCst), "the slot is held again");
        assert!(!job.kicked.load(Ordering::SeqCst), "the kick is served");
    }

    /// A panicking pass must not take the single-flight slot with it: the whole
    /// feature would be dead until the app restarted.
    #[test]
    fn a_panicking_pass_releases_the_slot() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        *job.status.lock() = WaveformStatus::Running {
            processed: 1,
            total: 9,
        };
        let emitted: Mutex<Vec<WaveformStatus>> = Mutex::new(Vec::new());

        job.drain_loop(
            || panic!("a decoder went down"),
            |next| emitted.lock().push(next),
        );

        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(
            matches!(emitted.lock().as_slice(), [WaveformStatus::Idle]),
            "the bar is cleared"
        );
    }

    /// A pass that died before it found work never put a bar up, and the topic
    /// carries running/idle transitions — not an idle that follows nothing.
    #[test]
    fn a_pass_that_panicked_before_finding_work_says_nothing() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        let emitted: Mutex<Vec<WaveformStatus>> = Mutex::new(Vec::new());

        job.drain_loop(
            || panic!("the work list query went down"),
            |next| emitted.lock().push(next),
        );

        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(
            emitted.lock().is_empty(),
            "no bar went up, so none comes down"
        );
    }

    /// The pass that would have consumed the cancel is gone, so the panic
    /// consumes it in its place — and stops, rather than letting a kick that was
    /// waiting behind the cancel start a fresh pass the operator just stopped.
    #[test]
    fn a_panicking_pass_under_a_cancel_stops() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.cancel();
        let passes = AtomicUsize::new(0);

        job.drain_loop(
            || {
                passes.fetch_add(1, Ordering::SeqCst);
                job.kicked.store(true, Ordering::SeqCst);
                panic!("a decoder went down");
            },
            |_| {},
        );

        assert_eq!(passes.load(Ordering::SeqCst), 1, "the stop stands");
        assert!(!job.cancel.load(Ordering::SeqCst), "the cancel is consumed");
        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(
            job.kicked.load(Ordering::SeqCst),
            "the kick stands for the next start"
        );
    }

    /// Work that arrived while the pass was dying still gets a pass, the way it
    /// would have from a drain that returned.
    #[test]
    fn a_kick_is_served_after_a_panicking_pass() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        let passes = AtomicUsize::new(0);

        job.drain_loop(
            || {
                if passes.fetch_add(1, Ordering::SeqCst) == 0 {
                    job.kicked.store(true, Ordering::SeqCst);
                    panic!("a decoder went down");
                }
                Stop::Drained
            },
            |_| {},
        );

        assert_eq!(passes.load(Ordering::SeqCst), 2, "the kick got its pass");
        assert!(!job.running.load(Ordering::SeqCst));
    }

    #[test]
    fn a_worker_nobody_kicked_stops() {
        let job = WaveformJob::default();
        assert!(!job.claim_rerun());
        assert!(!job.running.load(Ordering::SeqCst));
    }

    #[test]
    fn a_cancelled_worker_stops_despite_a_kick() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.kicked.store(true, Ordering::SeqCst);
        job.cancel();

        assert!(!job.finish(Stop::Cancelled));
        assert!(!job.running.load(Ordering::SeqCst));
        assert!(
            job.kicked.load(Ordering::SeqCst),
            "the kick stands for the next start"
        );
    }

    /// The cancel dies with the pass it stopped, so the next kick is served
    /// rather than swallowed until the next launch.
    #[test]
    fn a_cancel_is_consumed_by_the_pass_it_stopped() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.cancel();

        assert!(!job.finish(Stop::Cancelled));
        assert!(!job.cancel.load(Ordering::SeqCst), "the cancel is consumed");
    }

    /// A cancel that lands between the last drain and the release still keeps
    /// the worker from taking the slot back, and stands for the next `start`
    /// to clear.
    #[test]
    fn a_cancel_after_the_last_drain_holds_the_worker_back() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.kicked.store(true, Ordering::SeqCst);
        job.cancel();

        assert!(!job.finish(Stop::Drained));
        assert!(!job.running.load(Ordering::SeqCst));
    }

    #[test]
    fn a_drained_worker_with_a_kick_drains_again() {
        let job = WaveformJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.kicked.store(true, Ordering::SeqCst);

        assert!(job.finish(Stop::Drained));
        assert!(job.running.load(Ordering::SeqCst), "the slot is held again");
    }

    /// A fresh kick claimed the slot the instant it was released; it drains.
    #[test]
    fn a_worker_that_lost_the_slot_stops() {
        let job = WaveformJob::default();
        job.kicked.store(true, Ordering::SeqCst);
        job.running.store(true, Ordering::SeqCst);

        assert!(!job.claim_rerun());
    }

    /// The pass holds one whole file per decoder, one queued, and one in the
    /// reader's hand. A track can be 100 MB, so this is the arithmetic that
    /// bounds it — stated here so raising either constant forces a cut to the
    /// other.
    #[test]
    fn the_pass_never_holds_more_than_its_file_budget() {
        assert_eq!(MAX_CONCURRENCY + READAHEAD + 1, MAX_RESIDENT_FILES);
    }

    fn analysis_job(id: i64) -> AnalysisJob {
        AnalysisJob {
            id,
            path: format!("/library/{id}.flac"),
            mtime: None,
            content_type: "music".into(),
            needs_waveform: true,
            needs_fingerprint: false,
            needs_loudness: false,
            needs_auto_cue: false,
            needs_auto_cue_levels: false,
            needs_bpm: false,
            needs_key: false,
            needs_duration: false,
        }
    }

    fn fingerprint_only(id: i64) -> AnalysisJob {
        AnalysisJob {
            needs_waveform: false,
            needs_fingerprint: true,
            ..analysis_job(id)
        }
    }

    fn some_bytes() -> Payload {
        Payload::Whole(Arc::from(vec![0u8; 16].into_boxed_slice()))
    }

    /// Every measurement that needs a decode needs the whole file; a job that
    /// wants nothing but a fingerprint gets the head. This fails the moment a
    /// eighth measurement is added and the reader is not told about it.
    #[test]
    fn only_a_fingerprint_backfill_reads_the_head_alone() {
        type Flag = (&'static str, fn(&mut AnalysisJob));
        let flags: [Flag; 7] = [
            ("waveform", |j| j.needs_waveform = true),
            ("loudness", |j| j.needs_loudness = true),
            ("auto cue", |j| j.needs_auto_cue = true),
            ("auto cue levels", |j| j.needs_auto_cue_levels = true),
            ("bpm", |j| j.needs_bpm = true),
            ("key", |j| j.needs_key = true),
            ("duration", |j| j.needs_duration = true),
        ];
        for (name, set) in flags {
            let mut job = fingerprint_only(1);
            set(&mut job);
            assert!(
                matches!(fetch_kind(&job), Fetch::Whole),
                "{name} needs the whole file"
            );
        }
        assert!(matches!(fetch_kind(&fingerprint_only(1)), Fetch::Head));
    }

    /// The point of the whole change: however many decoders are running, the
    /// share is only ever asked for one file at a time.
    #[test]
    fn the_reader_reads_one_file_at_a_time() {
        let jobs: Vec<AnalysisJob> = (1..=12).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let reading = AtomicUsize::new(0);
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    decode_loop(
                        &rx,
                        &cancel,
                        |_| thread::sleep(Duration::from_millis(1)),
                        |_| panic!("no decode panicked"),
                    );
                });
            }
            read_loop(
                jobs,
                tx,
                &cancel,
                |_| panic!("no read failed"),
                |_| panic!("no read panicked"),
                |_| {
                    assert_eq!(
                        reading.fetch_add(1, Ordering::SeqCst),
                        0,
                        "a second read started while one was in flight"
                    );
                    thread::sleep(Duration::from_millis(1));
                    reading.fetch_sub(1, Ordering::SeqCst);
                    Ok(some_bytes())
                },
            );
        });
    }

    #[test]
    fn a_file_the_share_would_not_give_up_never_reaches_a_decoder() {
        let jobs: Vec<AnalysisJob> = (1..=3).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let decoded: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let dropped: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                decode_loop(
                    &rx,
                    &cancel,
                    |f| decoded.lock().push(f.track.id),
                    |_| panic!("no decode panicked"),
                );
            });
            read_loop(
                jobs,
                tx,
                &cancel,
                |track| dropped.lock().push(track.id),
                |_| panic!("no read panicked"),
                |track| {
                    if track.id == 2 {
                        Err("share went away".into())
                    } else {
                        Ok(some_bytes())
                    }
                },
            );
        });

        assert_eq!(*dropped.lock(), vec![2], "the reader disposed of it");
        let mut got = decoded.lock().clone();
        got.sort_unstable();
        assert_eq!(got, vec![1, 3]);
    }

    /// One file's decode going down costs that file. Uncaught it would be
    /// re-raised at the scope's join and take every other decode with it.
    #[test]
    fn a_panicking_decode_leaves_the_rest_of_the_pass_alone() {
        let cancel = AtomicBool::new(false);
        let decoded: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let panicked: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let (tx, rx) = sync_channel::<Fetched>(4);
        let rx = Mutex::new(rx);
        for id in 1..=3 {
            tx.try_send(Fetched {
                track: analysis_job(id),
                payload: some_bytes(),
            })
            .expect("queued");
        }
        drop(tx);

        decode_loop(
            &rx,
            &cancel,
            |f| {
                assert_ne!(f.track.id, 2, "id 2 panics before it is recorded");
                decoded.lock().push(f.track.id);
            },
            |track| panicked.lock().push(track.id),
        );

        assert_eq!(*decoded.lock(), vec![1, 3]);
        assert_eq!(*panicked.lock(), vec![2], "the file is disposed of");
    }

    /// The reader runs on the scope's parent thread, so a panic in a read has no
    /// join to be re-raised at — it simply ends the pass.
    #[test]
    fn a_panicking_read_leaves_the_reader_going() {
        let jobs: Vec<AnalysisJob> = (1..=3).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let decoded: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let panicked: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                decode_loop(
                    &rx,
                    &cancel,
                    |f| decoded.lock().push(f.track.id),
                    |_| panic!("no decode panicked"),
                );
            });
            read_loop(
                jobs,
                tx,
                &cancel,
                |_| panic!("no read failed"),
                |track| panicked.lock().push(track.id),
                |track| {
                    assert_ne!(track.id, 2, "id 2 panics instead of returning");
                    Ok(some_bytes())
                },
            );
        });

        assert_eq!(*panicked.lock(), vec![2], "the file is disposed of");
        let mut got = decoded.lock().clone();
        got.sort_unstable();
        assert_eq!(got, vec![1, 3], "the files after it are still read");
    }

    #[test]
    fn every_fetched_job_reaches_exactly_one_decoder() {
        let jobs: Vec<AnalysisJob> = (1..=20).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let decoded: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    decode_loop(
                        &rx,
                        &cancel,
                        |f| decoded.lock().push(f.track.id),
                        |_| panic!("no decode panicked"),
                    );
                });
            }
            read_loop(
                jobs,
                tx,
                &cancel,
                |_| panic!("no read failed"),
                |_| panic!("no read panicked"),
                |_| Ok(some_bytes()),
            );
        });

        let mut got = decoded.lock().clone();
        got.sort_unstable();
        assert_eq!(got, (1..=20).collect::<Vec<_>>());
    }

    /// Dropping the sender is what ends the pass. Without it the scope would
    /// never join.
    #[test]
    fn decoders_stop_when_the_reader_is_done() {
        let cancel = AtomicBool::new(false);
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| decode_loop(&rx, &cancel, |_| {}, |_| panic!("no decode panicked")));
            }
            read_loop(
                Vec::new(),
                tx,
                &cancel,
                |_| {},
                |_| panic!("no read panicked"),
                |_| Ok(some_bytes()),
            );
        });
    }

    #[test]
    fn a_cancel_stops_the_reader_before_the_next_file() {
        let jobs: Vec<AnalysisJob> = (1..=5).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let reads = AtomicUsize::new(0);
        let (tx, rx) = sync_channel::<Fetched>(READAHEAD);
        let rx = Mutex::new(rx);

        std::thread::scope(|scope| {
            scope.spawn(|| decode_loop(&rx, &cancel, |_| {}, |_| panic!("no decode panicked")));
            read_loop(
                jobs,
                tx,
                &cancel,
                |_| panic!("no read failed"),
                |_| panic!("no read panicked"),
                |_| {
                    reads.fetch_add(1, Ordering::SeqCst);
                    cancel.store(true, Ordering::SeqCst);
                    Ok(some_bytes())
                },
            );
        });

        assert_eq!(reads.load(Ordering::SeqCst), 1, "the reader stopped");
    }

    /// A cancel while every decoder is busy must not leave the reader parked in
    /// the hand-off: the receiver belongs to the scope, so a blocking send
    /// would wait for a reader that is never coming.
    #[test]
    fn a_cancel_frees_the_reader_from_a_full_handoff() {
        let jobs: Vec<AnalysisJob> = (1..=8).map(analysis_job).collect();
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = sync_channel::<Fetched>(READAHEAD);

        // No decoder at all, so the channel fills at once and stays full.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::SeqCst);
            });
            read_loop(
                jobs,
                tx,
                &cancel,
                |_| {},
                |_| panic!("no read panicked"),
                |_| Ok(some_bytes()),
            );
        });
    }

    #[test]
    fn a_cancelled_decoder_leaves_queued_bytes_undecoded() {
        let cancel = AtomicBool::new(false);
        let decoded = AtomicUsize::new(0);
        let (tx, rx) = sync_channel::<Fetched>(4);
        let rx = Mutex::new(rx);
        for id in 1..=3 {
            tx.try_send(Fetched {
                track: analysis_job(id),
                payload: some_bytes(),
            })
            .expect("queued");
        }
        drop(tx);
        cancel.store(true, Ordering::SeqCst);

        decode_loop(
            &rx,
            &cancel,
            |_| {
                decoded.fetch_add(1, Ordering::SeqCst);
            },
            |_| panic!("no decode panicked"),
        );

        assert_eq!(decoded.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn every_disposed_job_is_counted_once() {
        let progress = Progress::new();
        progress.begin_pass(3);
        let now = Instant::now();

        assert_eq!(progress.record(now).0, 1);
        assert_eq!(progress.record(now).0, 2);
        let (processed, total, _) = progress.record(now);
        assert_eq!((processed, total), (3, 3));
    }

    #[test]
    fn progress_carries_across_passes() {
        let progress = Progress::new();
        progress.begin_pass(3);
        let now = Instant::now();
        for _ in 0..3 {
            progress.record(now);
        }

        // A concurrent scan added two more tracks; the denominator grows.
        assert_eq!(progress.begin_pass(2), 5);
        assert_eq!(progress.record(now).1, 5);
    }

    #[test]
    fn the_progress_emit_is_throttled() {
        let progress = Progress::new();
        progress.begin_pass(3);
        let start = Instant::now();

        assert!(progress.record(start).2, "the first one is always due");
        assert!(!progress.record(start + Duration::from_millis(1)).2);
        assert!(progress.record(start + PROGRESS_THROTTLE).2);
    }
}
