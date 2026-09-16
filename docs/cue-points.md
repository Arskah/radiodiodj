# Cue points — per-track playback shaping

Non-destructive playback markers stored per track, applied automatically every
time the track airs. A song with eight seconds of intro, a long fade tail, or a
cold ending is prepped once and then always plays "radio edit" — on any deck, in
auto-playlist or manual, without ever modifying the audio file.

Implements [#279](https://github.com/Arskah/radiodiodj/issues/279). The segue
marker (`next_start_ms`) is consumed by the program bus — see
[program-bus.md](./program-bus.md).

**Status: authored and on air.** Shipped as the first three of four slices —
`MIGRATION_004` and the five columns, `audio/cue_points.rs` (clamp and
resolution), `Cmd::Load` carrying concrete `CuePoints`, the air timeline on
`<deck>:time` / `:duration` / `Cmd::Seek`, the two-stage accurate seek,
`take_duration` at the out-point, air time as the broadcast `durationSec`,
`audio/envelope.rs` applying the stored ramps, and the operator surfaces:
`CuePointOverlay.svelte`, the cue deck's _Absolute_ / _Preview_ modes, and
air-time durations everywhere a duration is shown. Clamping is backend-owned via
`set_cue_points`. Still to come: per-item overrides (the last slice) and the
confirm dialog on library-path removal described under
[Prune destroys cue points](#prune-destroys-cue-points--accepted).

## The model: five positions

```
cueIn   ≤   fadeIn   ……   fadeOut   ≤   cueOut
  │           │              │            │
  └─ ramp up ─┘              └ ramp down ─┘
 start                                   stop

                    nextStart ──► next track begins
```

Every cue point is a **position** — a millisecond offset into the file — not a
duration. The fade-in is expressed as the span `cueIn → fadeIn`, the fade-out as
`fadeOut → cueOut`.

### Superseded: fades as durations

The original #279 body specified `fade_in_ms` as "ramp up from silence over this
many ms at track start" — a duration. That is **superseded**. Two independent
sources agree on positions:

- [sakuvirtanen on #278](https://github.com/Arskah/radiodiodj/issues/278#issuecomment-5682029460):
  "assume tracks will have `cue_in_ms` and `fade_in_ms`, both representing
  full-file offset … gain gets interpolated from 0 to 1 between the two markers."
- [mAirList](https://wiki.mairlist.com/tutorials:general:getting-started:v6_3:v6_3_playlist)
  defines Fade In as "the point where full volume is reached, so you get a fade
  from Cue In to Fade In," and classifies it as a cue point alongside Cue In,
  Ramp, Fade Out, Fade End, and Cue Out. Every cue point in the product is a
  position.

Positions win on three counts. Clamping collapses to "sorted and bounded" with
no fade-sum arithmetic. Every drag handle is directly an x-coordinate on the
waveform, with no conversion on each pointer event. And an operator arriving
from mAirList, PlayIt, or RadioDJ already knows the model.

### Null semantics

All five columns are nullable. `NULL` means "no adjustment", resolved at load:

| stored          | `NULL` resolves to | meaning            |
| --------------- | ------------------ | ------------------ |
| `cue_in_ms`     | `0`                | start of file      |
| `fade_in_ms`    | `cueIn`            | no ramp up         |
| `fade_out_ms`   | `cueOut`           | no ramp down       |
| `cue_out_ms`    | file end           | play to the end    |
| `next_start_ms` | `cueOut`           | hard cut, no segue |

A resolved fade whose two positions coincide has zero width and is skipped
entirely, so "no ramp" costs nothing at playback time.

### Ordering

`cueIn ≤ fadeIn ≤ fadeOut ≤ cueOut`, all within `[0, fileEnd]`.

`nextStart` is constrained to `[cueIn, cueOut]` **independently of the fades**. A
segue may legitimately begin before the outgoing track starts fading — that is a
normal tight transition, not an error.

Clamping is therefore a sort plus a bound. There is exactly one implementation,
in Rust; see [Clamping is backend-owned](#clamping-is-backend-owned).

### Cue Out means playback stops

The term is genuinely ambiguous across playout products: in mAirList a cue-out
is the end of audio, while in PlayIt Live it is where the track _starts_ fading.

**This codebase uses mAirList's sense.** `cueOut` is where playback stops. The
point where a fade begins is `fadeOut`, and the point where the next track
begins is `nextStart`. Three distinct markers, no overloading.

## Air timeline

Everything crossing the Tauri boundary is measured **from `cueIn`**, so `0` is
the first audible sample:

- `<deck>:time` emits `pos − cueIn`
- `<deck>:duration` emits air time (`cueOut − cueIn`)
- `<deck>_seek(seconds)` takes air seconds; the worker adds `cueIn`

Only the player worker knows source-absolute positions; `seek_offset` stays
absolute internally.

The payoff is that cue points are invisible to the renderer's transport code.
`progressPct`, `NowPlaying.svelte`, `CueDeck.svelte`, and both seek bars keep
working unchanged — a trimmed track is simply a shorter track as far as they are
concerned. The only renderer-side adjustment is cropping the stored waveform
(400 buckets over the whole file) to the edit region for display.

"Air time" is also what the now-playing broadcast publishes as `durationSec`,
since that is what downstream automation schedules against.

## Resolution happens at load, in the worker

`NULL` cue-out and `NULL` fade-out both anchor to the end of the file, so
resolution needs a trustworthy file duration. The tag-derived `tracks.duration`
is nullable and wrong on VBR MP3.

The worker already computes the real value: `apply_load` does
`msg.duration.or(decoded_duration)`, falling back to `Decoder::total_duration()`
once the bytes are decoded. Resolution runs there, against that value.

If neither source yields a duration, markers anchored to the file end are
dropped with a `log::warn!` and the track plays uncut. Audio never fails because
a marker could not be resolved.

`set_cue_points` additionally clamps on write, against whatever duration the DB
holds, so the UI gets immediate feedback. The load-time resolution is
authoritative.

## Fades are source-level

Per-track fades are baked into the decoded source as a gain envelope keyed on
absolute file position. They are **deliberately not** built on the deck-level
ramp engine proposed in [#278](https://github.com/Arskah/radiodiodj/issues/278),
despite #279's body saying they would be.

That engine drives `sink.set_volume()` from the worker tick loop — a
deck-level control, correct for the live fade-out button
([#280](https://github.com/Arskah/radiodiodj/issues/280)) and for segue ramps,
because both are "from now, over N ms" operations.

Per-track fades are "at position X through position Y" operations. Routing both
through `sink.set_volume` makes them fight over one value: a live fade-out fired
while a track is inside its own stored fade-out would clobber it, and the last
writer each tick wins. As a source envelope multiplied by a sink gain, they
compose correctly at different stages — no priority rule needed.

Two further benefits: the envelope is sample-accurate rather than stepped at the
50 ms tick interval, and cue points ship without waiting for #278.

### rodio cannot express an outro ramp

Checked against rodio 0.21.1 before writing a custom source:

- `Source::fade_out(d)` is `linear_gain_ramp(input, d, 1.0, 0.0, true)` — it
  ramps from the **source's start**, not toward its end.
- `Source::linear_gain_ramp` has the same property.
- `TakeDuration::set_filter_fadeout()` is documented as "the fadeout covers the
  entire length of the take source", and the implementation confirms it:
  `sample * remaining_duration / requested_duration`. On a four-minute track
  that is a four-minute fade.

None of the three can express "ramp down over the last three seconds". Hence
`audio/envelope.rs` — an `Enveloped<I>` source of roughly sixty lines that
mirrors `TakeDuration`'s own structure: it holds `duration_per_sample`,
re-derives it when `current_span_len` is exhausted (so a mid-file sample-rate or
channel change is handled exactly as rodio handles it), tracks absolute position
from a `start_pos` offset, and multiplies each sample by `gain_at`.

`gain_at(pos, resolved) -> f32` is a pure function with five branches — before
`cueIn`, in the up-ramp, body, in the down-ramp, past `cueOut` — and is where
the envelope is unit-tested.

The alternatives were considered and rejected: splicing two stock sources with
`from_iter` decodes the file twice and puts a seek-accuracy-dependent join in the
middle of the audio; `periodic_access` stepping an `Amplify` factor produces
audible zipper noise on a slow ramp unless smoothed, at which point it is the
custom source again.

## Accurate seek

A cue point placed on a transient must sound identical on every airing.
Symphonia's MP3 seek estimates by bitrate when the file has no Xing TOC and can
land well off — tolerable for manual scrubbing, not for a stored marker.

The load path therefore does a two-stage seek: `try_seek` to roughly 200 ms
before the target, then `skip_duration` for the remainder. The landing is
sample-exact and only ~200 ms of pre-roll is decoded, rather than the whole
intro. When `try_seek` errors outright it falls back to `skip_duration` from
zero, which is what the code already did.

The same helper serves `Cmd::Seek`, so manual scrubbing gets more accurate too.

## Storage

Five nullable columns on `tracks` (migration 004), not a side table:

```sql
ALTER TABLE tracks ADD COLUMN cue_in_ms     INTEGER;
ALTER TABLE tracks ADD COLUMN fade_in_ms    INTEGER;
ALTER TABLE tracks ADD COLUMN fade_out_ms   INTEGER;
ALTER TABLE tracks ADD COLUMN cue_out_ms    INTEGER;
ALTER TABLE tracks ADD COLUMN next_start_ms INTEGER;
```

`next_start_ms` lands here despite not being consumed until the segue work, so
the program bus needs no migration of its own.

### Why columns and not a side table

A rescan must not destroy operator work. `UPSERT_TRACK_SQL`'s `ON CONFLICT … DO
UPDATE SET` clause names every column it writes, which is precisely why the
`waveform` column survives rescans — its own comment in `db.rs` says so. Cue
point columns are absent from that list and inherit the same protection.

Columns also mean cue points arrive with every `SELECT *` and land on the
existing `Track` struct. There is no second query, no renderer-side cache to
hydrate, and therefore no window in which a track could reach air with stale or
missing markers.

### Prune destroys cue points — accepted

`tracks.id` is `AUTOINCREMENT` and **Prune** hard-`DELETE`s rows whose path no
longer falls under a configured library path. Removing a library path destroys
the cue points of every track under it, and a re-scan mints new ids that cannot
reconnect.

Keying by `path` instead was considered and rejected: path is equally fragile
from the other direction, since renaming or moving a file already drops the
track from the library. That would swap one broken identity for another and
leave two to fix.

Cue points key on the track row so they inherit the eventual stable-track-identity
work for free. Until then the mitigation is a confirm dialog on the Settings
path-removal button, naming how many tracks and cue points will be lost.

### Clamping is backend-owned

`set_cue_points` clamps and **returns the clamped value**, mirroring the
established `set_tuning_config` pattern. The renderer adopts what comes back.

There is deliberately no TypeScript reimplementation of the clamp — one rule in
two languages drifts, and the authoritative clamp runs at load time against the
decoded duration, which the renderer can never see. The drag UI enforces the
only constraints it needs geometrically, since handles that cannot cross and
fades bounded by their region fall out of the pixel math anyway.

## Radio edit vs item override

Two levels, both non-destructive:

**Radio edit** — the cue points stored on the track. Apply to every airing,
everywhere.

**Item override** — cue points carried by a single playlist item, authored on the
cue deck and promoted with the track. Override the radio edit for that one
airing and never write back to the track.

Playlist items **reference** rather than snapshot: an item carries no cue points
unless explicitly overridden, so correcting a track's radio edit corrects every
queued airing of it. An item that should deliberately play the whole file
carries an all-`NULL` override, which is representable and distinct from "no
override".

Resolution is backend-side. `load` takes `Option<CuePoints>`: `None` means read
the radio edit from the track row, on the same thread that starts the load;
`Some` is an override used verbatim. Nothing on the playback path consults
renderer state, so no staleness can put an unedited track on air.

Item overrides live on backend playlist items — see
[backend-owned-playlist.md](./backend-owned-playlist.md).

## Editing

Two surfaces, sharing an interactive `Waveform.svelte` with draggable handles
over the peak curve.

**Cue deck** is the audition surface, with two modes that never mix:

- _Absolute_ (default) — loads with no cue points applied, so the waveform,
  seek bar, and time pill are all absolute and the operator can scrub the whole
  file to find an in-point.
- _Preview_ — reloads with cue points applied, switching the deck to air
  timeline and a cropped waveform, so they hear exactly what airs.

Auditioning on the cue deck never affects on-air output.

**Cue editor overlay** (`CuePointOverlay.svelte`), opened from the library row
context menu or the cue deck's marker button, gives the same waveform
full-width plus a millisecond field per marker for values a drag cannot hit.
Its _Preview on cue_ button loads the **unsaved draft** onto the cue deck —
`cue_load` takes an optional `cuePoints` precisely so a ramp can be heard
before it is committed.

Only markers that are actually set get a line on the waveform. An unset marker
has no position of its own (it resolves onto a neighbour), so drawing all five
would stack three grabbable handles on the cue-out; a _Set_ button beside the
field places one at its resolved position instead.

Dragging is a pointer convenience and the SVG stays `aria-hidden` — the
millisecond fields are the accessible way to set a marker.

Saving a radio edit while that track is on air applies from its **next** airing.
There is deliberately no main-deck equivalent of `Cmd::SetCuePoints`, so on-air
audio can never re-decode under the operator mid-broadcast.

### Naming

"Edit" in this codebase means metadata/tag editing and nothing else. The
playback markers are **cue points**, matching every playout product surveyed.
The tag-editing surfaces are renamed to say so explicitly — `MetadataOverlay`,
`editingMetadata` — so the two concepts stay unconfusable in every grep.

Note the mild overlap with the **cue deck**: a cue point is a position in a
track, the cue deck is the off-air monitoring deck. Real playout software lives
with exactly this overlap, and the cue deck is where cue points get placed.

## Edge cases

**Stale resume.** `session.json` stores the playback position in air time. If a
track's cue-out moved earlier between sessions, the stored position can fall
outside the new region. An air seek at or past the air duration **clamps to 0**
with a `log::warn!`, restarting the track.

Without the clamp the seek lands past the end, `take_duration` yields nothing,
`sink.empty()` fires immediately, and auto-advance silently skips the resumed
track at launch. Clamping to the region end instead would drop the operator into
the final second, which then ends and advances — the same outcome, less
predictably.

**Durations in the UI mean air time.** Library rows, playlist rows, the history
list, and the decks all report what actually airs, via `airDuration()` in
`shared/cuePoints.ts`. `TrackTooltip.svelte` carries both ("Airs 3:34 · File
5:02") when a track is trimmed, and a trimmed figure is tinted so the shorter
number reads as deliberate rather than as a stale tag. One meaning for the
column, and an operator filling a three-minute slot reads the number that
matters.

The renderer's optimistic duration — set the moment a track is adopted, before
the deck reports its decoded length — is air time too. Showing file time there
would make the seek bar jump the instant `<deck>:duration` arrived.

**Ending detection is unchanged.** `take_duration(cueOut − pos)` runs the sink
dry at the out-point, so the existing `sink.empty()` → `:ended` path fires
naturally. No new termination logic.
