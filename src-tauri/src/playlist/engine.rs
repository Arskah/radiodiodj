//! The playlist state machine.
//!
//! Pure by construction: a transition takes the current state plus whatever the
//! caller already knows and returns the [`Effect`]s the service must carry out.
//! No database, no audio device, no Tauri handle. The advancement specification
//! — ordering, outage skip-to-cached, refill sizing, stop markers — is therefore
//! testable here, which is where it moved to from the renderer.

use std::collections::HashSet;

use super::model::{PlaylistItem, Snapshot};
use crate::audio::cue_points::CuePoints;
use crate::library::db::Track;

/// Source of auto-playlist refill material, plus the sizing that governs it.
///
/// Implemented over the library database in the service and faked in tests. The
/// sizes are read per call so a settings change takes effect on the next refill
/// without a restart, matching how the interleave cadence already behaves.
pub trait Refiller {
    /// An interleaved block of `count` tracks, excluding ids already queued.
    fn generate(&self, count: i64, exclude: &[i64]) -> Vec<Track>;
    /// Target number of upcoming tracks the auto-playlist keeps queued.
    fn buffer(&self) -> i64;
    /// Refill once fewer than this many remain.
    fn threshold(&self) -> i64;
}

/// Something the service has to do in the world after a transition.
///
/// Prefetch-window updates and snapshot emission are deliberately absent: they
/// follow *every* transition, so the service does them unconditionally rather
/// than each transition having to remember to ask.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Load the track on the main deck and start playback.
    ///
    /// The override travels with the effect because the item it came off has
    /// already been consumed by the time the service runs this: nothing can
    /// look it up again afterwards. `None` means the service reads the track's
    /// radio edit out of the library.
    Play {
        id: i64,
        cue_override: Option<CuePoints>,
    },
    /// Load the track on the main deck, seek, and leave it paused — session
    /// resume, which restores position without putting audio on air.
    Resume {
        id: i64,
        seconds: f64,
        cue_override: Option<CuePoints>,
    },
    /// Stop the main deck.
    Stop,
    /// Count the airing against the track's play count.
    TrackPlayed(i64),
    /// Arm the outage retry timer. The index selects a delay from the backoff
    /// schedule, saturating at its last entry.
    ArmRetry(usize),
    /// Cancel any armed outage retry.
    CancelRetry,
}

/// The result of one transition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transition {
    pub effects: Vec<Effect>,
    /// The track that just left the main deck. The renderer appends it to
    /// history, so it is set only where history should actually grow: an
    /// explicit track change or a stop, never a session restore and never
    /// `prev` (which returns the outgoing track to the playlist instead).
    pub displaced: Option<Track>,
}

impl Transition {
    fn effects(effects: Vec<Effect>) -> Self {
        Self {
            effects,
            displaced: None,
        }
    }
}

/// What to play next, given what is queued and what is cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Nothing queued.
    Empty,
    /// No cache knowledge yet (cold start) — fall back to playing the head.
    Fallback,
    /// A stop marker is the next barrier; honour it.
    Stop(usize),
    /// The first upcoming cached track. Uncached tracks ahead of it stay queued
    /// for when the share recovers.
    Play(usize),
    /// Cache membership is known but nothing upcoming is in it — an outage.
    Wait,
}

#[derive(Default)]
pub struct Playlist {
    items: Vec<PlaylistItem>,
    current: Option<Track>,
    /// The override the track on air is playing under. Held next to `current`
    /// rather than folded into it so `prev` can return the item to the queue
    /// with its override intact — the defect the renderer-owned playlist had.
    current_override: Option<CuePoints>,
    auto_playlist: bool,
    auto_advance: bool,
    /// Track ids resident in the prefetch cache, from `main-deck:cache-state`.
    cached_ids: HashSet<i64>,
    awaiting_network: bool,
    /// Index into the backoff schedule for the next retry arm.
    retry_attempt: usize,
    /// A retry timer is pending. Guards against shortening the backoff by
    /// re-arming on every failure that arrives while one is already running.
    retry_armed: bool,
}

impl Playlist {
    pub fn new() -> Self {
        Self {
            auto_advance: true,
            ..Default::default()
        }
    }

    // ----- projections -----

    pub fn snapshot(&self, displaced: Option<Track>) -> Snapshot {
        Snapshot {
            playlist: self.items.clone(),
            current: self.current.clone(),
            displaced,
            auto_playlist_active: self.auto_playlist,
            auto_advance: self.auto_advance,
            current_override: self.current_override,
            awaiting_network: self.awaiting_network,
        }
    }

    /// Prefetch residency window: the track on air first, then the whole
    /// playlist in order. Stop markers contribute nothing. The cache's byte cap
    /// decides how many leading entries actually stay resident.
    pub fn prefetch_window(&self) -> Vec<i64> {
        self.current
            .iter()
            .map(|t| t.id)
            .chain(self.items.iter().filter_map(|i| i.as_track().map(|t| t.id)))
            .collect()
    }

    // ----- queue mutations -----

    pub fn add(&mut self, track: Track) -> Transition {
        self.items.push(PlaylistItem::track(track));
        Transition::default()
    }

    /// Insert at the head as next-up — cue promotion, and the outgoing track on
    /// `prev`.
    ///
    /// `cue_override` is what the operator auditioned on the cue deck when it
    /// differs from the track's radio edit; `None` leaves the item referencing
    /// the track.
    pub fn add_front(&mut self, track: Track, cue_override: Option<CuePoints>) -> Transition {
        self.items
            .insert(0, PlaylistItem::with_override(track, cue_override));
        Transition::default()
    }

