//! Filling tag columns that a row predates.
//!
//! A scan reads a file's tags only when the file changed:
//! [`listing::should_rescan`](super::listing::should_rescan) skips an unchanged
//! one without opening it, and there is no command that forces a full re-read.
//! So adding a tag-derived column leaves every existing row `NULL` forever —
//! which is what this pass exists to fix.
//!
//! [`scanner::TAG_READ_VERSION`] is the generation marker. Every row carries the
//! version its tags were read at; bumping the constant drops the whole library
//! back into the queue, and the next launch fills the new column in. That makes
//! the next tag addition a schema step and a bumped constant, with no new
//! machinery.
//!
//! It is shaped like [`waveform_scan`](super::waveform_scan) — single-flight,
//! cancelable, re-draining — but deliberately does not share its queue. That one
//! excludes rows with a recorded decode failure, and a file whose audio will not
//! decode very often still has perfectly readable tags. Sharing it would also
//! mean a tag read that failed stamped `analysis_failed_at` and blocked the
//! row's waveform, loudness and cue points for good.
//!
//! Reading is spread over a small fixed pool rather than the decode pool's
//! `cores - 2`: a tag read is a header read, so the pass is latency-bound on the
//! share rather than CPU-bound. Results are committed in batches, because each
//! one also rewrites the row's search-index entry.

use parking_lot::Mutex;
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use super::db::{Db, TagReadJob, TrackInsert};
use super::scanner::{self, read_file_tags};

/// Running/idle transitions. Going idle is the signal that library rows changed
/// underneath whatever the renderer last listed.
///
/// There is deliberately no per-track progress event. The pass is a header read
/// per row, so it is far quicker than the decode the waveform bar exists for,
/// and nothing in the UI shows it — an event nobody listens to is just noise on
/// the channel.
pub const STATE_EVENT: &str = "tag-backfill-state-changed";
/// Parallel tag readers. A tag read is a `stat` plus a header read, so this is
/// latency-bound on the library share rather than CPU-bound — the same reason
/// [`scanner::SCAN_CONCURRENCY`](super::scanner) uses a small fixed number
/// instead of scaling with cores.
const READ_CONCURRENCY: usize = 4;
/// Rows per commit. Each one rewrites an FTS entry, since `album_artist` is in
/// the index, so committing per row would pay an fsync for every track.
const COMMIT_BATCH: usize = 200;

/// Whether the pass is working, as [`STATE_EVENT`] reports it. `total` is what
/// the drain found, so a listener can say how much there was to do; there is no
/// running count, because nothing displays one.
#[derive(Serialize, Clone, Default, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TagBackfillStatus {
    #[default]
    Idle,
    #[serde(rename_all = "camelCase")]
    Running { total: usize },
}

#[derive(Default)]
pub struct TagBackfillJob {
    running: AtomicBool,
    /// Work arrived while a worker was running. See [`TagBackfillJob::start`].
    kicked: AtomicBool,
    cancel: AtomicBool,
    /// Whether the last thing announced was `Running`. Only then does a pass owe
    /// the renderer the idle that says the rows it listed have changed.
    announced: AtomicBool,
}

