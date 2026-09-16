# Track identity — surviving moves, renames and re-adds

A track keeps its id, and everything keyed on it, when its file is moved,
renamed, briefly unreachable, or removed from the library and added back. The
operator's work on a track is only deleted when the operator purges it.

Implements [#373](https://github.com/Arskah/radiodiodj/issues/373). The schema
and migration rules it relies on are in [database.md](./database.md).

## Problem

A track used to _be_ its path. `path` was `UNIQUE`, the scanner upserted
`ON CONFLICT(path)`, and two things hard-`DELETE`d rows: the scan prune, and the
pass that dropped rows outside every library path. Moving a file, remounting a
share under a new name, or removing and re-adding a library path deleted the
row. The next scan minted a new id with nothing attached.

The row carries work that exists nowhere else:

| state                      | column                      | cost to recreate       |
| -------------------------- | --------------------------- | ---------------------- |
| radio edit                 | `cue_*_ms`, `next_start_ms` | manual prep, per track |
| metadata edited in the app | `title`, `artist`, …        | manual, per track      |
| play count                 | `play_count`                | cannot be recreated    |
| waveform                   | `waveform`                  | a full decode          |

Queued items, history and `session.json` refer to tracks by id, so a deleted
row also left them dangling.

Worse, an **unmounted share wiped its whole root**. Walk errors were dropped, so
an unreachable root listed as zero files and prune deleted everything under it.
Root membership was a `LIKE root%` match, which is case-insensitive, treats `%`
and `_` as wildcards, and made a scan of `/Music` prune a sibling root
`/Music2`.

## Model

Two columns on `tracks`:

- `fingerprint` — the audio's identity, independent of path and tags. `NULL`
  until computed, or when the file cannot be demuxed.
- `missing_since` — unix ms when a scan first found the file gone. `NULL` means
  present.

Only present rows are held to a unique path (`tracks_path_present` is a partial
index). A missing row keeps its old path, so a different file can later take
that path as a new track.

## Fingerprint

`library/fingerprint.rs` computes `v1:` + SHA-256 over:

- the codec, sample rate and channel count
- the first **1 MiB** of demuxed packet payload for the default track

**Why packets.** Symphonia's demuxers already step over the tag blocks: ID3v2
and APE, FLAC metadata blocks, RIFF `LIST` and `id3 ` chunks, MP4 `udta`. They
also reassemble Ogg packets, so a comment edit that re-pages the stream changes
nothing. An external tag edit therefore keeps the identity, as does the
metadata write-back planned in
[#313](https://github.com/Arskah/radiodiodj/issues/313).

**Why no decode.** A decoded-PCM hash is tag-proof too, but lossy decoders
produce floats that a symphonia upgrade may shift, and that would orphan the
whole library at once. Packet bytes are file bytes.

**Why the head only.** A scan on an SMB or NFS share pays about a megabyte per
new file, not the whole file. The frame count and the tail are deliberately left
out: on an MP3 without a Xing header both are estimated from the file length,
and the file length includes the tags.

**Accepted risk.** Two masters that share their first megabyte of audio
collide, for example a radio edit and an album version with the same intro. A
collision only matters when one of them is missing while the other appears.

The `v1:` prefix lets a later algorithm coexist: values from different versions
never compare equal.

**When it is computed.**

- A **first scan** into an empty library stays tag-only, so it is as fast as
  before. There is nothing to match against yet.
- The **analysis pass** (`library/waveform_scan.rs`, the waveform worker)
  backfills every present row without one. It hashes the bytes it already read
  when a waveform is due, and reads only the head of the file otherwise. It
  starts at launch and after every scan, so it never blocks either.
- **Later scans** fingerprint new paths, and re-fingerprint known paths whose
  mtime changed.

## Missing, not deleted

A present row is marked missing when its file is not in the scan's listing and
**either**:

- a root containing it was listed completely, **or**
- no configured library path contains it (the path was removed).

A root that is not a readable directory is skipped entirely. A root listed only
partially, because some subdirectory could not be read, is scanned but marks
nothing missing. **An unreachable share marks nothing missing.** Membership is
decided with `Path::starts_with`, which compares whole path components.

A row already missing keeps its first timestamp.

**What a missing row does.**

- It is **hidden** from the library, search, stats, playlist generation, the
  prefetch window and the analysis pass.
- It is **still readable by id** (`get_track`, `get_tracks_by_ids`,
  `get_track_load_info`), so history and the restored session keep their names.
- A queued item whose file is missing is never prefetched, so the engine skips
  it like any uncached item. If it is loaded anyway, the read fails and
  advancement moves on through `:load-failed`.
- A queued item whose row was **purged** is treated as a failed load too.

## Reconciling a scan

A scan lists every root first, then inspects each file (tags, and a fingerprint
where needed) on a small thread pool. `Db::reconcile` then applies everything in
**one transaction**, in this order:

1. **Revive.** A file back at a missing row's path revives that row when the
   fingerprints match, or when either one is unknown. This is the
   remove-and-re-add case, and it works before any fingerprint exists.
2. **Update** known paths whose file changed (the existing mtime delta cache).
3. **Mark missing** as above. This runs before step 4, so a file moved within a
   single scan finds its old row already missing.
4. For each **new path**, the first match wins:
   1. **Reattach** — a missing row with the same fingerprint (newest
      `missing_since`, then highest id). Only `path`, `content_type`, `mtime`
      and `missing_since` change. Tags are not re-read, so metadata edited in
      the app survives, as do cue points, play count and waveform. A move
      across roots takes the new root's content type.
   2. **Duplicate** — a present row with the same fingerprint (lowest id). A
      new row is inserted, then copies that row's operator state: title,
      artist, album, genre, year, bpm, play count, waveform and all five cue
      points. The copy is its own track from then on.
   3. **New** — a plain insert.

Two missing rows with one fingerprint: the newest reattaches and the other
stays missing. Two new copies of one missing track: the first reattaches and
the second duplicates it.

A **canceled** scan commits updates and revivals only. New paths wait for the
next complete scan, because committing them without step 3 could turn a file
that merely moved into a duplicate.

The scan summary reports how many files moved and how many went missing.

## Purge

Missing rows are deleted only by **Purge** in _Settings → Library_. The panel
shows how many tracks are missing and how many of those carry cue points, and
the confirm step names both counts. `purge_tracks` refuses while a scan
is running, since the scan may be about to reattach some of them. There is no
automatic retention.

Status (2026-09-16): [library-health.md](./library-health.md) replaces this
panel with a per-track list and selective purge (`purge_tracks(ids)`), and
drops purged tracks from the playlist. Purge stays explicit, with no retention.

## Accepted limits

- **Moved and re-encoded or re-tagged in one step** — a new track. The old row
  stays missing until purged.
- **Swapped by rename** (`a` ↔ `b`, with mtimes kept) — each path keeps its row.
  An unchanged path with an unchanged mtime is trusted without being re-read;
  that trust is what keeps a rescan fast.
- **Libraries from before the baseline** are reset rather than migrated
  (see [database.md](./database.md#pre-10-resets)), so this starts from a fresh
  library.
- **Metadata edits live on the row.** A field edited in the app is flagged
  in `edited_fields`, and a rescan of a changed file keeps it. The flags move
  with the row when it is reattached, and a duplicate copies them. A database
  reset loses edits that were never written into the file; see
  [library.md](./library.md#editing-a-track) for the opt-in write-back.
- **Automatic cue ownership** ([#372](https://github.com/Arskah/radiodiodj/issues/372))
  lives on the track row and inherits all of the above.
