//! Amplitude-curve (waveform) computation for the seek UI.
//!
//! A track's samples are downsampled to a fixed number of RMS-energy buckets
//! ([`WAVEFORM_BUCKETS`]): each bucket holds the root-mean-square amplitude over
//! its slice of the track, normalised per-track and quantised to a `u8`
//! (0..=255). The resulting ~400-byte blob is stored per track in the DB and
//! rendered behind the deck seek bars so a DJ can see song structure (intro /
//! drop / breakdown) and seek accurately.
//!
//! RMS (perceived energy) rather than raw peak: heavily-limited modern masters
//! pin max-abs near full-scale across the whole track, hiding structure, whereas
//! RMS tracks where the energy actually rises and falls. Because RMS values are
//! small relative to full-scale, the curve is normalised to the loudest bucket
//! so it uses the full 0..=255 range — one track is shown per deck, so relative
//! scaling is what matters.

use anyhow::{Context, Result};
use ebur128::{EbuR128, Mode};
use rodio::{Decoder, Source};
use std::io::Cursor;
use std::sync::Arc;

use super::loudness::Loudness;

/// Number of buckets a track is reduced to. 400 gives enough horizontal
/// resolution for a seek bar a few hundred pixels wide while keeping the stored
/// blob tiny (one byte per bucket).
pub const WAVEFORM_BUCKETS: usize = 400;

/// Upper bound on the intermediate mip buffer. Once it fills, adjacent entries
/// are merged and the decimation doubles, keeping RAM bounded while retaining
/// resolution far finer than [`WAVEFORM_BUCKETS`] for a track of any length.
const MIP_CAP: usize = WAVEFORM_BUCKETS * 64;

type Bytes = Arc<[u8]>;

/// One intermediate mip cell: the summed square amplitude and sample count over
/// its window, so cells can be merged (on overflow) and re-bucketed by simply
/// adding the fields — RMS is derived only at the end.
#[derive(Clone, Copy, Default)]
struct Cell {
    sum_sq: f64,
    count: u64,
}

impl Cell {
    fn merge(self, other: Cell) -> Cell {
        Cell {
            sum_sq: self.sum_sq + other.sum_sq,
            count: self.count + other.count,
        }
    }

    fn rms(self) -> f32 {
        if self.count == 0 {
            0.0
        } else {
            (self.sum_sq / self.count as f64).sqrt() as f32
        }
    }
}

/// How many samples are buffered before being handed to the loudness meter.
/// Trimmed to a whole number of frames before each flush, so the meter never
/// sees a partial frame.
const METER_CHUNK: usize = 8192;

/// One decode of a track, giving both things the analysis pass needs from it.
pub struct Analysis {
    /// Exactly [`WAVEFORM_BUCKETS`] bytes.
    pub curve: Vec<u8>,
    /// `None` when the file is silent, empty, or shorter than the one 400 ms
    /// block integrated loudness needs.
    pub loudness: Option<Loudness>,
}

/// Decode an in-memory audio file once, measuring both the waveform curve and
/// the track's loudness from the one pass.
///
/// The two share the decode because it dominates the cost of either — the same
/// reason the fingerprint pass reuses these bytes rather than reading the file
/// again.
///
/// The curve is accumulated into an overflow-merging mip buffer (so no upfront
/// sample total is needed and RAM stays bounded), then downsampled to
/// [`WAVEFORM_BUCKETS`] via a float ratio and normalised. It is always exactly
/// [`WAVEFORM_BUCKETS`] bytes; a silent or empty track yields an all-zero curve
/// rather than an error.
///
/// Sample rate and channel count are read from the decoder once, up front. A
/// file that changes either mid-stream is measured as though it had not; such
/// files are rare, and the alternative is resetting the meter's integration
/// state partway through, which would bias the result far more than the span
/// change itself does.
pub fn analyze(bytes: Bytes) -> Result<Analysis> {
    let decoder = new_decoder(bytes)?;
    let channels = u32::from(decoder.channels());
    let rate = decoder.sample_rate();
    // A degenerate header leaves the meter unbuilt; the curve is still worth
    // computing, so this is not an error.
    let meter = (channels > 0 && rate > 0)
        .then(|| EbuR128::new(channels, rate, Mode::I).ok())
        .flatten();
    let mut meter = Meter {
        meter,
        buf: Vec::with_capacity(METER_CHUNK),
        channels: channels.max(1) as usize,
        peak: 0.0,
    };
    let curve = fill_buckets(decoder.inspect(|s| meter.push(*s)));
    Ok(Analysis {
        curve,
        loudness: meter.finish(),
    })
}

