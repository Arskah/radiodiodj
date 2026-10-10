//! Change capture for a shared library: which groups this machine has changed
//! and not sent yet. See `docs/shared-library.md#change-capture`.

use anyhow::Result;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use std::path::Path;

use super::{placeholders, Db, EditedFields, Opened, ID_CHUNK};

/// Step 17: the change log, and the triggers that write it.
///
/// Nothing reads `sync_rows` yet and nothing turns `sync_local.capture` on, so
/// on every library this step is inert. `saved_playlists.uid` is the exception:
/// a list gets one whether or not capture is on, because it has to have had one
/// all along by the time the library is shared.
///
/// A trigger's `UPDATE OF` list is what says a `tracks` column travels. A
/// column added later belongs in one, in a step that drops and recreates the
/// trigger; `every_tracks_column_is_claimed` fails until it is.
pub(super) const CHANGE_CAPTURE: &str = r#"
CREATE TABLE sync_rows (
  kind      TEXT NOT NULL,
  key       TEXT NOT NULL,
  edited_at INTEGER NOT NULL,
  machine   TEXT,
  deleted   INTEGER NOT NULL DEFAULT 0,
  pending   INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;

CREATE TABLE sync_local (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  capture    INTEGER NOT NULL DEFAULT 0,
  applying   INTEGER NOT NULL DEFAULT 0,
  library_id TEXT,
  pulled_rev INTEGER NOT NULL DEFAULT 0
);
INSERT INTO sync_local (id) VALUES (1);

ALTER TABLE saved_playlists ADD COLUMN uid TEXT;
UPDATE saved_playlists SET uid = lower(hex(randomblob(16)));
CREATE UNIQUE INDEX saved_playlists_uid ON saved_playlists(uid);

CREATE TRIGGER saved_playlists_ai AFTER INSERT ON saved_playlists
WHEN new.uid IS NULL BEGIN
  UPDATE saved_playlists SET uid = lower(hex(randomblob(16))) WHERE id = new.id;
END;

CREATE TRIGGER sync_track_ai AFTER INSERT ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_owned AFTER UPDATE OF
  root_id, path, content_type, duration, bpm, sample_rate, bitrate, format,
  mtime, added_at, waveform, fingerprint, missing_since, analysis_error,
  analysis_failed_at, rg_gain, rg_peak, rg_measured_at, auto_cue_version,
  auto_cue_silence_db, auto_cue_segue_db, auto_cue_at, auto_cue_levels, isrc,
  tags_read_version, detected_bpm, bpm_confidence, bpm_measured_at, bpm_version,
  detected_key, key_confidence, key_measured_at, key_version,
  duration_measured_at, tag_duration
ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_tags AFTER UPDATE OF
  title, artist, album, genre, year, album_artist, track_no, track_total,
  disc_no, disc_total, initial_key, comment
ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) AND (
     (new.title IS NOT old.title AND NOT new.edited_fields & 1)
  OR (new.artist IS NOT old.artist AND NOT new.edited_fields & 2)
  OR (new.album IS NOT old.album AND NOT new.edited_fields & 4)
  OR (new.genre IS NOT old.genre AND NOT new.edited_fields & 8)
  OR (new.year IS NOT old.year AND NOT new.edited_fields & 16)
  OR (new.album_artist IS NOT old.album_artist AND NOT new.edited_fields & 32)
  OR (new.track_no IS NOT old.track_no AND NOT new.edited_fields & 64)
  OR (new.track_total IS NOT old.track_total AND NOT new.edited_fields & 128)
  OR (new.disc_no IS NOT old.disc_no AND NOT new.edited_fields & 256)
  OR (new.disc_total IS NOT old.disc_total AND NOT new.edited_fields & 512)
  OR (new.initial_key IS NOT old.initial_key AND NOT new.edited_fields & 1024)
  OR (new.comment IS NOT old.comment AND NOT new.edited_fields & 2048)
) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_trio AFTER UPDATE OF
  cue_in_ms, cue_out_ms, next_start_ms, auto_cue_state
ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local)
  AND new.auto_cue_state <> 'manual' AND (
     new.cue_in_ms IS NOT old.cue_in_ms
  OR new.cue_out_ms IS NOT old.cue_out_ms
  OR new.next_start_ms IS NOT old.next_start_ms
  OR new.auto_cue_state <> old.auto_cue_state
) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_cue AFTER UPDATE OF
  cue_in_ms, fade_in_ms, fade_out_ms, cue_out_ms, next_start_ms, auto_cue_state
ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) AND (
     new.fade_in_ms IS NOT old.fade_in_ms
  OR new.fade_out_ms IS NOT old.fade_out_ms
  OR (new.auto_cue_state = 'manual' AND (
          old.auto_cue_state <> 'manual'
       OR new.cue_in_ms IS NOT old.cue_in_ms
       OR new.cue_out_ms IS NOT old.cue_out_ms
       OR new.next_start_ms IS NOT old.next_start_ms))
) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('cue', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_edit AFTER UPDATE OF
  title, artist, album, genre, year, album_artist, track_no, track_total,
  disc_no, disc_total, initial_key, comment, edited_fields
ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':title', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 1
  WHERE (new.edited_fields & 1 AND (new.title IS NOT old.title OR NOT old.edited_fields & 1))
     OR (old.edited_fields & 1 AND NOT new.edited_fields & 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':artist', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 2
  WHERE (new.edited_fields & 2 AND (new.artist IS NOT old.artist OR NOT old.edited_fields & 2))
     OR (old.edited_fields & 2 AND NOT new.edited_fields & 2)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':album', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 4
  WHERE (new.edited_fields & 4 AND (new.album IS NOT old.album OR NOT old.edited_fields & 4))
     OR (old.edited_fields & 4 AND NOT new.edited_fields & 4)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':genre', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 8
  WHERE (new.edited_fields & 8 AND (new.genre IS NOT old.genre OR NOT old.edited_fields & 8))
     OR (old.edited_fields & 8 AND NOT new.edited_fields & 8)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':year', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 16
  WHERE (new.edited_fields & 16 AND (new.year IS NOT old.year OR NOT old.edited_fields & 16))
     OR (old.edited_fields & 16 AND NOT new.edited_fields & 16)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':album_artist', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 32
  WHERE (new.edited_fields & 32 AND (new.album_artist IS NOT old.album_artist OR NOT old.edited_fields & 32))
     OR (old.edited_fields & 32 AND NOT new.edited_fields & 32)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':track_no', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 64
  WHERE (new.edited_fields & 64 AND (new.track_no IS NOT old.track_no OR NOT old.edited_fields & 64))
     OR (old.edited_fields & 64 AND NOT new.edited_fields & 64)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':track_total', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 128
  WHERE (new.edited_fields & 128 AND (new.track_total IS NOT old.track_total OR NOT old.edited_fields & 128))
     OR (old.edited_fields & 128 AND NOT new.edited_fields & 128)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':disc_no', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 256
  WHERE (new.edited_fields & 256 AND (new.disc_no IS NOT old.disc_no OR NOT old.edited_fields & 256))
     OR (old.edited_fields & 256 AND NOT new.edited_fields & 256)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':disc_total', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 512
  WHERE (new.edited_fields & 512 AND (new.disc_total IS NOT old.disc_total OR NOT old.edited_fields & 512))
     OR (old.edited_fields & 512 AND NOT new.edited_fields & 512)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':initial_key', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 1024
  WHERE (new.edited_fields & 1024 AND (new.initial_key IS NOT old.initial_key OR NOT old.edited_fields & 1024))
     OR (old.edited_fields & 1024 AND NOT new.edited_fields & 1024)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  SELECT 'edit', new.id || ':comment', CAST(unixepoch('subsec') * 1000 AS INTEGER), NOT new.edited_fields & 2048
  WHERE (new.edited_fields & 2048 AND (new.comment IS NOT old.comment OR NOT old.edited_fields & 2048))
     OR (old.edited_fields & 2048 AND NOT new.edited_fields & 2048)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_hidden AFTER UPDATE OF hidden_at ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local)
  AND new.hidden_at IS NOT old.hidden_at BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('hidden', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_forget AFTER DELETE ON tracks BEGIN
  DELETE FROM sync_rows WHERE kind IN ('cue', 'hidden') AND key = CAST(old.id AS TEXT);
  DELETE FROM sync_rows WHERE kind = 'edit' AND key LIKE old.id || ':%';
END;

CREATE TRIGGER sync_track_ad AFTER DELETE ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', old.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_root_ai AFTER INSERT ON library_roots
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('root', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_root_au AFTER UPDATE ON library_roots
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('root', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_root_ad AFTER DELETE ON library_roots
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('root', old.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_playlist_au AFTER UPDATE ON saved_playlists
WHEN (SELECT capture AND NOT applying FROM sync_local) AND new.uid IS NOT NULL BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('playlist', new.uid, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_playlist_ad AFTER DELETE ON saved_playlists
WHEN (SELECT capture AND NOT applying FROM sync_local) AND old.uid IS NOT NULL BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('playlist', old.uid, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_dismissal_ai AFTER INSERT ON health_dismissals
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('dismissal', new.kind || ':' || new.key, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_dismissal_au AFTER UPDATE ON health_dismissals
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('dismissal', new.kind || ':' || new.key, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_dismissal_ad AFTER DELETE ON health_dismissals
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('dismissal', old.kind || ':' || old.key, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;
"#;

/// Step 18: whether this database is a studio's copy of an owner's library —
/// what says its track ids are the owner's and it may be filled from the hub.
pub(super) const REPLICA: &str =
    "ALTER TABLE sync_local ADD COLUMN replica INTEGER NOT NULL DEFAULT 0;";

/// One group as the hub holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    pub kind: String,
    pub key: String,
    pub rev: i64,
    pub deleted: bool,
    pub doc: Option<serde_json::Value>,
    pub waveform: Option<Vec<u8>>,
    pub levels: Option<Vec<u8>>,
}

/// One group on its way to the hub: what it is, when this machine last
/// changed it, and the row as a document. A tombstone carries no document.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    pub kind: String,
    pub key: String,
    pub edited_at: i64,
    pub deleted: bool,
    pub doc: Option<serde_json::Value>,
    pub waveform: Option<Vec<u8>>,
    pub levels: Option<Vec<u8>>,
}

/// `tracks` columns a `track` document leaves out: the two BLOBs, which travel
/// beside it, what belongs to another group, and what never leaves the machine.
const NOT_IN_A_TRACK_DOCUMENT: &[&str] = &[
    "id",
    "play_count",
    "waveform",
    "auto_cue_levels",
    "fade_in_ms",
    "fade_out_ms",
    "edited_fields",
    "hidden_at",
];

/// The tag columns an operator can edit, each with its `edited_fields` bit.
const EDITABLE: [(&str, i64); 12] = [
    ("title", EditedFields::TITLE),
    ("artist", EditedFields::ARTIST),
    ("album", EditedFields::ALBUM),
    ("genre", EditedFields::GENRE),
    ("year", EditedFields::YEAR),
    ("album_artist", EditedFields::ALBUM_ARTIST),
    ("track_no", EditedFields::TRACK_NO),
    ("track_total", EditedFields::TRACK_TOTAL),
    ("disc_no", EditedFields::DISC_NO),
    ("disc_total", EditedFields::DISC_TOTAL),
    ("initial_key", EditedFields::INITIAL_KEY),
    ("comment", EditedFields::COMMENT),
];

impl Db {
    /// Start capturing changes, and owe the hub everything the library already
    /// holds. Does nothing to a library that is capturing already.
    pub fn start_capture(&self, now_ms: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let was_on: bool = tx.query_row("SELECT capture FROM sync_local", [], |r| r.get(0))?;
        if !was_on {
            tx.execute("UPDATE sync_local SET capture = 1", [])?;
            owe_everything(&tx, now_ms)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stop capturing. What was captured stays, for a library that is shared
    /// again later.
    pub fn stop_capture(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE sync_local SET capture = 0 WHERE capture", [])?;
        Ok(())
    }

    /// The hub library this database was last published as.
    pub fn library_id(&self) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn.query_row("SELECT library_id FROM sync_local", [], |r| r.get(0))?)
    }

    /// Take the id the hub now holds this library under, and owe it everything
    /// again: a hub given a new id was emptied first.
    pub fn publish_as(&self, library_id: &str, now_ms: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("UPDATE sync_local SET library_id = ?", [library_id])?;
        owe_everything(&tx, now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Up to `limit` groups of `kinds` that have not been sent, each with its
    /// document as the row reads now.
    pub fn outgoing(&self, kinds: &[&str], limit: usize) -> Result<Vec<Outgoing>> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT kind, key, edited_at, deleted FROM sync_rows \
             WHERE pending AND kind IN ({}) ORDER BY kind, key LIMIT ?",
            placeholders(kinds.len())
        );
        let waiting = {
            let mut stmt = conn.prepare(&sql)?;
            let params = kinds
                .iter()
                .map(|k| Value::Text((*k).to_owned()))
                .chain(std::iter::once(Value::Integer(limit as i64)));
            let rows = stmt.query_map(params_from_iter(params), |r| {
                Ok(Outgoing {
                    kind: r.get(0)?,
                    key: r.get(1)?,
                    edited_at: r.get(2)?,
                    deleted: r.get(3)?,
                    doc: None,
                    waveform: None,
                    levels: None,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        waiting
            .into_iter()
            .map(|group| match (group.deleted, group.kind.as_str()) {
                (true, _) => Ok(group),
                (false, "root") => root_document(&conn, group),
                (false, "track") => track_document(&conn, group),
                (false, kind) => anyhow::bail!("no document is defined for a {kind} group"),
            })
            .collect()
    }

    /// Note that `groups` reached the hub. One changed again since it was read
    /// stays owed.
    pub fn mark_sent(&self, groups: &[Outgoing]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut sent = tx.prepare(
                "UPDATE sync_rows SET pending = 0 \
                 WHERE kind = ?1 AND key = ?2 AND edited_at = ?3 AND deleted = ?4",
            )?;
            for g in groups {
                sent.execute(params![g.kind, g.key, g.edited_at, g.deleted])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

pub(super) fn is_replica(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("SELECT replica FROM sync_local", [], |r| r.get(0))?)
}

impl Db {
    /// Open the library for a machine that is a studio, or is not. Returns
    /// whether this launch joined: the database was made a copy just now.
    ///
    /// A studio plays from the owner's library under the owner's track ids.
    /// The library this machine had is not that, so it is set aside — here,
    /// before anything holds a track by its id. A machine that stops being a
    /// studio keeps its copy as its own.
    pub fn open_for(path: &Path, studio: bool) -> Result<(Opened, bool)> {
        let mut opened = Self::open(path)?;
        let replica = opened.db.is_replica()?;
        if studio && !replica {
            if !opened.db.holds_nothing()? {
                drop(opened);
                Self::set_aside(path)?;
                opened = Self::open(path)?;
            }
            opened.db.become_replica()?;
            return Ok((opened, true));
        }
        if !studio && replica {
            opened.db.leave_replica()?;
        }
        Ok((opened, false))
    }

    /// Whether this database is a studio's copy of an owner's library.
    pub fn is_replica(&self) -> Result<bool> {
        is_replica(&self.conn.lock())
    }

    /// Whether the library holds nothing a join would lose.
    pub fn holds_nothing(&self) -> Result<bool> {
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM tracks) \
                AND NOT EXISTS (SELECT 1 FROM library_roots) \
                AND NOT EXISTS (SELECT 1 FROM saved_playlists)",
            [],
            |r| r.get(0),
        )?)
    }

    /// Make this database a studio's copy, to be filled from the hub.
    pub fn become_replica(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sync_local SET replica = 1, capture = 0, library_id = NULL, pulled_rev = 0",
            [],
        )?;
        Ok(())
    }

    /// Keep the copy as this machine's own library. See
    /// `docs/shared-library.md#joining-and-leaving`.
    pub fn leave_replica(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sync_local SET replica = 0, library_id = NULL, pulled_rev = 0",
            [],
        )?;
        Ok(())
    }

    /// Start copying the hub library `library_id`, from its first revision.
    pub fn follow(&self, library_id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sync_local SET library_id = ?, pulled_rev = 0",
            [library_id],
        )?;
        Ok(())
    }

    /// The last hub revision this copy holds.
    pub fn pulled_rev(&self) -> Result<i64> {
        let conn = self.conn.lock();
        Ok(conn.query_row("SELECT pulled_rev FROM sync_local", [], |r| r.get(0))?)
    }

    /// Apply one page of the hub's rows, in revision order, and move the
    /// cursor past it — together, so a page is never half taken. Returns how
    /// many groups changed something here. See
    /// `docs/shared-library.md#applying-a-pull`.
    pub fn apply(&self, page: &[Incoming], now_ms: i64) -> Result<usize> {
        let Some(last) = page.iter().map(|g| g.rev).max() else {
            return Ok(0);
        };
        let mut conn = self.conn.lock();
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('tracks')")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let tx = conn.transaction()?;
        tx.execute("UPDATE sync_local SET applying = 1", [])?;
        let mut applied = 0;
        let mut purged = Vec::new();
        for group in page {
            let Ok(id) = group.key.parse::<i64>() else {
                continue;
            };
            match (group.kind.as_str(), group.deleted, &group.doc) {
                ("root", true, _) => {
                    applied += tx.execute("DELETE FROM library_roots WHERE id = ?", [id])?;
                }
                ("root", false, Some(doc)) => {
                    let Some(content_type) = doc.get("content_type").and_then(|v| v.as_str())
                    else {
                        continue;
                    };
                    applied += tx.execute(
                        "INSERT INTO library_roots (id, content_type) VALUES (?1, ?2) \
                         ON CONFLICT (id) DO UPDATE SET content_type = excluded.content_type",
                        params![id, content_type],
                    )?;
                }
                ("track", true, _) => purged.push(id),
                ("track", false, Some(doc)) => {
                    if apply_track(&tx, &columns, id, doc, group, now_ms)? {
                        applied += 1;
                    } else {
                        log::warn!(
                            "shared library: track {id} came without a path and was left out"
                        );
                    }
                }
                // A kind this build takes no part in yet.
                _ => {}
            }
        }
        applied += forget_tracks(&tx, &purged, now_ms)?;
        tx.execute("UPDATE sync_local SET applying = 0, pulled_rev = ?", [last])?;
        tx.commit()?;
        self.read_roots(&conn)?;
        Ok(applied)
    }
}

/// The trio and who owns it: left alone on a row this machine holds as
/// `manual`, as `set_auto_cue` leaves it.
const TRIO: &[&str] = &["cue_in_ms", "cue_out_ms", "next_start_ms", "auto_cue_state"];

/// Write one `track` document under the owner's id. `false` when the document
/// lacks what a row cannot be without.
fn apply_track(
    conn: &Connection,
    columns: &[String],
    id: i64,
    doc: &serde_json::Value,
    group: &Incoming,
    now_ms: i64,
) -> Result<bool> {
    let Some(doc) = doc.as_object() else {
        return Ok(false);
    };
    if !doc.get("path").is_some_and(serde_json::Value::is_string) {
        return Ok(false);
    }
    // A key this build has no column for is ignored, and a column the document
    // lacks keeps its default: the two builds need not be the same version.
    let sent: Vec<&String> = columns
        .iter()
        .filter(|c| !NOT_IN_A_TRACK_DOCUMENT.contains(&c.as_str()) && doc.contains_key(*c))
        .collect();
    let mut names = vec![
        "id".to_owned(),
        "waveform".to_owned(),
        "auto_cue_levels".to_owned(),
    ];
    let mut sets = vec![
        "waveform = excluded.waveform".to_owned(),
        "auto_cue_levels = excluded.auto_cue_levels".to_owned(),
    ];
    let mut values = vec![
        Value::Integer(id),
        group.waveform.clone().map_or(Value::Null, Value::Blob),
        group.levels.clone().map_or(Value::Null, Value::Blob),
    ];
    for column in sent {
        let keep = if let Some((_, bit)) = EDITABLE.iter().find(|(c, _)| c == column) {
            Some(format!("tracks.edited_fields & {bit}"))
        } else if TRIO.contains(&column.as_str()) {
            Some("tracks.auto_cue_state = 'manual'".to_owned())
        } else {
            None
        };
        sets.push(match keep {
            Some(kept) => format!(
                "{column} = CASE WHEN {kept} THEN tracks.{column} ELSE excluded.{column} END"
            ),
            None => format!("{column} = excluded.{column}"),
        });
        names.push(column.clone());
        values.push(match &doc[column.as_str()] {
            serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Value::Integer(i),
                None => n.as_f64().map_or(Value::Null, Value::Real),
            },
            serde_json::Value::String(text) => Value::Text(text.clone()),
            _ => Value::Null,
        });
    }
    let sql = format!(
        "INSERT INTO tracks ({}) VALUES ({}) ON CONFLICT (id) DO UPDATE SET {}",
        names.join(", "),
        placeholders(names.len()),
        sets.join(", ")
    );
    let mut write = conn.prepare_cached(&sql)?;
    match write.execute(params_from_iter(values.iter())) {
        Ok(_) => Ok(true),
        // The hub holds each track as it reads now, so the row that used to be
        // at this path may not have been told it left yet. Its own document
        // follows; until then it is missing, which is what it is.
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            conn.execute(
                "UPDATE tracks SET missing_since = ?1 \
                 WHERE missing_since IS NULL AND id <> ?2 \
                   AND root_id = ?3 AND path = ?4",
                params![
                    now_ms,
                    id,
                    doc.get("root_id")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    doc["path"].as_str()
                ],
            )?;
            write.execute(params_from_iter(values.iter()))?;
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Delete tracks the owner purged, with the cleanup a purge here does. The
/// hub keeps only the tombstone, so a copy that was behind never heard the
/// track went missing first — which is what the unbinding looks for.
fn forget_tracks(conn: &Connection, ids: &[i64], now_ms: i64) -> Result<usize> {
    for chunk in ids.chunks(ID_CHUNK) {
        let mut params = vec![now_ms];
        params.extend_from_slice(chunk);
        conn.execute(
            &format!(
                "UPDATE tracks SET missing_since = ?1 \
                 WHERE missing_since IS NULL AND id IN ({})",
                super::placeholders_from(chunk.len(), 2)
            ),
            params_from_iter(params),
        )?;
    }
    super::saved_playlists::unbind_purged(conn, ids)?;
    let mut deleted = 0;
    for chunk in ids.chunks(ID_CHUNK) {
        let list = placeholders(chunk.len());
        deleted += conn.execute(
            &format!("DELETE FROM tracks WHERE id IN ({list})"),
            params_from_iter(chunk),
        )?;
        conn.execute(
            &format!("UPDATE play_log SET track_id = NULL WHERE track_id IN ({list})"),
            params_from_iter(chunk),
        )?;
    }
    Ok(deleted)
}

/// Mark every group the library holds as owed, keeping the stamp of one that
/// already has it.
fn owe_everything(conn: &Connection, now_ms: i64) -> Result<()> {
    let mut groups = vec![
        "SELECT 'root', id FROM library_roots".to_owned(),
        "SELECT 'track', id FROM tracks".to_owned(),
        "SELECT 'cue', id FROM tracks \
         WHERE fade_in_ms IS NOT NULL OR fade_out_ms IS NOT NULL OR auto_cue_state = 'manual'"
            .to_owned(),
        "SELECT 'hidden', id FROM tracks WHERE hidden_at IS NOT NULL".to_owned(),
        "SELECT 'playlist', uid FROM saved_playlists WHERE uid IS NOT NULL".to_owned(),
        "SELECT 'dismissal', kind || ':' || key FROM health_dismissals".to_owned(),
    ];
    groups.extend(EDITABLE.iter().map(|(column, bit)| {
        format!("SELECT 'edit', id || ':{column}' FROM tracks WHERE edited_fields & {bit}")
    }));
    for select in groups {
        conn.execute(
            &format!(
                "INSERT INTO sync_rows (kind, key, edited_at) \
                 SELECT g.*, ?1 FROM ({select}) AS g WHERE true \
                 ON CONFLICT (kind, key) DO NOTHING"
            ),
            [now_ms],
        )?;
    }
    conn.execute("UPDATE sync_rows SET pending = 1", [])?;
    Ok(())
}

/// A row that has gone since the group was marked is sent as its tombstone.
fn gone(group: Outgoing) -> Outgoing {
    Outgoing {
        deleted: true,
        ..group
    }
}

fn root_document(conn: &Connection, group: Outgoing) -> Result<Outgoing> {
    let content_type: Option<String> = conn
        .query_row(
            "SELECT content_type FROM library_roots WHERE id = ?",
            [&group.key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match content_type {
        Some(content_type) => Outgoing {
            doc: Some(serde_json::json!({ "content_type": content_type })),
            ..group
        },
        None => gone(group),
    })
}

/// The row as a document keyed by column name, so a column added to `tracks`
/// needs nothing here. See `docs/shared-library.md#the-hub`.
fn track_document(conn: &Connection, group: Outgoing) -> Result<Outgoing> {
    let mut stmt = conn.prepare_cached("SELECT * FROM tracks WHERE id = ?")?;
    let columns: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    let mut rows = stmt.query([&group.key])?;
    let Some(row) = rows.next()? else {
        return Ok(gone(group));
    };
    let mut doc = serde_json::Map::new();
    for (i, column) in columns.iter().enumerate() {
        if NOT_IN_A_TRACK_DOCUMENT.contains(&column.as_str()) {
            continue;
        }
        let value = match row.get_ref(i)? {
            ValueRef::Null => serde_json::Value::Null,
            ValueRef::Integer(n) => n.into(),
            ValueRef::Real(x) => x.into(),
            ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
            ValueRef::Blob(_) => {
                anyhow::bail!("tracks.{column} is a BLOB with no place in the hub")
            }
        };
        doc.insert(column.clone(), value);
    }
    Ok(Outgoing {
        doc: Some(doc.into()),
        waveform: row.get("waveform")?,
        levels: row.get("auto_cue_levels")?,
        ..group
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rusqlite::{params, Connection};
    use rusqlite_migration::Migrations;

    use super::super::{Db, Dismissal, TrackInsert, TrackMetadataUpdate, MIGRATION_STEPS};
    use super::{Incoming, EDITABLE, NOT_IN_A_TRACK_DOCUMENT};
    use crate::audio::cue_points::CuePoints;
    use crate::audio_measure::level_envelope::RmsWindows;
    use crate::library::auto_cue::{Analysed, AutoCue, Thresholds};

    /// A library that captures, as one that has taken a role will.
    fn capturing() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .lock()
            .execute("UPDATE sync_local SET capture = 1", [])
            .unwrap();
        db
    }

    fn add_track(db: &Db) -> i64 {
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO tracks (path, content_type, title, artist, album, duration) \
             VALUES ('/' || hex(randomblob(4)) || '.mp3', 'music', 'T', 'A', 'B', 200.0)",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Forget what has been captured so far, as a push would.
    fn settle(db: &Db) {
        db.conn.lock().execute("DELETE FROM sync_rows", []).unwrap();
    }

    /// What is waiting to go out, as `kind key`, a tombstone marked `-`.
    fn pending(db: &Db) -> Vec<String> {
        let conn = db.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT kind || ' ' || key || iif(deleted, ' -', '') FROM sync_rows \
                 WHERE pending AND machine IS NULL ORDER BY kind, key",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    }

    fn analysis(cue_out_ms: i64) -> Analysed {
        Analysed {
            cue: AutoCue {
                cue_in_ms: Some(100),
                cue_out_ms: Some(cue_out_ms),
                next_start_ms: Some(cue_out_ms - 1000),
            },
            levels: RmsWindows {
                rms: vec![0.0, 0.5, 0.5, 0.0],
                duration_ms: 200,
            }
            .envelope(),
            thresholds: Thresholds {
                silence_dbfs: -70.0,
                segue_dbfs: -20.0,
            },
            at_ms: 7,
        }
    }

    fn retitle(db: &Db, id: i64, title: &str) {
        db.update_track_metadata(&TrackMetadataUpdate {
            id,
            title: Some(title.into()),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn a_standalone_library_captures_nothing() {
        let db = Db::open_in_memory().unwrap();
        let id = add_track(&db);
        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();
        retitle(&db, id, "Edited");
        db.add_root("music").unwrap();
        db.create_saved_playlist("Show", &[id], 1).unwrap();
        db.set_dismissal(&Dismissal {
            kind: "exact".into(),
            key: "k".into(),
            value: "1,2".into(),
        })
        .unwrap();

        let rows: i64 = db
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM sync_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn nothing_is_captured_while_a_pull_is_being_applied() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);
        db.conn
            .lock()
            .execute("UPDATE sync_local SET applying = 1", [])
            .unwrap();

        retitle(&db, id, "From elsewhere");
        db.set_waveform(id, &[1], None).unwrap();

        assert_eq!(pending(&db), [] as [&str; 0]);
    }

    #[test]
    fn a_change_is_stamped_with_this_machine_and_the_time() {
        let before = crate::library::scanner::now_ms();
        let db = capturing();
        let id = add_track(&db);

        let (machine, at): (Option<String>, i64) = db
            .conn
            .lock()
            .query_row(
                "SELECT machine, edited_at FROM sync_rows WHERE kind = 'track' AND key = ?",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(machine, None);
        assert!((before..=crate::library::scanner::now_ms()).contains(&at));
    }

    #[test]
    fn a_scanned_file_is_a_track_change() {
        let db = capturing();
        let file = TrackInsert {
            path: "/a.mp3".into(),
            content_type: "music".into(),
            title: Some("One".into()),
            ..Default::default()
        };
        db.insert_track(&file).unwrap();
        assert_eq!(pending(&db), ["track 1"]);

        settle(&db);
        db.insert_track(&TrackInsert {
            title: Some("Two".into()),
            ..file
        })
        .unwrap();
        assert_eq!(pending(&db), ["track 1"], "the rescan of a changed file");
    }

    #[test]
    fn a_measurement_is_a_track_change_and_not_a_cue_change() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        db.set_waveform(id, &[1, 2], None).unwrap();

        assert_eq!(pending(&db), [format!("track {id}")]);
    }

    #[test]
    fn an_airing_is_not_captured() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        db.record_airing(id, 1000).unwrap();

        assert_eq!(pending(&db), [] as [&str; 0]);
    }

    #[test]
    fn a_fade_is_a_cue_change_while_the_trio_stays_the_owners() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        db.set_cue_points(
            id,
            CuePoints {
                fade_in_ms: Some(2000),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(pending(&db), [format!("cue {id}")]);
    }

    #[test]
    fn a_trio_moved_by_hand_is_a_cue_change_and_not_a_track_change() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        db.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(pending(&db), [format!("cue {id}")]);
    }

    #[test]
    fn an_automatic_trio_is_a_track_change() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        assert!(db
            .set_auto_cue(id, &analysis(190_000), "music", None)
            .unwrap()
            .is_some());

        assert_eq!(pending(&db), [format!("track {id}")]);
    }

    /// The analysis commit sorts the fades against its trio. When that moves
    /// one, the stored cue set changed, and it travels as any other save does.
    #[test]
    fn an_analysis_that_moves_a_fade_is_a_cue_change_too() {
        let db = capturing();
        let id = add_track(&db);
        db.set_cue_points(
            id,
            CuePoints {
                fade_out_ms: Some(150_000),
                ..Default::default()
            },
        )
        .unwrap();
        settle(&db);

        assert!(db
            .set_auto_cue(id, &analysis(100_000), "music", None)
            .unwrap()
            .is_some());

        assert_eq!(pending(&db), [format!("cue {id}"), format!("track {id}")]);
    }

    /// Unhiding is the same group with no time in it, not a tombstone: the
    /// track is still there.
    #[test]
    fn hiding_and_unhiding_are_one_group() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        assert_eq!(db.hide_tracks(&[id], 5).unwrap(), [id]);
        assert_eq!(pending(&db), [format!("hidden {id}")]);

        settle(&db);
        assert_eq!(db.unhide_tracks(&[id]).unwrap(), [id]);
        assert_eq!(pending(&db), [format!("hidden {id}")]);
    }

    #[test]
    fn a_metadata_edit_is_captured_per_field() {
        let db = capturing();
        let id = add_track(&db);
        settle(&db);

        retitle(&db, id, "Edited");

        assert_eq!(pending(&db), [format!("edit {id}:title")]);
    }

    #[test]
    fn reverting_an_edit_leaves_a_tombstone_and_the_files_tags() {
        let db = capturing();
        let id = add_track(&db);
        retitle(&db, id, "Edited");
        settle(&db);

        db.revert_track_tags(
            id,
            &TrackInsert {
                title: Some("From the file".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            pending(&db),
            [format!("edit {id}:title -"), format!("track {id}")]
        );
    }

    #[test]
    fn a_purge_leaves_one_tombstone_for_the_track() {
        let db = capturing();
        let id = add_track(&db);
        retitle(&db, id, "Edited");
        db.set_cue_points(
            id,
            CuePoints {
                fade_in_ms: Some(2000),
                ..Default::default()
            },
        )
        .unwrap();
        db.conn
            .lock()
            .execute("UPDATE tracks SET missing_since = 5 WHERE id = ?", [id])
            .unwrap();

        assert_eq!(db.purge_tracks(&[id]).unwrap(), [id]);

        assert_eq!(pending(&db), [format!("track {id} -")]);
    }

    #[test]
    fn library_paths_are_captured() {
        let db = capturing();
        let id = db.add_root("music").unwrap();
        assert_eq!(pending(&db), [format!("root {id}")]);

        assert!(db.remove_root(id).unwrap());
        assert_eq!(pending(&db), [format!("root {id} -")]);
    }

    #[test]
    fn a_saved_playlist_is_captured_whole_under_its_uid() {
        let db = capturing();
        let track = add_track(&db);
        settle(&db);
        let list = db.create_saved_playlist("Show", &[track], 1).unwrap().id;
        let uid: String = db
            .conn
            .lock()
            .query_row("SELECT uid FROM saved_playlists", [], |r| r.get(0))
            .unwrap();
        assert_eq!(uid.len(), 32);
        assert_eq!(pending(&db), [format!("playlist {uid}")]);

        settle(&db);
        db.add_saved_entries(list, &[track], None, 2).unwrap();
        assert_eq!(pending(&db), [format!("playlist {uid}")], "an entry added");

        db.delete_saved_playlist(list).unwrap();
        assert_eq!(pending(&db), [format!("playlist {uid} -")]);
    }

    #[test]
    fn a_saved_playlist_has_a_uid_before_the_library_is_shared() {
        let db = Db::open_in_memory().unwrap();
        db.create_saved_playlist("One", &[], 1).unwrap();
        db.create_saved_playlist("Two", &[], 1).unwrap();

        let uids: BTreeSet<String> = {
            let conn = db.conn.lock();
            let mut stmt = conn.prepare("SELECT uid FROM saved_playlists").unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        assert_eq!(uids.len(), 2);
    }

    #[test]
    fn a_saved_playlist_made_before_this_step_is_given_a_uid() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = Migrations::from_slice(MIGRATION_STEPS);
        let step = MIGRATION_STEPS
            .iter()
            .position(|m| format!("{m:?}").contains("sync_rows"))
            .unwrap();
        migrations.to_version(&mut conn, step).unwrap();
        conn.execute_batch(
            "INSERT INTO saved_playlists (name, created_at, updated_at) VALUES ('A', 1, 1); \
             INSERT INTO saved_playlists (name, created_at, updated_at) VALUES ('B', 1, 1)",
        )
        .unwrap();

        migrations.to_latest(&mut conn).unwrap();

        let distinct: i64 = conn
            .query_row("SELECT COUNT(DISTINCT uid) FROM saved_playlists", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(distinct, 2);
    }

    #[test]
    fn dismissals_are_captured() {
        let db = capturing();
        db.set_dismissal(&Dismissal {
            kind: "exact".into(),
            key: "v2:ab".into(),
            value: "1,2".into(),
        })
        .unwrap();
        assert_eq!(pending(&db), ["dismissal exact:v2:ab"]);

        db.delete_dismissals(&[("exact".into(), "v2:ab".into())])
            .unwrap();
        assert_eq!(pending(&db), ["dismissal exact:v2:ab -"]);
    }

    fn waiting(db: &Db, kinds: &[&str]) -> Vec<String> {
        db.outgoing(kinds, 100)
            .unwrap()
            .into_iter()
            .map(|g| format!("{} {}{}", g.kind, g.key, if g.deleted { " -" } else { "" }))
            .collect()
    }

    #[test]
    fn taking_a_role_owes_the_hub_everything_already_there() {
        let db = Db::open_in_memory().unwrap();
        let plain = add_track(&db);
        let worked = add_track(&db);
        retitle(&db, worked, "Edited");
        db.set_cue_points(
            worked,
            CuePoints {
                fade_in_ms: Some(2000),
                ..Default::default()
            },
        )
        .unwrap();
        db.hide_tracks(&[worked], 5).unwrap();
        let root = db.add_root("music").unwrap();
        db.create_saved_playlist("Show", &[plain], 1).unwrap();

        db.start_capture(42).unwrap();

        let owed = pending(&db);
        for group in [
            format!("root {root}"),
            format!("track {plain}"),
            format!("track {worked}"),
            format!("cue {worked}"),
            format!("edit {worked}:title"),
            format!("hidden {worked}"),
        ] {
            assert!(owed.contains(&group), "{group} is owed, of {owed:?}");
        }
        assert!(!owed.contains(&format!("cue {plain}")));
        assert_eq!(
            owed.iter().filter(|g| g.starts_with("playlist ")).count(),
            1
        );
    }

    #[test]
    fn taking_a_role_twice_owes_nothing_new() {
        let db = Db::open_in_memory().unwrap();
        let id = add_track(&db);
        db.start_capture(42).unwrap();
        let sent = db.outgoing(&["track"], 10).unwrap();
        db.mark_sent(&sent).unwrap();

        db.start_capture(43).unwrap();

        assert_eq!(waiting(&db, &["track"]), [] as [&str; 0], "track {id}");
    }

    #[test]
    fn a_library_that_leaves_stops_capturing() {
        let db = capturing();
        db.stop_capture().unwrap();
        add_track(&db);
        assert_eq!(pending(&db), [] as [&str; 0]);
    }

    #[test]
    fn a_track_goes_out_as_its_row_with_the_blobs_beside_it() {
        let db = capturing();
        let id = add_track(&db);
        db.set_waveform(id, &[1, 2, 3], None).unwrap();
        db.record_airing(id, 1000).unwrap();

        let out = db.outgoing(&["track"], 10).unwrap();

        assert_eq!(out.len(), 1);
        let doc = out[0].doc.as_ref().unwrap().as_object().unwrap();
        assert_eq!(doc["title"], "T");
        assert_eq!(doc["duration"], 200.0);
        assert_eq!(doc["content_type"], "music");
        assert_eq!(doc["cue_in_ms"], serde_json::Value::Null);
        for local in NOT_IN_A_TRACK_DOCUMENT {
            assert!(
                !doc.contains_key(*local),
                "{local} stays out of the document"
            );
        }
        assert_eq!(out[0].waveform.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(out[0].levels, None);
    }

    #[test]
    fn a_library_path_goes_out_without_its_folder() {
        let db = capturing();
        let id = db.add_root("jingle").unwrap();

        let out = db.outgoing(&["root"], 10).unwrap();

        assert_eq!(out[0].key, id.to_string());
        assert_eq!(
            out[0].doc,
            Some(serde_json::json!({ "content_type": "jingle" }))
        );
    }

    #[test]
    fn only_the_kinds_asked_for_go_out_and_no_more_than_the_limit() {
        let db = capturing();
        let a = add_track(&db);
        let b = add_track(&db);
        retitle(&db, a, "Edited");

        assert_eq!(db.outgoing(&["track"], 1).unwrap().len(), 1);
        assert_eq!(
            waiting(&db, &["root", "track"]),
            [format!("track {a}"), format!("track {b}")]
        );
    }

    #[test]
    fn what_was_sent_is_no_longer_owed() {
        let db = capturing();
        add_track(&db);
        let sent = db.outgoing(&["track"], 10).unwrap();

        db.mark_sent(&sent).unwrap();

        assert_eq!(waiting(&db, &["track"]), [] as [&str; 0]);
    }

    #[test]
    fn a_group_changed_while_it_was_being_sent_stays_owed() {
        let db = capturing();
        let id = add_track(&db);
        let sent = db.outgoing(&["track"], 10).unwrap();
        db.conn
            .lock()
            .execute("UPDATE sync_rows SET edited_at = edited_at + 1", [])
            .unwrap();

        db.mark_sent(&sent).unwrap();

        assert_eq!(waiting(&db, &["track"]), [format!("track {id}")]);
    }

    #[test]
    fn a_purged_track_goes_out_as_a_tombstone() {
        let db = capturing();
        let id = add_track(&db);
        db.conn
            .lock()
            .execute("UPDATE tracks SET missing_since = 5 WHERE id = ?", [id])
            .unwrap();
        db.purge_tracks(&[id]).unwrap();

        let out = db.outgoing(&["track"], 10).unwrap();

        assert!(out[0].deleted);
        assert_eq!(out[0].doc, None);
    }

    #[test]
    fn a_new_hub_library_is_owed_everything_again() {
        let db = capturing();
        let id = add_track(&db);
        let sent = db.outgoing(&["track"], 10).unwrap();
        db.mark_sent(&sent).unwrap();
        assert_eq!(db.library_id().unwrap(), None);

        db.publish_as("lib-2", 99).unwrap();

        assert_eq!(db.library_id().unwrap().as_deref(), Some("lib-2"));
        assert_eq!(waiting(&db, &["track"]), [format!("track {id}")]);
        assert_eq!(
            db.outgoing(&["track"], 10).unwrap()[0].edited_at,
            sent[0].edited_at,
            "the stamp is when it changed, not when it was owed again"
        );
    }

    /// The edit triggers are a shipped migration step and cannot be made from
    /// [`EDITABLE`], so the two are written twice. A bit that differed between
    /// them would send an edit under another column's name.
    #[test]
    fn the_edit_triggers_name_every_editable_column_with_its_own_bit() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn.lock();
        let trigger = |name: &str| -> String {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let edits = trigger("sync_edit");
        let tags = trigger("sync_track_tags");

        for (column, bit) in EDITABLE {
            assert!(
                edits.contains(&format!(
                    "SELECT 'edit', new.id || ':{column}', CAST(unixepoch('subsec') * 1000 AS INTEGER), \
                     NOT new.edited_fields & {bit}\n"
                )),
                "sync_edit marks {column} with bit {bit}"
            );
            assert!(
                edits.contains(&format!(
                    "(new.edited_fields & {bit} AND (new.{column} IS NOT old.{column} \
                     OR NOT old.edited_fields & {bit}))"
                )),
                "sync_edit fires for {column} on bit {bit}"
            );
            assert!(
                tags.contains(&format!(
                    "(new.{column} IS NOT old.{column} AND NOT new.edited_fields & {bit})"
                )),
                "sync_track_tags leaves an edited {column} to its edit, by bit {bit}"
            );
        }
        assert_eq!(edits.matches("SELECT 'edit'").count(), EDITABLE.len());
        assert_eq!(tags.matches("IS NOT old.").count(), EDITABLE.len());
    }

    /// What an owner holding `owner`'s tracks would have published.
    fn published(owner: &Db) -> Vec<Incoming> {
        owner
            .outgoing(&["root", "track"], 100)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(i, g)| Incoming {
                kind: g.kind,
                key: g.key,
                rev: i as i64 + 1,
                deleted: g.deleted,
                doc: g.doc,
                waveform: g.waveform,
                levels: g.levels,
            })
            .collect()
    }

    fn replica() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.become_replica().unwrap();
        db
    }

    fn one(kind: &str, key: i64, rev: i64, doc: serde_json::Value) -> Incoming {
        Incoming {
            kind: kind.into(),
            key: key.to_string(),
            rev,
            deleted: false,
            doc: Some(doc),
            waveform: None,
            levels: None,
        }
    }

    fn tombstone(key: i64, rev: i64) -> Incoming {
        Incoming {
            deleted: true,
            doc: None,
            ..one("track", key, rev, serde_json::Value::Null)
        }
    }

    #[test]
    fn a_copy_takes_the_owners_tracks_under_the_owners_ids() {
        let owner = capturing();
        add_track(&owner);
        let id = add_track(&owner);
        owner.set_waveform(id, &[4, 5], None).unwrap();
        owner
            .set_auto_cue(id, &analysis(190_000), "music", None)
            .unwrap();
        let root = owner.add_root("jingle").unwrap();
        let copy = replica();

        let applied = copy.apply(&published(&owner), 1).unwrap();

        assert_eq!(applied, 3);
        let theirs = owner.get_track(id).unwrap().unwrap();
        let ours = copy.get_track(id).unwrap().unwrap();
        assert_eq!(ours.title, theirs.title);
        assert_eq!(ours.cue_points, theirs.cue_points);
        assert_eq!(copy.get_waveform(id).unwrap(), Some(vec![4, 5]));
        assert_eq!(copy.roots().get(root).unwrap().content_type, "jingle");
        assert_eq!(copy.pulled_rev().unwrap(), 3);
        assert_eq!(
            copy.search("T", Some("music"), None, None).unwrap().len(),
            2,
            "the copy is searchable"
        );
    }

    #[test]
    fn applying_is_not_captured_as_this_machines_change() {
        let owner = capturing();
        add_track(&owner);
        let copy = replica();
        copy.conn
            .lock()
            .execute("UPDATE sync_local SET capture = 1", [])
            .unwrap();

        copy.apply(&published(&owner), 1).unwrap();

        assert_eq!(pending(&copy), [] as [&str; 0]);
    }

    #[test]
    fn an_update_keeps_what_this_machine_edited_and_cued_by_hand() {
        let owner = capturing();
        let id = add_track(&owner);
        let copy = replica();
        copy.apply(&published(&owner), 1).unwrap();
        retitle(&copy, id, "Fixed here");
        copy.set_cue_points(
            id,
            CuePoints {
                cue_in_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();

        owner
            .conn
            .lock()
            .execute(
                "UPDATE tracks SET title = 'From the file', album = 'New album'",
                [],
            )
            .unwrap();
        owner
            .set_auto_cue(id, &analysis(190_000), "music", None)
            .unwrap();
        copy.apply(&published(&owner), 2).unwrap();

        let ours = copy.get_track(id).unwrap().unwrap();
        assert_eq!(ours.title, "Fixed here", "an edited column is kept");
        assert_eq!(ours.album, "New album", "an unedited one follows the owner");
        assert_eq!(
            ours.cue_points.cue_in_ms,
            Some(500),
            "a manual trio is kept"
        );
        assert_eq!(ours.cue_points.cue_out_ms, None);
    }

    #[test]
    fn a_purge_on_the_owner_deletes_here_and_unbinds_what_pointed_at_it() {
        let owner = capturing();
        let id = add_track(&owner);
        let copy = replica();
        copy.apply(&published(&owner), 1).unwrap();
        let list = copy.create_saved_playlist("Show", &[id], 1).unwrap().id;
        copy.record_airing(id, 1000).unwrap();

        assert_eq!(copy.apply(&[tombstone(id, 2)], 5).unwrap(), 1);

        assert!(copy.get_track(id).unwrap().is_none());
        let entries = copy.saved_playlist(list).unwrap().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].track.is_none(), "the entry stays, unmatched");
        let logged: Option<i64> = copy
            .conn
            .lock()
            .query_row("SELECT track_id FROM play_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(logged, None);
    }

    /// The hub holds each track as it reads now, so the newcomer at a path can
    /// arrive before the news that the old track left it.
    #[test]
    fn a_track_arriving_at_a_path_another_still_holds_retires_that_one() {
        let copy = replica();
        let doc = |title: &str| {
            serde_json::json!({
                "root_id": 0, "path": "/a.mp3", "content_type": "music", "title": title
            })
        };
        copy.apply(&[one("track", 1, 1, doc("Old"))], 1).unwrap();

        copy.apply(&[one("track", 2, 2, doc("New"))], 9).unwrap();

        let titles: Vec<String> = copy
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["New"]);
        assert_eq!(copy.missing_tracks().unwrap()[0].id, 1);
    }

    #[test]
    fn a_document_from_another_version_is_taken_as_far_as_it_is_understood() {
        let copy = replica();
        let doc = serde_json::json!({
            "path": "/a.mp3", "content_type": "music", "title": "T",
            "a_column_from_the_future": 1, "play_count": 99, "id": 7
        });

        assert_eq!(copy.apply(&[one("track", 3, 1, doc)], 1).unwrap(), 1);

        let track = copy.get_track(3).unwrap().unwrap();
        assert_eq!(track.title, "T");
        assert_eq!(track.play_count, 0, "a play count is never taken");
    }

    #[test]
    fn what_cannot_be_a_row_is_skipped_and_the_cursor_still_moves() {
        let copy = replica();
        let page = [
            one("track", 1, 1, serde_json::json!({ "title": "No path" })),
            one("cue", 1, 2, serde_json::json!({})),
            one(
                "track",
                2,
                3,
                serde_json::json!({ "path": "/b.mp3", "content_type": "music" }),
            ),
        ];

        assert_eq!(copy.apply(&page, 1).unwrap(), 1);

        assert!(copy.get_track(1).unwrap().is_none());
        assert!(copy.get_track(2).unwrap().is_some());
        assert_eq!(copy.pulled_rev().unwrap(), 3);
    }

    #[test]
    fn a_library_path_removed_by_the_owner_goes_here_too() {
        let copy = replica();
        copy.apply(
            &[one(
                "root",
                4,
                1,
                serde_json::json!({ "content_type": "music" }),
            )],
            1,
        )
        .unwrap();
        assert!(copy.roots().get(4).is_some());

        let gone = Incoming {
            deleted: true,
            doc: None,
            ..one("root", 4, 2, serde_json::Value::Null)
        };
        copy.apply(&[gone], 1).unwrap();

        assert!(copy.roots().get(4).is_none());
    }

    #[test]
    fn a_copy_does_not_decide_which_library_path_a_track_is_under() {
        let copy = replica();
        copy.apply(
            &[
                one("root", 1, 1, serde_json::json!({ "content_type": "music" })),
                one("root", 2, 2, serde_json::json!({ "content_type": "music" })),
                one(
                    "track",
                    5,
                    3,
                    serde_json::json!({
                        "root_id": 2, "path": "a.mp3", "content_type": "music"
                    }),
                ),
            ],
            1,
        )
        .unwrap();

        // Root 1 now contains the file as well, which on an owner would move
        // the track under it.
        let moved = copy
            .set_mounts(&std::collections::BTreeMap::from([
                (1, "/music".to_owned()),
                (2, "/music".to_owned()),
            ]))
            .unwrap();

        assert_eq!(moved, 0);
        let root: i64 = copy
            .conn
            .lock()
            .query_row("SELECT root_id FROM tracks WHERE id = 5", [], |r| r.get(0))
            .unwrap();
        assert_eq!(root, 2);
        assert_eq!(copy.get_paths_by_ids(&[5]).unwrap()[0].1, "/music/a.mp3");
    }

    fn track_count(db: &Db) -> usize {
        db.search("", None, None, None).unwrap().len()
    }

    #[test]
    fn a_machine_made_a_studio_sets_its_own_library_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        add_track(&Db::open(&path).unwrap().db);

        let (opened, joined) = Db::open_for(&path, true).unwrap();

        assert!(joined);
        assert!(opened.db.is_replica().unwrap());
        assert_eq!(track_count(&opened.db), 0);
        let kept = Db::open(&dir.path().join("radiodiodj.standalone.bak.db")).unwrap();
        assert_eq!(track_count(&kept.db), 1, "the old library is kept");
    }

    #[test]
    fn joining_a_second_time_keeps_the_library_set_aside_the_first_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        add_track(&Db::open(&path).unwrap().db);
        let (copy, _) = Db::open_for(&path, true).unwrap();
        add_track(&copy.db);
        add_track(&copy.db);
        drop(copy);
        drop(Db::open_for(&path, false).unwrap());

        let (again, joined) = Db::open_for(&path, true).unwrap();

        assert!(joined);
        assert_eq!(track_count(&again.db), 0);
        let first = Db::open(&dir.path().join("radiodiodj.standalone.bak.db")).unwrap();
        assert_eq!(track_count(&first.db), 1, "the library this machine built");
        let second = Db::open(&dir.path().join("radiodiodj.standalone.2.bak.db")).unwrap();
        assert_eq!(track_count(&second.db), 2, "the copy it left with");
    }

    #[test]
    fn a_studio_keeps_its_copy_from_one_launch_to_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        let (first, _) = Db::open_for(&path, true).unwrap();
        add_track(&first.db);
        drop(first);

        let (again, joined) = Db::open_for(&path, true).unwrap();

        assert!(!joined);
        assert_eq!(track_count(&again.db), 1);
        assert!(!dir.path().join("radiodiodj.standalone.bak.db").exists());
    }

    #[test]
    fn a_machine_that_stops_being_a_studio_keeps_the_copy_as_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radiodiodj.db");
        let (studio, _) = Db::open_for(&path, true).unwrap();
        add_track(&studio.db);
        drop(studio);

        let (alone, joined) = Db::open_for(&path, false).unwrap();

        assert!(!joined);
        assert!(!alone.db.is_replica().unwrap());
        assert_eq!(track_count(&alone.db), 1);
    }

    #[test]
    fn joining_and_leaving_reset_what_the_copy_follows() {
        let db = capturing();
        assert!(!db.is_replica().unwrap());
        assert!(db.holds_nothing().unwrap());
        add_track(&db);
        assert!(!db.holds_nothing().unwrap());

        db.become_replica().unwrap();
        db.follow("lib-1").unwrap();
        assert!(db.is_replica().unwrap());
        add_track(&db);
        assert_eq!(pending(&db).len(), 1, "a copy captures nothing new");

        db.leave_replica().unwrap();
        assert!(!db.is_replica().unwrap());
        assert_eq!(db.library_id().unwrap(), None);
    }

    /// Columns that are this machine's own and never travel. See
    /// `docs/shared-library.md#what-stays-on-the-machine`.
    const LOCAL_ONLY: &[&str] = &["id", "play_count"];

    /// A column no trigger names would never reach another machine, and
    /// nothing else would say so.
    #[test]
    fn every_tracks_column_is_claimed() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn.lock();
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('tracks')")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        let claimed: BTreeSet<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT sql FROM sqlite_master \
                     WHERE type = 'trigger' AND tbl_name = 'tracks' AND name LIKE 'sync_%'",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![], |r| r.get::<_, String>(0))
                .unwrap();
            rows.map(Result::unwrap)
                .filter_map(|sql| {
                    let list = sql.split_once("UPDATE OF")?.1.split_once("ON tracks")?.0;
                    Some(
                        list.split(',')
                            .map(|c| c.trim().to_string())
                            .collect::<Vec<_>>(),
                    )
                })
                .flatten()
                .collect()
        };

        for column in &columns {
            let local = LOCAL_ONLY.contains(&column.as_str());
            assert_ne!(
                local,
                claimed.contains(column),
                "tracks.{column}: name it in one sync trigger's UPDATE OF list, or in LOCAL_ONLY"
            );
        }
        for column in &claimed {
            assert!(columns.contains(column), "no tracks.{column}");
        }
    }
}
