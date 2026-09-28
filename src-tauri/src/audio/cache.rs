//! Shared prefetch byte cache for the audio decks.
//!
//! Holds whole track files resident in RAM keyed by track id, so that when a
//! deck loads an upcoming track the bytes are already available and no
//! (possibly networked) filesystem read is needed on the hot path.
//!
//! Residency is governed by a *whole-playlist* policy — NOT an LRU:
//! - The renderer pushes the whole playlist of upcoming track ids (current
//!   track first, then playlist order) via `set_window`. Entries whose id falls
//!   outside the latest window are evicted immediately.
//! - Total resident bytes are capped at [`MAX_CACHE_BYTES`]. When the window's
//!   entries would exceed the cap, the tracks first on the list win: the window
//!   is walked front-first and entries are retained until the cap is reached.
//!   The whole playlist is therefore attempted, but only as many leading tracks
//!   as fit in the cap stay resident.
//!
//! A single background worker fetches missing window entries sequentially,
//! front-first — never in parallel, to avoid hammering the (network) share.
//!
//! The rule belongs to the share rather than to that worker, so the cache also
//! holds the **in-flight set**: every reader of a library file — the prefetch
//! worker and a deck that missed — claims an id before it reads, and one file
//! therefore never crosses the share twice at once. A deck that misses an id
//! someone else is already reading waits for that read instead of starting its
//! own, which is never slower than a read that begins later.

use anyhow::Result;
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use super::player::{read_file, read_with_retry, Bytes};

/// Default hard cap on total resident bytes, used when no configured value is
/// supplied (tests, and the config default). The live cap is held per-`Cache`.
/// The whole playlist is attempted, but only as many leading tracks as fit
/// under the cap stay resident.
pub const MAX_CACHE_BYTES: usize = 150 * 1024 * 1024;

/// Event topic on which cache membership changes are broadcast. Prefetch is a
/// main-deck concept, so the cache-state event carries the main-deck prefix.
const CACHE_STATE_EVENT: &str = "main-deck:cache-state";

/// Event topic raised when a prefetch read fails — the share is (probably)
/// unreachable. Drives the reconnecting indicator; a later cache-state emit
/// (a read succeeded) signals recovery and clears it on the frontend.
const PREFETCH_FAILED_EVENT: &str = "main-deck:prefetch-failed";

/// How long a deck waiting on someone else's read sleeps before looking again.
/// The wait is driven by [`Shared::read_done`]; this bound only keeps a missed
/// notification from wedging the loop. What actually bounds the wait is the
/// deck's read watchdog, which fails the load exactly as it would a read of its
/// own.
const READ_WAIT_POLL: Duration = Duration::from_millis(250);

/// Internal, lock-guarded cache state. All bookkeeping (window membership and
/// byte-cap eviction) is implemented here as pure-ish methods so it can be
/// unit-tested without spawning real reads or touching an audio device.
struct Inner {
    /// Cached file bytes by track id.
    entries: HashMap<i64, Bytes>,
    /// Desired residency window — the whole playlist, ordered front-first
    /// (current track first), paired with the path to read on a miss.
    window: Vec<(i64, PathBuf)>,
    /// Bumped on every `set_window`; lets the prefetch worker abort a run whose
    /// window has been superseded.
    generation: u64,
    /// Hard cap on total resident bytes for this cache (from config).
    cap: usize,
    /// Ids a read is in flight for, whoever is doing it. Claimed before a read
    /// and released when it ends, so the share is never asked for one file
    /// twice at once.
    in_flight: HashSet<i64>,
}

