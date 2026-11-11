//! Wire types shared by the playlist engine, the service, and the renderer.

use serde::{Deserialize, Serialize};

use super::timing::PlaylistTiming;
use crate::audio::cue_points::CuePoints;
use crate::library::db::{SavedPlaylistKind, Track};
use crate::persist::session::SessionPlaylistItem;

/// One entry in the playlist.
///
/// Carries the whole [`Track`], not just its id: the snapshot the renderer
/// mirrors has to be displayable on its own, without a second round-trip to
/// resolve titles.
// A stop marker next to a whole Track is a lopsided enum, and boxing would fix
// that — at the cost of an allocation per queued item, to save a few kilobytes
// across a playlist that is twenty entries long.
#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlaylistItem {
    Track {
        track: Track,
        /// Cue points for this one airing, overriding the track's radio edit.
        ///
        /// `None` — the common case — means the item *references* the track, so
        /// correcting a radio edit corrects every queued airing of it. An
        /// all-`NULL` override is representable and distinct: it says "play the
        /// whole file this once".
        #[serde(default)]
        cue_override: Option<CuePoints>,
    },
    /// Barrier that halts auto-advance when playback reaches it.
    Stop {
        /// Presenter’s planned pause in whole seconds; `None` is unspecified.
        #[serde(default, rename = "plannedDurationSeconds")]
        planned_duration_seconds: Option<u32>,
    },
}

impl PlaylistItem {
    pub fn track(track: Track) -> Self {
        Self::Track {
            track,
            cue_override: None,
        }
    }

    pub fn with_override(track: Track, cue_override: Option<CuePoints>) -> Self {
        Self::Track {
            track,
            cue_override,
        }
    }

    pub fn as_track(&self) -> Option<&Track> {
        match self {
            Self::Track { track, .. } => Some(track),
            Self::Stop { .. } => None,
        }
    }

    pub fn is_stop(&self) -> bool {
        matches!(self, Self::Stop { .. })
    }

    /// Rebuild a persisted playlist, resolving ids through `lookup`.
    ///
    /// `items` is authoritative; `legacy_ids` is the pre-stop-marker format and
    /// is only consulted when `items` is absent, so a session file written by an
    /// older build still loads. Ids the library no longer has are dropped — a
    /// pruned track cannot be aired, and a hole in the playlist is better than a
    /// load that fails on the deck.
    pub fn from_session<F>(
        items: &[SessionPlaylistItem],
        legacy_ids: &[i64],
        lookup: F,
    ) -> Vec<Self>
    where
        F: Fn(i64) -> Option<Track>,
    {
        if items.is_empty() {
            return legacy_ids
                .iter()
                .filter_map(|id| lookup(*id))
                .map(Self::track)
                .collect();
        }
        items
            .iter()
            .filter_map(|item| match item {
                SessionPlaylistItem::Stop {
                    planned_duration_seconds,
                } => Some(Self::Stop {
                    planned_duration_seconds: *planned_duration_seconds,
                }),
                SessionPlaylistItem::Track { id, cue_override } => {
                    lookup(*id).map(|track| Self::with_override(track, *cue_override))
                }
            })
            .collect()
    }
}

/// One queued occurrence inside a copied saved-playlist block.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueChild {
    /// Identity of this airing, independent of its track id.
    pub id: String,
    /// Track or Stop at this position.
    pub item: PlaylistItem,
}

/// A top-level queue occurrence: one item or a copied saved list.
// A standalone node carries a whole Track for the renderer. Boxing that one
// case would allocate every ordinary queued row to shrink a small queue enum.
#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum QueueNode {
    /// An ordinary track or Stop outside any block.
    Standalone {
        /// Identity of this occurrence.
        id: String,
        /// Queued content.
        item: PlaylistItem,
    },
    /// An independent queued copy of a saved playlist or prepared show.
    Block {
        /// Identity of this queued copy.
        id: String,
        /// Title copied when the block was queued.
        title: String,
        /// Saved kind copied when the block was queued.
        saved_kind: SavedPlaylistKind,
        /// Remaining children in authored or operator-edited order.
        entries: Vec<QueueChild>,
    },
}

