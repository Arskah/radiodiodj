//! Wiring for the playlist engine.
//!
//! Owns the [`Playlist`] and turns its [`Effect`]s into real work: deck loads,
//! play-count increments, prefetch-window updates, outage retry timers. Every
//! transition ends with a [`Snapshot`] on `program:playlist-state`, which is the
//! renderer's only source of playlist truth.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Listener};

use super::engine::{Effect, Playlist, Refiller, Transition};
use super::generate;
use super::model::{PlaylistItem, Snapshot};
use crate::audio::cache::Cache;
use crate::audio::player::{Cmd, PlayerHandle};
use crate::broadcast::BroadcastService;
use crate::library::db::{Db, Track};
use crate::persist::config::Config;
use crate::persist::session::SessionState;

/// Topic the whole-playlist snapshot is emitted on.
pub const PLAYLIST_STATE_EVENT: &str = "program:playlist-state";

/// Fallback retry delay, used only if the configured schedule is empty. The
/// stored one is clamped non-empty on write, so this is a belt-and-braces value
/// rather than a tunable.
const DEFAULT_BACKOFF_MS: u64 = 1000;

/// Refill material drawn from the library, sized by the stored tuning. Both the
/// cadence and the sizes are read per call, so a settings change takes effect on
/// the next refill without a restart.
struct DbRefiller<'a> {
    db: &'a Db,
    interleave: generate::Interleave,
    buffer: i64,
    threshold: i64,
}

impl Refiller for DbRefiller<'_> {
    fn generate(&self, count: i64, exclude: &[i64]) -> Vec<Track> {
        generate::generate(self.db, count, exclude, &self.interleave).unwrap_or_else(|e| {
            log::error!("auto-playlist refill failed: {}", e);
            vec![]
        })
    }

    fn buffer(&self) -> i64 {
        self.buffer
    }

    fn threshold(&self) -> i64 {
        self.threshold
    }
}

struct Inner {
    playlist: Mutex<Playlist>,
    db: Arc<Db>,
    config: Arc<Config>,
    deck: Arc<PlayerHandle>,
    cache: Arc<Cache>,
    broadcast: Arc<BroadcastService>,
    app: AppHandle,
    /// Bumped whenever a retry is armed or cancelled. A sleeping timer whose
    /// generation no longer matches has been superseded and does nothing —
    /// which is how an explicit track change cancels a pending outage retry.
    retry_generation: AtomicU64,
}

pub struct PlaylistService {
    inner: Arc<Inner>,
}

