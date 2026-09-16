# Automatic cue analysis

Automatically derive useful Cue points from decoded audio so that unprepared library material still behaves like radio material.

This extends [cue-points.md](./cue-points.md). It does not change the five-point Cue point model, Handover semantics, the Program bus, or the Cue editor design established there and in [program-bus.md](./program-bus.md).

The intended operating assumption is that **almost all library tracks will never be manually cue-prepped**. Automatic analysis therefore needs to produce conservative starts and ends, but useful default music segues.

No source audio file is modified.

## What is inferred

Automatic analysis writes only these Track-level Radio edit points:

| Content type | Cue In | Cue Out | Next Start |
| ------------ | ------ | ------- | ---------- |
| `music`      | yes    | yes     | yes        |
| `commercial` | yes    | yes     | no         |
| `jingle`     | yes    | yes     | no         |

Fade In and Fade Out are never inferred automatically.

A manually authored Next Start remains valid for any content type. The table above governs only automatic analysis.

`NULL` retains the semantics from `cue-points.md`:

- `cue_in_ms = NULL` → file start
- `cue_out_ms = NULL` → file end
- `next_start_ms = NULL` → Handover waits until Cue Out

So analysis can legitimately finish with one or more `NULL` results.

## Analysis input

The analyzer works from decoded PCM samples.

It divides audio into fixed **50 ms windows** and computes RMS amplitude for each window. RMS is converted to dBFS against normalized full-scale PCM.

The 50 ms window is intentionally fixed in v1. It is short enough to avoid visibly coarse cue placement while keeping the detector simple and resistant to individual-sample transients.

Two thresholds are user-configurable:

- **Silence threshold** — default `-70 dBFS`
- **Segue threshold** — default `-20 dBFS`

The segue threshold must be greater than the silence threshold.

The two thresholds answer different questions:

- Silence threshold: “Is there meaningful programme audio here at all?”
- Segue threshold: “Has this music track become quiet enough that the next item may sensibly begin?”

Changing either threshold affects future analysis only. It does **not** automatically invalidate or recalculate any existing library material.

## Cue In

Cue In is derived from the first window whose RMS is greater than the configured silence threshold.

The stored position is the **beginning of that 50 ms window**.

```text
file start

silence  silence  silence  first active window  audio...
                           │
                           └─ Cue In
```

If the first window is already above threshold, store `NULL` rather than `0`.

That preserves the existing meaning:

```text
NULL cueIn = file start
```

The detector deliberately biases toward starting too early rather than too late. A small amount of retained silence is preferable to removing the beginning of programme audio.

## Cue Out

Cue Out is derived from the last window whose RMS is greater than the configured silence threshold.

The stored position is the **end of that 50 ms window**.

```text
...audio  last active window  silence  silence  file end
                         │
                         └─ Cue Out
```

If active audio reaches physical EOF, store `NULL` rather than the physical duration.

That preserves:

```text
NULL cueOut = file end
```

If no window in the file exceeds the silence threshold, both Cue In and Cue Out remain `NULL`.

The analyzer does not attempt to distinguish a genuinely silent file from material mastered below the configured threshold.

## Music Next Start

Only `music` receives an automatically generated Next Start.

The goal is deliberately tighter than simple sequential playback:

- a natural fade should hand over once the outgoing song has become quiet enough;
- a cold-ending song should still allow the next item to start slightly before the final hit has completely disappeared;
- an unusually long fade must not cause an absurdly early Handover.

Resolve first:

```text
effectiveCueIn  = cueIn  ?? 0
effectiveCueOut = cueOut ?? fileEnd
```

### Level-based candidate

Find the **last 50 ms window whose RMS is at or above the configured segue threshold**.

The end of that window is the level-based Next Start candidate.

Example:

```text
level

full ──────────────────────╲
                            ╲
                             ╲  -20 dB
                              ╲ │
                               ╲│
                                ╲.......... -70 dB
                                             │
                                             └─ Cue Out

                               ↑
                    level-based Next Start
```

For a normal fading song, this is the preferred handover point.

### Cold-ending candidate

Music also receives a candidate exactly **500 ms before effective Cue Out**:

```text
effectiveCueOut - 500 ms
```

This is intentional.

A song that remains loud until its final hit should still normally segue rather than force the next item to wait for complete silence.

```text
music ───────────────────────────────●
                               ↑     ↑
                         Next Start  Cue Out
                           500 ms
```