/// A gap in the queue, addressed by occurrence identity rather than row index.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(
    tag = "scope",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Destination {
    /// A gap between top-level nodes; `None` appends.
    Main {
        /// Top-level occurrence immediately after the gap.
        #[serde(default)]
        before_id: Option<String>,
    },
    /// A gap among one block's remaining children; `None` appends inside it.
    Block {
        /// Block receiving the item.
        block_id: String,
        /// Child occurrence immediately after the gap.
        #[serde(default)]
        before_id: Option<String>,
    },
}

/// Whole-playlist snapshot, emitted on `program:playlist-state` after every
/// mutation and every advance.
///
/// A snapshot rather than a delta: the playlist is small, snapshots are
/// idempotent, and a dropped or reordered event cannot leave the operator UI
/// showing something other than what is going to air.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Whole-snapshot generation, including schedule changes.
    pub revision: u64,
    /// The consumed Stop, distinct from an ordinary cleared deck.
    pub holding_stop: Option<HoldingStop>,
    /// One schedule for all upcoming occurrences.
    pub timing: PlaylistTiming,
    /// Upcoming top-level items and copied blocks in order.
    pub playlist: Vec<QueueNode>,
    /// Block whose child is on air, or whose remaining children are waiting
    /// behind an interior Stop. Its header stays visible while active.
    #[serde(default)]
    pub current_block_id: Option<String>,
    /// The track on the main deck.
    pub current: Option<Track>,
    /// What has aired, oldest first, capped by the stored tuning's
    /// `historyCap`. Carried whole rather than as an append event: a snapshot
    /// is idempotent, so a dropped or reordered one cannot leave the History
    /// tab showing a different past than the log holds.
    pub history: Vec<Track>,
    pub auto_playlist_active: bool,
    pub auto_advance: bool,
    /// The override the track on air is playing under, if it came off an item
    /// that carried one. The renderer's optimistic duration reads it, so a
    /// custom airing does not show file time for a frame before the deck
    /// reports its own.
    pub current_override: Option<CuePoints>,
    /// Playback is blocked waiting for the media share to come back. Drives the
    /// reconnecting indicator.
    pub awaiting_network: bool,
    /// The saved playlist the auto-playlist draws its music from. `None` is the
    /// music library. See `docs/saved-playlists.md`.
    #[serde(default)]
    pub source: Option<SourceInfo>,
    /// A source that ran out of playable music and was dropped for the music
    /// library, by name, until the operator chooses a source again.
    #[serde(default)]
    pub reverted_from: Option<String>,
}

/// A consumed Stop and a coherent sample of its local elapsed time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HoldingStop {
    /// Consumed occurrence, so consecutive Stops start distinct holds.
    pub occurrence_id: String,
    /// Intended pause; an explicit zero differs from an unspecified plan.
    pub planned_duration_seconds: Option<u32>,
    /// Monotonic elapsed time at `captured_at_unix_ms`.
    pub elapsed_seconds: f64,
    /// Wall-clock anchor of this elapsed sample.
    pub captured_at_unix_ms: f64,
    /// Copied block association, including a trailing Stop.
    pub block_id: Option<String>,
}