/// Feeds the loudness meter in whole frames while the waveform pass walks the
/// same sample stream, and tracks the peak alongside.
struct Meter {
    meter: Option<EbuR128>,
    buf: Vec<f32>,
    channels: usize,
    peak: f32,
}

impl Meter {
    fn push(&mut self, sample: f32) {
        let a = sample.abs();
        if a > self.peak {
            self.peak = a;
        }
        if self.meter.is_none() {
            return;
        }
        self.buf.push(sample);
        if self.buf.len() >= METER_CHUNK {
            self.flush();
        }
    }

    /// Hand the meter every whole frame buffered so far, keeping any trailing
    /// partial frame for the next flush.
    fn flush(&mut self) {
        let whole = self.buf.len() - self.buf.len() % self.channels;
        if whole > 0 {
            if let Some(m) = self.meter.as_mut() {
                // A meter that rejects a chunk cannot be trusted for the
                // integrated figure, so drop it and keep only the peak.
                if m.add_frames_f32(&self.buf[..whole]).is_err() {
                    self.meter = None;
                }
            }
            self.buf.drain(..whole);
        }
    }

    /// The measurement, or `None` when there was nothing to measure. A file
    /// too short for one integration block reports `-inf` LUFS rather than
    /// failing, which is not a gain this can act on.
    fn finish(mut self) -> Option<Loudness> {
        self.flush();
        let lufs = self.meter.as_ref()?.loudness_global().ok()?;
        lufs.is_finite().then_some(Loudness {
            lufs,
            peak: self.peak,
        })
    }
}

/// Decode the source once into an overflow-merging mip buffer, downsample that
/// to [`WAVEFORM_BUCKETS`] RMS buckets, and normalise to the loudest bucket.
///
/// The mip buffer avoids needing the total sample count in advance (which the
/// tag duration cannot supply accurately for VBR codecs): windows of `decim`
/// samples become one [`Cell`]; when the buffer hits [`MIP_CAP`] its cells are
/// pairwise-merged and `decim` doubles, so a track of any length collapses to at
/// most `MIP_CAP` cells — always far more than the 400 output buckets.
fn fill_buckets<S>(source: S) -> Vec<u8>
where
    S: Iterator<Item = f32>,
{
    let mut mip: Vec<Cell> = Vec::new();
    let mut decim: u64 = 1;
    let mut cur = Cell::default();

    for sample in source {
        let s = sample as f64;
        cur.sum_sq += s * s;
        cur.count += 1;
        if cur.count == decim {
            mip.push(cur);
            cur = Cell::default();
            if mip.len() == MIP_CAP {
                // Halve resolution: fold adjacent pairs, drop the tail if odd.
                let merged = mip.len() / 2;
                for j in 0..merged {
                    mip[j] = mip[2 * j].merge(mip[2 * j + 1]);
                }
                mip.truncate(merged);
                decim *= 2;
            }
        }
    }
    // Trailing partial window.
    if cur.count > 0 {
        mip.push(cur);
    }

    if mip.is_empty() {
        return vec![0u8; WAVEFORM_BUCKETS];
    }

    // Downsample the mip cells into the fixed bucket count. A float ratio maps
    // the last cell onto the last bucket, so there are no starved trailing
    // buckets (the integer-division approach this replaced left the outro blank
    // whenever the total was not a multiple of WAVEFORM_BUCKETS).
    let mut buckets = vec![Cell::default(); WAVEFORM_BUCKETS];
    let ratio = WAVEFORM_BUCKETS as f64 / mip.len() as f64;
    for (i, cell) in mip.iter().enumerate() {
        let b = ((i as f64 * ratio) as usize).min(WAVEFORM_BUCKETS - 1);
        buckets[b] = buckets[b].merge(*cell);
    }

    let rms: Vec<f32> = buckets.iter().map(|c| c.rms()).collect();
    // Per-track normalisation: RMS is small vs full-scale, so scale to the
    // loudest bucket to use the full 0..=255 range and surface structure.
    let max_rms = rms.iter().cloned().fold(0.0f32, f32::max);
    let normalised: Vec<f32> = if max_rms > 0.0 {
        rms.iter().map(|r| r / max_rms).collect()
    } else {
        rms
    };
    quantise(&normalised)
}

/// Resolution of [`compute_detail`]'s curve: one bucket per this many
/// milliseconds of audio.
pub const DETAIL_BUCKET_MS: u32 = 10;