impl TagBackfillJob {
    /// Request the running worker (if any) to stop after the current batch.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Kick the worker. No-op if one is already running (single-flight): the
    /// running worker re-drains on each pass, so it observes rows a concurrent
    /// scan just added. A kick landing after that worker's last drain is
    /// recorded rather than dropped.
    pub fn start(self: Arc<Self>, app: AppHandle, db: Arc<Db>) {
        if self.running.swap(true, Ordering::SeqCst) {
            self.kicked.store(true, Ordering::SeqCst);
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            let announce = |next: TagBackfillStatus| self.announce(&app, next);
            self.drain_loop(|| run(&self, &app, &db), announce);
        });
    }

    /// Drain until nothing asks for another pass, releasing the single-flight
    /// slot however each one ends — a panic included.
    ///
    /// Uncaught, a panic unwinds past the release and holds the slot for the
    /// life of the process: no row's tags are ever backfilled again, and the
    /// renderer never gets the idle event that tells it the rows changed. A tag
    /// read is `read_file_tags` over an operator's file, so the panic is one
    /// third-party parser away. The panic itself is logged by the hook `lib.rs`
    /// installs.
    ///
    /// A caught panic announces the idle the pass died owing — only if it had
    /// announced itself running, since that idle is what tells the renderer the
    /// rows it listed have changed. A panic under a cancel consumes the flag and
    /// stops, the way the pass it killed would have: a kick waiting behind the
    /// cancel must not be what starts a fresh pass over the whole queue.
    ///
    /// `pass` and `announce` are injected so the loop can be driven without a
    /// filesystem or an `AppHandle`.
    fn drain_loop(&self, mut pass: impl FnMut(), mut announce: impl FnMut(TagBackfillStatus)) {
        loop {
            // Cleared before the drain, so a kick during it is never lost.
            self.kicked.store(false, Ordering::SeqCst);
            let mut cancelled = false;
            if catch_unwind(AssertUnwindSafe(&mut pass)).is_err() {
                log::error!("tag backfill: the pass panicked; releasing the slot");
                if self.announced.load(Ordering::SeqCst) {
                    announce(TagBackfillStatus::Idle);
                }
                cancelled = self.cancel.swap(false, Ordering::SeqCst);
            }
            self.running.store(false, Ordering::SeqCst);
            if cancelled || !self.claim_rerun() {
                break;
            }
        }
    }

    /// Whether the worker that just released the slot should take it back: only
    /// for a kick it has not served, only if it is not cancelled, and only if no
    /// other kick claimed the free slot first.
    fn claim_rerun(&self) -> bool {
        !self.cancel.load(Ordering::SeqCst)
            && self.kicked.swap(false, Ordering::SeqCst)
            && !self.running.swap(true, Ordering::SeqCst)
    }

    fn announce(&self, app: &AppHandle, next: TagBackfillStatus) {
        self.announced.store(
            matches!(next, TagBackfillStatus::Running { .. }),
            Ordering::SeqCst,
        );
        let _ = app.emit(STATE_EVENT, &next);
    }
}

/// One pass: drain the queue once and fill what it returned.
///
/// Unlike the analysis pass this does not re-drain. A row only enters the queue
/// by being written at an older version, and the only writer that can do that
/// is a build older than this one — a concurrent scan stamps the current
/// version as it inserts. So a second drain can only return the rows this one
/// could not read, which would spin.
fn run(job: &TagBackfillJob, app: &AppHandle, db: &Db) {
    let pending = match db.tracks_needing_tag_read(scanner::TAG_READ_VERSION) {
        Ok(p) => p,
        Err(e) => {
            log::error!("tag backfill: query failed: {e}");
            return;
        }
    };
    if pending.is_empty() || job.cancel.load(Ordering::SeqCst) {
        return;
    }

    job.announce(
        app,
        TagBackfillStatus::Running {
            total: pending.len(),
        },
    );

    for chunk in pending.chunks(COMMIT_BATCH) {
        if job.cancel.load(Ordering::SeqCst) {
            break;
        }
        let read = read_batch(job, chunk);
        if let Err(e) = db.store_backfilled_tags(&read) {
            // The row state is the queue, so these rows simply come back on the
            // next launch. Stop rather than keep hammering a failing database.
            log::error!("tag backfill: store failed: {e}");
            break;
        }
    }

    // Going idle is what tells the renderer these rows changed under whatever
    // it last listed.
    job.announce(app, TagBackfillStatus::Idle);
}