impl Inner {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            window: Vec::new(),
            generation: 0,
            cap,
            in_flight: HashSet::new(),
        }
    }

    /// Take bytes some other reader pulled off the share.
    ///
    /// Refused for an id outside the current window, and that test is not
    /// optional: [`Inner::enforce_cap`] keeps what [`retain_within_cap`] walks,
    /// which is the *window*, so an out-of-window entry would be uncounted
    /// against the cap while resident and dropped by the next enforcement
    /// anyway. Residency is a playlist window, not an LRU.
    ///
    /// The window generation is deliberately not consulted. `run_prefetch`
    /// checks it because it is deciding whether a whole run has been
    /// superseded; this is a point operation, and membership under the lock is
    /// the authority on it.
    ///
    /// Returns whether membership changed.
    fn accept(&mut self, id: i64, bytes: Bytes) -> bool {
        if !self.window.iter().any(|(w, _)| *w == id) {
            return false;
        }
        let fresh = self.entries.insert(id, bytes).is_none();
        // The entry just added may itself be the one beyond the cap.
        let capped = self.enforce_cap();
        fresh || capped
    }

    /// Mark `id` as being read, unless someone is already reading it.
    fn claim(&mut self, id: i64) -> bool {
        self.in_flight.insert(id)
    }

    /// Evict cached entries whose id is not in the current window. Returns
    /// `true` if membership changed.
    fn evict_out_of_window(&mut self) -> bool {
        let ids: HashSet<i64> = self.window.iter().map(|(id, _)| *id).collect();
        let before = self.entries.len();
        self.entries.retain(|id, _| ids.contains(id));
        self.entries.len() != before
    }

    /// Enforce the byte cap by dropping entries beyond the cap, front-first.
    /// Returns `true` if membership changed.
    fn enforce_cap(&mut self) -> bool {
        let keep = retain_within_cap(&self.window, &self.entries, self.cap);
        let before = self.entries.len();
        self.entries.retain(|id, _| keep.contains(id));
        self.entries.len() != before
    }
}

/// Walk `window` front-first, keeping ids whose cached bytes fit under `cap`.
/// Stops at the first entry that would exceed the cap (leading entries win).
/// Ids not present in `entries` are ignored (not yet fetched).
fn retain_within_cap(
    window: &[(i64, PathBuf)],
    entries: &HashMap<i64, Bytes>,
    cap: usize,
) -> HashSet<i64> {
    let mut keep = HashSet::new();
    let mut total = 0usize;
    for (id, _) in window {
        if let Some(b) = entries.get(id) {
            let len = b.len();
            if total + len > cap {
                break;
            }
            total += len;
            keep.insert(*id);
        }
    }
    keep
}

/// What the prefetch worker should do with one window entry.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Already resident. Its bytes count against the cap; move on.
    Have(usize),
    /// Another reader has it in flight. Their bytes land in `entries` when they
    /// are done, and the next window push picks up whatever did not.
    Busy,
    /// Nobody holds it and nobody is reading it.
    Read,
}

/// What a deck's read should do for one id.
enum DeckStep {
    /// Resident already — no filesystem at all.
    Take(Bytes),
    /// Ours to read; the claim is released when it is dropped.
    Read(ReadClaim),
    /// Someone else is reading it. Wait for them rather than asking the share
    /// for the same file a second time.
    Wait,
}

/// The state every reader shares: the cache itself, and the signal that a read
/// of some id has ended.
///
/// Separate from [`Cache`] because it needs no [`AppHandle`] — which is what
/// makes the claim and step rules testable without a running app.
struct Shared {
    inner: Mutex<Inner>,
    /// Notified whenever a [`ReadClaim`] is released, so a deck waiting on
    /// another reader's file wakes as soon as it lands.
    read_done: Condvar,
}

impl Shared {
    fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::new(cap)),
            read_done: Condvar::new(),
        }
    }

    /// Claim the right to read `id` off the share, or `None` if another reader
    /// already holds it.
    fn claim(self: &Arc<Self>, id: i64) -> Option<ReadClaim> {
        self.inner.lock().claim(id).then(|| ReadClaim {
            shared: Arc::clone(self),
            id,
        })
    }

    /// What the prefetch worker should do with `id`.
    fn prefetch_step(&self, id: i64) -> Step {
        let guard = self.inner.lock();
        match guard.entries.get(&id) {
            Some(b) => Step::Have(b.len()),
            None if guard.in_flight.contains(&id) => Step::Busy,
            None => Step::Read,
        }
    }

    /// What a deck's read should do for `id`, claiming it in the same breath
    /// when the read falls to us — the decision and the claim have to be one
    /// step, or two decks both decide to read.
    fn deck_step(self: &Arc<Self>, id: i64) -> DeckStep {
        let mut guard = self.inner.lock();
        if let Some(bytes) = guard.entries.get(&id) {
            return DeckStep::Take(Arc::clone(bytes));
        }
        if guard.claim(id) {
            return DeckStep::Read(ReadClaim {
                shared: Arc::clone(self),
                id,
            });
        }
        DeckStep::Wait
    }

    /// Wait for some read to end, or for [`READ_WAIT_POLL`] to pass.
    fn wait_for_read(&self) {
        let mut guard = self.inner.lock();
        self.read_done.wait_for(&mut guard, READ_WAIT_POLL);
    }
}

