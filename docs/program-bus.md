# Program bus — multi-deck output and handover

The on-air audio path: a mixer summing an arbitrary number of decks into the
main output device, with decks taking turns holding the `main` role. Replaces
the fixed one-deck-per-output model that shipped with the cue deck.

Adopted from
[sakuvirtanen's proposal on #278](https://github.com/Arskah/radiodiodj/issues/278#issuecomment-5682029460).
Supersedes the deck model in
[audio-backend-and-cue-deck.md](./audio-backend-and-cue-deck.md). Consumes the
`next_start_ms` cue point defined in [cue-points.md](./cue-points.md).

## Why

The original plan for track-to-track transitions was a crossfade of configured
duration, triggered near the end of the outgoing track (#278 as originally
written). That is too simple a model for radio segues: the right moment for the
next track to start is a property of _this_ song — where its outro becomes
uninteresting — not a station-wide constant.

The replacement is an explicit `nextStart` marker per track, which in turn
requires two decks producing audio simultaneously, which requires a mixer. Once
there is a mixer, the deck count stops being structural, and soundboards and
transition sweepers become possible without another rewrite.

## Structure

rodio's model maps onto this directly: one `OutputStream` per physical device,
and any number of `Sink`s connected to that stream's `mixer()`.

```
                    ┌───────────────────────────────┐
  Deck A ─── Sink ──┤                               │
                    │  Program bus                  ├──► main output device
  Deck B ─── Sink ──┤  (one OutputStream + mixer)   │
                    │                               │
      … ─── Sink ──┤                               │
                    └───────────────────────────────┘

  Cue deck ─ Sink ──►  separate OutputStream ───────►  cue output device
```

**The cue deck stays off the bus.** It is monitoring on a different physical
device, not program audio. It keeps its own stream, its own lazy spawn, and its
own `cue:*` events.

**One worker thread drives all program decks.** A single tick loop advances
every deck, so handover needs no cross-thread coordination and `nextStart`
scheduling stays local to the loop that already tracks playhead position. This
replaces thread-per-`PlayerHandle` for program decks; the cue deck keeps its
own thread.

The self-healing output logic from
[#259](https://github.com/Arskah/radiodiodj/issues/259) — deferred load,
`open_retry_due` pacing, `output-unavailable` emitted only on transitions —
moves up to bus level and is shared by every deck on it. A device that is
absent, briefly held, or renamed at launch still cannot permanently disable
playback.

Per-deck sink volume is left free, so #278's ramp engine can drive it for live
fades ([#280](https://github.com/Arskah/radiodiodj/issues/280)) and segue ramps
without touching the source-level track envelope. The two compose by
multiplication — see [cue-points.md](./cue-points.md#fades-are-source-level).

## Landing it as a pure restructure

The bus replaces thread-per-deck with one worker driving a shared mixer. That is
a large internal change with no user-visible effect, so it ships **before** the
cue point work and separately from handover, with roles held static — deck A
permanently `main`, no deck B in play.

**Acceptance criterion: the existing test suite passes unmodified.** Nothing
about playback behaviour changes, so any test that needs editing is a signal the
restructure altered something it should not have. Handover, which does change
behaviour, is a later increment.

## Roles, not identities

```rust
enum DeckSlot { A, B }      // physical deck on the bus
enum DeckRole { Main, Arm } // what it is doing right now
```

**Roles move between decks; decks do not move between roles.** `main` is the
on-air deck whose output defines Now playing. `arm` is loaded and waiting to take
over.

This is a language change, not just a type: "Main deck" is now a role, not a
thing. `CONTEXT.md` reflects that.

`Vec<DeckState>` is what makes the deck count incidental. v1 constructs two.

## Handover

Assume track _Foo_ is playing on deck A.

| moment          | what happens                                                     |
| --------------- | ---------------------------------------------------------------- |
| steady state    | A is `main`, B is `arm` with the next playlist item loaded       |
| `Foo.nextStart` | B starts playing and becomes `main`; A becomes `arm`             |
| overlap         | both decks sum on the bus; A plays on toward `Foo.cueOut`        |
| `Foo.cueOut`    | A stops and is vacated; the following playlist item loads onto A |

During the overlap the outgoing track is still audible on air but no longer
drives the UI or the broadcast. That is correct: the moment the new track starts
is the moment it is on air, which is exactly what an operator and a listener
both perceive.

**Fallbacks.** `nextStart` null resolves to `cueOut`, which makes handover a hard
cut — today's behaviour, unchanged, for every track nobody has prepped.

**No handover across a stop marker.** A stop marker in the playlist is a deliberate
hard stop; segueing past it would defeat the point (#278 acceptance criterion).

## Events and commands

Events stay **role-mapped**, per sakuvirtanen's constraint that the frontend,
the now-playing webhook, and everything else continue to refer to `main`:

| topic           | meaning                                                        |
| --------------- | -------------------------------------------------------------- |
| `main-deck:*`   | emitted for whichever deck currently holds the `main` role     |
| `arm-deck:*`    | same shape, for the armed deck — lets the UI show what is next |
| `program:roles` | slot → role plus the track in each; debugging and future UI    |
| `cue:*`         | unchanged; the cue deck is not on the bus                      |

On handover, `main-deck:time` and `main-deck:duration` are **re-emitted
immediately** for the incoming track, so the renderer flips cleanly rather than
waiting for the next tick.

The consequence is that `NowPlaying.svelte`, the broadcast service, and every
existing `main-deck:*` listener keep working with no changes. Exposing
`deck-a:*` / `deck-b:*` and making the renderer resolve roles was considered and
rejected on exactly that cost.

`main_deck_*` transport commands target whichever deck holds `main`.

**Broadcast fires on handover**, backend-side, carrying air time as
`durationSec`. Today the renderer triggers it via `main_deck_load`; moving it to
the handover moment makes it correct during a segue, when the on-air track
changes without a load having just happened.

## Relationship to the playlist

Handover is timed by the backend, which means the backend must be able to load
the next item onto the arm deck at the right moment. That is what forces playlist
ownership into Rust — see
[backend-owned-playlist.md](./backend-owned-playlist.md).

## Scope notes

- The mixer summing N decks is the structural change; soundboard and sweeper
  decks are follow-on work that needs no further restructuring.
- Exclusive output mode remains deferred for the reasons recorded in
  [audio-backend-and-cue-deck.md](./audio-backend-and-cue-deck.md); the bus does
  not change that analysis, though it does mean an exclusive program output
  would be negotiated once for the bus rather than per deck.
