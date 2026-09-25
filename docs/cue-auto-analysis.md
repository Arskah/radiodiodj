# Automatic cue analysis

Automatically derive useful Cue points from decoded audio so that unprepared library material still behaves like radio material.

This extends [cue-points.md](./cue-points.md). It does not change the five-point Cue point model, Handover semantics, the Program bus, or the Cue editor design established there and in [program-bus.md](./program-bus.md).

The intended operating assumption is that **almost all library tracks will never be manually cue-prepped**. Automatic analysis therefore needs to produce conservative starts and ends, but useful default music segues.

No source audio file is modified.

The detector (landed 2026-09-19, [#372](https://github.com/Arskah/radiodiodj/issues/372)) is `src-tauri/src/audio/auto_cue.rs`, fed by the RMS windows `audio/waveform.rs` collects during the existing waveform decode and committed by `library/waveform_scan.rs`. Ownership and provenance live on the track row as `auto_cue_state` (`pending` / `auto` / `manual`), `auto_cue_version`, `auto_cue_silence_db`, `auto_cue_segue_db` and `auto_cue_at`. The switches and the thresholds are `tuning.autoCue` in `config.json`, under _Settings → Advanced_.

## The switches

`tuning.autoCue.apply` decides whether a derived trio takes effect. `tuning.autoCue.applyNextStart` sits under it and decides whether the derived Next Start alone takes effect. Both are on by default.

Switched off, **analysis still runs and still stores its result**. What changes is the answer the library gives: a row whose `auto_cue_state` is `auto` reports no Cue In, Cue Out or Next Start, so the deck airs the whole file and every duration in the app — queue rows, tab totals, the waveform crop, the cue editor — measures the whole file with it. Nothing is cleared, invalidated or re-decoded, so switching back on takes effect immediately.

The nested switch holds back the Next Start and nothing else: the trims still apply, so durations do not move, and the deck hands over at Cue Out instead of overlapping the incoming item. It is the answer to "trim my library, but do not segue my music" — trimming a leading silence is a safe mechanical edit, while an automatic Next Start is a taste call that overlaps the last 500 ms of every cold-ending song. There is no switch the other way round: the Next Start is derived from the effective Cue Out, so it has no meaning without the trims, and `apply` off hides all three regardless.

Both switches gate at one place, `effective_cue_points` in `library/db.rs`, so Handover, the air timeline, the cue deck, the cue editor and the library-health report all follow from the library's answer. Nothing in the program bus knows either switch exists — a `NULL` Next Start already resolves to Cue Out, which is a hard cut.

A track whose state is `pending` is held back with the `auto` ones. A requeue — a rescan of a changed file, a reclassification, a move between roots — keeps the trio it was last given while the state goes back to `pending`, and that trio is still analysis's work. Only an operator save reaches `manual`, so a row that is not `manual` holds nothing of theirs.

Three things are never held back:

- a manually owned trio. The switches are about automatic analysis; a radio edit the operator made is theirs either way, including a Next Start they authored by hand;
- the fades. Nothing infers them, so `fadeIn` and `fadeOut` apply whether the switches are on or off;
- an item's own Cue point override, which is a per-airing decision the operator made.

Ownership is judged against what the caller was shown, not against the stored row (`Db::set_cue_points`), and a save hands back what the caller will be shown next — a hidden marker stays hidden, a manual one comes straight back. Nobody can clear markers they were never given: saving a fade while either switch is off leaves the derived trio intact for when it comes back on. To clear a derived trio deliberately, switch the feature on first. A fade saved while a switch is off is still bounded by the Cue Out it is stored against, hidden or not, so it does not end up past it where the load-time resolve would drop it.

Moving the trio is different. It is the operator taking it over, and it is judged whole: a save that moves Cue In or Cue Out while the derived Next Start is hidden takes the row to `manual` and stores the `NULL` the operator was shown over it. The derived position is gone, and turning the nested switch back on does not bring it back — the row is theirs now and analysis no longer touches it. That is the intended reading: `NULL` means the handover waits for Cue Out, which is exactly what the switch asked for.

Flipping either switch re-reads every copy of a track the app holds — the library rows, the queue, the library-health report, the cue deck and an open editor — and re-arms the next track — markers are applied at load time, so the deck already holding it has to load it again. The track on air keeps what it started with, exactly as a radio edit saved mid-broadcast does.

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

## The level envelope

The decode is reduced to one byte per RMS window: the number of whole dBFS levels in `-100..=-3` that window is strictly above. That is about 20 bytes per second of audio, and it captures the decode exactly rather than approximately — everything the detector does is arithmetic on where the codes cross a level, plus the window count and the decoded duration.

A code is defined by the comparison the detector itself makes, not by converting an amplitude back to decibels: `rms > amplitude(level)` holds for a prefix of the ascending levels, and the code is the length of that prefix. There is no logarithm in the path, so there is no rounding that could put a window on the wrong side of a threshold.

That envelope is stored on the track row as `auto_cue_levels`, written by `set_auto_cue` in the same statement as the trio, so a reader never sees markers without the measurements they came from. It exists so that re-deriving a track's markers under different thresholds costs a SQL read rather than a decode: the windows themselves live only for the duration of `waveform::analyze`, and the stored waveform curve is normalised, so nothing else on disk can answer what level a passage was at.

Thresholds are rounded to whole decibels on the way into `config.json` for the same reason — a fractional one would fall between two levels, and the envelope would stop answering it exactly.

Keeping the measurement rather than a table of answers is deliberate. A table of the three crossings the detector asks about today would be smaller and fixed-width, but only those three questions could ever be put to it. A later rule that wants a crossing sustained for some duration — the usual fix for a click at the head of a file setting Cue In early — or a level after a given position, or the loudest passage, can be written against a stored envelope. Against a table it would need a fresh decode of the whole library. The table is derivable from the envelope in one pass; the reverse is not.

An envelope this build cannot read — a different layout version, a length too short to hold the header, or a code outside the resolved range — counts as missing, and the track is queued for a fresh decode rather than having markers derived from bytes that cannot be trusted. The first two screens run in SQL, so an upgrade does not decode every blob in the library to find out. The third cannot: a readable length is a function of the track's duration, so `Envelope::decode` is the authority, and the one caller that reads a stored envelope queues whatever fails it rather than trusting the SQL screen and the decoder to agree.

Three rules follow from what the table measures:

- a **manually owned** track never gets one. The table feeds automatic derivation, and nothing derives for a track the operator owns;
- a **fingerprint twin** inherits it, across content types as well as within one. It measures the audio, which the twin shares, and carries no decision about either class — unlike the trio, which does not cross classes;
- a **changed file** drops it. It measures audio that is no longer there, and deriving from it would produce markers for a file that has been replaced. The trio stands until a fresh result lands, which is the existing rule.

Existing libraries get the table by backfill through the ordinary analysis pass, not by a synchronous migration: it is a full decode per track. A track picked up for its table alone keeps its markers — re-deriving them there would be exactly the implicit mass re-analysis the next section rules out.

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

Recalculating existing automatic cues is a separate explicit maintenance operation — **Recalculate now**, under the two thresholds in _Settings → Advanced_. See [Re-analysis](#re-analysis).

That future operation can use stored algorithm/threshold provenance to identify which automatically generated Tracks were analyzed using different settings.

Mass re-analysis is never an implicit side effect of editing a setting.

## Re-analysis

**Recalculate now** applies the current thresholds to material already in the library. It is the only thing that does: nothing else re-analyses anything, by design.

It operates only on Tracks whose automatic Cue point set is not manually owned, and replaces the automatically managed trio together:

```text
Cue In
Cue Out
Next Start
```

according to the Track's current content type and current automatic-analysis settings. For commercial and jingle Tracks this means Next Start becomes `NULL`.

A separate future action may deliberately discard manual ownership and regenerate a Track, but ordinary re-analysis must never do so implicitly. The button counts the manually owned Tracks it skipped, so the number is visible rather than merely absent.

Two paths, partitioning the non-manual Tracks by whether the row carries a [level envelope](#the-level-envelope) this build can read:

| the row has         | what happens                                                     | cost                          |
| ------------------- | ---------------------------------------------------------------- | ----------------------------- |
| a readable envelope | the trio is re-derived from it and written with fresh provenance | a SQL read and arithmetic     |
| no usable envelope  | the row goes back to `pending` for the analysis pass             | one decode, in the background |

The second path also clears a recorded decode failure, so a Track that failed against a share that has since come back is tried again rather than staying invisible to the pass until a scan sees its file change.

A missing Track is in whichever path its row puts it, exactly like a present one. The envelope is the whole input to the first path, so re-deriving a Track whose share is unmounted costs nothing and needs no file; skipping it would strand it at the old thresholds for good, because a reattached Track comes back automatic and the pass never looks at it again. A missing Track with no usable envelope is queued and waits there — the pass passes it over until the file is back, which is the only moment a decode could reach it.

Which path a Track takes is settled by the decoder, not by the SQL screen. SQL rejects an absent blob, one too short to hold the header, and a layout version this build does not know; it cannot check the codes, because a readable length depends on the Track's duration. A blob that passes the screen and fails to decode is therefore queued like one that has no envelope at all, so no Track can fall between the two paths and keep stale markers with nothing coming back for it.

The stored fades are not derived, but they are sorted against the trio that just moved, the same way a fresh analysis sorts them. A fade left outside the new window would be folded onto a marker by the load-time clamp, so the row — and the cue editor — would show a ramp that no longer plays.

A queued Track keeps the markers it has until the fresh result lands — old positions beat none. Its new trio arrives as `cue-points-ready`, one Track at a time, exactly as a first analysis does.

The operation is refused while a library scan is running, as a purge is: a scan rewrites the same rows underneath it.

What the operator is told is what it did — how many were re-derived and at which levels, how many were queued, how many radio edits were left alone — not how far it got. A Track that was already waiting for the pass is not counted: it had nothing for this operation to move. Its recorded decode failure is still cleared, since that is the only thing keeping the pass away from it. The re-derived ones are done when the button returns; the queued ones are the analysis bar's business.

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

A Track's content type follows the Library path the file sits under, and changes in three ways, all of which requeue an automatically owned trio:

- a missing Track reattaches by fingerprint under a Library path of another content type — the operator moved the file from `/music` to `/jingles`. The trims stand until the fresh result lands, being the same audio either way, but the Next Start is cleared: it is derived only for music, and a jingle carrying one would hand over early on every airing until the pass reached it;
- the same file appears under a second Library path of another content type, which inserts a new Track that copies the twin's row. The copy is a Track of its own class, so it is queued for its own analysis rather than inheriting the twin's result — including when the twin is manually owned, since that was a decision about the other Track;
- `update_track_metadata` carries a content type. No UI sends one today — the metadata overlay edits tags only — and a rescan of the file would take the root's type back, so this is a backend affordance rather than an operator feature.

## Source-file changes

If the scanner detects that the underlying audio file has changed:

- an automatically owned Cue point set should be scheduled for fresh analysis;
- a manually owned Cue point set must be preserved.

Do not clear the old automatic values before replacement analysis succeeds.

If the new file cannot be decoded, retaining the previous values is preferable to destroying known-working playout metadata. Load-time validation from `cue-points.md` remains authoritative if the previous positions no longer fit the new file.

## Atomic writes and races

An automatic analysis result is one logical update.

Cue In, Cue Out, Next Start, provenance and analysis version must be committed together rather than as independent writes.

Before committing, the backend must re-check that the Track is still automatically owned, **still the content type the analysis ran under, and still the file that was decoded** (its `mtime`) — a reclassification mid-decode would otherwise land a music Next Start on a jingle and mark it analysed, and a file replaced mid-decode would land markers derived from audio that is gone, dropping the rescan's requeue for good.

A result discarded on either ground leaves the Track queued, so the pass takes it again — under its new class, or from the file that is there now.

Example race:

```text
background analysis starts
        ↓
operator saves a Radio edit, or reclassifies the Track
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
apply:              on
apply Next starts:  on
silence threshold: -70 dBFS
segue threshold:   -20 dBFS
```

All four are persisted settings. The segue threshold stays editable while automatic Next Starts are switched off — analysis derives and stores the position either way, so the level it works to is still live.

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
