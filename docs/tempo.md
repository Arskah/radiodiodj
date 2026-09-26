# Measured tempo

How a track's BPM is measured, why it sits beside the tag rather than over it,
and why the estimator never sees a sample.

Designed 2026-09-26.

## Two BPMs

`tracks.bpm` is what a tagger wrote. It arrives with the scan
(`library/scanner.rs::parse_track`, `ItemKey::Bpm`), it is the operator's — or
their vendor's — reading, and nothing in the analysis pass touches it.

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
so the queue screens `bpm_version < audio::bpm::VERSION` exactly as it screens
`fingerprint::VERSION`. Bumping that constant puts the library back in the queue.

`detected_key` and `key_confidence` are declared and written by nothing. Key
detection is a separate question — see [Not built](#not-built).

## No second decode, and no PCM

The measurement rides the decode the analysis pass already performs.
`audio/waveform.rs::analyze` reads a file once and walks its samples through one
`.inspect()` closure; `audio::bpm::Collector` is the fourth consumer on that
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

`audio/bpm.rs`, three stages, no FFT and no dependency:

1. **Novelty.** The envelope in decibels, first-order difference, rectified: only
   rises count, because a decay is the previous beat ending. Measuring the rise in
   the log domain makes the curve independent of how loud the track was mastered.
   A centred moving average (`LOCAL_MEAN_FRAMES`) is subtracted so a build-up or a
   long fade does not correlate at the length of the build.
2. **Periodicity.** Autocorrelation over the lags covering 60–200 BPM, each lag
   normalised by its overlap, then combed over four harmonics
   (`COMB_WEIGHTS`). The comb is what separates a period from its own
   subdivision: a true period correlates at every multiple of itself, a
   half-period peak only at even multiples of its own lag. The winning lag is then
   refined against its neighbours by parabola, because whole lags are 6.7 BPM
   apart at 200 BPM — coarser than the tempo itself is stable.
3. **Metrical level.** Autocorrelation cannot tell 75 BPM from 150; both periods
   are really there. `PREFERRED` (85–175 BPM) breaks the tie towards how the music
   would be counted, and `OCTAVE_MARGIN` stops a genuinely fast or slow track from
   being dragged into the middle.

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
