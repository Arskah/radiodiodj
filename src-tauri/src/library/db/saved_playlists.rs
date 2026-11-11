//! Saved playlists: named, stored, ordered lists of entries. See
//! `docs/saved-playlists.md`.

use std::collections::HashSet;

use anyhow::{anyhow, bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{placeholders, Db, Track, ID_CHUNK};

/// Whether a saved list can hold deliberate Stop markers.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SavedPlaylistKind {
    /// An ordinary saved playlist containing tracks only.
    #[default]
    Playlist,
    /// An authored sequence that may include Stop markers.
    PreparedShow,
}

impl SavedPlaylistKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::PreparedShow => "prepared_show",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "playlist" => Ok(Self::Playlist),
            "prepared_show" => Ok(Self::PreparedShow),
            _ => bail!("unknown saved playlist kind {value}"),
        }
    }
}

/// An authored saved entry, in the order it will air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedEntryInput {
    /// Add the track with this library id.
    Track(i64),
    /// Add a deliberate Stop marker.
    Stop,
}

/// A saved entry ready to append to the on-air queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedQueueEntry {
    /// A bound track and its current content type.
    Track {
        /// The library track id.
        id: i64,
        /// The track's current content type.
        content_type: String,
    },
    /// A deliberate Stop marker.
    Stop,
}

/// Whether one saved entry names audio or marks a pause.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SavedEntryKind {
    /// A track entry, including an unmatched one.
    #[default]
    Track,
    /// A deliberate Stop marker with no audio identity.
    Stop,
}

impl SavedEntryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Stop => "stop",
        }
    }
}

/// One row of the saved playlists list.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaylistSummary {
    pub id: i64,
    pub name: String,
    /// Whether the list permits Stops.
    pub kind: SavedPlaylistKind,
    pub entries: i64,
    /// Deliberate Stop markers among `entries`.
    pub stops: i64,
    /// Entries that cannot air: unmatched, or bound to a missing or hidden
    /// track.
    pub missing: i64,
}

/// One entry of a saved playlist. `track` is the live track it is bound to;
/// the rest is what the entry was written with, shown when nothing is bound.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedEntry {
    pub id: i64,
    /// Distinguishes an unmatched track from a Stop.
    pub kind: SavedEntryKind,
    pub track: Option<Track>,
    pub artist: String,
    pub title: String,
    pub duration: f64,
    pub content_type: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaylist {
    pub id: i64,
    pub name: String,
    /// Whether the list permits Stops.
    pub kind: SavedPlaylistKind,
    pub entries: Vec<SavedEntry>,
}

/// What a saved playlist offers the on-air playlist: its bound tracks in
/// order, each with its content type, and how many entries had none.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTracks {
    pub tracks: Vec<(i64, String)>,
    pub unmatched: usize,
}

/// The `format` every saved playlist file carries.
pub const FILE_FORMAT: &str = "radiodiodj-playlist";
/// The file version this build writes. Version 1 track-only files remain readable.
pub const FILE_VERSION: u32 = 2;

/// A saved playlist as it travels between installs: no track ids and no paths,
/// only what identifies a recording anywhere. See `docs/saved-playlists.md`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaylistFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// Defaults to playlist for legacy version 1 files.
    #[serde(default)]
    pub kind: SavedPlaylistKind,
    pub entries: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// Defaults to track for legacy version 1 files.
    #[serde(default)]
    pub kind: SavedEntryKind,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub content_type: Option<String>,
}

impl SavedPlaylistFile {
    /// Parse a file, refusing whole anything this build does not know how to
    /// read rather than importing what it happens to understand.
    pub fn parse(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Header {
            format: Option<String>,
            version: Option<u32>,
        }
        let header: Header =
            serde_json::from_str(json).map_err(|e| anyhow!("not a saved playlist file: {e}"))?;
        if header.format.as_deref() != Some(FILE_FORMAT) {
            bail!("not a saved playlist file");
        }
        match header.version {
            Some(1 | FILE_VERSION) => {}
            Some(v) => bail!(
                "this saved playlist file is version {v}, and this build reads version \
                 {FILE_VERSION}"
            ),
            None => bail!("not a saved playlist file"),
        }
        let file: Self =
            serde_json::from_str(json).map_err(|e| anyhow!("not a saved playlist file: {e}"))?;
        if file.version == 1
            && (file.kind != SavedPlaylistKind::Playlist
                || file.entries.iter().any(|e| e.kind != SavedEntryKind::Track))
        {
            bail!("version 1 saved playlist files can contain tracks only");
        }
        validate_entries(file.kind, &file.entries)?;
        Ok(file)
    }
}

fn validate_entries(kind: SavedPlaylistKind, entries: &[FileEntry]) -> Result<()> {
    if kind == SavedPlaylistKind::Playlist && entries.iter().any(|e| e.kind == SavedEntryKind::Stop)
    {
        bail!("a playlist cannot contain Stops");
    }
    if entries.iter().any(|e| {
        e.kind == SavedEntryKind::Stop
            && (e.fingerprint.is_some()
                || e.artist.is_some()
                || e.title.is_some()
                || e.duration.is_some()
                || e.content_type.is_some())
    }) {
        bail!("a Stop cannot carry audio identity");
    }
    Ok(())
}

/// Bind every unmatched entry to the present track with its fingerprint. Two
/// such tracks are told apart by the entry's duration, then by id.
fn bind(conn: &Connection) -> Result<usize> {
    const MATCH: &str = "FROM tracks t \
         WHERE t.fingerprint = saved_playlist_entries.fingerprint AND t.missing_since IS NULL AND t.hidden_at IS NULL";
    let sql = format!(
        "UPDATE saved_playlist_entries SET track_id = ( \
           SELECT t.id {MATCH} \
           ORDER BY ABS(COALESCE(t.duration, 0) - COALESCE(saved_playlist_entries.duration, 0)), \
                    t.id \
           LIMIT 1) \
         WHERE kind = 'track' AND track_id IS NULL AND fingerprint IS NOT NULL AND EXISTS (SELECT 1 {MATCH})"
    );
    Ok(conn.execute(&sql, [])?)
}

/// A name as it is compared: trimmed, and without regard to case.
fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn names(conn: &Connection, except: Option<i64>) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id, name FROM saved_playlists")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut taken = Vec::new();
    for row in rows {
        let (id, name) = row?;
        if Some(id) != except {
            taken.push(name_key(&name));
        }
    }
    Ok(taken)
}

