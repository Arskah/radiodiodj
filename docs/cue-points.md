# Cue points — per-track playback shaping

Non-destructive playback markers stored per track, applied automatically every
time the track airs. A song with eight seconds of intro, a long fade tail, or a
cold ending is prepped once and then always plays "radio edit" — on any deck, in
auto-playlist or manual, without ever modifying the audio file.

Implements [#279](https://github.com/Arskah/radiodiodj/issues/279). The segue
marker (`next_start_ms`) is consumed by the program bus — see
[program-bus.md](./program-bus.md).

## The five markers

```
cueIn   ≤   fadeIn   ……   fadeOut   ≤   cueOut
  │           │              │            │
  └─ ramp up ─┘              └ ramp down ─┘
 start                                   stop

                    nextStart ──► next track begins
```

Every cue point is a **position** — a millisecond offset into the file — not a
duration. A fade is the span between two of them: the fade-in is
`cueIn → fadeIn`, the fade-out is `fadeOut → cueOut`.

All five are nullable, and `NULL` means "no adjustment":

| marker          | what it means                  | `NULL` resolves to | effect of `NULL`   |
| --------------- | ------------------------------ | ------------------ | ------------------ |
| `cue_in_ms`     | first audible sample           | `0`                | start of file      |
| `fade_in_ms`    | where the ramp up reaches full | `cueIn`            | no ramp up         |
| `fade_out_ms`   | where the ramp down begins     | `cueOut`           | no ramp down       |
| `cue_out_ms`    | where playback stops           | file end           | play to the end    |
| `next_start_ms` | where the next track begins    | `cueOut`           | hard cut, no segue |

A resolved fade whose two positions coincide has zero width and is skipped
entirely, so "no ramp" costs nothing at playback time.

**Ordering.** `cueIn ≤ fadeIn ≤ fadeOut ≤ cueOut`, all within `[0, fileEnd]`.
`nextStart` is constrained to `[cueIn, cueOut]` **independently of the fades** —
a segue may legitimately begin before the outgoing track starts fading, which is
a normal tight transition rather than an error. Clamping is therefore a sort
plus a bound, and it runs [in one place](#clamping-is-backend-owned).

**Cue out means playback stops.** The term is ambiguous across playout products:
in mAirList a cue-out is the end of audio, in PlayIt Live it is where the track
_starts_ fading. This codebase uses mAirList's sense. The point where a fade
begins is `fadeOut`, and the point where the next track begins is `nextStart` —
three distinct markers, no overloading.

## Authoring

Two surfaces share one interactive `Waveform.svelte` with draggable handles over
the peak curve.

### Cue deck

The audition surface, with two modes that never mix:

- _Absolute_ (default) — no cue points applied, so the waveform, seek bar and
  time pill are all absolute and the whole file can be scrubbed for an in-point.
- _Preview_ — the markers applied, switching the deck to the
  [air timeline](#air-time) and cropping the waveform to the aired region, so
  the operator hears exactly what airs.

Auditioning on the cue deck never affects on-air output, and both modes load
**parked**. Cueing a track stages it; the operator decides when it makes noise.
That also keeps the mode toggle quiet, since switching reloads the deck.

Markers are read-only here. The deck is where a track is heard; the editor is
where its markers move.

### Cue editor

`CuePointOverlay.svelte`, opened from a library row's context menu or the cue
deck's marker button. The same waveform full-width, plus a millisecond field per
marker for values a drag cannot hit.

Only markers that are actually set get a line. An unset marker has no position
of its own — it resolves onto a neighbour — so drawing all five would stack
three grabbable handles on the cue-out; a _Set_ button beside each field places
one at its resolved position instead. Dragging is a pointer convenience and the
SVG stays `aria-hidden`: the millisecond fields are the accessible way to set a
marker.

**Auditioning happens inside the dialog.** _Audition_ loads the unsaved draft
onto the cue deck and plays it. Play/pause and stop are in the dialog's footer,
the editor's own curve carries the playhead, and clicking the curve seeks — so
an out-point can be checked without waiting out the track. Every press of
_Audition_ reloads, because markers are applied at load time; a draft edited
mid-audition is not heard until it does.

**The dialog borrows the cue deck and gives it back.** It snapshots what the
deck was showing when it opened and restores that, parked, on every exit — a
_Preview_ re-resolved against the track's markers as they are then, so closing
after a save shows the edit that was just stored. Nothing is left armed behind a
closed dialog.

That leaves three ways out, labelled by scope, and a draft leaves the dialog
through exactly two of them:

| exit              | what it does                                                                                             |
| ----------------- | -------------------------------------------------------------------------------------------------------- |
| **Save to track** | writes the radio edit: every airing of the track, from its next one                                      |
| **Use once**      | queues the track next-up carrying the draft as an [item override](#radio-edit-vs-item-override)          |
| **Cancel**        | discards — and asks first when there are changes to lose. ×, Escape and the backdrop take the same route |

Saving a radio edit while that track is on air applies from its **next** airing.
There is deliberately no main-deck equivalent of `Cmd::SetCuePoints`, so on-air
audio can never re-decode under the operator mid-broadcast.

## Radio edit vs item override

Two levels, both non-destructive:

**Radio edit** — the cue points stored on the track. Apply to every airing,
everywhere.

**Item override** — cue points carried by a single playlist item. Override the
radio edit for that one airing and never write back to the track.

Playlist items **reference** rather than snapshot: an item carries no cue points
unless explicitly overridden, so correcting a track's radio edit corrects every
queued airing of it. An item that should deliberately play the whole file
carries an all-`NULL` override, which is representable and distinct from "no
override".

### Where an override comes from, and how it goes away

**The editor's _Use once_** is the ordinary route: it queues the track next-up
carrying the draft, and never writes to the track. A draft identical to the
radio edit deliberately carries nothing, so a later correction to the track
still reaches the queued airing.

**Promoting from the cue deck** attaches one under the same rule, for the case
where what the deck has applied differs from the radio edit. Promoting from
_Absolute_ carries nothing: auditioning the whole file is how an in-point gets
found, not a statement about how the track should air.

**Clearing** is the marker badge on the playlist row, which hands the item back
to the track's radio edit.

**Stepping back** keeps it. `prev` returns the outgoing track to the head of the
queue as an _item_, carrying whatever override it was airing under. The track
being stepped back _to_ comes off history, which stores tracks, so it replays
under the radio edit.

**Saving a radio edit** does not disturb one. Queued items hold a copy of the
track for display, so `set_cue_points` refreshes those copies; the copy under an
override is refreshed too, while what that item _airs_ is still its own markers.
The track on air is left alone, matching the rule that a radio edit applies from
the next airing.

Both the queued overrides and the one the track on air is playing under live in
`session.json`, so a custom airing survives a restart. The items themselves live
on the backend playlist — see
[backend-owned-playlist.md](./backend-owned-playlist.md).

## What airs

### Resolution

`NULL` cue-out and `NULL` fade-out both anchor to the end of the file, so
resolution needs a trustworthy duration. The tag-derived `tracks.duration` is
nullable and wrong on VBR MP3, so resolution runs in the player worker, against
`msg.duration.or(decoded_duration)` — the decoded length once the bytes are in.

An item override resolves earlier, on the thread that starts the load:
`load_deck` reads the radio edit off the track row when an item carries no
override, and uses the override verbatim when it does. Nothing on the playback
path consults renderer state, so no staleness can put an unedited track on air.

If no duration can be established at all, markers anchored to the file end are
dropped with a `log::warn!` and the track plays uncut. Audio never fails because
a marker could not be resolved.

### Air time

Everything crossing the Tauri boundary is measured **from `cueIn`**, so `0` is
the first audible sample:

- `<deck>:time` emits `pos − cueIn`
- `<deck>:duration` emits air time (`cueOut − cueIn`)
- `<deck>_seek(seconds)` takes air seconds; the worker adds `cueIn`

Only the player worker knows source-absolute positions. The payoff is that cue
points are invisible to the renderer's transport code — a trimmed track is
simply a shorter track. The one renderer-side adjustment is cropping the stored
waveform (400 buckets over the whole file) to the edit region for display.

**Every duration an operator reads is air time**: library rows, playlist rows,
the history list and both decks, via `airDuration()` in `shared/cuePoints.ts`.
`TrackTooltip.svelte` carries both ("Airs 3:34 · File 5:02") when a track is
trimmed, and a trimmed figure is tinted so the shorter number reads as
deliberate rather than as a stale tag. The renderer's optimistic duration — set
the moment a track is adopted, before the deck reports its decoded length — is
air time too, so the seek bar does not jump when `<deck>:duration` arrives.

Air time is also what the now-playing broadcast publishes as `durationSec`,
since that is what downstream automation schedules against.

### Fades are source-level

Stored fades are baked into the decoded source as a gain envelope keyed on
absolute file position (`audio/envelope.rs`), not driven from `sink.set_volume`.
They are therefore sample-accurate rather than stepped at the worker's tick
interval, and they compose by multiplication with deck-level volume: a live
fade-out fired while a track is inside its own stored fade-out attenuates it
instead of fighting it. See
[why](#why-an-envelope-and-not-sink-volume).

### Ending and handover

`take_duration(cueOut − pos)` runs the sink dry at the out-point, so the
existing `sink.empty()` → `:ended` path ends a trimmed track with no new
termination logic. `next_start_ms` is what triggers handover on the program bus.

### Accurate seek

A cue point placed on a transient must sound identical on every airing.
Symphonia's MP3 seek estimates by bitrate when the file has no Xing TOC and can
land well off — tolerable for manual scrubbing, not for a stored marker. The
load path therefore seeks in two stages: `try_seek` to roughly 200 ms before the
target, then `skip_duration` for the remainder. The landing is sample-exact and
only ~200 ms of pre-roll is decoded. The same helper serves `Cmd::Seek`, so
manual scrubbing is more accurate too.

## Storage

Five nullable columns on `tracks` (migration 004):

```sql
ALTER TABLE tracks ADD COLUMN cue_in_ms     INTEGER;
ALTER TABLE tracks ADD COLUMN fade_in_ms    INTEGER;
ALTER TABLE tracks ADD COLUMN fade_out_ms   INTEGER;
ALTER TABLE tracks ADD COLUMN cue_out_ms    INTEGER;
ALTER TABLE tracks ADD COLUMN next_start_ms INTEGER;
```

They are deliberately absent from `UPSERT_TRACK_SQL`'s `ON CONFLICT … DO UPDATE
SET` list, which is what makes them survive a rescan — the same protection the
`waveform` column relies on. Being columns also means cue points arrive with
every `SELECT *` and land on the existing `Track` struct: no second query, no
renderer-side cache, and no window in which a track could reach air with stale
markers.

### Clamping is backend-owned

`set_cue_points` clamps on write and **returns the clamped value**, mirroring
the `set_tuning_config` pattern; the renderer adopts what comes back. There is
deliberately no TypeScript reimplementation — one rule in two languages drifts,
and the authoritative clamp runs at load time against the decoded duration,
which the renderer never sees. `shared/cuePoints.ts` only ever applies the
documented `NULL` fallbacks.

## Edge cases

**Prune destroys cue points.** `tracks.id` is `AUTOINCREMENT` and **Prune**
hard-`DELETE`s rows whose path no longer falls under a configured library path,
so removing a library path destroys the cue points of every track under it, and
a rescan mints new ids that cannot reconnect. Keying by `path` instead was
rejected: a rename or move already drops the track from the library, so that
swaps one broken identity for another. Cue points key on the track row and will
inherit any future stable-track-identity work; the outstanding mitigation is a
confirm dialog on the Settings path-removal button, naming how many tracks and
cue points will be lost.

**Stale resume.** `session.json` stores the playback position in air time. If a
track's cue-out moved earlier between sessions, the stored position can fall
outside the new region, so an air seek at or past the air duration clamps to `0`
with a `log::warn!` and restarts the track. Without the clamp the seek lands
past the end, `take_duration` yields nothing, and auto-advance silently skips
the resumed track at launch.

**No duration.** Covered under [resolution](#resolution): end-anchored markers
are dropped and the track plays uncut.

## Why it is built this way

### Positions, not durations

The original #279 body specified `fade_in_ms` as a duration ("ramp up over this
many ms at track start"). Positions replaced it, backed by two independent
sources:
[sakuvirtanen on #278](https://github.com/Arskah/radiodiodj/issues/278#issuecomment-5682029460)
("both representing full-file offset … gain gets interpolated from 0 to 1
between the two markers") and
[mAirList](https://wiki.mairlist.com/tutorials:general:getting-started:v6_3:v6_3_playlist),
which defines Fade In as "the point where full volume is reached" and classifies
every marker as a cue point.

Positions win on three counts: clamping collapses to a sort plus a bound with no
fade-sum arithmetic; every drag handle is directly an x-coordinate on the
waveform; and an operator arriving from mAirList, PlayIt or RadioDJ already
knows the model.

### Why an envelope and not sink volume

The deck-level ramp engine of
[#278](https://github.com/Arskah/radiodiodj/issues/278) drives
`sink.set_volume()` from the worker tick loop, which is right for "from now,
over N ms" operations — the live fade-out button
([#280](https://github.com/Arskah/radiodiodj/issues/280)) and segue ramps.
Stored fades are "at position X through position Y" operations, and routing both
through one value makes them fight: the last writer each tick wins. As a source
envelope multiplied by a sink gain they compose at different stages, needing no
priority rule.

rodio cannot express an outro ramp on its own — checked against 0.21.1.
`Source::fade_out` and `linear_gain_ramp` both ramp from the source's _start_,
and `TakeDuration::set_filter_fadeout` fades across the entire take (a
four-minute fade on a four-minute track). Hence `Enveloped<I>`, ~60 lines
mirroring `TakeDuration`'s own structure: it re-derives `duration_per_sample`
when `current_span_len` is exhausted, so a mid-file sample-rate or channel
change is handled exactly as rodio handles it, and multiplies each sample by
`gain_at(pos, resolved)` — a pure five-branch function, which is where the
envelope is unit-tested. Splicing two stock sources with `from_iter` would
decode the file twice and put a seek-dependent join mid-audio; `periodic_access`
stepping an `Amplify` factor zippers audibly on a slow ramp.

### Naming

"Edit" in this codebase means metadata/tag editing and nothing else. The
playback markers are **cue points**, matching every playout product surveyed,
and the stored set of them is a **radio edit**. The tag-editing surfaces say so
explicitly — `MetadataOverlay`, `editingMetadata` — so the two stay unconfusable
in every grep.

Note the mild overlap with the **cue deck**: a cue point is a position in a
track, the cue deck is the off-air monitoring deck. Real playout software lives
with exactly this overlap, and the cue deck is where cue points get placed.
