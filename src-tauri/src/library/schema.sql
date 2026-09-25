CREATE INDEX play_log_aired ON play_log(aired_at);

CREATE INDEX tracks_fingerprint ON tracks(fingerprint) WHERE fingerprint IS NOT NULL;

CREATE UNIQUE INDEX tracks_path_present ON tracks(path) WHERE missing_since IS NULL;

CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing')),
  key   TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, key)
);

CREATE TABLE play_log (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER,
  aired_at INTEGER NOT NULL,
  artist   TEXT,
  title    TEXT,
  duration REAL
);

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
  CHECK (auto_cue_state IN ('pending', 'auto', 'manual')), auto_cue_version INTEGER, auto_cue_silence_db REAL, auto_cue_segue_db REAL, auto_cue_at INTEGER, auto_cue_levels BLOB, track_no          INTEGER, track_total       INTEGER, disc_no           INTEGER, disc_total        INTEGER, album_artist      TEXT, isrc              TEXT, initial_key       TEXT, comment           TEXT, tags_read_version INTEGER);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
  title, artist, album, genre, album_artist,
  content='tracks',
  content_rowid='id'
);

CREATE TABLE 'tracks_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;

CREATE TABLE 'tracks_fts_data'(id INTEGER PRIMARY KEY, block BLOB);

CREATE TABLE 'tracks_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);

CREATE TABLE 'tracks_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;

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
