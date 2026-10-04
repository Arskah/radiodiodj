//! Saved playlists: named, stored, ordered lists of entries. See
//! `docs/saved-playlists.md`.

use anyhow::{anyhow, bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{placeholders, Db, Track, ID_CHUNK};

/// One row of the saved playlists list.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaylistSummary {
    pub id: i64,
    pub name: String,
    pub entries: i64,
    /// Entries that cannot air: unmatched, or bound to a missing track.
    pub missing: i64,
}

/// One entry of a saved playlist. `track` is the live track it is bound to;
/// the rest is what the entry was written with, shown when nothing is bound.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedEntry {
    pub id: i64,
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
    pub entries: Vec<SavedEntry>,
}

/// What a saved playlist offers the on-air playlist: its bound tracks in
/// order, each with its content type, and how many entries had none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTracks {
    pub tracks: Vec<(i64, String)>,
    pub unmatched: usize,
}

/// The `format` every saved playlist file carries.
pub const FILE_FORMAT: &str = "radiodiodj-playlist";
/// The only file version this build reads and writes.
pub const FILE_VERSION: u32 = 1;

/// A saved playlist as it travels between installs: no track ids and no paths,
/// only what identifies a recording anywhere. See `docs/saved-playlists.md`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlaylistFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
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
            Some(FILE_VERSION) => {}
            Some(v) => bail!(
                "this saved playlist file is version {v}, and this build reads version \
                 {FILE_VERSION}"
            ),
            None => bail!("not a saved playlist file"),
        }
        serde_json::from_str(json).map_err(|e| anyhow!("not a saved playlist file: {e}"))
    }
}

