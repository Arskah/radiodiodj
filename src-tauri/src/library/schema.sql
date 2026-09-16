CREATE INDEX tracks_fingerprint ON tracks(fingerprint) WHERE fingerprint IS NOT NULL;

CREATE UNIQUE INDEX tracks_path_present ON tracks(path) WHERE missing_since IS NULL;

CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing')),
  key   TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, key)
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
);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
  title, artist, album, genre,
  content='tracks',
  content_rowid='id'
);

CREATE TABLE 'tracks_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;

CREATE TABLE 'tracks_fts_data'(id INTEGER PRIMARY KEY, block BLOB);

CREATE TABLE 'tracks_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);

CREATE TABLE 'tracks_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;

CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
END;

CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;

CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album, genre ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, genre)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.genre);
  INSERT INTO tracks_fts(rowid, title, artist, album, genre)
  VALUES (new.id, new.title, new.artist, new.album, new.genre);
END;
