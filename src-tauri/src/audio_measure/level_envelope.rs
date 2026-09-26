//! The level envelope: what a decode says about how loud a track is, window by
//! window.
//!
//! Pure: no device, no file, no decoder. The analysis pass feeds the RMS windows
//! collected during the waveform decode ([`super::waveform::analyze`]), so this
//! never costs a second pass over the file.
//!
//! What it answers is one question, asked at a level: *where is the audio above
//! `dbfs`* ([`Envelope::span_above`], [`Envelope::last_end_above`]). It does not
//! know what a caller does with the answer. The rules that turn crossings into a
//! station's Cue In, Cue Out and Next Start are
//! [`crate::library::auto_cue`] — where "music", "segue" and what a `NULL`
//! column means belong. See `docs/cue-auto-analysis.md` for those rules and
//! `docs/audio-measure.md` for the split.
//!
//! The measurement is deliberately small — RMS over fixed
//! [`WINDOW_MS`] windows, quantised to whole decibels, and nothing else. It
//! detects no beats, phrases, vocals or hidden tracks.

/// Width of one RMS window. Fixed in v1: short enough to place a marker without
/// visible coarseness, long enough that a single transient cannot move one.
/// Changing it moves every position a stored envelope reports — see
/// [`LEVELS_FORMAT_VERSION`].
pub const WINDOW_MS: i64 = 50;

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

/// Lowest and highest whole-dBFS level an [`Envelope`] resolves. A caller asks
/// about whole levels inside this range — the app rounds and clamps its
/// thresholds to it before asking — so every level that can be asked about has a
/// code of its own.
pub const LEVEL_MIN_DBFS: i64 = -100;
pub const LEVEL_MAX_DBFS: i64 = -3;

/// Number of resolved levels, `LEVEL_MIN_DBFS..=LEVEL_MAX_DBFS`.
pub const LEVELS: usize = (LEVEL_MAX_DBFS - LEVEL_MIN_DBFS + 1) as usize;

/// One decode, reduced to the level of each window.
///
/// Callers only ever ask the windows where the audio crosses a level, and every
/// level they can ask about is a whole dBFS inside the resolved range. Keeping
/// one byte per window — the highest level that window exceeds — therefore
/// captures the decode exactly rather than approximately, and lets a later
/// threshold change re-derive an answer without reading the file again.
///
/// Everything after those crossings is arithmetic on them plus the window count
/// and the decoded duration, both of which are kept here too — which is why the
/// rules that read an envelope are not in this module.
///
/// Unlike a table of answers to the questions asked today, this keeps the
/// measurement itself: a later rule that wants a sustained crossing, a level
/// after a given position, or the loudest passage can be written against a
/// stored envelope, where it would need a fresh decode of the whole library
/// against a table of crossings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// One code per window, in order: the number of resolved levels that window
    /// is strictly above, as [`code_of`] assigns them.
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
/// - what a rule asks of the windows, which is the one item on this list that
///   lives outside this module ([`crate::library::auto_cue`] today). The
///   envelope answers where the audio crosses a level; a rule needing something
///   else — a crossing held for some duration, say — is not answerable from a v1
///   blob at all, and that rule's own version constant would not requeue
///   anything, since nothing screens on it. A rule that changes what it asks
///   has to come back here and bump this.
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