    /// Set or clear one queued item's override. `None` drops it back to the
    /// track's radio edit; an all-`NULL` `Some` is a deliberate "whole file
    /// this once" and is kept as such.
    pub fn set_item_cue_points(
        &mut self,
        index: usize,
        cue_override: Option<CuePoints>,
    ) -> Transition {
        if let Some(PlaylistItem::Track { track, .. }) = self.items.get(index) {
            self.items[index] = PlaylistItem::with_override(track.clone(), cue_override);
        }
        Transition::default()
    }

    pub fn add_stop(&mut self) -> Transition {
        self.items.push(PlaylistItem::Stop);
        Transition::default()
    }

    pub fn remove(&mut self, index: usize) -> Transition {
        if index < self.items.len() {
            self.items.remove(index);
        }
        Transition::default()
    }

    /// Drop every queued item for these tracks: their rows were purged.
    pub fn remove_tracks(&mut self, ids: &HashSet<i64>) -> Transition {
        self.items
            .retain(|item| item.as_track().is_none_or(|t| !ids.contains(&t.id)));
        Transition::default()
    }

    pub fn move_item(&mut self, from: usize, to: usize) -> Transition {
        if from == to || from >= self.items.len() || to >= self.items.len() {
            return Transition::default();
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        Transition::default()
    }

    pub fn clear(&mut self) -> Transition {
        self.items.clear();
        Transition::default()
    }

    // ----- transport -----

    /// Pull the item at `index` out of the playlist and act on it: play a
    /// track, or honour a stop marker by stopping (consuming it either way).
    pub fn play_index(&mut self, index: usize, r: &dyn Refiller) -> Transition {
        if index >= self.items.len() {
            return Transition::default();
        }
        match self.items.remove(index) {
            PlaylistItem::Stop => self.stop(),
            PlaylistItem::Track {
                track,
                cue_override,
            } => self.play_track(track, cue_override, r),
        }
    }

    /// Put a track straight on air, bypassing the playlist.
    pub fn play_now(&mut self, track: Track, r: &dyn Refiller) -> Transition {
        self.play_track(track, None, r)
    }

    pub fn next(&mut self, r: &dyn Refiller) -> Transition {
        if self.items.is_empty() {
            return Transition::default();
        }
        self.play_index(0, r)
    }

    /// Step back to `previous`, returning whatever is on air to the head of the
    /// playlist. History is the renderer's, so the caller supplies the track and
    /// keeps its own entry: `displaced` stays `None` here on purpose, or the
    /// renderer would append the outgoing track to a history it is stepping back
    /// through.
    pub fn prev(&mut self, previous: Track, r: &dyn Refiller) -> Transition {
        if let Some(current) = self.current.take() {
            self.items.insert(
                0,
                PlaylistItem::with_override(current, self.current_override),
            );
        }
        // The track being stepped back to comes off the renderer's history,
        // which stores tracks rather than items — a custom airing replays under
        // the radio edit.
        self.set_current(previous, None, r)
    }

    pub fn stop(&mut self) -> Transition {
        self.clear_retry();
        let displaced = self.current.take();
        self.current_override = None;
        self.auto_playlist = false;
        Transition {
            effects: vec![Effect::CancelRetry, Effect::Stop],
            displaced,
        }
    }

    pub fn set_auto_advance(&mut self, on: bool) -> Transition {
        self.auto_advance = on;
        Transition::default()
    }

    /// Toggle the auto-playlist. Switching it on tops the playlist up and, if
    /// nothing is on air, starts the show.
    pub fn set_auto_playlist(&mut self, on: bool, r: &dyn Refiller) -> Transition {
        self.auto_playlist = on;
        if !on {
            return Transition::default();
        }
        self.refill(r);
        if self.current.is_none() && !self.items.is_empty() {
            return self.play_index(0, r);
        }
        Transition::default()
    }

    // ----- deck events -----

    pub fn on_ended(&mut self, r: &dyn Refiller) -> Transition {
        if !self.auto_advance {
            return self.stop();
        }
        self.refill(r);
        self.advance(false, r)
    }

    /// A read failed after retries, or the watchdog fired. Skip to the next
    /// cached track (or wait for the share) rather than sitting in dead air.
    pub fn on_load_failed(&mut self, r: &dyn Refiller) -> Transition {
        if !self.auto_advance {
            return Transition::default();
        }
        self.advance(true, r)
    }

    /// Cache membership changed. If playback is stalled waiting for the share, a
    /// newly-cached track may now be playable.
    pub fn on_cache_state(&mut self, ids: Vec<i64>, r: &dyn Refiller) -> Transition {
        self.cached_ids = ids.into_iter().collect();
        if self.awaiting_network {
            return self.advance(true, r);
        }
        Transition::default()
    }

    /// The outage retry timer fired. The service re-pushes the prefetch window
    /// on every transition, which is what actually wakes the cache worker — it
    /// only reads on a window update, so without that an outage would never
    /// recover on its own.
    pub fn on_retry_tick(&mut self, r: &dyn Refiller) -> Transition {
        self.retry_armed = false;
        self.advance(true, r)
    }

    /// A radio edit was saved for `id`. Queued items carry a copy of the track
    /// for display, so the copies are refreshed here — otherwise the next
    /// snapshot would hand the renderer the duration the track had before the
    /// edit. Items with an override are copied into as well: what they *air*
    /// is their own markers, but the track underneath them still changed.
    ///
    /// The track on air is deliberately left alone. A radio edit saved
    /// mid-broadcast applies from the next airing, so its numbers must not move
    /// under the operator while it is playing.
    pub fn on_cue_points_saved(&mut self, id: i64, points: CuePoints) -> Transition {
        for item in &mut self.items {
            if let PlaylistItem::Track { track, .. } = item {
                if track.id == id {
                    track.cue_points = points;
                }
            }
        }
        Transition::default()
    }

    // ----- session -----

    /// Restore a persisted playlist. The current track is loaded and seeked but
    /// not played: a restart must not put audio on air by itself.
    #[allow(clippy::too_many_arguments)]
    pub fn hydrate(
        &mut self,
        items: Vec<PlaylistItem>,
        current: Option<Track>,
        current_override: Option<CuePoints>,
        seconds: f64,
        auto_playlist: bool,
        auto_advance: bool,
    ) -> Transition {
        self.items = items;
        self.auto_playlist = auto_playlist;
        self.auto_advance = auto_advance;
        self.current = current;
        self.current_override = current_override;
        match &self.current {
            // Clamped rather than trusted: a negative position from a mangled
            // session file would seek out of range and silently skip the track
            // at launch instead of resuming it.
            Some(track) => Transition::effects(vec![Effect::Resume {
                id: track.id,
                seconds: seconds.max(0.0),
                cue_override: current_override,
            }]),
            None => Transition::default(),
        }
    }

    // ----- internals -----

    fn play_track(
        &mut self,
        track: Track,
        cue_override: Option<CuePoints>,
        r: &dyn Refiller,
    ) -> Transition {
        let displaced = self.current.take();
        let mut transition = self.set_current(track, cue_override, r);
        transition.displaced = displaced;
        transition
    }

    /// Put `track` on air. Any pending outage retry is superseded: without
    /// that, an armed timer fires after the new track loads and advances again,
    /// skipping it.
    fn set_current(
        &mut self,
        track: Track,
        cue_override: Option<CuePoints>,
        r: &dyn Refiller,
    ) -> Transition {
        self.clear_retry();
        let id = track.id;
        self.current = Some(track);
        self.current_override = cue_override;
        self.refill(r);
        Transition::effects(vec![
            Effect::CancelRetry,
            Effect::Play { id, cue_override },
            Effect::TrackPlayed(id),
        ])
    }

    fn advance(&mut self, after_failure: bool, r: &dyn Refiller) -> Transition {
        match self.plan() {
            Plan::Empty => {
                self.clear_retry();
                Transition::effects(vec![Effect::CancelRetry])
            }
            // A cold offline start has no cache knowledge. After a failure that
            // means "wait for the share" — blindly retrying the head would burn
            // through the whole playlist.
            Plan::Fallback if after_failure => self.arm_retry(),
            Plan::Fallback => self.play_index(0, r),
            Plan::Stop(index) | Plan::Play(index) => self.play_index(index, r),
            Plan::Wait => self.arm_retry(),
        }
    }

    fn plan(&self) -> Plan {
        if self.items.is_empty() {
            return Plan::Empty;
        }
        if self.cached_ids.is_empty() {
            return Plan::Fallback;
        }
        for (i, item) in self.items.iter().enumerate() {
            match item {
                PlaylistItem::Stop => return Plan::Stop(i),
                PlaylistItem::Track { track, .. } if self.cached_ids.contains(&track.id) => {
                    return Plan::Play(i)
                }
                PlaylistItem::Track { .. } => {}
            }
        }
        Plan::Wait
    }

    fn refill(&mut self, r: &dyn Refiller) {
        if !self.auto_playlist {
            return;
        }
        // A stop marker is a barrier the operator placed deliberately; topping
        // up past it would bury it under generated tracks.
        if self.items.iter().any(PlaylistItem::is_stop) {
            return;
        }
        let queued = self.items.len() as i64;
        if queued >= r.threshold() {
            return;
        }
        let exclude: Vec<i64> = self
            .items
            .iter()
            .filter_map(|i| i.as_track().map(|t| t.id))
            .collect();
        let tracks = r.generate(r.buffer() - queued, &exclude);
        self.items
            .extend(tracks.into_iter().map(PlaylistItem::track));
    }

    fn arm_retry(&mut self) -> Transition {
        self.awaiting_network = true;
        if self.retry_armed {
            return Transition::default();
        }
        self.retry_armed = true;
        let attempt = self.retry_attempt;
        self.retry_attempt += 1;
        Transition::effects(vec![Effect::ArmRetry(attempt)])
    }

    fn clear_retry(&mut self) {
        self.retry_armed = false;
        self.retry_attempt = 0;
        self.awaiting_network = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn track(id: i64) -> Track {
        Track {
            id,
            title: format!("t{}", id),
            artist: format!("a{}", id),
            album: format!("al{}", id),
            duration: 100.0,
            play_count: 0,
            genre: None,
            year: None,
            bpm: None,
            sample_rate: None,
            bitrate: None,
            format: None,
            cue_points: Default::default(),
        }
    }

    /// Records what the engine asked for. `generating` hands back as many fresh
    /// tracks as were requested, the way the real generator does; the default
    /// stands in for an empty library and returns nothing.
    struct FakeRefiller {
        generating: bool,
        buffer: i64,
        threshold: i64,
        calls: RefCell<Vec<(i64, Vec<i64>)>>,
        next_id: RefCell<i64>,
    }

    impl FakeRefiller {
        fn new() -> Self {
            Self {
                generating: false,
                buffer: 20,
                threshold: 5,
                calls: RefCell::new(vec![]),
                next_id: RefCell::new(1000),
            }
        }

        fn generating(buffer: i64, threshold: i64) -> Self {
            Self {
                generating: true,
                buffer,
                threshold,
                ..Self::new()
            }
        }

        fn calls(&self) -> Vec<(i64, Vec<i64>)> {
            self.calls.borrow().clone()
        }
    }

    impl Refiller for FakeRefiller {
        fn generate(&self, count: i64, exclude: &[i64]) -> Vec<Track> {
            self.calls.borrow_mut().push((count, exclude.to_vec()));
            if !self.generating {
                return vec![];
            }
            (0..count.max(0))
                .map(|_| {
                    let mut id = self.next_id.borrow_mut();
                    *id += 1;
                    track(*id)
                })
                .collect()
        }

        fn buffer(&self) -> i64 {
            self.buffer
        }

        fn threshold(&self) -> i64 {
            self.threshold
        }
    }

    /// A recognisable override — only the marker's presence matters here, the
    /// clamp and the resolution are tested where they live.
    fn points(cue_in_ms: i64) -> CuePoints {
        CuePoints {
            cue_in_ms: Some(cue_in_ms),
            ..Default::default()
        }
    }

    /// The effect an ordinary airing produces: no item override, so the service
    /// reads the track's radio edit.
    fn play(id: i64) -> Effect {
        Effect::Play {
            id,
            cue_override: None,
        }
    }

    /// Never asked for material — asserts refill stayed out of a transition.
    struct NoRefill;

    impl Refiller for NoRefill {
        fn generate(&self, _count: i64, _exclude: &[i64]) -> Vec<Track> {
            panic!("refill must not be requested here");
        }
        fn buffer(&self) -> i64 {
            20
        }
        fn threshold(&self) -> i64 {
            5
        }
    }

    fn queued(p: &Playlist) -> Vec<Option<i64>> {
        p.snapshot(None)
            .playlist
            .iter()
            .map(|i| i.as_track().map(|t| t.id))
            .collect()
    }

    /// The override carried by one queued item, as the renderer would read it
    /// off the snapshot.
    fn item_override(p: &Playlist, index: usize) -> Option<CuePoints> {
        match &p.snapshot(None).playlist[index] {
            PlaylistItem::Track { cue_override, .. } => *cue_override,
            PlaylistItem::Stop => None,
        }
    }

    fn current_id(p: &Playlist) -> Option<i64> {
        p.snapshot(None).current.map(|t| t.id)
    }

    fn with(items: &[Option<i64>]) -> Playlist {
        let mut p = Playlist::new();
        for item in items {
            match item {
                Some(id) => p.add(track(*id)),
                None => p.add_stop(),
            };
        }
        p
    }

    // ----- queue mutations -----

    #[test]
    fn add_appends_tracks() {
        let p = with(&[Some(1), Some(2)]);
        assert_eq!(queued(&p), vec![Some(1), Some(2)]);
    }

    #[test]
    fn add_front_inserts_at_the_head() {
        let mut p = with(&[Some(1), Some(2)]);
        p.add_front(track(9), None);
        assert_eq!(queued(&p), vec![Some(9), Some(1), Some(2)]);
    }

    #[test]
    fn remove_splices_without_touching_the_track_on_air() {
        let mut p = with(&[Some(1), Some(2), Some(3)]);
        p.play_index(0, &NoRefill);
        p.remove(0);
        assert_eq!(queued(&p), vec![Some(3)]);
        assert_eq!(current_id(&p), Some(1));
    }

    #[test]
    fn remove_tracks_drops_every_airing_of_them_and_keeps_stop_markers() {
        let mut p = with(&[Some(1), Some(2), None, Some(1), Some(3)]);
        p.remove_tracks(&HashSet::from([1, 3]));
        assert_eq!(queued(&p), vec![Some(2), None]);
    }

    #[test]
    fn remove_out_of_range_is_a_no_op() {
        let mut p = with(&[Some(1)]);
        p.remove(7);
        assert_eq!(queued(&p), vec![Some(1)]);
    }

    #[test]
    fn move_item_reorders() {
        let mut p = with(&[Some(1), Some(2), Some(3)]);
        p.move_item(2, 0);
        assert_eq!(queued(&p), vec![Some(3), Some(1), Some(2)]);
    }

    #[test]
    fn move_item_is_a_no_op_when_from_equals_to() {
        let mut p = with(&[Some(1), Some(2)]);
        assert_eq!(p.move_item(1, 1), Transition::default());
        assert_eq!(queued(&p), vec![Some(1), Some(2)]);
    }

    /// The renderer's version had no bounds check: `splice(to, 0, item)` with a
    /// `to` past the end silently appended. Dropping the move is the safer read
    /// of a request that cannot be satisfied.
    #[test]
    fn move_item_out_of_range_is_a_no_op() {
        let mut p = with(&[Some(1), Some(2), Some(3)]);
        assert_eq!(p.move_item(0, 9), Transition::default());
        assert_eq!(p.move_item(9, 0), Transition::default());
        assert_eq!(queued(&p), vec![Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn clear_empties_the_queue_but_leaves_the_track_on_air() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_index(0, &NoRefill);
        p.clear();
        assert!(queued(&p).is_empty());
        assert_eq!(current_id(&p), Some(1));
    }

    // ----- transport -----

    #[test]
    fn play_index_pulls_the_track_out_of_the_queue_and_plays_it() {
        let mut p = with(&[Some(1), Some(2)]);
        let t = p.play_index(1, &NoRefill);
        assert_eq!(queued(&p), vec![Some(1)]);
        assert_eq!(current_id(&p), Some(2));
        assert_eq!(
            t.effects,
            vec![Effect::CancelRetry, play(2), Effect::TrackPlayed(2)]
        );
        assert_eq!(t.displaced, None);
    }

    #[test]
    fn playing_a_new_track_displaces_the_previous_one() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_index(0, &NoRefill);
        let t = p.play_index(0, &NoRefill);
        assert_eq!(t.displaced.map(|d| d.id), Some(1));
        assert_eq!(current_id(&p), Some(2));
    }

    #[test]
    fn play_index_out_of_range_is_a_no_op() {
        let mut p = with(&[Some(1)]);
        assert_eq!(p.play_index(5, &NoRefill), Transition::default());
        assert_eq!(queued(&p), vec![Some(1)]);
        assert_eq!(current_id(&p), None);
    }

    #[test]
    fn play_index_onto_a_stop_marker_stops_and_consumes_it() {
        let mut p = with(&[Some(1), None, Some(2)]);
        p.play_now(track(9), &NoRefill);
        let t = p.play_index(1, &NoRefill);
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(t.displaced.map(|d| d.id), Some(9));
        assert_eq!(current_id(&p), None);
        assert_eq!(queued(&p), vec![Some(1), Some(2)]);
    }

    #[test]
    fn next_onto_a_stop_marker_halts() {
        let mut p = with(&[None, Some(5)]);
        p.play_now(track(9), &NoRefill);
        p.next(&NoRefill);
        assert_eq!(current_id(&p), None);
        assert_eq!(queued(&p), vec![Some(5)]);
    }

    #[test]
    fn play_now_airs_a_track_without_enqueuing_it() {
        let mut p = with(&[Some(1)]);
        let t = p.play_now(track(9), &NoRefill);
        assert_eq!(queued(&p), vec![Some(1)]);
        assert_eq!(current_id(&p), Some(9));
        assert!(t.effects.contains(&play(9)));
    }

    #[test]
    fn next_plays_the_head() {
        let mut p = with(&[Some(1), Some(2)]);
        p.next(&NoRefill);
        assert_eq!(current_id(&p), Some(1));
        assert_eq!(queued(&p), vec![Some(2)]);
    }

    #[test]
    fn next_is_a_no_op_when_the_queue_is_empty() {
        let mut p = Playlist::new();
        assert_eq!(p.next(&NoRefill), Transition::default());
        assert_eq!(current_id(&p), None);
    }

    #[test]
    fn prev_returns_the_outgoing_track_to_the_head_and_displaces_nothing() {
        let mut p = with(&[Some(2)]);
        p.play_index(0, &NoRefill);
        let t = p.prev(track(1), &NoRefill);
        assert_eq!(current_id(&p), Some(1));
        assert_eq!(queued(&p), vec![Some(2)]);
        // History is the renderer's; `prev` steps back through it rather than
        // appending to it.
        assert_eq!(t.displaced, None);
    }

    #[test]
    fn prev_with_nothing_on_air_just_airs_the_previous_track() {
        let mut p = with(&[Some(2)]);
        let t = p.prev(track(1), &NoRefill);
        assert_eq!(current_id(&p), Some(1));
        assert_eq!(queued(&p), vec![Some(2)]);
        assert_eq!(t.displaced, None);
    }

    #[test]
    fn prev_cancels_a_pending_outage_retry() {
        let mut p = with(&[Some(2)]);
        p.on_load_failed(&NoRefill);
        assert!(p.snapshot(None).awaiting_network);
        let t = p.prev(track(1), &NoRefill);
        assert!(t.effects.contains(&Effect::CancelRetry));
        assert!(!p.snapshot(None).awaiting_network);
    }

    #[test]
    fn stop_with_nothing_on_air_still_stops_the_deck() {
        let mut p = Playlist::new();
        let t = p.stop();
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(t.displaced, None);
    }

    #[test]
    fn stop_clears_the_deck_the_auto_playlist_and_displaces_the_track() {
        let mut p = with(&[Some(2)]);
        p.set_auto_playlist(true, &FakeRefiller::new());
        let t = p.stop();
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(t.displaced.map(|d| d.id), Some(2));
        let snap = p.snapshot(None);
        assert_eq!(snap.current, None);
        assert!(!snap.auto_playlist_active);
    }

    #[test]
    fn set_auto_advance_flips_the_flag() {
        let mut p = Playlist::new();
        assert!(p.snapshot(None).auto_advance);
        p.set_auto_advance(false);
        assert!(!p.snapshot(None).auto_advance);
    }

    #[test]
    fn enabling_the_auto_playlist_refills_and_starts_the_show_when_idle() {
        let mut p = Playlist::new();
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert_eq!(current_id(&p), Some(1001));
        assert_eq!(queued(&p).len(), 19);
    }

    #[test]
    fn enabling_the_auto_playlist_leaves_a_playing_deck_alone() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_index(0, &NoRefill);
        let r = FakeRefiller::new();
        p.set_auto_playlist(true, &r);
        assert_eq!(current_id(&p), Some(1));
    }

    /// Faithful to the renderer, and worth pinning because it reads oddly: the
    /// marker is the head, so starting the show immediately runs into it, and
    /// stopping is what clears the auto-playlist flag again.
    #[test]
    fn enabling_the_auto_playlist_onto_a_stop_marker_stops_instead_of_starting() {
        let mut p = with(&[None, Some(1)]);
        let t = p.set_auto_playlist(true, &NoRefill);
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(current_id(&p), None);
        assert_eq!(queued(&p), vec![Some(1)]);
        assert!(!p.snapshot(None).auto_playlist_active);
    }

    #[test]
    fn disabling_the_auto_playlist_does_not_touch_playback() {
        let mut p = with(&[Some(1)]);
        p.play_index(0, &NoRefill);
        assert_eq!(p.set_auto_playlist(false, &NoRefill), Transition::default());
        assert_eq!(current_id(&p), Some(1));
    }

    // ----- auto-playlist refill -----

    #[test]
    fn refill_does_nothing_while_the_auto_playlist_is_off() {
        let mut p = with(&[Some(1)]);
        p.play_index(0, &NoRefill);
        assert_eq!(queued(&p), Vec::<Option<i64>>::new());
    }

    #[test]
    fn refill_tops_an_empty_queue_up_to_the_buffer() {
        let mut p = Playlist::new();
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert_eq!(r.calls(), vec![(20, vec![])]);
    }

    #[test]
    fn refill_requests_only_the_deficit_and_excludes_what_is_queued() {
        let mut p = with(&[Some(1), Some(2)]);
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert_eq!(r.calls(), vec![(18, vec![1, 2])]);
    }

    #[test]
    fn refill_does_nothing_while_the_queue_is_above_the_threshold() {
        let mut p = with(&[Some(1), Some(2), Some(3), Some(4), Some(5)]);
        p.play_now(track(9), &NoRefill);
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert!(r.calls().is_empty());
    }

    #[test]
    fn refill_is_skipped_while_a_stop_marker_is_queued() {
        let mut p = with(&[Some(1), None]);
        p.play_now(track(9), &NoRefill);
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert!(r.calls().is_empty());
    }

    /// Putting a track on air tops the playlist back up, which is what keeps a
    /// running auto-playlist ahead of the deck without any separate timer.
    #[test]
    fn airing_a_track_refills_behind_it() {
        let mut p = with(&[Some(1), Some(2)]);
        let r = FakeRefiller::generating(20, 5);
        p.set_auto_playlist(true, &r);
        assert_eq!(queued(&p).len(), 19);
        p.next(&r);
        assert_eq!(queued(&p).len(), 18);
        assert_eq!(r.calls().len(), 1);
    }

    // ----- end of track -----

    #[test]
    fn ended_advances_to_the_next_track() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_index(0, &NoRefill);
        p.on_ended(&FakeRefiller::new());
        assert_eq!(current_id(&p), Some(2));
    }

    #[test]
    fn ended_stops_when_auto_advance_is_off() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_index(0, &NoRefill);
        p.set_auto_advance(false);
        let t = p.on_ended(&NoRefill);
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(t.displaced.map(|d| d.id), Some(1));
        assert_eq!(queued(&p), vec![Some(2)]);
    }