impl PlaylistService {
    pub fn new(
        app: AppHandle,
        db: Arc<Db>,
        config: Arc<Config>,
        deck: Arc<PlayerHandle>,
        cache: Arc<Cache>,
        broadcast: Arc<BroadcastService>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                playlist: Mutex::new(Playlist::new()),
                db,
                config,
                deck,
                cache,
                broadcast,
                app,
                retry_generation: AtomicU64::new(0),
            }),
        }
    }

    /// Subscribe to the main deck's lifecycle. Advancement is driven from here
    /// rather than from the renderer: the deck that notices the track ended is
    /// in the same process as the playlist that knows what follows it.
    pub fn attach_to_app(&self, app: &AppHandle) {
        let ended = Arc::clone(&self.inner);
        app.listen("main-deck:ended", move |_| {
            Inner::apply(&ended, |p, r| p.on_ended(r));
        });

        let failed = Arc::clone(&self.inner);
        app.listen("main-deck:load-failed", move |_| {
            Inner::apply(&failed, |p, r| p.on_load_failed(r));
        });

        let cache_state = Arc::clone(&self.inner);
        app.listen("main-deck:cache-state", move |event| {
            let ids: Vec<i64> = serde_json::from_str(event.payload()).unwrap_or_default();
            Inner::apply(&cache_state, |p, r| p.on_cache_state(ids, r));
        });
    }

    /// Current state, for a renderer that has just (re)connected. Carries no
    /// `displaced` — nothing left the deck to produce it.
    pub fn snapshot(&self) -> Snapshot {
        self.inner.playlist.lock().snapshot(None)
    }

    /// Restore the persisted playlist and put the saved track back on the deck,
    /// paused at its saved position.
    pub fn hydrate(&self, state: &SessionState) {
        let items = self.resolve_items(state);
        let current = state
            .current_track_id
            .and_then(|id| self.inner.lookup(id).ok().flatten());
        let seconds = state.current_time;
        let auto_playlist = state.auto_playlist_active;
        let auto_advance = state.auto_advance;
        Inner::apply(&self.inner, move |p, _| {
            p.hydrate(items, current, seconds, auto_playlist, auto_advance)
        });
    }

    pub fn add(&self, id: i64) -> Result<(), String> {
        self.with_track(id, |p, _, track| p.add(track))
    }

    pub fn add_front(&self, id: i64) -> Result<(), String> {
        self.with_track(id, |p, _, track| p.add_front(track))
    }

    pub fn add_stop(&self) {
        Inner::apply(&self.inner, |p, _| p.add_stop());
    }

    /// Append one jingle or commercial picked the same way the auto-playlist
    /// would have. A no-op when the typed library is empty.
    pub fn add_filler(&self, content_type: generate::ContentType) -> Result<(), String> {
        let interleave = generate::Interleave::from_config(&self.inner.config.get_tuning());
        let picked = generate::pick_filler(&self.inner.db, content_type, &interleave)
            .map_err(|e| e.to_string())?;
        if let Some(track) = picked {
            Inner::apply(&self.inner, move |p, _| p.add(track));
        }
        Ok(())
    }

    pub fn remove(&self, index: usize) {
        Inner::apply(&self.inner, move |p, _| p.remove(index));
    }

    pub fn move_item(&self, from: usize, to: usize) {
        Inner::apply(&self.inner, move |p, _| p.move_item(from, to));
    }

    pub fn clear(&self) {
        Inner::apply(&self.inner, |p, _| p.clear());
    }

    pub fn play_index(&self, index: usize) {
        Inner::apply(&self.inner, move |p, r| p.play_index(index, r));
    }

    pub fn play_now(&self, id: i64) -> Result<(), String> {
        self.with_track(id, |p, r, track| p.play_now(track, r))
    }

    pub fn next(&self) {
        Inner::apply(&self.inner, |p, r| p.next(r));
    }

    /// Step back to `id`, which the renderer read off its own history.
    pub fn prev(&self, id: i64) -> Result<(), String> {
        self.with_track(id, |p, r, track| p.prev(track, r))
    }

    pub fn stop(&self) {
        Inner::apply(&self.inner, |p, _| p.stop());
    }

    pub fn set_auto_advance(&self, active: bool) {
        Inner::apply(&self.inner, move |p, _| p.set_auto_advance(active));
    }

    pub fn set_auto_playlist(&self, active: bool) {
        Inner::apply(&self.inner, move |p, r| p.set_auto_playlist(active, r));
    }

    fn with_track<F>(&self, id: i64, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut Playlist, &dyn Refiller, Track) -> Transition,
    {
        let track = self
            .inner
            .lookup(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("track {} not found", id))?;
        Inner::apply(&self.inner, move |p, r| f(p, r, track));
        Ok(())
    }

    fn resolve_items(&self, state: &SessionState) -> Vec<PlaylistItem> {
        PlaylistItem::from_session(&state.playlist_items, &state.playlist_ids, |id| {
            self.inner.lookup(id).ok().flatten()
        })
    }
}

/// Delay for retry number `attempt`, saturating on the schedule's last entry so
/// a long outage keeps retrying at a steady pace instead of running off the end.
fn backoff_ms(schedule: &[u64], attempt: usize) -> u64 {
    schedule
        .get(attempt.min(schedule.len().saturating_sub(1)))
        .copied()
        .unwrap_or(DEFAULT_BACKOFF_MS)
}

impl Inner {
    fn lookup(&self, id: i64) -> anyhow::Result<Option<Track>> {
        self.db.get_track(id)
    }

