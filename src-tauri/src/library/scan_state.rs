use parking_lot::Mutex;
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::db::Db;
use super::listing;
use super::scanner::{self, Missing};
use super::waveform_scan::WaveformJob;
use crate::persist::config::Config;

/// Running/idle transitions of the metadata scan.
const STATE_EVENT: &str = "scan-state-changed";

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ScanStatus {
    #[serde(rename_all = "camelCase")]
    Idle {
        last_result: Option<ScanResult>,
    },
    Running {
        processed: usize,
        total: usize,
    },
    Canceled {
        processed: usize,
        total: usize,
        added: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub total: usize,
    pub added: usize,
    /// Files matched back to the track they were before moving.
    pub reattached: usize,
    /// Tracks whose file this scan no longer found.
    pub missing: usize,
    /// Tracks whose file this scan found holding a different recording. They
    /// are missing too, reported apart because nothing moved or vanished — the
    /// operator overwrote the file.
    pub replaced: usize,
}

#[derive(Serialize, Clone)]
struct ScanProgress {
    processed: usize,
    total: usize,
}

#[derive(Serialize, Clone)]
pub struct StartResult {
    #[serde(rename = "alreadyRunning")]
    pub already_running: bool,
}

struct Inner {
    status: ScanStatus,
    cancel: Option<Arc<AtomicBool>>,
}

pub struct ScanState {
    inner: Mutex<Inner>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                status: ScanStatus::Idle { last_result: None },
                cancel: None,
            }),
        }
    }
}

impl ScanState {
    pub fn status(&self) -> ScanStatus {
        self.inner.lock().status.clone()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.inner.lock().status, ScanStatus::Running { .. })
    }

    pub fn cancel(&self) {
        if let Some(token) = &self.inner.lock().cancel {
            token.store(true, Ordering::SeqCst);
        }
    }

    /// Publish a status, and release the cancel token with it when it is a
    /// terminal one.
    ///
    /// The status *is* the single-flight slot, so the token has to die under the
    /// same lock that frees the slot: released afterwards, it would be the token
    /// of whichever scan claimed the slot in between, and that scan could then
    /// not be cancelled at all.
    fn emit_status(&self, app: &AppHandle, next: ScanStatus) {
        {
            let mut inner = self.inner.lock();
            inner.status = next.clone();
            if !matches!(next, ScanStatus::Running { .. }) {
                inner.cancel = None;
            }
        }
        let _ = app.emit(STATE_EVENT, &next);
    }

    /// Start the scan the operator asked for: it applies everything, missing
    /// files included.
    pub fn start(
        self: Arc<Self>,
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        waveform: Arc<WaveformJob>,
    ) -> StartResult {
        self.start_with(app, db, config, waveform, Missing::Mark)
    }

    /// Start a scan that adds and updates but retires nothing it merely failed
    /// to find, for the library check to run unattended. See
    /// [`Missing::OnlyMoved`].
    pub fn start_additive(
        self: Arc<Self>,
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        waveform: Arc<WaveformJob>,
    ) -> StartResult {
        self.start_with(app, db, config, waveform, Missing::OnlyMoved)
    }

    fn start_with(
        self: Arc<Self>,
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        waveform: Arc<WaveformJob>,
        missing: Missing,
    ) -> StartResult {
        if self.is_running() {
            return StartResult {
                already_running: true,
            };
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut g = self.inner.lock();
            g.status = ScanStatus::Running {
                processed: 0,
                total: 0,
            };
            g.cancel = Some(cancel.clone());
        }
        let _ = app.emit(
            STATE_EVENT,
            &ScanStatus::Running {
                processed: 0,
                total: 0,
            },
        );

        let s = Arc::clone(&self);
        std::thread::spawn(move || {
            let emit = |next: ScanStatus| {
                let _ = app.emit(STATE_EVENT, &next);
            };
            s.guarded_run(
                || {
                    run(
                        Arc::clone(&s),
                        app.clone(),
                        db,
                        config,
                        waveform,
                        missing,
                        cancel,
                    )
                },
                emit,
            );
        });
        StartResult {
            already_running: false,
        }
    }

    /// Run one scan and leave the state terminal however it ends — a panic
    /// included.
    ///
    /// `Running` *is* the single-flight slot here, so a panic that unwinds past
    /// the tail of [`run`] leaves every later scan answering `already_running`
    /// for the life of the process, with the bar stuck where it stopped and
    /// Cancel poking a token nobody reads. A scan is lofty and symphonia over
    /// operator files, so that is a realistic file away — `inspect_all` catches
    /// per file, and this is the backstop for everything else. The panic itself
    /// is logged by the hook `lib.rs` installs.
    ///
    /// Only a scan still showing as running is failed here. A panic after the
    /// result was published — the analysis kick that follows it is the way in —
    /// must not replace _2 986 tracks (12 new/updated…)_ with an error for a scan
    /// that in fact finished and committed.
    ///
    /// `scan` and `emit` are injected so the recovery can be driven without a
    /// filesystem or an `AppHandle`.
    fn guarded_run(&self, scan: impl FnOnce(), emit: impl FnOnce(ScanStatus)) {
        if catch_unwind(AssertUnwindSafe(scan)).is_err() {
            log::error!("scan: the scan panicked; releasing the slot");
            let next = ScanStatus::Error {
                message: "the scan stopped unexpectedly — see the log".to_string(),
            };
            let stuck = {
                let mut inner = self.inner.lock();
                let stuck = matches!(inner.status, ScanStatus::Running { .. });
                if stuck {
                    inner.status = next.clone();
                    inner.cancel = None;
                }
                stuck
            };
            if stuck {
                emit(next);
            }
        }
    }
}

