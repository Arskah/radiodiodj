# Airing log and rotation rules

Two things, in that order: a persistent record of what went on air, and the
auto-playlist selection rules that read it. The record is worth having on its
own — it is what the History tab should have been built on — and the rules are
impossible without it.

Designed 2026-09-18 against
[#282](https://github.com/Arskah/radiodiodj/issues/282).

## The problem

`generate()` in `playlist/generate.rs` picks a block of music with one
constraint: `exclude_ids`, the tracks already queued. It has no memory of what
aired. The same artist can return three songs later, and the same track can
return as soon as it leaves the queue. That is the most audible weakness of an
automated station, and the one thing every real rotation system solves first.

The material to fix it does not exist yet. `tracks.play_count` counts airings
but not _when_, and the only ordered record of recent play is
`AppState.history` in the renderer — a display array, capped at 100, wiped by a
Clear button, persisted as bare ids in `session.json`.

## The airing log

A single append-only table, written the moment a track goes on air.

```sql
CREATE TABLE play_log (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER,          -- NULL once the track is purged
  aired_at INTEGER NOT NULL, -- unix ms UTC, as missing_since
  artist   TEXT,             -- snapshot: what aired, under the tags it aired with
  title    TEXT,
  duration REAL
);
CREATE INDEX play_log_aired ON play_log(aired_at);
```

**Append-only, with no delete path.** Editing history is a lie about what the
station broadcast, and the Remove-row and Clear buttons on the History tab exist
only as visual symmetry with the playlist tab above them. They go. Nothing else
depends on them: `prev` reads the last entry without popping it, and the backend
returns the stepped-back track to the queue itself.

**The snapshot columns are the point.** A purge (#373) or a tag fix after the
fact must not rewrite or erase what was broadcast, so each row keeps the artist,
title and duration as they read at air time. `track_id` is a convenience for
requeue-from-history, not the identity of the row.

**No foreign key.** `PRAGMA foreign_keys` is off — SQLite's default, never set
in this codebase — so a declared `ON DELETE SET NULL` would silently not fire.
`Db::purge_tracks` nulls the ids itself, in the same transaction as the delete.
Explicit, at the call site, consistent with #373's stance that the operator
purges deliberately.

**No pruning.** At the live library's cadence a 24/7 station writes roughly
200k rows a year, a few megabytes beside the waveform blobs. The log is the
basis for a future export (royalty reporting is a real station obligation), and
a retention cap would quietly destroy the thing it was built for. If it ever
needs bounding, it is one `DELETE WHERE aired_at < ?`.

### What counts as an airing

Whatever `Effect::TrackPlayed` already counts. It fires from `set_current` and
from handover, so it covers auto-advance, operator `play_now`/`play_index`, and
`prev` — but not session `Resume` and not `Arm`, neither of which puts anything
on air. The log write joins the play-count bump inside one transaction, in the
same effect handler, so the two can never disagree.

Consequences accepted deliberately:

- **A skipped track still counts.** Two seconds of airtime logs an airing and
  blocks rotation. Gating on a minimum play duration would need a timer and
  would put `play_count` and the log permanently out of step.
- **`prev` logs twice.** The track genuinely aired twice.
- **The on-air track is in the log from the moment it starts**, so a refill
  during that song already sees it. That is the wanted behaviour.
- **A track purged between arming and airing logs nothing.** The write is
  `INSERT ... SELECT ... FROM tracks WHERE id = ?`; no row, no insert, no error.

### History moves to the backend

With the log in place, renderer-owned history is a duplicate of it, and
[backend-owned-playlist.md](./backend-owned-playlist.md) can drop its one
remaining exception.

`Snapshot` gains `history` and the renderer renders it.
`Transition::displaced` — which exists solely to feed renderer history — is
deleted along with `AppState.appendHistory`, `removeFromHistory`,
`clearHistory`, and `session.history_ids`.

The window itself lives in `Playlist`, appended where a track actually leaves
the deck, and hydrated from `recent_airings` at launch. The engine stays a pure
state machine — no database read inside a transition — and history semantics are
specified by the same suite as the rest of it. `play_log` remains the durable
record; this is the window of it the operator sees. On hydrate the log's newest
entry is the airing of the track being restored to the deck, so it is dropped:
history holds what aired _before_ what is on air. Airings of tracks the library
no longer has never reach the snapshot — the row keeps the record, but there is
no track to show or requeue.

A whole list on every snapshot rather than an append event, for the reason the
snapshot design already gives: it is idempotent, and a dropped event cannot
desynchronise the UI. `Track` carries no waveform, so the capped list is tens of
kilobytes on a transition that happens once per song.

`autoPlaylist.historyCap` stops being a retention limit and becomes the
snapshot's `LIMIT` — how many airings the History tab shows. Its settings label
should say so, or an operator will read it as "forget everything older".

There is no backfill. `session.history_ids` has no timestamps, so it cannot seed
the log; the History tab is empty once, after the upgrade.

## The rotation rules

Two constraints on music selection, both measured in minutes of wall clock
against `aired_at`:

- **No-repeat title** — a track that aired inside `title_window_min` is not
  selected.
- **No-repeat artist** — a track whose artist aired inside `artist_window_min`
  is not selected.

**Minutes, not track counts.** The log already carries the timestamp, so the
predicate is an indexed range scan with no ranking subquery. It is also what a
listener perceives — "that was on twenty minutes ago" — and it does not let a
run of 6-second jingles burn the window, which an N-tracks window would.

**Music only.** In the live library every jingle and every commercial shares a
single artist string; an artist rule over them would block the whole pool after
one airing. Jingles keep plain random selection, commercials keep
`pick_random_from_bottom`.

### The queue counts as already aired

Refill tops the queue to 20 items whenever it falls below 5, so a track picked
now goes on air anywhere from immediately to roughly an hour later, and the
block being generated has no log rows at all. Selection against the log alone
would let one block contain the same artist three times — the most audible
failure the rules are meant to prevent.

So the constraint set is the log window **union the entire current queue**,
unconditionally. The queue is about an hour of audio, so treating all of it as
just-aired is conservative without any projected-air-time arithmetic.

This changes two signatures. `Refiller::generate` passes the queued _tracks_,
not just their ids, so the service can read their artists; and `generate()`
takes a rotation parameter object rather than a bare `exclude_ids` slice.

### Matching artists

In SQL, on `lower(trim(artist))`, with the blocklist normalised the same way in
Rust so both sides agree.

SQLite's `lower()` is ASCII-only, as is `NOCASE`, so `Ämmä` and `ämmä` do not
match. On a Finnish station that is a real if narrow gap. It is accepted for now
because the failure mode is a _missed_ constraint — an artist repeats — never a
wrong result or a stall, and the fix (a Rust-maintained `artist_key` column,
written by the scanner's upsert and snapshotted onto the log) is a migration and
a backfill that should be its own increment, justified by collisions actually
observed in a real library.

### Relaxation

Selection must degrade, never stall: a fresh install with 80 tracks cannot
satisfy a 3-hour title window, and `refill` silently extends the queue with
however few rows came back.

Staged, refetching only the deficit:

1. Ask for `n` with both rules.
2. Short by `k`? Ask for `k` with the artist rule dropped, excluding what stage
   1 picked.
3. Still short? Ask for the remainder with the title rule dropped too.

`exclude_ids` — the queued tracks — is never relaxed at any stage. A duplicate
inside the queue is a bug, not a degradation.

Deficit-only means the block is as constrained as the library allows and only
its tail is compromised, rather than one scarce slot disabling the rule for
everything. Each relaxation logs at `warn` once per refill, so an operator
learns their library is too small instead of merely hearing repeats.

On the live library (4295 music tracks, 1002 distinct artists) the ladder should
never run. If it does, something is wrong.

### Windows

`TuningConfig` gains a `rotation` section: `title_window_min` and
`artist_window_min`, clamped to `0..=10080` (7 days) in `normalize_tuning`, with
`0` disabling a rule — the convention `cadence_count` already uses for
`jingle_every`/`commercial_every`.

Defaults are 180 and 45 minutes. They are deliberately timid: they cannot starve
a small library out of the box, and on a library large enough for them to feel
decorative — 180 minutes is about 45 tracks, one percent of the live library —
the operator is already in Settings tuning everything else.

## Increments

1. **Airing log.** The migration, the write inside the `TrackPlayed` handler,
   the purge cleanup, History fed from the snapshot, the delete buttons and
   `history_ids` removed. Selection is untouched, so the existing playlist and
   engine suites validate that nothing moved.
2. **Rotation rules.** Rotation parameters through `Refiller` and `generate()`,
   the SQL predicates, the relaxation ladder, config plus settings UI, and tests
   for both rules and the small-pool fallback.

The refactor lands first so the mature suite is what validates it — the same
reasoning that ordered the backend-owned playlist work. Increment 1 is worth
shipping alone: history that survives a restart is an improvement with or
without rotation.

## Not in scope

- **Export.** The log exists to make it possible; the command, format and UI are
  separate work.
- **An artist table, or splitting `Artist feat. Guest`.** Artists are TEXT on
  `tracks`, and cleaning that up is library-health work, not rotation work.
- **Album and genre separation rules.** Same machinery once the log is there, no
  demand yet.
- **Dayparting or scheduled rotation clocks.** A different feature that would
  read the same log.
