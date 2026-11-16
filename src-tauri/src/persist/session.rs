use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::atomic_write;
use crate::audio::cue_points::CuePoints;
use crate::library::db::SavedPlaylistKind;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionPlaylistItem {
    Track {
        id: i64,
        /// The item's cue-point override, so a custom airing survives a
        /// restart. Additive and defaulted: a session file written before
        /// overrides existed loads with none, as does a stop marker.
        #[serde(default, rename = "cueOverride", alias = "cue_override")]
        cue_override: Option<CuePoints>,
    },
    Stop,
}

/// One queued occurrence persisted by id rather than by a stale track snapshot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueChild {
    /// This occurrence's stable id, distinct from the library track id.
    pub id: String,
    /// The track reference or Stop marker in this position.
    pub item: SessionPlaylistItem,
}

/// The top-level queue structure. A block is a copy of a saved list, with no
/// link back to the saved original after insertion.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionQueueNode {
    /// One standalone track or Stop marker.
    Standalone {
        /// This occurrence's stable id.
        id: String,
        /// The item at this position.
        item: SessionPlaylistItem,
    },
    /// One independently queued copy of a saved playlist or prepared show.
    Block {
        /// This block occurrence's stable id.
        id: String,
        /// Title copied when the block was queued.
        title: String,
        /// Saved-list kind copied when the block was queued.
        #[serde(rename = "savedKind")]
        saved_kind: SavedPlaylistKind,
        /// Remaining children in authored or locally edited order.
        entries: Vec<SessionQueueChild>,
    },
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(default)]
    pub playlist_ids: Vec<i64>,
    #[serde(default)]
    pub playlist_items: Vec<SessionPlaylistItem>,
    /// Structured queue. `None` means a legacy flat session; `Some([])` is an
    /// intentionally empty queue and must not fall back to `playlist_items`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<SessionQueueNode>>,
    /// Active block, also retained while an interior Stop waits with no track
    /// on air. The engine clears it when that block finishes.
    #[serde(default)]
    pub current_block_id: Option<String>,
    /// Renderer-owned disclosure state; it has no effect on advancement.
    #[serde(default)]
    pub collapsed_block_ids: Vec<String>,
    #[serde(default)]
    pub current_track_id: Option<i64>,
    #[serde(default)]
    pub current_time: f64,
    /// The override the restored track was airing under, if any.
    #[serde(default)]
    pub current_cue_override: Option<CuePoints>,
    #[serde(default)]
    pub auto_playlist_active: bool,
    /// The saved playlist the auto-playlist draws from; `None` is the music
    /// library.
    #[serde(default)]
    pub auto_playlist_source: Option<i64>,
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
            queue: None,
            current_block_id: None,
            collapsed_block_ids: vec![],
            current_track_id: None,
            current_time: 0.0,
            current_cue_override: None,
            auto_playlist_active: false,
            auto_playlist_source: None,
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

    /// Write the session to disk and stage it as the last known state.
    ///
    /// The `last` lock is held across the write, which is what serialises two
    /// saves. `save_session` runs on the blocking pool, so the renderer's
    /// throttled save and the `flushSave` on close can be in flight together —
    /// `flushSave` cancels the timer, not a call already made. Two unsynchronised
    /// writes would interleave, and the one that finished last would decide the
    /// file whether or not it was the newer state. Under the lock, `last` always
    /// names what is on disk.
    ///
    /// The write itself is tmp+rename, so a reader never sees a half-written
    /// file and a crash mid-save leaves the previous session intact.
    pub fn save(&self, state: SessionState) -> Result<()> {
        let json = serde_json::to_string_pretty(&state)?;
        let mut last = self.last.lock();
        atomic_write(&self.path, json.as_bytes()).context("write session.json")?;
        *last = state;
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
    fn blocks_and_occurrence_ids_survive_a_session_round_trip() {
        let dir = tempdir().unwrap();
        let session = Session::open(dir.path());
        let track = |id| SessionPlaylistItem::Track {
            id,
            cue_override: Some(CuePoints {
                cue_in_ms: Some(1_000),
                ..CuePoints::default()
            }),
        };
        let state = SessionState {
            queue: Some(vec![
                SessionQueueNode::Block {
                    id: "block-1".into(),
                    title: "Friday".into(),
                    saved_kind: SavedPlaylistKind::PreparedShow,
                    entries: vec![
                        SessionQueueChild {
                            id: "child-1".into(),
                            item: track(7),
                        },
                        SessionQueueChild {
                            id: "child-2".into(),
                            item: SessionPlaylistItem::Stop,
                        },
                    ],
                },
                SessionQueueNode::Block {
                    id: "block-2".into(),
                    title: "Friday".into(),
                    saved_kind: SavedPlaylistKind::PreparedShow,
                    entries: vec![SessionQueueChild {
                        id: "child-3".into(),
                        item: track(7),
                    }],
                },
            ]),
            current_track_id: Some(7),
            current_time: 33.5,
            current_block_id: Some("block-1".into()),
            collapsed_block_ids: vec!["block-2".into()],
            ..SessionState::default()
        };
        session.save(state.clone()).unwrap();
        let reloaded = Session::open(dir.path()).load();
        assert_eq!(reloaded.queue, state.queue);
        assert_eq!(reloaded.current_block_id, state.current_block_id);
        assert_eq!(reloaded.collapsed_block_ids, state.collapsed_block_ids);
        assert_eq!(reloaded.current_track_id, Some(7));
        assert_eq!(reloaded.current_time, 33.5);
        let json = serde_json::to_value(&reloaded).unwrap();
        assert_eq!(json["queue"][0]["savedKind"], "prepared_show");
        assert_eq!(
            json["queue"][0]["entries"][0]["item"]["cueOverride"]["cue_in_ms"],
            1_000
        );
        assert_eq!(json["currentBlockId"], "block-1");
        assert_eq!(json["collapsedBlockIds"][0], "block-2");
    }

    #[test]
    fn absent_structured_queue_keeps_legacy_flat_session_distinct_from_empty_queue() {
        let old: SessionState = serde_json::from_str(
            r#"{"playlistItems":[{"kind":"track","id":3},{"kind":"stop"}],"currentTrackId":3}"#,
        )
        .unwrap();
        assert!(old.queue.is_none());
        assert!(old.current_block_id.is_none());
        assert!(old.collapsed_block_ids.is_empty());
        assert_eq!(old.playlist_items.len(), 2);

        let empty: SessionState =
            serde_json::from_str(r#"{"queue":[],"playlistItems":[{"kind":"track","id":3}]}"#)
                .unwrap();
        assert_eq!(empty.queue, Some(Vec::new()));
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

    /// The renderer's throttled save and the `flushSave` on close can be in
    /// flight together now that `save_session` answers on the blocking pool.
    #[test]
    fn concurrent_saves_leave_a_whole_file() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let session = Session::open(path);
        std::thread::scope(|scope| {
            for n in 0..8 {
                let session = &session;
                scope.spawn(move || {
                    for _ in 0..25 {
                        session
                            .save(SessionState {
                                playlist_ids: vec![n; 64],
                                ..Default::default()
                            })
                            .unwrap();
                        // Every read must see a complete file, never a write
                        // half-way through.
                        let reloaded = Session::open(path).load();
                        assert_eq!(reloaded.playlist_ids.len(), 64);
                        assert!(reloaded
                            .playlist_ids
                            .iter()
                            .all(|&id| id == reloaded.playlist_ids[0]));
                    }
                });
            }
        });
        // What `load` answers is what a restart would read back.
        assert_eq!(
            session.load().playlist_ids,
            Session::open(dir.path()).load().playlist_ids
        );
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
        let json = serde_json::to_value(&loaded).unwrap();
        assert_eq!(json["playlistItems"][0]["cueOverride"]["cue_in_ms"], 2_000);

        let legacy: SessionPlaylistItem = serde_json::from_value(serde_json::json!({
            "kind": "track",
            "id": 7,
            "cue_override": override_points,
        }))
        .unwrap();
        assert_eq!(
            legacy,
            SessionPlaylistItem::Track {
                id: 7,
                cue_override: Some(override_points),
            }
        );
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
        assert_eq!(loaded.current_track_id, None);
        assert_eq!(loaded.current_time, 0.0);
        assert_eq!(loaded.volume, 0.4);
        assert!(!loaded.auto_advance);
        assert!(dir.path().join("session.legacy.bak.json").exists());
    }
}
