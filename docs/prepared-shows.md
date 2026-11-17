# Prepared shows — groups in the playlist, and the hold at a Stop

**Planned.** Nothing on this page is built. It builds on
[Stops in saved playlists](./saved-playlists.md#stops), which are. The split
into increments is [at the end](#increments).

A presenter prepares a show before its hour: the music, the jingles, and the
points where the studio talks. They save it, carry it to the studio, append it
when the hour comes, and run it. While it airs, the operator can see which
queued rows are the show, and at each talk break the main deck says how long
the break was meant to last.

## The problem

Two things are missing.

- **An appended show dissolves into the playlist.** Its rows look like every
  other row. Nothing says where it starts and ends, or that the track on air
  belongs to it.
- **A reached Stop looks like a pressed one.** Advancement consumes the marker
  and calls the same `stop()` the Stop button does. The deck goes empty, and
  nothing tells the producer that this silence is a planned break, or how long
  it was meant to be.

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

**Save playlist as…** takes everything queued, Stops and their plans included,
as it does today. Groups are flattened.

**Save group as…**, on a header's menu, takes that group's remaining rows. It is
how an adjusted show is kept: edit it in the playlist, where it is the
operator's, and save it as a new list.

Both make a new saved playlist and are open while admin mode is locked, as
making one already is. Neither replaces an existing list; that stays an admin's
edit. The name follows the [existing rule](./saved-playlists.md#names), starting
from the group's name.

## Editing in the playlist

A queued copy is the operator's. Moving rows in and out of groups, removing a
group and saving one are open to everyone, as editing a queued Stop's plan
already is, because none of it changes a saved playlist.

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

The wall-clock side of a break is the station clock beside the toolbar's brand;
the counter only says how far into the break the studio is.

## Not in scope

- **Start times in Upcoming.** Each row's expected start, moving together while
  playback waits, and unknown past an unplanned Stop. It needs a timing schedule
  in the backend, fed from the audio worker, and gets its own design. The
  rejected first attempt and the shared-delay model that replaced it are in the
  `feat/stop-timing-and-eta` branch (#687), `docs/playlist-eta.md` and
  `playlist/timing.rs`.
- **Nested groups**, and a group as a unit the engine advances through.
- **Automatic resume** at the end of a planned Stop, now or as an option.
- **Dynamic commercial breaks.**

## Increments

| #   | increment              | holds                                                                                                | depends on | issue |
| --- | ---------------------- | ---------------------------------------------------------------------------------------------------- | ---------- | ----- |
| 2   | Groups in the playlist | `group` on items and in the session, what makes one, headers, colour, collapse, drops, Save group as | —          | #683  |
| 3   | The hold               | `holding` in the snapshot, the deck's STOP view and counter                                          | —          | #684  |

The original ask, a show prepared with its breaks, is delivered by
[Stops in saved playlists](./saved-playlists.md#stops). Groups and the hold do
not depend on each other; whichever lands second carries the hold's group.
Planned order: 2, 3.
