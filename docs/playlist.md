# Playlist — what is queued, what is on air, and how the two advance

The playlist is the ordered sequence of upcoming items that feeds the `main`
deck, plus the rules that move one item onto air and the next into place. The
**backend owns all of it**; the renderer is a projection.

| topic                                   | detail                             |
| --------------------------------------- | ---------------------------------- |
| which tracks the auto-playlist may pick | [rotation.md](./rotation.md)       |
| how a queued item reaches air           | [program-bus.md](./program-bus.md) |
| the markers an item can carry           | [cue-points.md](./cue-points.md)   |

## Backend ownership

The playlist, what is on air, advancement, auto-playlist refill, the prefetch
window and the outage retry all live in `src-tauri/src/playlist/`. The renderer
sends `playlist_*` commands and mirrors the snapshot that comes back. It keeps
no playlist of its own and computes no advancement.

The reason is timing: a handover is authorised by arm-loading the next item
before the outgoing track reaches its `next_start`, and an IPC round-trip in the
middle of an audible transition is not something to design in. Everything that
decides what plays next therefore sits on the same side of the boundary as the
decks.

The module splits three ways:

- **`generate.rs`** — picks tracks out of the library with jingle/commercial
  interleaving. The auto-playlist's source of material.
- **`engine.rs`** — the pure state machine. A transition takes the current state
  plus what the caller already knows and returns `Effect`s. No database, no
  audio device, no Tauri handle, so the advancement specification is testable on
  its own.
- **`service.rs`** — wiring. Turns effects into deck commands, play-count
  increments, prefetch-window updates and retry timers, and emits a snapshot at
  the end of every transition.

### The snapshot

```
program:playlist-state { playlist, current, history }
```

- `playlist` — the upcoming items in order, stop markers included
- `current` — the item now on the `main` deck
- `history` — the last `tuning.autoPlaylist.historyCap` (100) aired tracks

