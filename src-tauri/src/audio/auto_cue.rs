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

/// Derive the automatic trio from the windows.
///
/// `music` decides whether a Next Start is produced at all: commercials and
/// jingles get trimmed ends and nothing else, because a station segues out of
/// music, not out of an ad break.
pub fn detect(windows: &RmsWindows, music: bool, thresholds: Thresholds) -> AutoCue {
    let silence = amplitude(thresholds.silence_dbfs);
    let rms = &windows.rms;
    let Some(first) = rms.iter().position(|&r| f64::from(r) > silence) else {
        return AutoCue::default();
    };
    let last = rms
        .iter()
        .rposition(|&r| f64::from(r) > silence)
        .unwrap_or(first);

    // A marker at the very edge of the file is stored as NULL: the fallback
    // already means exactly that, and an explicit 0 would read as an operator
    // decision to everything downstream.
    let cue_in_ms = (first > 0).then(|| window_start(first, windows));
    let cue_out_ms = (last + 1 < rms.len()).then(|| window_end(last, windows));

    AutoCue {
        cue_in_ms,
        cue_out_ms,
        next_start_ms: music
            .then(|| next_start(windows, cue_in_ms, cue_out_ms, thresholds))
            .flatten(),
    }
}

/// The level-based candidate, the cold-ending candidate, and the bounds that
/// keep either sane. See `docs/cue-auto-analysis.md#music-next-start`.
fn next_start(
    windows: &RmsWindows,
    cue_in_ms: Option<i64>,
    cue_out_ms: Option<i64>,
    thresholds: Thresholds,
) -> Option<i64> {
    let cue_in = cue_in_ms.unwrap_or(0);
    let cue_out = cue_out_ms.unwrap_or(windows.duration_ms);
    if cue_out - cue_in <= MIN_AFTER_CUE_IN_MS {
        return None;
    }

    let segue = amplitude(thresholds.segue_dbfs);
    let level = windows
        .rms
        .iter()
        .rposition(|&r| f64::from(r) >= segue)
        .map(|i| window_end(i, windows));
    let cold = cue_out - COLD_END_LEAD_MS;
    let raw = level.map_or(cold, |l| l.min(cold));

    let lower = (cue_in + MIN_AFTER_CUE_IN_MS).max(cue_out - MAX_SEGUE_LEAD_MS);
    Some(raw.clamp(lower, cue_out))
}

fn window_start(index: usize, windows: &RmsWindows) -> i64 {
    (index as i64 * WINDOW_MS).min(windows.duration_ms)
}

fn window_end(index: usize, windows: &RmsWindows) -> i64 {
    ((index as i64 + 1) * WINDOW_MS).min(windows.duration_ms)
}

/// dBFS to the linear RMS amplitude the windows are measured in.
fn amplitude(dbfs: f64) -> f64 {
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

    #[test]
    fn silence_at_both_ends_is_trimmed() {
        // 10 silent windows, 20 loud, 10 silent: 500 ms in, 1500 ms out.
        let mut w = run(-120.0, 10);
        w.extend(run(-6.0, 20));
        w.extend(run(-120.0, 10));
        let cue = detect(&windows(w), false, DEFAULTS);
        assert_eq!(cue.cue_in_ms, Some(500));
        assert_eq!(cue.cue_out_ms, Some(1500));
    }

    #[test]
    fn audio_at_the_file_edges_stores_null() {
        let cue = detect(&windows(run(-6.0, 40)), false, DEFAULTS);
        assert_eq!(cue.cue_in_ms, None, "no leading silence to trim");
        assert_eq!(cue.cue_out_ms, None, "audio runs to EOF");
    }

    #[test]
    fn a_silent_file_yields_nothing() {
        let cue = detect(&windows(run(-120.0, 40)), true, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn an_empty_decode_yields_nothing() {
        let cue = detect(&RmsWindows::default(), true, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn a_cold_ending_hands_over_500_ms_before_cue_out() {
        // Loud to the last window before a short silent tail: no level-based
        // candidate exists below the segue threshold, so the 500 ms rule wins.
        let mut w = run(-6.0, 40); // 2000 ms
        w.extend(run(-120.0, 4)); // → cue out 2000 ms
        let cue = detect(&windows(w), true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(2000));
        assert_eq!(cue.next_start_ms, Some(1500));
    }

    #[test]
    fn a_normal_fade_hands_over_where_the_level_drops() {
        // 2000 ms loud, then 1000 ms between the two thresholds, then silence.
        let mut w = run(-6.0, 40);
        w.extend(run(-40.0, 20));
        w.extend(run(-120.0, 4));
        let cue = detect(&windows(w), true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(3000));
        assert_eq!(cue.next_start_ms, Some(2000), "last window above -20 dBFS");
    }

    #[test]
    fn a_very_long_fade_is_held_to_the_six_second_lead() {
        // Above the segue threshold for 1 s, then 10 s of quiet fade.
        let mut w = run(-6.0, 20);
        w.extend(run(-40.0, 200));
        w.extend(run(-120.0, 4));
        let cue = detect(&windows(w), true, DEFAULTS);
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
        let cue = detect(&windows(w), true, DEFAULTS);
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
        let cue = detect(&windows(w), true, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(500));
        assert_eq!(cue.next_start_ms, None);
    }

    #[test]
    fn a_commercial_gets_no_next_start() {
        let mut w = run(-6.0, 40);
        w.extend(run(-40.0, 20));
        w.extend(run(-120.0, 4));
        let cue = detect(&windows(w), false, DEFAULTS);
        assert_eq!(cue.cue_out_ms, Some(3000));
        assert_eq!(cue.next_start_ms, None);
    }

    #[test]
    fn a_jingle_that_runs_to_eof_is_all_null() {
        // Same rule as a commercial; nothing to trim, nothing to segue.
        let cue = detect(&windows(run(-6.0, 20)), false, DEFAULTS);
        assert_eq!(cue, AutoCue::default());
    }

    #[test]
    fn next_start_stays_inside_cue_in_and_cue_out() {
        let mut w = run(-120.0, 4);
        w.extend(run(-6.0, 100));
        w.extend(run(-120.0, 4));
        let cue = detect(&windows(w), true, DEFAULTS);
        let start = cue.next_start_ms.expect("music segues");
        assert!(start >= cue.cue_in_ms.unwrap());
        assert!(start <= cue.cue_out_ms.unwrap());
    }

    #[test]
    fn a_raised_silence_threshold_trims_more() {
        let mut w = run(-50.0, 10);
        w.extend(run(-6.0, 20));
        w.extend(run(-50.0, 10));
        let quiet = detect(&windows(w.clone()), false, DEFAULTS);
        assert_eq!(quiet.cue_in_ms, None, "-50 dBFS is above -70 dBFS");
        let loud = detect(
            &windows(w),
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
        let cue = detect(&w, true, DEFAULTS);
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

        let cue_in = detect(&w, false, DEFAULTS).cue_in_ms.unwrap();
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
