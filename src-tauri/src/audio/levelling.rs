//! Whether a track is levelled on the way to a deck.
//!
//! The measurement and the gain arithmetic are
//! [`crate::audio_measure::loudness`]; what is left here is the operator's
//! choice about applying it, which is a setting and therefore not the
//! measurement library's business.

use crate::audio_measure::loudness::linear_gain;
use crate::persist::config::ReplayGainMode;

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
}
