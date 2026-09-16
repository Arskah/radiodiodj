# Library health — missing tracks, duplicates and disk changes

A single view under _Settings_ tells the operator when the library needs
attention and lets them act on it. It reports three things:

1. **Missing tracks** — listed one by one, not only counted.
2. **Duplicates** — exact copies, and tracks that look like the same song.
3. **Disk changes** — new, changed or gone files that no scan has picked up
   yet.

A badge on the Settings button shows while any of them needs attention.

Implements [#376](https://github.com/Arskah/radiodiodj/issues/376). It builds on
the identity model in [track-identity.md](./track-identity.md), and its one
schema change follows [database.md](./database.md).

Status (2026-09-16): increments 1 to 5 built. _Locate…_ (increment 6) is
not. The increments are at the [end](#increments).

## Problem

[#373](https://github.com/Arskah/radiodiodj/issues/373) made track identity
stable, but the operator sees little of it:

- _Settings → Library Sync_ shows a **count** of missing tracks and one Purge
  button. The operator cannot see which tracks are missing, or purge some and
  keep others.
- **Duplicates are invisible.** Exact copies share a fingerprint, but nothing
  groups them. The same song in two encodings is not detected at all.
- A missing track can still sit in the playlist or history with no marking.
  While the cache is cold, advancement may even try to load it and wait out the
  retry schedule.
- The library only changes when the operator presses **Scan**. Files added to a
  share go unnoticed until then, and so does a file that has gone.

## Principles

- **The app never touches audio files.** It does not delete, move or rename
  them. Every fix to the disk is done by the operator, in the file manager.
- **Nothing here changes the library by itself.** The check reports, a scan
  applies, and only the operator's _Purge_ deletes rows.
- **An unreachable library path is reported as unreachable,** never as a folder
  full of gone files. This is the same guard the scan has.
- **The backend owns the report.** The renderer mirrors it and derives its
  badges from it, the same way it mirrors the playlist.

## Decisions

Settled on 2026-09-16, against the open questions in #376.

| question                            | decision                                                                                                                                                                                              |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| How is a present duplicate removed? | **The operator deletes the file.** The next scan marks the row missing and the operator purges it. There is no _ignored_ flag and no path exclusion list.                                             |
| Merge duplicates?                   | **No.** The operator keeps one copy and removes the others. Queue and history entries are not rewritten, and play counts are not combined.                                                            |
| Possible duplicates?                | **In v1.** Tracks sharing an artist and title are almost always the same recording, so the operator is told about them. The operator decides, by deleting a file or by correcting the metadata.       |
| Auto-scan after a check?            | **No.** A check only reports. The library never changes mid-show without the operator knowing.                                                                                                        |
| Filesystem watcher?                 | **Not in v1.** FSEvents and inotify miss changes made by other SMB or NFS clients, which is most of this library. The timer is the baseline, and a watcher would only make local roots report sooner. |
| Missing track in the playlist?      | **Skipped at once,** even when the cache is cold, and dropped from the playlist as it is passed. The retry schedule exists for an unreachable share, and a missing track is known to be gone.         |

## The health report

`library/health.rs` builds one `HealthReport`:

```text
HealthReport
  missing:   [MissingTrack]      every row with missing_since set
  exact:     [DuplicateGroup]    present rows sharing a fingerprint
  possible:  [DuplicateGroup]    present music rows sharing artist + title
  unhashed:  count               present rows the analysis pass has not fingerprinted
  check:     CheckReport | null  the latest library check, see below
  dismissed: Dismissals          what the operator has already seen
```

The renderer reads it with `library_health` and receives it again as a
`library-health` event whenever it changes. The backend rebuilds it:

- after a scan finishes, is canceled or fails
- after a purge
- after a metadata edit (artist or title can make or break a possible group)
- when the analysis pass finishes, since the fingerprints it fills in create
  exact groups
- after each library check

A rebuild is two indexed queries over present rows plus one over missing rows,
which is cheap next to any of those triggers.

### Missing tracks

Each `MissingTrack` carries what a purge would destroy, and what the operator
needs to decide:

| field          | from                                                         |
| -------------- | ------------------------------------------------------------ |
| title, artist  | the row                                                      |
| path           | the row: the last path the file was seen at                  |
| `missingSince` | the row                                                      |
| `hasCuePoints` | any of the five markers is set                               |
| `playCount`    | the row                                                      |
| `queued`       | added by the renderer, from the playlist snapshot it mirrors |
| `outsideRoots` | no configured library path contains the path                 |

`queued` is the renderer's, so the report does not depend on the playlist,
which will itself depend on the report for missing ids.

`outsideRoots` separates the two reasons a track goes missing. A file that was
deleted is one row. A library path that was removed from _Settings_ is usually
hundreds, and the list shows those as one collapsed group per former root, so a
long list stays readable.

The list is sorted newest `missingSince` first.

### Exact duplicates

```sql
SELECT fingerprint FROM tracks
WHERE missing_since IS NULL AND fingerprint IS NOT NULL
GROUP BY fingerprint HAVING COUNT(*) > 1
```

The group key is the fingerprint. The existing partial index
`tracks_fingerprint` serves it.

A row without a fingerprint cannot be in an exact group yet. Right after a first
scan the analysis pass is still filling them in, so exact groups appear over
several minutes rather than all at once. The report's `unhashed` count says how
many present rows still lack one, and the view shows _Still checking N tracks_ while that is
non-zero.

### Possible duplicates

Present `music` rows whose **normalised artist and title** are equal, but which
are not already one exact group. Normalising:

- lower-cases (`str::to_lowercase`)
- turns every run of whitespace and punctuation into one space, and trims
- leaves words alone. `(Remix)`, `feat. X` and `Radio Edit` still tell tracks
  apart.

Rows with an empty artist or an empty title are left out. Otherwise every
untagged file would be one group.

There is **no duration gate.** The operator asked for the notice, and a radio
edit next to the album version is exactly the case worth looking at. Each row in
the group shows its air time, so the difference is visible.

Jingles and commercials are left out on purpose. Station material reuses titles
(_Station ID_, _Weather_) for recordings that are genuinely different. Exact
duplicates still cover them.

The normalisation is done in Rust, not SQL. The query selects present music rows
with an artist and a title, and groups them in memory. The group key is the
normalised `artist\u{1f}title`.

A possible group whose members all share one fingerprint is an exact group, and
is shown only there. A group that mixes both kinds is shown under _possible_,
since the operator has to look at it anyway.

### Dismissals

A deliberate duplicate must not keep the badge lit forever. Each finding can be
dismissed, and a dismissal is kept **only for the exact state that was
dismissed**:

| kind       | key                       | remembered                  | lights again when                     |
| ---------- | ------------------------- | --------------------------- | ------------------------------------- |
| `exact`    | fingerprint               | sorted member ids           | a member is added or leaves           |
| `possible` | normalised artist + title | sorted member ids           | a member is added or leaves           |
| `missing`  | —                         | newest `missing_since` seen | another track goes missing            |
| `check`    | —                         | nothing; held in memory     | the next check finds a different diff |

A dismissed finding stays **listed**, greyed and marked _Dismissed_, with an
_Undo_. Dismissing only turns the badge off.

`exact`, `possible` and `missing` dismissals are stored in the database, in a
table added by migration step 2:

```sql
CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing')),
  key   TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, key)
);
```

They belong in the database, not in `session.json`, because they name track
ids. A pre-1.0 reset replaces the database and its ids together, whereas a
session file would need scrubbing, as `setup` already does for the playlist. A
dismissal whose group no longer exists is deleted when the report is rebuilt.

`check` dismissals are not stored. The launch check runs again anyway.

## Resolving findings

Nothing here deletes or edits a file. Each finding has a path back to a clean
report:

- **A missing track that is really gone** — select it and _Purge selected_, or
  _Purge all_.
- **A missing track that was moved and re-encoded** — _Locate…_ (a later
  increment, see below), or purge it and keep the new track.
- **An exact duplicate** — _Show in folder_ on the copy to drop, delete it in
  the file manager, then _Scan now_. The copy becomes a missing track, which the
  operator purges. The kept copy loses nothing. The dropped copy's play count is
  lost with it, which is the cost of not merging.
- **A possible duplicate that is the same song** — the same as an exact one.
- **A possible duplicate that is a different song** — _Edit metadata…_ to tell
  them apart, or _Dismiss_.
- **Disk changes** — _Scan now_.

_Show in folder_ is the row action added in
[#378](https://github.com/Arskah/radiodiodj/pull/378). The health view reuses it,
along with _Edit metadata…_ and _Cue points…_ from the library row menu.

### Purge

`purge_tracks(ids)` replaced `purge_missing_tracks` and `get_missing_summary`.
_Purge all_ passes every missing id. It:

- refuses while a scan is running, as today, because the scan may be about to
  reattach some of them
- deletes only rows that are still missing, and ignores any other id. A row
  revived by a scan since the list was drawn is kept.
- removes the purged ids from the playlist, so no queued item points at a row
  that no longer exists
- returns the number deleted

The confirm step names the number of tracks, how many carry cue points, and how
many are queued.

History is a renderer-side display log. A purged track stays in it, without a
badge, since there is no row left to describe.

### Locate… (later increment)

For a missing track whose audio changed, so that no fingerprint can match. The
operator picks a file, and `relocate_track(id, path)`:

- refuses unless the row is missing
- refuses a path that is already a present track's
- refuses a path outside every library path, since the next scan would mark it
  missing again
- sets `path`, `mtime`, the content type of the root containing it, and a fresh
  fingerprint, and clears `missing_since`. Tags are not re-read, which matches
  a scan's reattach.
- refuses while a scan is running

## Library check

`library/check.rs` notices, without a full scan, that the disk and the library
disagree. It lists every library path, reads each file's mtime, and compares the
result with `Db::track_index()`. It reads **no tags and no audio**, so on a
network share it costs what the listing phase of a scan costs: one directory
read per folder and one stat per file.

```text
CheckReport
  checkedAt    unix ms
  new          [path]   listed, and no present row has the path
  changed      [path]   present row, file listed, mtime or content type differs
  gone         [path]   present row under a fully listed root, file not listed
  unrooted     [path]   present row no library path contains any more
  unreachable  [root]   not a readable directory
  partial      [root]   listed with unreadable subfolders
```

**The rules are the scan's rules, shared rather than copied.** The listing, the
_changed_ test (`should_rescan`) and the _gone_ test live in one module that
both `scan_all` and the check call. So a check never predicts something the
scan would not do.

- A file at a **missing** row's path counts as new. The scan will revive it.
- **Gone** applies the prune guard exactly: a root that was only partly listed
  contributes nothing to _gone_, and an unreachable root is reported under
  _unreachable_ with its rows untouched.
- A row outside every root would be marked missing by the scan too. It is
  reported as `unrooted`, and the view says _No longer under a library path_.

### When it runs

- **At launch,** once the window is up. It is skipped when the launch starts a
  scan, as after a pre-1.0 reset, since that scan answers the same question.
- **On a timer:** _Settings → Advanced → Library check interval_, stored as
  `tuning.library.checkIntervalMin`. The default is 15 minutes, and `0` turns
  the timer off.
- **On demand,** from _Check now_ in the view.

One worker thread runs checks, and at most one check at a time.

- A check does not start while a **scan** is running. A scan that starts cancels
  a running check.
- A **completed** scan clears the report, since the library now matches the disk
  as it was listed. The timer resets from that moment.
- A **canceled** scan leaves new files unapplied, so a check runs straight
  after it.

The report is held in memory only. It describes the disk at `checkedAt`, and
the next launch checks again.

### Showing it

The view shows, for example: _12 new · 3 changed · 1 gone — checked 4 min ago_,
with _Scan now_ and _Check now_. Each count expands into its paths. An
unreachable root is shown above the counts, in the warning style the toolbar
uses for an unreachable output device.

## Missing tracks in the playlist

### Marking

The renderer takes the set of missing ids from the health report and marks every
row whose track is in it with a `link_off` badge and a tooltip _File missing
since …_. That covers:

- playlist rows
- history rows
- the main deck, when the track on air went missing while it played. The deck
  keeps the whole file in memory, so the airing finishes.

No `missing` field is added to `Track` or to the playlist snapshot. The queued
`Track` values are copies taken when they were queued, and would go stale after
the next scan. One set that is rebuilt with the report cannot.

### Advancement

The engine receives the missing ids from the service, as it already receives
cache membership (`on_missing_state(ids)`, next to `on_cache_state`). Then:

- `plan()` treats a missing item as **never playable**, including in
  `Plan::Fallback` during a cold start. Today the fallback plays the head of the
  playlist blindly.
- An item that advancement **passes** over because it is missing is dropped
  from the playlist, not left queued the way an uncached one is. An uncached
  item waits for the share to come back. A missing one has nothing to wait for.
- An explicit _Play_ on a missing item is refused with an error toast, and the
  item stays where it is.
- The prefetch window already leaves missing rows out.

A purged item is removed from the playlist by the purge itself (see above), so
the existing _purged row means load failed_ path remains only as a guard.

## Notification

- The **Settings button** in the toolbar shows a dot with a count while anything
  needs attention.
- The **Library health** tab in _Settings_ shows the same count.

The count is:

| counted                                                    | weight   |
| ---------------------------------------------------------- | -------- |
| missing tracks, unless the `missing` dismissal covers them | 1 if any |
| exact groups not dismissed                                 | 1 each   |
| possible groups not dismissed                              | 1 each   |
| a check with any change, not dismissed                     | 1        |
| unreachable library paths                                  | 1 each   |

Missing tracks count once, not per track: removing a library path should not
put _800_ on the button. An unreachable library path cannot be dismissed,
because it clears itself once the share is back.

## View layout

A new _Library health_ tab in _Settings_, between _Library Sync_ and _Now
Playing_. The missing-tracks block in _Library Sync_ is replaced by one line
linking to it.

```text
┌ Library health ─────────────────────────────────────────────┐
│ Disk changes                                                │
│   ⚠ /Volumes/radio/music is unreachable                     │
│   12 new · 3 changed · 1 gone — checked 4 min ago           │
│   [Scan now] [Check now] [Dismiss]                          │
│                                                             │
│ Missing tracks (5)                         [Dismiss]        │
│   ☐ Title — Artist   /old/path.mp3   2 d ago   ✂ ▶12  ≡     │
│   ☐ ▸ No longer under a library path (812)                  │
│   [Purge selected] [Purge all]                              │
│                                                             │
│ Duplicates                                                  │
│   Exact (2)                                                 │
│   ▾ Title — Artist                          [Dismiss]       │
│       /a/Title.mp3   music   3:34   ✂ ▶12   [⋯]             │
│       /b/Title.mp3   music   3:34   ✂ ▶12   [⋯]             │
│   Possible (1) — same artist and title, may differ          │
│   ▸ Title — Artist                          [Dismiss]       │
│   Still checking 140 tracks…                                │
└─────────────────────────────────────────────────────────────┘
✂ has cue points   ▶ play count   ≡ queued   [⋯] row menu
```

Sections with nothing to report collapse to one _No issues_ line.

## Accepted limits

- **Deleting a duplicate loses its play count.** Merging was declined.
- **A possible group can be wrong.** Two different songs with the same artist
  and title get grouped. _Dismiss_ is the answer.
- **Changes land up to one interval late** on every root, local ones included.
- **A check costs a stat per file.** On a slow share with a very large library,
  the operator can lengthen the interval or set it to `0`.
- **The check does not see a changed file whose mtime was preserved.** Neither
  does the scan. See _Swapped by rename_ in
  [track-identity.md](./track-identity.md#accepted-limits).
- **History is not rewritten** when a track is purged.

## Increments

Each increment is one PR and leaves the app shippable.

| #   | increment                                                                                                                                                                                                                                                                            | depends on |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------- |
| 1   | **Shared listing rules** (refactor). Move the root listing, `should_rescan` and the gone rule out of `scan_all` into `library/listing.rs`. `scanner.rs` tests pass **unmodified**.                                                                                                   | —          |
| 2   | **Health report.** `health.rs` with missing, exact and possible groups; `library_health` command and `library-health` event and their rebuild triggers; `purge_tracks(ids)`; migration step 2 with `health_dismissals`. Rust tests for grouping, normalisation and dismissal expiry. | —          |
| 3   | **Library check.** `check.rs` worker, launch and timer runs, `tuning.library.checkIntervalMin`, the scan interlock. Rust tests for new, changed, gone, unreachable and partial against the index.                                                                                    | 1, 2       |
| 4   | **Missing tracks in the playlist.** `on_missing_state`, skip-and-drop in `plan()` including the fallback, purge removing items; `link_off` badges in playlist, history and deck. Engine tests plus Vitest.                                                                           | 2          |
| 5   | **Library health view.** The Settings tab, lists, selective purge, dismiss and undo, the badges on the button and the tab; _Library Sync_ loses its missing block. Vitest for the badge count and the list actions.                                                                  | 2, 3, 4    |
| 6   | **Locate…** `relocate_track` and its row action.                                                                                                                                                                                                                                     | 5          |

Increments 1 to 5 deliver #376. Increment 6 is its optional item, and a watcher
stays out unless the timer proves too slow in use.

Increment 1 goes first, per the rule of landing refactors before the features
that need them: the scanner suite pins the prune guard, and the check must
inherit that guard rather than restate it.