/// Bind every unmatched entry to the present track with its fingerprint. Two
/// such tracks are told apart by the entry's duration, then by id.
fn bind(conn: &Connection) -> Result<usize> {
    const MATCH: &str = "FROM tracks t \
         WHERE t.fingerprint = saved_playlist_entries.fingerprint AND t.missing_since IS NULL";
    let sql = format!(
        "UPDATE saved_playlist_entries SET track_id = ( \
           SELECT t.id {MATCH} \
           ORDER BY ABS(COALESCE(t.duration, 0) - COALESCE(saved_playlist_entries.duration, 0)), \
                    t.id \
           LIMIT 1) \
         WHERE track_id IS NULL AND fingerprint IS NOT NULL AND EXISTS (SELECT 1 {MATCH})"
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
            "SELECT p.id, p.name, COUNT(e.id), \
                    COALESCE(SUM(e.id IS NOT NULL \
                                 AND (t.id IS NULL OR t.missing_since IS NOT NULL)), 0) \
             FROM saved_playlists p \
             LEFT JOIN saved_playlist_entries e ON e.playlist_id = p.id \
             LEFT JOIN tracks t ON t.id = e.track_id \
             GROUP BY p.id ORDER BY p.name COLLATE NOCASE, p.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SavedPlaylistSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                entries: r.get(2)?,
                missing: r.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn saved_playlist(&self, id: i64) -> Result<Option<SavedPlaylist>> {
        type Row = (i64, Option<i64>, String, String, f64, Option<String>);
        let (name, rows): (String, Vec<Row>) = {
            let conn = self.conn.lock();
            let Some(name) = conn
                .query_row("SELECT name FROM saved_playlists WHERE id = ?", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            else {
                return Ok(None);
            };
            let mut stmt = conn.prepare(
                "SELECT id, track_id, COALESCE(artist, ''), COALESCE(title, ''), \
                        COALESCE(duration, 0), content_type \
                 FROM saved_playlist_entries WHERE playlist_id = ? ORDER BY position, id",
            )?;
            let rows = stmt
                .query_map([id], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            (name, rows)
        };
        let mut bound: Vec<i64> = rows.iter().filter_map(|r| r.1).collect();
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
                |(id, track_id, artist, title, duration, content_type)| SavedEntry {
                    id,
                    track: track_id.and_then(|t| tracks.get(&t).cloned()),
                    artist,
                    title,
                    duration,
                    content_type,
                },
            )
            .collect();
        Ok(Some(SavedPlaylist { id, name, entries }))
    }

    /// Create a saved playlist holding `track_ids` in order. A name in use
    /// takes the next free one, so making a list never fails on its name.
    pub fn create_saved_playlist(
        &self,
        name: &str,
        track_ids: &[i64],
        now_ms: i64,
    ) -> Result<SavedPlaylistSummary> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let name = free_name(&tx, name)?;
        tx.execute(
            "INSERT INTO saved_playlists (name, created_at, updated_at) VALUES (?, ?, ?)",
            params![name, now_ms, now_ms],
        )?;
        let id = tx.last_insert_rowid();
        let added = insert_entries(&tx, id, track_ids)?;
        write_order(&tx, &added)?;
        tx.commit()?;
        Ok(SavedPlaylistSummary {
            id,
            name,
            entries: added.len() as i64,
            missing: 0,
        })
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
        let block: Vec<i64> = moved
            .iter()
            .copied()
            .filter(|id| order.contains(id))
            .collect();
        // Where the gap falls once the block has been lifted out.
        let at = order
            .iter()
            .take(gap.min(order.len()))
            .filter(|id| !block.contains(id))
            .count();
        let mut order: Vec<i64> = order.into_iter().filter(|id| !block.contains(id)).collect();
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
               WHERE e.playlist_id = p.id AND t.missing_since IS NULL \
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
        let Some(name) = conn
            .query_row("SELECT name FROM saved_playlists WHERE id = ?", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT COALESCE(t.fingerprint, e.fingerprint), COALESCE(t.artist, e.artist), \
                    COALESCE(t.title, e.title), COALESCE(t.duration, e.duration), \
                    COALESCE(t.content_type, e.content_type) \
             FROM saved_playlist_entries e LEFT JOIN tracks t ON t.id = e.track_id \
             WHERE e.playlist_id = ? ORDER BY e.position, e.id",
        )?;
        let entries = stmt
            .query_map([id], |r| {
                Ok(FileEntry {
                    fingerprint: r.get(0)?,
                    artist: r.get(1)?,
                    title: r.get(2)?,
                    duration: r.get(3)?,
                    content_type: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(SavedPlaylistFile {
            format: FILE_FORMAT.to_owned(),
            version: FILE_VERSION,
            name,
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
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let name = free_name(&tx, &file.name)?;
        tx.execute(
            "INSERT INTO saved_playlists (name, created_at, updated_at) VALUES (?, ?, ?)",
            params![name, now_ms, now_ms],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO saved_playlist_entries \
                   (playlist_id, position, fingerprint, artist, title, duration, content_type) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for (position, entry) in file.entries.iter().enumerate() {
                stmt.execute(params![
                    id,
                    position as i64,
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
            "SELECT COUNT(*) FROM saved_playlist_entries WHERE playlist_id = ? AND track_id IS NULL",
            [id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(SavedPlaylistSummary {
            id,
            name,
            entries: file.entries.len() as i64,
            missing,
        })
    }

    /// The bound tracks of a saved playlist, in order, for an append. A missing
    /// track is among them; an unmatched entry is only counted.
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
             WHERE e.playlist_id = ? ORDER BY e.position, e.id",
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

/// Unbind every entry of the purged tracks. Called inside the purge's own
/// transaction; the entries stay, as unmatched ones.
pub(super) fn unbind_purged(conn: &Connection, ids: &[i64]) -> Result<()> {
    for chunk in ids.chunks(ID_CHUNK) {
        let sql = format!(
            "UPDATE saved_playlist_entries SET track_id = NULL WHERE track_id IN ({})",
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
            entries: fingerprints
                .iter()
                .map(|fp| FileEntry {
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
    fn a_file_this_build_does_not_know_is_refused_whole() {
        let newer = r#"{"format":"radiodiodj-playlist","version":2,"name":"X","entries":[]}"#;
        assert!(SavedPlaylistFile::parse(newer)
            .unwrap_err()
            .to_string()
            .contains("version 2"));
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
