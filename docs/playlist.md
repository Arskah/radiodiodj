# Playlist — what is queued, what is on air, and how the two advance

The playlist is the ordered sequence of upcoming standalone items and copied
saved-list blocks that feeds the `main` deck, plus the rules that move one item
onto air and the next into place. The **backend owns all of it**; the renderer
is a projection.

| topic                                   | detail                                     |
| --------------------------------------- | ------------------------------------------ |
| which tracks the auto-playlist may pick | [rotation.md](./rotation.md)               |
| how a queued item reaches air           | [program-bus.md](./program-bus.md)         |
| the markers an item can carry           | [cue-points.md](./cue-points.md)           |
| stored, named lists                     | [saved-playlists.md](./saved-playlists.md) |

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
- **`timing.rs`** — the pure schedule projection, retaining one origin and shared
  delay across normal playback and handovers.
- **`service.rs`** — wiring. Turns effects into deck commands, play-count
  increments, prefetch-window updates and retry timers, and emits a snapshot at
  the end of every transition.

### The snapshot

```
program:playlist-state { playlist, currentBlockId, current, history }
```

- `playlist` — top-level standalone items and blocks in order
- `currentBlockId` — the block whose child is on air or holding at a Stop,
  including a trailing Stop
- `current` — the item now on the `main` deck
- `history` — the last `tuning.autoPlaylist.historyCap` (100) aired tracks
- `holdingStop` — the consumed Stop's plan, elapsed hold and block association
- `timing` — occurrence-keyed base ETAs and one shared moving or frozen delay
- `revision` — the sequence keeping queue and timing from older snapshots out

