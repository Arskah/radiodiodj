use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::audio::cue_points::CuePoints;

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

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TracksByType {
    pub music: i64,
    pub commercial: i64,
    pub jingle: i64,
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
    pub needs_waveform: bool,
    pub needs_fingerprint: bool,
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
}

pub struct Db {
    conn: Mutex<Connection>,
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
        })
    }

    pub fn search(
        &self,
        query: &str,
        content_type: Option<&str>,
        sort_by: Option<&str>,
        sort_dir: Option<&str>,
    ) -> Result<Vec<Track>> {
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
            let rows = stmt.query_map(params_from_iter(params.iter()), row_to_track)?;
            return rows.collect::<rusqlite::Result<_>>().map_err(Into::into);
        }

        let fts_q = trimmed
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t))
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
        let rows = stmt.query_map(params_from_iter(params.iter()), row_to_track)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_media_track(&self, id: i64) -> Result<Option<MediaTrack>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT path, duration FROM tracks WHERE id = ?")?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(MediaTrack {
                path: r.get(0)?,
                duration: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
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
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, album, genre, duration, content_type, path, \
                    cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms \
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
                cue_points: row_to_cue_points(r)?,
            })
        })?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    pub fn get_track(&self, id: i64) -> Result<Option<Track>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT * FROM tracks WHERE id = ?")?;
        let mut rows = stmt.query_map([id], row_to_track)?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    pub fn get_tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT * FROM tracks WHERE id IN ({})", placeholders);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(ids.iter()), row_to_track)?;
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
    pub fn set_cue_points(&self, id: i64, points: CuePoints) -> Result<CuePoints> {
        let conn = self.conn.lock();
        let duration: Option<f64> = conn
            .query_row("SELECT duration FROM tracks WHERE id = ?", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        let clamped = points.clamp(duration);
        let changed = conn.execute(
            "UPDATE tracks SET cue_in_ms = ?, fade_in_ms = ?, fade_out_ms = ?, \
                    cue_out_ms = ?, next_start_ms = ? \
             WHERE id = ?",
            params![
                clamped.cue_in_ms,
                clamped.fade_in_ms,
                clamped.fade_out_ms,
                clamped.cue_out_ms,
                clamped.next_start_ms,
                id
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("track {} is no longer in the library", id);
        }
        Ok(clamped)
    }

    /// Every present track still missing a waveform or a fingerprint, ordered
    /// by id. Drives the background analysis worker (backfill included).
    pub fn tracks_needing_analysis(&self) -> Result<Vec<AnalysisJob>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, waveform IS NULL, fingerprint IS NULL FROM tracks \
             WHERE missing_since IS NULL AND (waveform IS NULL OR fingerprint IS NULL) \
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AnalysisJob {
                id: r.get(0)?,
                path: r.get(1)?,
                needs_waveform: r.get(2)?,
                needs_fingerprint: r.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn set_fingerprint(&self, id: i64, fingerprint: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET fingerprint = ? WHERE id = ?",
            params![fingerprint, id],
        )?;
        Ok(())
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
            let mut reattach = tx.prepare(
                "UPDATE tracks SET path = ?1, content_type = ?2, mtime = ?3, \
                        missing_since = NULL \
                 WHERE id = ?4",
            )?;
            let mut present_twin = tx.prepare(
                "SELECT id FROM tracks WHERE fingerprint = ?1 AND missing_since IS NULL \
                 ORDER BY id LIMIT 1",
            )?;
            let mut insert = tx.prepare(&format!("{UPSERT_TRACK_SQL} RETURNING id"))?;
            let mut copy_state = tx.prepare(
                "UPDATE tracks SET title = s.title, artist = s.artist, album = s.album, \
                        genre = s.genre, year = s.year, bpm = s.bpm, \
                        play_count = s.play_count, waveform = s.waveform, \
                        cue_in_ms = s.cue_in_ms, fade_in_ms = s.fade_in_ms, \
                        fade_out_ms = s.fade_out_ms, cue_out_ms = s.cue_out_ms, \
                        next_start_ms = s.next_start_ms \
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

    pub fn increment_play_count(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE tracks SET play_count = play_count + 1 WHERE id = ?",
            [id],
        )?;
        Ok(())
    }

    /// Update metadata fields for a track. Only non-None fields are included
    /// in the UPDATE. Returns the updated [`Track`] so the caller can push it to
    /// the renderer as a fast-forward replacement; the update path never touches
    /// `play_count`, `waveform`, or `added_at`.
    pub fn update_track_metadata(&self, updates: &TrackMetadataUpdate) -> Result<Track> {
        let mut setters = Vec::<String>::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(v) = &updates.title {
            setters.push("title=?".to_string());
            params.push(rusqlite::types::Value::Text(v.clone()));
        }
        if let Some(v) = &updates.artist {
            setters.push("artist=?".to_string());
            params.push(rusqlite::types::Value::Text(v.clone()));
        }
        if let Some(v) = &updates.album {
            setters.push("album=?".to_string());
            params.push(rusqlite::types::Value::Text(v.clone()));
        }
        if updates.genre.is_some() {
            setters.push("genre=?".to_string());
            params.push(match &updates.genre {
                Some(Some(s)) => rusqlite::types::Value::Text(s.clone()),
                _ => rusqlite::types::Value::Null,
            });
        }
        if updates.year.is_some() {
            setters.push("year=?".to_string());
            params.push(match updates.year {
                Some(Some(i)) => rusqlite::types::Value::Integer(i),
                _ => rusqlite::types::Value::Null,
            });
        }
        if let Some(v) = &updates.content_type {
            setters.push("content_type=?".to_string());
            params.push(rusqlite::types::Value::Text(v.clone()));
        }

        if setters.is_empty() {
            return self
                .get_track(updates.id)?
                .ok_or_else(|| anyhow::anyhow!("track not found"));
        }

        let sql = format!("UPDATE tracks SET {} WHERE id = ?", setters.join(", "));
        // Add the WHERE `id` parameter after the dynamic value params.
        params.push(rusqlite::types::Value::Integer(updates.id));
        let n = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(&sql)?;
            stmt.execute(params_from_iter(params.iter()))?
        };

        if n == 0 {
            return Err(anyhow::anyhow!("track not found"));
        }

        self.get_track(updates.id)?
            .ok_or_else(|| anyhow::anyhow!("track not found"))
    }

    pub fn get_random_tracks(
        &self,
        content_type: &str,
        count: i64,
        exclude_ids: &[i64],
    ) -> Result<Vec<Track>> {
        if count <= 0 {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        // `NOT IN ()` is a syntax error in SQLite, so only add the clause when
        // there is something to exclude.
        let exclude_clause = exclude_sql(exclude_ids);
        let sql = format!(
            "SELECT * FROM tracks WHERE missing_since IS NULL AND content_type = ?{exclude_clause} \
             ORDER BY RANDOM() LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(
            std::iter::once(rusqlite::types::Value::Text(content_type.to_owned()))
                .chain(
                    exclude_ids
                        .iter()
                        .map(|&id| rusqlite::types::Value::Integer(id)),
                )
                .chain(std::iter::once(rusqlite::types::Value::Integer(count))),
        );
        let rows = stmt.query_map(params, row_to_track)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn pick_random_from_bottom(
        &self,
        content_type: &str,
        count: i64,
        bucket_size: i64,
        exclude_ids: &[i64],
    ) -> Result<Vec<Track>> {
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
        let rows = stmt.query_map(params, row_to_track)?;
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

fn row_to_track(row: &Row) -> rusqlite::Result<Track> {
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
        cue_points: row_to_cue_points(row)?,
    })
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
const MIGRATION_STEPS: &[M] = &[M::up(BASELINE)];
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

/// Upsert one present track's metadata by path. The operator-work columns are
/// deliberately absent: the waveform is filled asynchronously by the waveform
/// worker (`set_waveform`), and cue points and play counts are operator work a
/// metadata rescan must never clobber.
const UPSERT_TRACK_SQL: &str = "INSERT INTO tracks \
     (path, content_type, title, artist, album, genre, year, duration, bpm, \
      sample_rate, bitrate, format, mtime, fingerprint) \
     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
     ON CONFLICT(path) WHERE missing_since IS NULL DO UPDATE SET \
        content_type=excluded.content_type, \
        title=excluded.title, artist=excluded.artist, album=excluded.album, \
        genre=excluded.genre, year=excluded.year, duration=excluded.duration, \
        bpm=excluded.bpm, sample_rate=excluded.sample_rate, \
        bitrate=excluded.bitrate, format=excluded.format, mtime=excluded.mtime, \
        fingerprint=COALESCE(excluded.fingerprint, fingerprint)";

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
    const SEEDS: &[fn(&Connection)] = &[|conn| {
        conn.execute_batch(
            "INSERT INTO tracks (path, content_type, title, artist, album, duration, \
                                 play_count, waveform, cue_in_ms, fingerprint) \
             VALUES ('/seed.mp3', 'music', 'Seed', 'A', 'B', 100.0, 3, x'00ff', 1000, 'v1:ab')",
        )
        .unwrap();
    }];

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
            only_here(db.get_random_tracks("music", 10, &[]).unwrap()),
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
    fn analysis_lists_tracks_until_both_waveform_and_fingerprint_are_filled() {
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
        assert!(jobs.iter().all(|j| j.needs_waveform && j.needs_fingerprint));

        let (a, b) = (jobs[0].id, jobs[1].id);
        db.set_waveform(a, &[9]).unwrap();
        db.set_waveform(b, &[9]).unwrap();
        db.set_fingerprint(b, "v1:b").unwrap();

        let jobs = db.tracks_needing_analysis().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, a);
        assert!(!jobs[0].needs_waveform);
        assert!(jobs[0].needs_fingerprint);
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
            let picked = db.get_random_tracks("music", 5, &[1, 2, 4, 5]).unwrap();
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
        let picked = db.get_random_tracks("music", 5, &[]).unwrap();
        assert_eq!(picked.len(), 2);
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
