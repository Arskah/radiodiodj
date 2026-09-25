# Library — how RadiodioDJ knows its audio

The library is everything the station can put on air: every audio file under the
configured library paths, indexed with its tags, its cue points and its play
count. This page covers the whole feature and links to the detailed designs.

| topic                                         | detail                                         |
| --------------------------------------------- | ---------------------------------------------- |
| identity, moves, missing tracks, fingerprints | [track-identity.md](./track-identity.md)       |
| missing tracks, duplicates, the library check | [library-health.md](./library-health.md)       |
| schema and migrations                         | [database.md](./database.md)                   |
| cue points stored on a track                  | [cue-points.md](./cue-points.md)               |
| automatic cue points                          | [cue-auto-analysis.md](./cue-auto-analysis.md) |

The app **reads** audio files and never writes, moves or deletes them. Everything
the operator adds to a track (edited tags, cue points, play count) lives in the
library database.

## Library paths and content types

Every track has one **content type**: `music`, `jingle` or `commercial`. The
content type comes from the library path a file was found under, not from its
tags.

_Settings → Library_ holds three lists of folders, one per content type. Adding
a folder stores its canonical path in `config.json`, and removing one only
changes the list: its tracks go missing on the next scan, and come back intact if
the folder is added again.

A folder may hold subfolders; the scan recurses into them. If one file is under
two library paths, it is indexed once, under the first path in the order music,
commercials, jingles.

