use anyhow::{Context, Result};
use base64::Engine;
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use super::db::{Db, IndexRow, Reconcile, TrackInsert};
pub use super::listing::ScanRoot;
use super::listing::{self, Found};
#[cfg(test)]
use super::listing::{find_audio_files, should_rescan};
use crate::audio_measure::fingerprint;

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

/// Whether a scan may retire rows whose file it did not find.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Missing {
    /// Mark them missing, as the operator's _Scan Library Now_ does.
    Mark,
    /// Retire a row only where this scan can see the same audio arriving
    /// somewhere else — a moved file, matched by fingerprint. Absence alone
    /// retires nothing.
    ///
    /// What an **automatic** scan does. Adding and updating on the strength of
    /// a listing nobody watched is recoverable, but retiring a library because
    /// a share came back as an empty directory is the kind of thing that
    /// should need a person. A move still has to be retired in the same
    /// transaction as the file taking its place, or the scan mints a duplicate
    /// of a file that merely moved. The check still reports whatever is left,
    /// and the operator's button still applies it.
    OnlyMoved,
}

/// Scan every root and bring the library in line with what is on disk.
/// `on_progress` receives `(processed, total)` counted across all roots.
///
/// A row whose file is gone is marked missing, never deleted — and only when a
/// root that contains it was listed completely, or when no configured root
/// contains it at all, and only under [`Missing::Mark`]. Roots are matched by
/// path component, so `/Music` does not contain `/Music2`.
pub fn scan_all(
    db: &Db,
    roots: &[ScanRoot],
    missing: Missing,
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
        change.gone = match missing {
            Missing::Mark => listing.gone(&index, roots).map(|row| row.id).collect(),
            // The fingerprints this scan is about to commit. A gone row whose
            // audio is among them moved, and reconcile reattaches it once the
            // new file lands; anything else is merely absent, and absence is
            // not evidence enough to retire a track unattended.
            Missing::OnlyMoved => {
                let arriving: HashSet<&str> = change
                    .new_files
                    .iter()
                    .filter_map(|t| t.fingerprint.as_deref())
                    .collect();
                listing
                    .gone(&index, roots)
                    .filter(|row| {
                        row.fingerprint
                            .as_deref()
                            .is_some_and(|f| arriving.contains(f))
                    })
                    .map(|row| row.id)
                    .collect()
            }
        };
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
    match parse(fingerprint.clone()) {
        Some(track) => Some(Step::New(track)),
        // Only for a file that fingerprinted: its head came off the share and
        // demuxed, so the tag read failed on what the file holds rather than on
        // reaching it. A share that flaked returns nothing here and the file is
        // left for the next scan, exactly as before.
        None => fingerprint
            .map(|fp| untagged_track(&file.path, file.content_type, mtime_ms, Some(fp)))
            .map(Step::New),
    }
}

/// The row a file enters the library as when its tags cannot be read: named
/// after its file, carrying nothing the tags would have said.
///
/// Left out instead, it would have no row at all, so the library check would
/// report it as new on every pass — and with _Scan when files change_ on, start
/// a scan that fails the same way, for ever.
fn untagged_track(
    path: &str,
    content_type: &str,
    mtime_ms: i64,
    fingerprint: Option<String>,
) -> TrackInsert {
    let p = Path::new(path);
    TrackInsert {
        path: path.to_string(),
        content_type: content_type.to_string(),
        title: Some(
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
        ),
        artist: Some("Unknown".into()),
        album: Some("Unknown".into()),
        genre: None,
        year: None,
        duration: None,
        bpm: None,
        sample_rate: None,
        bitrate: None,
        format: p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase()),
        mtime: Some(mtime_ms),
        album_artist: None,
        track_no: None,
        track_total: None,
        disc_no: None,
        disc_total: None,
        isrc: None,
        initial_key: None,
        comment: None,
        fingerprint,
    }
}

