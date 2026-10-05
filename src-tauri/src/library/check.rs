//! The library check: what the next scan would change, found by listing and
//! stat alone. It never reads tags or audio and never writes to the library.

use anyhow::Result;
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Listener};

use super::db::{Db, IndexRow};
use super::health::Health;
use super::listing::{self, ScanRoot};
use super::scan_state::{ScanState, ScanStatus};
use super::waveform_scan::WaveformJob;
use crate::persist::config::Config;

/// Give the window time to come up before the launch check reads the disk.
const LAUNCH_DELAY: Duration = Duration::from_secs(5);
/// How often a disabled timer looks at its setting again.
const IDLE_POLL: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone, Debug, Default, PartialEq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub checked_at: i64,
    /// Listed files no present track has.
    pub new: Vec<String>,
    /// Present tracks whose file's mtime or root changed.
    pub changed: Vec<String>,
    /// Present tracks under a completely listed root whose file is gone.
    pub gone: Vec<String>,
    /// Present tracks no configured library path contains any more.
    pub unrooted: Vec<String>,
    /// Library paths that are not a readable directory.
    pub unreachable: Vec<String>,
    /// Library paths listed with unreadable parts; nothing under them counts
    /// as gone.
    pub partial: Vec<String>,
}

/// A finished check: what it found, and what the files it found looked like.
pub struct Checked {
    pub report: CheckReport,
    /// Identifies the **bytes** behind the new and changed files, not just
    /// their paths: every listed file's modification time and size go into it.
    /// Two checks that agree on this are two checks of a disk that has stopped
    /// moving, which is what an automatic scan waits for. A path list alone
    /// would read the same all the way through a long copy.
    pub settle: u64,
}

impl CheckReport {
    pub fn has_changes(&self) -> bool {
        !(self.new.is_empty()
            && self.changed.is_empty()
            && self.gone.is_empty()
            && self.unrooted.is_empty())
    }

    /// Whether anything here is a file to read. An automatic scan runs on these
    /// alone: it adds and updates, and never retires a track.
    pub fn has_additions(&self) -> bool {
        !(self.new.is_empty() && self.changed.is_empty())
    }

    /// Identifies what was found, not when.
    pub fn signature(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut copy = self.clone();
        copy.checked_at = 0;
        copy.hash(&mut hasher);
        hasher.finish()
    }
}

