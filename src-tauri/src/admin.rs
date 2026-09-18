//! Admin mode: a password that guards the settings and library-changing
//! commands against mistakes. It is not a security boundary — anyone with
//! file access can edit `config.json`. See `docs/admin-mode.md`.

use crate::persist::config::Config;
use anyhow::{anyhow, Result};
use argon2::password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

/// Commands refused while admin mode is locked. The invoke handler checks
/// every call against this list, so a new admin-only command must be added
/// here.
pub const ADMIN_COMMANDS: &[&str] = &[
    "update_track_metadata",
    "revert_track_tags",
    "retry_tag_write",
    "dismiss_tag_write",
    "add_path",
    "remove_path",
    "purge_tracks",
    "set_main_device",
    "set_cue_device",
    "set_now_playing_config",
    "set_tuning_config",
    "set_theme",
    "set_station_name",
    "set_station_image",
    "clear_station_image",
    "reload_themes",
    "reveal_themes_dir",
    "health_dismiss",
    "health_undismiss",
    "scan_libraries",
    "cancel_scan",
    "set_cue_points",
    "now_playing_test",
    "admin_set_password",
    "admin_clear_password",
    "admin_set_idle_lock_min",
];

pub const LOCKED_ERROR: &str = "admin mode is locked";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminStatus {
    pub password_set: bool,
    pub unlocked: bool,
    pub idle_lock_min: u64,
}

pub struct AdminLock {
    config: Arc<Config>,
    /// Starts false on every launch, so a restart locks again.
    unlocked: AtomicBool,
    app: Option<AppHandle>,
}

impl AdminLock {
    pub fn new(config: Arc<Config>, app: Option<AppHandle>) -> Self {
        Self {
            config,
            unlocked: AtomicBool::new(false),
            app,
        }
    }

    /// True when no password is set or the operator has unlocked.
    pub fn is_admin(&self) -> bool {
        self.config.password_hash().is_none() || self.unlocked.load(Ordering::SeqCst)
    }

    /// Refuse `command` if it is admin-only and admin mode is locked.
    pub fn gate(&self, command: &str) -> Result<(), String> {
        if ADMIN_COMMANDS.contains(&command) && !self.is_admin() {
            return Err(LOCKED_ERROR.into());
        }
        Ok(())
    }

    pub fn status(&self) -> AdminStatus {
        let password_set = self.config.password_hash().is_some();
        AdminStatus {
            password_set,
            unlocked: !password_set || self.unlocked.load(Ordering::SeqCst),
            idle_lock_min: self.config.idle_lock_min(),
        }
    }

    /// Check `password` and unlock on a match. A wrong password leaves the
    /// lock as it was. Slow by design; call it off the main thread.
    pub fn unlock(&self, password: &str) -> bool {
        let Some(stored) = self.config.password_hash() else {
            return true;
        };
        let ok = verify(password, &stored);
        if ok {
            self.unlocked.store(true, Ordering::SeqCst);
            self.emit();
        } else {
            log::warn!("admin unlock refused: wrong password");
        }
        ok
    }

    pub fn lock(&self) {
        self.unlocked.store(false, Ordering::SeqCst);
        self.emit();
    }

    /// Store a new password. The session stays unlocked, since whoever set it
    /// is the admin at the desk.
    pub fn set_password(&self, password: &str) -> Result<()> {
        if password.is_empty() {
            return Err(anyhow!("password must not be empty"));
        }
        let hash = Argon2::default()
            .hash_password(password.as_bytes())
            .map_err(|e| anyhow!("hash password: {e}"))?
            .to_string();
        self.config.set_password_hash(Some(hash))?;
        self.unlocked.store(true, Ordering::SeqCst);
        self.emit();
        Ok(())
    }

    pub fn clear_password(&self) -> Result<()> {
        self.config.set_password_hash(None)?;
        self.emit();
        Ok(())
    }

    pub fn set_idle_lock_min(&self, minutes: u64) -> Result<()> {
        self.config.set_idle_lock_min(minutes)?;
        self.emit();
        Ok(())
    }

    fn emit(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("admin-state-changed", self.status());
        }
    }
}