/// An auto-playlist source as the panel shows it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: i64,
    pub name: String,
    /// Distinct music tracks it can put on air.
    pub tracks: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64) -> Track {
        Track {
            id,
            title: format!("t{}", id),
            artist: "a".into(),
            album: "al".into(),
            duration: 100.0,
            play_count: 0,
            genre: None,
            year: None,
            bpm: None,
            sample_rate: None,
            bitrate: None,
            format: None,
            cue_points: Default::default(),
            ..Default::default()
        }
    }

    fn session_track(id: i64) -> SessionPlaylistItem {
        SessionPlaylistItem::Track {
            id,
            cue_override: None,
        }
    }

    /// Stands in for the library: ids 1..=9 exist, anything else was pruned.
    fn library(id: i64) -> Option<Track> {
        (1..=9).contains(&id).then(|| track(id))
    }

    fn cue_override(item: &PlaylistItem) -> Option<CuePoints> {
        match item {
            PlaylistItem::Track { cue_override, .. } => *cue_override,
            PlaylistItem::Stop { .. } => None,
        }
    }

    fn ids(items: &[PlaylistItem]) -> Vec<Option<i64>> {
        items.iter().map(|i| i.as_track().map(|t| t.id)).collect()
    }

    #[test]
    fn from_session_rebuilds_items_including_stop_markers() {
        let items = vec![
            session_track(1),
            SessionPlaylistItem::Stop {
                planned_duration_seconds: None,
            },
            session_track(2),
        ];
        let rebuilt = PlaylistItem::from_session(&items, &[], library);
        assert_eq!(ids(&rebuilt), vec![Some(1), None, Some(2)]);
    }

    #[test]
    fn from_session_falls_back_to_legacy_ids_when_items_are_absent() {
        let rebuilt = PlaylistItem::from_session(&[], &[3, 4], library);
        assert_eq!(ids(&rebuilt), vec![Some(3), Some(4)]);
    }

    #[test]
    fn from_session_drops_ids_the_library_no_longer_has() {
        let items = vec![session_track(1), session_track(99), session_track(2)];
        assert_eq!(
            ids(&PlaylistItem::from_session(&items, &[], library)),
            vec![Some(1), Some(2)]
        );
        assert_eq!(
            ids(&PlaylistItem::from_session(&[], &[1, 99, 2], library)),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn from_session_restores_item_overrides() {
        let points = CuePoints {
            cue_in_ms: Some(2_000),
            ..Default::default()
        };
        let items = vec![
            SessionPlaylistItem::Track {
                id: 1,
                cue_override: Some(points),
            },
            session_track(2),
        ];
        let rebuilt = PlaylistItem::from_session(&items, &[], library);
        assert_eq!(cue_override(&rebuilt[0]), Some(points));
        assert_eq!(cue_override(&rebuilt[1]), None);
    }

    /// The override is additive on the wire too: an item serialized before it
    /// existed still deserializes, as one referencing its track.
    #[test]
    fn a_wire_item_without_an_override_references_its_track() {
        let json = serde_json::json!({
            "kind": "track",
            "track": serde_json::to_value(track(1)).unwrap(),
        });
        let item: PlaylistItem = serde_json::from_value(json).unwrap();
        assert_eq!(item.as_track().map(|t| t.id), Some(1));
        assert_eq!(cue_override(&item), None);
    }

    #[test]
    fn from_session_of_an_empty_file_is_empty() {
        assert!(PlaylistItem::from_session(&[], &[], library).is_empty());
    }

    #[test]
    fn block_and_destination_wire_use_camel_case_fields() {
        let node = QueueNode::Block {
            id: "block-1".into(),
            title: "Show".into(),
            saved_kind: SavedPlaylistKind::PreparedShow,
            entries: vec![QueueChild {
                id: "child-1".into(),
                item: PlaylistItem::Stop {
                    planned_duration_seconds: None,
                },
            }],
        };
        let value = serde_json::to_value(&node).unwrap();
        assert_eq!(value["kind"], "block");
        assert_eq!(value["savedKind"], "prepared_show");
        assert_eq!(value["entries"][0]["id"], "child-1");
        assert_eq!(serde_json::from_value::<QueueNode>(value).unwrap(), node);

        let destination = Destination::Block {
            block_id: "block-1".into(),
            before_id: Some("child-1".into()),
        };
        let value = serde_json::to_value(&destination).unwrap();
        assert_eq!(value["scope"], "block");
        assert_eq!(value["blockId"], "block-1");
        assert_eq!(value["beforeId"], "child-1");
        assert_eq!(
            serde_json::from_value::<Destination>(value).unwrap(),
            destination
        );
    }
}
