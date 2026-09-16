use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::audio::cue_points::CuePoints;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionPlaylistItem {
    Track {
        id: i64,
        /// The item's cue-point override, so a custom airing survives a
        /// restart. Additive and defaulted: a session file written before
        /// overrides existed loads with none, as does a stop marker.
        #[serde(default)]
        cue_override: Option<CuePoints>,
    },
    Stop,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(default)]
    pub playlist_ids: Vec<i64>,
    #[serde(default)]
    pub playlist_items: Vec<SessionPlaylistItem>,
    #[serde(default)]
    pub history_ids: Vec<i64>,
    #[serde(default)]
    pub current_track_id: Option<i64>,
    #[serde(default)]
    pub current_time: f64,
    /// The override the restored track was airing under, if any.
    #[serde(default)]
    pub current_cue_override: Option<CuePoints>,
    #[serde(default)]
    pub auto_playlist_active: bool,
    #[serde(default = "default_auto_advance")]
    pub auto_advance: bool,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(default = "default_volume")]
    pub cue_volume: f64,
}

fn default_auto_advance() -> bool {
    true
}

fn default_volume() -> f64 {
    1.0
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            playlist_ids: vec![],
            playlist_items: vec![],
            history_ids: vec![],
            current_track_id: None,
            current_time: 0.0,
            current_cue_override: None,
            auto_playlist_active: false,
            auto_advance: true,
            volume: 1.0,
            cue_volume: 1.0,
        }
    }
}

pub struct Session {
    path: PathBuf,
    last: Mutex<SessionState>,
}

