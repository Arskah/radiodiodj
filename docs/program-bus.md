# Program bus — multi-deck output and handover

The on-air audio path: a mixer summing an arbitrary number of decks into the
main output device, with decks taking turns holding the `main` role. Replaces
the fixed one-deck-per-output model that shipped with the cue deck.

Adopted from
[sakuvirtanen's proposal on #278](https://github.com/Arskah/radiodiodj/issues/278#issuecomment-5682029460).
Supersedes the fixed main-deck/cue-deck pair the original audio backend
shipped with. Consumes the `next_start_ms` cue point defined in
[cue-points.md](./cue-points.md); the decode path and output devices underneath
it are [audio.md](./audio.md).

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

**Status: landed.** `audio/bus.rs` owns the mixer and the worker; `audio/deck.rs`
holds a deck and the loop that ticks a set of them; `audio/output.rs` holds the
shared self-healing open; `audio/cue.rs` is the off-bus cue deck reusing the
same deck worker on its own stream. Roles are static (A `main`, B `arm` and
idle — B connects no sink until handover gives it something to play), and
`program:roles` is emitted but not yet consumed. The 176 backend and 132
renderer tests passed unmodified; four new tests cover role routing and the
role snapshot. Next: handover.

## Roles, not identities

```rust
enum DeckSlot { A, B }            // physical deck on the bus
enum DeckRole { Main, Arm, Tail } // what it is doing right now
```

**Roles move between decks; decks do not move between roles.** `main` is the
on-air deck whose output defines Now playing. `arm` is loaded with the next
playlist item and waiting to take over. `tail` has already handed over and is
playing the outgoing track out — audible, but no longer Now playing.

`tail` exists because the role is what addresses a deck's events, and an
outgoing track is neither on air nor next. Folding it into `arm` would make
`arm-deck:*` mean "what is next" before a handover and "what is dying" after
one.

This is a language change, not just a type: "Main deck" is now a role, not a
thing. `CONTEXT.md` reflects that.

`Vec<DeckState>` is what makes the deck count incidental. v1 constructs two.

## Handover

Assume track _Foo_ is playing on deck A.

| moment          | what happens                                                      |
| --------------- | ----------------------------------------------------------------- |
| steady state    | A is `main`, B is `arm` with the next playlist item loaded        |
| `Foo.nextStart` | B starts playing and becomes `main`; A becomes `tail`             |
| overlap         | both decks sum on the bus; A plays on toward `Foo.cueOut`         |
| `Foo.cueOut`    | A is vacated, becomes `arm`, and the following item loads onto it |

During the overlap the outgoing track is still audible on air but no longer
drives the UI or the broadcast. That is correct: the moment the new track starts
is the moment it is on air, which is exactly what an operator and a listener
both perceive.

### Who decides, and who times it

The playlist engine **authorises** a handover; the bus worker **times** it.

Arming is the authorisation. The engine computes a desired arm target after
every transition and, when it differs from what the arm deck holds, loads it.
The target is `None` — nothing armed, so no handover is possible — whenever
auto-advance is off, nothing is on air, the next item is a stop marker, or a
tail deck is still draining. Every policy question is therefore answered at arm
time, and the worker needs no playlist knowledge beyond "swap when
`pos >= next_start`".

The alternative — the worker reporting the marker and the engine commanding the
swap — was rejected because the swap would then slip a tick plus a channel hop
behind the marker, and would have to race a sink that is about to empty.

An uncached track is armed anyway: the arm-load **is** the early read, which is
most of the value on a slow share. A late or failed one simply means no
handover fires.

**The swap is its own latch.** A deck hands over at most once per load for free:
the moment it does, it holds `tail`, and only the `main` deck is ever tested. A
per-deck "already handed over" flag was written into this design and then found
to be unreachable state — nothing makes a tail deck `main` again without a
`Load`, which resets everything anyway.

A handover that could not fire at `next_start` — nothing was armed yet — fires
as soon as one is ready, with a shorter overlap than authored. That is the
graceful degradation of a late arm-load, and strictly better than the hard cut
it replaces.

No command authorises a handover beyond the arm-load itself. The worker's test
is "is a deck armed, decoded and parked", which the engine controls by arming or
disarming, so a separate `ArmHandover` would carry no information the arm deck's
own readiness does not.

### Vacating a tail

A tail is vacated at its own `cueOut`, **or when the deck that took over from it
ends, whichever comes first**. A tail belongs to the track that displaced it and
dies with it, so at most two tracks are ever audible. Without that rule a 3 s
sting handing over with 10 s of tail left would leave the original playing under
its second successor — which is exactly what automatic analysis
([cue-auto-analysis.md](./cue-auto-analysis.md)) produces on a long outro
followed by a short item.

### Two decks, and what that costs

During an overlap no deck is armed: one is `main`, one is `tail`. The following
item is loaded only once the tail is vacated. So a second handover is possible
only if that arm-load completes between the vacate and the incoming track's own
`next_start` — minutes for music, negative for a chain of stings. **Consecutive
short items segue at most every other item**, falling back to a hard cut.

Force-vacating the tail to free a deck at the moment one is needed is not an
option: arming is a file read, so it cannot happen on demand. Giving a deck a
second queued sink is the three-deck design under another name. Three decks is
the way out if this ever matters; it needs no restructuring, which is the point
of the bus.

### Transport during an overlap

**Any explicit operator action that changes or stops what is on air also cuts
the tail.** Only reaching its own `cueOut` lets a tail finish.

- **Stop** cuts main and tail. Stop means silence.
- **Pause** pauses both and resumes both — pausing only main would leave a tail
  audible under a paused transport. Both decks pause on the same tick, so their
  relative offset survives.
- **Next, prev, play-index, play-now** hard-cut on the main deck as before, tail
  included. Making Next fire the armed handover instead was considered and
  rejected: during an overlap nothing is armed, so the hard-cut path exists
  regardless, and one behaviour for Next beats two. The armed read is not
  wasted — the item is inside the prefetch window, so re-loading it onto main is
  a cache hit.
- **Seek** acts on the incoming track, which is what `main` means.

**Fallbacks.** `nextStart` null resolves to `cueOut`, which makes handover a hard
cut — today's behaviour, unchanged, for every track nobody has prepped. The tick
then fires as the sink empties, the tail runs dry within a tick, and the result
is audibly identical to a hard cut with one code path instead of two: **handover
wins whenever one was armed and ready, and `main-deck:ended` advances only when
it was not.**

**No handover across a stop marker.** A stop marker in the playlist is a deliberate
hard stop; segueing past it would defeat the point (#278 acceptance criterion).
Nothing is armed ahead of one, so nothing can fire.

**Manual mode.** Auto-advance off arms nothing, so no handover happens.

### Landing handover

Three increments. The `Tail` role deliberately arrives with the code that
constructs it rather than ahead of it.

1. **Arm-load.** The engine's arm target and the effects that keep it honest,
   the split of the service's deck load so an arm-load sets no broadcast pending,
   and an ordinary parked `Load` on the arm deck. **No audible change** — the arm
   deck loads and sits silent, visible only in `program:roles`. The existing
   suite passes unmodified.
2. **Handover.** The `Tail` role, the swap on the tick, `program:handover` and
   the engine's reconcile, both vacate rules, transport cutting the tail, and
   broadcast's explicit on-air input. Indivisible:
   handover without the vacate rules is broken audio, and without the reconcile
   the playlist double-plays.
3. **Tail indication.** One line in `NowPlaying.svelte` while a tail is audible
   — the outgoing title and its remaining time, from `tail-deck:time` and the
   role snapshot. Without it, "why is the old song still playing" is a support
   ticket. No armed badge (the Upcoming list already shows the item and
   `program:roles` carries it), no handover countdown, and no new tuning: an
   overlap can never exceed the outgoing track's air time, and a station-wide
   maximum would be the configured-duration crossfade this document rejects
   above.

The trigger follows `watchdog_timed_out`'s precedent in `audio/deck.rs` — a
pure `handover_due(pos, next_start, playing, arm_ready)` fed the position,
unit-tested with no audio device, with the worker loop as a thin caller.
`tail_companion(cmd)` is pure for the same reason.
Everything else is an ordinary state-machine test in `playlist/engine.rs`.

## Events and commands

Events stay **role-mapped**, per sakuvirtanen's constraint that the frontend,
the now-playing webhook, and everything else continue to refer to `main`:

| topic               | meaning                                                        |
| ------------------- | -------------------------------------------------------------- |
| `main-deck:*`       | emitted for whichever deck currently holds the `main` role     |
| `arm-deck:*`        | same shape, for the armed deck — lets the UI show what is next |
| `tail-deck:*`       | same shape, for a deck playing an outgoing track out           |
| `program:roles`     | slot → role plus the track in each; debugging and future UI    |
| `program:handover`  | the `main` role moved: outgoing and incoming track ids         |
| `program:faded-out` | a fade to silence finished on air; the playlist stops          |
| `cue:*`             | unchanged; the cue deck is not on the bus                      |

On handover, `main-deck:time` and `main-deck:duration` are **re-emitted
immediately** for the incoming track, so the renderer flips cleanly rather than
waiting for the next tick.

`program:handover` is what the playlist engine reconciles against: it consumes
the queued item, counts the airing, and produces the `displaced` track the
renderer appends to history — the same bookkeeping `main-deck:ended` does today.

The consequence is that `NowPlaying.svelte`, the broadcast service, and every
existing `main-deck:*` listener keep working with no changes. Exposing
`deck-a:*` / `deck-b:*` and making the renderer resolve roles was considered and
rejected on exactly that cost.

`main_deck_*` transport commands target whichever deck holds `main`.

**Broadcast fires on handover**, backend-side, carrying air time as
`durationSec`. Moving it to the handover moment makes it correct during a segue,
when the on-air track changes without a load having just happened.

That needs one honest input rather than a second inference. Broadcast state
fires now-playing when `main-deck:pause-state` goes false with a pending track,
and the pending track is set at load time. An arm-load must therefore **not**
set it: the incoming track would become pending minutes early, and the operator
pausing and resuming the _outgoing_ track would broadcast the wrong one. So the
main-deck load keeps setting pending, the arm-load does not, and handover sets
and commits the incoming track explicitly — the swap's own pause-state event
still carries the already-fired outgoing track and is swallowed by the existing
dedupe.

## Live fades

Two operator transport actions ride on the bus: **Fade out** ramps the on-air
deck to silence and stops it, and **Fade to next** starts the next item now and
fades the outgoing track out underneath it.

The only new audio primitive is a **deck gain ramp**: `Cmd::Fade { to, ms,
on_complete }`, stepped from the same 50 ms tick that times handover. It is
runtime-only and never persisted, and it does not replace the operator's deck
volume — it multiplies it (`Deck::effective_volume`), so a sink rebuilt mid-fade
resumes at the faded level and the volume survives the fade. Every command that
changes what a deck is doing cancels the ramp and restores full gain, which is
what guarantees the next track on that deck starts at the level the operator
left. The curve is linear, matching `envelope::gain_at`.

**A fade-out ends as a Stop, not just a silent deck.** When a ramp with
`RampDone::Stop` completes on the `main` role the worker announces
`program:faded-out`, and the playlist runs the same `stop()` the Stop button
does: the track goes to history and `current` clears. Without that the engine
would go on believing a silent deck was playing, and the next Play would resume
a deck with nothing on it. So Play after a fade-out starts the next queued item,
exactly as it does after Stop. A tail fading out under an incoming track is not
that, and announces nothing.

This is deliberately a _deck_ control, where the stored fades of
[cue-points.md](./cue-points.md) are a _source_ envelope. The two compose by
multiplication rather than fighting over one value, so a live fade fired inside
a track's own stored fade-out attenuates it further with no jump in level.

**Fade to next is handover fired early.** `Cmd::HandOverNow` is the one command
that acts on two decks, so the worker intercepts it in its dispatch loop instead
of applying it to one: it calls the same `hand_over` the tick does at
`next_start`, then ramps the deck that just became the tail down to silence. The
playlist engine needs no special case at all — it reconciles against
`program:handover` exactly as it does for an automatic segue.

When there is nothing armed and decoded to hand over to, or a tail is already
playing (three audible tracks is not something the bus mixes), the ramp
completes with `RampDone::EndTrack` instead: the deck emits `{role}:ended`, and
the playlist advances under the rules it already applies at the end of any
track — advancing in Auto, stopping in Manual. That decision is the worker's,
because only it knows what is decoded on which deck this tick.

A fade aimed at `main` takes any tail with it, ramping both and cutting the tail
when the ramp completes. Cutting it up front would leave it silent while `main`
was still audible, which is not what the operator asked for.

Durations come from `tuning.player.fadeOutMs` and `fadeToNextMs`, read per press
from the stored config rather than the `PlayerTuning` the bus captured at spawn,
so a change in Settings applies to the next press.

## Relationship to the playlist

Handover is timed by the backend, which means the backend must be able to load
the next item onto the arm deck at the right moment. That is what forces playlist
ownership into Rust — see
[playlist.md](./playlist.md).

## Scope notes

- The mixer summing N decks is the structural change; soundboard and sweeper
  decks are follow-on work that needs no further restructuring.
- Exclusive output mode remains deferred for the reasons recorded in
  [audio.md](./audio.md#exclusive-output); the bus does not change that
  analysis, though it does mean an exclusive program output would be negotiated
  once for the bus rather than per deck.
