use anyhow::{bail, Context, Result};
use base64::Engine;
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

use super::db::{Db, IndexRow, TrackInsert};
use crate::audio::formats;

/// Upper bound on parallel tag-read workers. A scan is dominated by per-file
/// I/O (stat + header read), which on a networked share is latency-bound —
/// reading several files at once hides that latency. Capped so a scan does not
/// hammer the share.
const SCAN_CONCURRENCY: usize = 4;

/// The audio files found under one root. `complete` is false when part of the
/// tree could not be read, so the listing cannot prove a file is gone.
pub struct Enumeration {
    pub files: Vec<PathBuf>,
    pub complete: bool,
}

/// List the audio files under `dir`, skipping hidden entries. Fails when `dir`
/// itself is not a readable directory — an unmounted share must never look
/// like an empty one.
pub fn find_audio_files(dir: &Path) -> Result<Enumeration> {
    let meta = std::fs::metadata(dir)
        .with_context(|| format!("library path {} is unreachable", dir.display()))?;
    if !meta.is_dir() {
        bail!("library path {} is not a directory", dir.display());
    }
    let mut files = Vec::new();
    let mut complete = true;
    let walk = WalkDir::new(dir)
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
        });
    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::warn!("scan: cannot read under {}: {}", dir.display(), e);
                complete = false;
                continue;
            }
        };
        let is_audio = entry
            .path()
            .extension()
            .and_then(|x| x.to_str())
            .map(formats::is_audio_extension)
            .unwrap_or(false);
        if entry.file_type().is_file() && is_audio {
            files.push(entry.into_path());
        }
    }
    Ok(Enumeration { files, complete })
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
    /// Rows newly marked missing by this scan.
    pub missing: usize,
}

/// A file found on disk, with the content type of the root it was found under.
struct Found {
    path: String,
    content_type: &'static str,
}

