CREATE INDEX play_log_aired ON play_log(aired_at);

CREATE INDEX saved_playlist_entries_list
  ON saved_playlist_entries(playlist_id, position);

CREATE UNIQUE INDEX saved_playlists_uid ON saved_playlists(uid);

CREATE INDEX tracks_fingerprint ON tracks(fingerprint) WHERE fingerprint IS NOT NULL;

CREATE UNIQUE INDEX tracks_path_present ON tracks(root_id, path) WHERE missing_since IS NULL;

CREATE TABLE "health_dismissals" (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing', 'duration')),
  key   TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, key)
);

CREATE TABLE library_roots (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  content_type TEXT NOT NULL CHECK (content_type IN ('music', 'jingle', 'commercial'))
);

CREATE TABLE play_log (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER,
  aired_at INTEGER NOT NULL,
  artist   TEXT,
  title    TEXT,
  duration REAL
);

CREATE TABLE saved_playlist_entries (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id  INTEGER NOT NULL,
  position     INTEGER NOT NULL,
  track_id     INTEGER,
  fingerprint  TEXT,
  artist       TEXT,
  title        TEXT,
  duration     REAL,
  content_type TEXT
);

CREATE TABLE saved_playlists (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
, uid TEXT);

CREATE TABLE sync_local (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  capture    INTEGER NOT NULL DEFAULT 0,
  applying   INTEGER NOT NULL DEFAULT 0,
  library_id TEXT,
  pulled_rev INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE sync_rows (
  kind      TEXT NOT NULL,
  key       TEXT NOT NULL,
  edited_at INTEGER NOT NULL,
  machine   TEXT,
  deleted   INTEGER NOT NULL DEFAULT 0,
  pending   INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;

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
, edited_fields INTEGER NOT NULL DEFAULT 0, analysis_error TEXT, analysis_failed_at INTEGER, rg_gain REAL, rg_peak REAL, rg_measured_at INTEGER, auto_cue_state TEXT NOT NULL DEFAULT 'pending'
  CHECK (auto_cue_state IN ('pending', 'auto', 'manual')), auto_cue_version INTEGER, auto_cue_silence_db REAL, auto_cue_segue_db REAL, auto_cue_at INTEGER, auto_cue_levels BLOB, track_no          INTEGER, track_total       INTEGER, disc_no           INTEGER, disc_total        INTEGER, album_artist      TEXT, isrc              TEXT, initial_key       TEXT, comment           TEXT, tags_read_version INTEGER, detected_bpm REAL, bpm_confidence REAL, bpm_measured_at INTEGER, bpm_version INTEGER, detected_key TEXT, key_confidence REAL, key_measured_at INTEGER, key_version INTEGER, duration_measured_at INTEGER, tag_duration REAL, root_id INTEGER NOT NULL DEFAULT 0, hidden_at INTEGER);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
  title, artist, album, genre, album_artist,
  content='tracks',
  content_rowid='id'
);

CREATE TABLE 'tracks_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;

CREATE TABLE 'tracks_fts_data'(id INTEGER PRIMARY KEY, block BLOB);

CREATE TABLE 'tracks_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);

CREATE TABLE 'tracks_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;

CREATE TRIGGER saved_playlists_ai AFTER INSERT ON saved_playlists
WHEN new.uid IS NULL BEGIN
  UPDATE saved_playlists SET uid = lower(hex(randomblob(16))) WHERE id = new.id;
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

CREATE TRIGGER sync_dismissal_ad AFTER DELETE ON health_dismissals
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('dismissal', old.kind || ':' || old.key, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
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

CREATE TRIGGER sync_playlist_ad AFTER DELETE ON saved_playlists
WHEN (SELECT capture AND NOT applying FROM sync_local) AND old.uid IS NOT NULL BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('playlist', old.uid, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
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

CREATE TRIGGER sync_root_ad AFTER DELETE ON library_roots
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('root', old.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
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

CREATE TRIGGER sync_track_ad AFTER DELETE ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', old.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 1)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_ai AFTER INSERT ON tracks
WHEN (SELECT capture AND NOT applying FROM sync_local) BEGIN
  INSERT INTO sync_rows (kind, key, edited_at, deleted)
  VALUES ('track', new.id, CAST(unixepoch('subsec') * 1000 AS INTEGER), 0)
  ON CONFLICT (kind, key) DO UPDATE SET
    edited_at = excluded.edited_at, machine = NULL,
    deleted = excluded.deleted, pending = 1;
END;

CREATE TRIGGER sync_track_forget AFTER DELETE ON tracks BEGIN
  DELETE FROM sync_rows WHERE kind IN ('cue', 'hidden') AND key = CAST(old.id AS TEXT);
  DELETE FROM sync_rows WHERE kind = 'edit' AND key LIKE old.id || ':%';
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

CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre, album_artist)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre, old.album_artist);
END;

CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
  INSERT INTO tracks_fts(rowid, title, artist, album, genre, album_artist)
  VALUES (new.id, new.title, new.artist, new.album, new.genre, new.album_artist);
END;

CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album, genre, album_artist ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre, album_artist)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre, old.album_artist);
  INSERT INTO tracks_fts(rowid, title, artist, album, genre, album_artist)
  VALUES (new.id, new.title, new.artist, new.album, new.genre, new.album_artist);
END;
