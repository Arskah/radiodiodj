//! Cue In, Cue Out and Next Start, derived from a measured level envelope.
//!
//! The station's rules, not a measurement: what counts as silence worth
//! trimming, that a cold-ending track hands over half a second early, that a
//! jingle gets no Next Start because a station segues out of music and not out
//! of an ad break, and that a marker at the very edge of a file is stored as
//! `NULL`. All of it is arithmetic over
//! [`crate::audio_measure::level_envelope`]'s crossings, which is why it lives
//! here, beside the rows it is stored on, rather than in the module that
//! measured the audio.
//!
//! Pure: no file, no decoder, no database. See `docs/cue-auto-analysis.md`.

use crate::audio_measure::level_envelope::Envelope;

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

/// Derive the automatic trio from a measured envelope at a pair of thresholds.
///
/// `music` decides whether a Next Start is produced at all: commercials and
/// jingles get trimmed ends and nothing else, because a station segues out of
/// music, not out of an ad break.
pub fn detect(envelope: &Envelope, music: bool, thresholds: Thresholds) -> AutoCue {
    let Some(span) = envelope.span_above(thresholds.silence_dbfs) else {
        return AutoCue::default();
    };

    // A marker at the very edge of the file is stored as NULL: the fallback
    // already means exactly that, and an explicit 0 would read as an operator
    // decision to everything downstream.
    let cue_in_ms = (!span.starts_at_file_start).then_some(span.from_ms);
    let cue_out_ms = (!span.ends_at_file_end).then_some(span.to_ms);

    AutoCue {
        cue_in_ms,
        cue_out_ms,
        next_start_ms: music
            .then(|| next_start(envelope, cue_in_ms, cue_out_ms, thresholds))
            .flatten(),
    }
}

/// The level-based candidate, the cold-ending candidate, and the bounds that
/// keep either sane. See `docs/cue-auto-analysis.md#music-next-start`.
fn next_start(
    envelope: &Envelope,
    cue_in_ms: Option<i64>,
    cue_out_ms: Option<i64>,
    thresholds: Thresholds,
) -> Option<i64> {
    let cue_in = cue_in_ms.unwrap_or(0);
    let cue_out = cue_out_ms.unwrap_or(envelope.duration_ms());
    if cue_out - cue_in <= MIN_AFTER_CUE_IN_MS {
        return None;
    }

    let level = envelope.last_end_above(thresholds.segue_dbfs);
    let cold = cue_out - COLD_END_LEAD_MS;
    let raw = level.map_or(cold, |l| l.min(cold));

    let lower = (cue_in + MIN_AFTER_CUE_IN_MS).max(cue_out - MAX_SEGUE_LEAD_MS);
    Some(raw.clamp(lower, cue_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_measure::level_envelope::fixtures::*;
    use crate::audio_measure::level_envelope::{
        amplitude, RmsWindows, LEVEL_MAX_DBFS, LEVEL_MIN_DBFS, WINDOW_MS,
    };

    /// The rule as the cases below read it. An extension trait rather than a
    /// rewrite of every call, so that moving these tests out of the measurement
    /// module changed no assertion.
    trait Detect {
        fn detect(&self, music: bool, thresholds: Thresholds) -> AutoCue;
    }

    impl Detect for Envelope {
        fn detect(&self, music: bool, thresholds: Thresholds) -> AutoCue {
            detect(self, music, thresholds)
        }
    }

    const DEFAULTS: Thresholds = Thresholds {
        silence_dbfs: -70.0,
        segue_dbfs: -20.0,
    };

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
}
