//! Per-track cue points and their resolution to absolute file positions.
//!
//! Five nullable positions per track, stored as milliseconds from the start of
//! the file. `NULL` means "no adjustment" and resolves to a fallback at load
//! time. See `docs/cue-points.md` for the model and the null table.
//!
//! Pure: no device, no file, no decoder. Both the write path
//! ([`crate::library::db::Db::set_cue_points`]) and the playback path
//! ([`super::deck`]) go through the same clamp, so there is exactly one
//! implementation of the ordering rule.

use serde::{Deserialize, Serialize};

/// Cue points as stored on a track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CuePoints {
    /// First audible sample.
    pub cue_in_ms: Option<i64>,
    /// Where the ramp up reaches full volume.
    pub fade_in_ms: Option<i64>,
    /// Where the ramp down begins.
    pub fade_out_ms: Option<i64>,
    /// Where playback stops (mAirList's sense of cue-out, not PlayIt's).
    pub cue_out_ms: Option<i64>,
    /// Where the next track starts.
    pub next_start_ms: Option<i64>,
}

/// Cue points resolved to absolute file positions in seconds, with every `NULL`
/// replaced by its fallback. `None` survives only where the file end is
/// genuinely unknown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolved {
    /// First audible sample; `0` when no cue-in is set.
    pub cue_in: f64,
    /// Full volume is reached here; equals `cue_in` when there is no ramp up.
    pub fade_in: f64,
    /// The ramp down starts here; equals `cue_out` when there is no ramp down.
    pub fade_out: Option<f64>,
    /// Playback stops here. `None` means "play to the end of the file".
    pub cue_out: Option<f64>,
    /// The next track starts here; equals `cue_out` for a hard cut.
    pub next_start: Option<f64>,
}

impl Default for Resolved {
    /// An untrimmed track: the whole file, no ramps, a hard cut at its end.
    fn default() -> Self {
        Self {
            cue_in: 0.0,
            fade_in: 0.0,
            fade_out: None,
            cue_out: None,
            next_start: None,
        }
    }
}

impl Resolved {
    /// What the track is as far as everything above the worker is concerned:
    /// `cue_out − cue_in`. `None` when the file end is unknown, which is the
    /// same case in which the deck reports no duration at all today.
    pub fn air_duration(&self) -> Option<f64> {
        self.cue_out.map(|out| (out - self.cue_in).max(0.0))
    }

    /// Whether either ramp has non-zero width. A fade whose two positions
    /// coincide is skipped entirely rather than multiplying every sample by 1.0.
    pub fn has_fades(&self) -> bool {
        self.fade_in > self.cue_in
            || matches!((self.fade_out, self.cue_out), (Some(f), Some(out)) if out > f)
    }

    /// Absolute file position to air time: `0` is the first audible sample.
    pub fn air_time(&self, pos: f64) -> f64 {
        (pos - self.cue_in).max(0.0)
    }

    /// Air time back to an absolute file position.
    pub fn file_pos(&self, air: f64) -> f64 {
        self.cue_in + air.max(0.0)
    }

    /// How long the deck should play from `pos` before stopping, or `None` to
    /// run to the end of the file.
    pub fn take_from(&self, pos: f64) -> Option<f64> {
        self.cue_out.map(|out| (out - pos).max(0.0))
    }
}

impl CuePoints {
    /// No marker set at all — the overwhelmingly common case for a track nobody
    /// has prepped.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Sort the markers into `cueIn ≤ fadeIn ≤ fadeOut ≤ cueOut` within
    /// `[0, fileEnd]`, with `nextStart` bounded by `[cueIn, cueOut]`
    /// independently of the fades — a segue may legitimately begin before the
    /// outgoing track starts fading.
    ///
    /// `NULL` stays `NULL`: clamping bounds the markers an operator set and
    /// never invents one. With no file duration, only the lower bounds apply.
    pub fn clamp(self, file_duration: Option<f64>) -> Self {
        let end = file_duration.filter(|d| *d > 0.0).map(to_ms);

        let cue_in = self.cue_in_ms.map(|v| bound(v, 0, end));
        let cue_in_at = cue_in.unwrap_or(0);

        let cue_out = self.cue_out_ms.map(|v| bound(v, cue_in_at, end));
        let cue_out_at = cue_out.or(end);

        let fade_in = self.fade_in_ms.map(|v| bound(v, cue_in_at, cue_out_at));
        let fade_in_at = fade_in.unwrap_or(cue_in_at);

        Self {
            cue_in_ms: cue_in,
            fade_in_ms: fade_in,
            fade_out_ms: self.fade_out_ms.map(|v| bound(v, fade_in_at, cue_out_at)),
            cue_out_ms: cue_out,
            next_start_ms: self.next_start_ms.map(|v| bound(v, cue_in_at, cue_out_at)),
        }
    }