impl Session {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("session.json");
        let last = Self::load_from(&path).unwrap_or_default();
        Self {
            path,
            last: Mutex::new(last),
        }
    }

    fn load_from(path: &Path) -> Option<SessionState> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn load(&self) -> SessionState {
        self.last.lock().clone()
    }

    pub fn save(&self, state: SessionState) -> Result<()> {
        let json = serde_json::to_string_pretty(&state)?;
        fs::write(&self.path, json).context("write session.json")?;
        *self.last.lock() = state;
        Ok(())
    }

    /// Drop every track reference after the library was reset. Ids restart at
    /// 1 in a fresh database, so a kept id would restore a different track.
    /// Volumes and modes survive; the previous file is kept as a backup.
    pub fn forget_tracks(&self) -> Result<()> {
        if self.path.exists() {
            fs::copy(&self.path, self.path.with_extension("legacy.bak.json"))
                .context("back up session.json")?;
        }
        let kept = self.load();
        self.save(SessionState {
            auto_playlist_active: kept.auto_playlist_active,
            auto_advance: kept.auto_advance,
            volume: kept.volume,
            cue_volume: kept.cue_volume,
            ..SessionState::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_when_missing_file() {
        let dir = tempdir().unwrap();
        let session = Session::open(dir.path());
        let s = session.load();
        assert!(s.playlist_ids.is_empty());
        assert_eq!(s.volume, 1.0);
        assert!(s.auto_advance);
    }

    #[test]
    fn save_then_reload_round_trips() {
        let dir = tempdir().unwrap();
        let s1 = Session::open(dir.path());
        let state = SessionState {
            playlist_ids: vec![1, 2, 3],
            current_track_id: Some(2),
            current_time: 12.5,
            volume: 0.5,
            ..Default::default()
        };
        s1.save(state).unwrap();

        let s2 = Session::open(dir.path());
        let loaded = s2.load();
        assert_eq!(loaded.playlist_ids, vec![1, 2, 3]);
        assert_eq!(loaded.current_track_id, Some(2));
        assert_eq!(loaded.current_time, 12.5);
        assert_eq!(loaded.volume, 0.5);
    }

    #[test]
    fn partial_json_falls_back_to_defaults_for_missing_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, r#"{"playlistIds":[7]}"#).unwrap();
        let session = Session::open(dir.path());
        let s = session.load();
        assert_eq!(s.playlist_ids, vec![7]);
        assert!(s.auto_advance);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.cue_volume, 1.0);
    }

    #[test]
    fn playlist_items_round_trip() {
        let dir = tempdir().unwrap();
        let s1 = Session::open(dir.path());
        let state = SessionState {
            playlist_items: vec![
                SessionPlaylistItem::Track {
                    id: 7,
                    cue_override: None,
                },
                SessionPlaylistItem::Stop,
                SessionPlaylistItem::Track {
                    id: 9,
                    cue_override: None,
                },
            ],
            ..Default::default()
        };
        s1.save(state).unwrap();

        let s2 = Session::open(dir.path());
        let loaded = s2.load();
        assert_eq!(
            loaded.playlist_items,
            vec![
                SessionPlaylistItem::Track {
                    id: 7,
                    cue_override: None,
                },
                SessionPlaylistItem::Stop,
                SessionPlaylistItem::Track {
                    id: 9,
                    cue_override: None,
                },
            ]
        );
    }

    #[test]
    fn legacy_playlist_ids_load_with_empty_items() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, r#"{"playlistIds":[7,8]}"#).unwrap();
        let session = Session::open(dir.path());
        let s = session.load();
        assert_eq!(s.playlist_ids, vec![7, 8]);
        assert!(s.playlist_items.is_empty());
    }

    #[test]
    fn playlist_items_json_shape() {
        let raw = r#"{"playlistItems":[{"kind":"track","id":7},{"kind":"stop"}]}"#;
        let s: SessionState = serde_json::from_str(raw).unwrap();
        assert_eq!(
            s.playlist_items,
            vec![
                SessionPlaylistItem::Track {
                    id: 7,
                    cue_override: None,
                },
                SessionPlaylistItem::Stop
            ]
        );
    }

    /// A custom airing has to survive a restart: both the override on a queued
    /// item and the one the track on air is playing under.
    #[test]
    fn cue_overrides_round_trip() {
        let dir = tempdir().unwrap();
        let s1 = Session::open(dir.path());
        let override_points = CuePoints {
            cue_in_ms: Some(2_000),
            ..Default::default()
        };
        let state = SessionState {
            playlist_items: vec![SessionPlaylistItem::Track {
                id: 7,
                cue_override: Some(override_points),
            }],
            current_track_id: Some(9),
            current_cue_override: Some(override_points),
            ..Default::default()
        };
        s1.save(state).unwrap();

        let loaded = Session::open(dir.path()).load();
        assert_eq!(
            loaded.playlist_items,
            vec![SessionPlaylistItem::Track {
                id: 7,
                cue_override: Some(override_points),
            }]
        );
        assert_eq!(loaded.current_cue_override, Some(override_points));
    }

    #[test]
    fn cue_volume_round_trips() {
        let dir = tempdir().unwrap();
        let s1 = Session::open(dir.path());
        let state = SessionState {
            cue_volume: 0.4,
            ..Default::default()
        };
        s1.save(state).unwrap();

        let s2 = Session::open(dir.path());
        let loaded = s2.load();
        assert_eq!(loaded.cue_volume, 0.4);
    }

    #[test]
    fn forget_tracks_clears_ids_keeps_settings_and_backs_up() {
        let dir = tempdir().unwrap();
        let session = Session::open(dir.path());
        session
            .save(SessionState {
                playlist_ids: vec![1, 2],
                history_ids: vec![3],
                current_track_id: Some(4),
                current_time: 12.0,
                volume: 0.4,
                auto_advance: false,
                ..Default::default()
            })
            .unwrap();

        session.forget_tracks().unwrap();

        let loaded = Session::open(dir.path()).load();
        assert!(loaded.playlist_ids.is_empty());
        assert!(loaded.history_ids.is_empty());
        assert_eq!(loaded.current_track_id, None);
        assert_eq!(loaded.current_time, 0.0);
        assert_eq!(loaded.volume, 0.4);
        assert!(!loaded.auto_advance);
        assert!(dir.path().join("session.legacy.bak.json").exists());
    }
}