/// Read one chunk's tags across [`READ_CONCURRENCY`] threads, dropping the
/// files that could not be read at all — those keep their old version and come
/// back on the next launch. A parser that panics rather than returning an error
/// is dropped the same way: uncaught it would be re-raised when `thread::scope`
/// joins and take the whole pass with it.
fn read_batch(job: &TagBackfillJob, chunk: &[TagReadJob]) -> Vec<(TagReadJob, TrackInsert)> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(TagReadJob, TrackInsert)>> = Mutex::new(Vec::with_capacity(chunk.len()));
    std::thread::scope(|scope| {
        for _ in 0..READ_CONCURRENCY.min(chunk.len().max(1)) {
            scope.spawn(|| loop {
                if job.cancel.load(Ordering::SeqCst) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = chunk.get(i) else {
                    break;
                };
                match catch_unwind(AssertUnwindSafe(|| read_file_tags(&item.path))) {
                    Ok(Ok(parsed)) => out.lock().push((item.clone(), parsed)),
                    Ok(Err(e)) => {
                        log::warn!("tag backfill: read {} failed: {e:#}", item.path);
                    }
                    Err(_) => {
                        log::error!("tag backfill: reading {} panicked", item.path);
                    }
                }
            });
        }
    });
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panicking pass must not take the single-flight slot with it: no row's
    /// tags would ever be backfilled again, and the renderer would never get the
    /// idle event that tells it rows changed.
    #[test]
    fn a_panicking_pass_releases_the_slot() {
        let job = TagBackfillJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.announced.store(true, Ordering::SeqCst);
        let announced: Mutex<Vec<TagBackfillStatus>> = Mutex::new(Vec::new());

        job.drain_loop(
            || panic!("a tag parser went down"),
            |next| announced.lock().push(next),
        );

        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(
            matches!(announced.lock().as_slice(), [TagBackfillStatus::Idle]),
            "the rows are announced as settled"
        );
    }

    /// The idle is what tells the renderer to re-list the library. A pass that
    /// died before it touched a row has nothing to tell it.
    #[test]
    fn a_pass_that_panicked_before_touching_a_row_says_nothing() {
        let job = TagBackfillJob::default();
        job.running.store(true, Ordering::SeqCst);
        let announced: Mutex<Vec<TagBackfillStatus>> = Mutex::new(Vec::new());

        job.drain_loop(
            || panic!("the queue query went down"),
            |next| announced.lock().push(next),
        );

        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(announced.lock().is_empty(), "nothing was announced");
    }

    /// The panic consumes the cancel the dead pass would have, and stops with
    /// it: a kick waiting behind the cancel must not start a fresh pass over the
    /// whole queue the operator just stopped.
    #[test]
    fn a_panicking_pass_under_a_cancel_stops() {
        let job = TagBackfillJob::default();
        job.running.store(true, Ordering::SeqCst);
        job.cancel();
        let passes = AtomicUsize::new(0);

        job.drain_loop(
            || {
                passes.fetch_add(1, Ordering::SeqCst);
                job.kicked.store(true, Ordering::SeqCst);
                panic!("a tag parser went down");
            },
            |_| {},
        );

        assert_eq!(passes.load(Ordering::SeqCst), 1, "the stop stands");
        assert!(!job.cancel.load(Ordering::SeqCst), "the cancel is consumed");
        assert!(!job.running.load(Ordering::SeqCst), "the slot is free");
        assert!(
            job.kicked.load(Ordering::SeqCst),
            "the kick stands for the next start"
        );
    }

    /// Work that arrived while the pass was dying still gets a pass.
    #[test]
    fn a_kick_is_served_after_a_panicking_pass() {
        let job = TagBackfillJob::default();
        job.running.store(true, Ordering::SeqCst);
        let passes = AtomicUsize::new(0);

        job.drain_loop(
            || {
                if passes.fetch_add(1, Ordering::SeqCst) == 0 {
                    job.kicked.store(true, Ordering::SeqCst);
                    panic!("a tag parser went down");
                }
            },
            |_| {},
        );

        assert_eq!(passes.load(Ordering::SeqCst), 2, "the kick got its pass");
        assert!(!job.running.load(Ordering::SeqCst));
    }
}
