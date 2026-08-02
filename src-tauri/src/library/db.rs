use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, params_from_iter, Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackMetadataUpdate {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
}

pub struct TrackMtimeRow {
    pub content_type: String,
    pub mtime: Option<i64>,
}

pub struct MediaTrack {
    pub path: String,
    pub duration: f64,
}

pub struct TrackBroadcastInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: Option<String>,
    pub duration: f64,
    pub content_type: String,
    pub path: String,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("open sqlite")?;
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
        Self::with_connection(conn)
    }

    fn with_connection(conn: Connection) -> Result<Self> {
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v >= SCHEMA_VERSION {
            return Ok(());
        }
        // Apply pending steps and bump user_version in a single transaction so
        // the schema change and its version bump commit together. An interrupted
        // migration rolls back cleanly instead of leaving the DB half-applied
        // (schema ahead of version), which on the next launch would re-run an
        // ADD COLUMN and fail with "duplicate column name".
        let tx = conn.transaction()?;
        if v < 1 {
            tx.execute_batch(MIGRATION_001)?;
        }
        if v < 2 {
            tx.execute_batch(MIGRATION_002)?;
        }
        if v < 3 {
            tx.execute_batch(MIGRATION_003)?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
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
                        "SELECT * FROM tracks WHERE content_type = ? ORDER BY {} LIMIT 200",
                        order_sql
                    ),
                    vec![t.to_owned().into()],
                )
            } else {
                (
                    format!("SELECT * FROM tracks ORDER BY {} LIMIT 200", order_sql),
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
                     WHERE tracks_fts MATCH ? AND tracks.content_type = ? \
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
                     WHERE tracks_fts MATCH ? \
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

    pub fn get_track_broadcast_info(&self, id: i64) -> Result<Option<TrackBroadcastInfo>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, artist, album, genre, duration, content_type, path \
             FROM tracks WHERE id = ?",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(TrackBroadcastInfo {
                id: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                artist: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                album: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                genre: r.get::<_, Option<String>>(4)?,
                duration: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                content_type: r.get(6)?,
                path: r.get(7)?,
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
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
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
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id, path FROM tracks WHERE id IN ({})", placeholders);
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

    /// `(id, path, duration)` for every track still missing a waveform, ordered
    /// by id. Drives the background waveform worker's work list (backfill
    /// included); the duration seeds the peak-bucket sizing so the worker
    /// decodes each file only once.
    pub fn tracks_missing_waveform(&self) -> Result<Vec<(i64, String, Option<f64>)>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, path, duration FROM tracks WHERE waveform IS NULL ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<f64>>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Single-track upsert. Only the tests need this; the scanner batches via
    /// [`Db::insert_tracks`].
    #[cfg(test)]
    pub fn insert_track(&self, t: &TrackInsert) -> Result<()> {
        self.insert_tracks(std::slice::from_ref(t))
    }

    /// Upsert many tracks in a single transaction with one prepared statement.
    /// Used by the scanner: a per-row autocommit costs a WAL commit each, which
    /// dominates a large scan — one transaction turns thousands of commits into
    /// one. A no-op for an empty slice.
    pub fn insert_tracks(&self, tracks: &[TrackInsert]) -> Result<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(UPSERT_TRACK_SQL)?;
            for t in tracks {
                stmt.execute(upsert_params(t))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Map of `path -> (content_type, mtime)` for every track under `root`, in a
    /// single query. Lets the scanner decide what to re-parse without a
    /// per-file SELECT.
    pub fn track_meta_under(&self, root: &str) -> Result<HashMap<String, TrackMtimeRow>> {
        let conn = self.conn.lock();
        let pattern = format!("{}%", root);
        let mut stmt =
            conn.prepare("SELECT path, content_type, mtime FROM tracks WHERE path LIKE ?")?;
        let rows = stmt.query_map([pattern], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TrackMtimeRow {
                    content_type: r.get(1)?,
                    mtime: r.get::<_, Option<i64>>(2)?,
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_paths_under(&self, root: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let pattern = format!("{}%", root);
        let mut stmt = conn.prepare("SELECT path FROM tracks WHERE path LIKE ?")?;
        let rows = stmt.query_map([pattern], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn delete_by_paths(&self, paths: &[String]) -> Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut total = 0usize;
        for chunk in paths.chunks(500) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM tracks WHERE path IN ({})", placeholders);
            total += tx.execute(&sql, params_from_iter(chunk.iter()))?;
        }
        tx.commit()?;
        Ok(total)
    }

    pub fn remove_tracks_not_in_paths(&self, roots: &[String]) -> Result<usize> {
        let conn = self.conn.lock();
        if roots.is_empty() {
            let n = conn.execute("DELETE FROM tracks", [])?;
            return Ok(n);
        }
        let mut where_parts = Vec::new();
        let mut p: Vec<rusqlite::types::Value> = Vec::new();
        for r in roots {
            where_parts.push("path NOT LIKE ?".to_string());
            p.push(format!("{}%", r).into());
        }
        let sql = format!("DELETE FROM tracks WHERE {}", where_parts.join(" AND "));
        let n = conn.execute(&sql, params_from_iter(p.iter()))?;
        Ok(n)
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
            "SELECT * FROM tracks WHERE content_type = ?{exclude_clause} \
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
                SELECT * FROM tracks WHERE content_type = ?{exclude_clause} \
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
                 COALESCE(ROUND(SUM(duration) / 3600.0, 1), 0) FROM tracks",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut tracks_by_type = TracksByType::default();
        let mut stmt =
            conn.prepare("SELECT content_type, COUNT(*) FROM tracks GROUP BY content_type")?;
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
fn upsert_params(t: &TrackInsert) -> [&dyn rusqlite::ToSql; 13] {
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
    })
}

const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT UNIQUE NOT NULL,
  content_type TEXT NOT NULL DEFAULT 'music',
  title TEXT,
  artist TEXT,
  album TEXT,
  genre TEXT,
  year INTEGER,
  duration REAL,
  bpm REAL,
  sample_rate INTEGER,
  bitrate INTEGER,
  format TEXT,
  play_count INTEGER NOT NULL DEFAULT 0,
  added_at TEXT DEFAULT (datetime('now'))
);

CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
  title, artist, album, genre,
  content='tracks',
  content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS tracks_ai AFTER INSERT ON tracks BEGIN
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;

CREATE TRIGGER IF NOT EXISTS tracks_ad AFTER DELETE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
END;

CREATE TRIGGER IF NOT EXISTS tracks_au AFTER UPDATE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;
"#;

/// Current schema version. Bump alongside every new `MIGRATION_00N`.
const SCHEMA_VERSION: i64 = 3;

/// Upsert one track's metadata by path. The waveform column is deliberately
/// absent: metadata scans run tag-only and fast, and the waveform is filled
/// asynchronously by the waveform worker (`set_waveform`). Omitting it here
/// means a metadata rescan never clobbers an already-computed waveform.
const UPSERT_TRACK_SQL: &str = "INSERT INTO tracks \
     (path, content_type, title, artist, album, genre, year, duration, bpm, \
      sample_rate, bitrate, format, mtime) \
     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?) \
     ON CONFLICT(path) DO UPDATE SET \
        content_type=excluded.content_type, \
        title=excluded.title, artist=excluded.artist, album=excluded.album, \
        genre=excluded.genre, year=excluded.year, duration=excluded.duration, \
        bpm=excluded.bpm, sample_rate=excluded.sample_rate, \
        bitrate=excluded.bitrate, format=excluded.format, mtime=excluded.mtime";

const MIGRATION_002: &str = "ALTER TABLE tracks ADD COLUMN mtime INTEGER;";

/// Amplitude-curve peaks for the seek UI, one byte per bucket. Nullable so rows
/// scanned before this column (or files that failed to decode) simply have no
/// waveform.
const MIGRATION_003: &str = "ALTER TABLE tracks ADD COLUMN waveform BLOB;";

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

    #[test]
    fn migrate_sets_user_version() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn.lock();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn migrate_from_v2_adds_waveform_and_bumps_version() {
        // A released v2 DB (has mtime, no waveform) migrates cleanly to v3.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        conn.execute_batch(MIGRATION_002).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();

        let db = Db::with_connection(conn).expect("migrate v2 -> v3");
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        let conn = db.conn.lock();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
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
    fn tracks_missing_waveform_lists_only_unfilled() {
        let db = Db::open_in_memory().unwrap();
        db.insert_track(&TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        db.insert_track(&TrackInsert {
            path: "/b.mp3".into(),
            content_type: "music".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(db.tracks_missing_waveform().unwrap().len(), 2);

        let first = db.tracks_missing_waveform().unwrap()[0].0;
        db.set_waveform(first, &[9]).unwrap();
        let remaining = db.tracks_missing_waveform().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_ne!(remaining[0].0, first);
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
}