    #[test]
    fn ended_onto_a_stop_marker_halts_and_consumes_the_marker() {
        let mut p = with(&[None, Some(2)]);
        p.play_now(track(1), &NoRefill);
        let t = p.on_ended(&FakeRefiller::new());
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(current_id(&p), None);
        assert_eq!(queued(&p), vec![Some(2)]);
    }

    // ----- outage recovery (skip-to-cached) -----

    #[test]
    fn ended_skips_uncached_tracks_and_leaves_them_queued() {
        let mut p = with(&[Some(1), Some(2), Some(3)]);
        p.play_now(track(9), &NoRefill);
        p.on_cache_state(vec![3], &NoRefill);
        p.on_ended(&FakeRefiller::new());
        assert_eq!(current_id(&p), Some(3));
        assert_eq!(queued(&p), vec![Some(1), Some(2)]);
    }

    #[test]
    fn nothing_cached_waits_on_a_backoff_then_resumes_when_the_cache_returns() {
        let mut p = with(&[Some(1)]);
        p.play_now(track(9), &NoRefill);
        // A cache-state that holds only the track on air: nothing upcoming is
        // resident, so the share is down as far as the playlist is concerned.
        p.on_cache_state(vec![9], &NoRefill);
        let t = p.on_ended(&FakeRefiller::new());
        assert_eq!(t.effects, vec![Effect::ArmRetry(0)]);
        assert!(p.snapshot(None).awaiting_network);

        let t = p.on_cache_state(vec![9, 1], &NoRefill);
        assert!(t.effects.contains(&play(1)));
        assert!(!p.snapshot(None).awaiting_network);
    }

