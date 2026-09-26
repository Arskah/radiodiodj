# Measured key

How a track's musical key is measured, why it sits beside the tag rather than
over it, and why this is the one measurement that needs frames.

Designed and built 2026-09-26.

## Two keys

`tracks.initial_key` is what a tagger wrote. It arrives with the scan
(`library/scanner.rs::parse_track`), and nothing in the analysis pass touches it.
It is **not normalised**: ID3v2's `TKEY` holds a note name by the spec and a
Camelot code (`8A`) in practice, and some taggers write neither.

`tracks.detected_key` is what this app measured, with `key_confidence` beside it
saying how much stood behind the name. Both are reported and the tooltip shows
them as separate rows, on the same reasoning as
[tempo.md](./tempo.md#two-bpms): a measurement is not authority enough to
overwrite a file's own metadata, and the two disagree often enough to be worth
seeing.

Unlike the tag, a measured key is always one of **twenty-four note names** —
twelve pitch classes times major or minor. That is the point of storing the note
name rather than a Camelot code: it is the vocabulary a tagger uses, so the two
rows are comparable without a conversion in between.

`key_measured_at` is what "already measured" means, not a non-null
`detected_key`. A drum loop decodes successfully and has no key; keying off the
value alone would decode it again on every pass forever — the same reasoning as
`bpm_measured_at` and `rg_measured_at`.

`key_version` is what lets a later estimator disown an earlier one's work. The
queue screens `key_version < audio_measure::key::VERSION` exactly as it screens
`bpm::VERSION` and `fingerprint::VERSION`. Bumping that constant puts the library
back in the queue.

The two columns arrived a migration apart. `detected_key` and `key_confidence`
shipped with `DETECTED_TEMPO` against the day an estimator existed; the markers
that make them safe to write did not, so `DETECTED_KEY` adds them.

## Frames, not samples

Key detection is the **first consumer of the decode that needs frames**.

`audio_measure/waveform.rs::analyze` reads a file once and walks its samples
through one `.inspect()` closure. Four collectors already ride that walk — the
waveform buckets, the loudness meter, the level-envelope windows and the tempo's
onset envelope — and every one of them reduces to a sum of squares, for which an
interleaved `L,R,L,R…` stream is as good as a deinterleaved one. Order does not
matter to a mean.

A spectrum is not like that. Transforming an interleaved stereo stream as though
it were one signal reads every frequency at half its true value with a mirror
image folded on top of it. So `key::Collector` averages each channel-frame to one
mono sample before it buffers anything, and transforms mono frames.

State stays bounded, which is what keeps it affordable as a fifth consumer: one
`FRAME`-sized buffer, one window, one scratch vector and twelve accumulators. The
analysis pass runs up to eight of these at once.

## The chroma profile

A **chroma profile** is how much energy the track spent in each of the twelve
pitch classes. Per frame:

1. multiply by a Hann window, so a note's energy stays in its own bins rather
   than leaking out of the frame's edges;
2. transform (`realfft`, real-to-complex);
3. fold every bin inside the analysed band onto the pitch class of its nearest
   semitone, summing **magnitude** rather than power — squaring hands the loudest
   partial in the frame a vote several times the size of the chord under it;
4. normalise the twelve to sum to one, so a loud chorus cannot outvote a quiet
   verse, and add them to the running total.

`FRAME` is 8192 mono samples — 186 ms and a bin every 5.4 Hz at 44.1 kHz. The
shorter frames the other collectors use would blur a semitone into its neighbour
across the whole low register. `HOP` is half a frame, so a chord struck across a
frame boundary is measured whole by the next one.

The band is 130 Hz to 2100 Hz. Below it a bin is wider than the semitone it would
have to resolve, so the bass register votes for its neighbours as readily as for
itself; above it what is left is mostly upper harmonics of notes already counted
lower down, plus cymbals, which belong to no pitch class at all. A bass note
below 130 Hz is not lost — its second partial is an octave up, which is the same
pitch class.

Bin-to-pitch-class is precomputed once per track, because the alternative is a
logarithm per bin per frame and there are 4097 bins in each of them.

`CAP_MS` is 300 s, the same trade `bpm::CAP_MS` makes: a track establishes its
key in its opening minutes, and reading further buys nothing on a song while
costing real time on a two-hour recording.

## Naming the key

The profile is matched against all twenty-four keys by **Pearson correlation**
against the Krumhansl–Kessler profiles, rotated to each tonic. The winner is the
key; `None` when no candidate correlates positively.

Krumhansl–Kessler rather than a scale mask, because a scale mask scores a major
key and its own relative minor **identically** — they hold the same seven notes.
The published weights are listener ratings of how strongly each scale degree
belongs, so a profile leaning on C reads as C major where one leaning on A reads
as A minor, from the same set of pitches. `the_tonic_decides_between_relative_keys`
pins exactly that.

Pearson rather than a dot product, because a profile's own mean and spread say
nothing about which key it is; an untuned correlation would simply prefer
whichever template is largest.

Confidence is the winner's margin over the **mean of all twenty-four**,
normalised by the headroom above that mean. Against the mean rather than the
runner-up for the same reason `bpm`'s is: the runner-up is usually the winner's
own relative major or minor, whose score tracks it and says nothing about whether
the track is in a key at all. The normalisation is what stops a profile no
template fits well from reporting high confidence merely by fitting one slightly
better. Confidence is reported, never gated on.

### What it is weak at

A **bare equal-amplitude triad** is genuinely ambiguous, and the estimator says
so rather than pretending otherwise. Sounded as three sines with no bass and no
timbre, `Db F Ab` has no more claim to being D flat major than to being the upper
voices of F minor — nothing in the stimulus marks which note is the root. What
resolves it in real music is a bass note and a partial series, both of which
reinforce the root's own pitch class. This is why the module's tests synthesise
notes with harmonics over a bass root rather than bare sines: a stimulus without
them is not one any track contains.

Percussion, speech and noise have no key. They measure successfully with a low
confidence, or with no key at all, and either way the row says so.

## Camelot is presentation

The measurement reports a note name and stops. Naming that key `8A` is a DJ's
mixing convention — the Camelot wheel's neighbours are the keys that mix well —
and `audio_measure` has no business knowing what a Camelot wheel is. This is the
same boundary that puts "where the audio crosses a level" in
`audio_measure/level_envelope.rs` and "that crossing is a Cue In" in
`library/auto_cue.rs`. See [audio-measure.md](./audio-measure.md).

So `src/shared/camelot.ts` holds the twenty-four-entry map and the renderer shows
both: **`Am (8A)`**. The two notations are bijective, so deriving one from the
other loses nothing, and only one of them is stored.

Enharmonic spelling follows the wheel's own choices — `Db` not `C#`, `Bbm` not
`A#m` — so one table serves both notations instead of two disagreeing about what
pitch class 1 is called.

`camelotOf` returns `null` for a string it does not know, which is not an error:
`initial_key` carries whatever a tagger wrote, and a tag already holding `8A` is
shown unadorned rather than converted.

## Not built

- **Sorting and browsing by key.** The measurement reaches `Track` and the
  tooltip. A sortable column needs a `SORTS` entry and a `SortColumn` member —
  the same increment [tempo.md](./tempo.md#not-built) lists as unbuilt for BPM,
  and better done for both at once.
- **Harmonic rotation rules.** `SelectionFilter` in `library/db.rs` has no key
  predicate, so "follow this track with one that mixes" means new fields there
  and a new rung in the `LADDER` — see [rotation.md](./rotation.md).
- **A corpus survey.** `bpm.rs` carries an `#[ignore]`d `survey_a_library`
  measuring agreement across a real library. Key has no equivalent yet, and tags
  are poor ground truth here: most libraries are untagged, and the taggers that
  do write `TKEY` disagree about notation before they disagree about the key.

## Code map

| file                                     | holds                                               |
| ---------------------------------------- | --------------------------------------------------- |
| `audio_measure/key.rs`                   | the chroma collector, the profiles, the estimator   |
| `audio_measure/waveform.rs`              | `analyze`, where the collector rides the one decode |
| `library/db.rs`                          | the columns, `set_key`, and the queue screen        |
| `library/waveform_scan.rs`               | the pass that asks for it and stores the result     |
| `src/shared/camelot.ts`                  | note name → Camelot, and `formatKey`                |
| `src/features/track/TrackTooltip.svelte` | the _Measured key_ row                              |