/// Compare the disk with the library. `cancel` is polled between files.
pub fn check(db: &Db, roots: &[ScanRoot], cancel: &dyn Fn() -> bool) -> Result<Option<Checked>> {
    let listing = listing::list_roots(roots);
    let index = db.track_index()?;
    let present: HashMap<&str, &IndexRow> = index
        .iter()
        .filter(|r| r.missing_since.is_none())
        .map(|r| (r.path.as_str(), r))
        .collect();

    let mut report = CheckReport {
        checked_at: now_ms(),
        unreachable: listing.unreachable.clone(),
        partial: listing.partial.clone(),
        ..Default::default()
    };
    // Sorted before hashing, since the settle key has to identify the disk
    // rather than the order the roots happened to list it in.
    let mut stamps: Vec<(&str, listing::Stamp)> = Vec::new();
    for file in &listing.found {
        if cancel() {
            return Ok(None);
        }
        let stamp = file.stamp();
        match present.get(file.path.as_str()) {
            None => {
                report.new.push(file.path.clone());
                stamps.push((&file.path, stamp));
            }
            Some(row) if file.changed(row, stamp.mtime_ms) => {
                report.changed.push(file.path.clone());
                stamps.push((&file.path, stamp));
            }
            Some(_) => {}
        }
    }
    for row in listing.gone(&index, roots) {
        let path = Path::new(&row.path);
        if roots.iter().any(|r| path.starts_with(&r.path)) {
            report.gone.push(row.path.clone());
        } else {
            report.unrooted.push(row.path.clone());
        }
    }
    for list in [
        &mut report.new,
        &mut report.changed,
        &mut report.gone,
        &mut report.unrooted,
    ] {
        list.sort();
    }
    stamps.sort_by_key(|(path, _)| *path);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stamps.hash(&mut hasher);
    Ok(Some(Checked {
        report,
        settle: hasher.finish(),
    }))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct ScanStatusTag {
    status: String,
}

/// Whether a check is one an automatic scan may act on, decided apart from the
/// worker so the rules can be tested without a timer or a window.
///
/// `previous` is the [`Checked::settle`] key of the check before this one.
/// Requiring the two to match is what keeps a file still being written from
/// being scanned half-finished: the key carries every listed file's size and
/// modification time, so it only reads the same twice once the bytes have
/// stopped moving.
///
/// Files that are gone are deliberately not enough. An automatic scan adds and
/// updates; retiring a track stays the operator's, so a share that has come back
/// as an empty directory cannot quietly mark a library missing.
fn settled_with_additions(checked: &Checked, previous: Option<u64>) -> bool {
    checked.report.has_additions()
        && checked.report.unreachable.is_empty()
        && checked.report.partial.is_empty()
        && previous == Some(checked.settle)
}

/// Runs checks at launch, on the configured timer and on demand, never
/// alongside a scan.
pub struct LibraryCheck {
    db: Arc<Db>,
    config: Arc<Config>,
    scan: Arc<ScanState>,
    health: Arc<Health>,
    /// Handed to an automatic scan, which owes the library the analysis pass
    /// the same way the operator's button does.
    waveform: Arc<WaveformJob>,
    /// Bumped by every scan transition, so a check that overlapped one is
    /// discarded rather than reporting a disk the scan has since applied.
    generation: AtomicU64,
    cancel: AtomicBool,
    schedule: Mutex<Schedule>,
    /// The previous check's [`Checked::settle`] key, for the settling rule in
    /// [`settled_with_additions`]. Cleared by every scan transition, so an
    /// automatic scan is always preceded by two checks that agree.
    settling: Mutex<Option<u64>>,
    /// The settle key an automatic scan last ran on. A scan that leaves the
    /// disk reading exactly as it did — a file no tag reader can parse is
    /// reported as new forever — must not be started again on the same
    /// evidence, or the share is rescanned every two intervals for good.
    attempted: Mutex<Option<u64>>,
    wake: Condvar,
}

struct Schedule {
    /// When the last check ran, or a scan made the last report moot.
    last: Option<Instant>,
    now_requested: bool,
}

impl LibraryCheck {
    pub fn new(
        db: Arc<Db>,
        config: Arc<Config>,
        scan: Arc<ScanState>,
        health: Arc<Health>,
        waveform: Arc<WaveformJob>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            config,
            scan,
            health,
            waveform,
            generation: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            schedule: Mutex::new(Schedule {
                last: None,
                now_requested: false,
            }),
            settling: Mutex::new(None),
            attempted: Mutex::new(None),
            wake: Condvar::new(),
        })
    }

    /// Start the worker, and follow the scan: a starting scan cancels a check,
    /// a completed one makes the report moot, and a canceled one leaves new
    /// files behind that are worth reporting.
    pub fn start(self: &Arc<Self>, app: &AppHandle) {
        let this = Arc::clone(self);
        app.listen("scan-state-changed", move |event| {
            let Ok(tag) = serde_json::from_str::<ScanStatusTag>(event.payload()) else {
                return;
            };
            this.generation.fetch_add(1, Ordering::SeqCst);
            // Whatever the scan did, the disk it listed is no longer the one
            // the settling rule agreed on. Two fresh checks have to agree
            // before another automatic scan.
            *this.settling.lock() = None;
            match tag.status.as_str() {
                "running" => this.cancel.store(true, Ordering::SeqCst),
                "idle" => {
                    this.health.set_check(None);
                    this.schedule.lock().last = Some(Instant::now());
                }
                "canceled" => this.request(),
                _ => {}
            }
        });

        let this = Arc::clone(self);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(LAUNCH_DELAY);
            loop {
                this.run_once(&app);
                this.wait_until_due();
            }
        });
    }

    /// Check as soon as possible.
    pub fn request(&self) {
        self.schedule.lock().now_requested = true;
        self.wake.notify_all();
    }

    fn wait_until_due(&self) {
        let mut schedule = self.schedule.lock();
        loop {
            if schedule.now_requested {
                schedule.now_requested = false;
                return;
            }
            let interval_min = self.config.get_tuning().library.check_interval_min;
            let wait = if interval_min == 0 {
                IDLE_POLL
            } else {
                let interval = Duration::from_secs(interval_min * 60);
                let elapsed = schedule.last.map_or(interval, |t| t.elapsed());
                if elapsed >= interval {
                    return;
                }
                (interval - elapsed).min(IDLE_POLL)
            };
            self.wake.wait_for(&mut schedule, wait);
        }
    }

    fn run_once(&self, app: &AppHandle) {
        self.schedule.lock().last = Some(Instant::now());
        if self.scan.is_running() {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        let generation = self.generation.load(Ordering::SeqCst);
        self.health.set_checking(true);
        let roots = listing::configured_roots(&self.db);
        let outcome = check(&self.db, &roots, &|| self.cancel.load(Ordering::SeqCst));
        let mut result = None;
        match outcome {
            Ok(Some(checked)) if self.generation.load(Ordering::SeqCst) == generation => {
                let report = &checked.report;
                let level = if report.has_changes() || !report.unreachable.is_empty() {
                    log::Level::Info
                } else {
                    log::Level::Debug
                };
                log::log!(
                    level,
                    "library check: {} new, {} changed, {} gone, {} outside library paths, {} unreachable",
                    report.new.len(),
                    report.changed.len(),
                    report.gone.len(),
                    report.unrooted.len(),
                    report.unreachable.len()
                );
                result = Some(checked);
            }
            Ok(_) => log::debug!("library check: superseded by a scan"),
            Err(e) => log::error!("library check failed: {e:#}"),
        }
        let scan_now = result.as_ref().is_some_and(|c| self.may_scan(c));
        self.health.finish_check(result.map(|c| c.report));
        if scan_now {
            log::info!("library check: the disk has settled with new work, scanning");
            // Without the tag backfill the operator's button also starts: a
            // cancelled backfill was cancelled on purpose, and nothing the
            // operator stopped should restart because a file appeared.
            Arc::clone(&self.scan).start_additive(
                app.clone(),
                Arc::clone(&self.db),
                Arc::clone(&self.config),
                Arc::clone(&self.waveform),
            );
        }
    }

    /// Whether this check should start a scan by itself, and remember it for the
    /// next check either way.
    ///
    /// The setting is off by default, and four things hold a scan back even when
    /// it is on: a disk still moving or with nothing to add (see
    /// [`settled_with_additions`]), evidence a previous automatic scan already
    /// acted on and could not clear, a report the operator dismissed, and a scan
    /// they cancelled.
    fn may_scan(&self, checked: &Checked) -> bool {
        let previous = self.settling.lock().replace(checked.settle);
        if !self.config.get_tuning().library.scan_on_changes {
            return false;
        }
        if !settled_with_additions(checked, previous) {
            return false;
        }
        if *self.attempted.lock() == Some(checked.settle) {
            log::debug!("library check: this is the disk the last automatic scan already read");
            return false;
        }
        if self.health.check_dismissed() == Some(checked.report.signature()) {
            log::debug!("library check: the disk changes are settled, but dismissed");
            return false;
        }
        if matches!(self.scan.status(), ScanStatus::Canceled { .. }) {
            log::debug!("library check: the disk changes are settled, but a scan was canceled");
            return false;
        }
        *self.attempted.lock() = Some(checked.settle);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_measure::test_audio::write_wav;
    use crate::library::db::Reconcile;
    use crate::library::scanner::{scan_all, Missing};
    use std::fs;
    use tempfile::TempDir;

    fn root(dir: &Path) -> Vec<ScanRoot> {
        vec![ScanRoot {
            content_type: "music",
            path: dir.to_string_lossy().into_owned(),
        }]
    }

    fn scanned(files: &[&str]) -> (TempDir, Db) {
        let dir = TempDir::new().unwrap();
        for (i, name) in files.iter().enumerate() {
            write_wav(&dir.path().join(name), i as u32 + 1, 1);
        }
        let db = Db::open_in_memory().unwrap();
        scan_all(&db, &root(dir.path()), Missing::Mark, &|| false, |_, _| {}).unwrap();
        (dir, db)
    }

    fn run(db: &Db, roots: &[ScanRoot]) -> CheckReport {
        checked(db, roots).report
    }

    fn checked(db: &Db, roots: &[ScanRoot]) -> Checked {
        check(db, roots, &|| false).unwrap().unwrap()
    }

    fn names(paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn an_unchanged_library_reports_nothing() {
        let (dir, db) = scanned(&["a.wav", "b.wav"]);
        let report = run(&db, &root(dir.path()));
        assert!(!report.has_changes(), "{report:?}");
        assert!(report.unreachable.is_empty() && report.partial.is_empty());
    }

    #[test]
    fn new_changed_and_gone_files_are_reported_without_touching_the_library() {
        let (dir, db) = scanned(&["keep.wav", "edit.wav", "drop.wav"]);
        write_wav(&dir.path().join("new.wav"), 9, 1);
        let edit = dir.path().join("edit.wav");
        let later = SystemTime::now() + Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&edit)
            .unwrap()
            .set_modified(later)
            .unwrap();
        fs::remove_file(dir.path().join("drop.wav")).unwrap();
        let before = db.track_index().unwrap().len();

        let report = run(&db, &root(dir.path()));
        assert_eq!(names(&report.new), vec!["new.wav"]);
        assert_eq!(names(&report.changed), vec!["edit.wav"]);
        assert_eq!(names(&report.gone), vec!["drop.wav"]);
        assert!(report.unrooted.is_empty());

        let index = db.track_index().unwrap();
        assert_eq!(index.len(), before);
        assert!(index.iter().all(|r| r.missing_since.is_none()));
    }

    #[test]
    fn a_file_back_at_a_missing_tracks_path_is_new() {
        let (dir, db) = scanned(&["a.wav"]);
        let id = db.track_index().unwrap()[0].id;
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();
        let report = run(&db, &root(dir.path()));
        assert_eq!(names(&report.new), vec!["a.wav"]);
    }

    #[test]
    fn an_unreachable_root_is_reported_and_its_tracks_are_not_gone() {
        let (dir, db) = scanned(&["a.wav"]);
        let path = dir.path().to_path_buf();
        let roots = root(&path);
        drop(dir);
        let report = run(&db, &roots);
        assert_eq!(
            report.unreachable,
            vec![path.to_string_lossy().into_owned()]
        );
        assert!(!report.has_changes(), "{report:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_partially_readable_root_marks_nothing_gone() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, db) = scanned(&["a.wav"]);
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        fs::remove_file(dir.path().join("a.wav")).unwrap();
        let report = run(&db, &root(dir.path()));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(report.partial.len(), 1);
        assert!(report.gone.is_empty(), "{report:?}");
    }

    #[test]
    fn tracks_outside_every_root_are_reported_separately() {
        let (dir, db) = scanned(&["a.wav"]);
        let other = TempDir::new().unwrap();
        let report = run(&db, &root(other.path()));
        assert!(report.gone.is_empty());
        assert_eq!(names(&report.unrooted), vec!["a.wav"]);
        drop(dir);
    }

    #[test]
    fn a_check_agrees_with_the_scan_that_follows_it() {
        let (dir, db) = scanned(&["a.wav", "b.wav"]);
        fs::remove_file(dir.path().join("a.wav")).unwrap();
        write_wav(&dir.path().join("c.wav"), 7, 1);
        let report = run(&db, &root(dir.path()));
        let outcome =
            scan_all(&db, &root(dir.path()), Missing::Mark, &|| false, |_, _| {}).unwrap();
        assert_eq!(outcome.missing, report.gone.len());
        assert!(!run(&db, &root(dir.path())).has_changes());
    }

    #[test]
    fn a_canceled_check_reports_nothing() {
        let (dir, db) = scanned(&["a.wav"]);
        assert!(check(&db, &root(dir.path()), &|| true).unwrap().is_none());
    }

    /// A check of a settled disk holding one new file, with an arbitrary settle
    /// key standing in for its stamps.
    fn settled() -> Checked {
        Checked {
            report: CheckReport {
                checked_at: 1,
                new: vec!["a.wav".into()],
                ..Default::default()
            },
            settle: 7,
        }
    }

    #[test]
    fn an_automatic_scan_waits_for_two_checks_that_agree() {
        let checked = settled();
        assert!(
            !settled_with_additions(&checked, None),
            "the first check of a change has nothing to agree with"
        );
        assert!(settled_with_additions(&checked, Some(checked.settle)));
    }

    #[test]
    fn a_disk_still_moving_does_not_start_a_scan() {
        let second = settled();
        assert!(
            !settled_with_additions(&second, Some(second.settle ^ 1)),
            "the same paths with different bytes behind them are not settled"
        );
    }

    #[test]
    fn nothing_to_apply_starts_no_scan() {
        let quiet = Checked {
            report: CheckReport {
                checked_at: 1,
                ..Default::default()
            },
            settle: 7,
        };
        assert!(!settled_with_additions(&quiet, Some(quiet.settle)));
    }

    #[test]
    fn files_that_are_only_gone_start_no_scan() {
        let retirements = Checked {
            report: CheckReport {
                checked_at: 1,
                gone: vec!["a.wav".into()],
                unrooted: vec!["b.wav".into()],
                ..Default::default()
            },
            settle: 7,
        };
        assert!(
            retirements.report.has_changes(),
            "the operator is still told about them"
        );
        assert!(
            !settled_with_additions(&retirements, Some(retirements.settle)),
            "an empty share must not retire a library with nobody watching"
        );
    }

    #[test]
    fn an_unreadable_library_path_holds_an_automatic_scan_back() {
        for checked in [
            Checked {
                report: CheckReport {
                    unreachable: vec!["/mnt/radio".into()],
                    ..settled().report
                },
                settle: 7,
            },
            Checked {
                report: CheckReport {
                    partial: vec!["/mnt/radio".into()],
                    ..settled().report
                },
                settle: 7,
            },
        ] {
            assert!(
                !settled_with_additions(&checked, Some(checked.settle)),
                "a listing that cannot prove what is there must not drive a scan"
            );
        }
    }

    #[test]
    fn the_settle_key_follows_the_bytes_not_the_paths() {
        let (dir, db) = scanned(&["a.wav"]);
        write_wav(&dir.path().join("b.wav"), 3, 1);
        let first = checked(&db, &root(dir.path()));
        assert_eq!(names(&first.report.new), vec!["b.wav"]);
        assert_eq!(
            first.settle,
            checked(&db, &root(dir.path())).settle,
            "a disk nobody touched reads the same twice"
        );

        // The same path, more bytes behind it: a copy still in progress.
        write_wav(&dir.path().join("b.wav"), 3, 4);
        let grown = checked(&db, &root(dir.path()));
        assert_eq!(names(&grown.report.new), vec!["b.wav"], "same path");
        assert_ne!(
            first.settle, grown.settle,
            "a file that is still growing is not settled"
        );
    }

    #[test]
    fn the_signature_ignores_when_the_check_ran() {
        let a = CheckReport {
            checked_at: 1,
            new: vec!["x".into()],
            ..Default::default()
        };
        let b = CheckReport {
            checked_at: 2,
            ..a.clone()
        };
        assert_eq!(a.signature(), b.signature());
        let c = CheckReport {
            new: vec!["y".into()],
            ..a.clone()
        };
        assert_ne!(a.signature(), c.signature());
    }
}