/// Compute a fixed-time-resolution RMS curve for the cue editor's zoomed strip:
/// one byte per [`DETAIL_BUCKET_MS`] of audio, normalised like
/// [`analyze`]'s curve. Unlike the stored curve its length follows the track, so
/// a 20-minute file yields about 120 kB — computed on demand, never stored.
pub fn compute_detail(bytes: Bytes) -> Result<Vec<u8>> {
    let decoder = new_decoder(bytes)?;
    let per_bucket = u64::from(decoder.sample_rate())
        * u64::from(decoder.channels())
        * u64::from(DETAIL_BUCKET_MS)
        / 1000;
    Ok(fill_fixed(decoder, per_bucket.max(1)))
}

/// Accumulate `source` into consecutive windows of `per_bucket` samples and
/// normalise their RMS to the loudest window.
fn fill_fixed<S>(source: S, per_bucket: u64) -> Vec<u8>
where
    S: Iterator<Item = f32>,
{
    let mut cells: Vec<Cell> = Vec::new();
    let mut cur = Cell::default();
    for sample in source {
        let s = sample as f64;
        cur.sum_sq += s * s;
        cur.count += 1;
        if cur.count == per_bucket {
            cells.push(cur);
            cur = Cell::default();
        }
    }
    if cur.count > 0 {
        cells.push(cur);
    }
    let rms: Vec<f32> = cells.iter().map(|c| c.rms()).collect();
    let max_rms = rms.iter().cloned().fold(0.0f32, f32::max);
    if max_rms > 0.0 {
        quantise(&rms.iter().map(|r| r / max_rms).collect::<Vec<_>>())
    } else {
        quantise(&rms)
    }
}