/// `wanted`, or the first of `wanted (2)`, `wanted (3)`… that is free.
fn free_name(conn: &Connection, wanted: &str) -> Result<String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        bail!("a saved playlist needs a name");
    }
    let taken = names(conn, None)?;
    if !taken.contains(&name_key(wanted)) {
        return Ok(wanted.to_owned());
    }
    let mut n = 2;
    loop {
        let candidate = format!("{wanted} ({n})");
        if !taken.contains(&name_key(&candidate)) {
            return Ok(candidate);
        }
        n += 1;
    }
}

fn entry_ids(conn: &Connection, playlist_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM saved_playlist_entries WHERE playlist_id = ? ORDER BY position, id",
    )?;
    let rows = stmt.query_map([playlist_id], |r| r.get::<_, i64>(0))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn write_order(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare("UPDATE saved_playlist_entries SET position = ? WHERE id = ?")?;
    for (position, id) in ids.iter().enumerate() {
        stmt.execute(params![position as i64, id])?;
    }
    Ok(())
}

/// Append an entry per track id, each with a snapshot of the track as it reads
/// now, and hand back the new entry ids. An id with no row adds nothing.
fn insert_entries(conn: &Connection, playlist_id: i64, track_ids: &[i64]) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "INSERT INTO saved_playlist_entries \
           (playlist_id, position, track_id, fingerprint, artist, title, duration, content_type) \
         SELECT ?1, -1, id, fingerprint, artist, title, duration, content_type \
         FROM tracks WHERE id = ?2",
    )?;
    let mut added = Vec::with_capacity(track_ids.len());
    for track_id in track_ids {
        if stmt.execute(params![playlist_id, track_id])? == 1 {
            added.push(conn.last_insert_rowid());
        }
    }
    Ok(added)
}

fn touch(conn: &Connection, playlist_id: i64, now_ms: i64) -> Result<()> {
    let changed = conn.execute(
        "UPDATE saved_playlists SET updated_at = ? WHERE id = ?",
        params![now_ms, playlist_id],
    )?;
    if changed == 0 {
        bail!("no saved playlist {playlist_id}");
    }
    Ok(())
}

