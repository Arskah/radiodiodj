use anyhow::Result;
use base64::Engine;
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use super::db::{Db, IndexRow, Reconcile, TrackInsert};
use super::fingerprint;
pub use super::listing::ScanRoot;
use super::listing::{self, Found};
#[cfg(test)]
use super::listing::{find_audio_files, should_rescan};

/// Upper bound on parallel tag-read workers. A scan is dominated by per-file
/// I/O (stat + header read), which on a networked share is latency-bound —
/// reading several files at once hides that latency. Capped so a scan does not
/// hammer the share.
const SCAN_CONCURRENCY: usize = 4;

/// What a complete tag read stores today, recorded per row in
/// `tracks.tags_read_version`. Bump it whenever a tag-derived column is added:
/// every row falls back into `library::tag_backfill`'s queue and the background
/// pass fills the new column in, without a rescan and without the file's mtime
/// having to change. `NULL` in the column is version 0 — a row written before
/// the column existed.
pub const TAG_READ_VERSION: i64 = 1;

#[derive(Debug, Default, PartialEq)]
pub struct ScanOutcome {
    pub total: usize,
    pub added: usize,
    pub canceled: bool,
    /// Rows newly marked missing by this scan.
    pub missing: usize,
    /// Moved files matched back to their missing rows.
    pub reattached: usize,
    /// Rows whose own path turned out to hold a different recording. They are
    /// missing too, counted apart because the file is still there.
    pub replaced: usize,
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
    let listing = listing::list_roots(roots);
    let seen = &listing.seen;

    let index = db.track_index()?;
    let present: HashMap<&str, &IndexRow> = index
        .iter()
        .filter(|r| r.missing_since.is_none())
        .map(|r| (r.path.as_str(), r))
        .collect();
    // The newest missing row at each path that has a file again.
    let mut missing_at: HashMap<&str, &IndexRow> = HashMap::new();
    for row in index.iter().filter(|r| r.missing_since.is_some()) {
        if seen.contains(&row.path) && !present.contains_key(row.path.as_str()) {
            let slot = missing_at.entry(row.path.as_str()).or_insert(row);
            if (row.missing_since, row.id) > (slot.missing_since, slot.id) {
                *slot = row;
            }
        }
    }
    let known = Known {
        present,
        missing_at,
        // A first scan has nothing to match against; the background pass
        // fingerprints it without slowing the scan down.
        fingerprint_new: !index.is_empty(),
    };

    let steps = inspect_all(&listing.found, &known, cancel, &on_progress);
    let canceled = cancel();
    let mut change = Reconcile {
        now_ms: now_ms(),
        ..Default::default()
    };
    for step in steps {
        match step {
            Step::Update(t) => change.upserts.push(t),
            Step::Revive { id, update } => {
                change.revive.push(id);
                change.upserts.extend(update);
            }
            // Committing a new file without first marking what is gone could
            // mint a duplicate of a file that merely moved, so a canceled scan
            // leaves new files for the next one.
            Step::New(t) if !canceled => change.new_files.push(t),
            Step::New(_) => {}
            // A replacement is half a new file and inherits its rule: the row
            // is only retired once the file taking its place is committed.
            Step::Replaced { id, new } if !canceled => {
                change.replaced.push(id);
                change.new_files.push(new);
            }
            Step::Replaced { .. } => {}
        }
    }
    if !canceled {
        change.gone = listing.gone(&index, roots).map(|row| row.id).collect();
    }

