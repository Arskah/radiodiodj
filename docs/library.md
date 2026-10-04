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
   extension are kept: `mp3`, `flac`, `wav`, `aiff`, `aif`, `ogg`, `oga`,
   `aac`, `m4a`, `mp2` — which is exactly what a deck can play. Anything else,
   WMA and Opus included, is skipped; convert it first (see [Unsupported
   formats](#unsupported-formats)).
2. **Inspects** each file on up to four threads at once, which hides the latency
   of a network share without flooding it.
   - A known file whose modification time and content type are unchanged is
     skipped without being opened. This is the **delta cache**, and it is what
     keeps a rescan fast.
   - Anything else has its tags read — see [Reading tags](#reading-tags):
     title, artist, album, album artist, genre, year, BPM, track and disc
     number with their totals, ISRC, musical key, comment, duration, sample
     rate, bitrate, format. A file without a title is named after its file
     name; one without an artist or album gets `Unknown`.
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
   - A **new file whose tags cannot be read** still enters the library, named
     after its file and with nothing the tags would have carried. Only if it
     fingerprinted: that proves the head came off the share and demuxed, so the
     tag read failed on what the file holds rather than on reaching it. A file
     the share would not deliver is left for the next scan instead. Kept out
     altogether it would have no row, so the library check would report it as
     new on every pass and — with _Scan when files change_ on — start a scan
     that failed the same way, for ever. A known track is never overwritten
     this way: a tag read that fails on a file already in the library leaves
     the row exactly as it is.
3. **Reconciles** everything in one database transaction: changed files are
   re-tagged, moved files are **reattached** to their old track, copies become
   **duplicates**, files that are gone make their track **missing**, and the rest
   are inserted. A scan never deletes a track. The rules, and the guards that
   stop an unreachable share from looking empty, are in
   [track-identity.md](./track-identity.md#reconciling-a-scan).

A file that brings its parser down rather than returning an error costs that
file and nothing else: it is logged, skipped, and the scan carries on to the
next one. A scan that cannot finish at all reports an error and leaves the bar
free for the next attempt — it never leaves the library unable to scan again.

When the scan finishes, the bar reports what it did, for example _Scan complete
— 2 986 tracks (12 new/updated, 3 moved, 1 missing)_. The result stays until the
operator dismisses it or starts another scan. A canceled or failed scan reports
that instead. A canceled scan keeps its updates but leaves new files for the
next complete scan.

Scans are started by the operator. Between scans, the **library check** notices
new, changed and gone files without changing anything; see
[library-health.md](./library-health.md#disk-changes). With _Scan when files
change_ on, that check starts a scan itself once the disk has settled — which is
how music copied onto a share from another computer reaches the library without
anyone at the studio machine. That scan only adds and updates: a track whose file
is gone is retired by the operator's scan, never by an automatic one. See
[Scanning by itself](./library-health.md#scanning-by-itself).

### Reading tags

Tags are read with **`lofty` first, and `symphonia` when lofty refuses the
file**.

lofty picks its reader from the **extension**. That is usually right and it
reads ID3v2 well, but it means a file whose contents are not what its name
claims is rejected outright — an `.ogg` whose first logical stream is Theora
rather than Vorbis (cover art muxed as a single-frame video, which some
converters emit) fails with `failed to parse Vorbis file`. Before the fallback
existed such a file never entered the library at all, and the library check
reported it as new on every pass.

symphonia identifies a file by **sniffing the bytes**, so it reads that Ogg
without trouble. It is the demuxer the app already uses to decode and
fingerprint every track, so nothing new is pulled in for it.

**Why lofty is first rather than symphonia.** Not because it reads more. A
survey of 300 files from a real library (the `TAG_CORPUS` test in
`library/scanner.rs`) has them agreeing on every field symphonia supplies bar
four genre spellings, and symphonia ahead in two places: it recovers a year from
a `RecordingDate` on 51 files where lofty finds none, and it reads the real
`TIT2` of a file lofty gives up on and names after its file instead.

What lofty has that symphonia has not is the **bitrate** — `AudioCodecParameters`
carries none — so making symphonia the primary reader would empty that column
for the whole library. Staying with lofty also means no `TAG_READ_VERSION` bump
and no row anywhere being re-read, so a library that is correct today stays
exactly as it is. The fallback earns its place on the files lofty cannot open at
all, and changes nothing else.

Reading tags correctly matters more than it looks: an empty column is written
back to the file as an _absent_ tag, so a reader that drops a frame makes the
write-back delete it (see [Editing a track](#editing-a-track)). That is why the
survey exists and why it is kept.

A file both readers refuse still enters the library named after itself.

**Read every metadata revision, not the newest.** An MP3 may carry an ID3v2 tag
at its head and an ID3v1 one at its tail. symphonia reports them as two
revisions, and the ID3v1 is the _newer_ one — six fixed fields where the ID3v2
has everything. `Metadata::skip_to_latest` returns that one, which read four
tags out of a file holding fifteen and took a wrong track number with it. The
revisions are read oldest first and the first value for each field wins, so the
richer tag does.

### The analysis pass

Every launch and every completed scan starts the **analysis pass**
(`library/waveform_scan.rs`). It works through present tracks missing any of the
things below. A second bar under the scan bar shows its progress.

**One thread reads and several decode.** The pass pulls one file off the library
at a time — the same rule the prefetch cache follows, and for the same reason
([audio.md](./audio.md#the-prefetch-cache)) — and hands each to a pool of decode
workers (the core count less two, kept between 2 and 6) over a one-slot channel,
so the CPU fan-out costs the share nothing and playback and the UI stay
responsive. Every read the pass makes is the reader's, including the head read
behind a fingerprint-only job. What bounds it is whole files rather than cores:
one per decoder, one queued, one in the reader's hand, and a track can be 100 MB.

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
- The **duration** is overwritten with the decode's own sample count. This is
  the one measurement written in place over its tag-derived value rather than
  beside it: a tempo or a key read from a tag is a claim worth keeping next to a
  measurement that disagrees, while a duration is either the length of the audio
  or it is wrong. lofty reads a VBR MP3 with no Xing header as its file size
  over its first frame's bitrate, which on one real file is 1477 s of a 239 s
  track — and everything that divides by a duration, from the deck's waveform
  crop to the toolbar's _Playtime_, is then wrong by that ratio.
  The write is announced as `duration-ready`, and the playlist engine and the
  renderer take the new length into every copy of the track they hold but the
  one on air — a track queued before the pass reached it would otherwise go to
  air with the tag's. `duration_measured_at` is what "measured" means, since the column cannot say
  whose number it holds. A row analysed before this measurement existed is
  corrected and stamped at the next launch without a decode, from the duration
  its stored level envelope already carries; a row with no envelope to read —
  an operator-owned one is never given one — is left unstamped, which queues it
  for a decode. A decode that counted no audio is stamped too, and leaves the
  tag's length where it was: zero is not a length. The measurement hangs on
  the fingerprint like the others, so audio that changed only past the first
  megabyte keeps a stale one — see
  [track-identity.md](./track-identity.md#accepted-limits). What the tags said is kept in
  `tag_duration`, and a file whose tags disagree with its audio is listed under
  [Bad durations](./library-health.md#bad-durations).
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
tracks](./library-health.md#unreadable-tracks). A file that brings the decoder
down instead of returning an error is recorded the same way, and for the same
reason it has to be: it would do it again on every launch, having been pulled
across the share first. One such file costs one file — never the pass, and never
the pass's ability to run again. A file that could not be read is
skipped for the rest of the run only — by the reader, so it never reaches a
decoder at all. Cancelling a scan cancels the pass too, and the pass can be
stopped on its own from the status bar without stopping the scan that started
it. A cancel stops the reader after the file it is on and each decoder after the
file it holds; anything already read but not yet started is dropped undecoded.
Either way the queue is row state, so the next pass picks up where the last one
stopped.

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

A file whose tags bring the parser down is logged and skipped, exactly like one
whose tags could not be read: the row keeps its old version and comes back on the
next launch.

Adding another tag field later is a schema step plus a bump of
`TAG_READ_VERSION` — the whole library requeues and fills itself in.

## Tracks

A track is one row in the library database. Its id never changes, so the
playlist, the history and the saved session refer to it by id.

| part                   | source                           | survives a rescan                    |
| ---------------------- | -------------------------------- | ------------------------------------ |
| path, content type     | where the file is                | follows the file                     |
| tags, format           | the file                         | re-read when the file changes        |
| duration               | the tags, then the analysis pass | re-measured when the _audio_ changes |
| tags edited in the app | the operator                     | always                               |
| cue points             | the operator                     | always                               |
| play count             | airings                          | always                               |
| waveform, loudness     | the analysis pass                | re-measured when the _audio_ changes |
| automatic cue points   | the analysis pass                | re-derived when the _audio_ changes  |
| fingerprint            | the scan                         | recomputed when the file changes     |

A file's modification time moving does not mean its audio did. An external
tagger rewrites every file it touches, and so do `touch`, `rsync` and a share
remounting — in each case the samples underneath are untouched and every
measurement still describes them. So the scan re-fingerprints a known path
whose modification time moved, and the fingerprint decides:

- **the same audio** — nothing measured is disturbed. The tags are re-read and
  that is all. This is the common case, and re-deriving here would be a full
  decode per track for nothing. The one exception is a root the operator
  reclassified, which rescans its files without their audio moving: the
  automatic trio was derived for the other class and goes back to the pass,
  while the waveform, the loudness and the level envelope stand. See
  [cue-auto-analysis.md](./cue-auto-analysis.md).
- **different audio** — a different recording, so a different track. The row
  goes [missing](./track-identity.md#missing-not-deleted) and the file enters
  the library as a track of its own. See
  [track-identity.md](./track-identity.md#reconciling-a-scan).
- **no answer** — the file's head could not be demuxed, or the row has no
  fingerprint yet. The row is kept and its measurements are dropped, so the
  pass measures the file as it is now. Not knowing costs a re-measurement,
  because the alternative is a stale ReplayGain reaching air with nothing
  queued to correct it.

The analysis pass checks the same thing from its own end. A decode takes
seconds, and a file replaced inside that window would otherwise have the pass
write the old audio's result back over the invalidation the scan just made —
onto a row whose emptiness is the only thing that would have queued it again.
Each store is refused unless the row still holds the modification time the
decode read.

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
_Commercials_ or _Jingles_. A fourth tab, _Playlists_, lists the saved
playlists instead of tracks — see [saved-playlists.md](./saved-playlists.md).

- **Search** matches title, artist, album, album artist and genre. Each word is a prefix, so
  `beat abb` finds _Abbey Road_ by _The Beatles_. The search runs a quarter of a
  second after typing stops, and shows at most 200 tracks, best match first.
  Enter runs it at once and moves focus to the first result.
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
| pick it                 | click, or Space — see [below](#selecting-several)         |
| add to the playlist     | double-click, or the `+` button                           |
| add at a position       | drag the row onto the playlist; the line shows where      |
| preview on the cue deck | the headphones button (only with a cue device configured) |
| edit metadata           | the pencil button                                         |
| everything else         | right-click, the menu key, Shift+F10 or Ctrl+Enter        |

### Selecting several

Queueing an album, or a run of tracks picked across several searches, is one
action on a **selection**.

- **Click** a row to pick it, click again to drop it. A checkbox takes the place
  of the track number on the row under the pointer and on a picked row; every
  other row keeps its number. There is no column for it.
- **Shift-click** picks every row from the last one picked to this one.
- **Space** picks the focused row, **Shift+Space** a range, **Escape** clears.
- **Select all**, the button at the right of the column headers, picks every
  row in the list, and drops them again once every one is picked. It is always
  there, so it needs nothing picked first.
- **Ctrl+A** (**Cmd+A** on macOS) does the same. It means the library wherever
  focus is, with two exceptions: in a text field, the search box included, the
  same keys select the text, and nothing happens while Settings, a dialog or a
  row menu is open. Escape follows the same rule.
- **Enter** in the search box moves focus to the first result, so search,
  Enter, Ctrl/Cmd+A picks an album without the pointer.

A selection is **ordered by pick**: tracks are queued in the order they were
selected, and the checkbox shows each row's place in that order. A range, and
_Select all_, contribute their rows top to bottom as listed, whichever end was
clicked first — so an album sorted by `#` and selected whole is queued as the
record runs.

It **outlives the list**. Searching, sorting and changing tab leave it alone, so
it can hold tracks that are not on screen and tracks of different content
types. Those are counted, not listed: the bar reads `8 selected · 5 not shown`.
The playlist is where the result is looked over. It does not outlive the app: a
restart starts with nothing picked.

The **selection bar** floats over the bottom of the list while anything is
picked. It never takes space in the flow: a bar that pushed the rows down would
move them under the second click of a double-click.

| action                   | how                                                                                                      |
| ------------------------ | -------------------------------------------------------------------------------------------------------- |
| add to the playlist      | _Add N to playlist_ on the bar, or in a picked row's menu                                                |
| add as next              | _Add N as next_: the block goes to the head, first pick first                                            |
| add at a position        | drag any picked row; the whole selection lands where dropped                                             |
| pick every row           | Ctrl/Cmd+A, or _Select all_ at the right of the column headers                                           |
| drop the rows shown      | either again once all are picked                                                                         |
| keep as a saved playlist | _Save…_ on the bar, or in a picked row's menu — see [saved-playlists.md](./saved-playlists.md#authoring) |
| drop everything          | _Clear_, or Escape                                                                                       |

An action that succeeds **clears the selection**; a drag dropped nowhere keeps
it. Kept, the next add would queue the same tracks again, some of them out of
sight.

A row's own buttons and a double-click always act on that one row, picked or
not, and a double-click drops the row from the selection. Dragging a row that is
not picked carries only that row. The menu on a picked row is the selection's
when more than one track is picked, and offers the two adds and the saved
playlist dialog and nothing else:
_Play now_ is never offered for several, and clearing is the bar's, where the
count of tracks not shown sits beside it.

_Select all_ takes the rows in the list, which is at most 200. A large add is
not confirmed — the button carries the number. A picked track that has left the
library by the time it is added is skipped and logged; one whose file has gone
missing is queued with the missing badge, as a single add would queue it.

Several tracks are queued by one command, `playlist_add_many`: one transition
and one snapshot rather than one per track. See
[playlist.md](./playlist.md#commands).

### The row menu

The row menu offers _Add to playlist_, _Add as next_, _Preview on cue deck_,
_Add to saved playlist…_, _Edit metadata…_, _Cue points…_, _Show in folder_ and, set apart and marked as
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

1. Read the whole file into memory and **settle what the container is**, by
   probing it with symphonia — the demuxer the app already trusts to decode and
   fingerprint every track. lofty picks its writer by sniffing the bytes, and a
   sniff can be wrong: an Ogg whose first logical stream is Theora reads to it
   as an MPEG, so it would write an ID3v2 header onto an Ogg. A tag type the
   container cannot carry is refused here, and the file name never gets a vote —
   a name is a claim about a file rather than a reading of one. The fingerprint
   check in step 3 does not cover this, because symphonia steps over a leading
   ID3v2 tag and the audio compares equal either way.
2. Set the tags there with `lofty`. The comment replaces only the file's own,
   undescribed comment — an iTunes-processed file keeps frames like `iTunSMPB`,
   its gapless-playback data, which a plain overwrite of the comment would
   destroy. Album artist, the track and disc positions, the key and the comment
   are written only once
   the row has been read at the current tag generation, or for a field the
   operator edited: until the [tag backfill](#tag-backfill) reaches a row those
   columns are empty because nobody has looked, not because the file has none,
   and writing them back would strip the file's own.
3. Fingerprint the tagged copy. If the fingerprint differs from the stored one,
   stop, because the write would cost the track its identity.
4. Write a sibling `<name>.rdj-tmp`, flush it, copy the file's permissions, and
   rename it over the original.
5. Store the file's new mtime and clear the edited flags, so the next scan sees
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

Music can be drawn from one saved playlist instead of the whole music library:
_Auto source_ on an open saved playlist. Jingles and commercials still come from
their libraries. See
[saved-playlists.md](./saved-playlists.md#as-the-auto-playlists-source).

Tracks already in the playlist, and missing tracks, are never picked. The cadences and
buffer sizes are under _Settings → Advanced_. _+ Jingle_ and _+ Comm_ in the
playlist add one filler by the same rules.

## Library health

_Settings → Library_ also reports what needs attention: missing tracks, exact and
possible duplicates, and disk changes the library has not picked up. A count on
the Settings button says when there is something to look at. See
[library-health.md](./library-health.md).

## Unsupported formats

The scan accepts a format only if a deck can play it. A file the app cannot
decode has no business in a library: it would be listed, take a row, fail the
analysis pass and then fail on air. So the accepted extensions and the decoders
compiled into the build are two halves of one list — `audio_measure/formats.rs`
and the `symphonia` and `rodio` features in `Cargo.toml`.

**Enabling a format takes two features, not one.** There are two copies of
symphonia in the build and they do not unify: the direct dependency is 0.6 and
serves the demuxing the app does itself, for tags and fingerprints, while `rodio`
brings its own 0.5 and that is the one that _decodes_. So `symphonia/aiff` alone
makes a file fingerprint and go no further; `rodio/symphonia-aiff` is what lets a
deck play it. Both are set for every accepted format.

Skipped, and why:

| format         | why                                                                |
| -------------- | ------------------------------------------------------------------ |
| WMA            | no decoder                                                         |
| Opus           | symphonia demuxes Ogg Opus but ships no Opus decoder               |
| Matroska, WebM | would need the `mkv` feature, and WebM audio is nearly always Opus |

Convert them with [ffmpeg](https://ffmpeg.org/) first, e.g. to MP3:

```bash
ffmpeg -i track.wma -c:a libmp3lame -q:a 2 track.mp3
```

or a whole folder at once:

```bash
for f in *.wma; do ffmpeg -i "$f" -c:a libmp3lame -q:a 2 "${f%.wma}.mp3"; done
```

Tags are carried over. Delete or move the originals afterwards. An Opus file
converts the same way, and a `.webm` holding Opus with `-c:a libmp3lame` too.

## Where it is stored

| file            | holds                                                  |
| --------------- | ------------------------------------------------------ |
| `radiodiodj.db` | tracks, their operator work, and health dismissals     |
| `config.json`   | library paths and tuning, including the check interval |
| `session.json`  | the playlist and history, by track id                  |

They sit in the app data directory listed in `AGENTS.md`. The database uses
SQLite in WAL mode with an FTS5 index for search; its schema rules, backups and
baseline resets are in [database.md](./database.md).

## Code map

| area                               | where                                                                         |
| ---------------------------------- | ----------------------------------------------------------------------------- |
| listing and the changed/gone rules | `src-tauri/src/library/listing.rs`                                            |
| scan and reconcile                 | `library/scanner.rs`, `library/scan_state.rs`, `Db::reconcile`                |
| fingerprint                        | `audio_measure/fingerprint.rs`                                                |
| waveforms and fingerprints         | `library/waveform_scan.rs`                                                    |
| tag backfill                       | `library/tag_backfill.rs`                                                     |
| health report                      | `library/health.rs`                                                           |
| library check                      | `library/check.rs`                                                            |
| queries and schema                 | `library/db.rs`, `library/schema.sql`                                         |
| search design                      | [library-search.md](./library-search.md)                                      |
| auto-playlist selection            | `playlist/generate.rs`                                                        |
| library panel                      | `src/features/library/LibraryPanel.svelte`                                    |
| selection                          | `src/shared/selection.ts`                                                     |
| saved playlists                    | `library/db/saved_playlists.rs`, `src/features/library/SavedPlaylists.svelte` |
| hover card                         | `src/features/track/TrackTooltip.svelte`                                      |
| metadata editor                    | `src/features/track/MetadataOverlay.svelte`                                   |
| tag write-back                     | `library/tag_write.rs`                                                        |
| settings and health view           | `src/features/settings/`, `src/features/health/`                              |
