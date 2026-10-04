//! In-app updates: asks radiodiodj.org whether a newer release exists and,
//! when an admin says so, installs it and restarts.
//!
//! Checking is automatic; installing never is. See `docs/updates.md`.

use crate::persist::config::Config;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Error, Update, UpdaterExt};

/// Carries the whole [`UpdateState`] on every change.
pub const STATE_EVENT: &str = "update:state";

const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// Where the updater is in its work.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Phase {
    /// Nothing has been checked yet.
    Idle,
    /// Asking the manifest.
    Checking,
    /// The manifest names nothing newer.
    UpToDate,
    /// A newer release exists; see [`UpdateState::offer`].
    Available,
    /// Fetching the bundle. `total` is absent when the server does not say.
    Downloading {
        /// Bytes received.
        done: u64,
        /// Bytes expected.
        total: Option<u64>,
    },
    /// Replacing the installed app. The process restarts from here.
    Installing,
    /// A check the operator asked for, or an install, did not succeed.
    Failed {
        /// What went wrong, as the updater reported it.
        message: String,
    },
}

/// A release newer than the running one.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// The release's version, without a leading `v`.
    pub version: String,
    /// Its release notes, as Markdown. Empty when the manifest carries no
    /// bundle for this installation.
    pub notes: String,
    /// When it was published, RFC 3339.
    pub date: Option<String>,
    /// Whether this installation can install it itself. Otherwise the operator
    /// is sent to the website.
    pub installable: bool,
}

/// What the renderer mirrors.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    /// The running version.
    pub current_version: String,
    /// Where the updater is.
    pub phase: Phase,
    /// The newer release, once a check has found one. Outlives a later failed
    /// check: a station that goes offline still knows what it saw.
    pub offer: Option<Offer>,
}

/// What a check found.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Newer(Offer),
    Nothing,
    Error(String),
}

impl UpdateState {
    fn new(current_version: String) -> Self {
        Self {
            current_version,
            phase: Phase::Idle,
            offer: None,
        }
    }

    /// The phase to rest in when nothing is going on.
    fn settle(&mut self) {
        self.phase = if self.offer.is_some() {
            Phase::Available
        } else {
            Phase::Idle
        };
    }

    /// Adopt a check's result. A `quiet` check — one nobody asked for — never
    /// reports a failure: a station offline for the night is not an error.
    fn checked(&mut self, found: Found, quiet: bool) {
        match found {
            Found::Newer(offer) => {
                self.offer = Some(offer);
                self.phase = Phase::Available;
            }
            Found::Nothing => {
                self.offer = None;
                self.phase = Phase::UpToDate;
            }
            Found::Error(_) if quiet => self.settle(),
            Found::Error(message) => self.phase = Phase::Failed { message },
        }
    }
}

/// Whether the updater can replace this installation. A `.deb` or `.rpm`
/// belongs to the system's package manager, and an unbundled binary is a
/// development build.
fn self_updating() -> bool {
    matches!(
        bundle_type(),
        Some(BundleType::App | BundleType::AppImage | BundleType::Nsis | BundleType::Msi)
    )
}

/// Owner of the update state and of the one operation allowed at a time.
pub struct Updater {
    app: AppHandle,
    config: Arc<Config>,
    state: Mutex<UpdateState>,
    /// The update the last check found, kept for the install.
    pending: Mutex<Option<Update>>,
    /// Held across a check or an install, so the two never interleave.
    busy: tokio::sync::Mutex<()>,
}

impl Updater {
    /// An updater that has checked nothing yet.
    pub fn new(app: AppHandle, config: Arc<Config>) -> Arc<Self> {
        let version = app.package_info().version.to_string();
        Arc::new(Self {
            app,
            config,
            state: Mutex::new(UpdateState::new(version)),
            pending: Mutex::new(None),
            busy: tokio::sync::Mutex::new(()),
        })
    }