/// The right to read one file off the share, released on drop — including on a
/// panic, so a reader that dies cannot keep an id out of the cache for good.
///
/// A reader that *hangs* is a different matter: a read wedged on a dead mount
/// holds its claim until the OS finally errors it, and prefetch keeps skipping
/// that id meanwhile. That is the right way round — a file the share is
/// refusing to send is the last one to ask for twice — and it clears itself
/// when the read finally returns.
struct ReadClaim {
    shared: Arc<Shared>,
    id: i64,
}

impl Drop for ReadClaim {
    fn drop(&mut self) {
        self.shared.inner.lock().in_flight.remove(&self.id);
        self.shared.read_done.notify_all();
    }
}

/// Shared byte cache. Clone the `Arc<Cache>` to share it across decks — every
/// clone points at the same resident entries and the same prefetch worker.
pub struct Cache {
    shared: Arc<Shared>,
    /// Wakes the prefetch worker; the authoritative window lives in `shared`.
    prefetch_tx: Sender<()>,
    app: AppHandle,
}

impl Cache {
    pub fn new(app: AppHandle, max_cache_bytes: usize) -> Arc<Self> {
        let shared = Arc::new(Shared::new(max_cache_bytes));
        let (prefetch_tx, prefetch_rx) = channel::<()>();
        {
            let shared = Arc::clone(&shared);
            let app = app.clone();
            thread::spawn(move || prefetch_worker(shared, app, prefetch_rx));
        }
        Arc::new(Self {
            shared,
            prefetch_tx,
            app,
        })
    }

    /// Look up resident bytes for a track id. Never reads the filesystem.
    pub fn get(&self, id: i64) -> Option<Bytes> {
        self.shared.inner.lock().entries.get(&id).cloned()
    }

    /// Offer bytes another reader pulled off the share — a deck's cache miss.
    /// Refused for an id outside the window; see [`Inner::accept`].
    pub fn insert(&self, id: i64, bytes: Bytes) {
        let changed = self.shared.inner.lock().accept(id, bytes);
        if changed {
            self.emit_cache_state();
        }
    }

    /// Fetch a deck's bytes: the resident copy, or the read another reader
    /// already has in flight, or our own read — never a second read of a file
    /// the share is already sending. What we read ourselves is offered to the
    /// cache, so the next load of the same track is a hit.
    ///
    /// The mutex is never held across a read.
    pub(super) fn read_for_deck(
        &self,
        id: i64,
        path: &Path,
        backoffs: &[Duration],
    ) -> Result<Bytes> {
        loop {
            match self.shared.deck_step(id) {
                DeckStep::Take(bytes) => return Ok(bytes),
                DeckStep::Read(claim) => {
                    let read = read_with_retry(|| read_file(path), thread::sleep, backoffs);
                    // Resident before the claim is released, so a deck waking
                    // on the notification finds the bytes rather than deciding
                    // to read them again.
                    if let Ok(bytes) = &read {
                        self.insert(id, Arc::clone(bytes));
                    }
                    drop(claim);
                    return read;
                }
                DeckStep::Wait => self.shared.wait_for_read(),
            }
        }
    }

    /// Replace the residency window. Evicts out-of-window entries, enforces the
    /// byte cap, emits a cache-state event if membership changed, and wakes the
    /// prefetch worker to fetch any still-missing window entries.
    pub fn set_window(&self, window: Vec<(i64, PathBuf)>) {
        // The whole playlist is accepted; the byte cap (enforced below) bounds
        // how many leading tracks actually stay resident.
        let changed = {
            let mut inner = self.shared.inner.lock();
            inner.window = window;
            inner.generation = inner.generation.wrapping_add(1);
            let a = inner.evict_out_of_window();
            let b = inner.enforce_cap();
            a || b
        };
        if changed {
            self.emit_cache_state();
        }
        // Kick the worker even if nothing was evicted — the window may contain
        // not-yet-fetched entries.
        let _ = self.prefetch_tx.send(());
    }