/// Scan every root and bring the library in line with what is on disk.
/// `on_progress` receives `(processed, total)` counted across all roots.
///
/// A row whose file is gone is marked missing, never deleted — and only when a
/// root that contains it was listed completely, or when no configured root
/// contains it at all. Roots are matched by path
/// component, so `/Music` does not contain `/Music2`.
pub fn scan_all(
    db: &Db,
    roots: &[ScanRoot],
    cancel: &(impl Fn() -> bool + Sync),
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<ScanOutcome> {
    let mut found: Vec<Found> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut listed: Vec<&Path> = Vec::new();
    for root in roots {
        match find_audio_files(Path::new(&root.path)) {
            Ok(listing) => {
                if listing.complete {
                    listed.push(Path::new(&root.path));
                } else {
                    log::warn!("scan: {} was listed partially; not pruning it", root.path);
                }
                for file in listing.files {
                    let path = file.to_string_lossy().into_owned();
                    if seen.insert(path.clone()) {
                        found.push(Found {
                            path,
                            content_type: root.content_type,
                        });
                    }
                }
            }
            Err(e) => log::warn!("scan: {e:#}; keeping its tracks"),
        }
    }

    let index = db.track_index()?;
    let mut known: HashMap<&str, &IndexRow> = index
        .iter()
        .filter(|r| r.missing_since.is_none())
        .map(|r| (r.path.as_str(), r))
        .collect();
    // A missing row whose file is back at the same path is the same track —
    // the remove-and-re-add case. The newest one wins if several share it.
    let mut returned: HashMap<&str, &IndexRow> = HashMap::new();
    for row in index.iter().filter(|r| r.missing_since.is_some()) {
        if seen.contains(&row.path) && !known.contains_key(row.path.as_str()) {
            let slot = returned.entry(row.path.as_str()).or_insert(row);
            if (row.missing_since, row.id) > (slot.missing_since, slot.id) {
                *slot = row;
            }
        }
    }
    db.revive(&returned.values().map(|r| r.id).collect::<Vec<_>>())?;
    known.extend(returned);

    let parsed = parse_changed(&found, &known, cancel, &on_progress);
    let mut outcome = ScanOutcome {
        total: found.len(),
        added: parsed.len(),
        canceled: cancel(),
        missing: 0,
    };
    db.insert_tracks(&parsed)?;
    if outcome.canceled {
        return Ok(outcome);
    }

    let gone: Vec<i64> = index
        .iter()
        .filter(|row| row.missing_since.is_none() && !seen.contains(&row.path))
        .filter(|row| {
            let path = Path::new(&row.path);
            let configured = roots.iter().any(|r| path.starts_with(&r.path));
            !configured || listed.iter().any(|root| path.starts_with(root))
        })
        .map(|row| row.id)
        .collect();
    outcome.missing = db.mark_missing(&gone, now_ms())?;
    if outcome.missing > 0 {
        log::info!("scan marked {} tracks missing", outcome.missing);
    }
    Ok(outcome)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Tag-read every found file whose row is absent or out of date. Tag reads are
/// I/O-bound, so the files fan out across a small pool; each worker claims the
/// next index via `next`. The rows are returned for one batched upsert — a
/// per-row autocommit is the other thing that makes a big scan slow.
fn parse_changed(
    found: &[Found],
    existing: &HashMap<&str, &IndexRow>,
    cancel: &(impl Fn() -> bool + Sync),
    on_progress: &(impl Fn(usize, usize) + Sync),
) -> Vec<TrackInsert> {
    let total = found.len();
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
                let Some(file) = found.get(i) else {
                    break;
                };

                let mtime_ms = std::fs::metadata(&file.path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                let prev = existing.get(file.path.as_str());
                let need = should_rescan(
                    prev.map(|r| r.content_type.as_str()),
                    prev.and_then(|r| r.mtime),
                    mtime_ms,
                    file.content_type,
                );
                if need {
                    match parse_track(&file.path, file.content_type, mtime_ms) {
                        Ok(track) => pending.lock().push(track),
                        Err(e) => log::error!("scan: failed to parse {}: {}", file.path, e),
                    }
                }
                let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(done, total);
            });
        }
    });
    pending.into_inner()
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
    use crate::audio::cue_points::CuePoints;
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
    fn an_unreachable_root_keeps_its_tracks() {
        let (dir, db) = library();
        let root = dir.path().join("share");
        write_wav(&root.join("a.wav"), 1, 1);
        scan(&db, &[music(&root)]);

        std::fs::rename(&root, dir.path().join("unmounted")).unwrap();
        let outcome = scan(&db, &[music(&root)]);

        assert_eq!(outcome.total, 0);
        assert_eq!(titles(&db), ["a"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_partially_readable_root_keeps_its_tracks() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        write_wav(&dir.path().join("locked/b.wav"), 2, 1);
        scan(&db, &[music(dir.path())]);

        let locked = dir.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        scan(&db, &[music(dir.path())]);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(titles(&db), ["a", "b"]);
    }

    #[test]
    fn a_root_does_not_prune_a_sibling_sharing_its_prefix() {
        let (dir, db) = library();
        let (short, long) = (dir.path().join("Music"), dir.path().join("Music2"));
        write_wav(&short.join("a.wav"), 1, 1);
        write_wav(&long.join("b.wav"), 2, 1);

        scan(&db, &[music(&short), music(&long)]);
        scan(&db, &[music(&short), music(&long)]);

        assert_eq!(titles(&db), ["a", "b"]);
    }

    #[test]
    fn a_file_listed_as_a_root_is_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        assert!(find_audio_files(&file).is_err());
    }

    fn id_of(db: &Db, title: &str) -> i64 {
        db.search("", None, None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.title == title)
            .unwrap_or_else(|| panic!("{title} is not in the library"))
            .id
    }

    fn missing_since(db: &Db, id: i64) -> Option<i64> {
        db.track_index()
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .expect("row kept")
            .missing_since
    }

    /// Operator work that must outlive a file's absence.
    fn prepare(db: &Db, id: i64) {
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        db.increment_play_count(id).unwrap();
    }

    fn assert_prepared(db: &Db, id: i64) {
        let track = db.get_track(id).unwrap().expect("row kept");
        assert_eq!(track.cue_points.cue_in_ms, Some(100));
        assert_eq!(track.play_count, 1);
    }

    #[test]
    fn a_deleted_file_is_marked_missing_with_its_work_kept() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        let id = id_of(&db, "a");
        prepare(&db, id);

        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.missing, 1);
        let since = missing_since(&db, id).expect("marked missing");
        assert_prepared(&db, id);

        scan(&db, &[music(dir.path())]);
        assert_eq!(missing_since(&db, id), Some(since), "first timestamp kept");
    }

    #[test]
    fn a_file_back_at_its_path_is_the_same_track() {
        let (dir, db) = library();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        scan(&db, &[music(dir.path())]);
        let id = id_of(&db, "a");
        prepare(&db, id);
        let aside = dir.path().join(".aside.wav");
        std::fs::rename(&file, &aside).unwrap();
        scan(&db, &[music(dir.path())]);

        std::fs::rename(&aside, &file).unwrap();
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.added, 0, "unchanged file is not reparsed");
        assert_eq!(id_of(&db, "a"), id);
        assert_eq!(missing_since(&db, id), None);
        assert_prepared(&db, id);
    }

    #[test]
    fn removing_and_re_adding_a_root_keeps_its_tracks() {
        let (dir, db) = library();
        let (m, j) = (dir.path().join("music"), dir.path().join("jingles"));
        write_wav(&m.join("song.wav"), 1, 1);
        write_wav(&j.join("jingle.wav"), 2, 1);
        let jingles = || ScanRoot {
            content_type: "jingle",
            path: j.to_string_lossy().into_owned(),
        };
        scan(&db, &[music(&m), jingles()]);
        let id = id_of(&db, "jingle");
        prepare(&db, id);

        scan(&db, &[music(&m)]);
        assert!(missing_since(&db, id).is_some());
        assert_eq!(db.get_stats().unwrap().tracks_by_type.jingle, 0);

        scan(&db, &[music(&m), jingles()]);
        assert_eq!(id_of(&db, "jingle"), id);
        assert_eq!(db.get_stats().unwrap().tracks_by_type.jingle, 1);
        assert_prepared(&db, id);
    }

    #[test]
    fn removing_every_root_hides_everything_and_deletes_nothing() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        let id = id_of(&db, "a");

        scan(&db, &[]);

        assert!(titles(&db).is_empty());
        assert!(missing_since(&db, id).is_some());
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
