//! Writing metadata edits back into the audio file's tags, when the operator
//! has turned it on.
//!
//! Files may live on a network share, so a write never edits the file in
//! place (lofty rewrites a whole file in place for several formats, and a
//! dropped connection would leave it truncated). The file is read into memory,
//! tagged there, written to a sibling temp file and renamed over the original.
//! A tagged copy whose fingerprint differs from the stored one is never
//! written, so a write cannot cost the track its identity.
//!
//! One worker thread drains a queue of track ids; each job reads the row when
//! it runs, so the latest edit wins. A job that outlives the timeout is
//! reported as failed and left behind, since std file I/O cannot be canceled.
//! Failures stay listed in the library health report until a retry succeeds or
//! the operator dismisses them.

use anyhow::{bail, Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::db::{Db, EditedFields, TagValues};
use super::fingerprint;
use super::scanner;
use crate::persist::config::Config;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagWriteFailure {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub path: String,
    pub error: String,
    /// Unix ms.
    pub at: i64,
}

type Listener = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Queue {
    pending: VecDeque<i64>,
    running: bool,
}

pub struct TagWriter {
    db: Arc<Db>,
    config: Arc<Config>,
    queue: Mutex<Queue>,
    failures: Mutex<BTreeMap<i64, TagWriteFailure>>,
    listener: Mutex<Option<Listener>>,
}

impl TagWriter {
    pub fn new(db: Arc<Db>, config: Arc<Config>) -> Arc<Self> {
        Arc::new(Self {
            db,
            config,
            queue: Mutex::new(Queue::default()),
            failures: Mutex::new(BTreeMap::new()),
            listener: Mutex::new(None),
        })
    }

    /// Called whenever the failure list changes.
    pub fn set_listener(&self, listener: impl Fn() + Send + Sync + 'static) {
        *self.listener.lock() = Some(Arc::new(listener));
    }

    pub fn failures(&self) -> Vec<TagWriteFailure> {
        self.failures.lock().values().cloned().collect()
    }

    /// Queue a write of the track's current tags, when write-back is on.
    pub fn request(self: &Arc<Self>, id: i64) {
        if self.config.get_tuning().library.write_tags {
            self.enqueue(id);
        }
    }

    /// Queue a write regardless of the setting: the operator asked for it.
    pub fn retry(self: &Arc<Self>, id: i64) {
        self.enqueue(id);
    }

    /// Forget a failure. The edit stays in the library.
    pub fn dismiss(&self, id: i64) {
        if self.failures.lock().remove(&id).is_some() {
            self.notify();
        }
    }

    fn enqueue(self: &Arc<Self>, id: i64) {
        let mut queue = self.queue.lock();
        if !queue.pending.contains(&id) {
            queue.pending.push_back(id);
        }
        if !queue.running {
            queue.running = true;
            let this = Arc::clone(self);
            std::thread::spawn(move || this.drain());
        }
    }

    fn drain(self: Arc<Self>) {
        loop {
            let id = {
                let mut queue = self.queue.lock();
                match queue.pending.pop_front() {
                    Some(id) => id,
                    None => {
                        queue.running = false;
                        return;
                    }
                }
            };
            self.run(id);
        }
    }

    fn run(&self, id: i64) {
        let values = match self.db.tag_values(id) {
            Ok(Some(values)) => values,
            Ok(None) => return,
            Err(e) => {
                log::error!("tag write: cannot read track {id}: {e:#}");
                return;
            }
        };
        if values.edited_fields == 0 {
            return;
        }
        let timeout = Duration::from_secs(self.config.get_tuning().library.tag_write_timeout_sec);
        let outcome = write_with_timeout(values.clone(), timeout).and_then(|mtime| {
            self.db.finish_tag_write(id, &values, mtime)?;
            Ok(())
        });
        let changed = match outcome {
            Ok(()) => {
                log::info!("tag write: wrote {}", values.path);
                self.failures.lock().remove(&id).is_some()
            }
            Err(e) => {
                log::warn!("tag write: {}: {e:#}", values.path);
                self.failures.lock().insert(
                    id,
                    TagWriteFailure {
                        id,
                        title: values.title.clone().unwrap_or_default(),
                        artist: values.artist.clone().unwrap_or_default(),
                        path: values.path.clone(),
                        error: format!("{e:#}"),
                        at: now_ms(),
                    },
                );
                true
            }
        };
        if changed {
            self.notify();
        }
    }

    fn notify(&self) {
        let listener = self.listener.lock().clone();
        if let Some(listener) = listener {
            listener();
        }
    }
}

/// Run [`write_tags`] on its own thread and give up on it after `timeout`.
/// Returns the file's new mtime.
fn write_with_timeout(values: TagValues, timeout: Duration) -> Result<i64> {
    let (tx, rx) = mpsc::channel();
    let abandoned = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&abandoned);
    std::thread::spawn(move || {
        let _ = tx.send(write_tags(&values, &flag));
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            abandoned.store(true, Ordering::SeqCst);
            bail!("timed out after {} s", timeout.as_secs())
        }
    }
}

