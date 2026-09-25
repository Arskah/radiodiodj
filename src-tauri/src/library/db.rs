use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::auto_cue::{self, Analysed, Envelope};
use crate::audio::cue_points::CuePoints;
use crate::library::fingerprint;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub play_count: i64,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub bpm: Option<f64>,
    pub sample_rate: Option<i64>,
    pub bitrate: Option<i64>,
    pub format: Option<String>,
    /// The track's radio edit. Arrives with every `SELECT *`, so nothing can
    /// reach air with stale markers.
    pub cue_points: CuePoints,
    /// Which tag columns the operator edited, as [`EditedFields`] bits. A
    /// rescan keeps those columns instead of taking the file's tags.
    #[serde(default)]
    pub edited_fields: i64,
}

/// Bits of `tracks.edited_fields`, one per tag column.
pub struct EditedFields;

impl EditedFields {
    pub const TITLE: i64 = 1;
    pub const ARTIST: i64 = 2;
    pub const ALBUM: i64 = 4;
    pub const GENRE: i64 = 8;
    pub const YEAR: i64 = 16;
}

/// A track's tag columns, as a tag write-back reads and compares them.
#[derive(Clone, Debug, PartialEq)]
pub struct TagValues {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub fingerprint: Option<String>,
    pub edited_fields: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub total_artists: i64,
    pub total_albums: i64,
    pub total_hours: f64,
    pub tracks_by_type: TracksByType,
}

/// A missing row, as the library health view lists it.
pub struct MissingRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub path: String,
    pub missing_since: i64,
    pub play_count: i64,
    pub has_cue_points: bool,
}

/// A present row with the identity columns `Track` leaves out.
pub struct HealthRow {
    pub track: Track,
    pub path: String,
    pub content_type: String,
    pub fingerprint: Option<String>,
}

/// A present track the analysis pass could not decode.
pub struct UnreadableRow {
    pub row: HealthRow,
    pub error: String,
    /// Unix ms.
    pub failed_at: i64,
}

/// The operator's acknowledgement of one health finding. `value` is what the
/// finding looked like when it was dismissed.
#[derive(Clone, Debug, PartialEq)]
pub struct Dismissal {
    pub kind: String,
    pub key: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TracksByType {
    pub music: i64,
    pub commercial: i64,
    pub jingle: i64,
}

/// What a random pick may not return: the ids already queued, plus the rotation
/// constraints music selection adds on top of them.
///
/// The windows are instants, not durations, so every rung of one relaxation
/// ladder measures against the same `now`. `None` disables that rule, and
/// `artist_keys` is empty unless the artist rule is on. Keys are normalised by
/// [`artist_key`], which matches what the SQL side does to the stored column.
#[derive(Default, Clone, Copy, Debug)]
pub struct SelectionFilter<'a> {
    pub exclude_ids: &'a [i64],
    /// Reject a track that aired at or after this instant (unix ms).
    pub title_since: Option<i64>,
    /// Reject a track whose artist aired at or after this instant (unix ms).
    pub artist_since: Option<i64>,
    /// Reject a track whose artist key is in this list.
    pub artist_keys: &'a [String],
    /// Return at most one track per artist, so the rows of one block spread
    /// across the library instead of stacking up on one act.
    pub spread_artists: bool,
}

impl<'a> SelectionFilter<'a> {
    /// The plain exclusion every content type gets: nothing but the queue.
    pub fn excluding(exclude_ids: &'a [i64]) -> Self {
        Self {
            exclude_ids,
            ..Self::default()
        }
    }

    /// How rows are grouped when [`Self::spread_artists`] is on: by artist, but
    /// with every blank artist its own group — an untagged library shares one
    /// empty string, and one group for all of it would cap a block at a single
    /// untagged track.
    const SPREAD_KEY: &'static str =
        "CASE WHEN trim(coalesce(artist, '')) = '' THEN '\u{1}' || id \
         ELSE lower(trim(artist)) END";

    /// The `AND` clauses this filter contributes, in the order
    /// [`Self::params`] binds their placeholders.
    ///
    /// `NOT IN ()` is a syntax error in SQLite, so a clause appears only when
    /// it has something to say.
    fn sql(&self) -> String {
        let mut sql = exclude_sql(self.exclude_ids);
        if self.title_since.is_some() {
            sql.push_str(
                " AND id NOT IN (SELECT track_id FROM play_log \
                  WHERE track_id IS NOT NULL AND aired_at >= ?)",
            );
        }
        if self.artist_since.is_some() {
            // Blank log artists are skipped, or one untagged airing would block
            // every untagged track in the library.
            sql.push_str(
                " AND lower(trim(artist)) NOT IN (SELECT lower(trim(artist)) FROM play_log \
                  WHERE artist IS NOT NULL AND trim(artist) <> '' AND aired_at >= ?)",
            );
        }
        if !self.artist_keys.is_empty() {
            let placeholders = vec!["?"; self.artist_keys.len()].join(", ");
            sql.push_str(&format!(" AND lower(trim(artist)) NOT IN ({placeholders})"));
        }
        sql
    }

    fn params(&self) -> impl Iterator<Item = rusqlite::types::Value> + '_ {
        self.exclude_ids
            .iter()
            .map(|&id| rusqlite::types::Value::Integer(id))
            .chain(self.title_since.map(rusqlite::types::Value::Integer))
            .chain(self.artist_since.map(rusqlite::types::Value::Integer))
            .chain(
                self.artist_keys
                    .iter()
                    .map(|k| rusqlite::types::Value::Text(k.clone())),
            )
    }
}

/// Normalise an artist for rotation matching.
///
/// ASCII-only on purpose: SQLite's `lower()` is ASCII-only, and the blocklist
/// is compared against `lower(trim(artist))` evaluated in SQL, so lowering more
/// than SQLite does in Rust would make the two sides disagree. The cost is that
/// `Ämmä` and `ämmä` count as different artists — a missed constraint, never a
/// wrong result. See `docs/rotation.md`.
pub fn artist_key(artist: &str) -> String {
    artist.trim().to_ascii_lowercase()
}

/// Deserialize a nullable, optionally-present field into a "double option".
///
/// Serde's default `Option<Option<T>>` deserialize collapses an explicit JSON
/// `null` to `None` (indistinguishable from an absent field), which makes it
/// impossible to tell "leave unchanged" from "clear to NULL". This helper keeps
/// the distinction: absent → `None` (via `#[serde(default)]`), present `null`
/// → `Some(None)` (clear), present value → `Some(Some(v))` (set).
fn double_option<'de, T, D>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackMetadataUpdate {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub genre: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub year: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Default, Clone)]
pub struct TrackInsert {
    pub path: String,
    pub content_type: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub duration: Option<f64>,
    pub bpm: Option<f64>,
    pub sample_rate: Option<i64>,
    pub bitrate: Option<i64>,
    pub format: Option<String>,
    pub mtime: Option<i64>,
    /// Left as stored when `None`.
    pub fingerprint: Option<String>,
}

/// A track the analysis worker still has to read.
pub struct AnalysisJob {
    pub id: i64,
    pub path: String,
    /// The file as it was when the job was queued. Carried back into the
    /// commit, so a result decoded from audio the operator has since replaced
    /// is refused rather than stamped over the requeue.
    pub mtime: Option<i64>,
    /// Decides whether an automatic Next Start is derived at all.
    pub content_type: String,
    pub needs_waveform: bool,
    pub needs_fingerprint: bool,
    pub needs_loudness: bool,
    pub needs_auto_cue: bool,
    /// The row has a derived trio but no level table — analysed before the
    /// table existed. The decode fills the table in and leaves the trio alone:
    /// re-deriving it here would apply today's thresholds to a track analysed
    /// under yesterday's, which is the implicit mass re-analysis
    /// `docs/cue-auto-analysis.md` rules out.
    pub needs_auto_cue_levels: bool,
}

/// One row as the scanner sees it.
pub struct IndexRow {
    pub id: i64,
    pub path: String,
    pub content_type: String,
    pub mtime: Option<i64>,
    pub missing_since: Option<i64>,
    pub fingerprint: Option<String>,
}

/// Everything one scan changes, applied in a single transaction.
#[derive(Default)]
pub struct Reconcile {
    /// Missing rows whose file is back at the same path.
    pub revive: Vec<i64>,
    /// Rows for paths the library already holds.
    pub upserts: Vec<TrackInsert>,
    /// Present rows whose file is gone.
    pub gone: Vec<i64>,
    /// Files at paths the library does not hold. Matched by fingerprint
    /// after `gone` is marked, so a file moved within one scan reattaches.
    pub new_files: Vec<TrackInsert>,
    pub now_ms: i64,
}

#[derive(Debug, Default, PartialEq)]
pub struct Reconciled {
    pub missing: usize,
    pub reattached: usize,
    pub duplicated: usize,
    pub inserted: usize,
}

pub struct MediaTrack {
    pub path: String,
    pub duration: f64,
    pub loudness: StoredLoudness,
}

/// A track's stored ReplayGain measurement, as the load path reads it back.
/// Both fields are `None` until the analysis pass has measured the file, and
/// stay `None` for one it measured as silent.
#[derive(Clone, Copy, Default)]
pub struct StoredLoudness {
    pub gain_db: Option<f64>,
    pub peak: Option<f64>,
}

/// Everything one `Load` needs, in a single query: where the audio is, how the
/// deck should trim it, and what the now-playing broadcast should announce.
pub struct TrackLoadInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: Option<String>,
    /// Tag-derived, so nullable and wrong on VBR MP3. The deck resolves cue
    /// points against the decoded duration instead; this is the clamp the write
    /// path and the broadcast payload have to make do with.
    pub duration: f64,
    pub content_type: String,
    pub path: String,
    pub cue_points: CuePoints,
    pub loudness: StoredLoudness,
}

pub struct Db {
    conn: Mutex<Connection>,
    /// Whether an automatically derived cue set takes effect. Held here rather
    /// than read from the config per query: every row the library hands out
    /// passes this, and the answer changes only when the operator flips it.
    apply_auto_cue: AtomicBool,
    /// Whether a derived Next Start takes effect, under [`Self::apply_auto_cue`].
    apply_auto_next_start: AtomicBool,
}

/// What the library reports for a derived cue set. `trio` is the master switch
/// over Cue In, Cue Out and Next Start together; `next_start` hides the
/// Handover position alone, under it. Neither touches a manually owned row.
#[derive(Clone, Copy)]
struct AutoCuePolicy {
    trio: bool,
    next_start: bool,
}

impl Db {
    /// Open the library database, migrating it to the current schema.
    ///
    /// A database from before the current [`DB_EPOCH`] is moved aside and
    /// replaced by an empty one; the outcome names the backup so the caller can
    /// drop session state that points at the old ids. A database written by a
    /// newer build is refused untouched with [`OpenError`]. Pending migrations
    /// run only after a `VACUUM INTO` copy of the file has been taken.
    pub fn open(path: &Path) -> Result<Opened> {
        let reset_backup = retire_legacy(path)?;
        let mut conn = Connection::open(path).context("open sqlite")?;
        let found = user_version(&conn)?;
        let supported = schema_version();
        if found > supported {
            return Err(OpenError::TooNew { found, supported }.into());
        }
        // WAL for concurrent read/write; `synchronous = NORMAL` drops the fsync
        // on every autocommit (e.g. the per-track waveform writes) — safe under
        // WAL since only a crash mid-commit can lose the last transaction, and
        // all our writes (waveforms especially) are recomputable. `busy_timeout`
        // lets a writer wait briefly rather than erroring when the background
        // waveform threads contend for the connection.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        if found > 0 && found < supported {
            backup(&conn, path, found)?;
        }
        MIGRATIONS.to_latest(&mut conn).context("migrate library")?;
        Ok(Opened {
            db: Self {
                conn: Mutex::new(conn),
                apply_auto_cue: AtomicBool::new(true),
                apply_auto_next_start: AtomicBool::new(true),
            },
            reset_backup,
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        MIGRATIONS.to_latest(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            apply_auto_cue: AtomicBool::new(true),
            apply_auto_next_start: AtomicBool::new(true),
        })
    }

    pub fn search(
        &self,
        query: &str,
        content_type: Option<&str>,
        sort_by: Option<&str>,
        sort_dir: Option<&str>,
    ) -> Result<Vec<Track>> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let order = order_clause(sort_by, sort_dir);
        let trimmed = query.trim();

        if trimmed.is_empty() {
            let order_sql = order.unwrap_or_else(|| {
                "artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE".into()
            });
            let (sql, params): (String, Vec<rusqlite::types::Value>) = if let Some(t) = content_type
            {
                (
                    format!(
                        "SELECT * FROM tracks WHERE missing_since IS NULL AND content_type = ? \
                         ORDER BY {} LIMIT 200",
                        order_sql
                    ),
                    vec![t.to_owned().into()],
                )
            } else {
                (
                    format!(
                        "SELECT * FROM tracks WHERE missing_since IS NULL ORDER BY {} LIMIT 200",
                        order_sql
                    ),
                    vec![],
                )
            };
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map(params_from_iter(params.iter()), |r| row_to_track(r, policy))?;
            return rows.collect::<rusqlite::Result<_>>().map_err(Into::into);
        }

        let fts_q = trimmed
            .split_whitespace()
            .map(fts5_prefix_term)
            .collect::<Vec<_>>()
            .join(" ");
        let order_sql = order.unwrap_or_else(|| "rank".to_string());