fn fingerprint_of(path: &str) -> Option<String> {
    fingerprint::of_file(Path::new(path))
        .map_err(|e| log::warn!("scan: cannot fingerprint {path}: {e:#}"))
        .ok()
}

/// The tags `path` holds, read by whichever reader can make sense of the file.
///
/// lofty first — not because it reads more, but because it is the only one that
/// reports a bitrate, and leading with symphonia would empty that column for
/// every track. symphonia is the rescue, for a container lofty cannot identify:
/// it picks its reader by sniffing the bytes rather than trusting the extension,
/// which is what reads an Ogg whose first logical stream is Theora.
///
/// See `docs/library.md#reading-tags`.
fn parse_track(
    path: &str,
    content_type: &str,
    mtime_ms: i64,
    fingerprint: Option<String>,
) -> Result<TrackInsert> {
    match parse_track_lofty(path, content_type, mtime_ms, fingerprint.clone()) {
        Ok(track) => Ok(track),
        Err(e) => {
            let track = parse_track_symphonia(path, content_type, mtime_ms, fingerprint)?;
            log::info!("scan: {path} read by symphonia; lofty refused it: {e}");
            Ok(track)
        }
    }
}

fn parse_track_lofty(
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
    // Two keys, because lofty splits them by precision and a format carries one
    // or the other: `Bpm` is the decimal field (Vorbis `BPM`, MP4 iTunes), while
    // ID3v2's `TBPM` is integer-only and therefore `IntegerBpm`. Reading only the
    // first meant an MP3's tempo tag was never read at all.
    //
    // A tagger that writes `0` means "no tempo here", and 46 of 47 tagged files in
    // one sampled library said exactly that. Kept out of the row, or the library
    // reports a tempo of zero as though somebody had measured it.
    let bpm = primary
        .and_then(|t| {
            t.get_string(ItemKey::Bpm)
                .or_else(|| t.get_string(ItemKey::IntegerBpm))
        })
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0);
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

/// The fallback in [`parse_track`]: symphonia's reading of the same file.
///
/// It has no bitrate to offer — `AudioCodecParameters` carries none — so a track
/// that arrives this way has that column empty.
fn parse_track_symphonia(
    path: &str,
    content_type: &str,
    mtime_ms: i64,
    fingerprint: Option<String>,
) -> Result<TrackInsert> {
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::{MetadataOptions, StandardTag};

    let p = Path::new(path);
    let file = std::fs::File::open(path).context("open")?;
    let mut hint = Hint::new();
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("probe")?;

    let track = format
        .default_track(TrackType::Audio)
        .context("no audio track")?;
    let duration = match (track.num_frames, track.time_base) {
        (Some(frames), Some(base)) => base
            .calc_time((frames as i64).into())
            .map(|t| t.as_secs_f64()),
        _ => None,
    };
    let sample_rate = track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .and_then(|a| a.sample_rate)
        .map(i64::from);

    let tags = all_tags(format.metadata());

    let mut title = None;
    let mut artist = None;
    let mut album = None;
    let mut genre = None;
    let mut year = None;
    let mut bpm = None;
    let mut album_artist = None;
    let mut isrc = None;
    let mut initial_key = None;
    let mut track_no = None;
    let mut track_total = None;
    let mut disc_no = None;
    let mut disc_total = None;
    let mut recording_date: Option<String> = None;

    for tag in &tags {
        let Some(std) = &tag.std else { continue };
        let take = |slot: &mut Option<String>, v: &std::sync::Arc<String>| {
            if slot.is_none() && !v.is_empty() {
                *slot = Some(v.to_string());
            }
        };
        match std {
            StandardTag::TrackTitle(v) => take(&mut title, v),
            StandardTag::Artist(v) => take(&mut artist, v),
            StandardTag::Album(v) => take(&mut album, v),
            StandardTag::Genre(v) => take(&mut genre, v),
            StandardTag::AlbumArtist(v) => take(&mut album_artist, v),
            StandardTag::IdentIsrc(v) => take(&mut isrc, v),
            StandardTag::InitialKey(v) => take(&mut initial_key, v),
            StandardTag::RecordingDate(v) => take(&mut recording_date, v),
            StandardTag::RecordingYear(v) => year = year.or(Some(i64::from(*v))),
            StandardTag::Bpm(v) => bpm = bpm.or((*v > 0).then_some(*v as f64)),
            StandardTag::TrackNumber(v) => track_no = track_no.or(Some(*v as i64)),
            StandardTag::TrackTotal(v) => track_total = track_total.or(Some(*v as i64)),
            StandardTag::DiscNumber(v) => disc_no = disc_no.or(Some(*v as i64)),
            StandardTag::DiscTotal(v) => disc_total = disc_total.or(Some(*v as i64)),
            _ => {}
        }
    }

    // A date is only a year here; the column holds one.
    let year = year.or_else(|| {
        recording_date
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse::<i64>().ok())
    });

    Ok(TrackInsert {
        path: path.to_string(),
        content_type: content_type.to_string(),
        title: Some(title.unwrap_or_else(|| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string()
        })),
        artist: Some(artist.unwrap_or_else(|| "Unknown".into())),
        album: Some(album.unwrap_or_else(|| "Unknown".into())),
        genre,
        year,
        duration,
        bpm,
        sample_rate,
        bitrate: None,
        format: p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase()),
        mtime: Some(mtime_ms),
        album_artist,
        track_no,
        track_total,
        disc_no,
        disc_total,
        isrc,
        initial_key,
        comment: comment_of_symphonia(&tags),
        fingerprint,
    })
}

