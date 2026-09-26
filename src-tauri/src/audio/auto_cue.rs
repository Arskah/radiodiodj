//! Automatic derivation of Cue In, Cue Out and Next Start from decoded audio.
//!
//! Pure: no device, no file, no decoder. The analysis pass feeds it the RMS
//! windows collected during the waveform decode ([`super::waveform::analyze`]),
//! so an automatic cue never costs a second pass over the file.
//!
//! The algorithm is deliberately small — level thresholds over fixed 50 ms
//! windows, and nothing else. It does not detect beats, phrases, vocals or
//! hidden tracks; manual radio edits are the escape hatch for material it gets
//! wrong. See `docs/cue-auto-analysis.md`.

/// Width of one RMS window. Fixed in v1: short enough to place a marker without
/// visible coarseness, long enough that a single transient cannot move one.
pub const WINDOW_MS: i64 = 50;

/// Identifies the rules below. Stored per track so a future explicit
/// re-analysis can tell which tracks were produced by an older detector. Bump
/// it whenever a change here would move a marker.
pub const ALGORITHM_VERSION: i64 = 1;

/// How far before Cue Out a cold-ending music track hands over. The next item
/// beginning under the final fraction of a second is an accepted radio default.
const COLD_END_LEAD_MS: i64 = 500;

/// Cap on how early an automatic Next Start may be, so an unusually long quiet
/// fade cannot launch the next item far too soon.
const MAX_SEGUE_LEAD_MS: i64 = 6_000;

/// Floor on the distance from Cue In, so very short material is not segued over
/// before it has been heard.
const MIN_AFTER_CUE_IN_MS: i64 = 500;

/// The two operator-configurable levels, in dBFS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Below this there is no programme audio at all. Bounds Cue In / Cue Out.
    pub silence_dbfs: f64,
    /// Below this a music track has become quiet enough to hand over.
    pub segue_dbfs: f64,
}

/// The trio automatic analysis owns. `None` carries the same meaning it does on
/// the track row: file start, file end, and "wait until Cue Out".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoCue {
    pub cue_in_ms: Option<i64>,
    pub cue_out_ms: Option<i64>,
    pub next_start_ms: Option<i64>,
}

/// RMS amplitude per [`WINDOW_MS`] of audio, collected while the waveform pass
/// walks the decoded samples.
#[derive(Clone, Debug, Default)]
pub struct RmsWindows {
    /// Linear RMS (0.0..=1.0 for normalised PCM), one entry per window. The
    /// final entry may cover less than a whole window.
    pub rms: Vec<f32>,
    /// Decoded length of the file. Read from the sample count rather than the
    /// tag, which is wrong on VBR MP3.
    pub duration_ms: i64,
}

/// Accumulates [`RmsWindows`] from a sample stream.
pub struct Collector {
    /// Interleaved samples per millisecond: `rate × channels / 1000`. Kept as a
    /// float because it is not whole at every rate.
    samples_per_ms: f64,
    /// Sample count at which the window being filled closes.
    boundary: u64,
    samples: u64,
    sum_sq: f64,
    count: u64,
    rms: Vec<f32>,
}

impl Collector {
    /// `channels` and `rate` come from the decoder header. A degenerate header
    /// (either zero) yields a collector that produces no windows, so the caller
    /// simply gets no automatic cue rather than a bogus one.
    pub fn new(rate: u32, channels: u16) -> Self {
        let mut c = Self {
            samples_per_ms: f64::from(rate) * f64::from(channels) / 1000.0,
            boundary: 0,
            samples: 0,
            sum_sq: 0.0,
            count: 0,
            rms: Vec::new(),
        };
        c.boundary = c.next_boundary();
        c
    }

    /// Where the next window ends, computed from the window index rather than
    /// by adding a fixed width. 22.05 kHz puts 1102.5 samples in a 50 ms
    /// window; a fixed width would have to round that, and the grid would then
    /// slide against the clock until a marker is reported a whole window away
    /// from the audio it was found in.
    fn next_boundary(&self) -> u64 {
        let windows = self.rms.len() as f64 + 1.0;
        (((windows * WINDOW_MS as f64 * self.samples_per_ms).round()) as u64).max(self.samples + 1)
    }

