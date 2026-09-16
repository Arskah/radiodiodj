use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::db::Db;
use super::scanner::{self, ScanRoot};
use super::waveform_scan::WaveformJob;
use crate::persist::config::Config;

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

    fn emit_status(&self, app: &AppHandle, next: ScanStatus) {
        self.inner.lock().status = next.clone();
        let _ = app.emit("scan-state-changed", &next);
    }

    pub fn start(
        self: Arc<Self>,
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        waveform: Arc<WaveformJob>,
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
            "scan-state-changed",
            &ScanStatus::Running {
                processed: 0,
                total: 0,
            },
        );

        let s = Arc::clone(&self);
        std::thread::spawn(move || {
            run(s, app, db, config, waveform, cancel);
        });
        StartResult {
            already_running: false,
        }
    }
}

fn run(
    state: Arc<ScanState>,
    app: AppHandle,
    db: Arc<Db>,
    config: Arc<Config>,
    waveform: Arc<WaveformJob>,
    cancel: Arc<AtomicBool>,
) {
    const PROGRESS_THROTTLE: Duration = Duration::from_millis(200);
    let last_emit = Mutex::new(Instant::now() - PROGRESS_THROTTLE);
    let roots: Vec<ScanRoot> = ["music", "commercial", "jingle"]
        .into_iter()
        .flat_map(|content_type| {
            config
                .get_paths(content_type)
                .into_iter()
                .map(move |path| ScanRoot { content_type, path })
        })
        .collect();

    let outcome = scanner::scan_all(
        &db,
        &roots,
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
                    }),
                },
            );
            // Metadata is in — kick the async waveform pass. It runs on its own
            // thread and lands waveforms later, so the scan reports done now and
            // never blocks on the heavy per-track decode.
            Arc::clone(&waveform).start(app.clone(), Arc::clone(&db));
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

    state.inner.lock().cancel = None;
}
