# Measured tempo

How a track's BPM is measured, why it sits beside the tag rather than over it,
and why the estimator never sees a sample.

Designed 2026-09-26.

## Two BPMs

`tracks.bpm` is what a tagger wrote. It arrives with the scan
(`library/scanner.rs::parse_track`), it is the operator's — or their vendor's —
reading, and nothing in the analysis pass touches it.

Two things about reading that tag, both found by measuring a real library:

- **There are two keys.** lofty splits them by precision, and a format carries
  one or the other: `ItemKey::Bpm` is the decimal field (Vorbis `BPM`, MP4
  iTunes), while ID3v2's `TBPM` is integer-only and therefore
  `ItemKey::IntegerBpm`. The scanner read only the first, so **an MP3's tempo tag
  was never read at all**.
- **`BPM=0` is not a tempo.** It is a tagger declining to answer, and 46 of the 47
  tagged files in one sampled library said exactly that. A non-positive value is
  dropped rather than stored, or the library reports a tempo of zero as though
  somebody had measured it.

`tracks.detected_bpm` is what this app measured, with `bpm_confidence` beside it
saying how much stood behind the number. Both are reported, and the tooltip shows
them as separate rows, because they disagree often enough to be worth seeing:
most libraries are part-tagged, vendor values include half- and double-time
mistakes, and a measurement is not authority enough to overwrite a file's own
metadata.

