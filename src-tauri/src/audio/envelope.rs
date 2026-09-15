//! A per-track gain envelope, applied at the source.
//!
//! Stored fades are "at position X through position Y" operations, keyed on
//! absolute file position. They are deliberately **not** driven through
//! `sink.set_volume()`: that is a deck-level control, correct for the live
//! fade-out button and for segue ramps, which are "from now, over N ms"
//! operations. Routing both through one value makes them fight — a live fade
//! fired while a track is inside its own stored fade-out would clobber it, last
//! writer per tick wins. As a source envelope multiplied by a sink gain the two
//! compose at different stages, with no priority rule to get wrong.
//!
//! The envelope is also sample-accurate rather than stepped at the worker's
//! 50 ms tick.
//!
//! rodio 0.21.1 cannot express an outro ramp: `fade_out` and `linear_gain_ramp`
//! both ramp from the *source's* start, and `TakeDuration::set_filter_fadeout`
//! fades across the entire take. Hence this source, which mirrors
//! `TakeDuration`'s own structure — it holds the per-sample duration and
//! re-derives it whenever `current_span_len` is exhausted, so a mid-file sample
//! rate or channel change is handled exactly as rodio handles it.

use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::cue_points::Resolved;

/// Gain at an absolute file position, in `[0, 1]`.
///
/// Five branches: before the in-point, the ramp up, the body, the ramp down,
/// past the out-point. A ramp whose two positions coincide has zero width and
/// contributes nothing.
pub fn gain_at(pos: f64, cue: &Resolved) -> f32 {
    if pos < cue.cue_in {
        return 0.0;
    }
    if let Some(out) = cue.cue_out {
        if pos >= out {
            return 0.0;
        }
        if let Some(start) = cue.fade_out {
            if out > start && pos >= start {
                return ((out - pos) / (out - start)) as f32;
            }
        }
    }
    if cue.fade_in > cue.cue_in && pos < cue.fade_in {
        return ((pos - cue.cue_in) / (cue.fade_in - cue.cue_in)) as f32;
    }
    1.0
}

/// A source that multiplies each sample by the track's envelope at that sample's
/// absolute position in the file.
pub struct Enveloped<I> {
    input: I,
    cue: Resolved,
    /// Absolute file position of the next sample, in seconds.
    position: f64,
    /// Remaining samples in the current span; mirrors `TakeDuration`.
    current_span_len: Option<usize>,
    /// Only re-derived when the current span is exhausted.
    secs_per_sample: f64,
}

impl<I> Enveloped<I>
where
    I: Source,
{
    /// Wrap `input`, whose next sample sits at absolute file position `start`.
    /// The deck seeks before wrapping, so `start` is the seek target rather
    /// than zero.
    pub fn new(input: I, start: f64, cue: Resolved) -> Self {
        Self {
            current_span_len: input.current_span_len(),
            secs_per_sample: secs_per_sample(&input),
            input,
            cue,
            position: start,
        }
    }
}

/// Seconds of wall clock per *sample*, not per frame: rodio's sources are
/// interleaved, so one channel's worth of time passes every `channels` samples.
fn secs_per_sample<I>(input: &I) -> f64
where
    I: Source,
{
    let rate = input.sample_rate() as f64 * input.channels() as f64;
    if rate > 0.0 {
        1.0 / rate
    } else {
        0.0
    }
}

