# Backend-owned playlist

Move the playlist, advancement, and auto-playlist refill from the renderer
(`src/shared/state.svelte.ts`) into the Rust backend, making it the single
source of truth. The renderer becomes a projection that mirrors playlist state for
display.

Flagged as future work on 2026-07-21 during the network-resilience design
([#235](https://github.com/Arskah/radiodiodj/issues/235) and children), and
deliberately deferred then. The segue model adopted in
[#278](https://github.com/Arskah/radiodiodj/issues/278) is the forcing function
that un-defers it — see [program-bus.md](./program-bus.md).

## Why now

The original reasoning still holds: cache-aware advancement, prefetch, and the
byte cache all belong where the bytes live. A renderer-owned playlist forces an
id-pushing dance — `updatePrefetch` exists purely to tell the backend what the
renderer already knows.

What changed is that it stopped being optional. Handover is timed in the backend:
the worker tick loop notices the playhead crossing `nextStart` and swaps roles.
For that to work, the next item must already be loaded on the arm deck. With a
renderer-owned playlist the backend would have to ask the renderer what comes next,
putting an IPC round-trip in the middle of an audible transition.

## Why this lands first

This is the highest-risk change in the cue-points programme — it touches
advancement, auto-playlist refill, outage skip-to-cached, and session
persistence, all of which are load-bearing during a live broadcast. It
nonetheless goes **before** the cue point work rather than after, for one
reason: test coverage.

`src/shared/state.test.ts` is a mature behavioural specification written against
current, known-good semantics — advancement order, skip-to-cached during an
outage, interleave refill, history append, stop-marker handling. Porting that
specification across the ownership boundary is far safer while it still
describes today's behaviour.

Land cue points first and the picture degrades: the cue editor increment edits
those same assertions for air-time semantics, so the refactor would afterwards
be validated against tests that were themselves recently churned — weaker signal
exactly where the strongest is wanted.

**Acceptance criterion:** every behavioural assertion in the existing suite
survives. The tests will not run unmodified — the renderer becomes a projection,
so tests that drive `AppState.playlist` directly change shape — but each
assertion ports across intact, moving from renderer logic to backend logic plus
a thinner projection. An assertion that cannot be ported is a design problem in
this refactor, not a test to delete.

## Shape

The backend emits a full snapshot on every mutation and every advance:

```
program:playlist-state { playlist, current, displaced }
```

- `playlist` — the upcoming items in order, including stop markers
- `current` — the item now on the `main` deck
- `displaced` — the item that just left `main`, if any

The renderer mirrors `playlist` and `current` for display, appends `displaced` to
history, and persists the session from the snapshot. It stops computing
advancement, refill, and prefetch entirely.

A whole-snapshot event rather than deltas: the playlist is small, snapshots are
idempotent, and a dropped or out-of-order event cannot desynchronise the UI from
what is actually going to air.

### Commands

```
playlist_add(id)                    playlist_move(from, to)
playlist_add_stop_marker()          playlist_clear()
playlist_remove(index)              playlist_play_index(index)
playlist_set_auto(active)           playlist_set_item_cue_points(index, cuePoints)
```

### What moves

- **Advancement**, including the cache-aware skip-to-cached behaviour, which is
  currently split awkwardly across `planAdvance` in the renderer and
  `cache-state` events from the backend. Both halves end up on the same side.
- **Auto-playlist refill.** Interleave generation already lives in
  `playlist.rs`; the buffer/threshold loop joins it.
- **Prefetch window.** Computed backend-side from its own playlist.
  `main_deck_prefetch` and the renderer's `updatePrefetch` go away.
- **Network retry.** `scheduleNetRetry` and its backoff schedule move next to
  the cache they are retrying.

### What stays in the renderer

**History**, fed by `displaced`. It is a display log, not playback state, and
keeping it renderer-side bounds the refactor. The renderer continues to persist
it, along with its own UI state.

## Item overrides

Playlist items carry their cue-point override (see
[cue-points.md](./cue-points.md#radio-edit-vs-item-override)), so it travels with
the item through arm-load and handover automatically.

Implemented as `cue_override` on `PlaylistItem::Track`, resolved in
`PlaylistService::load_deck`. This dissolves a defect the renderer-owned design
had. `prev()` does
`playlist.unshift(trackItem(this.currentTrack))` — `currentTrack` is a `Track`,
not an item, so the override is stripped the moment a track starts playing.
Pressing prev mid-show would silently return an unedited track to the playlist.
Fixing that renderer-side required promoting `currentTrack` to a full item and
converting history to items too; with the playlist in Rust, the item simply never
stops being an item.

## Seams to resolve

The three flagged in 2026-07 are still the ones that need care:

- **`currentTrack` ownership.** The renderer reads it in roughly ten places
  across state and components. It becomes a projection of `current`.
- **History append on track-change.** Currently driven by `setCurrent`; becomes
  driven by `displaced` arriving in a snapshot. The distinction matters at the
  edges — stop, skip, and a track that fails to load.
- **Session persistence of playlist + `currentTrackId`.** Ownership splits: the
  backend owns playlist and current, the renderer owns history and UI state. Both
  halves must survive a restart and reassemble consistently.

`session.json` keeps its additive `serde(default)` convention. `SessionPlaylistItem::Track`
gained `cue_override: Option<CuePoints>` and `SessionState` gained
`current_cue_override`, both defaulted, so a session file written before
overrides existed loads untouched. `history_items` alongside `history_ids` is
still unbuilt — history stores tracks, so a custom airing replays under the radio
edit.

## Not in scope

An autonomous player that runs without the renderer attached. That becomes
_possible_ once the playlist lives in Rust, but nothing here depends on it, and the
renderer remains the only control surface.