impl Db {
    pub fn saved_playlists(&self) -> Result<Vec<SavedPlaylistSummary>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.kind, COUNT(e.id), \
                    COALESCE(SUM(e.kind = 'stop'), 0), \
                    COALESCE(SUM(e.id IS NOT NULL AND e.kind = 'track' \
                                 AND (t.id IS NULL OR t.missing_since IS NOT NULL \
                                      OR t.hidden_at IS NOT NULL)), 0) \
             FROM saved_playlists p \
             LEFT JOIN saved_playlist_entries e ON e.playlist_id = p.id \
             LEFT JOIN tracks t ON t.id = e.track_id \
             GROUP BY p.id ORDER BY p.name COLLATE NOCASE, p.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SavedPlaylistSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: SavedPlaylistKind::from_db(&r.get::<_, String>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                entries: r.get(3)?,
                stops: r.get(4)?,
                missing: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn saved_playlist(&self, id: i64) -> Result<Option<SavedPlaylist>> {
        type Row = (
            i64,
            SavedEntryKind,
            Option<i64>,
            String,
            String,
            f64,
            Option<String>,
        );
        let (name, kind, rows): (String, SavedPlaylistKind, Vec<Row>) = {
            let conn = self.conn.lock();
            let Some((name, kind)) = conn
                .query_row(
                    "SELECT name, kind FROM saved_playlists WHERE id = ?",
                    [id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
            else {
                return Ok(None);
            };
            let mut stmt = conn.prepare(
                "SELECT e.id, e.kind, e.track_id, COALESCE(e.artist, ''), \
                        COALESCE(e.title, ''), COALESCE(e.duration, 0), \
                        COALESCE(t.content_type, e.content_type) \
                 FROM saved_playlist_entries e LEFT JOIN tracks t ON t.id = e.track_id \
                 WHERE e.playlist_id = ? ORDER BY e.position, e.id",
            )?;
            let rows: Vec<Row> = stmt
                .query_map([id], |r| {
                    Ok((
                        r.get(0)?,
                        match r.get::<_, String>(1)?.as_str() {
                            "stop" => SavedEntryKind::Stop,
                            "track" => SavedEntryKind::Track,
                            _ => return Err(rusqlite::Error::InvalidQuery),
                        },
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let kind = SavedPlaylistKind::from_db(&kind)?;
            if kind == SavedPlaylistKind::Playlist
                && rows.iter().any(|row| row.1 == SavedEntryKind::Stop)
            {
                bail!("a playlist cannot contain Stops");
            }
            (name, kind, rows)
        };
        let mut bound: Vec<i64> = rows.iter().filter_map(|r| r.2).collect();
        bound.sort_unstable();
        bound.dedup();
        let mut tracks = std::collections::HashMap::new();
        for chunk in bound.chunks(ID_CHUNK) {
            for track in self.get_tracks_by_ids(chunk)? {
                tracks.insert(track.id, track);
            }
        }
        let entries = rows
            .into_iter()
            .map(
                |(id, kind, track_id, artist, title, duration, content_type)| SavedEntry {
                    id,
                    kind,
                    track: track_id.and_then(|t| tracks.get(&t).cloned()),
                    artist,
                    title,
                    duration,
                    content_type,
                },
            )
            .collect();
        Ok(Some(SavedPlaylist {
            id,
            name,
            kind,
            entries,
        }))
    }

    /// Create a saved playlist holding `track_ids` in order. A name in use
    /// takes the next free one, so making a list never fails on its name.
    #[cfg(test)]
    pub fn create_saved_playlist(
        &self,
        name: &str,
        track_ids: &[i64],
        now_ms: i64,
    ) -> Result<SavedPlaylistSummary> {
        let entries: Vec<_> = track_ids
            .iter()
            .copied()
            .map(SavedEntryInput::Track)
            .collect();
        self.create_saved_playlist_with_entries(name, SavedPlaylistKind::Playlist, &entries, now_ms)
    }

    /// Create a list from ordered tracks and Stops. A playlist cannot contain Stops.
    pub fn create_saved_playlist_with_entries(
        &self,
        name: &str,
        kind: SavedPlaylistKind,
        entries: &[SavedEntryInput],
        now_ms: i64,
    ) -> Result<SavedPlaylistSummary> {
        if kind == SavedPlaylistKind::Playlist && entries.contains(&SavedEntryInput::Stop) {
            bail!("a playlist cannot contain Stops");
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let name = free_name(&tx, name)?;
        tx.execute(
            "INSERT INTO saved_playlists (name, created_at, updated_at, kind) VALUES (?, ?, ?, ?)",
            params![name, now_ms, now_ms, kind.as_str()],
        )?;
        let id = tx.last_insert_rowid();
        let mut added = Vec::with_capacity(entries.len());
        let mut stops = 0;
        for entry in entries {
            match entry {
                SavedEntryInput::Track(track_id) => {
                    added.extend(insert_entries(&tx, id, &[*track_id])?);
                }
                SavedEntryInput::Stop => {
                    tx.execute(
                        "INSERT INTO saved_playlist_entries (playlist_id, position, kind) VALUES (?, -1, 'stop')",
                        [id],
                    )?;
                    added.push(tx.last_insert_rowid());
                    stops += 1;
                }
            }
        }
        write_order(&tx, &added)?;
        tx.commit()?;
        Ok(SavedPlaylistSummary {
            id,
            name,
            kind,
            entries: added.len() as i64,
            stops,
            missing: 0,
        })
    }

    /// Change a list's kind. Removing Stops is required before a downgrade.
    pub fn set_saved_playlist_kind(
        &self,
        id: i64,
        kind: SavedPlaylistKind,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        if kind == SavedPlaylistKind::Playlist {
            let stops: i64 = conn.query_row(
                "SELECT COUNT(*) FROM saved_playlist_entries WHERE playlist_id = ? AND kind = 'stop'",
                [id], |r| r.get(0)
            )?;
            if stops > 0 {
                bail!("remove Stops before changing to a playlist");
            }
        }
        let changed = conn.execute(
            "UPDATE saved_playlists SET kind = ?, updated_at = ? WHERE id = ?",
            params![kind.as_str(), now_ms, id],
        )?;
        if changed == 0 {
            bail!("no saved playlist {id}");
        }
        Ok(())
    }

    /// Insert a Stop in stored order, or append it when `index` is absent.
    pub fn add_saved_stop(
        &self,
        playlist_id: i64,
        index: Option<usize>,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let kind: String = tx.query_row(
            "SELECT kind FROM saved_playlists WHERE id = ?",
            [playlist_id],
            |r| r.get(0),
        )?;
        if kind != "prepared_show" {
            bail!("a playlist cannot contain Stops");
        }
        touch(&tx, playlist_id, now_ms)?;
        let mut order = entry_ids(&tx, playlist_id)?;
        tx.execute("INSERT INTO saved_playlist_entries (playlist_id, position, kind) VALUES (?, -1, 'stop')", [playlist_id])?;
        let at = index.map_or(order.len(), |i| i.min(order.len()));
        order.insert(at, tx.last_insert_rowid());
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    /// Add entries for `track_ids`, in order, ahead of the entry at `index`.
    /// `None` appends, and so does an index past the end.
    pub fn add_saved_entries(
        &self,
        playlist_id: i64,
        track_ids: &[i64],
        index: Option<usize>,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        touch(&tx, playlist_id, now_ms)?;
        let mut order = entry_ids(&tx, playlist_id)?;
        let added = insert_entries(&tx, playlist_id, track_ids)?;
        let at = index.map_or(order.len(), |i| i.min(order.len()));
        order.splice(at..at, added);
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_saved_entry(&self, entry_id: i64, now_ms: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let playlist_id: i64 = tx
            .query_row(
                "DELETE FROM saved_playlist_entries WHERE id = ? RETURNING playlist_id",
                [entry_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow!("no saved playlist entry {entry_id}"))?;
        touch(&tx, playlist_id, now_ms)?;
        let order = entry_ids(&tx, playlist_id)?;
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    /// Bind an entry to the track the operator chose for it. The entry's
    /// snapshot is rewritten from that track, so it reads from then on as one
    /// added from the library. A missing or hidden track is refused.
    pub fn bind_saved_entry(&self, entry_id: i64, track_id: i64, now_ms: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let playlist_id: i64 = tx
            .query_row(
                "UPDATE saved_playlist_entries                  SET track_id = t.id,                      fingerprint = COALESCE(t.fingerprint, saved_playlist_entries.fingerprint),                      artist = t.artist, title = t.title, duration = t.duration,                      content_type = t.content_type                  FROM tracks t                  WHERE saved_playlist_entries.id = ?1 AND saved_playlist_entries.kind = 'track' AND t.id = ?2 AND t.missing_since IS NULL AND t.hidden_at IS NULL                  RETURNING playlist_id",
                params![entry_id, track_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                anyhow!("no saved playlist entry {entry_id}, or no present track {track_id}")
            })?;
        touch(&tx, playlist_id, now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Remove several entries of one saved playlist at once. Ids that are not
    /// its entries are ignored.
    pub fn remove_saved_entries(
        &self,
        playlist_id: i64,
        entry_ids_to_remove: &[i64],
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        touch(&tx, playlist_id, now_ms)?;
        for chunk in entry_ids_to_remove.chunks(ID_CHUNK) {
            let sql = format!(
                "DELETE FROM saved_playlist_entries WHERE playlist_id = ?1 AND id IN ({})",
                super::placeholders_from(chunk.len(), 2)
            );
            let params = std::iter::once(playlist_id).chain(chunk.iter().copied());
            tx.execute(&sql, rusqlite::params_from_iter(params))?;
        }
        let order = entry_ids(&tx, playlist_id)?;
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    /// Move several entries as one block, in the order given, into the gap at
    /// `gap` — a position counted in the list as it stands, the moved entries
    /// included. A gap past the end is the end. Ids that are not entries of
    /// this saved playlist are ignored.
    pub fn move_saved_entries(
        &self,
        playlist_id: i64,
        moved: &[i64],
        gap: usize,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        touch(&tx, playlist_id, now_ms)?;
        let order = entry_ids(&tx, playlist_id)?;
        let present: HashSet<i64> = order.iter().copied().collect();
        let block: Vec<i64> = moved
            .iter()
            .copied()
            .filter(|id| present.contains(id))
            .collect();
        let lifted: HashSet<i64> = block.iter().copied().collect();
        // Where the gap falls once the block has been lifted out.
        let at = order
            .iter()
            .take(gap.min(order.len()))
            .filter(|id| !lifted.contains(id))
            .count();
        let mut order: Vec<i64> = order
            .into_iter()
            .filter(|id| !lifted.contains(id))
            .collect();
        order.splice(at..at, block);
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    /// Move the entry at `from` so that it ends up at `to`. Indices past the
    /// end are clamped: they describe a list that has since shrunk.
    pub fn move_saved_entry(
        &self,
        playlist_id: i64,
        from: usize,
        to: usize,
        now_ms: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        touch(&tx, playlist_id, now_ms)?;
        let mut order = entry_ids(&tx, playlist_id)?;
        if from >= order.len() {
            return Ok(());
        }
        let moved = order.remove(from);
        order.insert(to.min(order.len()), moved);
        write_order(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    /// Rename a saved playlist. Refused when another one holds the name.
    pub fn rename_saved_playlist(&self, id: i64, name: &str, now_ms: i64) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a saved playlist needs a name");
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if names(&tx, Some(id))?.contains(&name_key(name)) {
            bail!("a saved playlist named \u{201c}{name}\u{201d} already exists");
        }
        let changed = tx.execute(
            "UPDATE saved_playlists SET name = ?, updated_at = ? WHERE id = ?",
            params![name, now_ms, id],
        )?;
        if changed == 0 {
            bail!("no saved playlist {id}");
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_saved_playlist(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM saved_playlist_entries WHERE playlist_id = ?",
            [id],
        )?;
        tx.execute("DELETE FROM saved_playlists WHERE id = ?", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// A saved playlist as an auto-playlist source: its name, and how many
    /// distinct music tracks it can put on air.
    pub fn saved_playlist_pool(&self, id: i64) -> Result<Option<(String, i64)>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT p.name, ( \
               SELECT COUNT(DISTINCT t.id) \
               FROM saved_playlist_entries e JOIN tracks t ON t.id = e.track_id \
               WHERE e.playlist_id = p.id AND e.kind = 'track' AND t.missing_since IS NULL AND t.hidden_at IS NULL \
                 AND t.content_type = 'music') \
             FROM saved_playlists p WHERE p.id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(Into::into)
    }

    /// Bind unmatched entries whose file has arrived. Idempotent, and cheap
    /// enough to run whenever a fingerprint may have been written. Returns how
    /// many entries it bound.
    pub fn bind_saved_entries(&self) -> Result<usize> {
        bind(&self.conn.lock())
    }

    /// A saved playlist as a file. A bound entry is written as its track reads
    /// now, so a fingerprint version bump never exports a stale identity.
    pub fn export_saved_playlist(&self, id: i64) -> Result<Option<SavedPlaylistFile>> {
        let conn = self.conn.lock();
        let Some((name, kind)) = conn
            .query_row(
                "SELECT name, kind FROM saved_playlists WHERE id = ?",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT e.kind, COALESCE(t.fingerprint, e.fingerprint), COALESCE(t.artist, e.artist), \
                    COALESCE(t.title, e.title), COALESCE(t.duration, e.duration), \
                    COALESCE(t.content_type, e.content_type) \
             FROM saved_playlist_entries e LEFT JOIN tracks t ON t.id = e.track_id \
             WHERE e.playlist_id = ? ORDER BY e.position, e.id",
        )?;
        let entries = stmt
            .query_map([id], |r| {
                Ok(FileEntry {
                    kind: match r.get::<_, String>(0)?.as_str() {
                        "track" => SavedEntryKind::Track,
                        "stop" => SavedEntryKind::Stop,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    },
                    fingerprint: r.get(1)?,
                    artist: r.get(2)?,
                    title: r.get(3)?,
                    duration: r.get(4)?,
                    content_type: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(SavedPlaylistFile {
            format: FILE_FORMAT.to_owned(),
            version: FILE_VERSION,
            name,
            kind: SavedPlaylistKind::from_db(&kind)?,
            entries,
        }))
    }

    /// Make a new saved playlist from a file. Every entry is kept, in order;
    /// what no track answers to arrives unmatched. Never overwrites: a name in
    /// use takes the next free one.
    pub fn import_saved_playlist(
        &self,
        file: &SavedPlaylistFile,
        now_ms: i64,
    ) -> Result<SavedPlaylistSummary> {
        if file.version != 1 && file.version != FILE_VERSION {
            bail!("unsupported saved playlist file version {}", file.version);
        }
        if file.version == 1
            && (file.kind != SavedPlaylistKind::Playlist
                || file.entries.iter().any(|e| e.kind != SavedEntryKind::Track))
        {
            bail!("version 1 saved playlist files can contain tracks only");
        }
        validate_entries(file.kind, &file.entries)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let name = free_name(&tx, &file.name)?;
        tx.execute(
            "INSERT INTO saved_playlists (name, created_at, updated_at, kind) VALUES (?, ?, ?, ?)",
            params![name, now_ms, now_ms, file.kind.as_str()],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO saved_playlist_entries \
                   (playlist_id, position, kind, fingerprint, artist, title, duration, content_type) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for (position, entry) in file.entries.iter().enumerate() {
                stmt.execute(params![
                    id,
                    position as i64,
                    entry.kind.as_str(),
                    entry.fingerprint,
                    entry.artist,
                    entry.title,
                    entry.duration,
                    entry.content_type,
                ])?;
            }
        }
        bind(&tx)?;
        let missing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM saved_playlist_entries WHERE playlist_id = ? AND kind = 'track' AND track_id IS NULL",
            [id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(SavedPlaylistSummary {
            id,
            name,
            kind: file.kind,
            entries: file.entries.len() as i64,
            stops: file
                .entries
                .iter()
                .filter(|e| e.kind == SavedEntryKind::Stop)
                .count() as i64,
            missing,
        })
    }

    /// The bound tracks of a saved playlist, in order, for an append. A missing
    /// track is among them; an unmatched entry is only counted.
    #[cfg(test)]
    pub fn saved_playlist_tracks(&self, id: i64) -> Result<Option<SavedTracks>> {
        let conn = self.conn.lock();
        let exists: Option<i64> = conn
            .query_row("SELECT id FROM saved_playlists WHERE id = ?", [id], |r| {
                r.get(0)
            })
            .optional()?;
        if exists.is_none() {
            return Ok(None);
        }
        let mut stmt = conn.prepare(
            "SELECT t.id, t.content_type \
             FROM saved_playlist_entries e LEFT JOIN tracks t ON t.id = e.track_id \
             WHERE e.playlist_id = ? AND e.kind = 'track' ORDER BY e.position, e.id",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        let mut tracks = Vec::new();
        let mut unmatched = 0;
        for row in rows {
            match row? {
                (Some(id), Some(content_type)) => tracks.push((id, content_type)),
                _ => unmatched += 1,
            }
        }
        Ok(Some(SavedTracks { tracks, unmatched }))
    }
}

/// Unbind every entry of the missing tracks among `ids`, which are about to be
/// purged. Each entry first takes the track as it reads now for its snapshot,
/// so what stays behind as an unmatched entry is the track as last known.
/// Called inside the purge's own transaction, ahead of the delete.
pub(super) fn unbind_purged(conn: &Connection, ids: &[i64]) -> Result<()> {
    for chunk in ids.chunks(ID_CHUNK) {
        let sql = format!(
            "UPDATE saved_playlist_entries              SET (fingerprint, artist, title, duration, content_type) = (                    SELECT COALESCE(t.fingerprint, saved_playlist_entries.fingerprint),                           t.artist, t.title, t.duration, t.content_type                    FROM tracks t WHERE t.id = saved_playlist_entries.track_id),                  track_id = NULL              WHERE track_id IN (                    SELECT id FROM tracks WHERE missing_since IS NOT NULL AND id IN ({}))",
            placeholders(chunk.len())
        );
        conn.execute(&sql, rusqlite::params_from_iter(chunk))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_tracks(titles: &[&str]) -> (Db, Vec<i64>) {
        let db = Db::open_in_memory().unwrap();
        let ids = {
            let conn = db.conn.lock();
            titles
                .iter()
                .enumerate()
                .map(|(i, title)| {
                    conn.execute(
                        "INSERT INTO tracks (path, content_type, title, artist, album, duration, \
                                             fingerprint) \
                         VALUES (?, 'music', ?, 'a', 'al', 100.0, ?)",
                        params![format!("/{i}.mp3"), title, format!("v2:{i}")],
                    )
                    .unwrap();
                    conn.last_insert_rowid()
                })
                .collect()
        };
        (db, ids)
    }

    fn mark_missing(db: &Db, id: i64) {
        db.conn
            .lock()
            .execute("UPDATE tracks SET missing_since = 5 WHERE id = ?", [id])
            .unwrap();
    }

    fn titles(db: &Db, id: i64) -> Vec<String> {
        db.saved_playlist(id)
            .unwrap()
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.title)
            .collect()
    }

    #[test]
    fn a_created_list_holds_its_tracks_in_order_and_twice_if_asked() {
        let (db, ids) = db_with_tracks(&["A", "B", "C"]);
        let made = db
            .create_saved_playlist("Show", &[ids[2], ids[0], ids[2]], 1)
            .unwrap();
        assert_eq!(made.entries, 3);
        assert_eq!(titles(&db, made.id), ["C", "A", "C"]);
    }

    #[test]
    fn a_track_id_with_no_row_adds_no_entry() {
        let (db, ids) = db_with_tracks(&["A"]);
        let made = db.create_saved_playlist("Show", &[ids[0], 999], 1).unwrap();
        assert_eq!(titles(&db, made.id), ["A"]);
    }

    #[test]
    fn a_taken_name_takes_the_next_free_one_whatever_its_case() {
        let (db, _) = db_with_tracks(&[]);
        assert_eq!(
            db.create_saved_playlist("Show", &[], 1).unwrap().name,
            "Show"
        );
        assert_eq!(
            db.create_saved_playlist(" show ", &[], 1).unwrap().name,
            "show (2)"
        );
        assert_eq!(
            db.create_saved_playlist("SHOW", &[], 1).unwrap().name,
            "SHOW (3)"
        );
        assert!(db.create_saved_playlist("  ", &[], 1).is_err());
    }

    #[test]
    fn renaming_to_a_name_in_use_is_refused_but_recasing_its_own_is_not() {
        let (db, _) = db_with_tracks(&[]);
        let a = db.create_saved_playlist("A", &[], 1).unwrap();
        db.create_saved_playlist("B", &[], 1).unwrap();
        assert!(db.rename_saved_playlist(a.id, "b", 2).is_err());
        db.rename_saved_playlist(a.id, "a", 2).unwrap();
        assert_eq!(db.saved_playlists().unwrap()[0].name, "a");
    }

    #[test]
    fn entries_are_added_at_a_position_and_past_the_end_appends() {
        let (db, ids) = db_with_tracks(&["A", "B", "C", "D"]);
        let p = db.create_saved_playlist("Show", &ids[..2], 1).unwrap().id;
        db.add_saved_entries(p, &[ids[2]], Some(1), 2).unwrap();
        db.add_saved_entries(p, &[ids[3]], Some(99), 2).unwrap();
        db.add_saved_entries(p, &[ids[0]], None, 2).unwrap();
        assert_eq!(titles(&db, p), ["A", "C", "B", "D", "A"]);
    }

    #[test]
    fn an_entry_moves_and_is_removed_by_its_own_id() {
        let (db, ids) = db_with_tracks(&["A", "B", "C"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        db.move_saved_entry(p, 0, 2, 2).unwrap();
        assert_eq!(titles(&db, p), ["B", "C", "A"]);
        let first = db.saved_playlist(p).unwrap().unwrap().entries[0].id;
        db.remove_saved_entry(first, 3).unwrap();
        assert_eq!(titles(&db, p), ["C", "A"]);
    }

    fn entry_ids_of(db: &Db, id: i64) -> Vec<i64> {
        db.saved_playlist(id)
            .unwrap()
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.id)
            .collect()
    }

    #[test]
    fn several_entries_are_removed_at_once_and_only_from_their_own_list() {
        let (db, ids) = db_with_tracks(&["A", "B", "C", "D"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        let other = db.create_saved_playlist("Other", &ids[..1], 1).unwrap().id;
        let e = entry_ids_of(&db, p);
        let foreign = entry_ids_of(&db, other)[0];

        db.remove_saved_entries(p, &[e[3], e[1], foreign, 999], 2)
            .unwrap();

        assert_eq!(titles(&db, p), ["A", "C"]);
        assert_eq!(titles(&db, other), ["A"]);
    }

    #[test]
    fn a_block_moves_in_the_order_given_to_a_gap_of_the_list_as_it_stands() {
        let (db, ids) = db_with_tracks(&["A", "B", "C", "D", "E"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        let e = entry_ids_of(&db, p);

        // D then B, dropped above E.
        db.move_saved_entries(p, &[e[3], e[1]], 4, 2).unwrap();
        assert_eq!(titles(&db, p), ["A", "C", "D", "B", "E"]);

        // To the top, and a gap past the end is the end.
        let e = entry_ids_of(&db, p);
        db.move_saved_entries(p, &[e[4], e[2]], 0, 3).unwrap();
        assert_eq!(titles(&db, p), ["E", "D", "A", "C", "B"]);
        let e = entry_ids_of(&db, p);
        db.move_saved_entries(p, &[e[0]], 99, 4).unwrap();
        assert_eq!(titles(&db, p), ["D", "A", "C", "B", "E"]);
    }

    #[test]
    fn a_block_dropped_inside_itself_closes_up_where_it_was_dropped() {
        let (db, ids) = db_with_tracks(&["A", "B", "C", "D"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        let e = entry_ids_of(&db, p);
        db.move_saved_entries(p, &[e[1], e[2]], 2, 2).unwrap();
        assert_eq!(titles(&db, p), ["A", "B", "C", "D"]);
    }

    #[test]
    fn a_purged_track_leaves_an_unmatched_entry_with_its_snapshot() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        mark_missing(&db, ids[0]);
        assert_eq!(db.saved_playlists().unwrap()[0].missing, 1);

        db.purge_tracks(&[ids[0]]).unwrap();

        let list = db.saved_playlist(p).unwrap().unwrap();
        assert_eq!(list.entries.len(), 2);
        assert!(list.entries[0].track.is_none());
        assert_eq!(list.entries[0].title, "A");
        assert!(list.entries[1].track.is_some());
        assert_eq!(db.saved_playlists().unwrap()[0].missing, 1);
    }

    fn retitle(db: &Db, id: i64, title: &str, fingerprint: &str) {
        db.conn
            .lock()
            .execute(
                "UPDATE tracks SET title = ?, fingerprint = ? WHERE id = ?",
                params![title, fingerprint, id],
            )
            .unwrap();
    }

    #[test]
    fn a_purge_leaves_the_track_as_last_known_not_as_first_added() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        retitle(&db, ids[0], "A (remaster)", "v3:0");
        mark_missing(&db, ids[0]);

        db.purge_tracks(&ids).unwrap();

        let list = db.saved_playlist(p).unwrap().unwrap();
        assert!(list.entries[0].track.is_none());
        assert_eq!(list.entries[0].title, "A (remaster)");
        assert!(list.entries[1].track.is_some(), "a present track is kept");
        let out = db.export_saved_playlist(p).unwrap().unwrap();
        assert_eq!(out.entries[0].fingerprint.as_deref(), Some("v3:0"));
    }

    #[test]
    fn binding_by_hand_rewrites_the_entry_from_the_chosen_track() {
        let (db, ids) = db_with_tracks(&["Studio cut"]);
        let made = db
            .import_saved_playlist(&file("Show", &["v2:elsewhere", "v2:0"]), 1)
            .unwrap();
        let e = entry_ids_of(&db, made.id);
        assert_eq!(db.saved_playlists().unwrap()[0].missing, 1);

        db.bind_saved_entry(e[0], ids[0], 2).unwrap();

        let list = db.saved_playlist(made.id).unwrap().unwrap();
        assert_eq!(entry_ids_of(&db, made.id), e, "the entry keeps its place");
        assert_eq!(list.entries[0].track.as_ref().map(|t| t.id), Some(ids[0]));
        assert_eq!(list.entries[0].title, "Studio cut");
        assert_eq!(db.saved_playlists().unwrap()[0].missing, 0);

        // Purged, the entry is the chosen track's, not the file's stranger.
        mark_missing(&db, ids[0]);
        db.purge_tracks(&ids).unwrap();
        let out = db.export_saved_playlist(made.id).unwrap().unwrap();
        assert_eq!(out.entries[0].fingerprint.as_deref(), Some("v2:0"));
        assert_eq!(out.entries[0].title.as_deref(), Some("Studio cut"));
    }

    #[test]
    fn binding_by_hand_refuses_a_missing_track_and_an_unknown_entry() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids[..1], 1).unwrap().id;
        let e = entry_ids_of(&db, p);
        mark_missing(&db, ids[1]);

        assert!(db.bind_saved_entry(e[0], ids[1], 2).is_err());
        assert!(db.bind_saved_entry(e[0], 999, 2).is_err());
        assert!(db.bind_saved_entry(999, ids[0], 2).is_err());
        let list = db.saved_playlist(p).unwrap().unwrap();
        assert_eq!(list.entries[0].track.as_ref().map(|t| t.id), Some(ids[0]));
    }

    /// A hidden track is as unable to air as a missing one: counted with
    /// them, out of the auto-playlist pool, and not something to bind to.
    #[test]
    fn a_hidden_track_counts_as_missing_and_leaves_the_pool() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids[..1], 1).unwrap().id;
        let e = entry_ids_of(&db, p);
        assert_eq!(db.saved_playlist_pool(p).unwrap().unwrap().1, 1);

        db.hide_tracks(&ids, 2).unwrap();

        assert_eq!(db.saved_playlists().unwrap()[0].missing, 1);
        assert_eq!(db.saved_playlist_pool(p).unwrap().unwrap().1, 0);
        assert!(db.bind_saved_entry(e[0], ids[1], 3).is_err());

        db.unhide_tracks(&ids).unwrap();

        assert_eq!(db.saved_playlists().unwrap()[0].missing, 0);
        assert_eq!(db.saved_playlist_pool(p).unwrap().unwrap().1, 1);
    }

    #[test]
    fn an_append_takes_a_missing_track_and_counts_an_unmatched_entry() {
        let (db, ids) = db_with_tracks(&["A", "B", "C"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        mark_missing(&db, ids[1]);
        mark_missing(&db, ids[2]);
        db.purge_tracks(&[ids[2]]).unwrap();

        let offered = db.saved_playlist_tracks(p).unwrap().unwrap();
        assert_eq!(
            offered.tracks,
            [(ids[0], "music".to_owned()), (ids[1], "music".to_owned())]
        );
        assert_eq!(offered.unmatched, 1);
        assert_eq!(db.saved_playlist_tracks(999).unwrap(), None);
    }

    fn file(name: &str, fingerprints: &[&str]) -> SavedPlaylistFile {
        SavedPlaylistFile {
            format: FILE_FORMAT.into(),
            version: FILE_VERSION,
            name: name.into(),
            kind: SavedPlaylistKind::Playlist,
            entries: fingerprints
                .iter()
                .map(|fp| FileEntry {
                    kind: SavedEntryKind::Track,
                    fingerprint: Some((*fp).into()),
                    artist: Some("x".into()),
                    title: Some(format!("from {fp}")),
                    duration: Some(100.0),
                    content_type: Some("music".into()),
                })
                .collect(),
        }
    }

    fn bound(db: &Db, id: i64) -> Vec<Option<i64>> {
        db.saved_playlist(id)
            .unwrap()
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.track.map(|t| t.id))
            .collect()
    }

    #[test]
    fn an_import_keeps_every_entry_and_binds_the_ones_it_knows() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let made = db
            .import_saved_playlist(&file("Show", &["v2:1", "v2:nope", "v2:0"]), 1)
            .unwrap();
        assert_eq!((made.entries, made.missing), (3, 1));
        assert_eq!(bound(&db, made.id), [Some(ids[1]), None, Some(ids[0])]);
        assert_eq!(titles(&db, made.id)[1], "from v2:nope");
    }

    #[test]
    fn an_import_never_overwrites_a_saved_playlist_of_its_name() {
        let (db, ids) = db_with_tracks(&["A"]);
        let first = db.create_saved_playlist("Show", &ids, 1).unwrap();
        let second = db.import_saved_playlist(&file("Show", &[]), 2).unwrap();
        assert_eq!(second.name, "Show (2)");
        assert_eq!(titles(&db, first.id), ["A"]);
    }

    #[test]
    fn an_unmatched_entry_binds_once_its_file_is_in_the_library() {
        let (db, _) = db_with_tracks(&[]);
        let made = db
            .import_saved_playlist(&file("Show", &["v2:new"]), 1)
            .unwrap();
        assert_eq!(db.bind_saved_entries().unwrap(), 0);

        db.conn
            .lock()
            .execute(
                "INSERT INTO tracks (path, title, duration, fingerprint) \
                 VALUES ('/new.mp3', 'New', 100.0, 'v2:new')",
                [],
            )
            .unwrap();

        assert_eq!(db.bind_saved_entries().unwrap(), 1);
        assert!(bound(&db, made.id)[0].is_some());
        assert_eq!(db.bind_saved_entries().unwrap(), 0, "binding is idempotent");
    }

    #[test]
    fn a_missing_track_is_not_what_an_entry_binds_to() {
        let (db, ids) = db_with_tracks(&["A"]);
        mark_missing(&db, ids[0]);
        let made = db
            .import_saved_playlist(&file("Show", &["v2:0"]), 1)
            .unwrap();
        assert_eq!(bound(&db, made.id), [None]);
    }

    #[test]
    fn duration_tells_two_tracks_of_one_fingerprint_apart() {
        let (db, _) = db_with_tracks(&[]);
        {
            let conn = db.conn.lock();
            for (path, duration) in [("/edit.mp3", 180.0), ("/album.mp3", 300.0)] {
                conn.execute(
                    "INSERT INTO tracks (path, title, duration, fingerprint) \
                     VALUES (?, 'T', ?, 'v2:same')",
                    params![path, duration],
                )
                .unwrap();
            }
        }
        let mut wanted = file("Show", &["v2:same"]);
        wanted.entries[0].duration = Some(299.0);
        let made = db.import_saved_playlist(&wanted, 1).unwrap();
        assert_eq!(bound(&db, made.id), [Some(2)]);
    }

    #[test]
    fn an_export_writes_the_bound_tracks_fingerprint_as_it_reads_now() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        db.conn
            .lock()
            .execute(
                "UPDATE tracks SET fingerprint = 'v3:0' WHERE id = ?",
                [ids[0]],
            )
            .unwrap();
        mark_missing(&db, ids[1]);
        db.purge_tracks(&[ids[1]]).unwrap();

        let out = db.export_saved_playlist(p).unwrap().unwrap();
        assert_eq!(out.name, "Show");
        let fingerprints: Vec<_> = out.entries.iter().map(|e| e.fingerprint.clone()).collect();
        assert_eq!(fingerprints, [Some("v3:0".into()), Some("v2:1".into())]);
        assert_eq!(out.entries[1].title.as_deref(), Some("B"));
        assert_eq!(db.export_saved_playlist(999).unwrap(), None);
    }

    #[test]
    fn a_file_round_trips_through_json() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        let json = serde_json::to_string(&db.export_saved_playlist(p).unwrap().unwrap()).unwrap();
        assert!(json.contains("\"contentType\":\"music\""));
        let back = db
            .import_saved_playlist(&SavedPlaylistFile::parse(&json).unwrap(), 2)
            .unwrap();
        assert_eq!(back.name, "Show (2)");
        assert_eq!(bound(&db, back.id), [Some(ids[0]), Some(ids[1])]);
    }

    #[test]
    fn a_prepared_show_keeps_stops_in_order_through_storage_and_file_transfer() {
        let (db, ids) = db_with_tracks(&["A", "B"]);
        let made = db
            .create_saved_playlist_with_entries(
                "Show",
                SavedPlaylistKind::PreparedShow,
                &[
                    SavedEntryInput::Track(ids[0]),
                    SavedEntryInput::Stop,
                    SavedEntryInput::Track(ids[1]),
                    SavedEntryInput::Stop,
                ],
                1,
            )
            .unwrap();
        assert_eq!((made.entries, made.stops, made.missing), (4, 2, 0));
        let detail = db.saved_playlist(made.id).unwrap().unwrap();
        assert_eq!(
            detail
                .entries
                .iter()
                .map(|e| (e.kind, e.track.as_ref().map(|track| track.id)))
                .collect::<Vec<_>>(),
            [
                (SavedEntryKind::Track, Some(ids[0])),
                (SavedEntryKind::Stop, None),
                (SavedEntryKind::Track, Some(ids[1])),
                (SavedEntryKind::Stop, None),
            ]
        );
        let file = db.export_saved_playlist(made.id).unwrap().unwrap();
        let json = serde_json::to_string(&file).unwrap();
        let imported = db
            .import_saved_playlist(&SavedPlaylistFile::parse(&json).unwrap(), 2)
            .unwrap();
        assert_eq!(
            (
                imported.kind,
                imported.entries,
                imported.stops,
                imported.missing
            ),
            (SavedPlaylistKind::PreparedShow, 4, 2, 0)
        );
        assert_eq!(
            db.saved_playlist(imported.id)
                .unwrap()
                .unwrap()
                .entries
                .iter()
                .map(|e| (e.kind, e.track.as_ref().map(|track| track.id)))
                .collect::<Vec<_>>(),
            detail
                .entries
                .iter()
                .map(|e| (e.kind, e.track.as_ref().map(|track| track.id)))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_ordinary_playlist_rejects_stops_and_a_downgrade_is_atomic() {
        let (db, ids) = db_with_tracks(&["A"]);
        assert!(db
            .create_saved_playlist_with_entries(
                "Bad",
                SavedPlaylistKind::Playlist,
                &[SavedEntryInput::Track(ids[0]), SavedEntryInput::Stop],
                1
            )
            .is_err());
        assert!(db.saved_playlists().unwrap().is_empty());
        let made = db
            .create_saved_playlist_with_entries(
                "Show",
                SavedPlaylistKind::PreparedShow,
                &[SavedEntryInput::Track(ids[0]), SavedEntryInput::Stop],
                1,
            )
            .unwrap();
        assert!(db
            .set_saved_playlist_kind(made.id, SavedPlaylistKind::Playlist, 2)
            .unwrap_err()
            .to_string()
            .contains("remove Stops"));
        assert_eq!(
            db.saved_playlist(made.id).unwrap().unwrap().kind,
            SavedPlaylistKind::PreparedShow
        );
        let ordinary = db.create_saved_playlist("Ordinary", &ids, 1).unwrap();
        assert!(db.add_saved_stop(ordinary.id, None, 2).is_err());
        assert_eq!(
            db.saved_playlist(ordinary.id)
                .unwrap()
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn legacy_track_only_files_import_as_playlists_and_bad_v2_files_are_refused() {
        let legacy = r#"{"format":"radiodiodj-playlist","version":1,"name":"Old","entries":[{"title":"unmatched"}]}"#;
        let file = SavedPlaylistFile::parse(legacy).unwrap();
        assert_eq!(file.kind, SavedPlaylistKind::Playlist);
        assert_eq!(file.entries[0].kind, SavedEntryKind::Track);
        let (db, _) = db_with_tracks(&[]);
        let imported = db.import_saved_playlist(&file, 1).unwrap();
        assert_eq!(
            (imported.entries, imported.stops, imported.missing),
            (1, 0, 1)
        );
        let invalid = r#"{"format":"radiodiodj-playlist","version":2,"name":"Bad","kind":"playlist","entries":[{"kind":"stop"}]}"#;
        assert!(SavedPlaylistFile::parse(invalid).is_err());
        assert_eq!(db.saved_playlists().unwrap().len(), 1);
    }

    #[test]
    fn an_unmatched_track_next_to_a_stop_does_not_consume_the_stop() {
        let (db, ids) = db_with_tracks(&["B"]);
        let file = SavedPlaylistFile {
            format: FILE_FORMAT.into(),
            version: FILE_VERSION,
            name: "Show".into(),
            kind: SavedPlaylistKind::PreparedShow,
            entries: vec![
                FileEntry {
                    kind: SavedEntryKind::Track,
                    fingerprint: Some("v2:absent".into()),
                    artist: Some("A".into()),
                    title: Some("A".into()),
                    duration: Some(100.0),
                    content_type: Some("music".into()),
                },
                FileEntry {
                    kind: SavedEntryKind::Stop,
                    fingerprint: None,
                    artist: None,
                    title: None,
                    duration: None,
                    content_type: None,
                },
                FileEntry {
                    kind: SavedEntryKind::Track,
                    fingerprint: Some("v2:0".into()),
                    artist: Some("B".into()),
                    title: Some("B".into()),
                    duration: Some(100.0),
                    content_type: Some("music".into()),
                },
            ],
        };
        let made = db.import_saved_playlist(&file, 1).unwrap();
        assert_eq!((made.entries, made.stops, made.missing), (3, 1, 1));
        let offered = db.saved_playlist(made.id).unwrap().unwrap();
        assert_eq!(
            offered
                .entries
                .iter()
                .filter(|e| e.kind == SavedEntryKind::Track && e.track.is_none())
                .count(),
            1
        );
        assert_eq!(
            offered
                .entries
                .iter()
                .map(|e| (e.kind, e.track.as_ref().map(|track| track.id)))
                .collect::<Vec<_>>(),
            [
                (SavedEntryKind::Track, None),
                (SavedEntryKind::Stop, None),
                (SavedEntryKind::Track, Some(ids[0])),
            ]
        );
    }

    #[test]
    fn a_file_this_build_does_not_know_is_refused_whole() {
        let newer = r#"{"format":"radiodiodj-playlist","version":3,"name":"X","entries":[]}"#;
        assert!(SavedPlaylistFile::parse(newer)
            .unwrap_err()
            .to_string()
            .contains("version 3"));
        let other = r#"{"format":"m3u","version":1,"name":"X","entries":[]}"#;
        assert!(SavedPlaylistFile::parse(other).is_err());
        assert!(SavedPlaylistFile::parse("#EXTM3U").is_err());
        assert!(SavedPlaylistFile::parse(r#"{"version":1}"#).is_err());
    }

    #[test]
    fn a_pool_counts_each_playable_music_track_once() {
        let (db, ids) = db_with_tracks(&["A", "B", "C", "J"]);
        db.conn
            .lock()
            .execute(
                "UPDATE tracks SET content_type = 'jingle' WHERE id = ?",
                [ids[3]],
            )
            .unwrap();
        mark_missing(&db, ids[2]);
        let listed = [ids[0], ids[0], ids[1], ids[2], ids[3]];
        let p = db.create_saved_playlist("Show", &listed, 1).unwrap().id;
        assert_eq!(db.saved_playlist_pool(p).unwrap(), Some(("Show".into(), 2)));
        assert_eq!(db.saved_playlist_pool(999).unwrap(), None);
    }

    #[test]
    fn saved_detail_uses_current_content_type_for_a_reclassified_bound_track() {
        let (db, ids) = db_with_tracks(&["A"]);
        let list = db.create_saved_playlist("Show", &ids, 1).unwrap();
        db.conn
            .lock()
            .execute(
                "UPDATE tracks SET content_type = 'jingle' WHERE id = ?",
                [ids[0]],
            )
            .unwrap();

        let detail = db.saved_playlist(list.id).unwrap().unwrap();
        assert_eq!(detail.entries[0].content_type.as_deref(), Some("jingle"));
    }

    #[test]
    fn deleting_a_list_takes_its_entries_with_it() {
        let (db, ids) = db_with_tracks(&["A"]);
        let p = db.create_saved_playlist("Show", &ids, 1).unwrap().id;
        db.delete_saved_playlist(p).unwrap();
        assert!(db.saved_playlists().unwrap().is_empty());
        let left: i64 = db
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM saved_playlist_entries", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0);
    }
}