/// Every tag the file holds, oldest metadata revision first.
///
/// Not `Metadata::skip_to_latest`: that returns the *newest* revision, and an
/// MP3 may carry an ID3v2 tag at its head and an ID3v1 one at its tail. The
/// ID3v1 is the newer revision and has six fixed fields where the ID3v2 has
/// everything, so skipping to it read a title, a year, a comment and a wrong
/// track number out of a file with fifteen tags in it.
///
/// Oldest first, because the caller keeps the first value it is given for each
/// field, so the richer tag wins. `pop` returns the front revision and never
/// empties the log, so the last one is left for `current`.
fn all_tags(mut meta: symphonia::core::meta::Metadata<'_>) -> Vec<symphonia::core::meta::Tag> {
    let mut tags = Vec::new();
    while let Some(rev) = meta.pop() {
        tags.extend(rev.media.tags);
    }
    if let Some(rev) = meta.current() {
        tags.extend(rev.media.tags.iter().cloned());
    }
    tags
}

/// The track's own comment: the undescribed one.
///
/// symphonia maps every ID3v2 `COMM` frame onto `StandardTag::Comment`, as
/// lofty does, but keeps the frame's description in a `SHORT_DESCRIPTION`
/// sub-field. On an iTunes-processed file the described ones are `iTunNORM` and
/// `iTunSMPB` — volume and gapless data, not anything an operator wrote.
fn comment_of_symphonia(tags: &[symphonia::core::meta::Tag]) -> Option<String> {
    use symphonia::core::meta::StandardTag;
    const DESCRIPTION: &str = "SHORT_DESCRIPTION";
    tags.iter()
        .filter(|t| matches!(t.std, Some(StandardTag::Comment(_))))
        .find(|t| {
            t.raw
                .sub_fields
                .as_deref()
                .is_none_or(|subs| !subs.iter().any(|s| s.field == DESCRIPTION))
        })
        .and_then(|t| match &t.std {
            Some(StandardTag::Comment(v)) => (!v.is_empty()).then(|| v.to_string()),
            _ => None,
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
    use crate::audio_measure::bpm::Bpm;
    use crate::audio_measure::key::{self, Key};
    use crate::audio_measure::test_audio::write_wav;
    use crate::library::db::TrackMetadataUpdate;
    use crate::library::test_audio::{retag_externally, write_tag};
    use tempfile::TempDir;

    fn music(dir: &Path) -> ScanRoot {
        ScanRoot {
            content_type: "music",
            path: dir.to_string_lossy().into_owned(),
        }
    }

    fn scan(db: &Db, roots: &[ScanRoot]) -> ScanOutcome {
        scan_all(db, roots, Missing::Mark, &|| false, |_, _| {}).unwrap()
    }

    /// A scan as the library check starts one, unattended.
    fn scan_additively(db: &Db, roots: &[ScanRoot]) -> ScanOutcome {
        scan_all(db, roots, Missing::OnlyMoved, &|| false, |_, _| {}).unwrap()
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

    /// lofty picks its reader from the extension, so a file whose contents are
    /// not what its name claims is rejected outright — the same failure a real
    /// `.ogg` whose first logical stream is Theora produces. symphonia sniffs the
    /// bytes instead, so the track enters the library rather than being reported
    /// as new by every check from here on.
    ///
    /// A WAV is the one fixture that can be built without a binary in the repo,
    /// and symphonia surfaces neither lofty's chunked ID3v2 nor RIFF `LIST INFO`
    /// from one, so this asserts only that the file is admitted. That the
    /// fallback carries tags is covered by the corpus survey below, which reads
    /// real files.
    #[test]
    fn a_file_lofty_cannot_read_is_read_by_symphonia() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);

        write_wav(&dir.path().join("mislabelled.ogg"), 2, 1);
        let outcome = scan(&db, &[music(dir.path())]);

        assert_eq!(outcome.total, 2);
        assert_eq!(titles(&db), ["a", "mislabelled"]);
        let track = db
            .search("mislabelled", None, None, None)
            .unwrap()
            .into_iter()
            .next()
            .expect("the track is in the library");
        // symphonia read it, so the duration is there — the row is not the
        // tagless shell a file neither reader can open would leave.
        assert!(track.duration > 0.0, "duration {}", track.duration);
    }

    /// An MP3 may carry an ID3v2 tag at its head and an ID3v1 one at its tail.
    /// The ID3v1 is the *newer* revision and holds six fixed fields, so reading
    /// only the newest — which is what `Metadata::skip_to_latest` returns — read
    /// four tags out of a file that had fifteen, and took the wrong track number
    /// with it.
    #[test]
    fn the_richer_tag_wins_when_a_file_carries_two() {
        use symphonia::core::meta::well_known::{METADATA_ID_ID3V1, METADATA_ID_ID3V2};
        use symphonia::core::meta::{
            MetadataBuilder, MetadataInfo, MetadataLog, RawTag, StandardTag, Tag,
        };

        let info = |id, short_name| MetadataInfo {
            metadata: id,
            short_name,
            long_name: short_name,
        };
        let tagged =
            |key: &str, value: &str, std: StandardTag| Tag::new_std(RawTag::new(key, value), std);

        let mut id3v2 = MetadataBuilder::new(info(METADATA_ID_ID3V2, "id3v2"));
        id3v2.add_tag(tagged(
            "TPE1",
            "The Artist",
            StandardTag::Artist("The Artist".to_string().into()),
        ));
        id3v2.add_tag(tagged("TRCK", "2", StandardTag::TrackNumber(2)));

        // Read later, so the newer revision, and poorer.
        let mut id3v1 = MetadataBuilder::new(info(METADATA_ID_ID3V1, "id3v1"));
        id3v1.add_tag(tagged("TRCK", "1", StandardTag::TrackNumber(1)));

        let mut log = MetadataLog::default();
        log.push(id3v2.build());
        log.push(id3v1.build());

        let tags = all_tags(log.metadata());

        assert_eq!(tags.len(), 3, "every revision is read: {tags:?}");
        let first_track = tags
            .iter()
            .find_map(|t| match t.std {
                Some(StandardTag::TrackNumber(n)) => Some(n),
                _ => None,
            })
            .expect("a track number");
        assert_eq!(first_track, 2, "the ID3v2 track number comes first");
        assert!(
            tags.iter().any(
                |t| matches!(&t.std, Some(StandardTag::Artist(a)) if a.as_str() == "The Artist")
            ),
            "the artist only the ID3v2 has survives: {tags:?}"
        );
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
    fn an_automatic_scan_adds_without_retiring_what_it_cannot_find() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");

        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        write_wav(&dir.path().join("b.wav"), 2, 1);
        let outcome = scan_additively(&db, &[music(dir.path())]);

        assert_eq!(outcome.missing, 0, "absence alone retires nothing");
        assert_eq!(missing_since(&db, id), None, "the gone track is untouched");
        assert!(
            titles(&db).contains(&"b".to_string()),
            "the new file landed"
        );

        // The operator's own scan still applies it.
        let outcome = scan(&db, &[music(dir.path())]);
        assert_eq!(outcome.missing, 1);
        assert!(missing_since(&db, id).is_some());
    }

    #[test]
    fn an_automatic_scan_follows_a_file_that_moved() {
        let (dir, db) = library();
        write_wav(&dir.path().join("a.wav"), 1, 1);
        scan(&db, &[music(dir.path())]);
        backfill(&db);
        let id = id_of(&db, "a");

        std::fs::rename(dir.path().join("a.wav"), dir.path().join("sub/a.wav")).unwrap_or_else(
            |_| {
                std::fs::create_dir_all(dir.path().join("sub")).unwrap();
                std::fs::rename(dir.path().join("a.wav"), dir.path().join("sub/a.wav")).unwrap();
            },
        );
        let outcome = scan_additively(&db, &[music(dir.path())]);

        assert_eq!(
            outcome.reattached, 1,
            "the audio arriving elsewhere is evidence enough to retire the old path"
        );
        assert_eq!(id_of(&db, "a"), id, "and no duplicate row is minted");
        assert_eq!(missing_since(&db, id), None);
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

    /// A `BPM=0` tag is a tagger declining to answer, not a track standing still,
    /// and it is what most tagged files in a real library turn out to carry.
    ///
    /// Written as `IntegerBpm`, which is the only BPM field ID3v2 has. Reading
    /// just `Bpm` — the decimal one — is why an MP3's tempo tag went unread.
    #[test]
    fn a_zero_bpm_tag_is_not_a_tempo() {
        let (dir, db) = library();
        let file = dir.path().join("a.wav");
        write_wav(&file, 1, 1);
        write_tag(&file, ItemKey::IntegerBpm, "0", "Zero");

        let other = dir.path().join("b.wav");
        write_wav(&other, 2, 1);
        write_tag(&other, ItemKey::IntegerBpm, "128", "Real");

        scan(&db, &[music(dir.path())]);

        assert_eq!(
            db.get_track(id_of(&db, "Real")).unwrap().unwrap().bpm,
            Some(128.0),
            "a real tag is read, so the zero below is a decision and not a miss"
        );
        assert_eq!(db.get_track(id_of(&db, "Zero")).unwrap().unwrap().bpm, None);
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
        db.set_bpm(
            id,
            Some(Bpm {
                bpm: 128.0,
                confidence: 0.9,
            }),
            1234,
            row_mtime(&db, id),
        )
        .unwrap();
        db.set_key(
            id,
            Some(Key {
                pitch_class: 9,
                mode: key::Mode::Minor,
                confidence: 0.7,
            }),
            1234,
            row_mtime(&db, id),
        )
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

        let outcome = scan_all(
            &db,
            &[music(dir.path())],
            Missing::Mark,
            &|| true,
            |_, _| {},
        )
        .unwrap();

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

/// Reads both ways over a real library and reports where they differ.
///
/// Kept after the reader question was settled, because it is what answers it
/// again: if symphonia is ever to become the primary reader, this is the survey
/// that has to come back clean first. It is what showed that it cannot be, yet
/// — see `docs/library.md#reading-tags`.
///
/// Ignored by default and gated on `TAG_CORPUS`, like the tempo and key surveys
/// (`audio_measure/bpm.rs`, `audio_measure/key.rs`). It prints rather than
/// asserts: the point is a person reading the differences before the reader is
/// swapped, because a field lofty reads and symphonia does not is a field the
/// write-back would go on to delete from the operator's file.
///
/// ```text
/// TAG_CORPUS=/path/to/library cargo test --manifest-path src-tauri/Cargo.toml \
///   tag_corpus -- --ignored --nocapture
/// ```
#[cfg(test)]
mod tag_corpus {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn files(root: &Path) -> Vec<PathBuf> {
        let mut stack = vec![root.to_path_buf()];
        let mut out = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .is_some_and(|n| n.to_str().is_some_and(|n| n.starts_with('.')))
                {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(crate::audio_measure::formats::is_audio_extension)
                {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// Every column a tag read fills, as `(name, value)` pairs, so two reads can
    /// be compared without naming each field twice.
    fn fields(t: &TrackInsert) -> Vec<(&'static str, String)> {
        fn s(v: &Option<String>) -> String {
            v.clone().unwrap_or_else(|| "-".into())
        }
        fn n<T: std::fmt::Display>(v: &Option<T>) -> String {
            v.as_ref()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "-".into())
        }
        vec![
            ("title", s(&t.title)),
            ("artist", s(&t.artist)),
            ("album", s(&t.album)),
            ("album_artist", s(&t.album_artist)),
            ("genre", s(&t.genre)),
            ("year", n(&t.year)),
            ("bpm", n(&t.bpm)),
            ("isrc", s(&t.isrc)),
            ("initial_key", s(&t.initial_key)),
            ("comment", s(&t.comment)),
            ("track_no", n(&t.track_no)),
            ("track_total", n(&t.track_total)),
            ("disc_no", n(&t.disc_no)),
            ("disc_total", n(&t.disc_total)),
            (
                "duration",
                t.duration
                    .map(|d| format!("{d:.1}"))
                    .unwrap_or_else(|| "-".into()),
            ),
            ("sample_rate", n(&t.sample_rate)),
        ]
    }

    #[test]
    #[ignore = "needs a real library; set TAG_CORPUS"]
    fn lofty_and_symphonia_read_the_same_tags() {
        let Ok(root) = std::env::var("TAG_CORPUS") else {
            eprintln!("set TAG_CORPUS to a library root to run this");
            return;
        };
        let all = files(Path::new(&root));
        let limit: usize = std::env::var("TAG_SAMPLE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let step = (all.len() / limit.max(1)).max(1);
        let sample: Vec<_> = all.iter().step_by(step).take(limit).collect();
        println!("{} files under {root}, reading {}", all.len(), sample.len());

        // Per field: how often they agreed, and how often only one side had a
        // value. Only-lofty is the dangerous column.
        let mut agreed: BTreeMap<&str, usize> = BTreeMap::new();
        let mut only_lofty: BTreeMap<&str, usize> = BTreeMap::new();
        let mut lofty_empty: BTreeMap<&str, usize> = BTreeMap::new();
        let mut only_symphonia: BTreeMap<&str, usize> = BTreeMap::new();
        let mut differed: BTreeMap<&str, usize> = BTreeMap::new();
        let mut lofty_failed = 0usize;
        let mut symphonia_failed = 0usize;
        let mut examples: BTreeMap<&str, String> = BTreeMap::new();
        let mut duration_gaps: Vec<f64> = Vec::new();

        for path in &sample {
            let p = path.to_string_lossy().to_string();
            let a = parse_track(&p, "music", 0, None);
            let b = parse_track_symphonia(&p, "music", 0, None);
            match (&a, &b) {
                (Err(e), Err(_)) => {
                    lofty_failed += 1;
                    symphonia_failed += 1;
                    println!("both failed: {p}: {e}");
                    continue;
                }
                (Err(e), Ok(_)) => {
                    lofty_failed += 1;
                    println!("lofty only failed: {p}: {e}");
                    continue;
                }
                (Ok(_), Err(e)) => {
                    symphonia_failed += 1;
                    println!("SYMPHONIA FAILED: {p}: {e:#}");
                    continue;
                }
                (Ok(_), Ok(_)) => {}
            }
            for ((name, la), (_, sa)) in fields(a.as_ref().unwrap())
                .into_iter()
                .zip(fields(b.as_ref().unwrap()))
            {
                let (lv, sv) = (la == "-", sa == "-");
                match (lv, sv) {
                    (false, true) if la.is_empty() => {
                        // lofty reports an empty tag as a value, symphonia omits
                        // it. `tag_write::set` removes an empty value too, so
                        // this changes the column from "" to NULL and nothing
                        // else.
                        *lofty_empty.entry(name).or_default() += 1;
                    }
                    (false, true) => {
                        *only_lofty.entry(name).or_default() += 1;
                        println!("ONLY LOFTY {name}: {p} = {la:?}");
                    }
                    (true, false) => *only_symphonia.entry(name).or_default() += 1,
                    (true, true) => {}
                    (false, false) if la == sa => *agreed.entry(name).or_default() += 1,
                    (false, false) if name == "duration" => {
                        *differed.entry(name).or_default() += 1;
                        if let (Ok(l), Ok(v)) = (la.parse::<f64>(), sa.parse::<f64>()) {
                            duration_gaps.push((v - l).abs());
                        }
                    }
                    (false, false) => {
                        *differed.entry(name).or_default() += 1;
                        examples
                            .entry(name)
                            .or_insert_with(|| format!("{p} lofty={la:?} symphonia={sa:?}"));
                    }
                }
            }
        }

        println!("\nfailures: lofty {lofty_failed}, symphonia {symphonia_failed}");
        println!(
            "\n{:<14} {:>7} {:>7} {:>11} {:>11} {:>9}",
            "field", "agreed", "differ", "only lofty", "only symph", "lofty \"\""
        );
        for (name, _) in fields(&TrackInsert {
            path: String::new(),
            content_type: String::new(),
            title: None,
            artist: None,
            album: None,
            genre: None,
            year: None,
            duration: None,
            bpm: None,
            sample_rate: None,
            bitrate: None,
            format: None,
            mtime: None,
            album_artist: None,
            track_no: None,
            track_total: None,
            disc_no: None,
            disc_total: None,
            isrc: None,
            initial_key: None,
            comment: None,
            fingerprint: None,
        }) {
            println!(
                "{:<14} {:>7} {:>7} {:>11} {:>11} {:>9}",
                name,
                agreed.get(name).unwrap_or(&0),
                differed.get(name).unwrap_or(&0),
                only_lofty.get(name).unwrap_or(&0),
                only_symphonia.get(name).unwrap_or(&0),
                lofty_empty.get(name).unwrap_or(&0),
            );
        }
        if !duration_gaps.is_empty() {
            duration_gaps.sort_by(f64::total_cmp);
            let n = duration_gaps.len();
            println!(
                "\nduration gaps over {n}: min {:.2}s median {:.2}s max {:.2}s",
                duration_gaps[0],
                duration_gaps[n / 2],
                duration_gaps[n - 1]
            );
        }
        println!("\nfirst example per field that differed or lofty-only:");
        for (name, ex) in &examples {
            println!("  {name}: {ex}");
        }
    }
}