/// Where audio sits above a level, in window-aligned positions.
///
/// The two flags are facts about the file rather than about how anything stores
/// them: a span starting at the first window means the audio was already running
/// when the file opened. What that should be recorded as is the caller's rule,
/// not this module's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Start of the first window above the level.
    pub from_ms: i64,
    /// End of the last window above the level, clamped to the decoded duration.
    pub to_ms: i64,
    /// The file's first window is above the level.
    pub starts_at_file_start: bool,
    /// The file's last window is above the level.
    pub ends_at_file_end: bool,
}

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

    /// Where the audio sits above `dbfs`, or `None` if it never does.
    ///
    /// One comparison serves every caller, including one whose rule reads "at or
    /// above" where another reads "strictly above". The two cannot differ: a
    /// window would have to sit exactly on a level, and none of the resolved
    /// levels is `f32`-representable — see `no_resolved_level_can_be_hit_exactly`.
    pub fn span_above(&self, dbfs: f64) -> Option<Span> {
        let level = code_of(dbfs);
        let first = self.codes.iter().position(|&c| c >= level)?;
        let last = self
            .codes
            .iter()
            .rposition(|&c| c >= level)
            .unwrap_or(first);
        Some(Span {
            from_ms: self.window_start(first),
            to_ms: self.window_end(last),
            starts_at_file_start: first == 0,
            ends_at_file_end: last + 1 == self.codes.len(),
        })
    }

    /// Where the audio was last above `dbfs`, as the end of that window.
    ///
    /// Asked at a level well above silence this is the tail question — not where
    /// the audio stops, but where it last reached a level and never returned.
    pub fn last_end_above(&self, dbfs: f64) -> Option<i64> {
        let level = code_of(dbfs);
        self.codes
            .iter()
            .rposition(|&c| c >= level)
            .map(|i| self.window_end(i))
    }

    /// Decoded length of the file this envelope was measured from.
    pub fn duration_ms(&self) -> i64 {
        self.duration_ms
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
pub fn amplitude(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// Linear RMS for a dBFS level, so a test can state its windows in the
    /// units the spec is written in.
    pub(crate) fn at(dbfs: f64) -> f32 {
        amplitude(dbfs) as f32
    }

    /// Windows whose duration follows their count, as a whole-window file has.
    pub(crate) fn windows(rms: Vec<f32>) -> RmsWindows {
        let duration_ms = rms.len() as i64 * WINDOW_MS;
        RmsWindows { rms, duration_ms }
    }

    /// `count` windows at `level`, for building a file section by section.
    pub(crate) fn run(level: f64, count: usize) -> Vec<f32> {
        vec![at(level); count]
    }

    /// xorshift64. A fixed seed keeps the property test reproducible; a failure
    /// that only some runs saw would be worth less than no test at all.
    pub(crate) struct Rng(pub(crate) u64);

    impl Rng {
        pub(crate) fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        pub(crate) fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A file of arbitrary shape: digital silence, levels under the tabulated
    /// floor, levels over its ceiling, and a last window that sometimes covers
    /// less than its nominal width.
    pub(crate) fn random_windows(rng: &mut Rng) -> RmsWindows {
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
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

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

    /// A threshold outside the resolved range still has to answer. Both ends
    /// fold to the nearest level, which is what the caller's own clamp would
    /// have produced anyway.
    #[test]
    fn a_threshold_past_the_resolved_range_folds_to_the_nearest_level() {
        assert_eq!(code_of(-400.0), 1);
        assert_eq!(code_of(12.0), LEVELS as u8);
        assert_eq!(code_of(f64::NAN), 1, "a non-number trims nothing");
        assert_eq!(code_of(-70.4), code_of(-70.0));
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
    /// against the clock — after two minutes by a whole window, so a crossing is
    /// reported later than the audio it was found in, and whatever the caller
    /// places there lands past the start of the track.
    #[test]
    fn an_odd_sample_rate_does_not_drift_the_window_grid() {
        let mut c = Collector::new(22_050, 1);
        let onset = 2_430_000; // 110.204 s in
        for i in 0..2_450_000 {
            c.push(if i < onset { 0.0 } else { 1.0 });
        }
        let w = c.finish();

        let from_ms = w.envelope().span_above(-70.0).expect("audio above").from_ms;
        assert_eq!(from_ms, 110_200);
        assert!(
            f64::from(from_ms as i32) <= f64::from(onset) / 22.05,
            "a crossing may never be reported past the audio it was found in"
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

    #[test]
    fn a_span_reports_the_windows_the_audio_is_above_a_level() {
        // 10 silent windows, 20 loud, 10 silent.
        let mut w = run(-120.0, 10);
        w.extend(run(-6.0, 20));
        w.extend(run(-120.0, 10));
        let span = windows(w)
            .envelope()
            .span_above(-70.0)
            .expect("audio above");
        assert_eq!(span.from_ms, 500);
        assert_eq!(span.to_ms, 1500);
        assert!(!span.starts_at_file_start);
        assert!(!span.ends_at_file_end);
    }

    #[test]
    fn a_span_flags_audio_that_runs_to_both_file_edges() {
        let span = windows(run(-6.0, 40))
            .envelope()
            .span_above(-70.0)
            .expect("audio above");
        assert_eq!(span.from_ms, 0);
        assert_eq!(span.to_ms, 2000);
        assert!(span.starts_at_file_start);
        assert!(span.ends_at_file_end);
    }

    #[test]
    fn nothing_above_the_level_is_no_span() {
        assert!(windows(run(-120.0, 40))
            .envelope()
            .span_above(-70.0)
            .is_none());
        assert!(RmsWindows::default().envelope().span_above(-70.0).is_none());
    }

    #[test]
    fn a_span_never_runs_past_the_decoded_duration() {
        // The final window covers only 30 ms of the file.
        let w = RmsWindows {
            rms: run(-6.0, 60),
            duration_ms: 2_980,
        };
        let span = w.envelope().span_above(-70.0).expect("audio above");
        assert_eq!(span.to_ms, 2_980);
        assert_eq!(w.envelope().last_end_above(-70.0), Some(2_980));
    }

    #[test]
    fn the_tail_question_is_asked_above_silence() {
        // Loud, then quiet but not silent: the two levels answer differently.
        let mut w = run(-6.0, 20);
        w.extend(run(-40.0, 20));
        let envelope = windows(w).envelope();
        assert_eq!(envelope.last_end_above(-20.0), Some(1000), "loud part only");
        assert_eq!(envelope.last_end_above(-70.0), Some(2000), "all of it");
    }
}
