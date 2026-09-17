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
deck's marker button. Markers are placed roughly by eye, then tuned by ear, and
the layout follows that order:

```
 ┌ overview ─ whole file ── ║░░░ region ░░░║ ─── [window] ───────────┐  click = seek
 │            Cue In / Cue Out tabs ▲      ▲                           │
 ├ lane ───── ▼In ▼FI                 ▼FO ▼Out                       ─┤  flags drag
 ├ detail ─── ▒▒│▁▃▅▇▆▅▃▂▅▇█▇▅▃▂▁▃▅▆▇▆▅│▒▒  (envelope over it)       ─┤  click = seek
 │ [▶ Play] [🎧 Audition] [■] 8:55.1                     Airs 1:16     │
 │ ● Cue In   [ 8:55.152 ] ◀ ▶ ⌖ ✕   … one row per marker              │
 └───────────────────────────────────────────────────────────────────┘
```

**Two strips, always both.** The _overview_ draws the whole file from the
stored 400-bucket curve with the region shaded and the detail strip's window
outlined. The _detail_ strip zooms onto the region widened by
`max(2 s, 5 % of the region)` each side, the margin dimmed. Until Cue In or
Cue Out is set it follows the playhead in a 30-second window instead. Its curve
comes from `get_waveform_detail`, which decodes the file once on open into RMS
per 10 ms (about 120 kB for twenty minutes, taken from the prefetch cache when
the file is resident, never stored); the stored curve stands in until it
arrives. A thin line over the detail strip traces the gain envelope, a port of
`envelope::gain_at`, so what is drawn is what airs.

**A click on a curve only ever seeks.** Markers move by handles that are not
the curve: flags in a lane above the detail strip, and tabs on the region's
edges in the overview for Cue In and Cue Out. A press arms a drag without
moving anything — the handle moves only once the pointer travels 3 px, keeping
its grab offset — so a click selects a marker without nudging it. Flags that
would overlap stack into up to three lane rows. The detail frame holds still
during a drag and reframes on drop; it also reframes when a field is committed,
and when a nudge or mark carries Cue In or Cue Out out of view. Only set markers
get a flag or a line: an unset one resolves onto a neighbour.

**Two ways to listen.** _Play_ loads the whole file (`cue_load` without cue
points) and ignores the markers, so editing never interrupts it and the audio
either side of a boundary can be heard. _Audition_ loads the draft with its
trims and fades. An edit during an audition reloads it at the same file
position and keeps its play state, once a drag has dropped and typing or
nudging has paused for 250 ms — `cue_load` takes a `startAt` in air seconds for
this, and the worker clamps it. With nothing loaded, a seek parks the raw file
at that point, so there is a playhead to mark at before anything has played.

**Tuning by ear.** Each row has one time field (`m:ss.mmm`, `ss.mmm` or
`NNNms`, committed on Enter or blur; empty clears), nudge buttons, _mark at
playhead_ and clear. A row or flag click selects that marker for the keyboard:

| key               | action                                                                                   |
| ----------------- | ---------------------------------------------------------------------------------------- |
| Space             | play / pause what is loaded, raw when nothing is                                         |
| A                 | audition the draft                                                                       |
| I · O · F · G · N | mark Cue In · Cue Out · Fade In · Fade Out · Next Start at the playhead                  |
| ← / →             | nudge the selected marker 10 ms; Shift 100 ms, Alt 1 s                                   |
| P                 | pre-roll: Cue In raw from 2 s before it, anything else as an audition from 2 s before it |

Mark with nothing playing places the marker where it resolves, and a nudge on
an unset marker starts from there too.

**Input stops at a neighbour.** A drag, nudge, mark or typed value for Cue In,
Fade In, Fade Out or Cue Out stops at the nearest _set_ one of the others, and
at the ends of the file; Next Start is bounded only by the file. Unset markers
are ignored, since an unset Fade In resolves onto Cue In and would otherwise
pin it. This shapes input only — the backend's clamp (below) still decides what
is stored.

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

Two totals are built from the same figure. The Upcoming tab sums the queue
(`queueAirTime()`), each airing under its own override, and stops at the first
stop marker — the queue below one does not play unattended. The main deck
counts down `airTimeRemaining`: the rest of the track on air, plus that queue
total while Auto is advancing. The toolbar's library _Playtime_ is the one
deliberate exception: it describes library size, not anything scheduled, so it
stays file time.

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
documented `NULL` fallbacks. The editor's
[neighbour stops](#cue-editor) (`shared/cueEditor.ts`) are the one exception,
and only for input: they keep a drag from producing an order the sort would
rearrange, and never stand in for the clamp.

## Edge cases

**Moves, renames and re-adds keep cue points.** Cue points key on the track
row, and since [#373](https://github.com/Arskah/radiodiodj/issues/373) that row
outlives its file. **Prune** only marks a row missing. A file moved or renamed
is **reattached** to its row by content fingerprint, and a library path removed
and added back revives its rows by path. The radio edit is lost only when the
operator **purges** missing tracks, and the confirm step names how many of them
carry cue points. Keying cue points by `path` instead was rejected, because a
move would break it too. See [track-identity.md](./track-identity.md).

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
