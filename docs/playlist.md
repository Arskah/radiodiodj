# Playlist — what is queued, what is on air, and how the two advance

The playlist is the ordered sequence of upcoming items that feeds the `main`
deck, plus the rules that move one item onto air and the next into place. The
**backend owns all of it**; the renderer is a projection.

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
playlist_add             playlist_add_front        playlist_insert
playlist_add_many        playlist_add_saved        playlist_add_filler
playlist_add_items       playlist_set_stop_duration
playlist_move_group      playlist_remove_group
playlist_add_stop_marker playlist_remove           playlist_move
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
[saved-playlists.md](./saved-playlists.md#appending-to-the-playlist). A Tauri command handler declared without `async` runs on the process main
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
loading the next track, which is how an operator parks the show at a fixed point
without emptying the queue. They contribute nothing to the prefetch window and
nothing to the Upcoming tab's total duration.

A Stop carries an optional planned duration, `planned_duration_seconds`: how long
the break is meant to last, set on its Upcoming row as `m:ss` by anyone, since
it changes only that queued copy (`playlist_set_stop_duration`). It is an
estimate for the producer. Nothing reads it to resume playback: the studio talks
through a Stop, and only the producer ends it. A Stop arrives with its plan from
a [saved playlist](./saved-playlists.md#stops), whole or as picked entries
(`playlist_add_items`), and leaves with it through _Save playlist as…_.

A reached marker stops the deck the way the Stop button does; telling the two
apart is planned in [prepared-shows.md](./prepared-shows.md).

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

## Groups

A saved playlist queued whole stays recognisable in the playlist: its rows are
one **group**, drawn under a header in a colour of its own, and the track on air
says which show it belongs to.

### The model

The playlist stays one flat sequence of items. An item carries an optional
`group: Option<Group>`, `Group { id, name }`:

- `id` is fresh for each insertion, so the same saved playlist queued twice is
  two groups.
- `name` is copied when it is queued, so renaming or deleting the saved original
  changes nothing queued.
- Both item variants carry it: a Stop belongs to its show as much as a track
  does.

Advancement, refill, prefetch, the outage rules and Stop consumption never read
it. A group is a label the engine stores and returns, and every decision about
which group an item is in is made by the command that inserts or moves it,
which says so: `playlist_add_front`, `playlist_insert`, `playlist_add_many`,
`playlist_add_items` and `playlist_move` take the `group` the item lands in,
`null` for none. Only the renderer can see where a row was dropped.

The engine also keeps `current_group`, the group of the track on air. It is set
wherever an item becomes the track on air — played, handed over to, retried —
and cleared by a stop, by _Play now_ and by `prev`, which returns the outgoing
track to the head of the playlist in its group. The snapshot carries it as
`currentGroup`.

### What makes one

| action                                                  | result                                                         |
| ------------------------------------------------------- | -------------------------------------------------------------- |
| **Add to playlist** on a saved playlist                 | a new group: every entry in stored order                       |
| **Add with jingles and commercials**                    | a new group, the drawn items in it                             |
| tracks or a selection, from the library or a saved list | no group, appended                                             |
| **+**, **Add N tracks**, **+ Stop**, fillers            | no group, appended after everything                            |
| **Add as next**, and the cue editor's _Use once_        | the front of the playlist, in the group on air; otherwise none |
| refill                                                  | no group, appended                                             |

**Add as next** joins the show on air so that the show stays one run. Without
it, a track added as next during a show would sit ungrouped between the track
on air and the rest of the show, and the show would draw two headers.

### In Upcoming

A header is drawn over each run of consecutive items sharing a group id. That is
all the structure there is: `features/playlist/playlistGroups.ts` derives the
rows from the flat playlist, and the engine knows nothing of headers.

```text
Standalone track
▾ ▌ John's morning show · 3 tracks · 1 stop
  ▌ Opening jingle
  ▌ Track A
  ▌ Stop · 2:00
  ▌ Track B
Standalone track
```

- **The header** is the size of a track row: a disclosure, the name, what is
  left in it, and buttons to save the group and to remove it.
- **Colour.** A stripe on the header and every row of the group, Stops
  included, indented under it. Colours come from four theme tokens, handed out
  in the order groups are first seen, so a group and the next one differ. The
  renderer keeps the assignment for as long as the window is open.
- **Collapse** hides the rows and keeps the header. It is presentation only,
  never order or playback, and held in renderer memory: after a restart every
  group is open. The disclosure is a labelled button that exposes its state.
- **On air.** The main deck shows the group's name in its colour while the track
  on air carries one. Meanwhile its header stays at the top of Upcoming, pinned
  while the list scrolls, even with nothing of it left queued. It goes when the
  last of the group leaves the deck.

A run split by a row from elsewhere is two runs and draws two headers with one
name. Both show the whole group's size, since their buttons act on the whole
group.

### Moving rows

Drops are positional, over the rows as drawn:

- A row dropped **between two rows of one group** joins it, the gap under an
  open header included — so the gap under the pinned header of the group on air
  joins that group, as **Add as next** does.
- A row dropped **at a boundary** — between two groups, or between a group and
  a standalone row — joins none. Moving a row out of a group clears its group.
- A row dropped **on a collapsed header** is appended as the group's last item,
  and the group stays collapsed. A drop before or after the header is outside it.
- **A header** drags the whole run as one, while none of it is on air
  (`playlist_move_group`). Dropped inside another group, it lands ahead of that
  group. No run of the group on air moves, and the engine refuses to move it
  too, for a drag that began before the group went on air.
- **Removing a header** removes the group's remaining rows
  (`playlist_remove_group`). The track on air plays on.

So an open group cannot be extended at its very end by one drop; a collapsed one
can. Telling an inside drop from an outside one at the same boundary is what
made a nested queue hard to use and hard to build.

### Saving it back

**Save playlist as…** takes everything queued, Stops and their plans included.
Groups are flattened.

**Save group as…**, the header's save button, takes that group's remaining rows
(`playlist_save_as` with the group's id). It is how an adjusted show is kept:
edit it in the playlist, where it is the operator's, and save it as a new list,
named after the group to start with. A group with nothing left queued is
refused rather than saved empty.

Both make a new saved playlist and are open while admin mode is locked, as
making one already is. Neither replaces an existing list; that stays an admin's
edit. Everything else here is open to everyone too, because none of it changes a
saved playlist.

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

The queue, the current item and both overrides survive a restart via
`session.json`: `SessionPlaylistItem::Track` carries `cue_override` and
`SessionState` carries `current_cue_override`, both `serde(default)` so a
session file written before overrides existed loads untouched. A
`SessionPlaylistItem::Stop` carries its `planned_duration_seconds` the same
way, and both carry their `group`, as `SessionState` carries `current_group`. Restore goes
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