fn run(
    state: Arc<ScanState>,
    app: AppHandle,
    db: Arc<Db>,
    config: Arc<Config>,
    waveform: Arc<WaveformJob>,
    missing: Missing,
    cancel: Arc<AtomicBool>,
) {
    const PROGRESS_THROTTLE: Duration = Duration::from_millis(200);
    let last_emit = Mutex::new(Instant::now() - PROGRESS_THROTTLE);
    let roots = listing::configured_roots(&config);

    let outcome = scanner::scan_all(
        &db,
        &roots,
        missing,
        &|| cancel.load(Ordering::SeqCst),
        |processed, total| {
            state.inner.lock().status = ScanStatus::Running { processed, total };
            let mut last = last_emit.lock();
            if last.elapsed() >= PROGRESS_THROTTLE {
                *last = Instant::now();
                drop(last);
                let _ = app.emit("scan-progress", ScanProgress { processed, total });
            }
        },
    );

    match outcome {
        Ok(o) if o.canceled => state.emit_status(
            &app,
            ScanStatus::Canceled {
                processed: o.total,
                total: o.total,
                added: o.added,
            },
        ),
        Ok(o) => {
            state.emit_status(
                &app,
                ScanStatus::Idle {
                    last_result: Some(ScanResult {
                        total: o.total,
                        added: o.added,
                        reattached: o.reattached,
                        missing: o.missing,
                        replaced: o.replaced,
                    }),
                },
            );
            // Metadata is in — kick the async waveform pass. It runs on its own
            // thread and lands waveforms later, so the scan reports done now and
            // never blocks on the heavy per-track decode.
            Arc::clone(&waveform).start(app.clone(), Arc::clone(&db), Arc::clone(&config));
        }
        Err(e) => {
            log::error!("scan failed: {}", e);
            state.emit_status(
                &app,
                ScanStatus::Error {
                    message: e.to_string(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Running` is the single-flight slot, so a panic that skips the terminal
    /// status leaves every later scan answering `already_running` for the life of
    /// the process.
    #[test]
    fn a_panicking_scan_releases_the_slot() {
        let state = ScanState::default();
        state.inner.lock().status = ScanStatus::Running {
            processed: 0,
            total: 0,
        };
        state.inner.lock().cancel = Some(Arc::new(AtomicBool::new(true)));
        let emitted: Mutex<Vec<ScanStatus>> = Mutex::new(Vec::new());

        state.guarded_run(
            || panic!("a tag parser went down"),
            |next| emitted.lock().push(next),
        );

        assert!(!state.is_running(), "the slot is free");
        assert!(
            matches!(emitted.lock().as_slice(), [ScanStatus::Error { .. }]),
            "the bar is cleared with an error"
        );
        assert!(
            state.inner.lock().cancel.is_none(),
            "Cancel stops poking a token nobody reads"
        );
    }

    /// A panic after the scan published its result — the analysis kick that
    /// follows it is the way in — must not turn a scan that finished and
    /// committed into an error.
    #[test]
    fn a_panic_after_the_result_leaves_it_standing() {
        let state = ScanState::default();
        let emitted: Mutex<Vec<ScanStatus>> = Mutex::new(Vec::new());

        state.guarded_run(
            || {
                state.inner.lock().status = ScanStatus::Idle {
                    last_result: Some(ScanResult {
                        total: 2986,
                        added: 12,
                        reattached: 3,
                        missing: 1,
                        replaced: 0,
                    }),
                };
                panic!("the analysis pass could not be spawned");
            },
            |next| emitted.lock().push(next),
        );

        assert!(emitted.lock().is_empty(), "the result stands");
        assert!(matches!(
            state.inner.lock().status,
            ScanStatus::Idle {
                last_result: Some(_)
            }
        ));
    }

    #[test]
    fn a_scan_that_returns_keeps_the_status_it_emitted() {
        let state = ScanState::default();
        let emitted: Mutex<Vec<ScanStatus>> = Mutex::new(Vec::new());

        state.guarded_run(
            || {
                state.inner.lock().status = ScanStatus::Idle { last_result: None };
            },
            |next| emitted.lock().push(next),
        );

        assert!(emitted.lock().is_empty(), "nothing to recover from");
        assert!(matches!(
            state.inner.lock().status,
            ScanStatus::Idle { last_result: None }
        ));
    }
}