    let updated = change.upserts.len();
    let done = db.reconcile(&change)?;
    if done.missing + done.reattached + done.duplicated + done.replaced > 0 {
        log::info!(
            "scan: {} missing, {} reattached, {} duplicated, {} replaced",
            done.missing,
            done.reattached,
            done.duplicated,
            done.replaced
        );
    }
    Ok(ScanOutcome {
        total: listing.found.len(),
        added: updated + done.inserted + done.duplicated,
        canceled,
        missing: done.missing,
        reattached: done.reattached,
        replaced: done.replaced,
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// What the library holds, as the per-file workers need it.
struct Known<'a> {
    present: HashMap<&'a str, &'a IndexRow>,
    missing_at: HashMap<&'a str, &'a IndexRow>,
    fingerprint_new: bool,
}

/// What one found file asks of the library.
enum Step {
    /// A known path whose file changed, carrying the same audio as before or
    /// audio that cannot be compared.
    Update(TrackInsert),
    /// A known path whose file is now a different recording. The row it
    /// belonged to goes missing and the file enters as a track of its own.
    Replaced { id: i64, new: TrackInsert },
    /// A missing row whose file is back at its path — the same audio, or audio
    /// that cannot be compared. `update` carries new tags if the file changed.
    Revive {
        id: i64,
        update: Option<TrackInsert>,
    },
    /// A path the library does not hold.
    New(TrackInsert),
}

/// Inspect every found file. The work is I/O-bound, so the files fan out
/// across a small pool; each worker claims the next index via `next`. The
/// resulting rows are applied in one transaction — a per-row autocommit is the
/// other thing that makes a big scan slow.
fn inspect_all(
    found: &[Found],
    known: &Known,
    cancel: &(impl Fn() -> bool + Sync),
    on_progress: &(impl Fn(usize, usize) + Sync),
) -> Vec<Step> {
    let total = found.len();
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let steps: Mutex<Vec<Step>> = Mutex::new(Vec::new());
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
                if let Some(step) = inspect(file, known) {
                    steps.lock().push(step);
                }
                let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(done, total);
            });
        }
    });
    steps.into_inner()
}

fn inspect(file: &Found, known: &Known) -> Option<Step> {
    let mtime_ms = file.mtime_ms();
    let changed = |row: &IndexRow| file.changed(row, mtime_ms);
    let parse = |fingerprint: Option<String>| match parse_track(
        &file.path,
        file.content_type,
        mtime_ms,
        fingerprint,
    ) {
        Ok(track) => Some(track),
        Err(e) => {
            log::error!("scan: failed to parse {}: {}", file.path, e);
            None
        }
    };

    if let Some(row) = known.present.get(file.path.as_str()) {
        if !changed(row) {
            return None;
        }
        let fingerprint = fingerprint_of(&file.path);
        let track = parse(fingerprint.clone())?;
        // The same test the revive branch below applies, and for the same
        // reason: a row is only reused for audio it recognises. A fingerprint
        // that is absent on either side is not evidence, so it reads as the
        // same audio — a row is never retired on a guess.
        let replaced = matches!((&row.fingerprint, &fingerprint), (Some(a), Some(b)) if a != b);
        return Some(if replaced {
            Step::Replaced {
                id: row.id,
                new: track,
            }
        } else {
            Step::Update(track)
        });
    }

    let returning = known.missing_at.get(file.path.as_str());
    let fingerprint = (known.fingerprint_new || returning.is_some())
        .then(|| fingerprint_of(&file.path))
        .flatten();
    if let Some(row) = returning {
        let comparable = row.fingerprint.is_some() && fingerprint.is_some();
        if !comparable || row.fingerprint == fingerprint {
            return Some(Step::Revive {
                id: row.id,
                update: changed(row).then(|| parse(fingerprint)).flatten(),
            });
        }
    }
    parse(fingerprint).map(Step::New)
}

fn fingerprint_of(path: &str) -> Option<String> {
    fingerprint::of_file(Path::new(path))
        .map_err(|e| log::warn!("scan: cannot fingerprint {path}: {e:#}"))
        .ok()
}

fn parse_track(
    path: &str,
    content_type: &str,
    mtime_ms: i64,
    fingerprint: Option<String>,
) -> Result<TrackInsert> {
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
    let album_artist = primary.and_then(|t| t.get_string(ItemKey::AlbumArtist).map(str::to_string));
    let isrc = primary.and_then(|t| t.get_string(ItemKey::Isrc).map(str::to_string));
    let initial_key = primary.and_then(|t| t.get_string(ItemKey::InitialKey).map(str::to_string));
    let comment = primary.and_then(comment_of);
    // `Accessor` splits the pair forms — a single ID3v2 `TRCK` of "3/12" is two
    // columns here — so the number and its total both survive a write-back.
    let track_no = primary.and_then(Accessor::track).map(i64::from);
    let track_total = primary.and_then(Accessor::track_total).map(i64::from);
    let disc_no = primary.and_then(Accessor::disk).map(i64::from);
    let disc_total = primary.and_then(Accessor::disk_total).map(i64::from);

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
        album_artist,
        track_no,
        track_total,
        disc_no,
        disc_total,
        isrc,
        initial_key,
        comment,
        fingerprint,
    })
}