The next item beginning under the final fraction of a second of the outgoing song is an accepted radio default.

Commercials and jingles do **not** receive this rule because they receive no automatic Next Start at all.

### Candidate selection

When both candidates exist, use the **earlier** one:

```text
rawNextStart =
    min(
        levelBasedCandidate,
        effectiveCueOut - 500 ms
    )
```

If no level-based candidate exists but valid programme audio was detected, the 500 ms candidate still applies.

### Maximum segue lead

An automatic Next Start may not be more than **6 seconds before Cue Out**.

```text
earliestFromEnd = effectiveCueOut - 6000 ms
```

This prevents an unusually long quiet fade from launching the next item far too early.

### Protect the beginning of very short material

Automatic Next Start must also be at least **500 ms after effective Cue In**:

```text
earliestFromStart = effectiveCueIn + 500 ms
```

The lower bound is therefore:

```text
lowerBound =
    max(
        effectiveCueIn + 500 ms,
        effectiveCueOut - 6000 ms
    )
```

Final result:

```text
nextStart =
    clamp(
        rawNextStart,
        lowerBound,
        effectiveCueOut
    )
```

If the effective playable duration is `<= 500 ms`, leave Next Start `NULL`.

The automatic analyzer must never produce a Next Start before Cue In or after Cue Out.

## Examples

### Cold-ending music

```text
Cue In:     0:00.100
Cue Out:    3:30.000

Next Start: 3:29.500
```

The song remains loud to the end, so the fixed 500 ms music candidate wins.

### Normal fade

```text
Cue Out:             3:30.000
last >= -20 dB:      3:25.000
500 ms candidate:    3:29.500

Next Start:          3:25.000
```

The fading song hands over once its level has dropped below the segue threshold.

### Very long fade

```text
Cue Out:             3:30.000
last >= -20 dB:      3:20.000
6 s lower bound:     3:24.000

Next Start:          3:24.000
```

The raw level-based candidate is too early, so the six-second maximum lead wins.

### Commercial

```text
Cue In:      inferred
Cue Out:     inferred
Next Start:  NULL
```

Automatic Handover waits until Cue Out.

### Jingle

Same as commercial:

```text
Cue In:      inferred
Cue Out:     inferred
Next Start:  NULL
```

A manually prepared jingle may still carry a non-NULL Next Start.

## Deliberately simple heuristics

The analyzer does not attempt to understand musical structure.

It does not detect:

- beats;
- bars or phrases;
- vocals;
- hooks;
- spoken endings;
- hidden tracks;
- intentional silence inside a song;
- a “better” fade curve;
- whether a final transient is musically important.

For example:

```text
song
30 s silence
hidden sound
```

will likely place Cue Out after the hidden sound if it exceeds the silence threshold.

Likewise, a cold-ending song may have its final syllable or hit overlapped by the incoming item for roughly 500 ms.

Both are accepted consequences of a deliberately small, predictable algorithm. Manual Radio edits remain the escape hatch for exceptional material.

## Background analysis

Automatic cue analysis belongs in the existing heavy audio-analysis path, not the fast metadata scan.

Waveform generation already requires a full decode of the source. Newly imported material should therefore be decoded once and used for both:

```text
decoded PCM
    ├─ waveform
    └─ automatic cue analysis
```

Do not add a second full-file decode when both results are missing.

Metadata import remains fast, and playback never waits for cue analysis.

A Track may therefore temporarily exist with no automatic analysis. Until analysis completes, the ordinary `NULL` Cue point semantics apply.

Existing libraries receive automatic cue data through background backfill rather than a synchronous migration.

Analysis failure is non-fatal:

- Track import still succeeds;
- the Track remains playable;
- existing Radio edit values are not destroyed;
- failure of one file does not stop the analysis job.

## Automatic vs manual ownership

Automatic analysis must never overwrite a Radio edit that the operator has taken ownership of.

The Track therefore needs persisted automatic-analysis state in addition to the five Cue points.

At minimum the state must distinguish:

- never successfully analyzed;
- automatically analyzed;
- manually owned.

A manual change to any of these Track-level points:

- Cue In;
- Cue Out;
- Next Start;

makes the automatic trio manually owned.

Explicitly clearing one of those points to `NULL` also counts as a manual change. `NULL` may express intentional operator behavior and must not be interpreted as “missing automatic result”.

Changing Fade In or Fade Out does **not** disable automatic ownership of Cue In / Cue Out / Next Start.