    /// Resolve to absolute seconds against the duration the decoder reported.
    ///
    /// Clamps first, so a marker stored against a file that has since changed
    /// cannot seek the deck out of range. When no duration is available at all,
    /// the end-anchored fallbacks are dropped and the track plays to its end
    /// uncut: audio never fails because a marker could not be resolved.
    pub fn resolve(&self, file_duration: Option<f64>) -> Resolved {
        let end = file_duration.filter(|d| *d > 0.0);
        if end.is_none() && self.cue_out_ms.is_none() && !self.is_empty() {
            log::warn!("cue points: no file duration; end-anchored markers dropped");
        }

        let clamped = self.clamp(file_duration);
        let cue_in = clamped.cue_in_ms.map(to_secs).unwrap_or(0.0);
        let cue_out = clamped.cue_out_ms.map(to_secs).or(end);
        Resolved {
            cue_in,
            fade_in: clamped.fade_in_ms.map(to_secs).unwrap_or(cue_in),
            fade_out: clamped.fade_out_ms.map(to_secs).or(cue_out),
            cue_out,
            next_start: clamped.next_start_ms.map(to_secs).or(cue_out),
        }
    }
}

/// Bound `v` into `[lo, hi]`, tolerating an upper bound below the lower one
/// (a cue-out clamped back onto the cue-in leaves the fades nowhere to go).
fn bound(v: i64, lo: i64, hi: Option<i64>) -> i64 {
    let v = v.max(lo);
    match hi {
        Some(h) => v.min(h.max(lo)),
        None => v,
    }
}

