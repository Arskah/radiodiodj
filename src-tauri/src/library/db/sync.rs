//! Change capture for a shared library: which groups this machine has changed
//! and not sent yet. See `docs/shared-library.md#change-capture`.

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rusqlite::{params, Connection};
    use rusqlite_migration::Migrations;

    use super::super::{Db, Dismissal, TrackInsert, TrackMetadataUpdate, MIGRATION_STEPS};
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
