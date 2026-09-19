# Library database — schema and migrations

The library lives in SQLite (`radiodiodj.db`, WAL mode) and is opened by
`library/db.rs`. This page is the set of rules for changing its schema.

## Opening

`Db::open` runs these steps in order:

1. **Legacy reset.** If the file has a schema (`user_version > 0`) and its
   `PRAGMA application_id` is a known older epoch, it is moved aside and a
   fresh database is created. See [Pre-1.0 resets](#pre-10-resets).
2. **Refusal.** If the schema is newer than this build knows, or the epoch is
   one it does not recognise, `OpenError::TooNew` is returned **before anything
   is written**. The app hides its window, shows a dialog, and quits. An older
   build must never touch a newer library: a pre-#373 build would hard-delete
   rows that newer builds only mark missing.
3. **Backup.** If migrations are pending, the database is copied with
   `VACUUM INTO` to `radiodiodj.v{N}.bak.db`, where `N` is the version being
   migrated from. The newest two copies are kept.
4. **Migrate.** `rusqlite_migration` applies the pending steps in one
   transaction and bumps `user_version`, so an interrupted migration rolls back
   cleanly.

## Rules for a schema change

- **Append only.** `MIGRATION_STEPS` in `db.rs` is the schema's history. Add a
  new `M::up(...)` at the end. Never edit, reorder or remove a step that has
  shipped: `user_version` is simply the number of steps applied.
- **Keep steps fast.** Migrations run on the launch path. Anything that scales
  with the library, or reads audio files, belongs in a background pass. The
  fingerprint backfill in `library/waveform_scan.rs` is the pattern: a nullable
  column, filled after launch, with the code tolerating `NULL`.
- **A backfilled column needs its own "done" marker** when `NULL` is a valid
  result. `rg_gain IS NULL` cannot mean "not yet measured", because a silent
  file measures successfully and has no gain — so `rg_measured_at` carries that,
  and the analysis queue keys off it. Without the marker such a file is decoded
  again on every pass forever.
- **New columns are nullable or have a default**, so the step is a plain
  `ALTER TABLE ... ADD COLUMN`.
- **Anything SQLite cannot `ALTER`** (dropping a constraint, changing a column)
  needs the table-rebuild procedure. Rebuild `tracks_fts` with
  `INSERT INTO tracks_fts(tracks_fts) VALUES('rebuild')`, and recreate the
  triggers and partial indexes. Use `M::up_with_hook` if Rust code is needed.
- **Operator work is never in the upsert's `SET` list.** Cue points, play
  count, waveform, fingerprint and the loudness measurement must survive a
  rescan. `UPSERT_TRACK_SQL`
  touches only tag-derived columns, and it only overwrites the fingerprint with
  a non-null value. It also clears the recorded analysis failure, since it runs
  only for a file that changed.
- **No foreign keys.** `PRAGMA foreign_keys` is off (SQLite's default, never
  set here), so an `ON DELETE` clause would be decoration that silently never
  fires. A table referencing `tracks(id)` declares the column plain and the
  deleting code cleans up explicitly: `purge_tracks` nulls `play_log.track_id`
  in the same transaction as the delete.
- **A table that records what happened keeps its own snapshot** of the fields it
  reports on. `play_log` stores the artist, title and duration as they read at
  air time, so a purge or a later tag fix cannot rewrite the record. See
  [rotation.md](./rotation.md).
- **Index only what search needs.** `tracks_au` fires on
  `UPDATE OF title, artist, album, genre`, so writing a waveform or bumping a
  play count does not rewrite the FTS row.
- **Update the snapshot in the same commit.** `src-tauri/src/library/schema.sql`
  is what a fresh database looks like, and the test `schema_matches_snapshot`
  compares against it. Regenerate it with:

  ```bash
  UPDATE_SCHEMA=1 cargo test --manifest-path src-tauri/Cargo.toml schema_matches_snapshot
  ```

- **Seed the new version.** Append an entry to `SEEDS` in the `db.rs` tests.
  `every_step_preserves_seeded_rows` migrates a row written at every version to
  the latest one and checks that the operator work on it survives.

## Pre-1.0 resets

Before 1.0, the schema may be squashed into a new baseline instead of carrying
every step forever. The current baseline is epoch 1 (`application_id`
`0x52444a31`, "RDJ1"). It replaced the four hand-rolled steps shipped up to
0.17.0.

To squash again:

1. Replace `MIGRATION_STEPS` with a single new baseline, and have it stamp a new
   `application_id`.
2. Set `DB_EPOCH` to that value, and add the old epoch to `LEGACY_EPOCHS`.
3. Regenerate `schema.sql`, and reset `SEEDS` to one entry.

On its next launch, a database from an older epoch is renamed to
`radiodiodj.legacy-v{N}.bak.db` and a fresh one is created. `setup` then:

- drops every track id from `session.json`, keeping the old file as
  `session.legacy.bak.json`. Ids restart at 1, so a kept id would restore a
  different track. Volumes and modes are kept.
- starts a scan to repopulate the library.
- reports the reset through `load_session`, so the toolbar shows _Library
  rebuilt for this version — rescanning…_ until that scan finishes.

A reset loses play counts, cue points, metadata edits and waveforms, although
the old file keeps them. Don't reset after 1.0: write a migration instead.

## Files

Next to the database, in the app data directory:

| file                            | written when                                     |
| ------------------------------- | ------------------------------------------------ |
| `radiodiodj.v{N}.bak.db`        | before migrating from version `N`; newest 2 kept |
| `radiodiodj.legacy-v{N}.bak.db` | an older epoch was reset                         |
| `session.legacy.bak.json`       | the session was cleared by a reset               |
