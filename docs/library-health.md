# Library health — missing tracks, duplicates, unreadable files and disk changes

The _Library_ tab of _Settings_ tells the operator when the library needs
attention, and lets them act on it. It reports four things:

1. **Disk changes** — files added, changed or removed since the last scan.
2. **Unreadable tracks** — files the analysis pass could not decode.
3. **Missing tracks** — tracks whose file a scan could not find, one by one.
4. **Duplicates** — exact copies, and tracks that look like the same song.

A count on the Settings button says when any of them needs attention.

Implements [#376](https://github.com/Arskah/radiodiodj/issues/376). It builds on
the identity model in [track-identity.md](./track-identity.md). The library as a
whole is described in [library.md](./library.md).

Three rules hold throughout:

- **The app never touches audio files.** It does not delete, move or rename
  them. Every fix to the disk is made by the operator, in the file manager.
- **Nothing here changes the library by itself.** A check reports, a scan
  applies, and only the operator's _Purge_ deletes rows.
- **An unreachable library path is reported as unreachable,** never as a folder
  full of gone files.

## Where it lives

Library health sits in _Settings → Library_, under the library paths and _Scan
Library Now_. The paths, the scan, and what the scan left behind are in one
place.

```text
┌ Library ─────────────────────────────────────────────────────┐
│ Music / Commercials / Jingles paths                          │
│                                           [Scan Library Now] │
│ Disk changes                                      [Dismiss]  │
│   ⚠ /Volumes/radio/music is unreachable. Its tracks are      │
│     kept as they are.                                        │
│   12 new · 3 changed · 1 gone — checked 4 min ago            │
│   ▸ New (12)   ▸ Changed (3)   ▸ Gone (1)                    │
│   [Check now]                                                │
│                                                              │
│ Unreadable tracks (1)                                        │
│   Title / Artist   …/bad.mp3   fingerprint: probe: …      F  │
│                                                              │
│ Missing tracks (5)                                [Dismiss]  │
│   ☐ Title / Artist   …/old/path.mp3   2 d ago   ◇ ▶12  ≡     │
│   ☐ ▸ No longer under a library path (812)                   │
│   [Purge selected (1)…] [Purge all…]                         │
│                                                              │
│ Duplicates                                                   │
│   Still checking 140 tracks for exact copies…                │
│   Exact copies (2)                                           │
│   ▾ Title — Artist   2 copies                     [Dismiss]  │
│       Title / Artist  /a/Title.mp3  music  3:34  ▶12  F E C  │
│       Title / Artist  /b/Title.mp3  music  3:34  ▶0   F E C  │
│   Possible duplicates (1)                                    │
│   ▸ Title — Artist   2 copies   Dismissed   [Undo dismiss]   │
└──────────────────────────────────────────────────────────────┘
◇ has cue points   ▶ play count   ≡ in the playlist
F Show in folder   E Edit metadata…   C Cue points…
```

A section with nothing to report says _No issues_. Unreadable tracks and tag
writes are shown only when there is something to list. Disk changes says _Not
checked since the last scan_ or _No changes_ instead.

The tab and the Settings button carry the same **attention count**:

| counted                                      | weight   |
| -------------------------------------------- | -------- |
| missing tracks, unless dismissed             | 1 if any |
| exact duplicate groups, unless dismissed     | 1 each   |
| possible duplicate groups, unless dismissed  | 1 each   |
| a check that found changes, unless dismissed | 1        |
| unreachable library paths                    | 1 each   |
| failed tag writes                            | 1 each   |

Missing tracks count once, however many there are, so removing a library path
does not put _800_ on the button. An unreachable path cannot be dismissed. It
clears itself when the share is back.

A scan's result bar at the bottom of the window stays until the operator
dismisses it, so a scan that ends unwatched still says what it moved and marked
missing.

## Disk changes

The **library check** (`library/check.rs`) notices, without a scan, that the
disk and the library disagree. It lists every library path, reads each file's
modification time, and compares the result with the library. It reads **no tags
and no audio**, and **never writes**. On a network share it costs what the
listing phase of a scan costs: one directory read per folder and one stat per
file.

| reported      | meaning                                                           |
| ------------- | ----------------------------------------------------------------- |
| `new`         | a listed file no present track has                                |
| `changed`     | a present track whose file's mtime, or whose root's type, changed |
| `gone`        | a present track under a fully listed path whose file is not there |
| `unrooted`    | a present track no library path contains any more                 |
| `unreachable` | a library path that is not a readable directory                   |
| `partial`     | a library path listed with unreadable subfolders                  |

**The check predicts exactly what the next scan will do.** The listing, the
_changed_ test and the _gone_ rule live in `library/listing.rs`, and the scan and
the check both call them.

- A file back at a **missing** track's path counts as new. The scan will revive
  that track.
- A partly readable path contributes nothing to _gone_. An unreachable one is
  listed under _unreachable_, and its tracks are left alone.
- A track outside every library path is what the scan marks missing after a
  path is removed. The view calls it _No longer under a library path_.

Each count expands into its paths. The first 200 of each are drawn.

### When it runs

- **At launch,** five seconds in, unless a scan is already running (as after a
  [pre-1.0 reset](./database.md#pre-10-resets)). That scan answers the same
  question.
- **On a timer:** _Settings → Advanced → Library check interval (minutes)_,
  stored as `tuning.library.checkIntervalMin`. The default is 15, and `0` turns
  the timer off. A changed interval takes effect within a minute.
- **On demand,** from _Check now_.

One worker thread runs checks, one at a time, and never alongside a scan:

- A **scan that starts** cancels a running check. A check that overlapped a scan
  is thrown away rather than reported.
- A **completed scan** clears the report, since the library now matches the disk
  it listed, and restarts the timer.
- A **canceled scan** leaves new files unapplied, so a check runs straight
  after it.

The report is held in memory only. The next launch checks again.

The check never starts a scan. _Scan Library Now_, just above, is the operator's
button.

## Missing tracks

Every track whose file a scan could not find is listed, newest first, with:

- title, artist, and the path the file was last seen at
- how long ago it went missing, with the exact time on hover
- a marker when it has cue points, which a purge would delete
- its play count
- a marker when it is in the playlist or on air

Tracks that went missing because their **library path was removed** are folded
into one _No longer under a library path_ group with a select-all box. A removed
path is usually hundreds of tracks, while a deleted file is usually one.

### Purge

_Purge selected_ and _Purge all_ both ask first. The confirm step names how many
tracks will be deleted, how many of them have cue points, and how many will be
taken off the playlist. Both are disabled while a scan runs, since the scan may
be about to reattach some of them.

`purge_tracks(ids)`:

- is refused while a scan is running
- deletes only tracks that are still missing. A track a scan revived since the
  list was drawn is kept.
- removes the deleted tracks from the playlist, so nothing queued points at a
  row that no longer exists
- returns how many it deleted

History keeps a purged track, without a marker. It is a display log, and there
is no row left to describe.

### In the playlist, history and deck

Every playlist row, history row and main-deck header whose track is missing
carries a red `link_off` marker, with _File missing since …_ on hover. The deck
holds the whole file in memory, so a track that goes missing while it airs
finishes normally.

The marker comes from the report's list of missing ids, not from a field on the
track. Queued tracks are copies taken when they were queued, and would go stale
after the next scan.

Advancement treats a missing track as **never playable**:

- It skips it, even on a cold start before anything is cached. It does not load
  it and wait out the retry schedule.
- It **drops** the missing tracks it passes, up to the next stop marker. Missing
  tracks past a stop marker stay queued, marked, until advancement reaches them.
- An outage wait that only missing tracks were holding ends as soon as they are
  known to be missing.
- _Next_ skips them too.
- Playing a missing track from the playlist is refused, and the track stays
  where it is.

## Unreadable tracks

A track whose file the analysis pass read but could not decode: a truncated or
corrupt file, or a codec symphonia has no reader for (for example MP3 audio in a
RIFF/WAVE container). Each row shows the error and has _Show in folder_.

The failure is stored on the track (`analysis_error`, `analysis_failed_at`), and
the pass skips the track from then on. A scan that sees the file change (a new
modification time or content type), or reattaches it at a new path, clears the
failure, so a replaced file is tried again. A file that could not be _read_, such
as one on a share that dropped out, is not recorded: the pass tries it again on
its next run.

To fix one, replace the file with a good copy and scan, or delete it, scan, and
purge the missing track. Unreadable tracks are not in the attention count, and
cannot be dismissed.

## Duplicates

### Exact copies

Present tracks that share a [fingerprint](./track-identity.md#fingerprint): the
same audio at more than one path, of any content type.

Tracks are fingerprinted by the analysis pass that follows a first scan, so on a
new library exact copies appear over several minutes. While tracks are waiting
for the pass, the section says _Still checking N tracks for exact copies…_. A
track the pass could not decode is not waiting: it is listed under [Unreadable
tracks](#unreadable-tracks) instead, so the note does not stay up for good.

### Possible duplicates

Present **music** tracks with the same artist and title but different audio,
usually the same song in another encoding or edit. In practice they almost
always are the same recording, so the operator is told and decides.

Artist and title are compared after:

- lower-casing
- turning every run of anything but letters and digits into one space

Words are kept, so `Song (Remix)` and `Song` stay apart, while `Don't Stop!!`
and `DON'T STOP` match.

Left out:

- tracks without an artist or a title, and tracks whose artist is `Unknown`
  (what the scanner fills in for an untagged file). Otherwise every untagged
  file would land in one group.
- jingles and commercials, which reuse titles like _Station ID_ for recordings
  that really are different. Their exact copies are still found.

There is no duration limit. A radio edit next to the album version is worth
looking at, and each row shows its air time.

A group whose tracks all share one fingerprint is shown under exact copies only.

### Resolving them

Each row has _Show in folder_, _Edit metadata…_ and _Cue points…_.

- **An unwanted copy:** _Show in folder_, delete the file, then _Scan Library
  Now_. The copy becomes a missing track, which you purge. The kept copy loses
  nothing. The deleted copy's play count goes with it, because duplicates are
  never merged.
- **Two different songs grouped as possible duplicates:** _Edit metadata…_ to
  tell them apart, or _Dismiss_.
- **A deliberate copy:** _Dismiss_.

## Dismissing

Every finding has _Dismiss_. It takes the finding out of the attention count for
as long as the finding stays exactly as it was. A dismissed duplicate group
stays listed, greyed, folded and marked _Dismissed_, with _Undo dismiss_.

| finding        | remembered                 | counts again when                 |
| -------------- | -------------------------- | --------------------------------- |
| exact group    | its track ids              | a copy is added or leaves         |
| possible group | its track ids              | a copy is added or leaves         |
| missing tracks | the newest `missing_since` | another track goes missing        |
| disk changes   | what the check found       | a check finds something different |

Group and missing-track dismissals are stored in the `health_dismissals` table,
and deleted once their finding is gone. The disk-change dismissal is held in
memory, since the next launch checks again anyway.

## Tag writes

When _Write edits to file tags_ is on, a write that fails (a read-only file, a
share that is gone or too slow, or a copy whose fingerprint would change) is
listed under **Tag writes failed** with the error. It shows only while there is
a failure. The edit is still in the library. _Retry_ queues the write again,
even if the setting has since been turned off. _Dismiss_ drops the entry and
keeps the edit. The list lives in memory, so it is empty after a restart. See
[library.md](./library.md#editing-a-track).

On a macOS SMB mount, a file whose name another system created can sometimes
be read but not renamed. The write then fails with _the share could not rename
this file_. Rename the file on the server, then _Retry_.

## Wire and storage

`library/health.rs` keeps one `HealthReport`:

```text
HealthReport
  missing          [MissingTrack]    id, title, artist, path, missingSince,
                                     playCount, hasCuePoints, outsideRoots
  missingDismissed bool
  exact            [DuplicateGroup]  key, dismissed,
                                     tracks[{track, path, contentType}]
  possible         [DuplicateGroup]
  unhashed         count             present tracks waiting to be fingerprinted
  unreadable       [UnreadableTrack] track, path, contentType, error, failedAt
  check            CheckReport?      checkedAt, new, changed, gone, unrooted,
                                     unreachable, partial
  checkDismissed   bool
  tagWriteFailures [TagWriteFailure] id, title, artist, path, error, at
```

The renderer loads it with `library_health` and replaces it on every
`library-health` event. The backend rebuilds it:

- when a scan starts, finishes, is canceled or fails
- when the analysis pass starts or finishes
- after a metadata edit, since artist and title decide possible duplicates
- after a library path is added or removed
- after a purge, a dismissal, and every check
- when the tag write failures change

The renderer works out whether a missing track is queued from the playlist it
mirrors. That keeps the report independent of the playlist, which itself
depends on the report.

| command                       | does                                                                                         |
| ----------------------------- | -------------------------------------------------------------------------------------------- |
| `library_health`              | returns the current report                                                                   |
| `purge_tracks(ids)`           | see [Purge](#purge)                                                                          |
| `health_dismiss(kind, key)`   | `kind` is `exact`, `possible`, `missing` or `check`; `key` is the group key, empty otherwise |
| `health_undismiss(kind, key)` | undoes a dismissal                                                                           |
| `library_check_now`           | asks the worker for a check                                                                  |
| `retry_tag_write(id)`         | queues a failed tag write again                                                              |
| `dismiss_tag_write(id)`       | drops a failed tag write from the list                                                       |

Migration step 2 added the dismissals table:

```sql
CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing')),
  key   TEXT NOT NULL,   -- fingerprint, normalised "artist\x1ftitle", or ''
  value TEXT NOT NULL,   -- sorted ids, or the newest missing_since
  PRIMARY KEY (kind, key)
);
```

## Not built

**Locate…** would reattach a missing track to a file the operator picks, for
audio that changed so that no fingerprint can match. It would refuse unless the
track is missing, refuse a file that is already a present track, refuse a file
outside every library path, and refuse during a scan. It would set the path,
mtime, content type and a fresh fingerprint without re-reading tags. Until then,
purge the old track and keep the new one.

A **filesystem watcher** is not built either. The timer covers every share, and
a watcher would only make local paths report sooner.

## Accepted limits

- **Deleting a duplicate loses its play count.**
- **A possible group can be wrong.** _Dismiss_ it.
- **Changes are reported up to one interval late,** local paths included.
- **A check costs one stat per file.** On a slow share with a large library,
  lengthen the interval or set it to `0`.
- **A changed file whose mtime was preserved is not seen,** by the check or by
  the scan. See [track-identity.md](./track-identity.md#accepted-limits).
- **History is not rewritten** when a track is purged.

## Why it is built this way

**No _ignored_ state.** Flagging an unwanted duplicate would have needed a
hidden-but-present kind of track that every query and every scan respects.
Deleting the file reuses what already exists: the scan marks it missing, and the
operator purges it. A path exclusion list would be simpler still, but breaks the
moment a file moves.

**No merging.** Folding one track into another means rewriting queued items,
history and play counts. Keeping one copy covers the real case.

**Possible duplicates from the start.** Tracks that share an artist and title
are nearly always the same recording. The notice is worth more than the
occasional wrong group, which _Dismiss_ handles.

**A timer, not a watcher.** FSEvents and inotify do not report changes made by
other SMB or NFS clients, which is where this library lives.

**Report, never auto-scan.** A scan changes what the auto-playlist can pick. It
should not happen mid-show without the operator asking.

**Missing tracks are dropped, not retried.** The outage retry schedule waits for
a share to come back. A track that a completed scan could not find is not coming
back on its own.

**Shared rules, not copied ones.** The check calls the scan's listing and prune
rules, so the scanner's tests for the unreachable-share guard cover it too.

**Dismissals in the database.** They name track ids. A pre-1.0 reset replaces
the database and its ids together, whereas `session.json` would need scrubbing.