        let (sql, params): (String, Vec<rusqlite::types::Value>) = if let Some(t) = content_type {
            (
                format!(
                    "SELECT tracks.* FROM tracks_fts \
                     JOIN tracks ON tracks.id = tracks_fts.rowid \
                     WHERE tracks_fts MATCH ? AND tracks.missing_since IS NULL \
                       AND tracks.content_type = ? \
                     ORDER BY {} LIMIT 200",
                    order_sql
                ),
                vec![fts_q.into(), t.to_owned().into()],
            )
        } else {
            (
                format!(
                    "SELECT tracks.* FROM tracks_fts \
                     JOIN tracks ON tracks.id = tracks_fts.rowid \
                     WHERE tracks_fts MATCH ? AND tracks.missing_since IS NULL \
                     ORDER BY {} LIMIT 200",
                    order_sql
                ),
                vec![fts_q.into()],
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params.iter()), |r| row_to_track(r, policy))?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_media_track(&self, id: i64) -> Result<Option<MediaTrack>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT path, duration, rg_gain, rg_peak FROM tracks WHERE id = ?")?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(MediaTrack {
                path: r.get(0)?,
                duration: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                loudness: StoredLoudness {
                    gain_db: r.get(2)?,
                    peak: r.get(3)?,
                },
            })
        })?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    /// The one query a deck load runs: path and markers for playback, plus the
    /// fields the now-playing broadcast announces.
    pub fn get_track_load_info(&self, id: i64) -> Result<Option<TrackLoadInfo>> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, album, genre, duration, content_type, path, \
                    cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms, \
                    auto_cue_state, rg_gain, rg_peak \
             FROM tracks WHERE id = ?",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(TrackLoadInfo {
                id: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                artist: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                album: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                genre: r.get::<_, Option<String>>(4)?,
                duration: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                content_type: r.get(6)?,
                path: r.get(7)?,
                cue_points: effective_cue_points(r, policy)?,
                loudness: StoredLoudness {
                    gain_db: r.get("rg_gain")?,
                    peak: r.get("rg_peak")?,
                },
            })
        })?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    pub fn get_track(&self, id: i64) -> Result<Option<Track>> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT * FROM tracks WHERE id = ?")?;
        let mut rows = stmt.query_map([id], |r| row_to_track(r, policy))?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    pub fn get_tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        let policy = self.auto_cue_policy();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT * FROM tracks WHERE id IN ({})", placeholders);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(ids.iter()), |r| row_to_track(r, policy))?;
        let by_id: HashMap<i64, Track> = rows
            .collect::<rusqlite::Result<Vec<Track>>>()?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        Ok(ids.iter().filter_map(|i| by_id.get(i).cloned()).collect())
    }

    /// Resolve `(id, path)` for the given ids, preserving the input order and
    /// skipping ids with no matching row. Used by the prefetch cache to build
    /// its residency window without loading full track rows.
    pub fn get_paths_by_ids(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, path FROM tracks WHERE missing_since IS NULL AND id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let by_id: HashMap<i64, String> = rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect();
        Ok(ids
            .iter()
            .filter_map(|i| by_id.get(i).map(|p| (*i, p.clone())))
            .collect())
    }

    /// Fetch a track's stored amplitude-curve peaks, or `None` when the track is
    /// unknown or has no waveform yet (the async waveform worker fills it after
    /// the metadata scan).
    pub fn get_waveform(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT waveform FROM tracks WHERE id = ?")?;
        let mut rows = stmt.query_map([id], |r| r.get::<_, Option<Vec<u8>>>(0))?;
        match rows.next() {
            Some(r) => r.map_err(Into::into),
            None => Ok(None),
        }
    }

    /// Store a track's computed amplitude-curve peaks. Written by the async
    /// waveform worker, separately from the metadata upsert.
    pub fn set_waveform(&self, id: i64, peaks: &[u8]) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET waveform = ? WHERE id = ?",
            params![peaks, id],
        )?;
        Ok(())
    }

    /// Store a track's cue points, clamped to the duration the library holds so
    /// a bad input cannot put the deck out of range. Returns the clamped value
    /// for the caller to adopt: there is deliberately no second implementation
    /// of the clamp in the renderer.
    ///
    /// The authoritative clamp runs again at load time, against the decoded
    /// duration — the tag duration this clamps against is nullable and wrong on
    /// VBR MP3.
    ///
    /// A write that moves any of Cue In, Cue Out or Next Start — clearing one
    /// to `NULL` included — takes the trio away from automatic analysis for
    /// good. Editing only a fade does not: fades are never inferred, so
    /// touching one says nothing about the markers that are.
    ///
    /// Ownership is judged against what the caller was shown, which is not the
    /// stored row while automatic cue points are switched off. Nobody can clear
    /// markers they were never given: an edit made with the feature off leaves
    /// the derived trio intact for when it goes back on.
    ///
    /// Moving the trio is the one case that does write a hidden marker away:
    /// the operator has taken it over, and what they were shown had a `NULL`
    /// where the derived Next Start was. `NULL` means the handover waits for
    /// Cue Out, which is what switching automatic Next Starts off asked for.
    pub fn set_cue_points(&self, id: i64, points: CuePoints) -> Result<CuePoints> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let current: Option<(Option<f64>, CuePoints, CuePoints)> = conn
            .query_row(
                "SELECT duration, cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, \
                        next_start_ms, auto_cue_state \
                 FROM tracks WHERE id = ?",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        row_to_cue_points(r)?,
                        effective_cue_points(r, policy)?,
                    ))
                },
            )
            .optional()?;
        let Some((duration, stored, shown)) = current else {
            anyhow::bail!("track {} is no longer in the library", id);
        };
        // The stored duration is the tag's, and the tag is wrong on VBR MP3:
        // automatic analysis reads the length off the decode and may put a
        // marker past it. Both are lower bounds on the real file, so the clamp
        // runs against the further of the two — otherwise saving one marker
        // would drag every other one inside a length the file does not have.
        // With no duration at all there is still no ceiling: a stored marker
        // is a lower bound on the file, never a limit on where the next one
        // may go.
        let end_of_file = duration
            .filter(|d| *d > 0.0)
            .map(|d| match furthest(&stored) {
                Some(ms) => d.max(ms as f64 / 1000.0),
                None => d,
            });
        // Ownership is decided on what came in against what went out, not on
        // what the clamp makes of it: only a marker the operator moved takes
        // the trio off automatic.
        let owns = points.cue_in_ms != shown.cue_in_ms
            || points.cue_out_ms != shown.cue_out_ms
            || points.next_start_ms != shown.next_start_ms;
        // An untouched trio goes back as stored — which is not what came in
        // when the feature is off. The stored trio is restored *before* the
        // clamp so the fades are bounded by the Cue Out they will be stored
        // against: bounding them by the file end instead would let a fade sit
        // past Cue Out, where the load-time resolve drops it silently.
        let written = if owns {
            points
        } else {
            CuePoints {
                cue_in_ms: stored.cue_in_ms,
                cue_out_ms: stored.cue_out_ms,
                next_start_ms: stored.next_start_ms,
                ..points
            }
        }
        .clamp(end_of_file);
        let state = if owns {
            ", auto_cue_state = 'manual'"
        } else {
            ""
        };
        let changed = conn.execute(
            &format!(
                "UPDATE tracks SET cue_in_ms = ?, fade_in_ms = ?, fade_out_ms = ?, \
                        cue_out_ms = ?, next_start_ms = ?{state} \
                 WHERE id = ?"
            ),
            params![
                written.cue_in_ms,
                written.fade_in_ms,
                written.fade_out_ms,
                written.cue_out_ms,
                written.next_start_ms,
                id
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("track {} is no longer in the library", id);
        }
        // What the caller adopts is what it will be shown next. An untouched
        // trio therefore goes back exactly as it came out — hidden for a
        // derived one while the feature is off, and intact for a manual one,
        // which the feature never hides.
        Ok(if owns {
            written
        } else {
            CuePoints {
                cue_in_ms: shown.cue_in_ms,
                cue_out_ms: shown.cue_out_ms,
                next_start_ms: shown.next_start_ms,
                ..written
            }
        })
    }

    /// Commit one automatic analysis: the trio, the ownership state and the
    /// provenance in a single statement, and only while the row still matches
    /// what was analysed. An operator save that landed while the decode ran
    /// therefore wins and the result is discarded — reported as `false`.
    ///
    /// `content_type` is the class the analysis ran under and `mtime` the file
    /// it read, and the commit checks both: a reclassification during the
    /// decode would otherwise land a music-derived Next Start on a jingle, and
    /// a file replaced under the pass would land markers derived from audio
    /// that is gone. Either refusal leaves the row `pending`, so the drain loop
    /// picks it up again and re-derives it from what is there now.
    ///
    /// Unlike [`Db::set_cue_points`] this does not clamp against the stored
    /// duration: the positions come from the decode itself, whereas the stored
    /// duration is the tag's and is wrong on VBR MP3. The load-time clamp
    /// remains authoritative either way.
    ///
    /// The fades are not derived, but they are sorted against the new trio: an
    /// operator fade saved before the analysis landed could otherwise end up
    /// past the fresh Cue Out, where the load-time clamp folds it onto Cue Out
    /// and the ramp silently collapses to nothing.
    ///
    /// Returns the whole stored set on a commit — the trio the analysis wrote
    /// plus the fades it never touches — for the copies of the track held
    /// outside the DB, and `None` when the commit was refused.
    pub fn set_auto_cue(
        &self,
        id: i64,
        analysed: &Analysed,
        content_type: &str,
        mtime: Option<i64>,
    ) -> Result<Option<CuePoints>> {
        let Analysed {
            cue,
            levels,
            thresholds,
            at_ms,
        } = analysed;
        let music = content_type == "music";
        let conn = self.conn.lock();
        // `clamp` with no duration applies the ordering rules alone, which is
        // exactly the sorting the fades need against the incoming trio.
        let fades: Option<CuePoints> = conn
            .query_row(
                "SELECT cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms \
                 FROM tracks WHERE id = ?",
                [id],
                row_to_cue_points,
            )
            .optional()?;
        let sorted = CuePoints {
            cue_in_ms: cue.cue_in_ms,
            cue_out_ms: cue.cue_out_ms,
            next_start_ms: cue.next_start_ms,
            ..fades.unwrap_or_default()
        }
        .clamp(None);
        let changed = conn.execute(
            "UPDATE tracks SET cue_in_ms = ?1, cue_out_ms = ?2, next_start_ms = ?3, \
                    fade_in_ms = ?10, fade_out_ms = ?11, \
                    auto_cue_state = 'auto', auto_cue_version = ?4, \
                    auto_cue_silence_db = ?5, auto_cue_segue_db = ?6, auto_cue_at = ?7, \
                    auto_cue_levels = ?13 \
             WHERE id = ?8 AND auto_cue_state <> 'manual' AND content_type = ?9 \
               AND mtime IS ?12",
            params![
                cue.cue_in_ms,
                cue.cue_out_ms,
                cue.next_start_ms,
                auto_cue::ALGORITHM_VERSION,
                thresholds.silence_dbfs,
                // A commercial or jingle got no automatic Next Start, so no
                // segue threshold was used on it.
                music.then_some(thresholds.segue_dbfs),
                at_ms,
                id,
                content_type,
                sorted.fade_in_ms,
                sorted.fade_out_ms,
                mtime,
                levels.encode()
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(conn
            .query_row(
                "SELECT cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms, \
                        auto_cue_state \
                 FROM tracks WHERE id = ?",
                [id],
                |r| effective_cue_points(r, self.auto_cue_policy()),
            )
            .optional()?)
    }

    /// Every present track still missing a waveform, a loudness measurement or
    /// an automatic cue, or whose fingerprint is missing or was computed by an
    /// older [`fingerprint::VERSION`], ordered by id. Drives the background
    /// analysis worker (backfill included). A track whose analysis failed is
    /// left out until its file changes.
    ///
    /// A `manual` row is never queued for its level table: the table feeds
    /// automatic derivation, and nothing derives for a track the operator owns.
    pub fn tracks_needing_analysis(&self) -> Result<Vec<AnalysisJob>> {
        let stale_fingerprint = format!(
            "(fingerprint IS NULL OR fingerprint NOT LIKE '{}:%')",
            fingerprint::VERSION
        );
        // An envelope this build cannot read counts as missing. SQL screens as
        // far as it can — absent, too short to hold the header, or a layout
        // version it does not know, the version being the first byte — so a row
        // written by an older build is requeued without decoding every blob in
        // the library to find out. It cannot screen the codes themselves, since
        // a row's length is a function of its duration; `Envelope::decode` is
        // the authority on that, and the one caller that reads a stored
        // envelope queues whatever fails it rather than trusting the two to
        // agree. See `Db::recalculate_auto_cue`.
        let missing_levels = format!(
            "(auto_cue_state <> 'manual' AND {})",
            Self::unreadable_levels()
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, path, mtime, content_type, waveform IS NULL, {stale_fingerprint}, \
                    rg_measured_at IS NULL, auto_cue_state = 'pending', \
                    {missing_levels} \
             FROM tracks \
             WHERE missing_since IS NULL AND analysis_failed_at IS NULL \
               AND (waveform IS NULL OR {stale_fingerprint} \
                    OR rg_measured_at IS NULL OR auto_cue_state = 'pending' \
                    OR {missing_levels}) \
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(AnalysisJob {
                id: r.get(0)?,
                path: r.get(1)?,
                mtime: r.get(2)?,
                content_type: r.get(3)?,
                needs_waveform: r.get(4)?,
                needs_fingerprint: r.get(5)?,
                needs_loudness: r.get(6)?,
                needs_auto_cue: r.get(7)?,
                needs_auto_cue_levels: r.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Store the level table for a track whose trio is already derived.
    ///
    /// The backfill counterpart to [`Db::set_auto_cue`], which writes both at
    /// once for a track being analysed. It deliberately touches nothing else:
    /// the trio on the row was derived under whatever thresholds were set at
    /// the time, and re-deriving it here would be an implicit mass re-analysis.
    ///
    /// Guarded like the full commit — a row the operator took over while the
    /// decode ran, one reclassified under it, or one whose file was replaced,
    /// is refused and reported as `false`, leaving it queued for the next pass.
    /// `content_type` is checked only so a reclassification is not silently
    /// stamped over; the envelope itself is a property of the audio, not the
    /// class. Refusing on it costs nothing: a reclassified row is already
    /// `pending`, so the next pass runs the full commit and writes the envelope
    /// with the trio anyway.
    pub fn set_auto_cue_levels(
        &self,
        id: i64,
        levels: &Envelope,
        content_type: &str,
        mtime: Option<i64>,
    ) -> Result<bool> {
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE tracks SET auto_cue_levels = ?1 \
             WHERE id = ?2 AND auto_cue_state <> 'manual' AND content_type = ?3 \
               AND mtime IS ?4",
            params![levels.encode(), id, content_type, mtime],
        )?;
        Ok(changed > 0)
    }

    /// Store a completed loudness measurement. `gain`/`peak` are `None` for a
    /// file that decoded but held nothing measurable — silence, or less audio
    /// than one integration block — which still counts as measured, so the
    /// pass does not pick the track up again.
    pub fn set_loudness(
        &self,
        id: i64,
        gain: Option<f64>,
        peak: Option<f64>,
        at_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET rg_gain = ?, rg_peak = ?, rg_measured_at = ? WHERE id = ?",
            params![gain, peak, at_ms, id],
        )?;
        Ok(())
    }

    pub fn set_fingerprint(&self, id: i64, fingerprint: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET fingerprint = ? WHERE id = ?",
            params![fingerprint, id],
        )?;
        Ok(())
    }

    /// Record that the file could not be decoded. The analysis pass skips the
    /// track until a scan sees the file change.
    pub fn set_analysis_failed(&self, id: i64, error: &str, at_ms: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET analysis_error = ?, analysis_failed_at = ? WHERE id = ?",
            params![error, at_ms, id],
        )?;
        Ok(())
    }

    /// Present tracks the analysis pass could not decode, oldest failure first.
    pub fn unreadable_tracks(&self) -> Result<Vec<UnreadableRow>> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, path, content_type, fingerprint, \
                    analysis_error, analysis_failed_at \
             FROM tracks \
             WHERE missing_since IS NULL AND analysis_failed_at IS NOT NULL \
             ORDER BY analysis_failed_at, id"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(UnreadableRow {
                row: HealthRow {
                    track: row_to_track(r, policy)?,
                    path: r.get("path")?,
                    content_type: r.get("content_type")?,
                    fingerprint: r.get("fingerprint")?,
                },
                error: r
                    .get::<_, Option<String>>("analysis_error")?
                    .unwrap_or_default(),
                failed_at: r.get("analysis_failed_at")?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Single-track upsert, for tests; the scanner goes through
    /// [`Db::reconcile`].
    #[cfg(test)]
    pub fn insert_track(&self, t: &TrackInsert) -> Result<()> {
        self.conn
            .lock()
            .execute(UPSERT_TRACK_SQL, upsert_params(t))?;
        Ok(())
    }

    /// Every row, reduced to what the scanner reconciles against. Loaded once
    /// per scan so root membership can be decided by path component in Rust
    /// rather than by a `LIKE` prefix.
    pub fn track_index(&self) -> Result<Vec<IndexRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, content_type, mtime, missing_since, fingerprint FROM tracks",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(IndexRow {
                id: r.get(0)?,
                path: r.get(1)?,
                content_type: r.get(2)?,
                mtime: r.get(3)?,
                missing_since: r.get(4)?,
                fingerprint: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Apply a scan's changes atomically. For each new file, in order:
    ///
    /// 1. **Reattach** to a missing row with the same fingerprint (newest
    ///    first): only path, content type and mtime change, so everything the
    ///    operator did to the track — tags edited in the app included — stays.
    /// 2. **Duplicate** a present row with the same fingerprint (oldest
    ///    first): a new row that starts with a copy of that row's operator
    ///    state.
    /// 3. Otherwise insert it as a new track.
    pub fn reconcile(&self, change: &Reconcile) -> Result<Reconciled> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut done = Reconciled::default();
        for chunk in change.revive.chunks(500) {
            let sql = format!(
                "UPDATE tracks SET missing_since = NULL WHERE id IN ({})",
                vec!["?"; chunk.len()].join(",")
            );
            tx.execute(&sql, params_from_iter(chunk))?;
        }
        {
            let mut upsert = tx.prepare(UPSERT_TRACK_SQL)?;
            for t in &change.upserts {
                upsert.execute(upsert_params(t))?;
            }
        }
        for chunk in change.gone.chunks(500) {
            let sql = format!(
                "UPDATE tracks SET missing_since = ?1 \
                 WHERE missing_since IS NULL AND id IN ({})",
                (0..chunk.len())
                    .map(|i| format!("?{}", i + 2))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let params = std::iter::once(&change.now_ms).chain(chunk);
            done.missing += tx.execute(&sql, params_from_iter(params))?;
        }
        {
            let mut missing_twin = tx.prepare(
                "SELECT id FROM tracks WHERE fingerprint = ?1 AND missing_since IS NOT NULL \
                 ORDER BY missing_since DESC, id DESC LIMIT 1",
            )?;
            // A file back under a root of another content type is a different
            // class of material, so its automatic trio is requeued — the rule
            // a metadata reclassification applies. The trims stand until the
            // fresh result lands, being the same audio either way, but the
            // Next Start does not: it is derived only for music, and a jingle
            // carrying one hands over early on every airing until the pass
            // reaches it — for good, if its decode fails.
            let mut reattach = tx.prepare(
                "UPDATE tracks SET path = ?1, content_type = ?2, mtime = ?3, \
                        missing_since = NULL, \
                        analysis_error = NULL, analysis_failed_at = NULL, \
                        next_start_ms = CASE \
                          WHEN content_type <> ?2 AND auto_cue_state <> 'manual' \
                          THEN NULL ELSE next_start_ms END, \
                        auto_cue_state = CASE \
                          WHEN content_type <> ?2 AND auto_cue_state <> 'manual' \
                          THEN 'pending' ELSE auto_cue_state END \
                 WHERE id = ?4",
            )?;
            let mut present_twin = tx.prepare(
                "SELECT id FROM tracks WHERE fingerprint = ?1 AND missing_since IS NULL \
                 ORDER BY id LIMIT 1",
            )?;
            let mut insert = tx.prepare(&format!("{UPSERT_TRACK_SQL} RETURNING id"))?;
            // The twin's analysis — trio, ownership and provenance alike —
            // carries over only within one content type: the same file under
            // /music and /jingles is two separate jobs. Across classes the
            // trio is left NULL rather than inherited: file start, file end
            // and "wait until Cue Out" is the conservative reading, and a
            // music Next Start on a jingle would segue early on every airing
            // until the pass reaches it — for good, if its decode fails.
            // The fades are not inferred by anything, so they travel. So does
            // the level envelope: it measures the audio, which the twin shares,
            // and it carries no decision about either class.
            //
            // It is the one auto-cue column copied unconditionally, so a source
            // that has none nulls the target. That is only safe because this
            // runs source -> freshly inserted row; pointed at a row that has
            // already been analysed it would erase a good envelope.
            let mut copy_state = tx.prepare(
                "UPDATE tracks SET title = s.title, artist = s.artist, album = s.album, \
                        genre = s.genre, year = s.year, bpm = s.bpm, \
                        play_count = s.play_count, waveform = s.waveform, \
                        cue_in_ms = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.cue_in_ms ELSE NULL END, \
                        fade_in_ms = s.fade_in_ms, fade_out_ms = s.fade_out_ms, \
                        cue_out_ms = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.cue_out_ms ELSE NULL END, \
                        next_start_ms = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.next_start_ms ELSE NULL END, \
                        edited_fields = s.edited_fields, \
                        rg_gain = s.rg_gain, rg_peak = s.rg_peak, \
                        rg_measured_at = s.rg_measured_at, \
                        auto_cue_state = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.auto_cue_state ELSE 'pending' END, \
                        auto_cue_version = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.auto_cue_version ELSE NULL END, \
                        auto_cue_silence_db = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.auto_cue_silence_db ELSE NULL END, \
                        auto_cue_segue_db = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.auto_cue_segue_db ELSE NULL END, \
                        auto_cue_at = CASE WHEN s.content_type = tracks.content_type \
                          THEN s.auto_cue_at ELSE NULL END, \
                        auto_cue_levels = s.auto_cue_levels \
                 FROM (SELECT * FROM tracks WHERE id = ?1) AS s \
                 WHERE tracks.id = ?2",
            )?;
            for t in &change.new_files {
                let Some(fp) = &t.fingerprint else {
                    insert.query_row(upsert_params(t), |_| Ok(()))?;
                    done.inserted += 1;
                    continue;
                };
                let twin: Option<i64> = missing_twin.query_row([fp], |r| r.get(0)).optional()?;
                if let Some(id) = twin {
                    reattach.execute(params![t.path, t.content_type, t.mtime, id])?;
                    done.reattached += 1;
                    continue;
                }
                let source: Option<i64> = present_twin.query_row([fp], |r| r.get(0)).optional()?;
                let id: i64 = insert.query_row(upsert_params(t), |r| r.get(0))?;
                match source {
                    Some(source) => {
                        copy_state.execute(params![source, id])?;
                        done.duplicated += 1;
                    }
                    None => done.inserted += 1,
                }
            }
        }
        tx.commit()?;
        Ok(done)
    }

    /// Every missing row, newest first, with what a purge would destroy. The
    /// warning is about work the operator would have to do again: a derived
    /// trio comes back from the next analysis and does not count, while the
    /// fades — which nothing derives — always do, and so does a trio the
    /// operator took ownership of.
    pub fn missing_tracks(&self) -> Result<Vec<MissingRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, path, missing_since, play_count, \
                    (COALESCE(fade_in_ms, fade_out_ms) IS NOT NULL \
                     OR (auto_cue_state = 'manual' \
                         AND COALESCE(cue_in_ms, cue_out_ms, next_start_ms) IS NOT NULL)) \
             FROM tracks WHERE missing_since IS NOT NULL \
             ORDER BY missing_since DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MissingRow {
                id: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                artist: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                path: r.get(3)?,
                missing_since: r.get(4)?,
                play_count: r.get(5)?,
                has_cue_points: r.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Present rows sharing a fingerprint with another present row, ordered so
    /// each group is contiguous.
    pub fn fingerprint_twins(&self) -> Result<Vec<HealthRow>> {
        self.health_rows(
            "missing_since IS NULL AND fingerprint IN ( \
               SELECT fingerprint FROM tracks \
               WHERE missing_since IS NULL AND fingerprint IS NOT NULL \
               GROUP BY fingerprint HAVING COUNT(*) > 1) \
             ORDER BY fingerprint, id",
        )
    }

    /// Present music rows with both an artist and a title: the candidates for
    /// possible duplicates, which are grouped in Rust.
    pub fn tagged_music(&self) -> Result<Vec<HealthRow>> {
        self.health_rows(
            "missing_since IS NULL AND content_type = 'music' \
             AND COALESCE(artist, '') <> '' AND COALESCE(title, '') <> '' \
             ORDER BY id",
        )
    }

    fn health_rows(&self, filter: &str) -> Result<Vec<HealthRow>> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACK_COLUMNS}, path, content_type, fingerprint FROM tracks WHERE {filter}"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(HealthRow {
                track: row_to_track(r, policy)?,
                path: r.get("path")?,
                content_type: r.get("content_type")?,
                fingerprint: r.get("fingerprint")?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Present rows still waiting for the analysis pass to fingerprint them.
    /// A row whose analysis failed is not waiting.
    pub fn unhashed_count(&self) -> Result<i64> {
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks \
             WHERE missing_since IS NULL AND fingerprint IS NULL \
               AND analysis_failed_at IS NULL",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn dismissals(&self) -> Result<Vec<Dismissal>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT kind, key, value FROM health_dismissals")?;
        let rows = stmt.query_map([], |r| {
            Ok(Dismissal {
                kind: r.get(0)?,
                key: r.get(1)?,
                value: r.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn set_dismissal(&self, d: &Dismissal) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO health_dismissals (kind, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(kind, key) DO UPDATE SET value = excluded.value",
            params![d.kind, d.key, d.value],
        )?;
        Ok(())
    }

    pub fn delete_dismissals(&self, which: &[(String, String)]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut delete =
                tx.prepare("DELETE FROM health_dismissals WHERE kind = ?1 AND key = ?2")?;
            for (kind, key) in which {
                delete.execute(params![kind, key])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete the given rows for good — the only path that deletes tracks —
    /// skipping any that are not missing (a
    /// scan may have revived one since the operator chose it). Returns the ids
    /// actually deleted.
    ///
    /// Their airings keep their record and lose only the id: the null-out is
    /// explicit because `PRAGMA foreign_keys` is off, so an `ON DELETE` clause
    /// on `play_log` would never fire.
    pub fn purge_tracks(&self, ids: &[i64]) -> Result<Vec<i64>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut deleted = Vec::new();
        for chunk in ids.chunks(500) {
            let sql = format!(
                "DELETE FROM tracks WHERE missing_since IS NOT NULL AND id IN ({}) RETURNING id",
                vec!["?"; chunk.len()].join(",")
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk), |r| r.get::<_, i64>(0))?;
            for id in rows {
                deleted.push(id?);
            }
        }
        for chunk in deleted.chunks(500) {
            let sql = format!(
                "UPDATE play_log SET track_id = NULL WHERE track_id IN ({})",
                vec!["?"; chunk.len()].join(",")
            );
            tx.execute(&sql, params_from_iter(chunk))?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn increment_play_count(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET play_count = play_count + 1 WHERE id = ?",
            [id],
        )?;
        Ok(())
    }

    /// Record one airing: a `play_log` row and the track's play count, in one
    /// transaction so the two can never disagree.
    ///
    /// The logged artist, title and duration are read from the track row in the
    /// same statement that inserts, so nothing is passed in and a track purged
    /// between arming and airing simply logs nothing.
    pub fn record_airing(&self, id: i64, aired_at: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO play_log (track_id, aired_at, artist, title, duration) \
             SELECT id, ?1, artist, title, duration FROM tracks WHERE id = ?2",
            params![aired_at, id],
        )?;
        tx.execute(
            "UPDATE tracks SET play_count = play_count + 1 WHERE id = ?",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The last `limit` airings, oldest first, as the tracks they aired.
    ///
    /// Airings of tracks the library no longer has are left out: the row stays
    /// in the log for the record, but there is no track to show or requeue.
    pub fn recent_airings(&self, limit: i64) -> Result<Vec<Track>> {
        if limit <= 0 {
            return Ok(vec![]);
        }
        let ids: Vec<i64> = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT track_id FROM play_log WHERE track_id IS NOT NULL \
                 ORDER BY id DESC LIMIT ?",
            )?;
            let rows = stmt.query_map([limit], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
                .into_iter()
                .rev()
                .collect()
        };
        self.get_tracks_by_ids(&ids)
    }

    /// Rows whose stored envelope this build cannot use, as far as SQL can
    /// tell: absent, too short to hold the header, or a layout version it does
    /// not know.
    ///
    /// Deliberately not the whole of [`auto_cue::Envelope::decode`] — the codes
    /// cannot be screened in SQL, because a readable length depends on the
    /// track's duration. Every caller therefore treats this as a pre-filter and
    /// lets `decode` decide, so a row can never fall between the two.
    fn unreadable_levels() -> String {
        format!(
            "(auto_cue_levels IS NULL \
              OR length(auto_cue_levels) < {header} \
              OR substr(auto_cue_levels, 1, 1) <> x'{version:02x}')",
            header = auto_cue::LEVELS_HEADER_LEN,
            version = auto_cue::LEVELS_FORMAT_VERSION,
        )
    }

    /// Set what a derived cue set reports. Taken from the tuning config at
    /// startup and whenever the operator flips either switch; analysis runs and
    /// stores its result either way, so switching back on costs no second pass.
    pub fn set_auto_cue_policy(&self, apply: bool, apply_next_start: bool) {
        self.apply_auto_cue.store(apply, Ordering::Relaxed);
        self.apply_auto_next_start
            .store(apply_next_start, Ordering::Relaxed);
    }

    fn auto_cue_policy(&self) -> AutoCuePolicy {
        AutoCuePolicy {
            trio: self.apply_auto_cue.load(Ordering::Relaxed),
            next_start: self.apply_auto_next_start.load(Ordering::Relaxed),
        }
    }

    /// A track's content type, which [`Track`] does not carry — the class is a
    /// property of the Library path the file sits under, not of its tags.
    pub fn track_content_type(&self, id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT content_type FROM tracks WHERE id = ?", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Update metadata fields for a track. Only non-None fields are included
    /// in the UPDATE. A tag field whose value actually changes is flagged in
    /// `edited_fields`, so a rescan keeps it; the renderer sends every field, so
    /// an unchanged one must not be flagged. Returns the updated [`Track`] so
    /// the caller can push it to the renderer as a fast-forward replacement; the
    /// update path never touches `play_count`, `waveform`, or `added_at`.
    pub fn update_track_metadata(&self, updates: &TrackMetadataUpdate) -> Result<Track> {
        let policy = self.auto_cue_policy();
        use rusqlite::types::Value;

        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let current = tx
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?"),
                [updates.id],
                |r| row_to_track(r, policy),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("track not found"))?;

        let mut setters = Vec::<&str>::new();
        let mut params = Vec::<Value>::new();
        let mut edited = 0;
        let text = |v: &Option<String>| v.clone().map_or(Value::Null, Value::Text);

        if let Some(v) = &updates.title {
            setters.push("title=?");
            params.push(Value::Text(v.clone()));
            if *v != current.title {
                edited |= EditedFields::TITLE;
            }
        }
        if let Some(v) = &updates.artist {
            setters.push("artist=?");
            params.push(Value::Text(v.clone()));
            if *v != current.artist {
                edited |= EditedFields::ARTIST;
            }
        }
        if let Some(v) = &updates.album {
            setters.push("album=?");
            params.push(Value::Text(v.clone()));
            if *v != current.album {
                edited |= EditedFields::ALBUM;
            }
        }
        if let Some(v) = &updates.genre {
            setters.push("genre=?");
            params.push(text(v));
            if *v != current.genre {
                edited |= EditedFields::GENRE;
            }
        }
        if let Some(v) = updates.year {
            setters.push("year=?");
            params.push(v.map_or(Value::Null, Value::Integer));
            if v != current.year {
                edited |= EditedFields::YEAR;
            }
        }
        // A reclassification changes what automatic analysis would infer — a
        // music Next Start has no business surviving a move to jingle — so an
        // automatically owned track goes back in the queue. Its current markers
        // stay until the fresh result lands, and a manually owned one is left
        // alone entirely.
        let mut reclassified = false;
        if let Some(v) = &updates.content_type {
            let was: String = tx.query_row(
                "SELECT content_type FROM tracks WHERE id = ?",
                [updates.id],
                |r| r.get(0),
            )?;
            reclassified = *v != was;
            setters.push("content_type=?");
            params.push(Value::Text(v.clone()));
        }

        if setters.is_empty() {
            return Ok(current);
        }
        if reclassified {
            tx.execute(
                "UPDATE tracks SET auto_cue_state = 'pending' \
                 WHERE id = ? AND auto_cue_state <> 'manual'",
                [updates.id],
            )?;
        }

        setters.push("edited_fields = edited_fields | ?");
        params.push(Value::Integer(edited));
        params.push(Value::Integer(updates.id));
        tx.execute(
            &format!("UPDATE tracks SET {} WHERE id = ?", setters.join(", ")),
            params_from_iter(params.iter()),
        )?;
        let track = tx.query_row(
            &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?"),
            [updates.id],
            |r| row_to_track(r, policy),
        )?;
        tx.commit()?;
        Ok(track)
    }

    /// Replace the flagged tag columns with `parsed`, the file's own tags, and
    /// clear the flags.
    pub fn revert_track_tags(&self, id: i64, parsed: &TrackInsert) -> Result<Track> {
        let policy = self.auto_cue_policy();
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE tracks SET \
                title = CASE WHEN edited_fields & 1 THEN ?1 ELSE title END, \
                artist = CASE WHEN edited_fields & 2 THEN ?2 ELSE artist END, \
                album = CASE WHEN edited_fields & 4 THEN ?3 ELSE album END, \
                genre = CASE WHEN edited_fields & 8 THEN ?4 ELSE genre END, \
                year = CASE WHEN edited_fields & 16 THEN ?5 ELSE year END, \
                mtime = ?6, edited_fields = 0 \
             WHERE id = ?7",
            params![
                parsed.title,
                parsed.artist,
                parsed.album,
                parsed.genre,
                parsed.year,
                parsed.mtime,
                id
            ],
        )?;
        if n == 0 {
            anyhow::bail!("track not found");
        }
        Ok(conn.query_row(
            &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?"),
            [id],
            |r| row_to_track(r, policy),
        )?)
    }

    /// A present track's tag columns, as a write-back reads them.
    pub fn tag_values(&self, id: i64) -> Result<Option<TagValues>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT path, title, artist, album, genre, year, fingerprint, edited_fields \
             FROM tracks WHERE id = ? AND missing_since IS NULL",
            [id],
            |r| {
                Ok(TagValues {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    album: r.get(3)?,
                    genre: r.get(4)?,
                    year: r.get(5)?,
                    fingerprint: r.get(6)?,
                    edited_fields: r.get(7)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Record a finished write-back: `written` is now in the file, whose mtime
    /// is `mtime`. The flags clear only while the row still holds exactly what
    /// was written, so an edit made during the write keeps its flags. Returns
    /// whether the row matched.
    pub fn finish_tag_write(&self, id: i64, written: &TagValues, mtime: i64) -> Result<bool> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE tracks SET edited_fields = 0, mtime = ?1 \
             WHERE id = ?2 AND path = ?3 AND title IS ?4 AND artist IS ?5 \
               AND album IS ?6 AND genre IS ?7 AND year IS ?8",
            params![
                mtime,
                id,
                written.path,
                written.title,
                written.artist,
                written.album,
                written.genre,
                written.year
            ],
        )?;
        Ok(n > 0)
    }

    /// Random tracks of one content type, honouring `filter`.
    ///
    /// Filtering happens in SQL, so one query returns exactly `count` rows of
    /// eligible material — no over-fetch, no post-filter, and with
    /// `spread_artists` no artist twice inside the block it returns.
    pub fn get_random_tracks(
        &self,
        content_type: &str,
        count: i64,
        filter: &SelectionFilter,
    ) -> Result<Vec<Track>> {
        let policy = self.auto_cue_policy();
        if count <= 0 {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let where_clause = filter.sql();
        let sql = if filter.spread_artists {
            // One row per artist, and that row picked at random rather than by
            // rowid, or an artist's first-added track would be the only one
            // this path ever returns.
            format!(
                "SELECT * FROM tracks WHERE id IN ( \
                   SELECT id FROM ( \
                     SELECT id, ROW_NUMBER() OVER (PARTITION BY {key} ORDER BY RANDOM()) AS rn \
                     FROM tracks WHERE missing_since IS NULL AND content_type = ?{where_clause} \
                   ) WHERE rn = 1 ORDER BY RANDOM() LIMIT ? \
                 ) ORDER BY RANDOM()",
                key = SelectionFilter::SPREAD_KEY
            )
        } else {
            format!(
                "SELECT * FROM tracks WHERE missing_since IS NULL AND content_type = ?{where_clause} \
                 ORDER BY RANDOM() LIMIT ?"
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(
            std::iter::once(rusqlite::types::Value::Text(content_type.to_owned()))
                .chain(filter.params())
                .chain(std::iter::once(rusqlite::types::Value::Integer(count))),
        );
        let rows = stmt.query_map(params, |r| row_to_track(r, policy))?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn pick_random_from_bottom(
        &self,
        content_type: &str,
        count: i64,
        bucket_size: i64,
        exclude_ids: &[i64],
    ) -> Result<Vec<Track>> {
        let policy = self.auto_cue_policy();
        if bucket_size <= 0 || count <= 0 {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let exclude_clause = exclude_sql(exclude_ids);
        let sql = format!(
            "WITH bucket AS ( \
                SELECT * FROM tracks \
                WHERE missing_since IS NULL AND content_type = ?{exclude_clause} \
                ORDER BY play_count ASC, RANDOM() LIMIT ? \
            ) SELECT * FROM bucket ORDER BY RANDOM() LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(
            std::iter::once(rusqlite::types::Value::Text(content_type.to_owned()))
                .chain(
                    exclude_ids
                        .iter()
                        .map(|&id| rusqlite::types::Value::Integer(id)),
                )
                .chain([
                    rusqlite::types::Value::Integer(bucket_size),
                    rusqlite::types::Value::Integer(count),
                ]),
        );
        let rows = stmt.query_map(params, |r| row_to_track(r, policy))?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_stats(&self) -> Result<LibraryStats> {
        let conn = self.conn.lock();
        let (total_tracks, total_artists, total_albums, total_hours): (i64, i64, i64, f64) = conn
            .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT artist), COUNT(DISTINCT album), \
                 COALESCE(ROUND(SUM(duration) / 3600.0, 1), 0) FROM tracks \
                 WHERE missing_since IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut tracks_by_type = TracksByType::default();
        let mut stmt =
            conn.prepare("SELECT content_type, COUNT(*) FROM tracks WHERE missing_since IS NULL GROUP BY content_type")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (kind, n) = row?;
            match kind.as_str() {
                "music" => tracks_by_type.music = n,
                "commercial" => tracks_by_type.commercial = n,
                "jingle" => tracks_by_type.jingle = n,
                _ => {}
            }
        }
        Ok(LibraryStats {
            total_tracks,
            total_artists,
            total_albums,
            total_hours,
            tracks_by_type,
        })
    }
}

/// Bind params for [`UPSERT_TRACK_SQL`], in column order. Shared by the single
/// and batch insert paths so the two never drift.
fn upsert_params(t: &TrackInsert) -> [&dyn rusqlite::ToSql; 14] {
    [
        &t.path,
        &t.content_type,
        &t.title,
        &t.artist,
        &t.album,
        &t.genre,
        &t.year,
        &t.duration,
        &t.bpm,
        &t.sample_rate,
        &t.bitrate,
        &t.format,
        &t.mtime,
        &t.fingerprint,
    ]
}

/// Wrap one operator-typed word as an FTS5 prefix term.
///
/// The word is quoted so that punctuation inside it is tokenised rather than
/// read as query syntax — `AC/DC` becomes the phrase `ac dc` rather than a
/// syntax error. Embedded double quotes are doubled, which is the escape the
/// FTS5 string grammar defines: left alone, a quote closes the phrase early
/// and strands the trailing `*` inside an unterminated string, which SQLite
/// rejects. Inside a quoted string no other character carries meaning, so this
/// is the whole escape.
///
/// The result is bound as a parameter, so SQL quoting is a separate concern
/// handled by rusqlite; this escapes only the FTS5 query language.
fn fts5_prefix_term(word: &str) -> String {
    format!("\"{}\"*", word.replace('"', "\"\""))
}

/// Build a ` AND id NOT IN (?, ?, ...)` fragment with one placeholder per
/// excluded id, or an empty string when there is nothing to exclude (SQLite
/// rejects an empty `NOT IN ()`). Placeholders are bound separately, so the
/// ids never reach the SQL string.
fn exclude_sql(exclude_ids: &[i64]) -> String {
    if exclude_ids.is_empty() {
        return String::new();
    }
    let placeholders = vec!["?"; exclude_ids.len()].join(", ");
    format!(" AND id NOT IN ({placeholders})")
}

fn order_clause(sort_by: Option<&str>, sort_dir: Option<&str>) -> Option<String> {
    let col = sort_by?;
    if !matches!(col, "title" | "artist" | "album" | "play_count") {
        return None;
    }
    let dir = if matches!(sort_dir, Some("desc")) {
        "DESC"
    } else {
        "ASC"
    };
    let collate = if col == "play_count" {
        ""
    } else {
        "COLLATE NOCASE "
    };
    Some(format!("{} {}{}", col, collate, dir))
}

/// What [`row_to_track`] reads, for queries that must not drag the waveform
/// blob along with `SELECT *`.
const TRACK_COLUMNS: &str = "id, title, artist, album, duration, play_count, genre, year, bpm, \
     sample_rate, bitrate, format, cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms, \
     auto_cue_state, edited_fields";

/// The markers as they take effect. A derived trio is held back while the
/// feature is switched off — the row keeps its result, and nothing downstream
/// sees it — so the station airs the whole file until it is switched on again.
/// A manually prepared trio is never held back, and neither are the fades:
/// nothing infers those, so they are the operator's either way.
/// The cue points the library reports for a row: the stored set with whatever
/// the [`AutoCuePolicy`] holds back. The one gate both switches run through.
fn effective_cue_points(row: &Row, policy: AutoCuePolicy) -> rusqlite::Result<CuePoints> {
    let points = row_to_cue_points(row)?;
    // `manual`, not `auto`, is the test: a requeued track keeps the trio it was
    // last given while its state goes back to `pending`, and that trio is still
    // analysis's. Only an operator save reaches `manual`, so a row that is not
    // `manual` holds nothing of theirs.
    if row.get::<_, String>("auto_cue_state")? == "manual" {
        return Ok(points);
    }
    if !policy.trio {
        return Ok(CuePoints {
            cue_in_ms: None,
            cue_out_ms: None,
            next_start_ms: None,
            ..points
        });
    }
    if !policy.next_start {
        return Ok(CuePoints {
            next_start_ms: None,
            ..points
        });
    }
    Ok(points)
}

fn row_to_track(row: &Row, policy: AutoCuePolicy) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get("id")?,
        title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
        artist: row.get::<_, Option<String>>("artist")?.unwrap_or_default(),
        album: row.get::<_, Option<String>>("album")?.unwrap_or_default(),
        duration: row.get::<_, Option<f64>>("duration")?.unwrap_or(0.0),
        play_count: row.get("play_count")?,
        genre: row.get("genre")?,
        year: row.get("year")?,
        bpm: row.get("bpm")?,
        sample_rate: row.get("sample_rate")?,
        bitrate: row.get("bitrate")?,
        format: row.get("format")?,
        cue_points: effective_cue_points(row, policy)?,
        edited_fields: row.get("edited_fields")?,
    })
}

/// The furthest marker a set holds, as a lower bound on the file's length.
fn furthest(points: &CuePoints) -> Option<i64> {
    [
        points.cue_in_ms,
        points.fade_in_ms,
        points.fade_out_ms,
        points.cue_out_ms,
        points.next_start_ms,
    ]
    .into_iter()
    .flatten()
    .max()
}

/// Read the five marker columns off a row that selected them by name.
fn row_to_cue_points(row: &Row) -> rusqlite::Result<CuePoints> {
    Ok(CuePoints {
        cue_in_ms: row.get("cue_in_ms")?,
        fade_in_ms: row.get("fade_in_ms")?,
        fade_out_ms: row.get("fade_out_ms")?,
        cue_out_ms: row.get("cue_out_ms")?,
        next_start_ms: row.get("next_start_ms")?,
    })
}

/// Identifies a database created by the current schema baseline, stored in
/// `PRAGMA application_id` ("RDJ1"). Pre-1.0 the schema may be squashed into a
/// new baseline: bump this, move the old value into [`LEGACY_EPOCHS`], and every
/// older database is reset on its next open.
const DB_EPOCH: i32 = 0x5244_4a31;

/// Epochs this build knows to be older than [`DB_EPOCH`]. `0` is every
/// database written before epochs existed. Any other foreign value is treated as
/// newer and refused, so an old build never resets a newer library.
const LEGACY_EPOCHS: &[i32] = &[0];

/// How many pre-migration backups to keep beside the database.
const KEPT_BACKUPS: usize = 2;

/// The schema, as an append-only list. Never edit a step that has shipped: add
/// a new one and regenerate `schema.sql` (see `docs/database.md`).
const MIGRATION_STEPS: &[M] = &[
    M::up(BASELINE),
    M::up(HEALTH_DISMISSALS),
    M::up(EDITED_FIELDS),
    M::up(ANALYSIS_FAILURES),
    M::up(PLAY_LOG),
    M::up(LOUDNESS),
    M::up(AUTO_CUE),
    M::up(AUTO_CUE_LEVELS),
];
const MIGRATIONS: Migrations = Migrations::from_slice(MIGRATION_STEPS);

fn schema_version() -> usize {
    MIGRATION_STEPS.len()
}

/// Epoch-1 baseline.
///
/// - The operator-work columns (`play_count`, `waveform`, the five cue
///   markers) are absent from `UPSERT_TRACK_SQL`, so a rescan cannot destroy
///   them. The markers are milliseconds from the start of the file and `NULL`
///   means "no adjustment".
/// - `fingerprint` identifies the audio independently of path and tags;
///   `missing_since` (unix ms) marks a row whose file is gone. Only present
///   rows are held to a unique path, so a missing row can keep its old path
///   while a different file takes it.
/// - `tracks_au` fires only when an indexed column changes, not on every
///   waveform write or play count bump.
const BASELINE: &str = r#"
PRAGMA application_id = 1380207153;

CREATE TABLE tracks (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  path          TEXT NOT NULL,
  content_type  TEXT NOT NULL DEFAULT 'music'
                CHECK (content_type IN ('music', 'jingle', 'commercial')),
  title         TEXT,
  artist        TEXT,
  album         TEXT,
  genre         TEXT,
  year          INTEGER,
  duration      REAL,
  bpm           REAL,
  sample_rate   INTEGER,
  bitrate       INTEGER,
  format        TEXT,
  mtime         INTEGER,
  play_count    INTEGER NOT NULL DEFAULT 0,
  added_at      TEXT DEFAULT (datetime('now')),
  waveform      BLOB,
  cue_in_ms     INTEGER,
  fade_in_ms    INTEGER,
  fade_out_ms   INTEGER,
  cue_out_ms    INTEGER,
  next_start_ms INTEGER,
  fingerprint   TEXT,
  missing_since INTEGER
);

CREATE UNIQUE INDEX tracks_path_present ON tracks(path) WHERE missing_since IS NULL;
CREATE INDEX tracks_fingerprint ON tracks(fingerprint) WHERE fingerprint IS NOT NULL;

CREATE VIRTUAL TABLE tracks_fts USING fts5(
  title, artist, album, genre,
  content='tracks',
  content_rowid='id'
);

CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;

CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
END;

CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album, genre ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;
"#;

/// Step 2: the operator's dismissals of library health findings. Stored beside
/// the tracks because they name track ids, so a reset replaces both at once.
const HEALTH_DISMISSALS: &str = r#"
CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing')),
  key   TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, key)
);
"#;

/// Step 3: which tag columns the operator edited in the app, as
/// [`EditedFields`] bits. A rescan keeps an edited column.
const EDITED_FIELDS: &str = r#"
ALTER TABLE tracks ADD COLUMN edited_fields INTEGER NOT NULL DEFAULT 0;
"#;

/// Step 4: why the analysis pass could not decode a file, and when. Cleared
/// when a scan sees the file change, so the pass tries it again.
const ANALYSIS_FAILURES: &str = r#"
ALTER TABLE tracks ADD COLUMN analysis_error TEXT;
ALTER TABLE tracks ADD COLUMN analysis_failed_at INTEGER;
"#;

/// Step 5: what went on air, and when. Append-only — the station's record of
/// its own broadcast, which is why the row carries the artist, title and
/// duration as they read at air time rather than only a track id: a purge or a
/// later tag fix must not rewrite the past. `track_id` is a convenience for
/// requeueing, not the identity of the row, and is nulled when the track is
/// purged (`purge_tracks`) — there is no foreign key, since
/// `PRAGMA foreign_keys` is off and an `ON DELETE` clause would never fire.
const PLAY_LOG: &str = r#"
CREATE TABLE play_log (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER,
  aired_at INTEGER NOT NULL,
  artist   TEXT,
  title    TEXT,
  duration REAL
);

CREATE INDEX play_log_aired ON play_log(aired_at);
"#;

/// Step 6: what the track measured, for ReplayGain. Written only by the
/// analysis pass, never by the metadata scan, so `UPSERT_TRACK_SQL` does not
/// mention them and a rescan cannot clear a measurement.
///
/// `rg_measured_at` (unix ms) is what "already measured" means, not a non-null
/// gain: a silent or very short file measures successfully and legitimately
/// has no gain, and keying off the gain alone would put it back in the queue
/// on every pass forever.
const LOUDNESS: &str = r#"
ALTER TABLE tracks ADD COLUMN rg_gain REAL;
ALTER TABLE tracks ADD COLUMN rg_peak REAL;
ALTER TABLE tracks ADD COLUMN rg_measured_at INTEGER;
"#;

/// Step 7: who owns Cue In / Cue Out / Next Start, and how an automatic
/// result was produced.
///
/// `auto_cue_state` is the whole ownership model: `pending` has never been
/// analysed, `auto` was written by the analysis pass and may be replaced by it,
/// `manual` was authored by the operator and is never touched again. Existing
/// rows that already carry one of the three markers start `manual` — they can
/// only have got them from the operator, and analysis must not overwrite that
/// work. A row with only fades set stays `pending`, since a fade edit does not
/// take ownership of the trio.
///
/// The provenance columns exist for inspection and for a future *explicit*
/// recalculation. Reading them never schedules work: changing a threshold
/// affects later analyses and nothing already stored.
const AUTO_CUE: &str = r#"
ALTER TABLE tracks ADD COLUMN auto_cue_state TEXT NOT NULL DEFAULT 'pending'
  CHECK (auto_cue_state IN ('pending', 'auto', 'manual'));
ALTER TABLE tracks ADD COLUMN auto_cue_version INTEGER;
ALTER TABLE tracks ADD COLUMN auto_cue_silence_db REAL;
ALTER TABLE tracks ADD COLUMN auto_cue_segue_db REAL;
ALTER TABLE tracks ADD COLUMN auto_cue_at INTEGER;

UPDATE tracks SET auto_cue_state = 'manual'
 WHERE cue_in_ms IS NOT NULL OR cue_out_ms IS NOT NULL OR next_start_ms IS NOT NULL;
"#;

/// The decode reduced to the level of each window, so a later threshold change
/// can re-derive a track's markers without reading the file again. See
/// `audio::auto_cue::Envelope`.
///
/// Existing rows get it by backfill through the ordinary analysis pass, not by
/// a synchronous migration: it is a full decode per track. Until a row has one,
/// a recalculation sends it back through that pass instead.
///
/// `ADD COLUMN` is the cheap form — SQLite rewrites the stored schema and
/// nothing else, so this is O(1) whatever the library size. Existing records
/// simply hold fewer fields than the schema declares and read back as `NULL`,
/// which is exactly the state the backfill screens for. The migration and the
/// backfill are therefore the same mechanism, which is why 12,000 rows do not
/// have to be touched to add the column.
const AUTO_CUE_LEVELS: &str = r#"
ALTER TABLE tracks ADD COLUMN auto_cue_levels BLOB;
"#;

/// Upsert one present track's metadata by path. The operator-work columns are
/// deliberately absent: the waveform is filled asynchronously by the waveform
/// worker (`set_waveform`), and cue points and play counts are operator work a
/// metadata rescan must never clobber. A tag column flagged in
/// `edited_fields` keeps its value. A recorded analysis failure is cleared,
/// since the upsert only runs for a file that changed.
///
/// The level table goes, unlike the markers: it measures audio that is no
/// longer there, and a recalculation reading it would derive from a file that
/// has been replaced. The markers stand until a fresh result lands, which is
/// the existing rule — old positions beat none while the pass catches up.
const UPSERT_TRACK_SQL: &str = "INSERT INTO tracks \
     (path, content_type, title, artist, album, genre, year, duration, bpm, \
      sample_rate, bitrate, format, mtime, fingerprint) \
     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
     ON CONFLICT(path) WHERE missing_since IS NULL DO UPDATE SET \
        content_type=excluded.content_type, \
        title=CASE WHEN edited_fields & 1 THEN title ELSE excluded.title END, \
        artist=CASE WHEN edited_fields & 2 THEN artist ELSE excluded.artist END, \
        album=CASE WHEN edited_fields & 4 THEN album ELSE excluded.album END, \
        genre=CASE WHEN edited_fields & 8 THEN genre ELSE excluded.genre END, \
        year=CASE WHEN edited_fields & 16 THEN year ELSE excluded.year END, \
        duration=excluded.duration, \
        bpm=excluded.bpm, sample_rate=excluded.sample_rate, \
        bitrate=excluded.bitrate, format=excluded.format, mtime=excluded.mtime, \
        fingerprint=COALESCE(excluded.fingerprint, fingerprint), \
        auto_cue_state=CASE auto_cue_state WHEN 'auto' THEN 'pending' \
                       ELSE auto_cue_state END, \
        auto_cue_levels=NULL, \
        analysis_error=NULL, analysis_failed_at=NULL";

/// A database this build must not touch.
#[derive(Debug, PartialEq)]
pub enum OpenError {
    /// Written by a newer build: migrated past what this build knows, or
    /// stamped with an epoch it does not recognise.
    TooNew { found: usize, supported: usize },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::TooNew { found, supported } => write!(
                f,
                "the library database was written by a newer RadiodioDJ \
                 (schema {found}, this version supports {supported})"
            ),
        }
    }
}

impl std::error::Error for OpenError {}

pub struct Opened {
    pub db: Db,
    /// Where a pre-epoch database was moved, when opening reset the library.
    pub reset_backup: Option<PathBuf>,
}

fn user_version(conn: &Connection) -> Result<usize> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(usize::try_from(v).unwrap_or(0))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("library");
    path.with_file_name(format!("{stem}.{suffix}"))
}

/// Move a database from an older epoch aside, so a fresh one is created in its
/// place. Returns the backup path when that happened.
fn retire_legacy(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let (version, epoch) = {
        let conn = Connection::open(path).context("open sqlite")?;
        let version = user_version(&conn)?;
        let epoch: i32 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if version == 0 || epoch == DB_EPOCH {
            return Ok(None);
        }
        if !LEGACY_EPOCHS.contains(&epoch) {
            return Err(OpenError::TooNew {
                found: version,
                supported: schema_version(),
            }
            .into());
        }
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        (version, epoch)
    };
    let backup = sibling(path, &format!("legacy-v{version}.bak.db"));
    std::fs::rename(path, &backup).context("move legacy library aside")?;
    for ext in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(ext);
        let _ = std::fs::remove_file(PathBuf::from(side));
    }
    log::warn!(
        "library database from epoch {epoch:#x} (schema {version}) moved to {}; starting fresh",
        backup.display()
    );
    Ok(Some(backup))
}