/// The track's own comment, which is not simply `ItemKey::Comment`.
///
/// lofty maps *every* ID3v2 `COMM` frame onto that one key, keeping the frame's
/// description only when it is non-empty, and `get_string` returns whichever
/// came first. On an iTunes-processed file that is usually `COMM:iTunNORM` or
/// `COMM:iTunSMPB` — a hex blob, not a comment. The operator's comment is the
/// one with an empty descriptor. Other formats carry a single undescribed
/// comment, where this picks the same item `get_string` would.
///
/// Stored whole, never truncated: the write-back in `tag_write` sends this
/// value to the file, so a shortened copy would delete the rest of the
/// operator's text. Display does the shortening.
fn comment_of(tag: &lofty::tag::Tag) -> Option<String> {
    tag.get_items(ItemKey::Comment)
        .find(|item| item.description().is_empty())
        .and_then(|item| item.value().text())
        .map(str::to_string)
}

/// The tags `path` holds now, as a scan would store them.
pub fn read_file_tags(path: &str) -> Result<TrackInsert> {
    let mtime = listing::Found {
        path: path.to_string(),
        content_type: "music",
    }
    .mtime_ms();
    parse_track(path, "music", mtime, None)
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
    use crate::library::test_audio::{retag_externally, write_wav};
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

    /// lofty maps every ID3v2 `COMM` frame onto `ItemKey::Comment`, so
    /// `get_string` hands back whichever frame came first. On an
    /// iTunes-processed file that is a hex blob like `iTunNORM`, not a comment.
    #[test]
    fn the_comment_is_the_one_without_a_descriptor() {
        use lofty::tag::{ItemValue, Tag, TagItem, TagType};

        let mut tag = Tag::new(TagType::Id3v2);
        let mut itunes = TagItem::new(ItemKey::Comment, ItemValue::Text("0000A1B2 0000".into()));
        itunes.set_description("iTunNORM".into());
        tag.push(itunes);
        tag.push(TagItem::new(
            ItemKey::Comment,
            ItemValue::Text("the operator's note".into()),
        ));

        assert_eq!(comment_of(&tag).as_deref(), Some("the operator's note"));
    }

    /// A file with only described comments has none of its own — better empty
    /// than a hex blob shown to the operator as their note.
    #[test]
    fn a_described_only_comment_reads_as_none() {
        use lofty::tag::{ItemValue, Tag, TagItem, TagType};

        let mut tag = Tag::new(TagType::Id3v2);
        let mut itunes = TagItem::new(ItemKey::Comment, ItemValue::Text("0000A1B2".into()));
        itunes.set_description("iTunSMPB".into());
        tag.push(itunes);

        assert_eq!(comment_of(&tag), None);
    }

    /// One tag field, two columns. The pair forms (`TRCK` of "3/12") are split
    /// by `Accessor`, so the total survives to be written back.
    #[test]
    fn a_track_number_pair_is_read_as_a_number_and_a_total() {
        use lofty::tag::{Tag, TagType};

        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::TrackNumber, "3".into());
        tag.insert_text(ItemKey::TrackTotal, "12".into());

        assert_eq!(tag.track(), Some(3));
        assert_eq!(tag.track_total(), Some(12));
    }

    /// The scan reads the tag columns into the row, and stamps the generation
    /// of the read so the backfill knows to leave the row alone.
    #[test]
    fn a_scan_stores_the_extra_tag_columns() {
        let (dir, db) = library();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 1);
        // The WAV's primary tag carries the track number, its total and the
        // comment, but has no key for album artist, disc, ISRC or musical key —
        // those are covered at the tag level above rather than through a file.
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::tag::Tag;

            let mut tagged = Probe::open(&path).unwrap().read().unwrap();
            let mut tag = Tag::new(tagged.primary_tag_type());
            tag.insert_text(ItemKey::TrackTitle, "Autobahn".into());
            tag.insert_text(ItemKey::TrackNumber, "3".into());
            tag.insert_text(ItemKey::TrackTotal, "12".into());
            tag.insert_text(ItemKey::Comment, "a note".into());
            tagged.insert_tag(tag);
            tagged.save_to_path(&path, WriteOptions::default()).unwrap();
        }

        scan(&db, &[music(dir.path())]);

        let track = &db.search("", None, None, None).unwrap()[0];
        assert_eq!(track.track_no, Some(3));
        assert_eq!(track.track_total, Some(12));
        assert_eq!(track.comment.as_deref(), Some("a note"));
        let parsed = read_file_tags(&path.to_string_lossy()).unwrap();
        assert_eq!(parsed.track_no, Some(3));
        assert_eq!(parsed.comment.as_deref(), Some("a note"));
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
    fn an_edited_field_survives_an_external_retag() {
        let (dir, db) = library();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 1);
        scan(&db, &[music(dir.path())]);
        let id = db.search("", None, None, None).unwrap()[0].id;
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some("Edited".into()),
            ..Default::default()
        })
        .unwrap();

        retag_externally(&path, "Retagged", "Tagger");
        scan(&db, &[music(dir.path())]);

        let track = db.get_track(id).unwrap().unwrap();
        assert_eq!(track.title, "Edited");
        assert_eq!(track.artist, "Tagger");
    }

    #[test]
    fn reading_file_tags_sees_the_current_file() {
        let (dir, _db) = library();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 1);
        retag_externally(&path, "On Disk", "Tagger");
        let tags = read_file_tags(&path.to_string_lossy()).unwrap();
        assert_eq!(tags.title.as_deref(), Some("On Disk"));
        assert_eq!(
            tags.mtime,
            Some(
                Found {
                    path: path.to_string_lossy().into_owned(),
                    content_type: "music",
                }
                .mtime_ms()
            )
        );
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

    /// What the background pass does after a first scan.
    fn backfill(db: &Db) {
        for job in db.tracks_needing_analysis().unwrap() {
            let fp = fingerprint::of_file(Path::new(&job.path)).unwrap();
            assert!(db.set_fingerprint(job.id, &fp, job.mtime).unwrap());
        }
    }

    /// The modification time the library holds for a row — what the analysis
    /// pass's stores are checked against.
    fn row_mtime(db: &Db, id: i64) -> Option<i64> {
        db.track_index()
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .expect("row kept")
            .mtime
    }

    /// A prepared track with a waveform and an in-app title edit.
    fn prepare_fully(db: &Db, id: i64) {
        prepare(db, id);
        db.set_waveform(id, &[1, 2, 3], row_mtime(db, id)).unwrap();
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some("Edited".into()),
            ..Default::default()
        })
        .unwrap();
    }

    fn assert_fully_prepared(db: &Db, id: i64) {
        assert_prepared(db, id);
        assert_eq!(db.get_track(id).unwrap().unwrap().title, "Edited");
        assert_eq!(db.get_waveform(id).unwrap(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn a_first_scan_leaves_fingerprints_to_the_background_pass() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        assert!(db.track_index().unwrap()[0].fingerprint.is_none());

        write_wav(&dir.path().join("b.wav"), 2, 1);
        scan(&db, &[music(dir.path())]);
        let b = id_of(&db, "b");
        let index = db.track_index().unwrap();
        let row = index.iter().find(|r| r.id == b).unwrap();
        assert!(row.fingerprint.is_some(), "later files are fingerprinted");
    }

    #[test]
    fn a_renamed_file_keeps_its_track() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");
        prepare_fully(&db, id);

        std::fs::rename(dir.path().join("a.wav"), dir.path().join("sub-a.wav")).unwrap();
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.reattached, 1);
        assert_eq!(outcome.missing, 1, "the old path went missing first");
        assert_eq!(titles(&db), ["Edited"]);
        assert_eq!(id_of(&db, "Edited"), id);
        assert_fully_prepared(&db, id);
        let (path, missing) = {
            let index = db.track_index().unwrap();
            let row = index.into_iter().find(|r| r.id == id).unwrap();
            (row.path, row.missing_since)
        };
        assert!(path.ends_with("sub-a.wav"), "{path}");
        assert_eq!(missing, None);
    }

    #[test]
    fn a_file_moved_to_another_root_keeps_its_track_and_takes_the_roots_type() {
        let (dir, db) = library();
        let (m, j) = (dir.path().join("music"), dir.path().join("jingles"));
        write_wav(&m.join("a.wav"), 1, 1);
        std::fs::create_dir_all(&j).unwrap();
        let roots = || {
            [
                music(&m),
                ScanRoot {
                    content_type: "jingle",
                    path: j.to_string_lossy().into_owned(),
                },
            ]
        };
        scan(&db, &roots());
        backfill(&db);
        let id = id_of(&db, "a");
        prepare_fully(&db, id);

        std::fs::rename(m.join("a.wav"), j.join("a.wav")).unwrap();
        scan(&db, &roots());

        assert_eq!(id_of(&db, "Edited"), id);
        assert_eq!(db.get_stats().unwrap().tracks_by_type.jingle, 1);
        assert_eq!(db.get_stats().unwrap().tracks_by_type.music, 0);
        assert_fully_prepared(&db, id);
    }

    #[test]
    fn a_duplicate_copy_starts_with_the_originals_work() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");
        prepare_fully(&db, id);

        write_wav(&dir.path().join("copy/a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);

        let tracks = db.search("", None, None, None).unwrap();
        assert_eq!(tracks.len(), 2);
        let copy = tracks.iter().find(|t| t.id != id).unwrap().id;
        assert_fully_prepared(&db, id);
        assert_fully_prepared(&db, copy);
    }

    #[test]
    fn a_moved_and_re_encoded_file_is_a_new_track() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");

        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        write_wav(&dir.path().join("b.wav"), 2, 1);
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.reattached, 0);
        assert_ne!(id_of(&db, "b"), id);
        assert!(missing_since(&db, id).is_some());
    }

    #[test]
    fn different_audio_at_a_missing_tracks_path_is_a_new_track() {
        let (dir, db) = library();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");
        prepare(&db, id);
        std::fs::remove_file(&file).unwrap();
        scan(&db, &[music(dir.path())]);

        write_wav(&file, 2, 1);
        scan(&db, &[music(dir.path())]);

        let tracks = db.search("", None, None, None).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_ne!(tracks[0].id, id);
        assert_eq!(tracks[0].play_count, 0);
        assert!(missing_since(&db, id).is_some());
        assert_prepared(&db, id);
    }

    /// Push a file's modification time a minute forward, so the delta cache
    /// sees it as changed. Rewriting a file in a test is far quicker than the
    /// timestamp resolution the scan compares against.
    fn touch(path: &Path) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let at = file.metadata().unwrap().modified().unwrap() + std::time::Duration::from_secs(60);
        file.set_modified(at).unwrap();
    }

    /// The same rule as `different_audio_at_a_missing_tracks_path_is_a_new_track`,
    /// for a path the library still holds. A file overwritten in place is the
    /// only way the two can differ, and the row is reused for audio it
    /// recognises or not at all.
    #[test]
    fn different_audio_at_a_present_tracks_path_is_a_new_track() {
        let (dir, db) = library();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");
        prepare_fully(&db, id);

        write_wav(&file, 2, 1);
        touch(&file);
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.replaced, 1);
        assert_eq!(outcome.missing, 0, "nothing went away — the file is there");
        let tracks = db.search("", None, None, None).unwrap();
        assert_eq!(tracks.len(), 1, "the replaced row is hidden");
        assert_ne!(tracks[0].id, id);
        assert_eq!(tracks[0].play_count, 0, "the new track starts clean");
        assert_eq!(tracks[0].cue_points.cue_in_ms, None);
        assert!(missing_since(&db, id).is_some());
        assert_fully_prepared(&db, id);
    }

    /// An external tagger rewrites the file whole, which moves its
    /// modification time without touching a sample. The track stands, and so
    /// does every measurement taken from the audio — re-deriving them would be
    /// a full decode per track, for a file whose audio nobody touched.
    #[test]
    fn a_tag_edit_in_another_app_keeps_the_track_and_its_measurements() {
        let (dir, db) = library();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");
        db.set_waveform(id, &[1, 2, 3], row_mtime(&db, id)).unwrap();
        db.set_loudness(id, Some(-7.5), Some(0.9), 1234, row_mtime(&db, id))
            .unwrap();
        prepare(&db, id);

        retag_externally(&file, "Retagged", "Tagger");
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.replaced, 0);
        assert_eq!(id_of(&db, "Retagged"), id, "the tags were read");
        assert_prepared(&db, id);
        assert_eq!(db.get_waveform(id).unwrap(), Some(vec![1, 2, 3]));
        assert!(
            db.tracks_needing_analysis()
                .unwrap()
                .iter()
                .all(|j| j.id != id),
            "a tag edit sent the track back through a full decode"
        );
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