    /// Check shortly after launch and then a few times a day, for as long as
    /// the setting allows it. Read each round, so switching it takes effect
    /// without a restart.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_CHECK_AFTER).await;
            loop {
                if me.config.get_tuning().updates.auto_check {
                    me.check(true).await;
                }
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    /// The current state.
    pub fn status(&self) -> UpdateState {
        self.state.lock().clone()
    }

    fn apply(&self, change: impl FnOnce(&mut UpdateState)) -> UpdateState {
        let state = {
            let mut state = self.state.lock();
            change(&mut state);
            state.clone()
        };
        let _ = self.app.emit(STATE_EVENT, &state);
        state
    }

    /// Ask the manifest whether a newer release exists.
    pub async fn check(&self, quiet: bool) -> UpdateState {
        let _busy = self.busy.lock().await;
        self.apply(|s| s.phase = Phase::Checking);
        let (found, update) = self.look().await;
        if let Found::Error(message) = &found {
            log::warn!("update check failed: {message}");
        }
        *self.pending.lock() = update;
        self.apply(|s| s.checked(found, quiet))
    }

    async fn look(&self) -> (Found, Option<Update>) {
        // The plugin looks this installation's bundle up before it compares
        // versions, so a manifest without one fails the check even when there
        // is nothing newer. The comparator runs first and remembers what it
        // saw, which tells the two apart.
        let newer = Arc::new(Mutex::new(None::<String>));
        let seen = Arc::clone(&newer);
        let exiting = self.app.clone();
        let updater = self
            .app
            .updater_builder()
            .version_comparator(move |current, release| {
                let is_newer = release.version > current;
                if is_newer {
                    *seen.lock() = Some(release.version.to_string());
                }
                is_newer
            })
            // Windows only: the installer replaces a running app by ending it.
            .on_before_exit(move || crate::shut_down(&exiting))
            .build();
        let checked = match updater {
            Ok(updater) => updater.check().await,
            Err(e) => Err(e),
        };
        match checked {
            Ok(Some(update)) => {
                let offer = Offer {
                    version: update.version.clone(),
                    notes: update.body.clone().unwrap_or_default(),
                    date: update.raw_json["pub_date"].as_str().map(str::to_owned),
                    installable: self_updating(),
                };
                (Found::Newer(offer), Some(update))
            }
            Ok(None) => (Found::Nothing, None),
            Err(Error::TargetNotFound(_) | Error::TargetsNotFound(_)) => {
                let found = match newer.lock().take() {
                    Some(version) => Found::Newer(Offer {
                        version,
                        notes: String::new(),
                        date: None,
                        installable: false,
                    }),
                    None => Found::Nothing,
                };
                (found, None)
            }
            Err(e) => (Found::Error(e.to_string()), None),
        }
    }

    /// Download the offered release, verify it, install it and restart.
    ///
    /// Returns only on failure: on Windows the installer ends the process, and
    /// elsewhere the restart does.
    pub async fn install(&self) -> Result<(), String> {
        let _busy = self.busy.lock().await;
        let installable = self.status().offer.is_some_and(|offer| offer.installable);
        let update = self
            .pending
            .lock()
            .clone()
            .filter(|_| installable)
            .ok_or("there is no update to install")?;

        self.apply(|s| {
            s.phase = Phase::Downloading {
                done: 0,
                total: None,
            }
        });
        let mut done = 0u64;
        let mut reported = Instant::now();
        let downloaded = update
            .download(
                |chunk, total| {
                    done += chunk as u64;
                    if reported.elapsed() >= PROGRESS_EVERY {
                        reported = Instant::now();
                        self.apply(|s| s.phase = Phase::Downloading { done, total });
                    }
                },
                || {},
            )
            .await;
        let bytes = downloaded.map_err(|e| self.fail(&e))?;

        self.apply(|s| s.phase = Phase::Installing);
        tauri::async_runtime::spawn_blocking(move || update.install(bytes))
            .await
            .map_err(|e| self.fail(&e))?
            .map_err(|e| self.fail(&e))?;

        log::info!("update installed; restarting");
        self.app.request_restart();
        Ok(())
    }

    fn fail(&self, error: &dyn std::fmt::Display) -> String {
        let message = error.to_string();
        log::error!("update failed: {message}");
        self.apply(|s| {
            s.phase = Phase::Failed {
                message: message.clone(),
            }
        });
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(version: &str) -> Offer {
        Offer {
            version: version.into(),
            notes: "notes".into(),
            date: None,
            installable: true,
        }
    }

    fn state() -> UpdateState {
        UpdateState::new("1.0.0".into())
    }

    #[test]
    fn a_newer_release_is_offered() {
        let mut s = state();
        s.checked(Found::Newer(offer("1.1.0")), true);
        assert_eq!(s.phase, Phase::Available);
        assert_eq!(s.offer, Some(offer("1.1.0")));
    }

    #[test]
    fn nothing_newer_withdraws_an_earlier_offer() {
        let mut s = state();
        s.checked(Found::Newer(offer("1.1.0")), true);
        s.checked(Found::Nothing, true);
        assert_eq!(s.phase, Phase::UpToDate);
        assert_eq!(s.offer, None);
    }

    #[test]
    fn a_quiet_failure_is_not_reported() {
        let mut s = state();
        s.phase = Phase::Checking;
        s.checked(Found::Error("offline".into()), true);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn a_quiet_failure_keeps_what_was_already_offered() {
        let mut s = state();
        s.checked(Found::Newer(offer("1.1.0")), true);
        s.phase = Phase::Checking;
        s.checked(Found::Error("offline".into()), true);
        assert_eq!(s.phase, Phase::Available);
        assert_eq!(s.offer, Some(offer("1.1.0")));
    }

    #[test]
    fn a_requested_check_reports_its_failure() {
        let mut s = state();
        s.checked(Found::Newer(offer("1.1.0")), true);
        s.checked(Found::Error("offline".into()), false);
        assert_eq!(
            s.phase,
            Phase::Failed {
                message: "offline".into()
            }
        );
        assert_eq!(s.offer, Some(offer("1.1.0")));
    }

    #[test]
    fn the_phase_crosses_the_boundary_tagged() {
        let json = serde_json::to_value(Phase::Downloading {
            done: 5,
            total: None,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "downloading", "done": 5, "total": null })
        );
        assert_eq!(
            serde_json::to_value(Phase::UpToDate).unwrap(),
            serde_json::json!({ "kind": "upToDate" })
        );
    }
}