A whole snapshot on every mutation, never deltas: the playlist is short,
snapshots are idempotent, and a dropped or out-of-order event cannot
desynchronise the UI from what is actually going to air. `history` is carried
whole for the same reason; it is a display window over `play_log`, not a
retention limit — every airing stays logged. See
[rotation.md](./rotation.md#the-airing-log).

A `PlaylistItem::Track` carries the whole `Track`, not just its id, so the
snapshot is displayable without a second round-trip to resolve titles.

### Commands

```
playlist_add             playlist_add_front        playlist_add_filler
playlist_add_stop_marker playlist_remove           playlist_move
playlist_clear           playlist_play_index       playlist_play_now
playlist_next            playlist_prev             playlist_stop
playlist_set_auto_advance                          playlist_set_auto_playlist
playlist_set_item_cue_points                       playlist_sync
```

Every one of them but `playlist_sync` **queues** its transition and returns at
once. A Tauri command handler declared without `async` runs on the process main
thread, which is the thread the window is drawn from, and a transition is not
cheap: a refill under the queue's threshold is tens of milliseconds of SQL on a
large library, and it waits on the same database lock the analysis pass holds, so
the wait is not bounded by its own query. Skipping tracks froze the UI for as
long as that took, while the audio — driven from the bus worker, off a file
already resident in RAM — started on time. What the operator saw was the next
track playing before the deck redrew.

The queue is one thread, not the async runtime's pool, because the order
commands are applied in is part of what they mean: `playlist_remove` and
`playlist_move` carry queue indices, and a pool would let the second of two
clicks overtake the first and act on positions that no longer exist.

Nothing is lost by answering before the work is done. The snapshot was always
what the renderer read — the commands' return values were never anything but
errors it logged — so a command that cannot be carried out (a track purged
between the click and the lookup, an item whose file is missing) is logged in the
backend instead. `playlist_sync` is the exception and stays direct: it is a read,
and the renderer calls it to find the state it missed. Session restore is direct
for the same reason, so a `playlist_sync` arriving as the window comes up cannot
be answered with an empty playlist that a still-queued restore is about to fill.

The renderer's two per-track-change reads, `get_waveform` and `get_cover_art`,
are off the main thread too, and for `get_cover_art` that is not optional: it
opens and parses the audio file itself, so on a wedged share it would hold the
main thread for as long as the mount takes to answer. The decks never read a
share on their hot path ([audio.md](./audio.md)); artwork must not be the
exception.

## Effects

A transition returns effects rather than performing them:

| effect                        | means                                                                   |
| ----------------------------- | ----------------------------------------------------------------------- |
| `Play { id, cue_override }`   | load on `main` and start                                                |
| `Resume { id, seconds, .. }`  | load, seek, stay paused — session restore                               |
| `Stop`                        | stop the main deck                                                      |
| `Arm { id, cue_override }`    | load the next item parked and silent, so a handover has somewhere to go |
| `Disarm`                      | what was armed is no longer next                                        |
| `NowPlaying { id, .. }`       | announce a track that went on air by handover                           |
| `TrackPlayed(id)`             | count the airing                                                        |
| `ArmRetry(n)` / `CancelRetry` | the outage backoff timer                                                |

Two rules worth keeping straight. **Arming is not an airing**: no play count, no
history entry, no broadcast pending track — all of that happens when the `main`
role moves. And `TrackPlayed` is issued when the track actually reached the deck,
never when a load was merely asked for, because a read that fails over a dead
share is not an airing.

## Modes

**Auto advance** (AUTO / MANUAL) decides whether the end of a track pulls the
next one. In MANUAL the deck simply stops; `on_ended`, `on_load_failed` and arm
reconciliation all return empty transitions.

**Auto-playlist** keeps the queue full by itself. It maintains a lookahead
buffer of `tuning.autoPlaylist.autoPlaylistBuffer` (20) items and refills when
fewer than `autoPlaylistThreshold` (5) remain. The refill runs inside
`engine`, so a refill and the track change that triggered it are one transition
— there is no window in which the queue is observably empty.

Which tracks may be picked is [rotation.md](./rotation.md): a title window, an
artist window, and one track per artist within a generated block.

**Interleave** weaves non-music into a generated block: one jingle every
`tuning.interleave.jingleEvery` (4) music tracks and one commercial every
`commercialEvery` (8). Only music advances the counters, so a manually queued
jingle does not shift the cadence.

**Stop markers** are items too. Advancement consumes one and stops instead of
loading the next track, which is how an operator parks the show at a fixed point
without emptying the queue. They contribute nothing to the prefetch window and
nothing to the Upcoming tab's total duration.

## Outages

The playlist is where a dead network share becomes a survivable event rather
than dead air.

- **Skip to cached.** When a load fails, the failed id is dropped from the
  cached set (so stale membership cannot spin an instant retry loop), the track
  is marked for retry, and advancement looks for the next item whose bytes are
  already resident in RAM — see [audio.md](./audio.md#the-prefetch-cache).
- **Retry.** `ArmRetry(n)` selects a delay from
  `tuning.autoPlaylist.netRetryBackoffsMs` (1 s, 2 s, 5 s, saturating at the
  last). A fresh `cache-state` naming the track is what makes it a candidate
  again, and the interrupted track goes back on when the share returns.
- **Missing tracks.** `on_missing_state` takes ids from the same
  `library-health` event the health report uses, and advancement drops them up
  to the next stop marker, even on a cold cache. See
  [library-health.md](./library-health.md).

## Item overrides

A `PlaylistItem::Track` carries `cue_override: Option<CuePoints>` — cue points
for that one airing.

- `None`, the common case, means the item **references** the track. Correcting a
  radio edit therefore corrects every queued airing of it.
- An all-`NULL` override is representable and distinct: it means "play the whole
  file this once".

`Effect::Play`, `Resume`, `Arm` and `NowPlaying` all carry the override, because
the item it came off has already been consumed by the time the service runs the
effect and nothing can look it up again. `prev` returns the outgoing track to
the queue as an **item**, which is the structural reason the override survives a
prev — the renderer-owned design stripped it there, since `currentTrack` was a
`Track` and not an item.

An override is authored by the cue editor's _Use once_ (queues the track
next-up carrying the draft, never writing to the track) and by promotion from
the cue deck, but only when what is applied there differs from the radio edit.
The playlist row's marker badge clears it. See
[cue-points.md](./cue-points.md#radio-edit-vs-item-override).

## Persistence

The queue, the current item and both overrides survive a restart via
`session.json`: `SessionPlaylistItem::Track` carries `cue_override` and
`SessionState` carries `current_cue_override`, both `serde(default)` so a
session file written before overrides existed loads untouched. Restore goes
through `Effect::Resume`, which reloads and seeks without putting audio on air.

History stores tracks rather than items, so a custom airing replays under the
radio edit.

## Code map

| file                         | holds                                                 |
| ---------------------------- | ----------------------------------------------------- |
| `playlist/engine.rs`         | the state machine, `Effect`, `Transition`, `Refiller` |
| `playlist/service.rs`        | effects → deck commands, timers, snapshots, `Serial`  |
| `playlist/generate.rs`       | selection + interleave, `Interleave` cadence          |
| `playlist/model.rs`          | `PlaylistItem`, `Snapshot` — the wire types           |
| `src/features/playlist/`     | the Upcoming and History tabs                         |
| `src/shared/state.svelte.ts` | the renderer's mirror of the snapshot                 |

## Why the refactor landed first

Moving the playlist out of the renderer (`src/shared/state.svelte.ts`) was the
highest-risk change in the cue-points programme: it touched advancement,
auto-playlist refill, outage skip-to-cached and session persistence, all
load-bearing during a live broadcast. It went **before** the cue-point work
anyway, for one reason — test coverage.

`src/shared/state.test.ts` was a mature behavioural specification written
against known-good semantics: advancement order, skip-to-cached during an
outage, interleave refill, history append, stop markers. Porting that
specification across the ownership boundary was far safer while it still
described current behaviour. Landing cue points first would have meant
validating the refactor against assertions the cue editor had just churned for
air-time semantics — the weakest signal exactly where the strongest was wanted.

The acceptance criterion was that every behavioural assertion survived. They did
not run unmodified — tests that drove `AppState.playlist` directly changed shape
once the renderer became a projection — but each assertion ported across intact.
An assertion that could not be ported would have been a design problem in the
refactor, not a test to delete.

This is the general rule for the repo: a high-risk refactor goes in before the
features that will rewrite its tests.