/// Write `values` into the file's tags. Returns the file's new mtime.
fn write_tags(values: &TagValues, abandoned: &AtomicBool) -> Result<i64> {
    let path = Path::new(&values.path);
    let bytes = std::fs::read(path).context("read")?;
    let tagged = retag(bytes, values, path)?;

    if let Some(stored) = &values.fingerprint {
        let extension = path.extension().and_then(|e| e.to_str());
        let written = fingerprint::of_source(Box::new(Cursor::new(tagged.clone())), extension)
            .context("fingerprint the tagged copy")?;
        if &written != stored {
            bail!("writing the tags would change the track's audio identity");
        }
    }

    let temp = temp_path(path);
    let result = replace(path, &temp, &tagged, abandoned);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result?;
    mtime_ms(path)
}

/// `bytes` with `values` in its primary tag.
fn retag(bytes: Vec<u8>, values: &TagValues, path: &Path) -> Result<Vec<u8>> {
    let mut probe = Probe::new(Cursor::new(&bytes[..])).guess_file_type()?;
    if probe.file_type().is_none() {
        probe = probe.set_file_type(FileType::from_path(path).context("unknown file type")?);
    }
    let mut file = probe.read().context("read tags")?;
    if file.primary_tag().is_none() {
        let tag_type = file.primary_tag_type();
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.primary_tag_mut().context("no writable tag")?;
    set(tag, ItemKey::TrackTitle, values.title.clone());
    set(tag, ItemKey::TrackArtist, values.artist.clone());
    set(tag, ItemKey::AlbumTitle, values.album.clone());
    set(tag, ItemKey::Genre, values.genre.clone());
    set(tag, ItemKey::Year, values.year.map(|y| y.to_string()));
    // The columns added in #463 are only the file's truth once the row has been
    // read at the current generation. Before that they are `NULL` meaning
    // "nobody has looked", and writing them would delete the file's album
    // artist, track and disc numbers, key and comment — as a side effect of an
    // edit to something else entirely. A column the operator edited is written
    // regardless: that value came from them, not from an unread row.
    let known = values.tags_read_version == Some(scanner::TAG_READ_VERSION);
    let mut write = |bit: i64, key: ItemKey, value: Option<String>| {
        if known || values.edited_fields & bit != 0 {
            set(tag, key, value);
        }
    };
    write(
        EditedFields::ALBUM_ARTIST,
        ItemKey::AlbumArtist,
        values.album_artist.clone(),
    );
    write(
        EditedFields::INITIAL_KEY,
        ItemKey::InitialKey,
        values.initial_key.clone(),
    );
    // Both halves of a pair go together. Writing the number alone would leave
    // the total behind, and a single `TRCK` frame of "3/12" would come back as
    // bare "3".
    write(
        EditedFields::TRACK_NO,
        ItemKey::TrackNumber,
        values.track_no.map(|n| n.to_string()),
    );
    write(
        EditedFields::TRACK_TOTAL,
        ItemKey::TrackTotal,
        values.track_total.map(|n| n.to_string()),
    );
    write(
        EditedFields::DISC_NO,
        ItemKey::DiscNumber,
        values.disc_no.map(|n| n.to_string()),
    );
    write(
        EditedFields::DISC_TOTAL,
        ItemKey::DiscTotal,
        values.disc_total.map(|n| n.to_string()),
    );
    if known || values.edited_fields & EditedFields::COMMENT != 0 {
        set_comment(tag, values.comment.clone());
    }

    let mut out = Cursor::new(bytes);
    file.save_to(&mut out, WriteOptions::default())
        .context("write tags")?;
    Ok(out.into_inner())
}

fn set(tag: &mut Tag, key: ItemKey, value: Option<String>) {
    match value {
        Some(value) if !value.is_empty() => {
            tag.insert_text(key, value);
        }
        _ => tag.remove_key(key),
    }
}

/// Replace the track's own comment, leaving every *described* one alone.
///
/// lofty maps every ID3v2 `COMM` frame onto `ItemKey::Comment`, and
/// `Tag::remove_key` removes all of them — so the ordinary [`set`] here would
/// delete iTunes' `iTunSMPB` alongside the operator's note, and with it the
/// file's gapless-playback data. Only the undescribed entry is ours to touch.
/// On formats that carry a single undescribed comment this does exactly what
/// [`set`] would.
fn set_comment(tag: &mut Tag, value: Option<String>) {
    tag.retain(|item| item.key() != ItemKey::Comment || !item.description().is_empty());
    if let Some(value) = value {
        if !value.is_empty() {
            // `push`, not `insert_text`: the latter is defined as "replacing any
            // existing one of the same key", which would drop the described
            // frames just retained above.
            tag.push(TagItem::new(ItemKey::Comment, ItemValue::Text(value)));
        }
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".rdj-tmp");
    path.with_file_name(name)
}

/// Write `bytes` beside `path` and rename them over it.
fn replace(path: &Path, temp: &Path, bytes: &[u8], abandoned: &AtomicBool) -> Result<()> {
    let permissions = std::fs::metadata(path).context("stat")?.permissions();
    if permissions.readonly() {
        bail!("the file is read-only");
    }
    let mut file = std::fs::File::create(temp).context("create temp file")?;
    file.write_all(bytes).context("write temp file")?;
    file.sync_all().context("flush temp file")?;
    drop(file);
    std::fs::set_permissions(temp, permissions).context("copy permissions")?;
    if abandoned.load(Ordering::SeqCst) {
        bail!("abandoned after the timeout");
    }
    std::fs::rename(temp, path).map_err(|e| rename_error(e, temp, path))
}

/// A rename that reports "not found" while both files are there is the share
/// refusing to rename this file: seen on macOS smbfs with a name another
/// system created. Say so, since the OS error alone reads as a missing file.
fn rename_error(e: std::io::Error, temp: &Path, path: &Path) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound && temp.exists() && path.exists() {
        anyhow::anyhow!(
            "the share could not rename this file, although it is there. Its name \
             may have been created by another system: rename the file on the \
             server, then retry ({e})"
        )
    } else {
        anyhow::Error::new(e).context("replace the file")
    }
}