    pub fn push(&mut self, sample: f32) {
        if self.samples_per_ms <= 0.0 {
            return;
        }
        self.samples += 1;
        let s = f64::from(sample);
        self.sum_sq += s * s;
        self.count += 1;
        if self.samples >= self.boundary {
            self.rms.push(self.window_rms());
            self.sum_sq = 0.0;
            self.count = 0;
            self.boundary = self.next_boundary();
        }
    }

    pub fn finish(mut self) -> RmsWindows {
        if self.count > 0 {
            self.rms.push(self.window_rms());
        }
        let duration_ms = if self.samples_per_ms > 0.0 {
            (self.samples as f64 / self.samples_per_ms).round() as i64
        } else {
            0
        };
        RmsWindows {
            rms: self.rms,
            duration_ms,
        }
    }

    fn window_rms(&self) -> f32 {
        (self.sum_sq / self.count as f64).sqrt() as f32
    }
}

/// Lowest and highest whole-dBFS level an [`Envelope`] resolves. Both operator
/// thresholds are rounded and held to exactly this range by `persist::config`,
/// so every threshold the detector can be asked about has a code of its own.
pub const LEVEL_MIN_DBFS: i64 = -100;
pub const LEVEL_MAX_DBFS: i64 = -3;

/// Number of resolved levels, `LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS`.
pub const LEVELS: usize = (LEVEL_MAX_DBFS - LEVEL_MIN_DBFS + 1) as usize;

/// One decode, reduced to the level of each window.
///
/// The detector only ever asks the windows where the audio crosses a level, and
/// both thresholds are whole dBFS inside the resolved range. Keeping one byte
/// per window — the highest level that window exceeds — therefore captures the
/// decode exactly rather than approximately, and lets a later threshold change
/// re-derive the trio without reading the file again.
///
/// Everything after those crossings — the two candidates, the bounds, the
/// clamp, the edge-of-file `NULL` rules — is arithmetic on them plus the window
/// count and the decoded duration, both of which are kept here too.
///
/// Unlike a table of answers to the questions [`Envelope::detect`] asks today,
/// this keeps the measurement itself: a later rule that wants a sustained
/// crossing, a level after a given position, or the loudest passage can be
/// written against a stored envelope, where it would need a fresh decode of the
/// whole library against a table of crossings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// One code per window, in order: the number of resolved levels that window
    /// is strictly above, as [`code_of`] assigns them. A window's code is all
    /// the detector needs of it.
    codes: Vec<u8>,
    /// Decoded length of the file, as [`RmsWindows::duration_ms`].
    duration_ms: i64,
}

impl RmsWindows {
    /// Reduce this decode to one code per window.
    ///
    /// A window's code is the number of resolved levels it is strictly above,
    /// so it is defined by the comparison the detector itself makes rather than
    /// by a conversion back to decibels. That is what makes a code and a fresh
    /// scan agree exactly: `rms > amplitude(level)` holds for a prefix of the
    /// ascending levels, and the code is the length of that prefix — no
    /// logarithm, and so no rounding of one to land on the wrong side of a
    /// threshold. See `a_code_answers_exactly_what_a_scan_would`.
    pub fn envelope(&self) -> Envelope {
        let amplitudes = level_amplitudes();
        Envelope {
            codes: self
                .rms
                .iter()
                .map(|&r| amplitudes.partition_point(|&a| f64::from(r) > a) as u8)
                .collect(),
            duration_ms: self.duration_ms,
        }
    }
}

/// One completed automatic analysis: the trio it derived, the envelope it
/// derived them from, the levels it worked to, and when it finished. These
/// belong to each other — the provenance is only meaningful against the markers
/// it produced — so they are committed as one thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysed {
    pub cue: AutoCue,
    pub levels: Envelope,
    pub thresholds: Thresholds,
    pub at_ms: i64,
}

