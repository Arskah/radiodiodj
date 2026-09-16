use anyhow::Result;
use base64::Engine;
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

use super::db::{Db, TrackInsert};
use crate::audio::formats;

/// Upper bound on parallel tag-read workers. A scan is dominated by per-file
/// I/O (stat + header read), which on a networked share is latency-bound —
/// reading several files at once hides that latency. Capped so a scan does not
/// hammer the share.
const SCAN_CONCURRENCY: usize = 4;

pub struct ScanRunResult {
    pub total: usize,
    pub added: usize,
    pub live_files: Vec<String>,
}

pub fn find_audio_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || e.file_name()
                    .to_string_lossy()
                    .chars()
                    .next()
                    .map(|c| c != '.')
                    .unwrap_or(true)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(formats::is_audio_extension)
                .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect()
}

pub fn should_rescan(
    existing_content_type: Option<&str>,
    existing_mtime: Option<i64>,
    file_mtime_ms: i64,
    content_type: &str,
) -> bool {
    let Some(prev_ct) = existing_content_type else {
        return true;
    };
    let Some(prev_mtime) = existing_mtime else {
        return true;
    };
    if prev_ct != content_type {
        return true;
    }
    prev_mtime != file_mtime_ms
}

/// A configured library path and the content type it feeds.
pub struct ScanRoot {
    pub content_type: &'static str,
    pub path: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct ScanOutcome {
    pub total: usize,
    pub added: usize,
    pub canceled: bool,
}

/// Scan every root and bring the library in line with what is on disk.
/// `on_progress` receives `(processed, total)` counted across all roots.
pub fn scan_all(
    db: &Db,
    roots: &[ScanRoot],
    cancel: &(impl Fn() -> bool + Sync),
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<ScanOutcome> {
    let all_paths: Vec<String> = roots.iter().map(|r| r.path.clone()).collect();
    db.remove_tracks_not_in_paths(&all_paths)?;

    let mut outcome = ScanOutcome::default();
    let mut live_by_root: Vec<(&str, HashSet<String>)> = Vec::new();
    for root in roots {
        let (done_before, total_before) = (outcome.total, outcome.total);
        let r = scan_directory(
            db,
            Path::new(&root.path),
            root.content_type,
            cancel,
            |processed, total| on_progress(done_before + processed, total_before + total),
        )?;
        outcome.total += r.total;
        outcome.added += r.added;
        live_by_root.push((&root.path, r.live_files.into_iter().collect()));
        if cancel() {
            outcome.canceled = true;
            return Ok(outcome);
        }
    }

    let mut pruned = 0usize;
    for (root, live) in live_by_root {
        let stale: Vec<String> = db
            .get_paths_under(root)?
            .into_iter()
            .filter(|p| !live.contains(p))
            .collect();
        if !stale.is_empty() {
            pruned += db.delete_by_paths(&stale)?;
        }
    }
    if pruned > 0 {
        log::info!("scan pruned {} missing files", pruned);
    }
    Ok(outcome)
}

pub fn scan_directory<F>(
    db: &Db,
    dir: &Path,
    content_type: &str,
    cancel: &(impl Fn() -> bool + Sync),
    on_progress: F,
) -> Result<ScanRunResult>
where
    F: Fn(usize, usize) + Sync,
{
    let files = find_audio_files(dir);
    let total = files.len();
    // All discovered paths — used for prune bookkeeping regardless of how many
    // we get through (a cancel just skips the prune step upstream).
    let live: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    // One query for all existing rows under this directory instead of a SELECT
    // per file.
    let existing = db.track_meta_under(&dir.to_string_lossy())?;

    // Tag reads are I/O-bound, so fan the files out across a small pool; each
    // worker claims the next index via `next`. Parsed rows collect in `pending`
    // and are upserted in a single transaction at the end — a per-row
    // autocommit is the other thing that makes a big scan slow.
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let pending: Mutex<Vec<TrackInsert>> = Mutex::new(Vec::new());
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(SCAN_CONCURRENCY);

    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            scope.spawn(|| loop {
                if cancel() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(i) else {
                    break;
                };
                let path_str = &live[i];

                let mtime_ms = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                let prev = existing.get(path_str);
                let need = should_rescan(
                    prev.map(|r| r.content_type.as_str()),
                    prev.and_then(|r| r.mtime),
                    mtime_ms,
                    content_type,
                );
                if need {
                    match parse_track(path_str, content_type, mtime_ms) {
                        Ok(track) => pending.lock().push(track),
                        Err(e) => log::error!("scan: failed to parse {}: {}", path_str, e),
                    }
                }
                let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(done, total);
            });
        }
    });

    let pending = pending.into_inner();
    let added = pending.len();
    db.insert_tracks(&pending)?;

    Ok(ScanRunResult {
        total,
        added,
        live_files: live,
    })
}