/// Copy the database before migrating it, keeping the newest few copies.
fn backup(conn: &Connection, path: &Path, version: usize) -> Result<()> {
    let target = sibling(path, &format!("v{version}.bak.db"));
    let _ = std::fs::remove_file(&target);
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy()])
        .context("back up library before migrating")?;
    log::info!("library backed up to {}", target.display());

    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) else {
        return Ok(());
    };
    let prefix = format!("{stem}.v");
    let mut copies: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let v = name.strip_prefix(&prefix)?.strip_suffix(".bak.db")?;
            Some((v.parse().ok()?, e.path()))
        })
        .collect();
    copies.sort_by_key(|c| std::cmp::Reverse(c.0));
    for (_, old) in copies.into_iter().skip(KEPT_BACKUPS) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::auto_cue::{self, AutoCue, Thresholds};

    const THRESHOLDS: Thresholds = Thresholds {
        silence_dbfs: -70.0,
        segue_dbfs: -20.0,
    };
    use rusqlite::params;

    fn insert(db: &Db, path: &str, title: &str, artist: &str, album: &str, content_type: &str) {
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO tracks (path, content_type, title, artist, album, duration, play_count) \
             VALUES (?, ?, ?, ?, ?, 100.0, 0)",
            params![path, content_type, title, artist, album],
        )
        .unwrap();
    }

    fn insert_with_play_count(db: &Db, path: &str, content_type: &str, play_count: i64) {
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO tracks (path, content_type, title, artist, album, duration, play_count) \
             VALUES (?, ?, 't', 'a', 'al', 100.0, ?)",
            params![path, content_type, play_count],
        )
        .unwrap();
    }

    fn application_id(conn: &Connection) -> i32 {
        conn.pragma_query_value(None, "application_id", |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrations_are_valid() {
        MIGRATIONS.validate().unwrap();
    }

    #[test]
    fn a_fresh_database_is_stamped_with_the_epoch_and_latest_version() {
        assert_eq!(DB_EPOCH, 1_380_207_153, "BASELINE stamps this literal");
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn.lock();
        assert_eq!(user_version(&conn).unwrap(), schema_version());
        assert_eq!(application_id(&conn), DB_EPOCH);
    }

    /// The checked-in `schema.sql` is what a fresh database looks like, so a
    /// schema change shows up in review. `UPDATE_SCHEMA=1` rewrites it.
    #[test]
    fn schema_matches_snapshot() {
        let db = Db::open_in_memory().unwrap();
        let actual = {
            let conn = db.conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT sql FROM sqlite_master \
                     WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
                     ORDER BY type, name",
                )
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            let mut out: String = rows.map(|r| format!("{};\n\n", r.unwrap())).collect();
            out.truncate(out.trim_end().len());
            out.push('\n');
            out
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/library/schema.sql");
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(&path, &actual).unwrap();
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "schema changed: rerun with UPDATE_SCHEMA=1 and commit schema.sql"
        );
    }

    /// Seeds one representative row into a database at schema version `i + 1`.
    /// Append one whenever a migration step is appended.
    fn seed_track(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO tracks (path, content_type, title, artist, album, duration, \
                                 play_count, waveform, cue_in_ms, fingerprint) \
             VALUES ('/seed.mp3', 'music', 'Seed', 'A', 'B', 100.0, 3, x'00ff', 1000, 'v1:ab')",
        )
        .unwrap();
    }

    fn seed_dismissal(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO health_dismissals (kind, key, value) VALUES ('exact', 'v1:ab', '1,2')",
        )
        .unwrap();
    }

    const SEEDS: &[fn(&Connection)] = &[
        seed_track,
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch("UPDATE tracks SET edited_fields = 1")
                .unwrap();
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch(
                "UPDATE tracks SET edited_fields = 1, \
                        analysis_error = 'bad', analysis_failed_at = 5",
            )
            .unwrap();
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch(
                "UPDATE tracks SET edited_fields = 1, \
                        analysis_error = 'bad', analysis_failed_at = 5; \
                 INSERT INTO play_log (track_id, aired_at, artist, title, duration) \
                 SELECT id, 1000, artist, title, duration FROM tracks",
            )
            .unwrap();
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch(
                "UPDATE tracks SET edited_fields = 1, \
                        analysis_error = 'bad', analysis_failed_at = 5, \
                        rg_gain = -6.5, rg_peak = 0.98, rg_measured_at = 7; \
                 INSERT INTO play_log (track_id, aired_at, artist, title, duration) \
                 SELECT id, 1000, artist, title, duration FROM tracks",
            )
            .unwrap();
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch(
                "UPDATE tracks SET edited_fields = 1, \
                        analysis_error = 'bad', analysis_failed_at = 5, \
                        rg_gain = -6.5, rg_peak = 0.98, rg_measured_at = 7, \
                        auto_cue_state = 'manual'; \
                 INSERT INTO play_log (track_id, aired_at, artist, title, duration) \
                 SELECT id, 1000, artist, title, duration FROM tracks",
            )
            .unwrap();
        },
        |conn| {
            seed_track(conn);
            seed_dismissal(conn);
            conn.execute_batch(
                "UPDATE tracks SET edited_fields = 1, \
                        analysis_error = 'bad', analysis_failed_at = 5, \
                        rg_gain = -6.5, rg_peak = 0.98, rg_measured_at = 7, \
                        auto_cue_state = 'manual', auto_cue_levels = x'00'; \
                 INSERT INTO play_log (track_id, aired_at, artist, title, duration) \
                 SELECT id, 1000, artist, title, duration FROM tracks",
            )
            .unwrap();
        },
    ];

    /// Operator work written at any schema version survives every later step.
    #[test]
    fn every_step_preserves_seeded_rows() {
        assert_eq!(
            SEEDS.len(),
            schema_version(),
            "add a seed for the new migration step"
        );
        for version in 1..=schema_version() {
            let mut conn = Connection::open_in_memory().unwrap();
            MIGRATIONS.to_version(&mut conn, version).unwrap();
            SEEDS[version - 1](&conn);
            MIGRATIONS.to_latest(&mut conn).unwrap();
            let db = Db {
                conn: Mutex::new(conn),
                apply_auto_cue: AtomicBool::new(true),
                apply_auto_next_start: AtomicBool::new(true),
            };
            let id = only_id(&db);
            let track = db.get_track(id).unwrap().unwrap();
            assert_eq!(track.title, "Seed", "seeded at v{version}");
            assert_eq!(track.play_count, 3, "seeded at v{version}");
            assert_eq!(
                track.cue_points.cue_in_ms,
                Some(1000),
                "seeded at v{version}"
            );
            assert_eq!(db.get_waveform(id).unwrap(), Some(vec![0x00, 0xff]));
            let edited = if version >= 3 { EditedFields::TITLE } else { 0 };
            assert_eq!(track.edited_fields, edited, "seeded at v{version}");
            let aired = if version >= 5 { vec![id] } else { vec![] };
            assert_eq!(
                db.recent_airings(10)
                    .unwrap()
                    .iter()
                    .map(|t| t.id)
                    .collect::<Vec<_>>(),
                aired,
                "seeded at v{version}"
            );
        }
    }

    /// A database from before epochs existed, shaped like the released 0.17.0
    /// schema, with WAL on as the app leaves it.
    fn write_legacy_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE tracks (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               path TEXT UNIQUE NOT NULL,
               content_type TEXT NOT NULL DEFAULT 'music',
               title TEXT, play_count INTEGER NOT NULL DEFAULT 0,
               mtime INTEGER, waveform BLOB
             );
             INSERT INTO tracks (path, title) VALUES ('/old.mp3', 'Old');
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    #[test]
    fn a_legacy_database_is_moved_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        write_legacy_db(&path);

        let opened = Db::open(&path).unwrap();

        let backup = opened.reset_backup.expect("reset reported");
        assert_eq!(backup, dir.path().join("radiodiodj.legacy-v3.bak.db"));
        let old = Connection::open(&backup).unwrap();
        let title: String = old
            .query_row("SELECT title FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Old");
        assert_eq!(opened.db.get_stats().unwrap().total_tracks, 0);
        let conn = opened.db.conn.lock();
        assert_eq!(application_id(&conn), DB_EPOCH);
        assert_eq!(user_version(&conn).unwrap(), schema_version());
    }

    #[test]
    fn reopening_a_current_database_neither_resets_nor_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        let first = Db::open(&path).unwrap();
        first
            .db
            .insert_track(&TrackInsert {
                path: "/a.mp3".into(),
                content_type: "music".into(),
                ..Default::default()
            })
            .unwrap();
        drop(first);

        let again = Db::open(&path).unwrap();
        assert!(again.reset_backup.is_none());
        assert_eq!(again.db.get_stats().unwrap().total_tracks, 1);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(!names.iter().any(|n| n.contains(".bak.")), "{names:?}");
    }

    fn assert_refused_untouched(path: &Path) {
        let before = std::fs::read(path).unwrap();
        let err = Db::open(path).err().expect("refused");
        assert!(
            matches!(
                err.downcast_ref::<OpenError>(),
                Some(OpenError::TooNew { .. })
            ),
            "{err:#}"
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE t (x); PRAGMA application_id = 1380207153; PRAGMA user_version = 99;",
            )
            .unwrap();
        assert_refused_untouched(&path);
    }

    #[test]
    fn a_database_from_an_unknown_epoch_is_refused_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE t (x); PRAGMA application_id = 7; PRAGMA user_version = 1;",
            )
            .unwrap();
        assert_refused_untouched(&path);
    }

    #[test]
    fn backups_keep_only_the_newest_copies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);")
            .unwrap();
        for version in [1, 2, 3] {
            backup(&conn, &path, version).unwrap();
        }
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.contains(".bak."))
            .collect();
        names.sort();
        assert_eq!(names, ["radiodiodj.v2.bak.db", "radiodiodj.v3.bak.db"]);
        let copy = Connection::open(dir.path().join("radiodiodj.v3.bak.db")).unwrap();
        let x: i64 = copy.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 1);
    }

    #[test]
    fn missing_rows_are_hidden_but_readable_by_id() {
        let db = Db::open_in_memory().unwrap();
        insert_with_play_count(&db, "/gone.mp3", "music", 0);
        insert_with_play_count(&db, "/here.mp3", "music", 0);
        let ids: Vec<i64> = db.track_index().unwrap().iter().map(|r| r.id).collect();
        let (gone, here) = (ids[0], ids[1]);
        let mark = |now_ms| {
            db.reconcile(&Reconcile {
                gone: vec![gone],
                now_ms,
                ..Default::default()
            })
            .unwrap()
            .missing
        };
        assert_eq!(mark(1_000), 1);
        assert_eq!(mark(2_000), 0, "first mark wins");

        let only_here = |tracks: Vec<Track>| tracks.iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(only_here(db.search("", None, None, None).unwrap()), [here]);
        assert_eq!(only_here(db.search("t", None, None, None).unwrap()), [here]);
        assert_eq!(
            only_here(db.search("t", Some("music"), None, None).unwrap()),
            [here]
        );
        assert_eq!(
            only_here(
                db.get_random_tracks("music", 10, &SelectionFilter::default())
                    .unwrap()
            ),
            [here]
        );
        assert_eq!(
            only_here(db.pick_random_from_bottom("music", 10, 10, &[]).unwrap()),
            [here]
        );
        assert_eq!(db.get_stats().unwrap().total_tracks, 1);
        assert_eq!(db.get_stats().unwrap().tracks_by_type.music, 1);
        assert_eq!(
            db.get_paths_by_ids(&[gone, here]).unwrap(),
            [(here, "/here.mp3".to_string())]
        );
        let unfilled: Vec<i64> = db
            .tracks_needing_analysis()
            .unwrap()
            .iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(unfilled, [here]);

        assert!(db.get_track(gone).unwrap().is_some());
        assert!(db.get_track_load_info(gone).unwrap().is_some());
        assert_eq!(db.get_tracks_by_ids(&[gone]).unwrap().len(), 1);

        db.reconcile(&Reconcile {
            revive: vec![gone],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(db.get_stats().unwrap().total_tracks, 2);
    }

    fn new_file(path: &str, fingerprint: &str) -> TrackInsert {
        TrackInsert {
            path: path.into(),
            content_type: "music".into(),
            title: Some(path.into()),
            fingerprint: Some(fingerprint.into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_most_recently_missing_twin_reattaches() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/old.mp3", "v1:x")).unwrap();
        db.insert_track(&new_file("/older.mp3", "v1:x")).unwrap();
        let ids: Vec<i64> = db.track_index().unwrap().iter().map(|r| r.id).collect();
        let (old, older) = (ids[0], ids[1]);
        for (id, now_ms) in [(older, 1_000), (old, 2_000)] {
            db.reconcile(&Reconcile {
                gone: vec![id],
                now_ms,
                ..Default::default()
            })
            .unwrap();
        }

        let done = db
            .reconcile(&Reconcile {
                new_files: vec![new_file("/new.mp3", "v1:x")],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(done.reattached, 1);
        let index = db.track_index().unwrap();
        let row = |id| index.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row(old).path, "/new.mp3");
        assert_eq!(row(old).missing_since, None);
        assert_eq!(row(older).missing_since, Some(1_000));
        assert_eq!(index.len(), 2, "nothing inserted");
    }

    #[test]
    fn two_copies_of_one_missing_track_reattach_one_and_duplicate_it() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.increment_play_count(id).unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        let done = db
            .reconcile(&Reconcile {
                new_files: vec![new_file("/b.mp3", "v1:x"), new_file("/c.mp3", "v1:x")],
                ..Default::default()
            })
            .unwrap();

        assert_eq!((done.reattached, done.duplicated), (1, 1));
        let tracks = db.search("", None, None, None).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|t| t.play_count == 1));
    }

    #[test]
    fn purge_deletes_only_missing_rows() {
        let db = Db::open_in_memory().unwrap();
        for path in ["/a.mp3", "/b.mp3", "/c.mp3"] {
            db.insert_track(&TrackInsert {
                path: path.into(),
                content_type: "music".into(),
                title: Some(path.into()),
                duration: Some(100.0),
                ..Default::default()
            })
            .unwrap();
        }
        let ids: Vec<i64> = db.track_index().unwrap().iter().map(|r| r.id).collect();
        db.set_cue_points(
            ids[0],
            CuePoints {
                cue_out_ms: Some(50_000),
                ..Default::default()
            },
        )
        .unwrap();
        db.reconcile(&Reconcile {
            gone: vec![ids[0], ids[1]],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();
        let missing = db.missing_tracks().unwrap();
        assert_eq!(missing.len(), 2);
        assert_eq!(missing.iter().filter(|m| m.has_cue_points).count(), 1);

        assert_eq!(db.purge_tracks(&ids).unwrap().len(), 2);

        assert!(db.missing_tracks().unwrap().is_empty());
        assert_eq!(db.track_index().unwrap().len(), 1);
        assert!(db.get_track(ids[0]).unwrap().is_none());
        assert_eq!(db.search("c", None, None, None).unwrap().len(), 1);
        assert!(db.search("a", None, None, None).unwrap().is_empty());
    }

    // ----- airing log -----

    fn log_rows(db: &Db) -> Vec<(Option<i64>, i64, String, String)> {
        let conn = db.conn.lock();
        let mut stmt = conn
            .prepare("SELECT track_id, aired_at, artist, title FROM play_log ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn recording_an_airing_logs_it_and_bumps_the_play_count() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Song", "Band", "Album", "music");
        let id = only_id(&db);

        db.record_airing(id, 1_700).unwrap();

        assert_eq!(
            log_rows(&db),
            vec![(Some(id), 1_700, "Band".into(), "Song".into())]
        );
        assert_eq!(db.get_track(id).unwrap().unwrap().play_count, 1);
    }

    /// The row records what aired, not what the library says now: a later tag
    /// fix must not rewrite the broadcast record.
    #[test]
    fn a_logged_airing_keeps_the_tags_it_aired_with() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Old Title", "Old Band", "Album", "music");
        let id = only_id(&db);
        db.record_airing(id, 1).unwrap();

        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some("New Title".into()),
            artist: Some("New Band".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            log_rows(&db),
            vec![(Some(id), 1, "Old Band".into(), "Old Title".into())]
        );
    }

    /// Nothing to read the snapshot off, so nothing is logged — and no error.
    #[test]
    fn airing_a_track_that_is_gone_logs_nothing() {
        let db = Db::open_in_memory().unwrap();
        db.record_airing(404, 1).unwrap();
        assert!(log_rows(&db).is_empty());
    }

    #[test]
    fn recent_airings_are_oldest_first_and_capped() {
        let db = Db::open_in_memory().unwrap();
        for i in 1..=3 {
            insert(&db, &format!("/{i}.mp3"), "t", "a", "al", "music");
        }
        let ids: Vec<i64> = db.track_index().unwrap().iter().map(|r| r.id).collect();
        for (n, id) in ids.iter().enumerate() {
            db.record_airing(*id, n as i64).unwrap();
        }

        let airings: Vec<i64> = db.recent_airings(2).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(airings, vec![ids[1], ids[2]]);
        assert!(db.recent_airings(0).unwrap().is_empty());
    }

    /// The same track twice is two airings, not one.
    #[test]
    fn recent_airings_repeat_a_track_aired_twice() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "t", "a", "al", "music");
        let id = only_id(&db);
        db.record_airing(id, 1).unwrap();
        db.record_airing(id, 2).unwrap();

        let airings: Vec<i64> = db
            .recent_airings(10)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(airings, vec![id, id]);
    }

    /// A purge takes the track, never the record of its airing.
    #[test]
    fn purging_a_track_keeps_its_airings_and_only_drops_the_id() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Song", "Band", "Album", "music");
        let id = only_id(&db);
        db.record_airing(id, 5).unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(db.purge_tracks(&[id]).unwrap(), vec![id]);

        assert_eq!(log_rows(&db), vec![(None, 5, "Band".into(), "Song".into())]);
        assert!(db.recent_airings(10).unwrap().is_empty());
    }

    /// Cue points are clamped on write and the clamped value comes back, so the
    /// renderer never has to reimplement the rule.
    #[test]
    fn set_cue_points_clamps_and_returns_what_it_stored() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);

        let stored = db
            .set_cue_points(
                id,
                CuePoints {
                    cue_in_ms: Some(-1_000),
                    cue_out_ms: Some(500_000),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(stored.cue_in_ms, Some(0));
        assert_eq!(stored.cue_out_ms, Some(200_000), "bounded by the file");
        assert_eq!(db.get_track(id).unwrap().unwrap().cue_points, stored);
    }

    /// Reads the ownership state and provenance the public API deliberately
    /// does not expose — nothing outside the analysis pass acts on them.
    fn auto_cue_row(db: &Db, id: i64) -> (String, Option<i64>, Option<f64>, Option<f64>) {
        let conn = db.conn.lock();
        conn.query_row(
            "SELECT auto_cue_state, auto_cue_version, auto_cue_silence_db, \
                    auto_cue_segue_db FROM tracks WHERE id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }

    /// The Next Start as the row holds it, past whatever the policy reports.
    fn stored_next_start(db: &Db, id: i64) -> Option<i64> {
        let conn = db.conn.lock();
        conn.query_row("SELECT next_start_ms FROM tracks WHERE id = ?", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// Drop a row's level table, as a database written before it existed has.
    fn clear_levels(db: &Db, id: i64) {
        let conn = db.conn.lock();
        conn.execute(
            "UPDATE tracks SET auto_cue_levels = NULL WHERE id = ?",
            [id],
        )
        .unwrap();
    }

    /// Write a raw blob into a row's level table, as another build might have.
    fn put_levels(db: &Db, id: i64, blob: &[u8]) {
        let conn = db.conn.lock();
        conn.execute(
            "UPDATE tracks SET auto_cue_levels = ?1 WHERE id = ?2",
            params![blob, id],
        )
        .unwrap();
    }

    fn music_track(db: &Db, path: &str) -> i64 {
        typed_track(db, path, "music")
    }

    /// Like [`music_track`] but usable more than once: the id is found by path
    /// rather than by "the first music row".
    fn another_music_track(db: &Db, path: &str) -> i64 {
        db.insert_track(&TrackInsert {
            path: path.into(),
            content_type: "music".into(),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();
        db.track_index()
            .unwrap()
            .into_iter()
            .find(|r| r.path == path)
            .unwrap()
            .id
    }

    fn typed_track(db: &Db, path: &str, content_type: &str) -> i64 {
        db.insert_track(&TrackInsert {
            path: path.into(),
            content_type: content_type.into(),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();
        db.search("", Some(content_type), None, None).unwrap()[0].id
    }

    /// [`AUTO`] as a cue-point set, for a save that leaves the trio as it is.
    const AUTO_POINTS: CuePoints = CuePoints {
        cue_in_ms: AUTO.cue_in_ms,
        fade_in_ms: None,
        fade_out_ms: None,
        cue_out_ms: AUTO.cue_out_ms,
        next_start_ms: AUTO.next_start_ms,
    };

    /// A completed analysis to commit, with [`AUTO`]'s trio or another.
    fn one_analysis(cue: AutoCue, at_ms: i64) -> Analysed {
        Analysed {
            cue,
            levels: levels(),
            thresholds: THRESHOLDS,
            at_ms,
        }
    }

    /// A level table to commit alongside [`AUTO`]. Its contents do not matter
    /// here — only that a commit carries one and a read gets it back.
    fn levels() -> auto_cue::Envelope {
        auto_cue::RmsWindows {
            rms: vec![0.0, 0.5, 0.5, 0.0],
            duration_ms: 200,
        }
        .envelope()
    }

    const AUTO: AutoCue = AutoCue {
        cue_in_ms: Some(100),
        cue_out_ms: Some(190_000),
        next_start_ms: Some(189_000),
    };

    /// The level table as the row holds it.
    fn stored_levels(db: &Db, id: i64) -> Option<Envelope> {
        let conn = db.conn.lock();
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT auto_cue_levels FROM tracks WHERE id = ?",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        blob.map(|b| Envelope::decode(&b).expect("a table this build wrote"))
    }

    /// The table lands with the trio, in the one statement, so a reader can
    /// never see markers without the measurements they came from.
    #[test]
    fn an_automatic_result_lands_with_its_level_envelope() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");
        assert_eq!(stored_levels(&db, id), Some(levels()));
    }

    /// The backfill: a row analysed before the table existed gets one without
    /// its markers moving. Re-deriving here would apply today's thresholds to a
    /// track analysed under yesterday's.
    #[test]
    fn a_level_envelope_can_be_backfilled_without_touching_the_trio() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");
        clear_levels(&db, id);

        let other = auto_cue::RmsWindows {
            rms: vec![0.9; 8],
            duration_ms: 400,
        }
        .envelope();
        assert!(db.set_auto_cue_levels(id, &other, "music", None).unwrap());

        assert_eq!(stored_levels(&db, id), Some(other));
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(points.cue_out_ms, AUTO.cue_out_ms, "the trio stands");
        assert_eq!(points.next_start_ms, AUTO.next_start_ms);
    }

    /// The same three-way guard the full commit runs: a row the operator took
    /// over, one reclassified, or one whose file was replaced under the decode.
    #[test]
    fn a_backfilled_envelope_is_refused_once_the_row_has_moved_on() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !db.set_auto_cue_levels(id, &levels(), "music", None)
                .unwrap(),
            "the row is the operator's now"
        );

        let other = music_track(&db, "/b.mp3");
        assert!(
            !db.set_auto_cue_levels(other, &levels(), "jingle", None)
                .unwrap(),
            "reclassified under the decode"
        );
        assert!(
            !db.set_auto_cue_levels(other, &levels(), "music", Some(9))
                .unwrap(),
            "the file was replaced under the decode"
        );
        assert_eq!(stored_levels(&db, other), None);
    }

    /// A row with a table is done; one without is queued for the decode that
    /// fills it, and a row the operator owns is never queued for one at all.
    #[test]
    fn a_missing_level_envelope_queues_the_track() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");
        db.set_waveform(id, &[0]).unwrap();
        db.set_loudness(id, Some(-6.0), Some(0.9), 1).unwrap();
        db.set_fingerprint(id, "x").ok();

        let queued = |db: &Db| {
            db.tracks_needing_analysis()
                .unwrap()
                .into_iter()
                .find(|j| j.id == id)
        };
        assert!(
            queued(&db).is_none_or(|j| !j.needs_auto_cue_levels),
            "a row with a table is not queued for one"
        );

        clear_levels(&db, id);
        let job = queued(&db).expect("queued for its table");
        assert!(job.needs_auto_cue_levels);
        assert!(!job.needs_auto_cue, "the trio is already derived");
    }

    /// A table written by a build with a different layout counts as missing:
    /// the row goes back through the decode rather than having its markers
    /// derived from bytes this build cannot read.
    #[test]
    fn an_envelope_this_build_cannot_read_queues_the_track() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");
        db.set_waveform(id, &[0]).unwrap();
        db.set_loudness(id, Some(-6.0), Some(0.9), 1).unwrap();

        let queued = |db: &Db| {
            db.tracks_needing_analysis()
                .unwrap()
                .into_iter()
                .find(|j| j.id == id)
                .is_some_and(|j| j.needs_auto_cue_levels)
        };

        let mut future = levels().encode();
        future[0] = auto_cue::LEVELS_FORMAT_VERSION.wrapping_add(1);
        put_levels(&db, id, &future);
        assert!(queued(&db), "a layout this build does not know");

        put_levels(&db, id, &future[..future.len() - 1]);
        assert!(queued(&db), "a length that does not match the layout");

        put_levels(&db, id, &levels().encode());
        assert!(!queued(&db), "a table this build wrote");
    }

    /// A rescan of a changed file drops the table — it measures audio that is
    /// gone, and a recalculation reading it would derive from a file that has
    /// been replaced. The markers stand, as they always do, until a fresh
    /// result lands.
    #[test]
    fn a_replaced_file_drops_the_level_envelope() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");

        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            duration: Some(200.0),
            mtime: Some(99),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            stored_levels(&db, id),
            None,
            "it measures audio that is gone"
        );
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.cue_out_ms,
            AUTO.cue_out_ms,
            "the markers stand until a fresh result lands"
        );
    }

    #[test]
    fn an_automatic_result_lands_with_its_provenance() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        let stored = db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");

        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(stored, points, "the caller is handed what was stored");
        assert_eq!(points.cue_in_ms, AUTO.cue_in_ms);
        assert_eq!(points.cue_out_ms, AUTO.cue_out_ms);
        assert_eq!(points.next_start_ms, AUTO.next_start_ms);
        assert_eq!(
            auto_cue_row(&db, id),
            (
                "auto".into(),
                Some(auto_cue::ALGORITHM_VERSION),
                Some(THRESHOLDS.silence_dbfs),
                Some(THRESHOLDS.segue_dbfs)
            )
        );
    }

    /// No automatic Next Start was derived, so no segue threshold was used.
    #[test]
    fn a_commercial_records_no_segue_threshold() {
        let db = Db::open_in_memory().unwrap();
        let id = typed_track(&db, "/ad.mp3", "commercial");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "commercial", None)
            .unwrap();
        assert_eq!(auto_cue_row(&db, id).3, None);
    }

    #[test]
    fn an_operator_edit_takes_the_trio_off_automatic() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(2_000),
                cue_out_ms: AUTO.cue_out_ms,
                next_start_ms: AUTO.next_start_ms,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    /// `NULL` may be a deliberate operator decision, so clearing a marker takes
    /// ownership exactly as moving one does.
    #[test]
    fn clearing_a_marker_takes_the_trio_off_automatic() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: AUTO.cue_in_ms,
                cue_out_ms: AUTO.cue_out_ms,
                next_start_ms: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    #[test]
    fn a_fade_edit_leaves_the_trio_automatic() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: AUTO.cue_in_ms,
                fade_out_ms: Some(180_000),
                cue_out_ms: AUTO.cue_out_ms,
                next_start_ms: AUTO.next_start_ms,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(auto_cue_row(&db, id).0, "auto");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.fade_out_ms,
            Some(180_000)
        );
    }

    /// The race the ownership state exists for: the operator saves while the
    /// decode runs, and the completed analysis is thrown away.
    #[test]
    fn an_analysis_that_finishes_after_an_operator_save_is_discarded() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        let manual = CuePoints {
            cue_in_ms: Some(5_000),
            ..Default::default()
        };
        db.set_cue_points(id, manual).unwrap();

        assert!(db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .is_none());
        assert_eq!(db.get_track(id).unwrap().unwrap().cue_points, manual);
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    #[test]
    fn a_reclassification_requeues_an_automatic_track() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            content_type: Some("jingle".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "pending");
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(
            points.next_start_ms, AUTO.next_start_ms,
            "the old values stand until the fresh result lands"
        );
        assert!(db.tracks_needing_analysis().unwrap()[0].needs_auto_cue);
    }

    #[test]
    fn a_reclassification_leaves_a_manual_track_alone() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_cue_points(
            id,
            CuePoints {
                next_start_ms: Some(1_000),
                ..Default::default()
            },
        )
        .unwrap();

        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            content_type: Some("jingle".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "manual");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.next_start_ms,
            Some(1_000)
        );
    }

    /// An edit that does not touch the content type must not put the track
    /// back in the queue.
    #[test]
    fn a_tag_edit_leaves_an_automatic_track_analysed() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some("New".into()),
            content_type: Some("music".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "auto");
    }

    /// Switched off, a derived set is held back everywhere a track is read —
    /// the row keeps it, so switching back on costs no second pass.
    #[test]
    fn an_automatic_set_is_held_back_while_the_feature_is_off() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.set_auto_cue_policy(false, true);
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points,
            CuePoints::default()
        );
        assert_eq!(
            db.get_track_load_info(id).unwrap().unwrap().cue_points,
            CuePoints::default(),
            "the deck airs the whole file"
        );
        assert_eq!(
            db.search("", None, None, None).unwrap()[0].cue_points,
            CuePoints::default()
        );
        assert_eq!(
            db.get_tracks_by_ids(&[id]).unwrap()[0].cue_points,
            CuePoints::default()
        );
        assert_eq!(auto_cue_row(&db, id).0, "auto", "the result is still there");

        db.set_auto_cue_policy(true, true);
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.cue_out_ms,
            AUTO.cue_out_ms,
            "and comes straight back"
        );
    }

    /// The trims are a different decision from the handover, so the Next Start
    /// switch holds back that marker alone. The row keeps it either way.
    #[test]
    fn only_the_next_start_is_held_back_while_its_switch_is_off() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.set_auto_cue_policy(true, false);
        let trimmed = CuePoints {
            cue_in_ms: AUTO.cue_in_ms,
            cue_out_ms: AUTO.cue_out_ms,
            next_start_ms: None,
            ..Default::default()
        };
        assert_eq!(db.get_track(id).unwrap().unwrap().cue_points, trimmed);
        assert_eq!(
            db.get_track_load_info(id).unwrap().unwrap().cue_points,
            trimmed,
            "the deck hands over at Cue Out"
        );
        assert_eq!(
            db.search("", None, None, None).unwrap()[0].cue_points,
            trimmed
        );
        assert_eq!(db.get_tracks_by_ids(&[id]).unwrap()[0].cue_points, trimmed);
        assert_eq!(
            stored_next_start(&db, id),
            AUTO.next_start_ms,
            "the derived position is still there"
        );

        db.set_auto_cue_policy(true, true);
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.next_start_ms,
            AUTO.next_start_ms,
            "and comes straight back"
        );
    }

    /// A requeued track holds a trio that is still analysis's, so it follows
    /// the switch with the `auto` ones.
    #[test]
    fn a_requeued_next_start_is_held_back_too() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE tracks SET auto_cue_state = 'pending' WHERE id = ?",
                [id],
            )
            .unwrap();

        db.set_auto_cue_policy(true, false);
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(points.cue_out_ms, AUTO.cue_out_ms);
        assert_eq!(points.next_start_ms, None);
    }

    /// Nobody can clear a marker they were never given: a fade save while the
    /// Next Start is hidden leaves the derived one on the row.
    #[test]
    fn a_fade_save_keeps_a_hidden_next_start() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.set_auto_cue_policy(true, false);

        let shown = db.get_track(id).unwrap().unwrap().cue_points;
        let returned = db
            .set_cue_points(
                id,
                CuePoints {
                    fade_out_ms: Some(185_000),
                    ..shown
                },
            )
            .unwrap();

        assert_eq!(returned.next_start_ms, None, "still hidden from the caller");
        assert_eq!(stored_next_start(&db, id), AUTO.next_start_ms);
        assert_eq!(auto_cue_row(&db, id).0, "auto", "still analysis's");
    }

    /// Moving the trio is an operator taking it over, and what they were shown
    /// had no Next Start — so the derived one goes. `NULL` means the handover
    /// waits for Cue Out, which is what the switch asked for.
    #[test]
    fn moving_the_trio_clears_a_hidden_next_start() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.set_auto_cue_policy(true, false);

        let shown = db.get_track(id).unwrap().unwrap().cue_points;
        db.set_cue_points(
            id,
            CuePoints {
                cue_out_ms: Some(180_000),
                ..shown
            },
        )
        .unwrap();

        assert_eq!(stored_next_start(&db, id), None);
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    /// The switch is about automatic analysis. A radio edit the operator made
    /// is theirs, and airs either way.
    #[test]
    fn a_manual_set_is_never_held_back() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        let manual = CuePoints {
            cue_in_ms: Some(5_000),
            ..Default::default()
        };
        db.set_cue_points(id, manual).unwrap();

        db.set_auto_cue_policy(false, true);
        assert_eq!(db.get_track(id).unwrap().unwrap().cue_points, manual);
        assert_eq!(
            db.get_track_load_info(id).unwrap().unwrap().cue_points,
            manual
        );
    }

    /// A hand-made radio edit is never hidden, so a fade save while the feature
    /// is off must hand it straight back. Handing back an empty trio would take
    /// it off every copy the app holds — and the next save, made against that,
    /// would read as clearing it and wipe it from the row.
    #[test]
    fn a_manual_trio_survives_a_fade_save_with_the_feature_off() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        let edit = CuePoints {
            cue_in_ms: Some(10_000),
            cue_out_ms: Some(180_000),
            ..Default::default()
        };
        db.set_cue_points(id, edit).unwrap();
        db.set_auto_cue_policy(false, true);

        let returned = db
            .set_cue_points(
                id,
                CuePoints {
                    fade_out_ms: Some(170_000),
                    ..edit
                },
            )
            .unwrap();

        assert_eq!(returned.cue_in_ms, edit.cue_in_ms, "still theirs");
        assert_eq!(returned.cue_out_ms, edit.cue_out_ms);
        assert_eq!(returned.fade_out_ms, Some(170_000));
        assert_eq!(db.get_track(id).unwrap().unwrap().cue_points, returned);

        // And the save made against what came back changes nothing.
        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(160_000),
                ..returned
            },
        )
        .unwrap();
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(points.cue_out_ms, edit.cue_out_ms);
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    /// The purge warning is about work the operator would have to do again. A
    /// derived trio comes back from the next analysis; a fade never does, and
    /// setting one leaves the track automatic — so the state alone cannot
    /// decide it.
    #[test]
    fn the_purge_warning_counts_operator_work_only() {
        let db = Db::open_in_memory().unwrap();
        let analysed = another_music_track(&db, "/analysed.mp3");
        let faded = another_music_track(&db, "/faded.mp3");
        let edited = another_music_track(&db, "/edited.mp3");
        let untouched = another_music_track(&db, "/untouched.mp3");

        for id in [analysed, faded, edited] {
            db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
                .unwrap();
        }
        db.set_cue_points(
            faded,
            CuePoints {
                fade_out_ms: Some(180_000),
                ..AUTO_POINTS
            },
        )
        .unwrap();
        db.set_cue_points(
            edited,
            CuePoints {
                cue_in_ms: Some(2_000),
                ..AUTO_POINTS
            },
        )
        .unwrap();
        assert_eq!(auto_cue_row(&db, faded).0, "auto", "a fade owns nothing");

        db.reconcile(&Reconcile {
            gone: vec![analysed, faded, edited, untouched],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        let flagged: Vec<i64> = db
            .missing_tracks()
            .unwrap()
            .into_iter()
            .filter(|m| m.has_cue_points)
            .map(|m| m.id)
            .collect();
        assert!(flagged.contains(&faded), "the fade is theirs");
        assert!(flagged.contains(&edited), "so is an owned trio");
        assert!(!flagged.contains(&analysed), "analysis regenerates");
        assert!(!flagged.contains(&untouched));
    }

    /// A file replaced while the pass was decoding it: the result describes
    /// audio that is gone, and stamping it `auto` would drop the requeue the
    /// rescan just made for good.
    #[test]
    fn an_analysis_of_a_file_that_has_since_changed_is_discarded() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            duration: Some(200.0),
            mtime: Some(1),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        let job = db.tracks_needing_analysis().unwrap().remove(0);
        assert_eq!(job.mtime, Some(1));

        // The scan sees the file change while the decode runs.
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            duration: Some(200.0),
            mtime: Some(2),
            ..Default::default()
        })
        .unwrap();

        assert!(db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", job.mtime)
            .unwrap()
            .is_none());
        assert_eq!(auto_cue_row(&db, id).0, "pending", "still queued");
        assert!(db
            .set_auto_cue(id, &one_analysis(AUTO, 8), "music", Some(2))
            .unwrap()
            .is_some());
    }

    /// A fade the operator saved before the analysis landed must not end up
    /// past the Cue Out it arrives with: the load-time clamp would fold it onto
    /// Cue Out and the ramp would collapse to nothing, unasked.
    #[test]
    fn an_analysis_sorts_the_fades_it_finds_against_its_own_trio() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(196_000),
                ..Default::default()
            },
        )
        .unwrap();
        // A fade owns nothing, so the track is still up for analysis.
        assert_eq!(auto_cue_row(&db, id).0, "pending");

        let stored = db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");

        assert_eq!(stored.cue_out_ms, AUTO.cue_out_ms);
        assert_eq!(
            stored.fade_out_ms, AUTO.cue_out_ms,
            "pulled back onto Cue Out, where it still plays"
        );
    }

    /// Ordinary fades are left exactly where the operator put them.
    #[test]
    fn an_analysis_leaves_a_fade_inside_its_trio_alone() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(180_000),
                ..Default::default()
            },
        )
        .unwrap();

        let stored = db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .expect("committed");

        assert_eq!(stored.fade_out_ms, Some(180_000));
    }

    /// A requeue keeps the old trio and sets the state back to `pending`, so
    /// `pending` has to be held back too — or a rescan, a reclassification or a
    /// move between roots would put the derived markers back on air behind the
    /// operator's back.
    #[test]
    fn a_requeued_track_is_held_back_as_well() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.set_auto_cue_policy(false, true);

        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            content_type: Some("jingle".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "pending", "requeued, trio intact");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points,
            CuePoints::default()
        );
        assert_eq!(
            db.get_track_load_info(id).unwrap().unwrap().cue_points,
            CuePoints::default(),
            "and the deck still airs the whole file"
        );
    }

    /// A fade saved while the feature is off must still be bounded by the Cue
    /// Out it is stored against. Bounded by the file end instead it would sit
    /// past Cue Out, and the load-time resolve would drop it without a word.
    #[test]
    fn a_fade_saved_with_the_feature_off_stays_inside_the_hidden_cue_out() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap(); // cue out 190 s
        db.set_auto_cue_policy(false, true);

        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(196_000),
                ..Default::default()
            },
        )
        .unwrap();

        db.set_auto_cue_policy(true, true);
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(points.cue_out_ms, AUTO.cue_out_ms);
        assert_eq!(
            points.fade_out_ms, AUTO.cue_out_ms,
            "bounded by Cue Out, so the fade still plays"
        );
    }

    /// Nobody can clear markers they were never shown: with the feature off the
    /// editor offers no trio, so saving a fade must not read as clearing one.
    #[test]
    fn a_save_made_with_the_feature_off_keeps_the_derived_trio() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.set_auto_cue_policy(false, true);

        // What the editor shows while the feature is off: no markers at all.
        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(180_000),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "auto", "a fade owns nothing");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.fade_out_ms,
            Some(180_000),
            "the fade is theirs, and applies either way"
        );

        db.set_auto_cue_policy(true, true);
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.cue_out_ms,
            AUTO.cue_out_ms,
            "and the analysis is still there when it comes back on"
        );
    }

    /// The other side of the same race: the operator reclassifies while the
    /// decode runs, so the result was derived under a class the row no longer
    /// has. The row stays `pending` and the pass takes it again.
    #[test]
    fn an_analysis_that_finishes_after_a_reclassification_is_discarded() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            content_type: Some("jingle".into()),
            ..Default::default()
        })
        .unwrap();

        // The in-flight decode reports what it analysed: the old class.
        assert!(db
            .set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap()
            .is_none());
        assert_eq!(auto_cue_row(&db, id).0, "pending");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.next_start_ms,
            None,
            "a jingle never takes a music Next Start"
        );
        assert!(db.tracks_needing_analysis().unwrap()[0].needs_auto_cue);

        assert!(db
            .set_auto_cue(id, &one_analysis(AUTO, 8), "jingle", None)
            .unwrap()
            .is_some());
    }

    /// A VBR MP3 whose tag duration undersells the file: automatic analysis
    /// stores a Cue Out past it, and a fade-only save must not pull that back.
    #[test]
    fn a_fade_edit_leaves_a_marker_past_the_tag_duration_alone() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/vbr.mp3"); // tag duration 200 s
        let decoded = AutoCue {
            cue_in_ms: Some(100),
            cue_out_ms: Some(204_000),
            next_start_ms: Some(203_000),
        };
        db.set_auto_cue(id, &one_analysis(decoded, 7), "music", None)
            .unwrap();

        let saved = db
            .set_cue_points(
                id,
                CuePoints {
                    cue_in_ms: decoded.cue_in_ms,
                    fade_out_ms: Some(180_000),
                    cue_out_ms: decoded.cue_out_ms,
                    next_start_ms: decoded.next_start_ms,
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "auto");
        assert_eq!(saved.cue_out_ms, decoded.cue_out_ms);
        assert_eq!(saved.fade_out_ms, Some(180_000));
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(points.cue_out_ms, decoded.cue_out_ms);
        assert_eq!(points.next_start_ms, decoded.next_start_ms);
    }

    /// Moving one marker must not drag the others: the edit takes ownership,
    /// and an automatic Cue Out past the tag duration stays where the decode
    /// put it.
    #[test]
    fn an_edit_leaves_the_markers_it_did_not_touch_where_they_are() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/vbr.mp3"); // tag duration 200 s
        let decoded = AutoCue {
            cue_in_ms: Some(100),
            cue_out_ms: Some(203_500),
            next_start_ms: Some(203_000),
        };
        db.set_auto_cue(id, &one_analysis(decoded, 7), "music", None)
            .unwrap();

        let saved = db
            .set_cue_points(
                id,
                CuePoints {
                    cue_in_ms: Some(300),
                    cue_out_ms: decoded.cue_out_ms,
                    next_start_ms: decoded.next_start_ms,
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.cue_in_ms, Some(300));
        assert_eq!(saved.cue_out_ms, decoded.cue_out_ms);
        assert_eq!(saved.next_start_ms, decoded.next_start_ms);
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    /// Moving a marker still takes ownership, and is still bounded by the
    /// duration the library holds.
    #[test]
    fn an_edit_past_the_tag_duration_is_still_clamped() {
        let db = Db::open_in_memory().unwrap();
        let id = music_track(&db, "/a.mp3");
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        let saved = db
            .set_cue_points(
                id,
                CuePoints {
                    cue_in_ms: AUTO.cue_in_ms,
                    cue_out_ms: Some(500_000),
                    next_start_ms: AUTO.next_start_ms,
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.cue_out_ms, Some(200_000));
        assert_eq!(auto_cue_row(&db, id).0, "manual");
    }

    /// A file that comes back under a different root is a different class of
    /// material, so its automatic trio is re-derived.
    #[test]
    fn a_reattach_under_another_root_requeues_the_analysis() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/music/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        let done = db
            .reconcile(&Reconcile {
                new_files: vec![TrackInsert {
                    content_type: "jingle".into(),
                    ..new_file("/jingles/a.mp3", "v1:x")
                }],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(done.reattached, 1);
        assert_eq!(auto_cue_row(&db, id).0, "pending");
        let points = db.get_track(id).unwrap().unwrap().cue_points;
        assert_eq!(
            points.cue_out_ms, AUTO.cue_out_ms,
            "the trims stand until the fresh result lands: same audio"
        );
        assert_eq!(
            points.next_start_ms, None,
            "but a music Next Start has no business on a jingle"
        );
    }

    #[test]
    fn a_reattach_under_the_same_root_keeps_the_analysis() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/music/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        db.reconcile(&Reconcile {
            new_files: vec![new_file("/music/moved.mp3", "v1:x")],
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "auto");
    }

    /// The operator's markers survive the move: a reclassification never
    /// overrides a manual trio, here as anywhere else.
    #[test]
    fn a_reattach_under_another_root_leaves_a_manual_trio_alone() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/music/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(1_000),
                ..Default::default()
            },
        )
        .unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 1,
            ..Default::default()
        })
        .unwrap();

        db.reconcile(&Reconcile {
            new_files: vec![TrackInsert {
                content_type: "jingle".into(),
                ..new_file("/jingles/a.mp3", "v1:x")
            }],
            ..Default::default()
        })
        .unwrap();

        assert_eq!(auto_cue_row(&db, id).0, "manual");
        assert_eq!(
            db.get_track(id).unwrap().unwrap().cue_points.cue_in_ms,
            Some(1_000)
        );
    }

    /// The same file filed under two roots is two jobs: the jingle copy is
    /// analysed as a jingle rather than inheriting the music row's result.
    #[test]
    fn a_duplicate_in_another_root_is_analysed_for_itself() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/music/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        let done = db
            .reconcile(&Reconcile {
                new_files: vec![TrackInsert {
                    content_type: "jingle".into(),
                    ..new_file("/jingles/a.mp3", "v1:x")
                }],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(done.duplicated, 1);
        let copy = db
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id != id)
            .unwrap();
        assert_eq!(
            auto_cue_row(&db, copy.id),
            ("pending".into(), None, None, None),
            "no provenance from an analysis this track never had"
        );
        assert_eq!(
            copy.cue_points,
            CuePoints::default(),
            "nor a music trio on a jingle"
        );
        assert_eq!(auto_cue_row(&db, id).0, "auto", "the twin is untouched");
        assert_eq!(
            stored_levels(&db, copy.id),
            Some(levels()),
            "the table still travels: it measures the audio, not the class"
        );
    }

    #[test]
    fn a_duplicate_in_the_same_root_inherits_the_analysis() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&new_file("/music/a.mp3", "v1:x")).unwrap();
        let id = only_id(&db);
        db.set_auto_cue(id, &one_analysis(AUTO, 7), "music", None)
            .unwrap();

        db.reconcile(&Reconcile {
            new_files: vec![new_file("/music/copy.mp3", "v1:x")],
            ..Default::default()
        })
        .unwrap();

        let copy = db
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id != id)
            .unwrap();
        assert_eq!(auto_cue_row(&db, copy.id).0, "auto");
        assert_eq!(copy.cue_points.next_start_ms, AUTO.next_start_ms);
        assert_eq!(
            stored_levels(&db, copy.id),
            Some(levels()),
            "the table measures audio the twin shares"
        );
    }

    /// Cue points ride along on the one query a deck load already runs.
    #[test]
    fn load_info_carries_the_cue_points() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(10_000),
                cue_out_ms: Some(30_000),
                ..Default::default()
            },
        )
        .unwrap();

        let info = db.get_track_load_info(id).unwrap().expect("track present");
        assert_eq!(info.cue_points.cue_in_ms, Some(10_000));
        assert_eq!(info.cue_points.cue_out_ms, Some(30_000));
    }

    /// A metadata rescan rewrites the tag columns; operator work on the same row
    /// has to survive it, exactly as the waveform does.
    #[test]
    fn a_rescan_does_not_clobber_cue_points() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            title: Some("Before".into()),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(10_000),
                ..Default::default()
            },
        )
        .unwrap();

        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            title: Some("After".into()),
            duration: Some(200.0),
            ..Default::default()
        })
        .unwrap();

        let track = db.get_track(id).unwrap().unwrap();
        assert_eq!(track.title, "After", "the rescan did land");
        assert_eq!(track.cue_points.cue_in_ms, Some(10_000));
    }

    fn tagged(path: &str, title: &str, artist: &str) -> TrackInsert {
        TrackInsert {
            path: path.into(),
            content_type: "music".into(),
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some("Album".into()),
            fingerprint: Some("v1:x".into()),
            ..Default::default()
        }
    }

    fn retitle(db: &Db, id: i64, title: &str) -> Track {
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some(title.into()),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn an_edited_field_survives_a_rescan_and_the_rest_follow_the_file() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&tagged("/a.mp3", "File", "Artist"))
            .unwrap();
        let id = only_id(&db);
        let edited = retitle(&db, id, "Edited");
        assert_eq!(edited.edited_fields, EditedFields::TITLE);

        db.insert_track(&tagged("/a.mp3", "Retagged", "New Artist"))
            .unwrap();

        let track = db.get_track(id).unwrap().unwrap();
        assert_eq!(track.title, "Edited");
        assert_eq!(track.artist, "New Artist");
    }

    #[test]
    fn a_field_saved_unchanged_is_not_flagged() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&tagged("/a.mp3", "Title", "Artist"))
            .unwrap();
        let id = only_id(&db);
        let track = db
            .update_track_metadata(&TrackMetadataUpdate {
                id,
                title: Some("Title".into()),
                artist: Some("Other".into()),
                album: Some("Album".into()),
                genre: Some(None),
                year: Some(None),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(track.edited_fields, EditedFields::ARTIST);
    }

    #[test]
    fn a_duplicate_inherits_the_edited_fields() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&tagged("/a.mp3", "File", "Artist"))
            .unwrap();
        let id = only_id(&db);
        retitle(&db, id, "Edited");

        let done = db
            .reconcile(&Reconcile {
                new_files: vec![tagged("/b.mp3", "File", "Artist")],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(done.duplicated, 1);
        let copy = db
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id != id)
            .unwrap();
        assert_eq!(copy.title, "Edited");
        assert_eq!(copy.edited_fields, EditedFields::TITLE);
        db.insert_track(&tagged("/b.mp3", "Retagged", "Artist"))
            .unwrap();
        assert_eq!(db.get_track(copy.id).unwrap().unwrap().title, "Edited");
    }

    #[test]
    fn revert_takes_the_file_tags_and_clears_the_flags() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&tagged("/a.mp3", "File", "Artist"))
            .unwrap();
        let id = only_id(&db);
        retitle(&db, id, "Edited");
        db.conn
            .lock()
            .execute("UPDATE tracks SET artist = 'Kept' WHERE id = ?", [id])
            .unwrap();

        let mut on_disk = tagged("/a.mp3", "On Disk", "Other");
        on_disk.mtime = Some(42);
        let track = db.revert_track_tags(id, &on_disk).unwrap();

        assert_eq!(track.title, "On Disk");
        assert_eq!(track.artist, "Kept", "an unedited field is left alone");
        assert_eq!(track.edited_fields, 0);
        assert_eq!(db.track_index().unwrap()[0].mtime, Some(42));
    }

    #[test]
    fn a_finished_tag_write_clears_the_flags_only_for_what_it_wrote() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&tagged("/a.mp3", "File", "Artist"))
            .unwrap();
        let id = only_id(&db);
        retitle(&db, id, "First");
        let written = db.tag_values(id).unwrap().unwrap();

        retitle(&db, id, "Second");
        assert!(!db.finish_tag_write(id, &written, 7).unwrap());
        assert_eq!(
            db.get_track(id).unwrap().unwrap().edited_fields,
            EditedFields::TITLE
        );

        let written = db.tag_values(id).unwrap().unwrap();
        assert!(db.finish_tag_write(id, &written, 7).unwrap());
        assert_eq!(db.get_track(id).unwrap().unwrap().edited_fields, 0);
        assert_eq!(db.track_index().unwrap()[0].mtime, Some(7));
    }

    fn only_id(db: &Db) -> i64 {
        db.search("", Some("music"), None, None)
            .unwrap()
            .first()
            .map(|t| t.id)
            .unwrap()
    }

    #[test]
    fn waveform_is_none_until_set_then_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/wave.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        // Metadata insert leaves the waveform empty.
        assert_eq!(db.get_waveform(id).unwrap(), None);

        let peaks = vec![0u8, 64, 128, 255];
        db.set_waveform(id, &peaks).unwrap();
        assert_eq!(db.get_waveform(id).unwrap(), Some(peaks));
    }

    #[test]
    fn metadata_reinsert_preserves_waveform() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/wave.mp3".into(),
            content_type: "music".into(),
            title: Some("Old".into()),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_waveform(id, &[1, 2, 3]).unwrap();

        // A metadata rescan (upsert on the same path) must not wipe the waveform.
        db.insert_track(&TrackInsert {
            path: "/wave.mp3".into(),
            content_type: "music".into(),
            title: Some("New".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(db.get_waveform(id).unwrap(), Some(vec![1u8, 2, 3]));
    }

    #[test]
    fn analysis_lists_tracks_until_every_result_is_filled() {
        let db = Db::open_in_memory().unwrap();
        for path in ["/a.mp3", "/b.mp3"] {
            db.insert_track(&TrackInsert {
                path: path.into(),
                content_type: "music".into(),
                ..Default::default()
            })
            .unwrap();
        }
        let jobs = db.tracks_needing_analysis().unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.needs_waveform
            && j.needs_fingerprint
            && j.needs_loudness
            && j.needs_auto_cue));

        let (a, b) = (jobs[0].id, jobs[1].id);
        db.set_waveform(a, &[9]).unwrap();
        db.set_waveform(b, &[9]).unwrap();
        db.set_fingerprint(b, &format!("{}:b", fingerprint::VERSION))
            .unwrap();
        db.set_loudness(b, Some(-6.0), Some(0.9), 1).unwrap();
        db.set_auto_cue(b, &one_analysis(AutoCue::default(), 1), "music", None)
            .unwrap();

        let jobs = db.tracks_needing_analysis().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, a);
        assert!(!jobs[0].needs_waveform);
        assert!(jobs[0].needs_fingerprint);
        assert!(jobs[0].needs_loudness);
        assert!(jobs[0].needs_auto_cue);
    }

    /// A fingerprint from an older algorithm is worthless for matching a moved
    /// file against the library, so the analysis pass recomputes it.
    #[test]
    fn a_fingerprint_from_an_older_version_is_analysed_again() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_waveform(id, &[9]).unwrap();
        db.set_loudness(id, Some(-6.0), Some(0.9), 1).unwrap();
        db.set_auto_cue(id, &one_analysis(AutoCue::default(), 1), "music", None)
            .unwrap();

        db.set_fingerprint(id, "v1:stale").unwrap();
        let jobs = db.tracks_needing_analysis().unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].needs_fingerprint);

        db.set_fingerprint(id, &format!("{}:fresh", fingerprint::VERSION))
            .unwrap();
        assert!(db.tracks_needing_analysis().unwrap().is_empty());
    }

    /// A silent or very short file measures successfully with no gain to
    /// store. It must still count as measured, or the pass decodes it again on
    /// every run forever.
    #[test]
    fn a_track_with_nothing_to_measure_is_not_queued_again() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/silent.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_waveform(id, &[0]).unwrap();
        db.set_fingerprint(id, &format!("{}:s", fingerprint::VERSION))
            .unwrap();
        db.set_loudness(id, None, None, 42).unwrap();
        db.set_auto_cue(id, &one_analysis(AutoCue::default(), 42), "music", None)
            .unwrap();

        assert!(db.tracks_needing_analysis().unwrap().is_empty());
    }

    #[test]
    fn a_measurement_reaches_both_load_paths() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_loudness(id, Some(-6.5), Some(0.98), 1).unwrap();

        let media = db.get_media_track(id).unwrap().unwrap();
        assert_eq!(media.loudness.gain_db, Some(-6.5));
        assert_eq!(media.loudness.peak, Some(0.98));

        let info = db.get_track_load_info(id).unwrap().unwrap();
        assert_eq!(info.loudness.gain_db, Some(-6.5));
        assert_eq!(info.loudness.peak, Some(0.98));
    }

    #[test]
    fn a_failed_analysis_is_not_retried_or_counted_as_waiting() {
        let db = Db::open_in_memory().unwrap();
        for path in ["/a.mp3", "/b.mp3"] {
            db.insert_track(&TrackInsert {
                path: path.into(),
                content_type: "music".into(),
                ..Default::default()
            })
            .unwrap();
        }
        let jobs = db.tracks_needing_analysis().unwrap();
        let (a, b) = (jobs[0].id, jobs[1].id);
        assert_eq!(db.unhashed_count().unwrap(), 2);

        db.set_analysis_failed(a, "probe: unsupported feature", 7)
            .unwrap();

        let jobs = db.tracks_needing_analysis().unwrap();
        assert_eq!(jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![b]);
        assert_eq!(db.unhashed_count().unwrap(), 1);
        let unreadable = db.unreadable_tracks().unwrap();
        assert_eq!(unreadable.len(), 1);
        assert_eq!(unreadable[0].row.track.id, a);
        assert_eq!(unreadable[0].row.path, "/a.mp3");
        assert_eq!(unreadable[0].error, "probe: unsupported feature");
        assert_eq!(unreadable[0].failed_at, 7);
    }

    #[test]
    fn a_changed_file_clears_its_analysis_failure() {
        let db = Db::open_in_memory().unwrap();
        let file = TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            mtime: Some(1),
            ..Default::default()
        };
        db.insert_track(&file).unwrap();
        let id = only_id(&db);
        db.set_analysis_failed(id, "bad", 7).unwrap();

        db.reconcile(&Reconcile {
            upserts: vec![TrackInsert {
                mtime: Some(2),
                ..file
            }],
            ..Default::default()
        })
        .unwrap();

        assert!(db.unreadable_tracks().unwrap().is_empty());
        assert_eq!(db.tracks_needing_analysis().unwrap()[0].id, id);
        assert_eq!(db.unhashed_count().unwrap(), 1);
    }

    #[test]
    fn a_reattached_file_clears_its_analysis_failure() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            fingerprint: Some("v1:a".into()),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_analysis_failed(id, "bad", 7).unwrap();

        db.reconcile(&Reconcile {
            gone: vec![id],
            new_files: vec![TrackInsert {
                path: "/moved/a.mp3".into(),
                content_type: "music".into(),
                fingerprint: Some("v1:a".into()),
                ..Default::default()
            }],
            now_ms: 9,
            ..Default::default()
        })
        .unwrap();

        assert!(db.unreadable_tracks().unwrap().is_empty());
        assert_eq!(db.tracks_needing_analysis().unwrap()[0].id, id);
    }

    #[test]
    fn a_missing_track_is_not_listed_as_unreadable() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let id = only_id(&db);
        db.set_analysis_failed(id, "bad", 7).unwrap();
        db.reconcile(&Reconcile {
            gone: vec![id],
            now_ms: 9,
            ..Default::default()
        })
        .unwrap();
        assert!(db.unreadable_tracks().unwrap().is_empty());
    }

    #[test]
    fn fts5_search_finds_track_by_title_prefix() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Hello World", "Band", "Album", "music");
        insert(&db, "/b.mp3", "Other", "Band", "Album", "music");
        let r = db.search("hel", None, None, None).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Hello World");
    }

    #[test]
    fn fts5_prefix_term_doubles_embedded_quotes() {
        assert_eq!(fts5_prefix_term("beat"), r#""beat"*"#);
        assert_eq!(fts5_prefix_term(r#"12""#), r#""12"""*"#);
        assert_eq!(fts5_prefix_term(r#"""#), r#"""""*"#);
        assert_eq!(fts5_prefix_term("AC/DC"), r#""AC/DC"*"#);
    }

    #[test]
    fn search_tolerates_double_quotes_in_query() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "12\" Disco Mix", "Band", "Album", "music");
        insert(&db, "/b.mp3", "Other", "Band", "Album", "music");

        let r = db.search("12\"", None, None, None).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "12\" Disco Mix");

        for q in ["\"", "\"\"\"", "a\"b\"", "\" \""] {
            db.search(q, None, None, None)
                .unwrap_or_else(|e| panic!("search({q:?}) errored: {e}"));
        }
    }

    #[test]
    fn search_filters_by_content_type() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Hello", "X", "Y", "music");
        insert(&db, "/b.mp3", "Hello", "X", "Y", "jingle");
        let r = db.search("hello", Some("music"), None, None).unwrap();
        assert_eq!(r.len(), 1);
        let r = db.search("hello", Some("jingle"), None, None).unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn empty_query_lists_with_default_order() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "T", "Beta", "Y", "music");
        insert(&db, "/b.mp3", "T", "Alpha", "Y", "music");
        let r = db.search("", None, None, None).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].artist, "Alpha");
        assert_eq!(r[1].artist, "Beta");
    }

    #[test]
    fn sort_by_play_count_desc() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "A", "X", "Y", "music");
        insert(&db, "/b.mp3", "B", "X", "Y", "music");
        db.increment_play_count(2).unwrap();
        db.increment_play_count(2).unwrap();
        db.increment_play_count(1).unwrap();
        let r = db
            .search("", None, Some("play_count"), Some("desc"))
            .unwrap();
        assert_eq!(r[0].id, 2);
        assert_eq!(r[1].id, 1);
    }

    #[test]
    fn get_stats_counts_by_type() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "T", "X", "Y", "music");
        insert(&db, "/b.mp3", "T", "X", "Y", "music");
        insert(&db, "/c.mp3", "T", "X", "Y", "jingle");
        let s = db.get_stats().unwrap();
        assert_eq!(s.total_tracks, 3);
        assert_eq!(s.tracks_by_type.music, 2);
        assert_eq!(s.tracks_by_type.jingle, 1);
        assert_eq!(s.tracks_by_type.commercial, 0);
    }

    #[test]
    fn get_tracks_by_ids_preserves_order_and_skips_missing() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "A", "X", "Y", "music"); // id=1
        insert(&db, "/b.mp3", "B", "X", "Y", "music"); // id=2
        let r = db.get_tracks_by_ids(&[2, 99, 1]).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id, 2);
        assert_eq!(r[1].id, 1);
    }

    #[test]
    fn pick_random_from_bottom_only_returns_low_play_count_rows() {
        let db = Db::open_in_memory().unwrap();
        // 3 hot, 5 cold commercials. Bucket = 5 → only cold should ever be picked.
        for i in 0..3 {
            insert_with_play_count(&db, &format!("/hot{i}.mp3"), "commercial", 100);
        }
        for i in 0..5 {
            insert_with_play_count(&db, &format!("/cold{i}.mp3"), "commercial", 0);
        }
        for _ in 0..50 {
            let picked = db.pick_random_from_bottom("commercial", 2, 5, &[]).unwrap();
            assert_eq!(picked.len(), 2);
            for t in picked {
                assert_eq!(t.play_count, 0, "hot track leaked into bottom-N pick");
            }
        }
    }

    #[test]
    fn pick_random_from_bottom_breaks_ties_randomly() {
        // All 8 rows tied at play_count = 0. Bucket = 4. Over many picks we
        // should observe more than just the first 4 rows by ROWID — i.e. tie
        // ordering is not deterministic.
        let db = Db::open_in_memory().unwrap();
        for i in 0..8 {
            insert_with_play_count(&db, &format!("/c{i}.mp3"), "commercial", 0);
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let picked = db.pick_random_from_bottom("commercial", 1, 4, &[]).unwrap();
            seen.insert(picked[0].id);
            if seen.len() > 4 {
                break;
            }
        }
        assert!(
            seen.len() > 4,
            "tie ordering deterministic: only saw ids {:?}",
            seen
        );
    }

    #[test]
    fn get_random_tracks_excludes_given_ids() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            insert(&db, &format!("/m{i}.mp3"), "M", "X", "Y", "music"); // ids 1..=5
        }
        // Exclude everything but id 3 — it must be the only row ever returned.
        for _ in 0..50 {
            let picked = db
                .get_random_tracks("music", 5, &SelectionFilter::excluding(&[1, 2, 4, 5]))
                .unwrap();
            assert_eq!(picked.len(), 1);
            assert_eq!(picked[0].id, 3);
        }
    }

    #[test]
    fn get_random_tracks_empty_exclude_returns_rows() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/m1.mp3", "M", "X", "Y", "music");
        insert(&db, "/m2.mp3", "M", "X", "Y", "music");
        // Empty slice must not produce `NOT IN ()` (a SQLite syntax error).
        let picked = db
            .get_random_tracks("music", 5, &SelectionFilter::default())
            .unwrap();
        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn the_title_window_hides_a_track_that_aired_inside_it() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/old.mp3", "Old", "A", "Y", "music");
        insert(&db, "/new.mp3", "New", "B", "Y", "music");
        let (old, new) = (1, 2);
        db.record_airing(old, 1_000).unwrap();
        db.record_airing(new, 5_000).unwrap();

        let picked = |since| {
            let f = SelectionFilter {
                title_since: Some(since),
                ..Default::default()
            };
            let mut ids: Vec<i64> = db
                .get_random_tracks("music", 10, &f)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(picked(6_000), vec![old, new], "window opened after both");
        assert_eq!(picked(5_000), vec![old], "the newer airing is inside it");
        assert_eq!(picked(1_000), Vec::<i64>::new(), "both inside it");
    }

    #[test]
    fn the_artist_window_hides_every_track_by_an_artist_that_aired() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a1.mp3", "One", " Band ", "Y", "music");
        insert(&db, "/a2.mp3", "Two", "BAND", "Y", "music");
        insert(&db, "/b1.mp3", "Three", "Other", "Y", "music");
        db.record_airing(1, 5_000).unwrap();

        let f = SelectionFilter {
            artist_since: Some(4_000),
            ..Default::default()
        };
        let ids: Vec<i64> = db
            .get_random_tracks("music", 10, &f)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![3], "the other spelling of the same artist too");
    }

    /// An untagged library shares one blank artist, so a blank airing must not
    /// take the whole pool out.
    #[test]
    fn a_blank_artist_airing_blocks_nothing() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a1.mp3", "One", "", "Y", "music");
        insert(&db, "/a2.mp3", "Two", "  ", "Y", "music");
        db.record_airing(1, 5_000).unwrap();

        let f = SelectionFilter {
            artist_since: Some(1),
            ..Default::default()
        };
        assert_eq!(db.get_random_tracks("music", 10, &f).unwrap().len(), 2);
    }

    #[test]
    fn the_artist_blocklist_hides_matching_tracks() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a1.mp3", "One", "Band", "Y", "music");
        insert(&db, "/b1.mp3", "Two", "Other", "Y", "music");
        let keys = vec![artist_key(" BAND ")];
        let f = SelectionFilter {
            artist_keys: &keys,
            ..Default::default()
        };
        let ids: Vec<i64> = db
            .get_random_tracks("music", 10, &f)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![2]);
    }

    /// A purged track keeps its log row with a NULL id; the title rule must not
    /// read that as "no track aired" and blow up or match everything.
    #[test]
    fn a_purged_airing_constrains_by_artist_but_not_by_title() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/gone.mp3", "Gone", "Band", "Y", "music");
        insert(&db, "/kept.mp3", "Kept", "Band", "Y", "music");
        db.record_airing(1, 5_000).unwrap();
        db.reconcile(&Reconcile {
            gone: vec![1],
            now_ms: 6_000,
            ..Default::default()
        })
        .unwrap();
        db.purge_tracks(&[1]).unwrap();

        let by_title = SelectionFilter {
            title_since: Some(1),
            ..Default::default()
        };
        let ids: Vec<i64> = db
            .get_random_tracks("music", 10, &by_title)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![2], "the surviving track is still selectable");

        let by_artist = SelectionFilter {
            artist_since: Some(1),
            ..Default::default()
        };
        assert!(
            db.get_random_tracks("music", 10, &by_artist)
                .unwrap()
                .is_empty(),
            "the snapshot artist outlives the track"
        );
    }

    #[test]
    fn spreading_artists_returns_one_track_per_artist() {
        let db = Db::open_in_memory().unwrap();
        for (i, artist) in ["A", "A", "A", "B", "C"].iter().enumerate() {
            insert(&db, &format!("/m{i}.mp3"), "t", artist, "al", "music");
        }
        let f = SelectionFilter {
            spread_artists: true,
            ..Default::default()
        };
        let mut seen_first: std::collections::HashSet<i64> = Default::default();
        for _ in 0..50 {
            let picked = db.get_random_tracks("music", 10, &f).unwrap();
            let mut keys: Vec<String> = picked.iter().map(|t| artist_key(&t.artist)).collect();
            keys.sort();
            assert_eq!(keys, vec!["a", "b", "c"]);
            seen_first.extend(picked.iter().filter(|t| t.artist == "A").map(|t| t.id));
        }
        assert!(
            seen_first.len() > 1,
            "the artist's representative is random, not the lowest rowid: {seen_first:?}"
        );
    }

    /// An untagged library shares one blank artist. Grouping them together
    /// would cap every block at a single track.
    #[test]
    fn spreading_artists_treats_each_untagged_track_as_its_own() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            insert(&db, &format!("/m{i}.mp3"), "t", "", "al", "music");
        }
        let f = SelectionFilter {
            spread_artists: true,
            ..Default::default()
        };
        assert_eq!(db.get_random_tracks("music", 10, &f).unwrap().len(), 5);
    }

    #[test]
    fn pick_random_from_bottom_excludes_given_ids() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            insert_with_play_count(&db, &format!("/c{i}.mp3"), "commercial", 0);
            // ids 1..=5
        }
        for _ in 0..50 {
            let picked = db
                .pick_random_from_bottom("commercial", 5, 10, &[1, 2, 3, 4])
                .unwrap();
            assert_eq!(picked.len(), 1);
            assert_eq!(picked[0].id, 5);
        }
    }

    #[test]
    fn increment_play_count_persists() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "A", "X", "Y", "music");
        db.increment_play_count(1).unwrap();
        db.increment_play_count(1).unwrap();
        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.play_count, 2);
    }

    #[test]
    fn update_track_metadata_updates_all_fields() {
        let db = Db::open_in_memory().unwrap();
        insert(
            &db,
            "/a.mp3",
            "Old Title",
            "Old Artist",
            "Old Album",
            "music",
        );
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            title: Some("New Title".into()),
            artist: Some("New Artist".into()),
            album: Some("New Album".into()),
            genre: Some(Some("Electronic".into())),
            year: Some(Some(2025)),
            ..Default::default()
        })
        .unwrap();
        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.title, "New Title");
        assert_eq!(t.artist, "New Artist");
        assert_eq!(t.album, "New Album");
        assert_eq!(t.genre, Some("Electronic".into()));
        assert_eq!(t.year, Some(2025));
    }

    #[test]
    fn update_track_metadata_partially_updates() {
        let db = Db::open_in_memory().unwrap();
        insert(
            &db,
            "/a.mp3",
            "Old Title",
            "Old Artist",
            "Old Album",
            "music",
        );
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            title: Some("Updated Title".into()),
            ..Default::default()
        })
        .unwrap();
        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.title, "Updated Title");
        // Other fields unchanged
        assert_eq!(t.artist, "Old Artist");
        assert_eq!(t.album, "Old Album");
    }

    #[test]
    fn update_track_metadata_clears_nullable_fields() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Title", "Artist", "Album", "music");
        // Set genre and year first via a partial update
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            genre: Some(Some("Rock".into())),
            year: Some(Some(2020)),
            ..Default::default()
        })
        .unwrap();
        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.genre.as_deref(), Some("Rock"));
        assert_eq!(t.year, Some(2020));

        // Clear them to NULL
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            genre: Some(None),
            year: Some(None),
            ..Default::default()
        })
        .unwrap();
        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.genre, None::<String>);
        assert_eq!(t.year, None::<i64>);
    }

    #[test]
    fn update_track_metadata_returns_error_for_missing_id() {
        let db = Db::open_in_memory().unwrap();
        let err = db.update_track_metadata(&TrackMetadataUpdate {
            id: 999,
            title: Some("Nope".into()),
            ..Default::default()
        });
        assert!(err.is_err());
    }

    #[test]
    fn update_track_metadata_skips_update_when_no_fields_set() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Original", "Artist", "Album", "music");
        // No fields set — should return the track unchanged without error
        let t = db
            .update_track_metadata(&TrackMetadataUpdate {
                id: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(t.title, "Original");
    }

    #[test]
    fn fts5_search_finds_genre_and_year_after_update() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Hello World", "Band", "Album", "music");
        // FTS5 triggers should pick up changes made via update_track_metadata
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            genre: Some(Some("Jazz".into())),
            year: Some(Some(2023)),
            ..Default::default()
        })
        .unwrap();

        // Title should still be found by FTS
        let results = db.search("hello", None, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn metadata_update_deserialize_distinguishes_absent_null_and_value() {
        // Absent field → None (leave unchanged).
        let absent: TrackMetadataUpdate = serde_json::from_str(r#"{"id":1}"#).unwrap();
        assert_eq!(absent.genre, None);
        assert_eq!(absent.year, None);

        // Explicit JSON null → Some(None) (clear to NULL). Without the
        // `double_option` deserializer serde would collapse this to None,
        // silently turning a "clear" request into a no-op.
        let cleared: TrackMetadataUpdate =
            serde_json::from_str(r#"{"id":1,"genre":null,"year":null}"#).unwrap();
        assert_eq!(cleared.genre, Some(None));
        assert_eq!(cleared.year, Some(None));

        // Present value → Some(Some(v)) (set).
        let set: TrackMetadataUpdate =
            serde_json::from_str(r#"{"id":1,"genre":"Rock","year":2020}"#).unwrap();
        assert_eq!(set.genre, Some(Some("Rock".to_string())));
        assert_eq!(set.year, Some(Some(2020)));
    }

    #[test]
    fn metadata_update_clears_field_via_deserialized_payload() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/a.mp3", "Title", "Artist", "Album", "music");
        db.update_track_metadata(&TrackMetadataUpdate {
            id: 1,
            genre: Some(Some("Rock".into())),
            year: Some(Some(2020)),
            ..Default::default()
        })
        .unwrap();

        // Payload as it arrives over the Tauri IPC boundary from the renderer.
        let update: TrackMetadataUpdate =
            serde_json::from_str(r#"{"id":1,"genre":null,"year":null}"#).unwrap();
        db.update_track_metadata(&update).unwrap();

        let t = db.get_track(1).unwrap().unwrap();
        assert_eq!(t.genre, None::<String>);
        assert_eq!(t.year, None::<i64>);
    }
}