    fn refiller(&self) -> DbRefiller<'_> {
        let tuning = self.config.get_tuning();
        DbRefiller {
            db: &self.db,
            interleave: generate::Interleave::from_config(&tuning),
            buffer: tuning.auto_playlist.auto_playlist_buffer as i64,
            threshold: tuning.auto_playlist.auto_playlist_threshold as i64,
        }
    }

    /// Run one transition and settle its consequences.
    ///
    /// Two ordering rules, both about re-entrancy. Tauri dispatches to backend
    /// listeners inline, and `set_window` emits `main-deck:cache-state`, which
    /// this service listens to — so a transition can re-enter `apply` before it
    /// returns.
    ///
    /// The playlist lock is therefore released before any effect runs, or the
    /// nested call would deadlock on it. And the snapshot is emitted before the
    /// effects, so the nested transition's snapshot lands *after* this one:
    /// emitting last would have the outer call overwrite the renderer with the
    /// state as it was before the nested advance.
    fn apply<F>(inner: &Arc<Inner>, f: F)
    where
        F: FnOnce(&mut Playlist, &dyn Refiller) -> Transition,
    {
        let (transition, snapshot, window) = {
            let refiller = inner.refiller();
            let mut playlist = inner.playlist.lock();
            let transition = f(&mut playlist, &refiller);
            let snapshot = playlist.snapshot(transition.displaced.clone());
            let window = playlist.prefetch_window();
            (transition, snapshot, window)
        };

        let _ = inner.app.emit(PLAYLIST_STATE_EVENT, &snapshot);
        for effect in &transition.effects {
            Inner::run(inner, effect);
        }
        // Re-pushed on every transition, not only when the window changed: the
        // cache worker reads on window updates, so this is also what retries a
        // failed prefetch once the share is back.
        inner.set_window(window);
    }

    fn run(inner: &Arc<Inner>, effect: &Effect) {
        match effect {
            Effect::Play(id) => {
                if inner.load_deck(*id) {
                    inner.deck.send(Cmd::Play);
                }
            }
            Effect::Resume { id, seconds } => {
                if inner.load_deck(*id) && *seconds > 0.0 {
                    inner.deck.send(Cmd::Seek(*seconds));
                }
            }
            Effect::Stop => inner.deck.send(Cmd::Stop),
            Effect::TrackPlayed(id) => {
                if let Err(e) = inner.db.increment_play_count(*id) {
                    log::error!("play count update failed for track {}: {}", id, e);
                }
            }
            Effect::ArmRetry(attempt) => Inner::arm_retry(inner, *attempt),
            Effect::CancelRetry => {
                inner.retry_generation.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// Resolve the track's path and hand it to the deck. Reports whether the
    /// load was actually issued, so a missing row does not leave the caller
    /// sending Play at nothing.
    fn load_deck(&self, id: i64) -> bool {
        match self.db.get_track_broadcast_info(id) {
            Ok(Some(track)) => {
                let path = PathBuf::from(track.path.clone());
                let duration = track.duration;
                self.broadcast.set_pending_track(track.into());
                self.deck.send(Cmd::Load {
                    id,
                    path,
                    duration: (duration > 0.0).then_some(duration),
                });
                true
            }
            Ok(None) => {
                log::error!("playlist: track {} is no longer in the library", id);
                false
            }
            Err(e) => {
                log::error!("playlist: track {} lookup failed: {}", id, e);
                false
            }
        }
    }

    fn set_window(&self, ids: Vec<i64>) {
        match self.db.get_paths_by_ids(&ids) {
            Ok(paths) => {
                let window = paths
                    .into_iter()
                    .map(|(id, path)| (id, PathBuf::from(path)))
                    .collect();
                self.cache.set_window(window);
            }
            Err(e) => log::error!("prefetch window lookup failed: {}", e),
        }
    }

    /// Sleep out the backoff on a throwaway thread, then re-plan. The schedule's
    /// last entry repeats, so a long outage keeps retrying at a steady pace.
    fn arm_retry(inner: &Arc<Inner>, attempt: usize) {
        let delay = backoff_ms(
            &inner
                .config
                .get_tuning()
                .auto_playlist
                .net_retry_backoffs_ms,
            attempt,
        );
        let generation = inner.retry_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let inner = Arc::clone(inner);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay));
            if inner.retry_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            Inner::apply(&inner, |p, r| p.on_retry_tick(r));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_walks_the_schedule() {
        let schedule = [1000, 2000, 5000];
        assert_eq!(backoff_ms(&schedule, 0), 1000);
        assert_eq!(backoff_ms(&schedule, 1), 2000);
        assert_eq!(backoff_ms(&schedule, 2), 5000);
    }

    #[test]
    fn backoff_saturates_on_the_last_entry() {
        let schedule = [1000, 2000, 5000];
        assert_eq!(backoff_ms(&schedule, 3), 5000);
        assert_eq!(backoff_ms(&schedule, 99), 5000);
    }

    #[test]
    fn backoff_falls_back_when_the_schedule_is_empty() {
        assert_eq!(backoff_ms(&[], 0), DEFAULT_BACKOFF_MS);
        assert_eq!(backoff_ms(&[], 7), DEFAULT_BACKOFF_MS);
    }
}
