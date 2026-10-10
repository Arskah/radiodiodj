# Library health — missing and hidden tracks, duplicates, unreadable files, bad durations and disk changes

The _Library_ tab of _Settings_ tells the operator when the library needs
attention, and lets them act on it. It reports five things, and lists a sixth:

1. **Disk changes** — files added, changed or removed since the last scan.
2. **Unreadable tracks** — files the analysis pass could not decode.
3. **Bad durations** — files whose tags give a length the audio does not have.
4. **Missing tracks** — tracks whose file a scan could not find, one by one.
5. **Duplicates** — exact copies, and tracks that look like the same song.
6. **Hidden tracks** — tracks an admin took out of the library, to put back.

A count on the Settings button says when any of them needs attention.

Implements [#376](https://github.com/Arskah/radiodiodj/issues/376). It builds on
the identity model in [track-identity.md](./track-identity.md). The library as a
whole is described in [library.md](./library.md).

Three rules hold throughout:

- **The app never touches audio files.** It does not delete, move or rename
  them. Every fix to the disk is made by the operator, in the file manager.
- **Nothing here changes the library by itself,** unless the operator asks it
  to. A check reports, a scan applies, and only the operator's _Purge_ deletes
  rows. _Scan when files change_ is the one opt-out, and it starts a scan —
  never a delete. See [Scanning by itself](#scanning-by-itself).
- **An unreachable library path is reported as unreachable,** never as a folder
  full of gone files.

## Where it lives

Library health sits in _Settings → Library_, under the library paths, the
settings that govern a scan — whether a check may start one, how often it looks,
and tag writing — and _Scan Library Now_ itself. The paths, the scan, what the
scan is allowed to do and what it left behind are in one place, rather than split
across _Advanced_.

```text
┌ Library ─────────────────────────────────────────────────────┐
│ Music / Commercials / Jingles paths                          │
│ Scan when files change                                  (•)  │
│ Library check interval (minutes)                      [ 15 ] │
│                                           [Scan Library Now] │
│ Write edits to file tags                                ( )  │
│ Tag write timeout (seconds)                           [ 30 ] │
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
│ Bad durations (1)                                            │
│   Title / Artist  …/vbr.mp3  tag says 24:37, audio is 3:58 F │
│                                                              │
│ Missing tracks (5)                                [Dismiss]  │
│   ☐ Title / Artist   …/old/path.mp3   2 d ago   ◇ ▶12  ≡     │
│   ☐ ▸ No longer under a library path (812)                   │
│   [Purge selected (1)…] [Purge all…]                         │
│                                                              │
│ ▸ Hidden tracks (3)                            [Restore all] │
│     Title / Artist — Album  /comp/Title.mp3  music  2 d ago R│
│                                                              │
│ Duplicates                                                   │
│   Still checking 140 tracks for exact copies…                │
│   Exact copies (2)                                           │
│   ▾ Title — Artist   2 copies                     [Dismiss]  │
│       Title / Artist  /a/Title.mp3  music  3:34  ▶12 F E C H │
│       Title / Artist  /b/Title.mp3  music  3:34  ▶0  F E C H │
│   Possible duplicates (1)                                    │
│   ▸ Title — Artist   2 copies   Dismissed   [Undo dismiss]   │
└──────────────────────────────────────────────────────────────┘
◇ has cue points   ▶ play count   ≡ in the playlist
F Show in folder   E Edit metadata…   C Cue points…
H Hide from library   R Restore to library
```

A section with nothing to report says _No issues_. Unreadable tracks, bad
durations, hidden tracks and tag writes are shown only when there is something to list. Disk changes says _Not
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
| bad durations, unless dismissed              | 1 if any |

Missing tracks count once, however many there are, so removing a library path
does not put _800_ on the button; bad durations count once for the same reason.
A library path this computer has no folder for
([library.md](./library.md#where-a-library-path-is)) is reported the same way,
under a name that says so in place of a folder. An unreachable path cannot be
dismissed. It clears itself when the share is
back.

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
  [baseline reset](./database.md#baseline-resets)). That scan answers the same
  question.
- **On a timer:** _Settings → Library → Library check interval (minutes)_,
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

While a check runs, the section says _Checking library paths…_ and _Check now_
is disabled. When it finishes the summary flashes, even if the result is the same
as before, and hovering it shows the exact time of the check.

The report is held in memory only. The next launch checks again.

### Scanning by itself

By default the check never starts a scan: _Scan Library Now_, below the
settings, is the operator's button. _Settings → Library → **Scan when files
change**_ (`tuning.library.scanOnChanges`) hands it that button, so music copied
onto a library path from another computer is picked up without anyone walking to
the studio machine. This starts the scan from elsewhere; it does not move the
work. Running the scan on another machine is
[external-library.md](./external-library.md).

**An automatic scan only ever adds and updates.** It never marks a track missing
on the strength of a listing nobody watched — a share that came back as an empty
directory, or a root the operator emptied, would otherwise retire a whole library
unattended. The one exception is a file that **moved**: a gone row whose
fingerprint matches audio arriving elsewhere in the same scan is retired with it,
because committing the new file without retiring the old one mints a duplicate of
a file that merely moved. Everything else stays listed under _Disk changes_ until
the operator scans. In the code this is `Missing::OnlyMoved` (`library/scanner.rs`).

Five things have to line up before a check starts a scan, and each is a way it
stays out of the operator's way:

- **There is something to read.** New or changed files. Gone and unrooted rows
  are reported as always, but they never start a scan by themselves.
- **The disk has settled.** Two consecutive checks must agree on the **settle
  key** (`Checked::settle`), which hashes every new and changed file's path,
  modification time and size. Comparing paths alone would read the same all the
  way through a long copy — the key changes while the bytes are still moving, so
  a half-written file is never scanned. The cost is one extra interval: half an
  hour on the default fifteen.
- **Every library path was readable.** Anything `unreachable` or `partial` and
  the check reports as usual and starts nothing — a share going up and down must
  not drive a scan loop.
- **The evidence is new.** A scan that leaves the disk reading exactly as it did
  is not repeated on the same settle key. Without this, one file no tag reader
  can parse — reported as new forever, since a failed parse writes no row — would
  rescan the share every two intervals for good.
- **The operator has not said otherwise.** A dismissed _Disk changes_ report is
  them saying _not these_, and a cancelled scan stands until they scan again.
  Neither is overruled. (Both live in memory, so a relaunch forgets them.)

Every scan transition clears the remembered settle key, so the two checks that
agree are always two checks since the last scan. An automatic scan runs the
analysis pass exactly as the button does, but leaves the tag backfill alone: a
cancelled backfill was cancelled deliberately. With the check interval at `0`
there is no timer, and so no automatic scan either.

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

## Hidden tracks

A track an admin has taken out of the library without touching its file — the
copy of a song that is also on a compilation, say. _Hide from library_ is on a
library row's menu (and on a selection's, for all of it) and on every row of a
duplicate group. It needs admin mode, asks nothing, and is undone from the list
here: _Restore to library_ on a row, or _Restore all_.

A hidden track is **still a present track to the scan.** Its row keeps its path,
so the file is never read as a new one, and a rescan neither restores it nor
disturbs it. Everywhere else it is out:

- not in a library tab, search, the statistics or _Playtime_
- never picked by the auto-playlist, from the music library or from a saved
  playlist chosen as its source
- not in a duplicate group, an unreadable or a bad-duration list — hiding one of
  two copies ends that finding
- **unplayable where it already is**, exactly as a missing track is: a queued
  item is badged, skipped and dropped by advancement, and a saved playlist entry
  bound to it counts as missing and offers _Find in library…_. See
  [In the playlist, history and deck](#in-the-playlist-history-and-deck). The
  badge is `visibility_off` in place of `link_off`. A hidden track that is on
  air finishes.

Nothing is lost: cue points, play count, edits and measurements stay on the row,
and the analysis pass still measures it, so a restored track is ready at once.
Hidden tracks are not in the attention count and cannot be purged.

Hiding is a mark on **that row**, not on the audio:

- A hidden track whose **file then disappears** goes missing like any other. It
  moves to _Missing tracks_, where it can be purged; if the file comes back it
  reattaches still hidden.
- A **new copy** of the same audio is a new track, and starts visible. It
  copies its twin's operator state as every duplicate does, but not the mark.

## Unreadable tracks

A track whose file the analysis pass read but could not decode: a truncated or
corrupt file, or a codec symphonia has no reader for (for example MP3 audio in a
RIFF/WAVE container). Each row shows the error and has _Show in folder_.

The failure is stored on the track (`analysis_error`, `analysis_failed_at`), and
the pass skips the track from then on. A scan that sees the file change (a new
modification time or content type), or reattaches it at a new path, clears the
failure, so a replaced file is tried again. A failure is recorded against the
modification time the pass read the file at, so one that arrives after such a
scan is dropped rather than stamped on the replacement. A file that could not be
_read_, such as one on a share that dropped out, is not recorded: the pass tries
it again on its next run.

To fix one, replace the file with a good copy and scan, or delete it, scan, and
purge the missing track. Unreadable tracks are not in the attention count, and
cannot be dismissed.

## Bad durations

A track whose tags give a length the audio does not have. The usual cause is a
VBR MP3 with no Xing header: a tag reader then reports the file size divided by
the first frame's bitrate, which can be several times the real length. Each row
shows both lengths, in file time, and has _Show in folder_.

Nothing is wrong with how the library plays these. The analysis pass measures
the length and `tracks.duration` holds that
([library.md](./library.md#the-analysis-pass)); `tracks.tag_duration` keeps what
the tags said, and a track is listed when the two differ by more than a second
(`BAD_DURATION_TOLERANCE_S`) **and** by more than 2 % of the measured length
(`BAD_DURATION_TOLERANCE_RATIO`). Encoder delay and padding put an honest tag
tens of milliseconds out; seconds alone would list every long recording whose
tag rounds differently, and a percentage alone every short jingle. Both are
constants, not settings: no health finding has a knob. It is a warning because the file is still
wrong for every other program that reads it, and a header that gets the length
this wrong is usually one a decoder cannot seek by either.

A track whose tags carry no length and whose decode counted no audio is listed
too, as having no length at all.

A track is only listed once the pass has measured it. To fix one, repair the
header or re-encode the file and scan: the scan re-reads the tags, and the track
leaves the list when they agree with the audio. Bad durations are in the
attention count, once however many there are, until the list is
[dismissed](#dismissing).

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
usually the same song in another encoding or edit, or on another release. In
practice they almost always are the same recording, so the operator is told and
decides.

Artist and title are compared after:

- lower-casing
- turning every run of anything but letters and digits into one space

Words are kept, so `Song (Remix)` and `Song` stay apart, while `Don't Stop!!`
and `DON'T STOP` match.

The album is not compared. The same song on its album, a single and a
compilation is one group, and the copies not wanted on air are
[hidden](#hidden-tracks).

#### Typos

Two spellings are also grouped when one field is the same and the other is one
typo away — the same artist under two close titles, or the same title under two
close artists. One typo is either of:

- **one edit in one word** of four letters or more, every other word the same.
  An edit is a letter added, dropped or changed, or two neighbours swapped.
- **one edit once the spaces are taken out**, when the words are split
  differently and the name is five letters or more without them. A whole word
  added or dropped is not a typo.

| titles                               | grouped | why                     |
| ------------------------------------ | ------- | ----------------------- |
| `Possesion`, `Possession`            | yes     | one letter              |
| `Kraftwerk`, `Kraftwrek`             | yes     | a swap                  |
| `Dope Man`, `Dopeman`                | yes     | the word break          |
| `Believe`, `I Believe`               | no      | a whole word            |
| `Part I`, `Part II`                  | no      | the word is too short   |
| `Club Mix`, `Club Remix`             | no      | the word is too short   |
| `Symphony 15`, `Symphony 16`         | no      | the edit is in a number |
| `Fussin and Fightin`, `Fussing and…` | no      | two words differ        |
| `Humppatauti`, `Humppatähti`         | no      | two edits               |

Typos chain: three spellings each one edit from the next are one group.

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

- **An unwanted copy whose file should stay** — the same song on a compilation:
  _Hide from library_. The group goes, the file and the row stay. See
  [Hidden tracks](#hidden-tracks).
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

| finding        | remembered                 | counts again when                  |
| -------------- | -------------------------- | ---------------------------------- |
| exact group    | its track ids              | a copy is added or leaves          |
| possible group | its track ids              | a copy is added or leaves          |
| missing tracks | the newest `missing_since` | another track goes missing         |
| bad durations  | each track and its lengths | a track joins, or its lengths move |
| disk changes   | what the check found       | a check finds something different  |

A repaired file leaving the bad-duration list does not light it again: the rest
were already seen.

Group, missing-track and bad-duration dismissals are stored in the `health_dismissals` table,
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
  hidden           [HiddenTrack]     id, title, artist, album, path,
                                     contentType, hiddenAt
  exact            [DuplicateGroup]  key, dismissed,
                                     tracks[{track, path, contentType}]
  possible         [DuplicateGroup]
  unhashed         count             present tracks waiting to be fingerprinted
  unreadable       [UnreadableTrack] track, path, contentType, error, failedAt
  badDurations     [BadDurationTrack] track, path, contentType, tagDuration,
                                     measuredDuration
  badDurationsDismissed bool
  check            CheckReport?      checkedAt, new, changed, gone, unrooted,
                                     unreachable, partial
  checkDismissed   bool
  checking         bool              a library check is running now
  tagWriteFailures [TagWriteFailure] id, title, artist, path, error, at
```

The renderer loads it with `library_health` and replaces it on every
`library-health` event. The backend rebuilds it:

- when a scan starts, finishes, is canceled or fails
- when the analysis pass starts or finishes
- after a metadata edit, since artist and title decide possible duplicates
- after a library path is added or removed
- after a purge, a hide or restore, a dismissal, and every check
- when a check starts, and when one ends without a report (canceled by a scan,
  or failed); only the `checking` flag changes then
- when the tag write failures change

The last event delivered is always the last report stored. Two refreshes can run
at once — the scan thread, the tag writer's listener and four commands on the
blocking pool all reach `refresh` — and whichever stores second must also emit
second, or the renderer mirrors a report that has already been replaced and
nothing corrects it. Each store stamps a sequence under the report lock, and the
emit is skipped if a higher sequence has already gone out. The report lock is not
held across the emit: `library-health` has a backend listener too, which Tauri
runs on the emitting thread.

The renderer works out whether a missing or hidden track is queued from the
playlist it mirrors. That keeps the report independent of the playlist, which itself
depends on the report.

| command                       | does                                                                                                     |
| ----------------------------- | -------------------------------------------------------------------------------------------------------- |
| `library_health`              | returns the current report                                                                               |
| `purge_tracks(ids)`           | see [Purge](#purge)                                                                                      |
| `hide_tracks(ids)`            | hides present tracks; missing and already hidden ids are skipped. Returns how many it hid                |
| `unhide_tracks(ids)`          | restores hidden tracks. Returns how many it restored                                                     |
| `health_dismiss(kind, key)`   | `kind` is `exact`, `possible`, `missing`, `duration` or `check`; `key` is the group key, empty otherwise |
| `health_undismiss(kind, key)` | undoes a dismissal                                                                                       |
| `library_check_now`           | asks the worker for a check                                                                              |
| `retry_tag_write(id)`         | queues a failed tag write again                                                                          |
| `dismiss_tag_write(id)`       | drops a failed tag write from the list                                                                   |

Migration step 2 added the dismissals table, and step 13 rebuilt it to admit
`duration`:

```sql
CREATE TABLE health_dismissals (
  kind  TEXT NOT NULL CHECK (kind IN ('exact', 'possible', 'missing', 'duration')),
  key   TEXT NOT NULL,   -- fingerprint, normalised "artist\x1falbum\x1ftitle", or ''
  value TEXT NOT NULL,   -- sorted ids, the newest missing_since, or id:tagMs:audioMs,…
  PRIMARY KEY (kind, key)
);
```

Migration step 16 added `tracks.hidden_at` (unix ms, `NULL` when not hidden). It
is operator work, so it stays out of `UPSERT_TRACK_SQL`. `missing_since` remains
the only thing the scan, the path index and purge read; a query that offers
tracks to an operator or to the auto-playlist asks for both to be `NULL`.

## Not built

**Locate…** would reattach a missing track to a file the operator picks, for
audio that changed so that no fingerprint can match. It would refuse unless the
track is missing, refuse a file that is already a present track, refuse a file
outside every library path, and refuse during a scan. It would set the path,
mtime, content type and a fresh fingerprint without re-reading tags. Until then,
purge the old track and keep the new one.

A **filesystem watcher** is not built either. The timer covers every share, and
a watcher would only make local paths report sooner.

A **scan request file** would force a scan on demand, where _Scan when files
change_ only reacts to the disk and reacts an interval late. The operator names
a directory on the share (`library.scanRequestPath`, empty by default, never a
library path); the check's worker, which already wakes once a minute, stats one
file there and starts a scan when its mtime is newer than the last one honoured.
That mtime is remembered in `config.json`, so a relaunch does not re-fire a stale
request and the app needs no write permission on it, and a status file written
on every scan transition is the reply. Two things it would have to say plainly:
it bypasses admin mode, since its authority is write access to that directory
and nothing else ([admin-mode.md](./admin-mode.md)), and a request mid-show is
honoured. Worth building only if waiting an interval turns out to be the
complaint.

Two other triggers were considered and refused. An **HTTP listener** in the
studio app is an inbound port on the on-air machine, a token to manage, firewall
and NAT, to save a minute over the above. **Scheduled nightly scans** run at a
fixed hour whether anything changed or not; scanning because the disk moved is
the same idea with a better condition, and a schedule is worth adding only as a
quiet-hours window around it.

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

**The check may start a scan, but only when told to.** Nothing in library health
changes the library by itself, and _Scan when files change_ is the operator's
opt-out of that rule — which is why it is off by default. It was chosen over
every remote trigger because the machine was already looking: the check knew
which files were new, and did nothing with the answer until someone pressed a
button. Copying music onto the share became the trigger, with nothing to
install, authenticate or open on the other computer.

**Hidden is a second mark, not a kind of missing.** Deleting the file was first
the only way to drop a duplicate, which fails for the case that turned out to be
the common one: a compilation the station wants kept whole on disk. Reusing
`missing_since` for it would have told the scan the path was free, and the next
scan would have inserted the file again. So `hidden_at` is its own column: the
scan keeps treating the row as present, and only what is offered to play reads
it. A path exclusion list would be simpler still, but breaks the moment a file
moves.

**A hidden track is unplayable, not merely unlisted.** The playlist already has
one rule for a track that must not air — the missing one — with its badge, its
skip and its drop. Hidden ids ride the same list, so there is no third state for
an operator to learn.

**No merging.** Folding one track into another means rewriting queued items,
history and play counts. Keeping one copy covers the real case.

**Possible duplicates from the start.** Tracks that share an artist and a title
are nearly always the same recording. The notice is worth more than the
occasional wrong group, which _Dismiss_ handles.

**The album is not part of the match.** It was, once: artist and title alone
grouped every _Intro_, _Outro_ and _Skit_ an artist ever released, and nothing
could be done about a group but dismiss it. That also hid the common case, the
same song on an album and on a compilation, which is exactly the copy a station
does not want aired twice as often. Since a track can be
[hidden](#hidden-tracks) the group has an answer that leaves the file alone, so
the wider match is worth its _Intro_ groups, which are dismissed once.

**One typo, and only where a typo is likely.** A wider budget finds more, and
most of what it adds is different songs: `Part I` and `Part II`, `Mix` and
`Remix`, `Believe` and `I Believe`, `Life` and `Time`. So a short word is never
a typo, a number is never one, an added word is never one, and a second edit is
a different title. Only one field may differ, because
with both loose every pair of tracks in the library is a candidate; a typo in
the artist and the title of the same file is rare enough to miss.

**A timer, not a watcher.** FSEvents and inotify do not report changes made by
other SMB or NFS clients, which is where this library lives.

**Report, never auto-scan.** A scan changes what the auto-playlist can pick. It
should not happen mid-show without the operator asking.

**Missing tracks are dropped, not retried.** The outage retry schedule waits for
a share to come back. A track that a completed scan could not find is not coming
back on its own.

**Shared rules, not copied ones.** The check calls the scan's listing and prune
rules, so the scanner's tests for the unreachable-share guard cover it too.

**Dismissals in the database.** They name track ids. A baseline reset replaces
the database and its ids together, whereas `session.json` would need scrubbing.
