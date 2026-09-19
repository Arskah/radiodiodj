//! Background waveform and fingerprint computation, decoupled from the
//! metadata scan.
//!
//! The metadata scan (tag reads) is fast and finishes quickly. Computing a
//! track's amplitude curve requires a full audio decode, which is far heavier —
//! so it runs here, on its own worker thread, after the scan. The same pass
//! backfills [fingerprints](super::fingerprint): from the bytes already read
//! when a waveform is due, from the head of the file otherwise. The same decode
//! also yields the [automatic cue points](crate::audio::auto_cue), so an
//! unprepped track airs trimmed without a second pass over the file. Waveforms
//! land in the DB one at a time and a `waveform-ready` event is emitted per
//! track so the renderer can refresh a curve for the deck that is currently
//! showing it.
//!
//! Progress is surfaced separately from the metadata scan via
//! `waveform-progress` / `waveform-state-changed` so the UI can show a second
//! bar under the tag-scan bar. Like the metadata scan, the `processed`/`total`
//! counts are cumulative across all libraries (the worker drains one flat
//! missing-waveform list spanning every library).
//!
//! The job is single-flight (only one worker at a time) and cancelable. It
//! drains [`Db::tracks_needing_analysis`] in a loop so tracks added while it runs
//! are still picked up. A file that cannot be decoded is recorded in the DB and
//! left alone until a scan sees it change; one that merely could not be read
//! (a share dropping out) is skipped for the rest of the run and tried again on
//! the next.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::db::{AnalysisJob, Db};
use super::fingerprint;
use super::scanner::now_ms;
use crate::audio::{auto_cue, loudness, waveform};
use crate::persist::config::Config;

type Bytes = Arc<[u8]>;

/// Event emitted after a track's waveform is stored. Payload is the track id.
const WAVEFORM_READY_EVENT: &str = "waveform-ready";
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

#[derive(Default)]
pub struct WaveformJob {
    running: AtomicBool,
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
    pub fn start(self: Arc<Self>, app: AppHandle, db: Arc<Db>, config: Arc<Config>) {
        // Claim the single-flight slot; bail if a worker already holds it.
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            run(&self, &app, &db, &config);
            self.running.store(false, Ordering::SeqCst);
        });
    }

    fn set_status(&self, app: &AppHandle, next: WaveformStatus) {
        *self.status.lock() = next.clone();
        let _ = app.emit(WAVEFORM_STATE_EVENT, &next);
    }
}

fn run(job: &WaveformJob, app: &AppHandle, db: &Db, config: &Config) {
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

    loop {
        if job.cancel.load(Ordering::SeqCst) {
            break;
        }
        let missing = match db.tracks_needing_analysis() {
            Ok(m) => m,
            Err(e) => {
                log::error!("waveform: query missing failed: {}", e);
                break;
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
            break;
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
    }

    if started {
        job.set_status(app, WaveformStatus::Idle);
    }
}

/// What analysing one track came to.
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
    let fingerprint = if job.needs_waveform || job.needs_loudness || job.needs_auto_cue {
        let start = Instant::now();
        let bytes: Bytes = match std::fs::read(path) {
            Ok(v) => Arc::from(v.into_boxed_slice()),
            Err(e) => {
                log::warn!("waveform: read {} failed: {}", job.path, e);
                return Outcome::Retry;
            }
        };
        match waveform::analyze(Arc::clone(&bytes)) {
            Ok(analysis) => {
                let decode_ms = start.elapsed().as_millis();
                let store_start = Instant::now();
                if job.needs_waveform {
                    if let Err(e) = db.set_waveform(job.id, &analysis.curve) {
                        log::error!("waveform: store {} failed: {}", job.id, e);
                        retry = true;
                    } else {
                        log::debug!(
                            "waveform: {} decode {}ms write {}ms",
                            job.path,
                            decode_ms,
                            store_start.elapsed().as_millis()
                        );
                        let _ = app.emit(WAVEFORM_READY_EVENT, job.id);
                    }
                }
                if job.needs_loudness {
                    let m = analysis.loudness;
                    let gain = m.map(|l| loudness::gain_db(l.lufs));
                    let peak = m.map(|l| f64::from(l.peak));
                    if let Err(e) = db.set_loudness(job.id, gain, peak, now_ms()) {
                        log::error!("loudness: store {} failed: {}", job.id, e);
                        retry = true;
                    } else {
                        log::debug!(
                            "loudness: {} {:?} LUFS gain {:?} dB",
                            job.path,
                            m.map(|l| l.lufs),
                            gain
                        );
                    }
                }
                if job.needs_auto_cue {
                    store_auto_cue(job, db, config, &analysis.windows, &mut retry);
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
            if let Err(e) = db.set_fingerprint(job.id, &fp) {
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
/// outright if the operator took ownership while the decode ran, which is the
/// whole race the ownership state exists for — a discarded result is not a
/// failure and leaves nothing to retry.
fn store_auto_cue(
    job: &AnalysisJob,
    db: &Db,
    config: &Config,
    windows: &auto_cue::RmsWindows,
    retry: &mut bool,
) {
    // Read per track rather than once per run: a threshold changed mid-backfill
    // then applies from the next file, matching "future analyses only".
    let thresholds = config.get_tuning().auto_cue.thresholds();
    let music = job.content_type == "music";
    let cue = auto_cue::detect(windows, music, thresholds);
    match db.set_auto_cue(job.id, cue, thresholds, music, now_ms()) {
        Ok(true) => log::debug!("auto cue: {} {:?}", job.path, cue),
        Ok(false) => log::debug!("auto cue: {} kept the operator's edit", job.path),
        Err(e) => {
            log::error!("auto cue: store {} failed: {}", job.id, e);
            *retry = true;
        }
    }
}
