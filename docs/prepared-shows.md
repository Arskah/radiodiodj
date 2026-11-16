# Prepared shows — Stops in saved playlists, groups in the playlist

**Planned.** Nothing on this page is built. The split into increments is
[at the end](#increments).

A presenter prepares a show before its hour: the music, the jingles, and the
points where the studio talks. They save it, carry it to the studio, append it
when the hour comes, and run it. While it airs, the operator can see which
queued rows are the show, and at each talk break the main deck says how long
the break was meant to last.

## The problem

Three things are missing.

- **A saved playlist cannot hold a Stop.** A Stop marker is a playlist item and
  not a track, so [saved-playlists.md](./saved-playlists.md) left it out: the
  operator adds one after appending. A show with four talk breaks is four Stops
  placed by hand, on air, under time pressure.
- **An appended show dissolves into the playlist.** Its rows look like every
  other row. Nothing says where it starts and ends, or that the track on air
  belongs to it.
- **A reached Stop looks like a pressed one.** Advancement consumes the marker
  and calls the same `stop()` the Stop button does. The deck goes empty, and
  nothing tells the producer that this silence is a planned break, or how long
  it was meant to be.

## No kind of saved playlist

Any saved playlist may hold Stops. There is no "prepared show" type beside an
ordinary playlist: the two would share every control, and a type that refused
Stops would only guard against appending a list with breaks in it and walking
away.

That case is answered by showing it. A saved playlist's row reads
`12 tracks · 2 stops`, and a Stop is an obvious row once queued. Appending never
asks for confirmation, and nothing ever resumes playback on its own: an
explicit Stop is the author's decision, and studio discussion fills it. See
[the hold](#the-hold).

The title of this page names the use, not a type.

## Stops in a saved playlist

### The entry

An entry is a track or a Stop. `saved_playlist_entries` gains two columns in
one appended migration step:

```sql
kind                     TEXT NOT NULL DEFAULT 'track',  -- 'track' | 'stop'
planned_duration_seconds INTEGER                         -- Stops only; NULL = unplanned
```

`kind` is the word the playlist already uses: `PlaylistItem` is
`#[serde(tag = "kind")]` with `track` and `stop`, and so are the session file
and the renderer's types. A Stop is not a content type; `ContentType` stays
music, jingle and commercial.

A Stop row has every track column `NULL`. Without `kind` it would read as an
**unmatched entry**, which is the hazard: binding, the missing count, the
fingerprint match and the auto-playlist pool would each have to tell the two
apart by a guess. With it, each of them filters on `kind = 'track'` and a Stop
can never be bound, counted missing or picked.

A planned duration is whole seconds, zero included: `0:00` is an explicit
zero-length plan, distinct from none. It is not the audio `duration` column,
which is a track's snapshot.

### Authoring

An open saved playlist gets **+ Stop** beside its controls, for an admin, as
every edit of an existing list is ([admin mode](./saved-playlists.md#admin-mode)).
It appends a Stop, which is dragged into place by the existing reorder and
removed like any entry.

A Stop row reads _Stop_ and carries an `m:ss` field for its planned duration:
blank is unplanned, minutes above 59 are accepted, and invalid input is refused
without changing the stored value. It has no track columns, card, cue button or
**+**. It can be picked, moved and removed as part of the entry selection, and
**Add N to playlist** takes the picked Stops with the picked tracks, in pick
order.

The list's row shows tracks and Stops counted apart. Durations, missing counts
and the playable-music count ignore Stops.

### Using one

**Add to playlist** queues every entry in stored order, Stops included, with
their planned durations. It never starts playback.

**Add with jingles and commercials** treats a Stop as one more fixed entry: the
cadence is counted over the whole list as today, and the drawn jingles and
commercials are spread by `interleave_evenly` around the author's entries. A
drawn ident can land beside a Stop, which is what a talk break usually has.

**As the auto-playlist source**, a list with Stops is a pool of its music
entries as before. The pick reads bound tracks, so Stops are never in it.

### The file and the hub

The file always writes `version: 2`. A reader takes 1 and 2; a version-1 file is
all tracks. An entry gains `kind`, absent meaning `track`:

```json
{ "kind": "stop", "plannedDurationSeconds": 120 }
```

An older app refuses version 2 whole, as it refuses any version it does not
know. That is the point of the bump: a Stop read by a build that ignores `kind`
becomes an unmatched entry with no fingerprint, and the next save of that list
makes it one for good.

The hub's `playlist` document carries the same entry, in its own key style:
`{"kind": "stop", "planned_duration_seconds": 120}`. `hub_station.protocol` goes
from 2 to 3. Inside one protocol an unknown key is ignored, so a protocol-2
studio would read a Stop as an unmatched entry and sync that back on its next
save of the list. A studio that does not know 3 stops syncing and says so, until
it is updated. The music-library web page reads no `playlist` rows and checks no
protocol; its drafts stay version-1 files of tracks.

## Groups in the playlist

### The model

The playlist stays one flat sequence of items, and the engine stays as it is.
An item gains an optional **group**:

```rust
group: Option<Group>   // Group { id, name }
```

`id` is fresh for each insertion, so the same saved playlist queued twice is two
groups. `name` is copied when it is queued, so renaming or deleting the saved
original changes nothing queued. Both item variants carry it: a Stop belongs to
its show as much as a track does. `SessionPlaylistItem` carries it under
`serde(default)`, so groups survive a restart and an older session loads with
none.

Advancement, refill, prefetch, the outage rules and Stop consumption never read
it. A group is a label the engine stores and returns. Every decision about what
belongs to which group is made by the command that inserts or moves the item,
which says so explicitly.

### What makes one

| action                                                  | result                                                                                                   |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| **Add to playlist** on a saved playlist, or its drag    | a new group: every entry in stored order                                                                 |
| **Add with jingles and commercials**                    | a new group, the drawn items in it                                                                       |
| tracks or a selection, from the library or a saved list | no group, appended                                                                                       |
| **+**, **Add N tracks**, **+ Stop**                     | no group, appended after everything                                                                      |
| **Add as next**                                         | the front of the playlist, in the group of the track on air, or of the [hold](#the-hold); otherwise none |
| refill                                                  | no group, appended                                                                                       |

**Add as next** joins the show on air so that the show stays one run. Without
it, a track added as next during a show would sit untagged between the track on
air and the rest of the show, and the show would draw two headers.

### In Upcoming

A group is drawn from consecutive items that share a group id. That is all the
structure there is; the renderer derives the rest.

```text
Standalone track
▾ ▌ John's morning show · 3 tracks · 1 stop
  ▌ Opening jingle
  ▌ Track A
  ▌ Stop · 2:00
  ▌ Track B
Standalone track
```

- **Header.** A row the size of a track row: disclosure, name, and what remains
  in it. It is the renderer's, not an item.
- **Colour.** A stripe on the header and every row of the group, Stops included.
  The colour is picked per group from a small palette in insertion order, so two
  neighbouring groups differ. It comes from theme tokens.
- **Collapse** hides the rows and keeps the header. It is presentation only,
  never order or playback, held in renderer memory and not saved: after a
  restart every group is open. The disclosure is a labelled button that exposes
  its state.
- **On air.** The main deck shows the group's stripe and name when the track on
  air carries one. While it does, or while a hold carries it, the header stays
  at the top of Upcoming, even with nothing left beneath it. It goes when the
  last of the group leaves the deck.

### Moving rows

Drops are positional, by the existing `gapAt` and `moveTarget`:

- A track dropped **between two rows of one group** joins it.
- A track dropped **at a boundary** — between two groups, or between a group and
  a standalone row — joins none. Moving a row out of a group clears its group.
- A track dropped **on a collapsed header** is appended as the group's last item,
  and the group stays collapsed. A drop before or after the header is outside it.
- **A header** drags the whole run as one, while none of it is on air. The
  header of the group on air does not move.
- **Removing a header** removes the group's remaining rows. The track on air
  plays on.

So an open group cannot be extended at its very end by one drop; a collapsed one
can. Telling an inside drop from an outside one at the same boundary was what
made the nested design hard to use and hard to build.

### Saving it back

**Save playlist as…** takes everything queued, as today, and now keeps its Stops
and their planned durations in order. Groups are flattened; item overrides are
still not carried; the track on air is not included.

**Save group as…**, on a header's menu, takes that group's remaining rows. It is
how an adjusted show is kept: edit it in the playlist, where it is the
operator's, and save it as a new list.

Both make a new saved playlist and are open while admin mode is locked, as
making one already is. Neither replaces an existing list; that stays an admin's
edit. The name follows the [existing rule](./saved-playlists.md#names), starting
from the group's name.

## Editing in the playlist

A queued copy is the operator's. Adding, moving and removing rows, adding Stops
and editing a queued Stop's planned duration are open to everyone, because none
of it changes a saved playlist. The `m:ss` field is the same one the saved
playlist uses.

## The hold

Consuming a Stop stops the deck as today, and the snapshot gains a hold:

```rust
holding: Option<Hold>   // Hold { planned_duration_seconds, group }
```

The hold ends with anything that ends the break: Play, Next, play-now, the Stop
button, a track loaded on the main deck. Editing Upcoming does not end it.
Reaching another Stop starts a fresh one. It is not saved: a restart during a
hold comes back to the ordinary stopped deck with the playlist intact.

The backend keeps no time. The renderer notes when the hold arrived and counts
from there, sampling the clock on each tick rather than counting ticks:

```text
planned 2:00          unplanned
  −2:00  break         0:00
  −0:45                1:15
   0:00                3:40 …
  +0:30  overrun
```

The main deck shows **STOP** where the title is and the counter where the
remaining time is. Past a planned deadline the counter takes the warning colour
and the heading pulses, with no pulse under reduced motion. An unplanned Stop
counts up and never warns. The waveform is hidden and seeking is off; Play and
Next keep their meaning, and either ends the hold.

**Reaching zero does nothing to playback.** The counter is for the producer.
The studio is talking, so playback starts only when the producer starts it.

A hold keeps its group, which is what keeps the header pinned through a break
and what **Add as next** joins.

## The station clock

The toolbar shows the host's local time, `HH:MM:SS`, beside the station brand,
the brand being text or an image. It is the renderer's clock alone and has no
part in the playlist, so it can ship on its own. A producer times a break
against it.

## Not in scope

- **Start times in Upcoming.** Each row's expected start, moving together while
  playback waits, and unknown past an unplanned Stop. It needs a timing schedule
  in the backend, fed from the audio worker, and gets its own design. The
  rejected first attempt and the shared-delay model that replaced it are in the
  `feat/stop-timing-and-eta` branch (#687), `docs/playlist-eta.md` and
  `playlist/timing.rs`.
- **Nested groups**, and a group as a unit the engine advances through.
- **Automatic resume** at the end of a planned Stop, now or as an option.
- **Stops authored on the web page.** Its drafts stay track-only files; that is
  the web page's own repository.
- **Dynamic commercial breaks.**

## Increments

| #   | increment                | holds                                                                                                                           | depends on | issue |
| --- | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------- | ---------- | ----- |
| A   | Station clock            | the toolbar clock                                                                                                               | —          | new   |
| 1   | Stops in saved playlists | `kind` and planned duration, migration, file 2, hub protocol 3, **+ Stop** and the `m:ss` field in both places, counts, Save as | —          | #682  |
| 2   | Groups in the playlist   | `group` on items and in the session, what makes one, headers, colour, collapse, drops, Save group as                            | —          | #683  |
| 3   | The hold                 | `holding` in the snapshot, the deck's STOP view and counter                                                                     | 1          | #684  |

The original ask, a show prepared with its breaks, is delivered by 1. Groups and
the hold do not depend on each other; whichever lands second carries the hold's
group. Planned order: A, 1, 2, 3.