fn parse_track(path: &str, content_type: &str, mtime_ms: i64) -> Result<TrackInsert> {
    let p = Path::new(path);
    let basename = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let format = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());

    let tagged = Probe::open(path)?.read()?;
    let primary = tagged.primary_tag();

    let title = primary
        .and_then(|t| t.get_string(ItemKey::TrackTitle).map(|s| s.to_string()))
        .unwrap_or(basename);
    let artist = primary
        .and_then(|t| t.get_string(ItemKey::TrackArtist).map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown".into());
    let album = primary
        .and_then(|t| t.get_string(ItemKey::AlbumTitle).map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown".into());
    let genre = primary.and_then(|t| t.get_string(ItemKey::Genre).map(|s| s.to_string()));
    let year = primary.and_then(|t| {
        t.get_string(ItemKey::Year)
            .and_then(|s| s.parse::<i64>().ok())
    });
    let bpm = primary.and_then(|t| {
        t.get_string(ItemKey::Bpm)
            .and_then(|s| s.parse::<f64>().ok())
    });

    let props = tagged.properties();
    let duration = props.duration().as_secs_f64();
    let sample_rate = props.sample_rate().map(|x| x as i64);
    let bitrate = props.audio_bitrate().map(|x| x as i64);

    Ok(TrackInsert {
        path: path.to_string(),
        content_type: content_type.to_string(),
        title: Some(title),
        artist: Some(artist),
        album: Some(album),
        genre,
        year,
        duration: Some(duration),
        bpm,
        sample_rate,
        bitrate,
        format,
        mtime: Some(mtime_ms),
    })
}

/// Read the first embedded cover-art picture from `path` and return it as a
/// base64 `data:` URL (ready for an `<img src>`), or `None` when the file has
/// no artwork or cannot be read. Read on demand for the deck's vinyl disc — the
/// image is never stored, keeping the library DB free of large blobs.
pub fn read_cover_art(path: &str) -> Option<String> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let picture = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())?
        .pictures()
        .first()?;
    let mime = picture
        .mime_type()
        .map(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image/jpeg");
    let encoded = base64::engine::general_purpose::STANDARD.encode(picture.data());
    Some(format!("data:{mime};base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::TrackMetadataUpdate;
    use crate::library::test_audio::write_wav;
    use tempfile::TempDir;

    fn music(dir: &Path) -> ScanRoot {
        ScanRoot {
            content_type: "music",
            path: dir.to_string_lossy().into_owned(),
        }
    }

    fn scan(db: &Db, roots: &[ScanRoot]) -> ScanOutcome {
        scan_all(db, roots, &|| false, |_, _| {}).unwrap()
    }

    /// Titles of the tracks the library shows, sorted. Untagged files take
    /// their file stem as the title.
    fn titles(db: &Db) -> Vec<String> {
        let mut t: Vec<String> = db
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        t.sort();
        t
    }

    fn library() -> (TempDir, Db) {
        (tempfile::tempdir().unwrap(), Db::open_in_memory().unwrap())
    }

    #[test]
    fn scan_adds_every_audio_file_under_a_root() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        write_wav(&dir.path().join("sub/b.wav"), 2, 1);
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();

        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.total, 2);
        assert_eq!(outcome.added, 2);
        assert!(!outcome.canceled);
        assert_eq!(titles(&db), ["a", "b"]);
    }

    #[test]
    fn an_unchanged_file_is_not_reparsed() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        let id = db.search("", None, None, None).unwrap()[0].id;
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some("Edited".into()),
            ..Default::default()
        })
        .unwrap();

        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.added, 0);
        assert_eq!(titles(&db), ["Edited"]);
    }

    #[test]
    fn a_deleted_file_leaves_the_library() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        write_wav(&dir.path().join("b.wav"), 2, 1);
        scan(&db, &[music(dir.path())]);

        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        scan(&db, &[music(dir.path())]);

        assert_eq!(titles(&db), ["b"]);
    }

    #[test]
    fn a_removed_root_leaves_the_library() {
        let (dir, db) = library();
        let (m, j) = (dir.path().join("music"), dir.path().join("jingles"));
        write_wav(&m.join("song.wav"), 1, 1);
        write_wav(&j.join("jingle.wav"), 2, 1);
        let jingles = ScanRoot {
            content_type: "jingle",
            path: j.to_string_lossy().into_owned(),
        };
        scan(&db, &[music(&m), jingles]);
        assert_eq!(db.get_stats().unwrap().tracks_by_type.jingle, 1);

        scan(&db, &[music(&m)]);

        assert_eq!(titles(&db), ["song"]);
    }

    #[test]
    fn a_canceled_scan_removes_nothing() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        std::fs::remove_file(dir.path().join("a.wav")).unwrap();

        let outcome = scan_all(&db, &[music(dir.path())], &|| true, |_, _| {}).unwrap();

        assert!(outcome.canceled);
        assert_eq!(titles(&db), ["a"]);
    }

    #[test]
    fn should_rescan_when_no_existing_row() {
        assert!(should_rescan(None, None, 100, "music"));
    }

    #[test]
    fn should_rescan_when_existing_lacks_mtime() {
        assert!(should_rescan(Some("music"), None, 100, "music"));
    }

    #[test]
    fn should_rescan_when_content_type_changed() {
        assert!(should_rescan(Some("jingle"), Some(100), 100, "music"));
    }

    #[test]
    fn should_rescan_when_mtime_differs() {
        assert!(should_rescan(Some("music"), Some(99), 100, "music"));
    }

    #[test]
    fn should_skip_when_unchanged() {
        assert!(!should_rescan(Some("music"), Some(100), 100, "music"));
    }
}