fn mtime_ms(path: &Path) -> Result<i64> {
    let modified = std::fs::metadata(path).context("stat")?.modified()?;
    Ok(modified.duration_since(UNIX_EPOCH)?.as_millis() as i64)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::{EditedFields, Track, TrackMetadataUpdate};
    use crate::library::listing::{should_rescan, ScanRoot};
    use crate::library::scanner::{read_file_tags, scan_all};
    use crate::library::test_audio::write_wav;
    use std::time::Instant;
    use tempfile::TempDir;

    struct Fixture {
        _dir: TempDir,
        db: Arc<Db>,
        writer: Arc<TagWriter>,
        path: PathBuf,
        id: i64,
    }

    fn fixture(write_tags: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("music/a.wav");
        write_wav(&path, 1, 1);
        let db = Arc::new(Db::open_in_memory().unwrap());
        let root = ScanRoot {
            content_type: "music",
            path: dir.path().join("music").to_string_lossy().into_owned(),
        };
        scan_all(&db, &[root], &|| false, |_, _| {}).unwrap();
        let id = db.search("", None, None, None).unwrap()[0].id;
        let scanned_mtime = db.track_index().unwrap()[0].mtime;
        db.set_fingerprint(id, &fingerprint::of_file(&path).unwrap(), scanned_mtime)
            .unwrap();
        let config = Arc::new(Config::open(dir.path()).unwrap());
        let mut tuning = config.get_tuning();
        tuning.library.write_tags = write_tags;
        config.set_tuning(tuning).unwrap();
        let writer = TagWriter::new(Arc::clone(&db), Arc::clone(&config));
        Fixture {
            _dir: dir,
            db,
            writer,
            path,
            id,
        }
    }

    impl Fixture {
        fn edit(&self, title: &str) -> Track {
            let track = self
                .db
                .update_track_metadata(&TrackMetadataUpdate {
                    id: self.id,
                    title: Some(title.into()),
                    genre: Some(Some("Jazz".into())),
                    ..Default::default()
                })
                .unwrap();
            self.writer.request(self.id);
            track
        }

        fn wait(&self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.writer.queue.lock().running {
                assert!(Instant::now() < deadline, "tag writer did not finish");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn track(&self) -> Track {
            self.db.get_track(self.id).unwrap().unwrap()
        }

        fn stored_mtime(&self) -> Option<i64> {
            self.db.track_index().unwrap()[0].mtime
        }

        fn leftovers(&self) -> Vec<PathBuf> {
            std::fs::read_dir(self.path.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p != &self.path)
                .collect()
        }
    }

    /// One `TRCK` frame carries both halves, so writing the number without the
    /// total would turn "3/12" into a bare "3".
    #[test]
    fn a_written_track_number_keeps_its_total() {
        let f = fixture(true);
        f.db.update_track_metadata(&TrackMetadataUpdate {
            id: f.id,
            track_no: Some(Some(3)),
            track_total: Some(Some(12)),
            disc_no: Some(Some(1)),
            disc_total: Some(Some(2)),
            album_artist: Some(Some("Various".into())),
            initial_key: Some(Some("8A".into())),
            ..Default::default()
        })
        .unwrap();
        f.writer.request(f.id);
        f.wait();

        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(on_disk.track_no, Some(3));
        assert_eq!(on_disk.track_total, Some(12));
        assert_eq!(on_disk.disc_no, Some(1));
        assert_eq!(on_disk.disc_total, Some(2));
        assert_eq!(on_disk.album_artist.as_deref(), Some("Various"));
        assert_eq!(on_disk.initial_key.as_deref(), Some("8A"));
        assert_eq!(f.track().edited_fields, 0, "flags outlived the write");
    }

    /// lofty maps every `COMM` frame onto one key and `Tag::remove_key` removes
    /// all of them, so writing the comment the ordinary way would delete
    /// iTunes' `iTunSMPB` — the file's gapless-playback data — as a side effect
    /// of an unrelated edit.
    #[test]
    fn writing_a_comment_leaves_the_itunes_frames_alone() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;
        use lofty::tag::{ItemValue, Tag, TagItem};

        let f = fixture(true);
        {
            let mut tagged = Probe::open(&f.path).unwrap().read().unwrap();
            let mut tag = Tag::new(tagged.primary_tag_type());
            let mut gapless = TagItem::new(
                ItemKey::Comment,
                ItemValue::Text("00000000 00000840 000002EA".into()),
            );
            gapless.set_description("iTunSMPB".into());
            tag.push(gapless);
            tag.push(TagItem::new(
                ItemKey::Comment,
                ItemValue::Text("original note".into()),
            ));
            tagged.insert_tag(tag);
            tagged
                .save_to_path(&f.path, WriteOptions::default())
                .unwrap();
        }

        f.db.update_track_metadata(&TrackMetadataUpdate {
            id: f.id,
            comment: Some(Some("operator note".into())),
            ..Default::default()
        })
        .unwrap();
        f.writer.request(f.id);
        f.wait();

        let tagged = Probe::open(&f.path).unwrap().read().unwrap();
        let tag = tagged.primary_tag().unwrap();
        let described: Vec<&str> = tag
            .get_items(ItemKey::Comment)
            .filter(|i| i.description() == "iTunSMPB")
            .filter_map(|i| i.value().text())
            .collect();
        assert_eq!(
            described,
            ["00000000 00000840 000002EA"],
            "the gapless frame was destroyed by writing the comment"
        );
        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(on_disk.comment.as_deref(), Some("operator note"));
    }

    /// A file's tags, as `TagValues` for a row in a given read generation.
    fn values_for(path: &std::path::Path, version: Option<i64>, edited: i64) -> TagValues {
        TagValues {
            path: path.to_string_lossy().into_owned(),
            title: Some("Corrected".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            genre: None,
            year: None,
            // Every new column NULL, which is what a row holds before the
            // backfill has read it.
            album_artist: None,
            track_no: None,
            track_total: None,
            disc_no: None,
            disc_total: None,
            initial_key: None,
            comment: None,
            fingerprint: None,
            tags_read_version: version,
            edited_fields: edited,
        }
    }

    fn richly_tagged(path: &std::path::Path) {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;
        use lofty::tag::Tag;

        let mut tagged = Probe::open(path).unwrap().read().unwrap();
        let mut tag = Tag::new(tagged.primary_tag_type());
        tag.insert_text(ItemKey::AlbumArtist, "Kraftwerk".into());
        tag.insert_text(ItemKey::TrackNumber, "3".into());
        tag.insert_text(ItemKey::TrackTotal, "12".into());
        tag.insert_text(ItemKey::InitialKey, "8A".into());
        tag.insert_text(ItemKey::Comment, "sleeve note".into());
        tagged.insert_tag(tag);
        tagged.save_to_path(path, WriteOptions::default()).unwrap();
    }

    /// A row that predates the new columns holds `NULL` for them meaning
    /// "nobody has looked", not "the file has none". Writing those `NULL`s back
    /// would strip the file's album artist, numbers, key and comment as a side
    /// effect of correcting a title — the whole library is in that state
    /// between the migration and the backfill finishing.
    #[test]
    fn a_write_back_keeps_tags_the_row_has_not_read_yet() {
        let f = fixture(true);
        richly_tagged(&f.path);
        let bytes = std::fs::read(&f.path).unwrap();

        let out = retag(bytes, &values_for(&f.path, None, 1), &f.path).unwrap();

        std::fs::write(&f.path, out).unwrap();
        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(
            on_disk.title.as_deref(),
            Some("Corrected"),
            "the edit landed"
        );
        assert_eq!(on_disk.album_artist.as_deref(), Some("Kraftwerk"));
        assert_eq!(on_disk.track_no, Some(3));
        assert_eq!(on_disk.track_total, Some(12));
        assert_eq!(on_disk.initial_key.as_deref(), Some("8A"));
        assert_eq!(on_disk.comment.as_deref(), Some("sleeve note"));
    }

    /// Clearing a box is an instruction, not an absence: an edited column is
    /// written even on a row the backfill has not reached.
    #[test]
    fn a_write_back_clears_a_column_the_operator_edited() {
        let f = fixture(true);
        richly_tagged(&f.path);
        let bytes = std::fs::read(&f.path).unwrap();

        let values = values_for(&f.path, None, EditedFields::ALBUM_ARTIST);
        let out = retag(bytes, &values, &f.path).unwrap();

        std::fs::write(&f.path, out).unwrap();
        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(
            on_disk.album_artist, None,
            "the operator's clear was ignored"
        );
        assert_eq!(on_disk.track_no, Some(3), "an unedited column still stands");
    }

    /// Once the row has been read at the current generation its `NULL` does
    /// mean the file has none, so a write-back may remove the frame.
    #[test]
    fn a_write_back_clears_a_column_a_current_row_says_is_empty() {
        let f = fixture(true);
        richly_tagged(&f.path);
        let bytes = std::fs::read(&f.path).unwrap();

        let values = values_for(&f.path, Some(scanner::TAG_READ_VERSION), 0);
        let out = retag(bytes, &values, &f.path).unwrap();

        std::fs::write(&f.path, out).unwrap();
        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(on_disk.album_artist, None);
        assert_eq!(on_disk.track_no, None);
    }

    /// The rights registry owns the ISRC, so no edit can reach it and a
    /// write-back must leave whatever the file holds untouched.
    #[test]
    fn a_write_back_never_touches_the_isrc() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;
        use lofty::tag::Tag;

        let f = fixture(true);
        {
            let mut tagged = Probe::open(&f.path).unwrap().read().unwrap();
            let mut tag = Tag::new(tagged.primary_tag_type());
            tag.insert_text(ItemKey::Isrc, "FIFIN2400123".into());
            tagged.insert_tag(tag);
            tagged
                .save_to_path(&f.path, WriteOptions::default())
                .unwrap();
        }

        f.edit("Written");
        f.wait();

        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(on_disk.isrc.as_deref(), Some("FIFIN2400123"));
    }

    #[test]
    fn an_edit_lands_in_the_file_and_the_track_keeps_its_identity() {
        let f = fixture(true);
        let fingerprint = fingerprint::of_file(&f.path).unwrap();
        let listener_calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&listener_calls);
        f.writer.set_listener(move || *counter.lock() += 1);

        f.edit("Written");
        f.wait();

        let on_disk = read_file_tags(&f.path.to_string_lossy()).unwrap();
        assert_eq!(on_disk.title.as_deref(), Some("Written"));
        assert_eq!(on_disk.genre.as_deref(), Some("Jazz"));
        assert_eq!(fingerprint::of_file(&f.path).unwrap(), fingerprint);
        assert_eq!(f.track().edited_fields, 0);
        assert_eq!(f.stored_mtime(), on_disk.mtime);
        assert!(
            !should_rescan(
                Some("music"),
                f.stored_mtime(),
                on_disk.mtime.unwrap(),
                "music"
            ),
            "the next scan must not re-read our own write"
        );
        assert!(f.writer.failures().is_empty());
        assert_eq!(*listener_calls.lock(), 0, "nothing to report");
        assert!(f.leftovers().is_empty());
    }

    #[test]
    fn a_failed_write_keeps_the_edit_and_is_reported() {
        let f = fixture(true);
        let before = std::fs::read(&f.path).unwrap();
        let mut permissions = std::fs::metadata(&f.path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&f.path, permissions.clone()).unwrap();
        let notified = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&notified);
        f.writer
            .set_listener(move || flag.store(true, Ordering::SeqCst));

        f.edit("Unwritable");
        f.wait();

        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        let track = f.track();
        assert_eq!(track.title, "Unwritable");
        assert_eq!(
            track.edited_fields,
            EditedFields::TITLE | EditedFields::GENRE
        );
        let failures = f.writer.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].id, f.id);
        assert!(
            failures[0].error.contains("read-only"),
            "{}",
            failures[0].error
        );
        assert!(notified.load(Ordering::SeqCst));
        assert!(f.leftovers().is_empty());

        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&f.path, permissions).unwrap();
        f.writer.retry(f.id);
        f.wait();
        assert!(f.writer.failures().is_empty());
        assert_eq!(f.track().edited_fields, 0);
    }

    #[test]
    fn a_rename_the_share_refuses_is_explained() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.mp3");
        let temp = temp_path(&path);
        let not_found = || std::io::Error::from(std::io::ErrorKind::NotFound);

        let e = rename_error(not_found(), &temp, &path);
        assert!(format!("{e:#}").starts_with("replace the file"), "{e:#}");

        std::fs::write(&path, b"old").unwrap();
        std::fs::write(&temp, b"new").unwrap();
        let e = rename_error(not_found(), &temp, &path);
        assert!(
            format!("{e:#}").contains("rename the file on the server"),
            "{e:#}"
        );

        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let e = rename_error(denied, &temp, &path);
        assert!(format!("{e:#}").starts_with("replace the file"), "{e:#}");
    }

    #[test]
    fn a_missing_file_is_reported() {
        let f = fixture(true);
        std::fs::remove_file(&f.path).unwrap();
        f.edit("Gone");
        f.wait();
        assert_eq!(f.writer.failures().len(), 1);
        assert_eq!(f.track().title, "Gone");

        f.writer.dismiss(f.id);
        assert!(f.writer.failures().is_empty());
        assert_ne!(f.track().edited_fields, 0);
    }

    #[test]
    fn nothing_is_written_while_write_back_is_off() {
        let f = fixture(false);
        let before = std::fs::read(&f.path).unwrap();
        let mtime = f.stored_mtime();

        f.edit("Library only");
        f.wait();

        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        assert_eq!(f.stored_mtime(), mtime);
        assert_eq!(
            f.track().edited_fields,
            EditedFields::TITLE | EditedFields::GENRE
        );
    }

    #[test]
    fn a_copy_that_would_change_identity_is_not_written() {
        let f = fixture(true);
        f.db.set_fingerprint(f.id, "v1:someone-else", f.stored_mtime())
            .unwrap();
        let before = std::fs::read(&f.path).unwrap();

        f.edit("Mismatch");
        f.wait();

        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        assert!(f.writer.failures()[0].error.contains("identity"));
    }
}