/// Layout version of a stored envelope. A blob written by any other version
/// decodes to `None`, which sends the track back through the decode path rather
/// than deriving markers from bytes this build cannot read.
///
/// **Bump this for more than a change to the bytes.** The layout is only half
/// of what a stored envelope means; the rest is constants that are not in the
/// blob, and a build that changes one of them reads every existing row as
/// version 1 and derives from it anyway:
///
/// - [`WINDOW_MS`], which turns a window index into a timestamp. Halving it
///   would put every re-derived marker at half its true position, library-wide,
///   with no decode and no error;
/// - [`LEVEL_MIN_DBFS`] and [`LEVEL_MAX_DBFS`], which decide what a code counts.
///   Widening the range shifts the meaning of every code by the change in the
///   floor;
/// - what [`Envelope::detect`] asks of the windows. The envelope answers where
///   the audio crosses a level; a rule needing something else — a crossing held
///   for some duration, say — is not answerable from a v1 blob at all, and
///   `ALGORITHM_VERSION` alone would not requeue anything, since nothing
///   screens on it.
///
/// Bumping is cheap: every row fails the screen and rides the ordinary backfill
/// through one decode. Not bumping is silent and wrong.
pub const LEVELS_FORMAT_VERSION: u8 = 1;

/// Version byte, then the decoded duration. The codes follow, one per window,
/// so a stored envelope is `LEVELS_HEADER_LEN + windows` bytes — variable,
/// unlike a fixed table of crossings, which is why the library screens a stored
/// blob in SQL only as far as SQL can go and lets [`Envelope::decode`] be the
/// authority. See `Db::recalculate_auto_cue`.
pub const LEVELS_HEADER_LEN: usize = 1 + 8;

impl Envelope {
    /// The envelope as it is stored on the track row.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LEVELS_HEADER_LEN + self.codes.len());
        out.push(LEVELS_FORMAT_VERSION);
        out.extend_from_slice(&self.duration_ms.to_le_bytes());
        out.extend_from_slice(&self.codes);
        out
    }

    /// Read a stored envelope back, or `None` for one this build cannot use: a
    /// different layout version, a length that cannot hold the header, or a
    /// code outside the resolved range.
    ///
    /// The code check is what a fixed-width layout got from its length alone.
    /// It is the one screen SQL cannot make — a row's length is a function of
    /// its duration here — so this is the authority on whether a stored
    /// envelope is usable, and callers treat a `None` as "decode this track
    /// again" rather than as an error.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < LEVELS_HEADER_LEN || bytes[0] != LEVELS_FORMAT_VERSION {
            return None;
        }
        let duration_ms = i64::from_le_bytes(bytes[1..LEVELS_HEADER_LEN].try_into().ok()?);
        let codes = &bytes[LEVELS_HEADER_LEN..];
        if codes.iter().any(|&c| c as usize > LEVELS) {
            return None;
        }
        Some(Self {
            codes: codes.to_vec(),
            duration_ms,
        })
    }

    /// Derive the automatic trio at a pair of thresholds.
    ///
    /// `music` decides whether a Next Start is produced at all: commercials and
    /// jingles get trimmed ends and nothing else, because a station segues out
    /// of music, not out of an ad break.
    pub fn detect(&self, music: bool, thresholds: Thresholds) -> AutoCue {
        let silence = code_of(thresholds.silence_dbfs);
        let Some(first) = self.codes.iter().position(|&c| c >= silence) else {
            return AutoCue::default();
        };
        let last = self
            .codes
            .iter()
            .rposition(|&c| c >= silence)
            .unwrap_or(first);

        // A marker at the very edge of the file is stored as NULL: the fallback
        // already means exactly that, and an explicit 0 would read as an
        // operator decision to everything downstream.
        let cue_in_ms = (first > 0).then(|| self.window_start(first));
        let cue_out_ms = (last + 1 < self.codes.len()).then(|| self.window_end(last));

        AutoCue {
            cue_in_ms,
            cue_out_ms,
            next_start_ms: music
                .then(|| self.next_start(cue_in_ms, cue_out_ms, thresholds))
                .flatten(),
        }
    }

    /// The level-based candidate, the cold-ending candidate, and the bounds that
    /// keep either sane. See `docs/cue-auto-analysis.md#music-next-start`.
    ///
    /// The spec states this crossing as "at or above" where the silence bounds
    /// are "strictly above". The two cannot differ: a window would have to sit
    /// exactly on a threshold, and none of the resolved levels is
    /// `f32`-representable — see `no_resolved_level_can_be_hit_exactly`. One
    /// comparison therefore serves both.
    fn next_start(
        &self,
        cue_in_ms: Option<i64>,
        cue_out_ms: Option<i64>,
        thresholds: Thresholds,
    ) -> Option<i64> {
        let cue_in = cue_in_ms.unwrap_or(0);
        let cue_out = cue_out_ms.unwrap_or(self.duration_ms);
        if cue_out - cue_in <= MIN_AFTER_CUE_IN_MS {
            return None;
        }

        let segue = code_of(thresholds.segue_dbfs);
        let level = self
            .codes
            .iter()
            .rposition(|&c| c >= segue)
            .map(|i| self.window_end(i));
        let cold = cue_out - COLD_END_LEAD_MS;
        let raw = level.map_or(cold, |l| l.min(cold));

        let lower = (cue_in + MIN_AFTER_CUE_IN_MS).max(cue_out - MAX_SEGUE_LEAD_MS);
        Some(raw.clamp(lower, cue_out))
    }

    /// How many windows the decode produced. `detect` reads `codes` directly;
    /// this is for the tests that walk an envelope window by window.
    #[cfg(test)]
    fn windows(&self) -> usize {
        self.codes.len()
    }

    /// What a window's code means: the number of resolved levels it is strictly
    /// above. `0` is below every threshold the detector can be asked about,
    /// digital silence included; [`LEVELS`] is above all of them.
    ///
    /// Only the round-trip and property tests need to read one directly — the
    /// detector compares codes against [`code_of`] and never converts back.
    #[cfg(test)]
    fn code_of(&self, window: usize) -> u8 {
        self.codes[window]
    }

    fn window_start(&self, index: usize) -> i64 {
        (index as i64 * WINDOW_MS).min(self.duration_ms)
    }

    fn window_end(&self, index: usize) -> i64 {
        ((index as i64 + 1) * WINDOW_MS).min(self.duration_ms)
    }
}