fn verify(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        log::error!("admin password hash in config.json is not a valid PHC string");
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const OPEN_COMMANDS: &[&str] = &[
        "playlist_add",
        "playlist_set_item_cue_points",
        "cue_load",
        "get_waveform_detail",
        "reveal_track",
        "library_health",
        "library_check_now",
        "admin_status",
        "admin_unlock",
        "admin_lock",
    ];

    fn lock_in(dir: &std::path::Path) -> AdminLock {
        AdminLock::new(Arc::new(Config::open(dir).unwrap()), None)
    }

    #[test]
    fn no_password_allows_everything() {
        let dir = tempdir().unwrap();
        let admin = lock_in(dir.path());
        assert!(admin.is_admin());
        for cmd in ADMIN_COMMANDS.iter().chain(OPEN_COMMANDS) {
            assert_eq!(admin.gate(cmd), Ok(()), "{cmd}");
        }
        assert_eq!(
            admin.status(),
            AdminStatus {
                password_set: false,
                unlocked: true,
                idle_lock_min: 15
            }
        );
    }

    #[test]
    fn fresh_launch_with_password_refuses_admin_commands() {
        let dir = tempdir().unwrap();
        lock_in(dir.path()).set_password("hunter42").unwrap();

        let admin = lock_in(dir.path());
        assert!(!admin.is_admin());
        for cmd in ADMIN_COMMANDS {
            assert_eq!(admin.gate(cmd), Err(LOCKED_ERROR.to_string()), "{cmd}");
        }
        for cmd in OPEN_COMMANDS {
            assert_eq!(admin.gate(cmd), Ok(()), "{cmd}");
        }
    }

    #[test]
    fn unlock_needs_the_right_password_and_lock_relocks() {
        let dir = tempdir().unwrap();
        lock_in(dir.path()).set_password("hunter42").unwrap();
        let admin = lock_in(dir.path());

        assert!(!admin.unlock("hunter43"));
        assert!(!admin.unlock(""));
        assert!(admin.gate("set_cue_points").is_err());

        assert!(admin.unlock("hunter42"));
        assert!(admin.gate("set_cue_points").is_ok());
        assert!(admin.status().unlocked);

        admin.lock();
        assert!(admin.gate("set_cue_points").is_err());
        assert!(!admin.status().unlocked);
    }

    #[test]
    fn setting_a_password_keeps_the_session_unlocked() {
        let dir = tempdir().unwrap();
        let admin = lock_in(dir.path());
        admin.set_password("hunter42").unwrap();
        assert!(admin.is_admin());
        assert!(admin.status().password_set);
    }

    #[test]
    fn empty_password_is_refused() {
        let dir = tempdir().unwrap();
        let admin = lock_in(dir.path());
        assert!(admin.set_password("").is_err());
        assert!(!admin.status().password_set);
    }

    #[test]
    fn hash_is_stored_never_the_plaintext() {
        let dir = tempdir().unwrap();
        lock_in(dir.path()).set_password("hunter42").unwrap();
        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(!raw.contains("hunter42"));
        assert!(raw.contains("\"passwordHash\": \"$argon2id$"));
    }

    #[test]
    fn clearing_the_password_unlocks_everything() {
        let dir = tempdir().unwrap();
        lock_in(dir.path()).set_password("hunter42").unwrap();
        let admin = lock_in(dir.path());
        assert!(admin.gate("add_path").is_err());
        // The field removed by hand, as in the recovery steps.
        admin.clear_password().unwrap();
        assert!(admin.gate("add_path").is_ok());
        assert!(lock_in(dir.path()).is_admin());
    }

    #[test]
    fn corrupt_hash_stays_locked() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"admin":{"passwordHash":"not-a-hash"}}"#,
        )
        .unwrap();
        let admin = lock_in(dir.path());
        assert!(!admin.unlock("not-a-hash"));
        assert!(admin.gate("add_path").is_err());
    }

    #[test]
    fn idle_lock_min_is_clamped() {
        let dir = tempdir().unwrap();
        let admin = lock_in(dir.path());
        admin.set_idle_lock_min(0).unwrap();
        assert_eq!(admin.status().idle_lock_min, 1);
    }

    #[test]
    fn every_admin_command_is_a_registered_command() {
        let lib = include_str!("lib.rs");
        for cmd in ADMIN_COMMANDS {
            assert!(
                lib.contains(&format!("fn {cmd}(")) && lib.contains(&format!("            {cmd},")),
                "{cmd} is not a command registered in lib.rs"
            );
        }
    }
}
