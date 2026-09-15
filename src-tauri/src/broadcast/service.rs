use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use tauri::async_runtime::{self, JoinHandle};
use tauri::{AppHandle, Listener};

use crate::library::db::TrackLoadInfo;
use crate::persist::config::Config;

use super::file_sink::FileSink;
use super::payload::{test_payload, BroadcastTrack, Payload};
use super::state::{Effect, State};
use super::webhook::Webhook;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

pub struct BroadcastService {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    config: Arc<Config>,
    webhook: Webhook,
    default_dir: PathBuf,
    in_flight: Mutex<Option<JoinHandle<()>>>,
    shutdown_done: Mutex<bool>,
}

impl From<TrackLoadInfo> for BroadcastTrack {
    fn from(info: TrackLoadInfo) -> Self {
        // Air time, not file time: downstream automation schedules against what
        // actually goes out. Resolved against the tag duration rather than the
        // decoded one, so on a VBR file with no cue-out this can differ slightly
        // from the deck's `:duration` — the same discrepancy that already exists
        // between the tag and the decoder.
        let duration_sec = info
            .cue_points
            .resolve(Some(info.duration))
            .air_duration()
            .unwrap_or(info.duration);
        BroadcastTrack {
            id: info.id,
            title: info.title,
            artist: info.artist,
            album: info.album,
            genre: info.genre,
            duration_sec,
            content_type: info.content_type,
            path: info.path,
        }
    }
}

impl BroadcastService {
    pub fn new(config: Arc<Config>, default_dir: PathBuf) -> anyhow::Result<Self> {
        let webhook = Webhook::new()?;
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::new()),
                config,
                webhook,
                default_dir,
                in_flight: Mutex::new(None),
                shutdown_done: Mutex::new(false),
            }),
        })
    }

    pub fn set_pending_track(&self, track: BroadcastTrack) {
        self.inner.state.lock().set_pending(track);
    }

    /// Attach app.listen handlers for main-deck:pause-state and main-deck:ended.
    /// Cue deck uses cue:* topics and is intentionally not subscribed.
    pub fn attach_to_app(&self, app: &AppHandle) {
        let pause_state_inner = Arc::clone(&self.inner);
        app.listen("main-deck:pause-state", move |event| {
            let paused: bool = serde_json::from_str(event.payload()).unwrap_or(false);
            Inner::dispatch(&pause_state_inner, |s| s.on_pause_state(paused, Utc::now()));
        });

        let ended_inner = Arc::clone(&self.inner);
        app.listen("main-deck:ended", move |_event| {
            Inner::dispatch(&ended_inner, |s| s.on_ended(Utc::now()));
        });
    }

    /// Test webhook: synthetic `{"event":"test"}` payload. Returns delivered status
    /// or error string for inline UI feedback. Blocks until result.
    pub fn test_webhook_blocking(&self) -> Result<u16, String> {
        let cfg = self.inner.config.get_now_playing();
        let url = cfg
            .webhook_url
            .clone()
            .filter(|u| !u.is_empty())
            .ok_or_else(|| "webhook URL not configured".to_string())?;
        let secret = cfg.webhook_secret.clone();
        let inner = Arc::clone(&self.inner);
        async_runtime::block_on(async move {
            let payload = test_payload(Utc::now());
            match inner
                .webhook
                .send_test(&url, secret.as_deref(), &payload)
                .await
            {
                Ok(status) => Ok(status.as_u16()),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    /// Synchronous shutdown: fire final stop if needed, wait up to 2s.
    /// Idempotent. Safe to call from frontend close hook and RunEvent::Exit.
    pub fn shutdown_blocking(&self) {
        {
            let mut done = self.inner.shutdown_done.lock();
            if *done {
                return;
            }
            *done = true;
        }

        let effect = self.inner.state.lock().on_shutdown(Utc::now());
        if let Effect::Fire(payload) = effect {
            let inner = Arc::clone(&self.inner);
            let _ = async_runtime::block_on(async move {
                tokio::time::timeout(SHUTDOWN_TIMEOUT, Inner::deliver(&inner, payload)).await
            });
        }

        // Abort any still-running in-flight task; we ignore its result.
        if let Some(h) = self.inner.in_flight.lock().take() {
            h.abort();
        }
    }
}

impl Inner {
    fn dispatch<F>(inner: &Arc<Inner>, f: F)
    where
        F: FnOnce(&mut State) -> Effect,
    {
        let effect = f(&mut inner.state.lock());
        if let Effect::Fire(payload) = effect {
            inner.spawn_deliver(payload);
        }
    }

    fn spawn_deliver(self: &Arc<Inner>, payload: Payload) {
        // Cancel in-flight: new event supersedes prior unfinished delivery.
        if let Some(h) = self.in_flight.lock().take() {
            h.abort();
        }
        let inner = Arc::clone(self);
        let handle = async_runtime::spawn(async move {
            Inner::deliver(&inner, payload).await;
        });
        *self.in_flight.lock() = Some(handle);
    }

    async fn deliver(inner: &Arc<Inner>, payload: Payload) {
        let cfg = inner.config.get_now_playing();
        if cfg.file_enabled {
            let dir = cfg
                .file_dir
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| inner.default_dir.clone());
            let sink = FileSink::new(dir);
            if let Err(e) = sink.write(&payload) {
                log::error!("broadcast: file sink write failed: {e:#}");
            }
        }

        if cfg.webhook_enabled {
            if let Some(url) = cfg.webhook_url.as_deref().filter(|u| !u.is_empty()) {
                match inner
                    .webhook
                    .send(url, cfg.webhook_secret.as_deref(), &payload)
                    .await
                {
                    Ok(status) if status.is_success() => {
                        log::debug!("broadcast: webhook {} -> {}", url, status);
                    }
                    Ok(status) => {
                        log::warn!("broadcast: webhook {} -> {}", url, status);
                    }
                    Err(e) => {
                        log::warn!("broadcast: webhook {} failed: {e:#}", url);
                    }
                }
            }
        }
    }
}

pub fn default_now_playing_dir(app_data_dir: &std::path::Path) -> PathBuf {
    app_data_dir.join("now-playing")
}