fn to_ms(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn to_secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(cue_in: i64, fade_in: i64, fade_out: i64, cue_out: i64) -> CuePoints {
        CuePoints {
            cue_in_ms: Some(cue_in),
            fade_in_ms: Some(fade_in),
            fade_out_ms: Some(fade_out),
            cue_out_ms: Some(cue_out),
            next_start_ms: None,
        }
    }

    /// The common case: nobody has touched the track, so it plays whole.
    #[test]
    fn a_track_with_no_markers_resolves_to_the_whole_file() {
        let r = CuePoints::default().resolve(Some(200.0));
        assert_eq!(r.cue_in, 0.0);
        assert_eq!(r.fade_in, 0.0);
        assert_eq!(r.cue_out, Some(200.0));
        assert_eq!(r.fade_out, Some(200.0));
        assert_eq!(r.next_start, Some(200.0));
        assert_eq!(r.air_duration(), Some(200.0));
    }

    /// Each null resolves to its own fallback, per the table in the design doc.
    #[test]
    fn each_null_resolves_to_its_documented_fallback() {
        let r = CuePoints {
            cue_in_ms: Some(10_000),
            cue_out_ms: Some(30_000),
            ..Default::default()
        }
        .resolve(Some(200.0));
        // fade_in → cue_in, fade_out → cue_out, next_start → cue_out.
        assert_eq!(r.fade_in, 10.0);
        assert_eq!(r.fade_out, Some(30.0));
        assert_eq!(r.next_start, Some(30.0));
        assert_eq!(r.air_duration(), Some(20.0));
    }

    /// Air time, not file time, is what the deck reports and what the webhook
    /// publishes.
    #[test]
    fn air_duration_is_the_span_between_the_cue_points() {
        let r = points(10_000, 12_000, 170_000, 180_000).resolve(Some(200.0));
        assert_eq!(r.air_duration(), Some(170.0));
        assert_eq!(r.take_from(r.cue_in), Some(170.0));
        assert_eq!(r.take_from(100.0), Some(80.0));
    }

    /// Zero-width ramps cost nothing at playback time: the deck skips the
    /// envelope wrapper rather than multiplying every sample by 1.0.
    #[test]
    fn a_zero_width_ramp_is_not_a_fade() {
        assert!(!CuePoints::default().resolve(Some(200.0)).has_fades());
        assert!(!CuePoints {
            cue_in_ms: Some(10_000),
            cue_out_ms: Some(30_000),
            ..Default::default()
        }
        .resolve(Some(200.0))
        .has_fades());

        let r = points(10_000, 12_000, 170_000, 180_000).resolve(Some(200.0));
        assert!(r.has_fades());
    }

    /// The two timelines are inverses of each other, which is what lets the
    /// renderer's transport code stay unaware that cue points exist at all.
    #[test]
    fn air_time_and_file_position_are_inverses() {
        let r = points(10_000, 12_000, 170_000, 180_000).resolve(Some(200.0));
        assert_eq!(r.air_time(10.0), 0.0, "cue in is air zero");
        assert_eq!(r.air_time(25.0), 15.0);
        assert_eq!(r.file_pos(15.0), 25.0);
        assert_eq!(r.air_time(r.file_pos(42.0)), 42.0);
    }

    /// The playhead can sit fractionally behind the cue-in on the first tick
    /// after a load; that is not a negative time to report.
    #[test]
    fn a_position_before_the_cue_in_reports_air_zero() {
        let r = points(10_000, 12_000, 170_000, 180_000).resolve(Some(200.0));
        assert_eq!(r.air_time(9.5), 0.0);
    }

    /// A position already past the out-point yields no audio rather than a
    /// negative take, which would panic building a `Duration`.
    #[test]
    fn take_from_past_the_out_point_is_zero() {
        let r = points(0, 0, 30_000, 30_000).resolve(Some(200.0));
        assert_eq!(r.take_from(45.0), Some(0.0));
    }

    #[test]
    fn markers_are_sorted_into_order() {
        // Deliberately scrambled: fade_out before fade_in, cue_out before both.
        let c = points(5_000, 40_000, 20_000, 30_000).clamp(Some(200.0));
        assert_eq!(c.cue_in_ms, Some(5_000));
        assert_eq!(c.cue_out_ms, Some(30_000));
        assert_eq!(c.fade_in_ms, Some(30_000), "fade in cannot pass cue out");
        assert_eq!(
            c.fade_out_ms,
            Some(30_000),
            "fade out cannot precede fade in"
        );
    }

    #[test]
    fn markers_are_bounded_by_the_file() {
        let c = points(-5_000, 0, 500_000, 500_000).clamp(Some(200.0));
        assert_eq!(c.cue_in_ms, Some(0));
        assert_eq!(c.cue_out_ms, Some(200_000));
        assert_eq!(c.fade_out_ms, Some(200_000));
    }

    /// A tight transition starts the next track before this one fades, which is
    /// normal radio practice and not an error to be clamped away.
    #[test]
    fn next_start_is_bounded_independently_of_the_fades() {
        let c = CuePoints {
            next_start_ms: Some(15_000),
            ..points(10_000, 12_000, 170_000, 180_000)
        }
        .clamp(Some(200.0));
        assert_eq!(c.next_start_ms, Some(15_000), "before fade_out is legal");

        let c = CuePoints {
            next_start_ms: Some(190_000),
            ..points(10_000, 12_000, 170_000, 180_000)
        }
        .clamp(Some(200.0));
        assert_eq!(c.next_start_ms, Some(180_000), "but never past cue out");
    }

    /// Clamping bounds what the operator set; it never fills a null in.
    #[test]
    fn clamping_never_invents_a_marker() {
        let c = CuePoints::default().clamp(Some(200.0));
        assert_eq!(c, CuePoints::default());
    }

    /// No duration anywhere: the start-anchored markers still apply, and the
    /// track simply plays to its end.
    #[test]
    fn without_a_file_duration_the_track_plays_to_its_end() {
        let r = CuePoints {
            cue_in_ms: Some(10_000),
            ..Default::default()
        }
        .resolve(None);
        assert_eq!(r.cue_in, 10.0);
        assert_eq!(r.cue_out, None);
        assert_eq!(r.fade_out, None);
        assert_eq!(r.air_duration(), None);
        assert_eq!(r.take_from(10.0), None, "no take: run to the file end");
    }

    /// An explicit cue-out needs no file duration to be usable.
    #[test]
    fn an_explicit_cue_out_survives_an_unknown_file_duration() {
        let r = CuePoints {
            cue_out_ms: Some(30_000),
            ..Default::default()
        }
        .resolve(None);
        assert_eq!(r.cue_out, Some(30.0));
        assert_eq!(r.air_duration(), Some(30.0));
    }

    /// A zero or missing tag duration is treated as unknown rather than as a
    /// zero-length file that would clamp every marker to 0.
    #[test]
    fn a_zero_duration_is_treated_as_unknown() {
        let c = points(10_000, 12_000, 20_000, 30_000).clamp(Some(0.0));
        assert_eq!(c.cue_in_ms, Some(10_000));
        assert_eq!(c.cue_out_ms, Some(30_000));
    }

    /// Resolution clamps as well, so a marker stored against a file that has
    /// since been replaced by a shorter one cannot seek out of range.
    #[test]
    fn resolution_clamps_against_the_decoded_duration() {
        let r = points(10_000, 12_000, 170_000, 180_000).resolve(Some(60.0));
        assert_eq!(r.cue_out, Some(60.0));
        assert_eq!(r.air_duration(), Some(50.0));
    }
}
