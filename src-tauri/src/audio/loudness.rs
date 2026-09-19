//! ReplayGain: how loud a track measured, and what gain that earns it.
//!
//! The measurement itself rides along with the waveform decode
//! ([`super::waveform::analyze`]); this module owns only the numbers derived
//! from it, so the arithmetic is testable without decoding anything.
//!
//! Loudness is measured rather than read from `replaygain_track_gain` tags.
//! Most station libraries are largely untagged, and normalising only the
//! tagged half would pull those tracks down toward reference while the rest
//! stayed at full scale — a level split where there was none. Tags are also
//! not comparable with each other: ReplayGain 1.0 (89 dB reference) and 2.0
//! (-18 LUFS, EBU R128) write the same tag name from different targets.

use crate::persist::config::ReplayGainMode;

/// Reference level every track is normalised to, in LUFS, per ReplayGain 2.0.
pub const TARGET_LUFS: f64 = -18.0;

/// A full-decode loudness measurement. Absent for a track that is silent or
/// shorter than one integration block, which is why the analysis pass stores
/// these as nullable columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness over the whole file.
    pub lufs: f64,
    /// Highest absolute sample value seen, linear. `1.0` is full scale; a
    /// clipped master can exceed it.
    pub peak: f32,
}

/// The gain that brings `lufs` to [`TARGET_LUFS`], in dB.
pub fn gain_db(lufs: f64) -> f64 {
    TARGET_LUFS - lufs
}

/// The factor to multiply samples by, from a stored gain and peak.
///
/// Clamped so the loudest sample lands no higher than full scale: a track
/// already peaking near 1.0 cannot be turned up, which is ordinary ReplayGain
/// behaviour and leaves the residue to whatever processing follows. A peak of
/// zero or a non-finite input yields unity rather than a division by zero.
pub fn linear_gain(gain_db: f64, peak: Option<f64>) -> f32 {
    if !gain_db.is_finite() {
        return 1.0;
    }
    let raw = 10f64.powf(gain_db / 20.0);
    let limited = match peak {
        Some(p) if p.is_finite() && p > 0.0 => raw.min(1.0 / p),
        _ => raw,
    };
    limited as f32
}

/// The factor to load a track at: its stored measurement under `mode`.
///
/// Unity whenever levelling is off, or the track has no measurement yet — a
/// file the analysis pass has not reached, or one it measured as silent. Both
/// call sites that build a `Load` go through here so the program decks and the
/// cue deck cannot drift apart on what a track's level should be.
pub fn factor(mode: ReplayGainMode, gain_db: Option<f64>, peak: Option<f64>) -> f32 {
    match (mode, gain_db) {
        (ReplayGainMode::Off, _) | (_, None) => 1.0,
        (ReplayGainMode::Track, Some(g)) => linear_gain(g, peak),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levelling_off_leaves_every_track_as_mastered() {
        assert_eq!(factor(ReplayGainMode::Off, Some(-10.0), Some(1.0)), 1.0);
    }

    #[test]
    fn an_unmeasured_track_plays_at_unity() {
        assert_eq!(factor(ReplayGainMode::Track, None, None), 1.0);
        assert_eq!(factor(ReplayGainMode::Track, None, Some(0.9)), 1.0);
    }

    #[test]
    fn a_measured_track_is_levelled() {
        assert_eq!(
            factor(ReplayGainMode::Track, Some(-10.0), Some(1.0)),
            linear_gain(-10.0, Some(1.0))
        );
    }

    #[test]
    fn a_track_at_reference_earns_no_gain() {
        assert_eq!(gain_db(TARGET_LUFS), 0.0);
        assert_eq!(linear_gain(0.0, Some(1.0)), 1.0);
    }

    #[test]
    fn a_loud_track_is_turned_down() {
        // -8 LUFS is 10 dB over reference.
        assert!((gain_db(-8.0) - -10.0).abs() < 1e-9);
        let g = linear_gain(-10.0, Some(1.0));
        assert!((g - 0.3162278).abs() < 1e-6, "{g}");
    }

    #[test]
    fn a_quiet_track_is_turned_up() {
        assert!((gain_db(-28.0) - 10.0).abs() < 1e-9);
        let g = linear_gain(10.0, Some(0.1));
        assert!((g - 3.1622777).abs() < 1e-5, "{g}");
    }

    #[test]
    fn the_peak_caps_the_boost() {
        // +10 dB would be 3.16x, but a peak of 0.5 only allows 2x.
        let g = linear_gain(10.0, Some(0.5));
        assert!((g - 2.0).abs() < 1e-6, "{g}");
    }

    #[test]
    fn a_clipped_master_is_pushed_below_full_scale() {
        // Peak already over 1.0, so even unity gain is too much.
        let g = linear_gain(0.0, Some(1.25));
        assert!((g - 0.8).abs() < 1e-6, "{g}");
    }

    #[test]
    fn an_unknown_or_degenerate_peak_does_not_divide_by_zero() {
        assert_eq!(linear_gain(0.0, None), 1.0);
        assert_eq!(linear_gain(0.0, Some(0.0)), 1.0);
        assert_eq!(linear_gain(0.0, Some(f64::NAN)), 1.0);
    }

    #[test]
    fn a_silent_measurement_yields_unity() {
        // gain_db(-inf) is +inf; the factor must not follow it.
        assert_eq!(linear_gain(gain_db(f64::NEG_INFINITY), Some(1.0)), 1.0);
    }
}
