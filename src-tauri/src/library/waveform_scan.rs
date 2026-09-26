//! Background waveform and fingerprint computation, decoupled from the
//! metadata scan.
//!
//! The metadata scan (tag reads) is fast and finishes quickly. Computing a
//! track's amplitude curve requires a full audio decode, which is far heavier —
//! so it runs here, on its own worker thread, after the scan. The same pass
//! backfills [fingerprints](crate::audio_measure::fingerprint): from the bytes
//! already read when a waveform is due, from the head of the file otherwise.
//! The same decode also yields the
//! [automatic cue points](crate::library::auto_cue), so an unprepped track
//! airs trimmed without a second pass over the file. Waveforms land in the DB
//! one at a time and a `waveform-ready` event is emitted per track so the
//! renderer can refresh a curve for the deck that is currently showing it.
//!
//! Progress is surfaced separately from the metadata scan via
//! `waveform-progress` / `waveform-state-changed` so the UI can show a second
//! bar under the tag-scan bar. Like the metadata scan, the `processed`/`total`
//! counts are cumulative across all libraries (the worker drains one flat
//! missing-waveform list spanning every library).
//!
//! The job is single-flight (only one worker at a time) and cancelable. A
//! cancel stops the pass after the file each worker is on, and is consumed by
//! that pass: it means "not right now", so the next kick — a scan, a
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
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
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
/// Hard ceiling on parallel decode workers. Decoding is CPU-heavy and each file
/// is independent, so we fan out across cores; the actual worker count is
/// `cores - 2` (reserving headroom for playback/UI) clamped into `2..=MAX`. This
/// ceiling keeps a huge-core machine — or a networked share — from being flooded
/// with concurrent reads.
const MAX_CONCURRENCY: usize = 8;
/// Cores held back from the decode pool so audio playback and the UI stay
/// responsive when a backfill runs mid-set.
const RESERVED_CORES: usize = 2;

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
        std::thread::spawn(move || loop {
            // Cleared before the drain, so a kick during it is never lost.
            self.kicked.store(false, Ordering::SeqCst);
            let stop = run(&self, &app, &db, &config);
            if !self.finish(stop) {
                break;
            }
        });
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
    // passes so the drain loop cannot spin on them. Shared across the decode
    // threads.
    let failed: Mutex<HashSet<i64>> = Mutex::new(HashSet::new());
    // Total files processed across all passes; drives the progress numerator.
    let processed = AtomicUsize::new(0);
    let last_emit = Mutex::new(
        Instant::now()
            .checked_sub(PROGRESS_THROTTLE)
            .unwrap_or_else(Instant::now),
    );
    // Balanced pool: use the cores that exist minus a reserve for playback/UI,
    // never fewer than 2, never more than the MAX_CONCURRENCY ceiling.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let concurrency = cores
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
        let total = processed.load(Ordering::Relaxed) + pending.len();
        // Shared cursor into `pending`; each worker claims the next index.
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..concurrency {
                scope.spawn(|| loop {
                    if job.cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(track) = pending.get(i) else {
                        break;
                    };
                    match analyse(track, db, app, config) {
                        Outcome::Done => {}
                        Outcome::Retry => {
                            failed.lock().insert(track.id);
                        }
                        Outcome::Unreadable(error) => {
                            if let Err(e) = db.set_analysis_failed(track.id, &error, now_ms()) {
                                log::error!("analysis: store failure {} failed: {}", track.id, e);
                                failed.lock().insert(track.id);
                            }
                        }
                    }
                    let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    *job.status.lock() = WaveformStatus::Running {
                        processed: done,
                        total,
                    };
                    let mut le = last_emit.lock();
                    if le.elapsed() >= PROGRESS_THROTTLE {
                        *le = Instant::now();
                        drop(le);
                        let _ = app.emit(
                            WAVEFORM_PROGRESS_EVENT,
                            WaveformProgress {
                                processed: done,
                                total,
                            },
                        );
                    }
                });
            }
        });
    };

    if started {
        job.set_status(app, WaveformStatus::Idle);
    }
    stop
}

/// What analysing one track came to.
///
/// A store the row refused — the file moved on while the decode ran — is none
/// of these. It is `Done`: the rescan that moved it left the row queued, so
/// the drain loop takes it again against the file that is there now.
enum Outcome {
    Done,
    /// The file could not be read, or a result could not be stored. Worth
    /// another try on the next run.
    Retry,
    /// The file was read but cannot be decoded.
    Unreadable(String),
}

/// Fill whatever `job` is missing and store it.
fn analyse(job: &AnalysisJob, db: &Db, app: &AppHandle, config: &Config) -> Outcome {
    let path = Path::new(&job.path);
    let mut retry = false;
    let mut errors: Vec<String> = Vec::new();
    let fingerprint = if job.needs_waveform
        || job.needs_loudness
        || job.needs_auto_cue
        || job.needs_auto_cue_levels
        || job.needs_bpm
    {
        let start = Instant::now();
        let bytes: Bytes = match std::fs::read(path) {
            Ok(v) => Arc::from(v.into_boxed_slice()),
            Err(e) => {
                log::warn!("waveform: read {} failed: {}", job.path, e);
                return Outcome::Retry;
            }
        };
        // Read per track for the same reason `store_auto_cue` does: a threshold
        // changed mid-backfill applies from the next file on.
        let silence_dbfs = config.get_tuning().auto_cue.silence_dbfs;
        match waveform::analyze(Arc::clone(&bytes), silence_dbfs) {
            Ok(analysis) => {
                let decode_ms = start.elapsed().as_millis();
                let store_start = Instant::now();
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
        job.needs_fingerprint.then(|| {
            let ext = path.extension().and_then(|e| e.to_str());
            fingerprint::of_source(Box::new(Cursor::new(bytes)), ext)
        })
    } else {
        job.needs_fingerprint.then(|| fingerprint::of_file(path))
    };
    match fingerprint {
        Some(Ok(fp)) => {
            if let Err(e) = db.set_fingerprint(job.id, &fp, job.mtime) {
                log::error!("fingerprint: store {} failed: {}", job.id, e);
                retry = true;
            }
        }
        Some(Err(e)) => {
            log::warn!("fingerprint: {} failed: {:#}", job.path, e);
            if fingerprint::is_read_error(&e) {
                retry = true;
            } else {
                errors.push(format!("fingerprint: {e:#}"));
            }
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
}
