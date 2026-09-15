//! Wire types shared by the playlist engine, the service, and the renderer.

use serde::{Deserialize, Serialize};

use crate::library::db::Track;
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
    },
    /// Barrier that halts auto-advance when playback reaches it.
    Stop,
}

impl PlaylistItem {
    pub fn track(track: Track) -> Self {
        Self::Track { track }
    }

    pub fn as_track(&self) -> Option<&Track> {
        match self {
            Self::Track { track } => Some(track),
            Self::Stop => None,
        }
    }

    pub fn is_stop(&self) -> bool {
        matches!(self, Self::Stop)
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
                SessionPlaylistItem::Stop => Some(Self::Stop),
                SessionPlaylistItem::Track { id } => lookup(*id).map(Self::track),
            })
            .collect()
    }
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
    /// Upcoming items in order, stop markers included.
    pub playlist: Vec<PlaylistItem>,
    /// The track on the main deck.
    pub current: Option<Track>,
    /// The track that just left the main deck, if any. The renderer appends it
    /// to history; it is set on exactly the transition that displaced it.
    pub displaced: Option<Track>,
    pub auto_playlist_active: bool,
    pub auto_advance: bool,
    /// Playback is blocked waiting for the media share to come back. Drives the
    /// reconnecting indicator.
    pub awaiting_network: bool,
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
        }
    }

    /// Stands in for the library: ids 1..=9 exist, anything else was pruned.
    fn library(id: i64) -> Option<Track> {
        (1..=9).contains(&id).then(|| track(id))
    }

    fn ids(items: &[PlaylistItem]) -> Vec<Option<i64>> {
        items.iter().map(|i| i.as_track().map(|t| t.id)).collect()
    }

    #[test]
    fn from_session_rebuilds_items_including_stop_markers() {
        let items = vec![
            SessionPlaylistItem::Track { id: 1 },
            SessionPlaylistItem::Stop,
            SessionPlaylistItem::Track { id: 2 },
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
        let items = vec![
            SessionPlaylistItem::Track { id: 1 },
            SessionPlaylistItem::Track { id: 99 },
            SessionPlaylistItem::Track { id: 2 },
        ];
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
    fn from_session_of_an_empty_file_is_empty() {
        assert!(PlaylistItem::from_session(&[], &[], library).is_empty());
    }
}
