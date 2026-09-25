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

use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter};

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

/// Whole track file resident in RAM. Cheaply cloned (Arc) between the cache and
/// the decoder.
type Bytes = Arc<[u8]>;

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
}

impl Inner {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            window: Vec::new(),
            generation: 0,
            cap,
        }
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

/// Shared byte cache. Clone the `Arc<Cache>` to share it across decks — every
/// clone points at the same resident entries and the same prefetch worker.
pub struct Cache {
    inner: Arc<Mutex<Inner>>,
    /// Wakes the prefetch worker; the authoritative window lives in `inner`.
    prefetch_tx: Sender<()>,
    app: AppHandle,
}

impl Cache {
    pub fn new(app: AppHandle, max_cache_bytes: usize) -> Arc<Self> {
        let inner = Arc::new(Mutex::new(Inner::new(max_cache_bytes)));
        let (prefetch_tx, prefetch_rx) = channel::<()>();
        {
            let inner = Arc::clone(&inner);
            let app = app.clone();
            thread::spawn(move || prefetch_worker(inner, app, prefetch_rx));
        }
        Arc::new(Self {
            inner,
            prefetch_tx,
            app,
        })
    }

    /// Look up resident bytes for a track id. Never reads the filesystem.
    pub fn get(&self, id: i64) -> Option<Bytes> {
        self.inner.lock().entries.get(&id).cloned()
    }

    /// Replace the residency window. Evicts out-of-window entries, enforces the
    /// byte cap, emits a cache-state event if membership changed, and wakes the
    /// prefetch worker to fetch any still-missing window entries.
    pub fn set_window(&self, window: Vec<(i64, PathBuf)>) {
        // The whole playlist is accepted; the byte cap (enforced below) bounds
        // how many leading tracks actually stay resident.
        let changed = {
            let mut inner = self.inner.lock();
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
        self.inner.lock().entries.keys().copied().collect()
    }

    fn emit_cache_state(&self) {
        let ids = self.cached_ids();
        let _ = self.app.emit(CACHE_STATE_EVENT, ids);
    }
}

/// Background prefetch loop. Woken by `set_window`; reads missing window entries
/// sequentially, front-first, one at a time.
fn prefetch_worker(inner: Arc<Mutex<Inner>>, app: AppHandle, rx: Receiver<()>) {
    while rx.recv().is_ok() {
        // Coalesce a burst of wake-ups into a single run against the latest
        // window.
        while rx.try_recv().is_ok() {}
        run_prefetch(&inner, &app);
    }
}

fn run_prefetch(inner: &Arc<Mutex<Inner>>, app: &AppHandle) {
    let (window, generation, cap) = {
        let guard = inner.lock();
        (guard.window.clone(), guard.generation, guard.cap)
    };

    let mut retained_bytes: usize = 0;
    for (id, path) in &window {
        // Abort if a newer window superseded ours; the worker will be woken
        // again for the new window.
        {
            let guard = inner.lock();
            if guard.generation != generation {
                return;
            }
            if let Some(b) = guard.entries.get(id) {
                retained_bytes += b.len();
                continue;
            }
        }

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
            let mut guard = inner.lock();
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
            let ids = inner.lock().entries.keys().copied().collect::<Vec<_>>();
            let _ = app.emit(CACHE_STATE_EVENT, ids);
        }
    }
}

/// Read a whole file into a shared byte buffer. Kept as a standalone helper so
/// the read path can be shared/adjusted independently of cache bookkeeping.
fn read_file(path: &Path) -> Result<Bytes> {
    let vec = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(Arc::from(vec.into_boxed_slice()))
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
}