/// The amplitude of each resolved level, ascending.
///
/// Rebuilt per decode rather than cached: 98 `powf` calls against a decode that
/// just read a whole file off a share is not a cost worth a lock.
fn level_amplitudes() -> [f64; LEVELS] {
    std::array::from_fn(|i| amplitude((LEVEL_MIN_DBFS + i as i64) as f64))
}

/// The code a threshold is compared against: a window at or above this code is
/// above the threshold.
///
/// `persist::config` already rounds and clamps both thresholds to the resolved
/// range; a value from anywhere else is folded to the nearest level rather than
/// refused, and a non-number reads as the floor, which trims nothing.
fn code_of(dbfs: f64) -> u8 {
    let level = if dbfs.is_nan() {
        LEVEL_MIN_DBFS
    } else {
        (dbfs.round() as i64).clamp(LEVEL_MIN_DBFS, LEVEL_MAX_DBFS)
    };
    (level - LEVEL_MIN_DBFS + 1) as u8
}

/// dBFS to the linear RMS amplitude the windows are measured in.
pub(super) fn amplitude(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: Thresholds = Thresholds {
        silence_dbfs: -70.0,
        segue_dbfs: -20.0,
    };

    /// Linear RMS for a dBFS level, so a test can state its windows in the
    /// units the spec is written in.
    fn at(dbfs: f64) -> f32 {
        amplitude(dbfs) as f32
    }

    /// Windows whose duration follows their count, as a whole-window file has.
    fn windows(rms: Vec<f32>) -> RmsWindows {
        let duration_ms = rms.len() as i64 * WINDOW_MS;
        RmsWindows { rms, duration_ms }
    }

    /// `count` windows at `level`, for building a file section by section.
    fn run(level: f64, count: usize) -> Vec<f32> {
        vec![at(level); count]
    }

    /// The detector as it was written before the envelope: three scans over the
    /// windows. Kept as the oracle the envelope is measured against — if the two
    /// ever disagree, the envelope is wrong, because this is the behaviour
    /// `docs/cue-auto-analysis.md` describes.
    fn detect_scanning(windows: &RmsWindows, music: bool, thresholds: Thresholds) -> AutoCue {
        let silence = amplitude(thresholds.silence_dbfs);
        let rms = &windows.rms;
        let Some(first) = rms.iter().position(|&r| f64::from(r) > silence) else {
            return AutoCue::default();
        };
        let last = rms
            .iter()
            .rposition(|&r| f64::from(r) > silence)
            .unwrap_or(first);

        let start = |i: usize| (i as i64 * WINDOW_MS).min(windows.duration_ms);
        let end = |i: usize| ((i as i64 + 1) * WINDOW_MS).min(windows.duration_ms);

        let cue_in_ms = (first > 0).then(|| start(first));
        let cue_out_ms = (last + 1 < rms.len()).then(|| end(last));

        let next_start_ms = music.then(|| {
            let cue_in = cue_in_ms.unwrap_or(0);
            let cue_out = cue_out_ms.unwrap_or(windows.duration_ms);
            if cue_out - cue_in <= MIN_AFTER_CUE_IN_MS {
                return None;
            }
            let segue = amplitude(thresholds.segue_dbfs);
            let level = rms.iter().rposition(|&r| f64::from(r) >= segue).map(end);
            let cold = cue_out - COLD_END_LEAD_MS;
            let raw = level.map_or(cold, |l| l.min(cold));
            let lower = (cue_in + MIN_AFTER_CUE_IN_MS).max(cue_out - MAX_SEGUE_LEAD_MS);
            Some(raw.clamp(lower, cue_out))
        });

        AutoCue {
            cue_in_ms,
            cue_out_ms,
            next_start_ms: next_start_ms.flatten(),
        }
    }

    /// xorshift64. A fixed seed keeps the property test reproducible; a failure
    /// that only some runs saw would be worth less than no test at all.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A file of arbitrary shape: digital silence, levels under the tabulated
    /// floor, levels over its ceiling, and a last window that sometimes covers
    /// less than its nominal width.
    fn random_windows(rng: &mut Rng) -> RmsWindows {
        let count = 1 + rng.below(80) as usize;
        let rms = (0..count)
            .map(|_| match rng.below(8) {
                0 => 0.0,
                1 => at(-120.0),
                2 => at(0.0),
                _ => at(-(rng.below(110) as f64) - rng.below(2) as f64 / 2.0),
            })
            .collect::<Vec<f32>>();
        let full = count as i64 * WINDOW_MS;
        RmsWindows {
            rms,
            duration_ms: if rng.below(4) == 0 {
                full - 1 - rng.below(WINDOW_MS as u64 - 1) as i64
            } else {
                full
            },
        }
    }

    /// Every code, against the comparison it stands in for.
    ///
    /// This is deliberately separate from the behavioural property below. A
    /// code is the number of levels a window is strictly above, and the whole
    /// exactness argument is that this is decided by the detector's own
    /// comparison rather than by converting an amplitude back to decibels — so
    /// it is worth checking directly, not only through the trio it produces.
    #[test]
    fn a_code_answers_exactly_what_a_scan_would() {
        let mut rng = Rng(0xC0_FFEE);
        for case in 0..8 {
            let w = random_windows(&mut rng);
            let envelope = w.envelope();
            for (i, &r) in w.rms.iter().enumerate() {
                for level in LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS {
                    let amp = amplitude(level as f64);
                    assert_eq!(
                        envelope.code_of(i) >= code_of(level as f64),
                        f64::from(r) > amp,
                        "window {i} against {level} dBFS, case {case}"
                    );
                }
            }
        }
    }

    /// A code is bounded by the number of levels, whatever the window held —
    /// digital silence, something under the floor, or something over the
    /// ceiling. A stored envelope is validated against this on the way back in.
    #[test]
    fn every_code_is_within_range() {
        let mut rng = Rng(0xB0_11AD);
        for _ in 0..8 {
            let w = random_windows(&mut rng);
            let envelope = w.envelope();
            for i in 0..envelope.windows() {
                assert!(envelope.code_of(i) as usize <= LEVELS);
            }
        }
        let extremes = windows(vec![0.0, at(-300.0), at(-100.5), at(-50.0), at(0.0)]);
        let codes: Vec<u8> = (0..5).map(|i| extremes.envelope().code_of(i)).collect();
        assert_eq!(codes[0], 0, "digital silence is below every level");
        assert_eq!(codes[1], 0, "and so is anything under the floor");
        assert_eq!(codes[2], 0, "including a level just under it");
        // -100 dBFS up to -51 dBFS, but not -50 itself: the comparison is
        // strict, so a window never counts the level it sits on.
        assert_eq!(codes[3], 50);
        assert_eq!(codes[4], LEVELS as u8, "full scale is above every level");
    }

    /// Why one comparison serves both the silence bounds, which the spec states
    /// as "strictly above", and the segue crossing, which it states as "at or
    /// above".
    ///
    /// The two differ only where a window sits exactly on a threshold. A window
    /// is an `f32` widened to `f64`, so that needs the threshold's amplitude to
    /// be exactly `f32`-representable — and none of the resolved levels is. If
    /// that ever stops being true, this fails loudly and the segue crossing
    /// needs a comparison of its own again.
    #[test]
    fn no_resolved_level_can_be_hit_exactly() {
        for level in LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS {
            let amp = amplitude(level as f64);
            assert_ne!(
                f64::from(amp as f32),
                amp,
                "{level} dBFS is f32-representable, so a window can sit exactly on it"
            );
        }
    }

    #[test]
    fn a_stored_envelope_round_trips() {
        let mut rng = Rng(0x00DE_C0DE);
        for _ in 0..8 {
            let envelope = random_windows(&mut rng).envelope();
            let blob = envelope.encode();
            assert_eq!(blob.len(), LEVELS_HEADER_LEN + envelope.windows());
            assert_eq!(Envelope::decode(&blob), Some(envelope));
        }
    }

    /// A blob this build cannot read must send the track back to the decoder
    /// rather than yield half an envelope.
    ///
    /// The out-of-range code is the case a fixed-width layout caught by length
    /// alone. It is also the one the SQL screen cannot make, which is why
    /// `Db::recalculate_auto_cue` queues whatever fails here rather than
    /// trusting the two to agree.
    #[test]
    fn an_unreadable_envelope_decodes_to_nothing() {
        let good = windows(run(-6.0, 12)).envelope().encode();

        let mut wrong_version = good.clone();
        wrong_version[0] = LEVELS_FORMAT_VERSION.wrapping_add(1);
        assert_eq!(
            Envelope::decode(&wrong_version),
            None,
            "a layout it cannot read"
        );

        let mut bad_code = good.clone();
        *bad_code.last_mut().unwrap() = LEVELS as u8 + 1;
        assert_eq!(Envelope::decode(&bad_code), None, "a code past the range");

        assert_eq!(
            Envelope::decode(&good[..LEVELS_HEADER_LEN - 1]),
            None,
            "too short to hold the header"
        );
        assert_eq!(Envelope::decode(&[]), None);

        assert_eq!(
            Envelope::decode(&good[..LEVELS_HEADER_LEN]).map(|e| e.windows()),
            Some(0),
            "a header alone is a readable envelope of no windows"
        );
    }

    /// The whole safety argument for the envelope: it must answer exactly what
    /// a fresh scan would, for every threshold pair an operator can reach.
    #[test]
    fn the_envelope_agrees_with_a_scan_at_every_threshold() {
        let mut rng = Rng(0x5EED_1E55);
        for case in 0..24 {
            let w = random_windows(&mut rng);
            let envelope = w.envelope();
            for silence in LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS {
                for segue in (silence + 1)..=LEVEL_MAX_DBFS {
                    let thresholds = Thresholds {
                        silence_dbfs: silence as f64,
                        segue_dbfs: segue as f64,
                    };
                    for music in [false, true] {
                        assert_eq!(
                            envelope.detect(music, thresholds),
                            detect_scanning(&w, music, thresholds),
                            "case {case}, silence {silence} dBFS, segue {segue} dBFS, music {music}"
                        );
                    }
                }
            }
        }
    }

    /// A threshold outside the resolved range still has to answer. Both ends
    /// fold to the nearest level, which is what the clamp in `persist::config`
    /// would have produced anyway.
    #[test]
    fn a_threshold_past_the_resolved_range_folds_to_the_nearest_level() {
        assert_eq!(code_of(-400.0), 1);
        assert_eq!(code_of(12.0), LEVELS as u8);
        assert_eq!(code_of(f64::NAN), 1, "a non-number trims nothing");
        assert_eq!(code_of(-70.4), code_of(-70.0));
    }

    #[test]
    fn silence_at_both_ends_is_trimmed() {
        // 10 silent windows, 20 loud, 10 silent: 500 ms in, 1500 ms out.
        let mut w = run(-120.0, 10);
        w.extend(run(-6.0, 20));
        w.extend(run(-120.0, 10));
        let cue = windows(w).envelope().detect(false, DEFAULTS);
        assert_eq!(cue.cue_in_ms, Some(500));
        assert_eq!(cue.cue_out_ms, Some(1500));
    }

    #[test]
    fn audio_at_the_file_edges_stores_null() {
        let cue = windows(run(-6.0, 40)).envelope().detect(false, DEFAULTS);
        assert_eq!(cue.cue_in_ms, None, "no leading silence to trim");
        assert_eq!(cue.cue_out_ms, None, "audio runs to EOF");
    }

    #[test]
    fn a_silent_file_yields_nothing() {
        let cue = windows(run(-120.0, 40)).envelope().detect(true, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn an_empty_decode_yields_nothing() {
        let cue = RmsWindows::default().envelope().detect(true, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn a_cold_ending_hands_over_500_ms_before_cue_out() {
        // Loud to the last window before a short silent tail: no level-based
        // candidate exists below the segue threshold, so the 500 ms rule wins.
        let mut w = run(-6.0, 40); // 2000 ms
        w.extend(run(-120.0, 4)); // → cue out 2000 ms
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(2000));
        assert_eq!(cue.next_start_ms, Some(1500));
    }

    #[test]
    fn a_normal_fade_hands_over_where_the_level_drops() {
        // 2000 ms loud, then 1000 ms between the two thresholds, then silence.
        let mut w = run(-6.0, 40);
        w.extend(run(-40.0, 20));
        w.extend(run(-120.0, 4));
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(3000));
        assert_eq!(cue.next_start_ms, Some(2000), "last window above -20 dBFS");
    }

    #[test]
    fn a_very_long_fade_is_held_to_the_six_second_lead() {
        // Above the segue threshold for 1 s, then 10 s of quiet fade.
        let mut w = run(-6.0, 20);
        w.extend(run(-40.0, 200));
        w.extend(run(-120.0, 4));
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(11_000));
        assert_eq!(cue.next_start_ms, Some(5_000), "cue out − 6 s");
    }

    #[test]
    fn next_start_never_precedes_cue_in_on_short_material() {
        // 1 s of leading silence, then 700 ms of audio: the 6 s lead would put
        // the segue before the track starts, so the Cue In floor wins.
        let mut w = run(-120.0, 20);
        w.extend(run(-6.0, 14));
        w.extend(run(-120.0, 2));
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_in_ms, Some(1000));
        assert_eq!(cue.cue_out_ms, Some(1700));
        assert_eq!(cue.next_start_ms, Some(1500));
    }

    #[test]
    fn material_shorter_than_the_minimum_gets_no_next_start() {
        // 500 ms of playable audio: exactly the lower bound, so there is no
        // room for a segue.
        let mut w = run(-6.0, 10);
        w.extend(run(-120.0, 2));
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(500));
        assert_eq!(cue.next_start_ms, None);
    }

    #[test]
    fn a_commercial_gets_no_next_start() {
        let mut w = run(-6.0, 40);
        w.extend(run(-40.0, 20));
        w.extend(run(-120.0, 4));
        let cue = windows(w).envelope().detect(false, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(3000));
        assert_eq!(cue.next_start_ms, None);
    }

    #[test]
    fn a_jingle_that_runs_to_eof_is_all_null() {
        // Same rule as a commercial; nothing to trim, nothing to segue.
        let cue = windows(run(-6.0, 20)).envelope().detect(false, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn next_start_stays_inside_cue_in_and_cue_out() {
        let mut w = run(-120.0, 4);
        w.extend(run(-6.0, 100));
        w.extend(run(-120.0, 4));
        let cue = windows(w).envelope().detect(true, DEFAULTS);
        let start = cue.next_start_ms.expect("music segues");
        assert!(start >= cue.cue_in_ms.unwrap());
        assert!(start <= cue.cue_out_ms.unwrap());
    }

    #[test]
    fn a_raised_silence_threshold_trims_more() {
        let mut w = run(-50.0, 10);
        w.extend(run(-6.0, 20));
        w.extend(run(-50.0, 10));
        let quiet = windows(w.clone()).envelope().detect(false, DEFAULTS);
        assert_eq!(quiet.cue_in_ms, None, "-50 dBFS is above -70 dBFS");
        let loud = windows(w).envelope().detect(
            false,
            Thresholds {
                silence_dbfs: -40.0,
                ..DEFAULTS
            },
        );
        assert_eq!(loud.cue_in_ms, Some(500));
        assert_eq!(loud.cue_out_ms, Some(1500));
    }

    #[test]
    fn a_partial_last_window_does_not_overrun_the_file() {
        // The final window covers only 30 ms, so the file ends before the
        // window nominally would.
        let w = RmsWindows {
            rms: run(-6.0, 60),
            duration_ms: 2_980,
        };
        let cue = w.envelope().detect(true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, None);
        assert_eq!(cue.next_start_ms, Some(2_480), "file end − 500 ms");
    }

    #[test]
    fn the_collector_splits_the_stream_into_fifty_millisecond_windows() {
        // 1 s of mono 8 kHz: 20 windows of 400 samples.
        let mut c = Collector::new(8_000, 1);
        for i in 0..8_000 {
            c.push(if i < 4_000 { 0.0 } else { 1.0 });
        }
        let w = c.finish();
        assert_eq!(w.rms.len(), 20);
        assert_eq!(w.duration_ms, 1_000);
        assert!(w.rms[..10].iter().all(|&r| r == 0.0));
        assert!(w.rms[10..].iter().all(|&r| (r - 1.0).abs() < 1e-6));
    }

    #[test]
    fn the_collector_keeps_a_partial_final_window() {
        let mut c = Collector::new(8_000, 1);
        for _ in 0..8_200 {
            c.push(1.0);
        }
        let w = c.finish();
        assert_eq!(w.rms.len(), 21);
        assert_eq!(w.duration_ms, 1_025);
    }

    /// 22.05 kHz mono puts 1102.5 interleaved samples in a 50 ms window. A
    /// window of whole samples has to round that, and the grid then slides
    /// against the clock — after two minutes by a whole window, so a marker is
    /// reported later than the audio it was found in and trims the head off
    /// the track.
    #[test]
    fn an_odd_sample_rate_does_not_drift_the_window_grid() {
        let mut c = Collector::new(22_050, 1);
        let onset = 2_430_000; // 110.204 s in
        for i in 0..2_450_000 {
            c.push(if i < onset { 0.0 } else { 1.0 });
        }
        let w = c.finish();

        let cue_in = w.envelope().detect(false, DEFAULTS).cue_in_ms.unwrap();
        assert_eq!(cue_in, 110_200);
        assert!(
            f64::from(cue_in as i32) <= f64::from(onset) / 22.05,
            "a marker may never be reported past the audio it was found in"
        );
    }

    #[test]
    fn a_degenerate_header_collects_nothing() {
        let mut c = Collector::new(0, 0);
        for _ in 0..1_000 {
            c.push(1.0);
        }
        let w = c.finish();
        assert!(w.rms.is_empty());
        assert_eq!(w.duration_ms, 0);
    }
}