The three content types make up three libraries: the **Music**, **Jingle** and
**Commercial libraries**. They matter in two places. The library panel shows one
at a time, and the auto-playlist draws music and interleaves jingles and
commercials (see [Auto-playlist](#auto-playlist)).

## Scanning

_Settings → Library → Scan Library Now_ brings the library in line with the
disk. The Settings window closes, and a bar at the bottom of the main window
shows progress with a _Cancel_ button. Only one scan runs at a time.

A scan:

1. **Lists** every library path. Hidden files and folders (a leading `.`) are
   skipped, and symbolic links are not followed. Only files with an audio
   extension are kept: `mp3`, `flac`, `wav`, `ogg`, `oga`, `aac`, `m4a`,
   `opus`, `webm`, `aiff`, `aif`, `mka`, `mp2`. WMA is not supported; convert
   such files first (see [Unsupported formats](#unsupported-formats)).
2. **Inspects** each file on up to four threads at once, which hides the latency
   of a network share without flooding it.
   - A known file whose modification time and content type are unchanged is
     skipped without being opened. This is the **delta cache**, and it is what
     keeps a rescan fast.
   - Anything else has its tags read with `lofty`: title, artist, album, album
     artist, genre, year, BPM, track and disc number with their totals, ISRC,
     musical key, comment, duration, sample rate, bitrate, format. A file
     without a title is named after its file name; one without an artist or
     album gets `Unknown`.
   - The comment is the tag's _undescribed_ one. lofty maps every ID3v2 `COMM`
     frame onto the same key, so the first one is often iTunes' `iTunNORM`
     volume data rather than anything an operator wrote.
   - Only album artist joins the search index, beside title, artist, album and
     genre. Numbers, keys and comments would swamp a prefix search — see
     [library-search.md](./library-search.md).
   - A new file is also fingerprinted, except on the first scan into an empty
     library.
   - Every read stamps the row with `scanner::TAG_READ_VERSION`, the generation
     of the tag read. A row at an older generation is filled in after launch by
     the **tag backfill** below, since a scan alone would never reopen it.
3. **Reconciles** everything in one database transaction: changed files are
   re-tagged, moved files are **reattached** to their old track, copies become
   **duplicates**, files that are gone make their track **missing**, and the rest
   are inserted. A scan never deletes a track. The rules, and the guards that
   stop an unreachable share from looking empty, are in
   [track-identity.md](./track-identity.md#reconciling-a-scan).

When the scan finishes, the bar reports what it did, for example _Scan complete
— 2 986 tracks (12 new/updated, 3 moved, 1 missing)_. The result stays until the
operator dismisses it or starts another scan. A canceled or failed scan reports
that instead. A canceled scan keeps its updates but leaves new files for the
next complete scan.

Scans are started by the operator. Between scans, the **library check** notices
new, changed and gone files without changing anything; see
[library-health.md](./library-health.md#disk-changes).

### The analysis pass

Every launch and every completed scan starts the **analysis pass**
(`library/waveform_scan.rs`). It works through present tracks missing any of the
things below, decoding on several threads (the core count less two, kept between
2 and 8) so playback and the UI stay responsive. A second bar under the scan bar
shows its progress.

One decode yields all of them. `waveform::analyze` walks the samples once and
measures the curve, the loudness and the automatic-cue levels together, so a
track missing only one of them costs no more than a track missing all four.

- The **waveform** is the amplitude curve drawn behind the deck's seek bar and in
  the cue editor. It needs a full decode, which is why it is not part of the
  scan.
- The **fingerprint** identifies the audio independently of path and tags. It is
  computed from the bytes already read for the waveform, or from the first
  megabyte of the file otherwise. A track whose stored fingerprint predates the
  current algorithm is picked up by this pass too, so a version bump costs one
  extra read per track and nothing else. See
  [track-identity.md](./track-identity.md#fingerprint).
- The **loudness** is the EBU R128 measurement the deck plays a track back at.
  Measured here, never read from tags. See [audio.md](./audio.md#replaygain).
- The **automatic cue points** are the derived trio, and the **level envelope**
  the trio is derived from — the decode reduced to one byte per window, so a
  later threshold change can re-derive a track's markers without reading the
  file again. A track analysed before the envelope existed is picked up by this
  pass for the envelope alone; its markers are left exactly where they are, since
  re-deriving them would apply today's thresholds to a track analysed under
  yesterday's. See
  [cue-auto-analysis.md](./cue-auto-analysis.md).

A file that fails to decode is recorded on its track and skipped until a scan
sees the file change; it is listed under [Unreadable
tracks](./library-health.md#unreadable-tracks). A file that could not be read is
skipped for the rest of the run only. Cancelling a scan cancels the pass too; the
next one picks up where it stopped.

### Tag backfill

Adding a tag-derived column leaves every existing row empty: the delta cache
skips a file whose modification time has not changed without opening it, and
there is no command that forces a full re-read. A background pass after launch
closes that gap, reading tags for every row written at an older
`TAG_READ_VERSION` and filling the new columns in.

It is separate from the waveform pass on purpose. That one skips a track whose
audio failed to decode — but a file whose audio is broken usually still has
readable tags, and a tag read that failed there would block the track's
waveform, loudness and cue points for good.

The pass never disturbs what it did not read: the modification time stands, so
the delta cache is unaffected; a recorded analysis failure, the level envelope
and the cue points are left alone; and a column the operator edited keeps the
operator's value. Each row is written only while its modification time still
matches what the queue saw, so a scan running alongside it always wins.

Adding another tag field later is a schema step plus a bump of
`TAG_READ_VERSION` — the whole library requeues and fills itself in.

## Tracks

A track is one row in the library database. Its id never changes, so the
playlist, the history and the saved session refer to it by id.

| part                   | source            | survives a rescan                |
| ---------------------- | ----------------- | -------------------------------- |
| path, content type     | where the file is | follows the file                 |
| tags, duration, format | the file          | re-read when the file changes    |
| tags edited in the app | the operator      | always                           |
| cue points             | the operator      | always                           |
| play count             | airings           | always                           |
| waveform, fingerprint  | the analysis pass | always                           |
| loudness               | the analysis pass | always, including a changed file |
| automatic cue points   | the analysis pass | re-derived when the file changes |

The automatic cue points are the only measurement a changed file gives up, and
only because the rescan drops the level envelope they are derived from. The
waveform and the loudness are kept: nothing clears them, so a re-encoded file
keeps the curve and the ReplayGain of the audio it replaced until something
else queues it for the pass.

A track remembers which tag fields the operator edited (`edited_fields`). When
the file changes, the scan re-reads it but keeps those fields. The other fields
still follow the file.

The **play count** goes up by one each time the track is put on air.

A track whose file is gone is **missing**. It is hidden from the library panel,
search, the statistics and the auto-playlist, but it keeps its id, cue points
and play count until the operator purges it. The same track comes back if the
file reappears, at the same path or, by fingerprint, at a new one. See
[track-identity.md](./track-identity.md#missing-not-deleted) and
[library-health.md](./library-health.md#missing-tracks).

## The library panel

The library panel shows one library at a time: _Music_,
_Commercials_ or _Jingles_.

- **Search** matches title, artist, album, album artist and genre. Each word is a prefix, so
  `beat abb` finds _Abbey Road_ by _The Beatles_. The search runs a quarter of a
  second after typing stops, and shows at most 200 tracks, best match first.
  It does not match inside a word or forgive a typo — see
  [library-search.md](./library-search.md) for why, and the fuzzy matching
  planned to fix it.
- **Sort** by number, title, artist, album or plays by clicking a column header;
  click again to reverse. Text sorts ignore case. With no sort and no search,
  the list is ordered by artist, album and title.
- **#** is the track's number on its record, and sorting by it is _album order_:
  album, then disc, then track. A bare track-number sort would interleave every
  album's track 1, which is no use for reading a record in order. A track with
  no number sorts last whichever way the arrow points.
- **Time** is the track's [air time](./cue-points.md#air-time): what reaches air
  once its cue points apply. A trimmed track shows its time in the cue colour.
- **Hovering** a row shows album, album artist, track and disc position, genre,
  year, duration (the file length too when cue points trim it), BPM, musical
  key, format, bitrate, sample rate, ISRC, plays and the comment. The first six
  rows read _Unknown_ when a file lacks them; the fields most files never carry
  are left out of the tooltip entirely rather than filling it with _Unknown_.
  A long comment is shortened.

On each row:

| action                  | how                                                       |
| ----------------------- | --------------------------------------------------------- |
| add to the playlist     | double-click, or the `+` button                           |
| preview on the cue deck | the headphones button (only with a cue device configured) |
| edit metadata           | the pencil button                                         |
| everything else         | right-click, the menu key, Shift+F10 or Ctrl+Enter        |

The row menu offers _Add to playlist_, _Add as next_, _Preview on cue deck_,
_Edit metadata…_, _Cue points…_, _Show in folder_ and, set apart and marked as
dangerous, _Play now (on air)_. Play now is never the item under the cursor when
the menu opens, so a stray click cannot reach air.

_Show in folder_ opens the platform file manager at the file. It takes a track
id rather than a path, so the renderer can only reveal files the library already
knows.

## Editing a track

**Edit metadata…** opens a form for title, artist, album, album artist, track
and disc position, genre, year, musical key and comment. The title is required,
the year must be a whole number from 1900 to 2100, and each position number must
be a whole number from 1 to 9999. Saving updates the database, and the change
shows in the library, the playlist and the deck at once. Artist and title
changes can also make or break a
[possible duplicate](./library-health.md#possible-duplicates).

A position is edited as a pair — _3 of 12_ — because one tag frame carries both
halves, and writing the number without its total would leave the record's length
behind.

The **ISRC** cannot be edited. It identifies the recording in the rights
registry, so the file is the authority and a typo would be silently wrong in
anything reported from the airing log. It is shown on hover and read from the
file on every scan.

Only a field whose value actually changes is marked as edited. The form marks
those fields, and **Revert to file tags** discards them: it reads the file's tags
again right away, because a rescan skips a file that has not changed.

**Write edits to file tags** (_Settings → Library_, off by default) also writes
the edit into the file, so it survives moving the file to another library and a
database reset. The write runs on a background worker, never inside a scan or
playback, and never edits the file in place:

1. Read the whole file into memory and set the tags there with `lofty`. The
   comment replaces only the file's own, undescribed comment — an
   iTunes-processed file keeps frames like `iTunSMPB`, its gapless-playback
   data, which a plain overwrite of the comment would destroy.
2. Fingerprint the tagged copy. If the fingerprint differs from the stored one,
   stop, because the write would cost the track its identity.
3. Write a sibling `<name>.rdj-tmp`, flush it, copy the file's permissions, and
   rename it over the original.
4. Store the file's new mtime and clear the edited flags, so the next scan sees
   no change. An edit saved while the write was running keeps its flags and is
   written next.

In-place writes are avoided because lofty rewrites the whole file in place for
Ogg, ID3v2 and WAV/AIFF, and a dropped share connection would leave it
truncated. A read-only file is not written. A write that takes longer than
`tuning.library.tagWriteTimeoutSec` (default 30 s) is reported as failed and
left behind. A failure keeps the edit and its flags, and is listed under
[Library health](./library-health.md#tag-writes) until a retry succeeds or it is
dismissed. With the setting off, nothing is written to any file.

**Cue points…** opens the cue editor, which stores the track's radio edit; see
[cue-points.md](./cue-points.md).

In this app, "edit" always means metadata. Playback markers are always cue
points.

## Statistics

The toolbar shows the number of present tracks, distinct artists, and the total
**playtime** in hours. Playtime alone is file time, not air time.

## Cover art

A track's embedded picture is read from the file when the track is loaded on a
deck, and shown on the deck's disc. It is never stored in the database.

## Auto-playlist

When _Auto Mode_ is on, the playlist tops itself up from the library:

- **music** is picked at random
- **jingles** are picked at random, one every _N_ music tracks
- **commercials** are picked at random from the least-played ones, one every _M_
  music tracks, so every spot gets its airings

Tracks already in the playlist, and missing tracks, are never picked. The cadences and
buffer sizes are under _Settings → Advanced_. _+ Jingle_ and _+ Comm_ in the
playlist add one filler by the same rules.

## Library health

_Settings → Library_ also reports what needs attention: missing tracks, exact and
possible duplicates, and disk changes the library has not picked up. A count on
the Settings button says when there is something to look at. See
[library-health.md](./library-health.md).

## Unsupported formats

WMA (Windows Media Audio) files are not supported, and a scan skips them.
Convert them with [ffmpeg](https://ffmpeg.org/) first, e.g. to MP3:

```bash
ffmpeg -i track.wma -c:a libmp3lame -q:a 2 track.mp3
```

or a whole folder at once:

```bash
for f in *.wma; do ffmpeg -i "$f" -c:a libmp3lame -q:a 2 "${f%.wma}.mp3"; done
```

Tags are carried over. Delete or move the `.wma` originals afterwards.

## Where it is stored

| file            | holds                                                  |
| --------------- | ------------------------------------------------------ |
| `radiodiodj.db` | tracks, their operator work, and health dismissals     |
| `config.json`   | library paths and tuning, including the check interval |
| `session.json`  | the playlist and history, by track id                  |

They sit in the app data directory listed in `AGENTS.md`. The database uses
SQLite in WAL mode with an FTS5 index for search; its schema rules, backups and
pre-1.0 resets are in [database.md](./database.md).

## Code map

| area                               | where                                                          |
| ---------------------------------- | -------------------------------------------------------------- |
| listing and the changed/gone rules | `src-tauri/src/library/listing.rs`                             |
| scan and reconcile                 | `library/scanner.rs`, `library/scan_state.rs`, `Db::reconcile` |
| fingerprint                        | `library/fingerprint.rs`                                       |
| waveforms and fingerprints         | `library/waveform_scan.rs`                                     |
| tag backfill                       | `library/tag_backfill.rs`                                      |
| health report                      | `library/health.rs`                                            |
| library check                      | `library/check.rs`                                             |
| queries and schema                 | `library/db.rs`, `library/schema.sql`                          |
| search design                      | [library-search.md](./library-search.md)                       |
| auto-playlist selection            | `playlist/generate.rs`                                         |
| library panel                      | `src/features/library/LibraryPanel.svelte`                     |
| hover card                         | `src/features/track/TrackTooltip.svelte`                       |
| metadata editor                    | `src/features/track/MetadataOverlay.svelte`                    |
| tag write-back                     | `library/tag_write.rs`                                         |
| settings and health view           | `src/features/settings/`, `src/features/health/`               |