An Item override also does **not** affect Track-level automatic ownership. It is one-airing Playlist state and never changes the Track's Radio edit.

## Analysis provenance

Persist enough provenance to identify how an automatically generated result was produced.

The required information is:

- automatic-cue algorithm version;
- silence threshold used;
- segue threshold used, when applicable.

For commercials and jingles the stored segue threshold may be `NULL`, because no automatic Next Start analysis occurred.

The fixed v1 constants:

- 50 ms RMS window;
- 500 ms cold-ending music lead;
- 6 second maximum music segue lead;
- 500 ms minimum distance from Cue In;

belong to the algorithm version and do not need separate per-track fields.

The purpose of threshold provenance is inspection and future **explicit** recalculation. It must not itself trigger background work.

## Settings changes do not re-analyze the library

Changing either automatic-analysis threshold:

```text
-70 dBFS → -65 dBFS
```

does not:

- clear existing Cue points;
- mark the entire library stale;
- launch background decoding;
- change manually prepared Radio edits.

New automatic analyses use the new settings.

Recalculating existing automatic cues is a separate explicit maintenance operation.

That future operation can use stored algorithm/threshold provenance to identify which automatically generated Tracks were analyzed using different settings.

Mass re-analysis is never an implicit side effect of editing a setting.

## Re-analysis

Explicit automatic re-analysis operates only on Tracks whose automatic Cue point set is not manually owned.

Re-analysis replaces the automatically managed trio together:

```text
Cue In
Cue Out
Next Start
```

according to the Track's current content type and current automatic-analysis settings.

For commercial and jingle Tracks this means Next Start becomes `NULL`.

A separate future action may deliberately discard manual ownership and regenerate a Track, but ordinary re-analysis must never do so implicitly.

## Content-type changes

Content type changes affect automatic inference.

For an automatically owned Track:

```text
music → commercial
music → jingle
```

requires fresh analysis under the new content-type rules. In particular, the automatically generated music Next Start must no longer remain active.

Likewise:

```text
commercial → music
jingle → music
```

requires fresh analysis so that a music Next Start can be generated.

For a manually owned Radio edit, a content-type change leaves Cue In, Cue Out and Next Start untouched.

A manually authored Next Start therefore survives a later reclassification to jingle or commercial.

## Source-file changes

If the scanner detects that the underlying audio file has changed:

- an automatically owned Cue point set should be scheduled for fresh analysis;
- a manually owned Cue point set must be preserved.

Do not clear the old automatic values before replacement analysis succeeds.

If the new file cannot be decoded, retaining the previous values is preferable to destroying known-working playout metadata. Load-time validation from `cue-points.md` remains authoritative if the previous positions no longer fit the new file.

## Atomic writes and races

An automatic analysis result is one logical update.

Cue In, Cue Out, Next Start, provenance and analysis version must be committed together rather than as independent writes.

Before committing, the backend must re-check that the Track is still automatically owned.

Example race:

```text
background analysis starts
        ↓
operator saves a Radio edit
        ↓
background analysis finishes
```

The completed automatic result must be discarded.

The operator's Radio edit wins.

Cancellation behaves similarly: already committed Tracks stay complete, and Tracks that have not committed remain unchanged.

## Persistence limitation

Automatic and manual Cue points inherit the Track-row lifecycle described in [cue-points.md](./cue-points.md).

The row survives moves, renames and removing then re-adding a Library path, and is deleted only by an explicit purge (see [track-identity.md](./track-identity.md)). Automatic-cue ownership and provenance live on the same row, so they move with the Cue points rather than inventing a separate identity system.

## Configuration

The automatic-analysis settings belong with the application's other playback/library tuning:

```text
silence threshold: -70 dBFS
segue threshold:   -20 dBFS
```

Both are persisted settings.

Validation must at least guarantee:

```text
segue threshold > silence threshold
```

Changing a value affects future analysis only, as described above.

The fixed v1 constants are intentionally not user-facing.

## Scope

This document specifies only automatic generation and lifecycle of existing Cue points.

It does **not** change or define:

- the five-point Cue point model;
- Cue point clamping/resolution;
- source-level fade envelopes;
- Cue editor UX;
- Item overrides;
- the Air timeline;
- Program bus structure;
- Deck-role handling;
- Handover implementation;
- backend Playlist ownership.

Those are defined by the existing DJ-tools design documents.

Automatic analysis only supplies better default Track-level Radio edits for those systems to consume.