A whole snapshot on every mutation, never deltas: the playlist is short,
snapshots are idempotent, and a dropped or out-of-order event cannot
desynchronise the UI from what is actually going to air. `history` is carried
whole for the same reason; it is a display window over `play_log`, not a
retention limit — every airing stays logged. See
[rotation.md](./rotation.md#the-airing-log).

A `PlaylistItem::Track` carries the whole `Track`, not just its id, so the
snapshot is displayable without a second round-trip to resolve titles. Each
standalone occurrence, block and block child also has a stable occurrence id;
track ids alone cannot distinguish repeated entries.

### Commands

```
playlist_add             playlist_add_front        playlist_insert
playlist_add_many        playlist_copy_tracks      playlist_add_as_next
playlist_add_saved       playlist_add_filler       playlist_add_stop_marker
playlist_remove          playlist_remove_occurrence playlist_move_occurrence
playlist_move_block      playlist_remove_block     playlist_move
playlist_clear           playlist_play_index       playlist_play_now
playlist_next            playlist_prev             playlist_stop
playlist_set_auto_advance                          playlist_set_auto_playlist
playlist_set_source
playlist_set_item_cue_points                       playlist_sync
```

`playlist_save_as` is not in that list: it reads the playlist and writes a saved
playlist, and changes nothing here.

Every one of them but `playlist_sync` **queues** its transition and returns at
once. `playlist_add_saved` does too, but reads the saved playlist first and
answers with what it found — see
[saved-playlists.md](./saved-playlists.md#adding-to-the-playlist). A Tauri command handler declared without `async` runs on the process main
thread, which is the thread the window is drawn from, and a transition is not
cheap: a refill under the queue's threshold is tens of milliseconds of SQL on a
large library, and it waits on the same database lock the analysis pass holds, so
the wait is not bounded by its own query. Skipping tracks froze the UI for as
long as that took, while the audio — driven from the bus worker, off a file
already resident in RAM — started on time. What the operator saw was the next
track playing before the deck redrew.

The queue is one thread, not the async runtime's pool, because the order
commands are applied in is part of what they mean: `playlist_remove`,
`playlist_move`, `playlist_insert` and `playlist_add_many` carry queue indices, and a pool would let
the second of two clicks overtake the first and act on positions that no longer
exist.

Ordering is not the same as freshness, and the queue only buys the first. An
index still describes the snapshot the renderer had on screen when the operator
clicked, and that snapshot now arrives when the queue drains rather than inside
the command — so two quick clicks on the same row's neighbour can still act on
positions that have shifted under them. Closing that means the renderer echoing
a snapshot generation back with every index-bearing command and the engine
refusing a stale one; until then the window is wider than it was.

`PlaylistService::drain` is what shutdown calls. A queued transition owes work
that outlives the process — `Effect::TrackPlayed` is what writes the airing log
rotation reads back — so `RunEvent::ExitRequested` waits for the queue before the
broadcast goes quiet, bounded, because shutdown is no place to wait on a database
lock the analysis pass happens to hold. The renderer's own `session.json` flush
still runs before that wait, so a transition queued at the moment of quitting can
leave the saved session one transition behind.

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
exception. `purge_tracks` is there for the same reason: `Health::refresh` emits
its report inline, and the playlist answers that event with a full transition.

That is now the rule for every command that does I/O rather than a property of
these three — see [architecture.md](./architecture.md#commands) for where the
line falls and why a blocking body in an `async` command is not the same thing.

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

History keeps the same rule from the other end. A track joins it when it leaves
the deck **having aired**, so one whose load failed or was given up on is in
neither record, and `prev` — which reads the newest history entry — steps back
to something the listeners heard.

## Modes

**Auto advance** (AUTO / MANUAL) decides whether the end of a track pulls the
next one. In MANUAL the deck simply stops; `on_ended`, `on_load_failed` and arm
reconciliation all return empty transitions.

**Auto-playlist** keeps the queue full by itself. It maintains a lookahead
buffer of `tuning.autoPlaylist.autoPlaylistBuffer` (20) items and refills when
fewer than `autoPlaylistThreshold` (5) remain. The refill runs inside
`engine`, so a refill and the track change that triggered it are one transition
— there is no window in which the queue is observably empty.

Music comes from the whole music library unless a saved playlist is chosen as
the **source**, which narrows the music pick to that list and changes nothing
else: `playlist_set_source`, and
[saved-playlists.md](./saved-playlists.md#as-the-auto-playlists-source).

Which tracks may be picked is [rotation.md](./rotation.md): a title window, an
artist window, and one track per artist within a generated block.

**Interleave** weaves non-music into a generated block: one jingle every
`tuning.interleave.jingleEvery` (4) music tracks and one commercial every
`commercialEvery` (8). Only music advances the counters, so a manually queued
jingle does not shift the cadence.

**Stop markers** are items too. Advancement consumes one and stops instead of
loading the next track, leaving later items for the operator to resume or
remove. A Stop inside a block lets that block park after its preceding track
finishes at Cue Out; a trailing Stop also prevents the next top-level item from
starting. Stops contribute nothing to prefetch or duration. There is no
implicit Stop at the end of a prepared show.

A Stop's optional `plannedDurationSeconds` estimates the presenter's pause.
Blank is unspecified; zero is an explicit plan. Upcoming and saved prepared-show
rows accept whole-second `m:ss` values, including minutes above 59. Editing a
queued copy does not change its saved original. A plan never resumes playback.

Consuming a Stop enters **Holding at stop**, retaining its plan and active block
after its row leaves Upcoming. The deck shows elapsed minus planned time, then
steady **Over plan** overtime; an unplanned hold shows elapsed time alone. The
consumed Stop is read-only. Play and Next retain their transport meanings, while
Stop clears the hold. Upcoming edits do not end it.

## Start-time estimates

Upcoming displays local wall-clock ETAs beside durations, including for Stop
arrival. Blocks add no time, and collapsing them changes no estimate. Auto
uses Next Start between tracks and Cue Out before a Stop; Manual assumes the
presenter starts each next track at Cue Out. An audible tail constrains the next
overlap so estimates never require three simultaneous tracks. Unavailable audio
is skipped; unknown required timing hides later estimates. An upcoming unplanned
Stop exposes its own arrival and hides all later ETAs until reached.

The backend retains one schedule across ordinary position events, automatic
consumption and normal handovers. Fractional spans accumulate before absolute
ETAs are truncated once. Pauses, idle transport and overdue holds move every
known ETA by one shared delay; resuming freezes it without changing labels.
Planned holds allow that delay to grow only after their deadline. Queue edits,
cue changes, seeks, early skips and real output or loading interruptions can
revise the affected schedule. The renderer uses one sampled clock for the
toolbar, hold counter and moving delay, without per-second playlist transitions.
See [playlist-eta.md](./playlist-eta.md) for the timing contract and acceptance
cases. Planned pauses never contribute to unattended audio coverage.

## Playlist blocks

Adding a saved playlist as a whole creates a **playlist block** in Upcoming.
The block copies the saved title, kind and ordered entries; it is independent
from the original and from other copies. Editing, renaming or deleting the saved
list does not change a queued block. Tracks added individually stay standalone.
See [saved-playlists.md](./saved-playlists.md#adding-to-the-playlist) for
which actions create a block.

The snapshot keeps blocks as top-level nodes with their remaining children.
Playback, prefetch, outage handling and air-time calculations traverse those
children in playback order and continue through the same playlist engine.
Headers add no audio or duration. Auto-playlist refill counts remaining
children but appends generated tracks outside every block; a queued Stop remains
a barrier to refill.

Upcoming shows each block with a header and its remaining track count. Expanded
children are indented; a disclosure button collapses or expands them. Collapse
changes only presentation. The active header remains visible while its child is
on air, through the last child, and while holding at any of its Stops. It clears
when that hold or the block finishes; an outgoing audible tail does not keep it
active.

Drops between top-level entries place a whole block or standalone item there.
Indented gaps place tracks among a block's children. Dropping on a
collapsed header appends inside it; a line before or after it stays outside.
Moving a queued track removes its former occurrence. An unstarted block can be
moved as a unit; the active block stays pinned. **Add as next** puts a run of
tracks before the active block's remaining children, even while collapsed or
while its last child is on air. With no active block it inserts at the main
front. Adding a track from the library or saved-list entries and **+** appends
standalone tracks outside all blocks.
None of these adds starts or resumes playback.

**Nothing in the Upcoming tab reaches air by a double-click.** A queued track
goes on by its row's play button, and a stop marker only by the show arriving at
it. A double-click used to do both — it played the track, and on a stop marker
it stopped what was on air — which is the stray-click hazard the library row's
_Play now_ was moved behind a menu to remove. A double-click on a History row
queues that track again, at the end: it adds, and never reaches air.

## Outages

The playlist is where a dead network share becomes a survivable event rather
than dead air.

- **Skip to cached.** When a load fails, the failed id is dropped from the
  cached set (so stale membership cannot spin an instant retry loop), the track
  is marked for retry, and advancement looks for the next item whose bytes are
  already resident in RAM — see [audio.md](./audio.md#the-prefetch-cache). A
  failure naming a track that is no longer the one on air is ignored: it is
  about a load that has since been superseded.
- **Retry.** `ArmRetry(n)` selects a delay from
  `tuning.autoPlaylist.netRetryBackoffsMs` (1 s, 2 s, 5 s, saturating at the
  last). This is the case where nothing queued is resident either, so the
  failed track stays on the deck: a fresh `cache-state` naming it is what makes
  it a candidate again, and it goes back on when the share returns.
- **A slow load costs a track its turn, not its place.** `{role}:load-failed`
  says whether the load was **abandoned** — a time bound gave up on a read that
  is still running — or the read itself failed. When advancement moves past an
  abandoned track it returns to the head of the playlist, with its override.
  That puts it back in the prefetch window, which is the only reason the
  abandoned read's bytes are kept when they land, and it goes on at the next
  track change. A track whose read errored, or whose row was purged, is
  dropped: asking again would fail again, and it would sit at the head being
  skipped until someone removed it. Only advancement returns a track — an
  operator who presses next or stop while one is waiting has dropped it.
- **Neither is an airing.** A track that never reached the deck is not in
  history and not in the airing log, whichever way it left.
- **Missing tracks.** `on_missing_state` takes ids from the same
  `library-health` event the health report uses, and advancement drops them up
  to the next stop marker, even on a cold cache. A track an admin hid is in
  that set too, so it is skipped the same way. See
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
next-up carrying the draft, never writing to the track) and by adding the cue
deck track to the end of the playlist, but only when what is applied there
differs from the radio edit.
The playlist row's marker badge clears it. See
[cue-points.md](./cue-points.md#radio-edit-vs-item-override).

## Persistence

`session.json` stores the structured queue, including standalone items, blocks,
their stable occurrence ids and remaining child order. It also stores the active
block association, renderer-owned collapsed block ids, the current item and both
cue overrides. Restoring a Stop preserves its position. An absent structured
queue identifies a legacy session, whose flat `playlistItems` / `playlistIds`
are restored as standalone items; an explicitly empty structured queue remains
empty. Old sessions restore paused, as structured sessions do. Track entries
carry `cue_override`, and `SessionState` carries `current_cue_override`, both
`serde(default)` so older files load. Restore goes through `Effect::Resume`,
which reloads and seeks without putting audio on air.

A held Stop saves its plan, block association and elapsed time. Restore starts
counting from that elapsed value; time with the app closed is excluded. Neither
ETAs nor an active playback clock are persisted, and restoring never starts
audio.

History stores tracks rather than items, so a custom airing replays under the
radio edit.

## Code map

| file                         | holds                                                 |
| ---------------------------- | ----------------------------------------------------- |
| `playlist/engine.rs`         | the state machine, `Effect`, `Transition`, `Refiller` |
| `playlist/service.rs`        | effects → deck commands, timers, snapshots, `Serial`  |
| `playlist/generate.rs`       | selection + interleave, `Interleave` cadence          |
| `playlist/model.rs`          | `PlaylistItem`, `Snapshot` — the wire types           |
| `playlist/timing.rs`         | occurrence-keyed ETA projection and shared delay      |
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