/// Map normalised amplitudes (nominally 0.0..=1.0, clamped) to `u8` 0..=255.
fn quantise(peaks: &[f32]) -> Vec<u8> {
    peaks
        .iter()
        .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn new_decoder(bytes: Bytes) -> Result<Decoder<Cursor<Bytes>>> {
    Decoder::new(Cursor::new(bytes)).context("waveform decoder")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal 16-bit mono PCM WAV whose per-sample value is supplied by `f`,
    /// so a test can shape the amplitude envelope without an on-disk fixture.
    fn synth_wav_with(sample_rate: u32, samples: u32, f: impl Fn(u32) -> i16) -> Vec<u8> {
        let bits = 16u16;
        let channels = 1u16;
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let block_align = channels * (bits / 8);
        let data_len = samples * (bits as u32 / 8);
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&sample_rate.to_le_bytes());
        w.extend_from_slice(&byte_rate.to_le_bytes());
        w.extend_from_slice(&block_align.to_le_bytes());
        w.extend_from_slice(&bits.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            w.extend_from_slice(&f(i).to_le_bytes());
        }
        w
    }

    /// 16-bit mono PCM WAV that is silent for the first half and full-scale for
    /// the second, so the RMS curve is clearly low-then-high across buckets.
    fn synth_wav(sample_rate: u32, samples: u32) -> Vec<u8> {
        synth_wav_with(sample_rate, samples, |i| {
            if i < samples / 2 {
                0
            } else {
                i16::MAX
            }
        })
    }

    fn bytes_of(v: Vec<u8>) -> Bytes {
        Arc::from(v.into_boxed_slice())
    }

    #[test]
    fn returns_fixed_bucket_count() {
        let wav = bytes_of(synth_wav(8000, 8000));
        let peaks = analyze(wav).expect("compute").curve;
        assert_eq!(peaks.len(), WAVEFORM_BUCKETS);
    }

    #[test]
    fn silent_half_is_quieter_than_loud_half() {
        let wav = bytes_of(synth_wav(8000, 8000));
        let peaks = analyze(wav).expect("compute").curve;
        let first = &peaks[..WAVEFORM_BUCKETS / 2];
        let second = &peaks[WAVEFORM_BUCKETS / 2..];
        let max_first = *first.iter().max().unwrap();
        let min_second = *second.iter().min().unwrap();
        assert!(
            (max_first as u16) < (min_second as u16),
            "silent half {max_first} should be below loud half {min_second}"
        );
    }

    #[test]
    fn loud_half_reaches_full_scale() {
        let wav = bytes_of(synth_wav(8000, 8000));
        let peaks = analyze(wav).expect("compute").curve;
        // The loudest bucket normalises to 255.
        assert_eq!(*peaks.iter().max().unwrap(), 255, "loudest bucket → 255");
    }

    #[test]
    fn no_trailing_empty_buckets_when_indivisible() {
        // Sample count not a multiple of WAVEFORM_BUCKETS, with audio in the
        // final window. The old integer-division bucketing left the last
        // buckets starved (blank outro); the float ratio must fill bucket 399.
        let wav = bytes_of(synth_wav(8000, 8001));
        let peaks = analyze(wav).expect("compute").curve;
        assert!(
            peaks[WAVEFORM_BUCKETS - 1] > 0,
            "last bucket must not be starved"
        );
    }

    #[test]
    fn uniform_amplitude_is_flat() {
        // Constant full-scale track: every bucket has the same RMS, so after
        // normalisation the whole curve is flat at the top.
        let wav = bytes_of(synth_wav_with(8000, 8000, |_| i16::MAX));
        let peaks = analyze(wav).expect("compute").curve;
        assert!(peaks.iter().all(|&p| p == 255), "uniform → all 255");
    }

    #[test]
    fn garbage_bytes_error() {
        let bad = bytes_of(vec![0u8, 1, 2, 3, 4, 5]);
        assert!(analyze(bad).is_err());
    }

    #[test]
    fn empty_source_yields_zero_curve() {
        let peaks = fill_buckets(std::iter::empty::<f32>());
        assert_eq!(peaks, vec![0u8; WAVEFORM_BUCKETS]);
    }

    #[test]
    fn detail_has_one_bucket_per_ten_milliseconds() {
        // 2 s at 8 kHz mono: 200 buckets of 80 samples each.
        let wav = bytes_of(synth_wav(8000, 16_000));
        let detail = compute_detail(wav).expect("compute");
        assert_eq!(detail.len(), 200);
    }

    #[test]
    fn detail_keeps_the_partial_last_bucket() {
        let wav = bytes_of(synth_wav(8000, 16_040));
        let detail = compute_detail(wav).expect("compute");
        assert_eq!(detail.len(), 201);
        assert!(
            detail[200] > 0,
            "partial tail is loud, so it must not be empty"
        );
    }

    #[test]
    fn detail_follows_the_signal() {
        let wav = bytes_of(synth_wav(8000, 16_000));
        let detail = compute_detail(wav).expect("compute");
        assert!(detail[..100].iter().all(|&p| p == 0), "silent half");
        assert!(detail[100..].iter().all(|&p| p == 255), "loud half");
    }

    #[test]
    fn detail_of_garbage_errors() {
        assert!(compute_detail(bytes_of(vec![0u8, 1, 2, 3])).is_err());
    }

    /// One second of a 1 kHz sine at `amp` of full scale — long enough for the
    /// 400 ms block integrated loudness needs, and a waveform whose loudness is
    /// a known function of its amplitude.
    fn synth_tone(amp: f64) -> Bytes {
        let rate = 44_100u32;
        bytes_of(synth_wav_with(rate, rate, |i| {
            let t = i as f64 / rate as f64;
            (amp * i16::MAX as f64 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as i16
        }))
    }

    #[test]
    fn measures_loudness_and_peak_in_the_waveform_pass() {
        let a = analyze(synth_tone(1.0)).expect("analyze");
        assert_eq!(a.curve.len(), WAVEFORM_BUCKETS);
        let l = a.loudness.expect("a full-scale tone is measurable");
        // A full-scale sine sits near -3 LUFS; the K-weighting filter shifts it
        // a little, so this only pins the order of magnitude.
        assert!((-6.0..0.0).contains(&l.lufs), "{}", l.lufs);
        assert!(l.peak > 0.99, "{}", l.peak);
    }

    #[test]
    fn a_quieter_track_measures_quieter_by_the_amplitude_ratio() {
        let loud = analyze(synth_tone(1.0)).expect("analyze").loudness.unwrap();
        let quiet = analyze(synth_tone(0.1)).expect("analyze").loudness.unwrap();
        // A tenth of the amplitude is 20 dB down, and loudness is a dB scale.
        assert!(
            (loud.lufs - quiet.lufs - 20.0).abs() < 0.5,
            "{loud:?} {quiet:?}"
        );
        assert!((quiet.peak - 0.1).abs() < 0.01, "{}", quiet.peak);
    }

    #[test]
    fn silence_is_not_measurable() {
        let a = analyze(bytes_of(synth_wav_with(44_100, 44_100, |_| 0))).expect("analyze");
        assert_eq!(a.curve, vec![0u8; WAVEFORM_BUCKETS]);
        assert!(a.loudness.is_none());
    }

    #[test]
    fn a_track_too_short_to_integrate_is_not_measurable() {
        // 100 ms, well under the 400 ms block.
        let a = analyze(bytes_of(synth_wav_with(44_100, 4_410, |i| {
            if i % 2 == 0 {
                i16::MAX
            } else {
                i16::MIN
            }
        })))
        .expect("analyze");
        assert!(a.loudness.is_none());
    }
}
