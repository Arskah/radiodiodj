# Prepared shows — the hold at a Stop

**Planned.** Nothing on this page is built. It builds on
[Stops in saved playlists](./saved-playlists.md#stops) and on
[groups in the playlist](./playlist.md#groups), which are. The split into
increments is [at the end](#increments).

A presenter prepares a show before its hour: the music, the jingles, and the
points where the studio talks. They save it, carry it to the studio, append it
when the hour comes, and run it. While it airs, the operator can see which
queued rows are the show, and at each talk break the main deck says how long
the break was meant to last.

## The problem

A reached Stop looks like a pressed one. Advancement consumes the marker and
calls the same `stop()` the Stop button does. The deck goes empty, and nothing
tells the producer that this silence is a planned break, or how long it was
meant to be.

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

A hold keeps the group of the Stop it consumed, which keeps that group's header
pinned through the break. **Add as next** joins it, as it joins the group of a
track on air.

The wall-clock side of a break is the station clock beside the toolbar's brand;
the counter only says how far into the break the studio is.

## Not in scope

- **Start times in Upcoming.** Each row's expected start, moving together while
  playback waits, and unknown past an unplanned Stop. It needs a timing schedule
  in the backend, fed from the audio worker, and gets its own design. The
  rejected first attempt and the shared-delay model that replaced it are in the
  `feat/stop-timing-and-eta` branch (#687), `docs/playlist-eta.md` and
  `playlist/timing.rs`.
- **Automatic resume** at the end of a planned Stop, now or as an option.
- **Dynamic commercial breaks.**

## Increments

| #   | increment | holds                                                       | depends on | issue |
| --- | --------- | ----------------------------------------------------------- | ---------- | ----- |
| 3   | The hold  | `holding` in the snapshot, the deck's STOP view and counter | —          | #684  |

The original ask, a show prepared with its breaks, is delivered by
[Stops in saved playlists](./saved-playlists.md#stops).