`bpm_measured_at` is what "already measured" means, not a non-null value. A
spoken-word recording measures successfully and has no tempo; keying off the
value alone would decode it again on every pass forever — the same reasoning as
`rg_measured_at` in [audio.md](./audio.md#replaygain).

`bpm_version` is what lets a later estimator disown an earlier one's work. The
marker that stops an endless re-decode would otherwise also prevent a wanted one,
so the queue screens `bpm_version < audio_measure::bpm::VERSION` exactly as it screens
`fingerprint::VERSION`. Bumping that constant puts the library back in the queue.

`detected_key` and `key_confidence` are declared and written by nothing. Key
detection is a separate question — see [Not built](#not-built).

## No second decode, and no PCM

The measurement rides the decode the analysis pass already performs.
`audio_measure/waveform.rs::analyze` reads a file once and walks its samples through one
`.inspect()` closure; `audio_measure::bpm::Collector` is the fourth consumer on that
walk, beside the loudness meter, the automatic-cue windows and the waveform
buckets.

What it accumulates is an **onset envelope** — one RMS value per 10 ms
(`ENVELOPE_MS`) — never PCM. That is 100 values per second, so a six-minute track
is ~36,000 floats, about 144 KB. A mono PCM buffer of the same track at 44.1 kHz
would be ~64 MB, and the analysis pass runs up to eight of these at once.

The envelope is enough because a beat is an energy event. The frequency content
under it carries no tempo, so the samples that would cost the memory contribute
nothing the envelope does not already hold.

This also puts tempo on the same footing as the automatic cue points: the stored
measurement, not an answer derived from it, is what the library keeps. An
envelope written to a column could be re-judged by a better estimator without
reading a file, the way `recalculate_auto_cue` re-derives markers from
`auto_cue_levels`. Today only the result is stored — the version marker requeues
a decode instead — but the shape is deliberately the same.

## The silence gate is the operator's threshold

Collection starts at the first window whose RMS clears
`tuning.autoCue.silenceDbfs` and stops `CAP_MS` (300 s) later.

Gating on that setting rather than a constant of its own is the point: "audible"
then means the same thing to the estimator as it does to the automatic cue
points. A fade-in the operator has told the app to treat as silence is not fed to
the estimator as rhythm, and a library re-tuned to a different threshold does not
have two definitions of where a track begins. The threshold is read per track,
like `store_auto_cue` reads it, so a change mid-backfill applies from the next
file on.

The cap exists because tempo is a property of a passage, not of a file. Reading
further buys nothing on a track and costs real time on a two-hour recording,
where the autocorrelation runs over the whole envelope.

## The estimator

`audio_measure/bpm.rs`, three stages, no FFT and no dependency:

1. **Novelty.** The envelope in decibels, first-order difference, rectified: only
   rises count, because a decay is the previous beat ending. Measuring the rise in
   the log domain makes the curve independent of how loud the track was mastered.
   A centred moving average (`LOCAL_MEAN_FRAMES`) is subtracted so a build-up or a
   long fade does not correlate at the length of the build.
2. **Periodicity.** Autocorrelation over the lags covering 60–200 BPM, each lag
   normalised by its overlap, then combed over four harmonics
   (`COMB_WEIGHTS`). The comb is what separates a period from its own
   subdivision: a true period correlates at every multiple of itself, a
   half-period peak only at even multiples of its own lag. The correlation is
   computed far enough that **every** candidate's harmonics exist, and each comb
   score is divided by the weight actually used — otherwise a slow candidate is
   scored on one term while its double is scored on four, and the double wins on
   count rather than on evidence. The winning lag is then refined against its
   neighbours by parabola, because whole lags are 6.7 BPM apart at 200 BPM —
   coarser than the tempo itself is stable.
3. **Metrical level.** Autocorrelation cannot tell 75 BPM from 150; both periods
   are really there. `PREFERRED` (70–175 BPM) breaks the tie towards how the music
   would be counted, and `OCTAVE_MARGIN` stops a genuinely fast or slow track from
   being dragged into the middle. The floor is 70 rather than a more comfortable
   85 because a ballad is what needs it: below the floor, halving is never a
   candidate at all.

The novelty curve is smoothed with a triangular kernel before the
autocorrelation. A beat period is rarely a whole number of frames — 174 BPM is
34.48 of them — so successive onsets straddle the grid differently, and a sharp
curve correlates with itself at even multiples of the period but not at odd ones.
Without smoothing, 174 BPM reads as 87.

Confidence is how far the winning lag stands above the mean of every candidate,
measured against the mean rather than the runner-up, because the runner-up is
usually the winner's own harmonic and says nothing about whether the track has a
pulse at all. It is **reported, never gated on**: a weak reading on a spoken-word
track is information, and the tooltip marks it weak rather than hiding it.

## What it measures

Against 14 tracks whose tempo is published (`measure_named_files`, see below),
**13 land within 2 BPM**:

|                         | measured | published |
| ----------------------- | -------- | --------- |
| Beat It                 | 139.0    | 139       |
| Billie Jean             | 117.2    | 117       |
| Thriller                | 118.4    | 118       |
| Lose Yourself           | 171.4    | 171       |
| Thunderstruck           | 133.4    | 134       |
| Highway to Hell         | 115.3    | 117       |
| Come as You Are         | 120.6    | 120       |
| Smells Like Teen Spirit | 117.1    | 117       |
| Back in Black           | 91.1     | 92        |
| The Real Slim Shady     | 104.7    | 104       |
| Hey Jude                | 73.7     | 73        |
| Come Together           | 82.4     | 83        |
| Let It Be               | 71.9     | 72        |
| **Smooth Criminal**     | **78.9** | **118**   |

Smooth Criminal is a 2:3 error at confidence 0.66 — the one case that is
confidently wrong rather than honestly unsure, and the known weakness of this
estimator. The file was checked: 4:17 at 44.1 kHz, the ordinary album edit, so the
estimator locked onto the three-against-two in that shuffle rather than the
backbeat.

On 120 tracks taken as an even spread across a 65,000-file library, every one
measured and none failed to decode. Without published tempi the check is
agreement between passages that share no audio:

|                                  |                                        |
| -------------------------------- | -------------------------------------- |
| agrees with its own first minute | 77.5% same tempo, 2.5% an octave apart |
| agrees with everything past 90 s | 73.2% same tempo, 4.5% an octave apart |
| confidence >= 0.30               | 45%                                    |
| confidence quartiles             | 0.13 / 0.28 / 0.44                     |

What it measured across that sample is a bell centred on 90–120 BPM, which is what
a mixed library should look like; a uniform spread over 60–200 would have meant it
was finding noise. One track in the sample carried a real BPM tag, and the
measurement agrees with it.

Two findings from that set are worth keeping, because both were systematic rather
than incidental:

- **An unnormalised comb prefers fast tempi.** Scoring each candidate on whatever
  harmonics happened to fit put Back in Black at 184 against a published 92, Hey
  Jude at 148 against 73, and The Real Slim Shady at 192 against 104. Dividing by
  the weight used fixed all three.
- **The preferred range's floor decides whether ballads can be halved.** With the
  floor at 85, Hey Jude, Come Together and Let It Be all read as exactly double
  their published tempo, because the true value sat below the range the tie-break
  would consider. Lowering it to 70 corrected all three and moved nothing else.

### Checking it yourself

Three ignored tests do this, none of which can ship a fixture:

- `measure_named_files` — `BPM_FILES` is a text file of paths; prints what each
  one measures from its whole window, its first minute and everything past 90 s.
- `survey_a_library` — `BPM_CORPUS` a directory, `BPM_SAMPLE` how many tracks to
  take as an even spread. Without ground truth it reports **agreement**: a reading
  from a track's opening and one from 90 s in share no audio, so when they land on
  the same tempo the estimator is measuring the music and not a passage.
- `measured_against_a_tagged_library` — scores against whatever BPM tags a library
  carries, listing its worst disagreements, since a vendor's own mistake and an
  octave error look identical in the aggregate.

Staging the audio locally first is worth it: copying a sample out of a network
share once turns every later run into a local read. `local-audio/` is excluded in
`.git/info/exclude`, which every worktree of this repo shares and nothing commits.

## Not built

- **Key detection.** The columns exist; nothing writes them. Chroma extraction
  needs an FFT, which tempo did not, so it is its own increment with its own
  dependency question.
- **A stored envelope.** Re-judging the library currently means a decode, gated by
  `bpm_version`. Storing the envelope as a blob, like `auto_cue_levels`, would
  make it a no-decode pass.
- **Tempo in rotation.** `SelectionFilter` in `library/db.rs` has no numeric
  predicates, so a tempo-aware rule (a daypart's range, or a smoother segue) means
  new fields there and a new rung in the `LADDER` — see
  [rotation.md](./rotation.md).
- **Sorting and browsing by tempo.** The measurement reaches `Track` and the
  tooltip. A sortable column needs a `SORTS` entry and a `SortColumn` member.