    /// The barrier holds during an outage too. Skipping to a cached track that
    /// sits *past* a stop marker would put audio on air the operator explicitly
    /// blocked (#278).
    #[test]
    fn skip_to_cached_never_crosses_a_stop_marker() {
        let mut p = with(&[Some(1), None, Some(3)]);
        p.play_now(track(9), &NoRefill);
        // Track 1 is uncached, track 3 is cached — but the marker comes first.
        p.on_cache_state(vec![3, 9], &NoRefill);
        let t = p.on_ended(&FakeRefiller::new());
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert_eq!(current_id(&p), None);
        assert_eq!(queued(&p), vec![Some(1), Some(3)]);
    }

    /// A stop marker is checked before cache membership, so it ends an outage
    /// wait instead of holding air until the share returns. Carried over from
    /// the renderer unchanged; the alternative — airing everything ahead of the
    /// marker first, whenever the share allows — would be a behaviour change.
    #[test]
    fn a_stop_marker_ends_the_outage_wait_rather_than_holding_air() {
        let mut p = with(&[Some(1), None]);
        p.play_now(track(9), &NoRefill);
        // Nothing upcoming is cached; without the marker this would wait.
        p.on_cache_state(vec![9], &NoRefill);
        let t = p.on_ended(&FakeRefiller::new());
        assert_eq!(t.effects, vec![Effect::CancelRetry, Effect::Stop]);
        assert!(!p.snapshot(None).awaiting_network);
        assert_eq!(queued(&p), vec![Some(1)]);
    }