impl<I> Iterator for Enveloped<I>
where
    I: Source,
{
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if let Some(span_len) = self.current_span_len.take() {
            if span_len > 0 {
                self.current_span_len = Some(span_len - 1);
            } else {
                self.current_span_len = self.input.current_span_len();
                // Sample rate or channel count may have changed with the span.
                self.secs_per_sample = secs_per_sample(&self.input);
            }
        }

        let sample = self.input.next()?;
        let gain = gain_at(self.position, &self.cue);
        self.position += self.secs_per_sample;
        Some(sample * gain)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<I> Source for Enveloped<I>
where
    I: Source,
{
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    #[inline]
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.input.try_seek(pos)?;
        self.position = pos.as_secs_f64();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::cue_points::CuePoints;
    use rodio::buffer::SamplesBuffer;

    /// Cue in 10s, full volume at 12s, ramp down from 170s, out at 180s.
    fn shaped() -> Resolved {
        CuePoints {
            cue_in_ms: Some(10_000),
            fade_in_ms: Some(12_000),
            fade_out_ms: Some(170_000),
            cue_out_ms: Some(180_000),
            next_start_ms: None,
        }
        .resolve(Some(200.0))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn silence_before_the_in_point() {
        assert_eq!(gain_at(0.0, &shaped()), 0.0);
        assert_eq!(gain_at(9.999, &shaped()), 0.0);
    }

    #[test]
    fn the_ramp_up_runs_from_the_in_point_to_full_volume() {
        let cue = shaped();
        assert_eq!(gain_at(10.0, &cue), 0.0, "silent at the in point");
        assert!(close(gain_at(11.0, &cue), 0.5), "halfway");
        assert_eq!(gain_at(12.0, &cue), 1.0, "full volume at the fade in");
    }

    #[test]
    fn the_body_plays_untouched() {
        let cue = shaped();
        assert_eq!(gain_at(12.0, &cue), 1.0);
        assert_eq!(gain_at(90.0, &cue), 1.0);
        assert_eq!(gain_at(169.999, &cue), 1.0);
    }

    #[test]
    fn the_ramp_down_runs_from_the_fade_out_to_the_out_point() {
        let cue = shaped();
        assert_eq!(gain_at(170.0, &cue), 1.0, "full volume at the fade out");
        assert!(close(gain_at(175.0, &cue), 0.5), "halfway");
        assert_eq!(gain_at(180.0, &cue), 0.0, "silent at the out point");
    }

    #[test]
    fn silence_past_the_out_point() {
        assert_eq!(gain_at(180.0, &shaped()), 0.0);
        assert_eq!(gain_at(500.0, &shaped()), 0.0);
    }

    /// An untrimmed track is full volume everywhere; this is the case the deck
    /// skips the wrapper for entirely.
    #[test]
    fn a_track_with_no_markers_is_never_attenuated() {
        let cue = CuePoints::default().resolve(Some(200.0));
        assert!(!cue.has_fades());
        assert_eq!(gain_at(0.0, &cue), 1.0);
        assert_eq!(gain_at(100.0, &cue), 1.0);
        // The out-point is still the end of audio, envelope or not.
        assert_eq!(gain_at(200.0, &cue), 0.0);
    }

    /// Cue points with no ramps: hard in and hard out, no division by a
    /// zero-width span.
    #[test]
    fn coincident_ramp_positions_produce_no_ramp() {
        let cue = CuePoints {
            cue_in_ms: Some(10_000),
            cue_out_ms: Some(30_000),
            ..Default::default()
        }
        .resolve(Some(200.0));
        assert!(!cue.has_fades());
        assert_eq!(gain_at(9.999, &cue), 0.0);
        assert_eq!(gain_at(10.0, &cue), 1.0, "hard in, not a ramp from zero");
        assert_eq!(gain_at(29.999, &cue), 1.0);
        assert_eq!(gain_at(30.0, &cue), 0.0);
    }

    /// A ramp up on a track whose end is unknown still works: only the outro
    /// needs the file end.
    #[test]
    fn a_ramp_up_survives_an_unknown_file_duration() {
        let cue = CuePoints {
            cue_in_ms: Some(0),
            fade_in_ms: Some(2_000),
            ..Default::default()
        }
        .resolve(None);
        assert!(cue.has_fades());
        assert!(close(gain_at(1.0, &cue), 0.5));
        assert_eq!(gain_at(2.0, &cue), 1.0);
        assert_eq!(gain_at(500.0, &cue), 1.0, "no out point to fall off");
    }

    /// The envelope walks the file position forward one sample at a time, so a
    /// source handed to it mid-track is attenuated by where it really is rather
    /// than by how far it has played.
    #[test]
    fn the_envelope_tracks_absolute_file_position() {
        // 4 Hz mono, so each sample is 250 ms. Cue in at 0, full volume at 1s.
        let cue = CuePoints {
            cue_in_ms: Some(0),
            fade_in_ms: Some(1_000),
            cue_out_ms: Some(2_000),
            ..Default::default()
        }
        .resolve(Some(2.0));
        let buffer = SamplesBuffer::new(1, 4, vec![1.0f32; 8]);
        let out: Vec<f32> = Enveloped::new(buffer, 0.0, cue).collect();
        assert_eq!(out.len(), 8);
        assert!(close(out[0], 0.0), "first sample is at the in point");
        assert!(close(out[1], 0.25));
        assert!(close(out[2], 0.5));
        assert!(close(out[3], 0.75));
        assert!(close(out[4], 1.0), "full volume from the fade in on");
        assert!(close(out[7], 1.0));
    }

    /// The same buffer starting at 1s into the file is already past the ramp.
    #[test]
    fn a_source_wrapped_mid_track_starts_from_its_own_position() {
        let cue = CuePoints {
            cue_in_ms: Some(0),
            fade_in_ms: Some(1_000),
            cue_out_ms: Some(2_000),
            ..Default::default()
        }
        .resolve(Some(2.0));
        let buffer = SamplesBuffer::new(1, 4, vec![1.0f32; 4]);
        let out: Vec<f32> = Enveloped::new(buffer, 1.0, cue).collect();
        assert!(out.iter().all(|s| close(*s, 1.0)), "{out:?}");
    }

    /// A ramp is a span of wall clock, so the channel count has to divide into
    /// the per-sample step: eight interleaved stereo samples at 4 Hz are one
    /// second of audio, where eight mono samples would be two.
    ///
    /// Within a frame the channels land one sample apart on the ramp — the same
    /// approximation rodio's own `TakeDuration` makes, and 11 µs of ramp
    /// difference between left and right at 44.1 kHz.
    #[test]
    fn a_ramp_spans_wall_clock_not_sample_count() {
        let cue = CuePoints {
            cue_in_ms: Some(0),
            fade_in_ms: Some(1_000),
            cue_out_ms: Some(2_000),
            ..Default::default()
        }
        .resolve(Some(2.0));
        // 4 Hz stereo: 8 samples is 4 frames, one second, the whole ramp.
        let buffer = SamplesBuffer::new(2, 4, vec![1.0f32; 8]);
        let out: Vec<f32> = Enveloped::new(buffer, 0.0, cue).collect();
        assert!(close(out[0], 0.0));
        assert!(close(out[2], 0.25), "one frame in, a quarter of the way up");
        assert!(close(out[4], 0.5));
        assert!(close(out[7], 0.875), "still ramping at the last sample");
    }
}