    /// Snapshot of the currently cached track ids.
    pub fn cached_ids(&self) -> Vec<i64> {
        self.shared.inner.lock().entries.keys().copied().collect()
    }

    fn emit_cache_state(&self) {
        let ids = self.cached_ids();
        let _ = self.app.emit(CACHE_STATE_EVENT, ids);
    }
}

/// Background prefetch loop. Woken by `set_window`; reads missing window entries
/// sequentially, front-first, one at a time.
fn prefetch_worker(shared: Arc<Shared>, app: AppHandle, rx: Receiver<()>) {
    while rx.recv().is_ok() {
        // Coalesce a burst of wake-ups into a single run against the latest
        // window.
        while rx.try_recv().is_ok() {}
        run_prefetch(&shared, &app);
    }
}

fn run_prefetch(shared: &Arc<Shared>, app: &AppHandle) {
    let (window, generation, cap) = {
        let guard = shared.inner.lock();
        (guard.window.clone(), guard.generation, guard.cap)
    };

    let mut retained_bytes: usize = 0;
    for (id, path) in &window {
        // Abort if a newer window superseded ours; the worker will be woken
        // again for the new window.
        if shared.inner.lock().generation != generation {
            return;
        }
        match shared.prefetch_step(*id) {
            Step::Have(len) => {
                retained_bytes += len;
                continue;
            }
            Step::Busy => continue,
            Step::Read => {}
        }
        let Some(_claim) = shared.claim(*id) else {
            continue;
        };

        // Read on the worker thread — one file at a time.
        let bytes = match read_file(path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("cache: prefetch read {} failed: {}", path.display(), e);
                // Surface the failure so the UI can flag the share as
                // unreachable. Idempotent on the frontend; cleared by the next
                // successful cache-state emit below.
                let _ = app.emit(PREFETCH_FAILED_EVENT, ());
                continue;
            }
        };

        // Stop once the cap would be exceeded — leading entries are prioritised
        // and later ones would only be evicted anyway.
        if retained_bytes + bytes.len() > cap {
            break;
        }

        let changed = {
            let mut guard = shared.inner.lock();
            if guard.generation != generation {
                return;
            }
            if !guard.window.iter().any(|(w, _)| w == id) {
                continue;
            }
            guard.entries.insert(*id, bytes.clone());
            retained_bytes += bytes.len();
            // Re-run cap enforcement in case concurrent inserts pushed us over.
            let _ = guard.enforce_cap();
            true
        };
        if changed {
            let ids = shared
                .inner
                .lock()
                .entries
                .keys()
                .copied()
                .collect::<Vec<_>>();
            let _ = app.emit(CACHE_STATE_EVENT, ids);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic cached entry of `len` bytes — no real file/audio needed.
    fn bytes(len: usize) -> Bytes {
        Arc::from(vec![0u8; len].into_boxed_slice())
    }

    fn win(ids: &[i64]) -> Vec<(i64, PathBuf)> {
        ids.iter().map(|id| (*id, PathBuf::from("x"))).collect()
    }

    #[test]
    fn evict_out_of_window_drops_ids_outside_window() {
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.entries.insert(1, bytes(10));
        inner.entries.insert(2, bytes(10));
        inner.entries.insert(3, bytes(10));
        // New window keeps only ids 2 and 4 (4 not yet cached).
        inner.window = win(&[2, 4]);

        let changed = inner.evict_out_of_window();
        assert!(changed, "membership should change");
        let ids: HashSet<i64> = inner.entries.keys().copied().collect();
        assert_eq!(ids, HashSet::from([2]));
    }

    #[test]
    fn evict_out_of_window_is_noop_when_all_in_window() {
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.entries.insert(1, bytes(10));
        inner.entries.insert(2, bytes(10));
        inner.window = win(&[1, 2, 3]);

        assert!(!inner.evict_out_of_window());
        assert_eq!(inner.entries.len(), 2);
    }

    #[test]
    fn enforce_cap_retains_nearest_first_until_full() {
        // Three ~60 MB entries, cap 150 MB → only the first two fit.
        let big = 60 * 1024 * 1024;
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1, 2, 3]);
        inner.entries.insert(1, bytes(big));
        inner.entries.insert(2, bytes(big));
        inner.entries.insert(3, bytes(big));

        let changed = inner.enforce_cap();
        assert!(changed, "third entry should be evicted by the cap");
        let ids: HashSet<i64> = inner.entries.keys().copied().collect();
        assert_eq!(ids, HashSet::from([1, 2]), "nearest two entries retained");
    }

    #[test]
    fn enforce_cap_noop_when_under_cap() {
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1, 2]);
        inner.entries.insert(1, bytes(1024));
        inner.entries.insert(2, bytes(1024));
        assert!(!inner.enforce_cap());
        assert_eq!(inner.entries.len(), 2);
    }

    #[test]
    fn retain_within_cap_stops_at_first_overflow() {
        let big = 100 * 1024 * 1024;
        let window = win(&[1, 2, 3]);
        let mut entries: HashMap<i64, Bytes> = HashMap::new();
        entries.insert(1, bytes(big));
        entries.insert(2, bytes(big)); // 1 + 2 = 200 MB > 150 MB cap
        entries.insert(3, bytes(1024));
        let keep = retain_within_cap(&window, &entries, MAX_CACHE_BYTES);
        assert_eq!(keep, HashSet::from([1]), "only the nearest entry fits");
    }

    #[test]
    fn an_offered_track_in_the_window_becomes_resident() {
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1, 2]);

        assert!(inner.accept(2, bytes(1024)), "membership changed");
        assert_eq!(inner.entries.keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn an_offered_track_outside_the_window_is_not_kept() {
        // The cue deck auditioning a track nobody queued. Keeping it would make
        // residency an LRU by the back door, and the next enforcement would
        // drop it anyway.
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1]);

        assert!(!inner.accept(9, bytes(1024)));
        assert!(inner.entries.is_empty());
    }

    #[test]
    fn an_offered_track_beyond_the_cap_is_not_kept() {
        let big = 100 * 1024 * 1024;
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1, 2]);
        inner.entries.insert(1, bytes(big));

        // 100 + 100 MB over a 150 MB cap: the entry just offered is the one
        // beyond it.
        inner.accept(2, bytes(big));
        assert_eq!(inner.entries.keys().copied().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn re_offering_a_resident_track_changes_nothing() {
        let mut inner = Inner::new(MAX_CACHE_BYTES);
        inner.window = win(&[1]);
        inner.entries.insert(1, bytes(1024));

        assert!(
            !inner.accept(1, bytes(1024)),
            "no membership change to emit"
        );
        assert_eq!(inner.entries.len(), 1);
    }

    #[test]
    fn a_second_reader_is_turned_away_while_a_read_is_in_flight() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        let first = shared.claim(7);
        assert!(first.is_some(), "nobody was reading it");
        assert!(shared.claim(7).is_none(), "someone already is");
        assert!(shared.claim(8).is_some(), "a different file is free");
    }

    #[test]
    fn releasing_a_claim_lets_the_next_reader_in() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        drop(shared.claim(7));
        assert!(shared.claim(7).is_some(), "the claim was released on drop");
    }

    #[test]
    fn prefetch_skips_a_track_a_deck_is_already_reading() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        let _claim = shared.claim(7);
        assert_eq!(shared.prefetch_step(7), Step::Busy);
    }

    #[test]
    fn prefetch_reads_a_track_nobody_holds() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        assert_eq!(shared.prefetch_step(7), Step::Read);
    }

    #[test]
    fn prefetch_counts_a_track_that_is_already_resident() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        shared.inner.lock().entries.insert(7, bytes(1024));
        assert_eq!(shared.prefetch_step(7), Step::Have(1024));
    }

    #[test]
    fn a_deck_takes_the_resident_copy() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        shared.inner.lock().entries.insert(7, bytes(1024));
        assert!(matches!(shared.deck_step(7), DeckStep::Take(b) if b.len() == 1024));
    }

    #[test]
    fn a_deck_reads_a_track_nobody_is_reading() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        assert!(matches!(shared.deck_step(7), DeckStep::Read(_)));
    }

    #[test]
    fn a_deck_waits_for_a_read_already_in_flight() {
        let shared = Arc::new(Shared::new(MAX_CACHE_BYTES));
        let _claim = shared.claim(7);
        assert!(matches!(shared.deck_step(7), DeckStep::Wait));
    }
}