    #[test]
    fn load_failed_skips_to_the_next_cached_track() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_now(track(9), &NoRefill);
        p.on_cache_state(vec![2], &NoRefill);
        p.on_load_failed(&FakeRefiller::new());
        assert_eq!(current_id(&p), Some(2));
    }

    #[test]
    fn load_failed_with_no_cache_knowledge_waits_instead_of_burning_the_queue() {
        let mut p = with(&[Some(1), Some(2)]);
        let t = p.on_load_failed(&NoRefill);
        assert_eq!(t.effects, vec![Effect::ArmRetry(0)]);
        assert_eq!(queued(&p), vec![Some(1), Some(2)]);
        assert_eq!(current_id(&p), None);
    }

    #[test]
    fn load_failed_does_nothing_while_auto_advance_is_off() {
        let mut p = with(&[Some(1)]);
        p.set_auto_advance(false);
        assert_eq!(p.on_load_failed(&NoRefill), Transition::default());
    }

    #[test]
    fn a_retry_already_armed_is_not_re_armed_on_the_next_failure() {
        let mut p = with(&[Some(1)]);
        assert_eq!(
            p.on_load_failed(&NoRefill).effects,
            vec![Effect::ArmRetry(0)]
        );
        assert_eq!(p.on_load_failed(&NoRefill), Transition::default());
        // Still waiting — the second failure adds no timer, but it must not
        // drop the banner either.
        assert!(p.snapshot(None).awaiting_network);
    }

    #[test]
    fn the_last_track_ending_with_nothing_queued_leaves_the_deck_idle() {
        let mut p = Playlist::new();
        p.play_now(track(1), &NoRefill);
        let t = p.on_ended(&FakeRefiller::new());
        assert_eq!(t.effects, vec![Effect::CancelRetry]);
        // Not a stop: the track played itself out, so there is nothing to
        // displace into history and nothing to tell the deck.
        assert_eq!(t.displaced, None);
        assert_eq!(current_id(&p), Some(1));
        assert!(!p.snapshot(None).awaiting_network);
    }

    #[test]
    fn each_retry_tick_that_fails_again_moves_further_down_the_backoff_schedule() {
        let mut p = with(&[Some(1)]);
        p.on_load_failed(&NoRefill);
        assert_eq!(
            p.on_retry_tick(&NoRefill).effects,
            vec![Effect::ArmRetry(1)]
        );
        assert_eq!(
            p.on_retry_tick(&NoRefill).effects,
            vec![Effect::ArmRetry(2)]
        );
    }

    /// The tick re-plans rather than just re-reading: a track queued during the
    /// outage that happens to be resident already is playable, and nothing else
    /// would notice — queueing does not advance, and no cache-state is coming.
    #[test]
    fn a_retry_tick_airs_a_track_queued_during_the_outage() {
        let mut p = with(&[Some(1)]);
        p.play_now(track(9), &NoRefill);
        p.on_cache_state(vec![9], &NoRefill);
        assert_eq!(
            p.on_ended(&FakeRefiller::new()).effects,
            vec![Effect::ArmRetry(0)]
        );

        // Operator requeues the track that is still in RAM.
        p.add(track(9));
        let t = p.on_retry_tick(&NoRefill);
        assert!(t.effects.contains(&play(9)));
        assert!(!p.snapshot(None).awaiting_network);
        assert_eq!(queued(&p), vec![Some(1)]);
    }

    #[test]
    fn an_explicit_track_change_cancels_the_pending_retry() {
        let mut p = with(&[Some(1)]);
        p.on_load_failed(&NoRefill);
        assert!(p.snapshot(None).awaiting_network);

        let t = p.play_index(0, &NoRefill);
        assert!(t.effects.contains(&Effect::CancelRetry));
        assert!(!p.snapshot(None).awaiting_network);
        // Back to the top of the schedule, so a later outage waits a second
        // rather than resuming mid-backoff.
        p.add(track(2));
        assert_eq!(
            p.on_load_failed(&NoRefill).effects,
            vec![Effect::ArmRetry(0)]
        );
    }

    #[test]
    fn an_empty_queue_cancels_the_retry_rather_than_waiting_forever() {
        let mut p = Playlist::new();
        let t = p.on_load_failed(&NoRefill);
        assert_eq!(t.effects, vec![Effect::CancelRetry]);
        assert!(!p.snapshot(None).awaiting_network);
    }

    #[test]
    fn cache_state_does_not_advance_while_playback_is_healthy() {
        let mut p = with(&[Some(1)]);
        p.play_now(track(9), &NoRefill);
        assert_eq!(
            p.on_cache_state(vec![1, 9], &NoRefill),
            Transition::default()
        );
        assert_eq!(current_id(&p), Some(9));
    }

    // ----- prefetch window -----

    #[test]
    fn the_prefetch_window_is_the_track_on_air_then_the_queue_in_order() {
        let mut p = with(&[Some(1), None, Some(2)]);
        p.play_now(track(9), &NoRefill);
        assert_eq!(p.prefetch_window(), vec![9, 1, 2]);
    }

    /// No count cap: the whole playlist is offered and the cache's byte cap
    /// decides how much of it stays resident.
    #[test]
    fn the_prefetch_window_covers_the_whole_queue() {
        let mut p = Playlist::new();
        for id in 1..=20 {
            p.add(track(id));
        }
        assert_eq!(p.prefetch_window(), (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn the_prefetch_window_is_just_the_queue_when_nothing_is_on_air() {
        let p = with(&[Some(1), Some(2)]);
        assert_eq!(p.prefetch_window(), vec![1, 2]);
    }

    // ----- session -----

    #[test]
    fn hydrate_restores_the_queue_and_resumes_paused_at_the_saved_position() {
        let mut p = Playlist::new();
        let t = p.hydrate(
            vec![PlaylistItem::track(track(1)), PlaylistItem::Stop],
            Some(track(9)),
            None,
            12.5,
            true,
            false,
        );
        assert_eq!(
            t.effects,
            vec![Effect::Resume {
                id: 9,
                seconds: 12.5,
                cue_override: None,
            }]
        );
        // A restore must not put audio on air by itself.
        assert!(!t.effects.contains(&play(9)));
        assert_eq!(t.displaced, None);
        let snap = p.snapshot(None);
        assert_eq!(queued(&p), vec![Some(1), None]);
        assert_eq!(snap.current.map(|t| t.id), Some(9));
        assert!(snap.auto_playlist_active);
        assert!(!snap.auto_advance);
    }

    #[test]
    fn hydrate_replaces_whatever_was_already_queued() {
        let mut p = with(&[Some(1), Some(2)]);
        p.play_now(track(9), &NoRefill);
        p.hydrate(
            vec![PlaylistItem::track(track(3))],
            None,
            None,
            0.0,
            false,
            true,
        );
        assert_eq!(queued(&p), vec![Some(3)]);
        assert_eq!(current_id(&p), None);
    }

    #[test]
    fn hydrate_without_a_saved_track_issues_nothing() {
        let mut p = Playlist::new();
        let t = p.hydrate(
            vec![PlaylistItem::track(track(1))],
            None,
            None,
            0.0,
            false,
            true,
        );
        assert_eq!(t, Transition::default());
    }

    #[test]
    fn hydrate_clamps_a_negative_saved_position() {
        let mut p = Playlist::new();
        let t = p.hydrate(vec![], Some(track(9)), None, -4.0, false, true);
        assert_eq!(
            t.effects,
            vec![Effect::Resume {
                id: 9,
                seconds: 0.0,
                cue_override: None,
            }]
        );
    }

    // ----- item overrides -----

    #[test]
    fn add_front_carries_an_override_onto_the_item() {
        let mut p = with(&[Some(1)]);
        p.add_front(track(9), Some(points(2_000)));
        assert_eq!(item_override(&p, 0), Some(points(2_000)));
    }

    #[test]
    fn playing_an_item_hands_its_override_to_the_deck() {
        let mut p = Playlist::new();
        p.add_front(track(9), Some(points(2_000)));
        let t = p.play_index(0, &NoRefill);
        assert!(t.effects.contains(&Effect::Play {
            id: 9,
            cue_override: Some(points(2_000)),
        }));
        assert_eq!(p.snapshot(None).current_override, Some(points(2_000)));
    }

    /// An item that carries no override plays the radio edit, which the service
    /// reads out of the library — the engine sends `None` rather than a copy.
    #[test]
    fn an_ordinary_item_sends_no_override() {
        let mut p = with(&[Some(1)]);
        assert!(p.play_index(0, &NoRefill).effects.contains(&play(1)));
        assert_eq!(p.snapshot(None).current_override, None);
    }

    /// The defect the renderer-owned playlist had: `currentTrack` was a Track,
    /// not an item, so stepping back returned an unedited track to the queue.
    #[test]
    fn prev_returns_the_outgoing_item_with_its_override_intact() {
        let mut p = Playlist::new();
        p.add_front(track(9), Some(points(2_000)));
        p.play_index(0, &NoRefill);
        p.prev(track(4), &NoRefill);
        assert_eq!(queued(&p), vec![Some(9)]);
        assert_eq!(item_override(&p, 0), Some(points(2_000)));
        // The track stepped back to comes off history, which stores tracks.
        assert_eq!(p.snapshot(None).current_override, None);
    }

    #[test]
    fn stopping_clears_the_override_on_air() {
        let mut p = Playlist::new();
        p.add_front(track(9), Some(points(2_000)));
        p.play_index(0, &NoRefill);
        p.stop();
        assert_eq!(p.snapshot(None).current_override, None);
    }

    #[test]
    fn set_item_cue_points_sets_and_clears_one_item() {
        let mut p = with(&[Some(1), Some(2)]);
        p.set_item_cue_points(1, Some(points(3_000)));
        assert_eq!(item_override(&p, 1), Some(points(3_000)));
        assert_eq!(item_override(&p, 0), None);
        p.set_item_cue_points(1, None);
        assert_eq!(item_override(&p, 1), None);
    }

    /// "Play the whole file this once" is an all-`NULL` override, which is a
    /// different thing from carrying no override at all.
    #[test]
    fn an_all_null_override_is_kept_as_an_override() {
        let mut p = with(&[Some(1)]);
        p.set_item_cue_points(0, Some(CuePoints::default()));
        assert_eq!(item_override(&p, 0), Some(CuePoints::default()));
    }

    #[test]
    fn set_item_cue_points_ignores_a_stop_marker_and_an_index_past_the_end() {
        let mut p = with(&[None, Some(1)]);
        p.set_item_cue_points(0, Some(points(3_000)));
        p.set_item_cue_points(7, Some(points(3_000)));
        assert!(p.snapshot(None).playlist[0].is_stop());
        assert_eq!(queued(&p), vec![None, Some(1)]);
    }

    #[test]
    fn saving_a_radio_edit_refreshes_the_queued_copies_of_the_track() {
        let mut p = with(&[Some(1), Some(2)]);
        p.on_cue_points_saved(1, points(4_000));
        let queued_points = |p: &Playlist, i: usize| {
            p.snapshot(None).playlist[i]
                .as_track()
                .map(|t| t.cue_points)
        };
        assert_eq!(queued_points(&p, 0), Some(points(4_000)));
        assert_eq!(queued_points(&p, 1), Some(CuePoints::default()));
    }

    /// The item still airs under its override; only the track copy it displays
    /// alongside is refreshed.
    #[test]
    fn saving_a_radio_edit_leaves_an_items_override_alone() {
        let mut p = Playlist::new();
        p.add_front(track(1), Some(points(9_000)));
        p.on_cue_points_saved(1, points(4_000));
        assert_eq!(item_override(&p, 0), Some(points(9_000)));
    }

    /// A radio edit saved mid-broadcast applies from the next airing, so the
    /// numbers under the track on air must not move.
    #[test]
    fn saving_a_radio_edit_does_not_touch_the_track_on_air() {
        let mut p = with(&[Some(1)]);
        p.play_index(0, &NoRefill);
        p.on_cue_points_saved(1, points(4_000));
        assert_eq!(
            p.snapshot(None).current.map(|t| t.cue_points),
            Some(CuePoints::default())
        );
    }

    #[test]
    fn hydrate_resumes_under_the_saved_override() {
        let mut p = Playlist::new();
        let t = p.hydrate(
            vec![],
            Some(track(9)),
            Some(points(2_000)),
            12.5,
            false,
            true,
        );
        assert_eq!(
            t.effects,
            vec![Effect::Resume {
                id: 9,
                seconds: 12.5,
                cue_override: Some(points(2_000)),
            }]
        );
        assert_eq!(p.snapshot(None).current_override, Some(points(2_000)));
    }
}
