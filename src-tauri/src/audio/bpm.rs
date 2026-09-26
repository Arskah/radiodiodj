//! Tempo measured from the analysis decode.
//!
//! The estimator never sees PCM. It works from an onset envelope — one RMS
//! value per [`ENVELOPE_MS`] of audio — collected while the waveform pass walks
//! the decoded samples ([`super::waveform::analyze`]), the same way
//! [`super::auto_cue::Collector`] collects its 50 ms windows from that one pass.
//!
//! An envelope is four orders of magnitude smaller than the audio it describes:
//! 100 values per second, so a six-minute track is ~36,000 floats. That is what
//! makes a whole-track window affordable where a PCM buffer would not be, and it
//! is enough for periodicity — a beat is an energy event, and the frequency
//! content under it carries no tempo.

use super::auto_cue::amplitude;

/// Width of one envelope value. 10 ms puts 50 frames in a beat period at
/// 120 BPM, which resolves adjacent tempi that a 50 ms grid folds together, and
/// matches the detail waveform's bucket width.
pub const ENVELOPE_MS: i64 = 10;

/// How much audio past the first audible window is measured.
///
/// Tempo is a property of a passage, not of a file, so reading further than this
/// buys nothing on a track and costs real time on a two-hour recording, where
/// the autocorrelation runs over the whole envelope.
pub const CAP_MS: i64 = 300_000;

/// What generation of this module produced a stored measurement.
///
/// Bumped when the estimator's output could differ for audio it has already
/// read, which puts the library back in the analysis queue — the mechanism
/// `library::fingerprint::VERSION` uses. A stored value carries the version that
/// produced it, so one run is never mistaken for a better one.
pub const VERSION: i64 = 1;

/// Slowest and fastest tempo reported. Wider than the music a station airs,
/// because a track at the edge should be measured rather than folded inwards;
/// the metrical level it is reported at is [`PREFERRED`]'s business.
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 200.0;

/// Where a tempo is reported when the envelope supports two metrical levels
/// equally well. Autocorrelation cannot tell 75 BPM from 150 — both periods are
/// really there — so the tie is broken towards how the music would be counted.
const PREFERRED: std::ops::RangeInclusive<f64> = 85.0..=175.0;

/// How much comb score an octave inside [`PREFERRED`] may give up and still be
/// chosen over one outside it.
const OCTAVE_MARGIN: f64 = 0.8;

/// Harmonic weights for the comb: a true period also correlates at two, three
/// and four times itself, whereas a half-period peak does not correlate at odd
/// multiples of its own lag. Summing them sharpens the peak and suppresses the
/// subdivision that plain autocorrelation scores highest.
const COMB_WEIGHTS: [f64; 4] = [1.0, 0.5, 0.25, 0.125];

/// Shortest envelope worth measuring. Below a few beat periods a peak in the
/// autocorrelation says more about the window than about the music.
const MIN_FRAMES: usize = 500;

/// Width of the moving average subtracted from the novelty curve, in frames.
/// Long enough to leave a beat's rise standing, short enough to flatten a build
/// or a fade that would otherwise read as one slow pulse.
const LOCAL_MEAN_FRAMES: usize = 100;

/// One track's measured tempo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bpm {
    pub bpm: f64,
    /// 0.0..=1.0. Low is a real answer, not an error: a spoken-word recording
    /// has no tempo to find, and the row says so rather than holding a number
    /// nothing vouches for.
    pub confidence: f32,
}

/// Accumulates the onset envelope from a sample stream.
///
/// Collection starts at the first window whose RMS clears the operator's silence
/// threshold and stops [`CAP_MS`] later. Gating on that threshold rather than a
/// constant of its own is what makes "audible" mean the same thing here as it
/// does to the automatic cue points: a fade-in the operator has told the app to
/// treat as silence is not fed to the estimator as rhythm.
pub struct Collector {
    /// Interleaved samples per millisecond: `rate × channels / 1000`. Kept as a
    /// float because it is not whole at every rate.
    samples_per_ms: f64,
    /// Sample count at which the window being filled closes.
    boundary: u64,
    samples: u64,
    sum_sq: f64,
    count: u64,
    /// Windows closed so far, latched or not. The envelope holds only the
    /// latched ones, so its length cannot serve as the window index.
    closed: u64,
    /// Linear RMS of every window from the first audible one on.
    envelope: Vec<f32>,
    /// The RMS a window has to exceed to open the latch.
    gate: f64,
    /// How many windows are collected once the latch is open.
    cap: usize,
    latched: bool,
}

impl Collector {
    /// `rate` and `channels` come from the decoder header; a degenerate header
    /// (either zero) yields a collector that produces no windows and therefore
    /// no measurement, rather than a bogus one.
    pub fn new(rate: u32, channels: u16, silence_dbfs: f64) -> Self {
        let mut c = Self {
            samples_per_ms: f64::from(rate) * f64::from(channels) / 1000.0,
            boundary: 0,
            samples: 0,
            sum_sq: 0.0,
            count: 0,
            closed: 0,
            envelope: Vec::new(),
            gate: amplitude(silence_dbfs),
            cap: (CAP_MS / ENVELOPE_MS) as usize,
            latched: false,
        };
        c.boundary = c.next_boundary();
        c
    }

    /// Where the next window ends, computed from the window index rather than by
    /// adding a fixed width, so a rate that puts a fractional sample count in a
    /// window cannot slide the grid against the clock. The automatic cue
    /// collector computes its boundaries the same way and for the same reason.
    fn next_boundary(&self) -> u64 {
        let windows = self.closed as f64 + 1.0;
        (((windows * ENVELOPE_MS as f64 * self.samples_per_ms).round()) as u64)
            .max(self.samples + 1)
    }

    pub fn push(&mut self, sample: f32) {
        if self.samples_per_ms <= 0.0 || self.full() {
            return;
        }
        self.samples += 1;
        let s = f64::from(sample);
        self.sum_sq += s * s;
        self.count += 1;
        if self.samples >= self.boundary {
            self.close_window();
        }
    }

    fn close_window(&mut self) {
        let rms = (self.sum_sq / self.count as f64).sqrt();
        self.sum_sq = 0.0;
        self.count = 0;
        self.closed += 1;
        self.boundary = self.next_boundary();
        self.latched |= rms > self.gate;
        if self.latched {
            self.envelope.push(rms as f32);
        }
    }

    /// Whether the cap has been reached, after which the rest of the decode
    /// costs this collector nothing.
    fn full(&self) -> bool {
        self.latched && self.envelope.len() >= self.cap
    }

    /// The measured tempo, or `None` when the audio held none to find.
    pub fn finish(mut self) -> Option<Bpm> {
        if self.count > 0 && !self.full() {
            self.close_window();
        }
        estimate(&self.envelope)
    }
}

/// Measure the tempo of an onset envelope.
///
/// Three stages: a novelty curve of where energy rises, an autocorrelation of
/// that curve combed over harmonics, and the choice of which metrical level to
/// report the winning period at.
fn estimate(envelope: &[f32]) -> Option<Bpm> {
    let novelty = novelty(envelope);
    if novelty.len() < MIN_FRAMES {
        return None;
    }
    let min_lag = lag_of(MAX_BPM);
    let max_lag = lag_of(MIN_BPM).min(novelty.len() / 3);
    if min_lag >= max_lag {
        return None;
    }
    let correlation: Vec<f64> = (0..=max_lag).map(|lag| correlate(&novelty, lag)).collect();
    let comb: Vec<f64> = (0..=max_lag)
        .map(|lag| comb_score(&correlation, lag, max_lag))
        .collect();
    let best = (min_lag..=max_lag).max_by(|&a, &b| comb[a].total_cmp(&comb[b]))?;
    if comb[best] <= 0.0 {
        return None;
    }
    let lag = octave(&comb, best, min_lag, max_lag);
    let refined = refine(&comb, lag);
    Some(Bpm {
        bpm: 60_000.0 / (refined * ENVELOPE_MS as f64),
        confidence: confidence(&comb, lag, min_lag, max_lag),
    })
}

/// Where energy rises, in decibels per frame, with the local trend removed.
///
/// The rise is measured in the log domain so the curve does not depend on how
/// loud the track was mastered, and rectified because only onsets carry tempo —
/// a decay is the previous beat ending, not the next one starting. Subtracting a
/// moving average then removes what a build-up or a long fade contributes, which
/// would otherwise correlate at the length of the build rather than the beat.
fn novelty(envelope: &[f32]) -> Vec<f64> {
    /// Keeps the log of a silent window finite without reaching a level any
    /// resolved envelope value occupies.
    const FLOOR: f64 = 1e-9;

    let db: Vec<f64> = envelope
        .iter()
        .map(|&r| 20.0 * (f64::from(r).max(FLOOR)).log10())
        .collect();
    let flux: Vec<f64> = db.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    let mean = local_mean(&flux, LOCAL_MEAN_FRAMES);
    let rectified: Vec<f64> = flux
        .iter()
        .zip(&mean)
        .map(|(f, m)| (f - m).max(0.0))
        .collect();
    smooth(&rectified)
}

/// Spread each onset over its neighbouring frames, with a triangular kernel.
///
/// A beat period is rarely a whole number of frames — 174 BPM is 34.48 of them —
/// so successive onsets straddle the grid differently and a sharp curve
/// correlates with itself at even multiples of the period but not at odd ones,
/// which reports the tempo an octave low. Smoothing costs the peak nothing it
/// has not already lost to the window width.
fn smooth(novelty: &[f64]) -> Vec<f64> {
    (0..novelty.len())
        .map(|i| {
            let left = if i > 0 { novelty[i - 1] } else { 0.0 };
            let right = novelty.get(i + 1).copied().unwrap_or(0.0);
            0.25 * left + 0.5 * novelty[i] + 0.25 * right
        })
        .collect()
}

/// A centred moving average of `width` frames, shortened at both ends rather
/// than zero-padded, so the curve's start is not read as a rise.
fn local_mean(flux: &[f64], width: usize) -> Vec<f64> {
    let half = width / 2;
    let sums: Vec<f64> = std::iter::once(0.0)
        .chain(flux.iter().scan(0.0, |acc, v| {
            *acc += v;
            Some(*acc)
        }))
        .collect();
    (0..flux.len())
        .map(|i| {
            let from = i.saturating_sub(half);
            let to = (i + half + 1).min(flux.len());
            (sums[to] - sums[from]) / (to - from) as f64
        })
        .collect()
}

/// Autocorrelation at one lag, normalised by the overlap so a long lag is not
/// penalised for the samples it cannot reach.
fn correlate(novelty: &[f64], lag: usize) -> f64 {
    let overlap = novelty.len().saturating_sub(lag);
    if overlap == 0 {
        return 0.0;
    }
    let sum: f64 = novelty[lag..].iter().zip(novelty).map(|(a, b)| a * b).sum();
    sum / overlap as f64
}

/// The correlation at `lag` plus the support its harmonics give it.
fn comb_score(correlation: &[f64], lag: usize, max_lag: usize) -> f64 {
    COMB_WEIGHTS
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let harmonic = lag * (i + 1);
            (harmonic <= max_lag).then(|| w * correlation[harmonic])
        })
        .sum()
}

/// Which metrical level to report the winning period at.
///
/// Half and double the period both correlate — they are the same grid counted
/// differently — so the one landing in [`PREFERRED`] is taken unless it scores
/// worse than [`OCTAVE_MARGIN`] of the winner, which is what stops a genuinely
/// fast or slow track from being dragged into the middle.
fn octave(comb: &[f64], best: usize, min_lag: usize, max_lag: usize) -> usize {
    if PREFERRED.contains(&bpm_of(best)) {
        return best;
    }
    [best / 2, best * 2]
        .into_iter()
        .filter(|&lag| (min_lag..=max_lag).contains(&lag) && PREFERRED.contains(&bpm_of(lag)))
        .find(|&lag| comb[lag] >= comb[best] * OCTAVE_MARGIN)
        .unwrap_or(best)
}

/// Interpolate the true peak between whole lags, by fitting a parabola to the
/// winner and its neighbours. Whole lags are 6.7 BPM apart at 200 BPM, which is
/// coarser than the tempo itself is stable.
fn refine(comb: &[f64], lag: usize) -> f64 {
    if lag == 0 || lag + 1 >= comb.len() {
        return lag as f64;
    }
    let (left, mid, right) = (comb[lag - 1], comb[lag], comb[lag + 1]);
    let denominator = 2.0 * (2.0 * mid - left - right);
    if denominator.abs() < f64::EPSILON {
        return lag as f64;
    }
    lag as f64 + (right - left) / denominator
}

/// How much the winning period stands out from every other candidate.
///
/// Measured against the mean rather than the runner-up, because the runner-up is
/// usually the winner's own harmonic and says nothing about whether the track
/// has a pulse at all.
fn confidence(comb: &[f64], lag: usize, min_lag: usize, max_lag: usize) -> f32 {
    let span = &comb[min_lag..=max_lag];
    let mean = span.iter().sum::<f64>() / span.len() as f64;
    if comb[lag] <= 0.0 || mean <= 0.0 {
        return 0.0;
    }
    (1.0 - mean / comb[lag]).clamp(0.0, 1.0) as f32
}

/// Frames in one beat period at `bpm`.
fn lag_of(bpm: f64) -> usize {
    (60_000.0 / (bpm * ENVELOPE_MS as f64)).round() as usize
}

fn bpm_of(lag: usize) -> f64 {
    60_000.0 / (lag as f64 * ENVELOPE_MS as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped silence threshold.
    const SILENT_DBFS: f64 = -70.0;

    /// A click train at `bpm`, as the collector would see it: one full-scale
    /// frame on each beat, near-silence between. `mono` samples at 8 kHz keeps
    /// the fixtures small; the collector works in milliseconds, so the rate only
    /// has to be enough to place a beat inside its own window.
    fn clicks(bpm: f64, secs: f64) -> Collector {
        let rate = 8_000u32;
        let mut c = Collector::new(rate, 1, SILENT_DBFS);
        let period = 60.0 / bpm * f64::from(rate);
        let total = (secs * f64::from(rate)) as u64;
        for i in 0..total {
            let since = (i as f64 % period).round() as u64;
            c.push(if since < 40 { 0.9 } else { 0.001 });
        }
        c
    }

    #[test]
    fn a_click_train_measures_its_own_tempo() {
        for expected in [90.0, 100.0, 120.0, 128.0, 140.0, 174.0] {
            let got = clicks(expected, 30.0).finish().expect("a tempo");
            assert!(
                (got.bpm - expected).abs() <= 1.0,
                "{expected} BPM read as {:.2}",
                got.bpm
            );
            assert!(got.confidence > 0.3, "{expected} BPM read weakly");
        }
    }

    #[test]
    fn a_tempo_under_the_preferred_range_is_not_doubled() {
        let got = clicks(70.0, 30.0).finish().expect("a tempo");
        assert!(
            (got.bpm - 70.0).abs() <= 1.0,
            "70 BPM read as {:.2}",
            got.bpm
        );
    }

    #[test]
    fn leading_silence_does_not_move_the_tempo() {
        let rate = 8_000u32;
        let mut gated = Collector::new(rate, 1, SILENT_DBFS);
        for _ in 0..rate * 5 {
            gated.push(0.0);
        }
        let period = 60.0 / 128.0 * f64::from(rate);
        for i in 0..u64::from(rate) * 30 {
            let since = (i as f64 % period).round() as u64;
            gated.push(if since < 40 { 0.9 } else { 0.001 });
        }
        let with_silence = gated.finish().expect("a tempo");
        let without = clicks(128.0, 30.0).finish().expect("a tempo");
        assert!(
            (with_silence.bpm - without.bpm).abs() < 0.5,
            "{:.2} against {:.2}",
            with_silence.bpm,
            without.bpm
        );
    }

    #[test]
    fn silence_has_no_tempo() {
        let mut c = Collector::new(8_000, 1, SILENT_DBFS);
        for _ in 0..8_000 * 10 {
            c.push(0.0);
        }
        assert_eq!(c.finish(), None);
    }

    #[test]
    fn a_track_too_short_to_judge_has_no_tempo() {
        assert_eq!(clicks(128.0, 2.0).finish(), None);
    }

    #[test]
    fn a_degenerate_header_has_no_tempo() {
        let mut c = Collector::new(0, 0, SILENT_DBFS);
        for _ in 0..10_000 {
            c.push(0.9);
        }
        assert_eq!(c.finish(), None);
    }

    #[test]
    fn the_cap_bounds_the_envelope() {
        let mut c = Collector::new(1_000, 1, SILENT_DBFS);
        let frames = (CAP_MS + 60_000) as u64;
        for _ in 0..frames {
            c.push(0.9);
        }
        assert_eq!(c.envelope.len(), (CAP_MS / ENVELOPE_MS) as usize);
    }

    #[test]
    fn a_steady_tone_carries_no_pulse() {
        let mut c = Collector::new(8_000, 1, SILENT_DBFS);
        for i in 0..8_000 * 30 {
            c.push(if i % 2 == 0 { 0.5 } else { -0.5 });
        }
        let measured = c.finish();
        assert!(
            measured.is_none_or(|b| b.confidence < 0.3),
            "a tone read as {measured:?}"
        );
    }
}

/// Scoring the estimator against a real library, which no fixture can stand in
/// for: the synthesized click trains above say the arithmetic is right, not that
/// it is right about music.
///
/// Ignored by default — it needs audio nobody can check in. Point `BPM_CORPUS` at
/// a directory of tracks carrying vendor BPM tags and run:
///
/// ```text
/// BPM_CORPUS=~/Music cargo test --manifest-path src-tauri/Cargo.toml \
///     -- --ignored --nocapture measured_against_a_tagged_library
/// ```
///
/// The tags are ground truth only as far as the taggers were right, so the
/// report lists its worst disagreements rather than only a percentage: an octave
/// error and a vendor's own mistake look identical in the aggregate.
#[cfg(test)]
mod corpus {
    use super::*;
    use crate::audio::waveform;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::*;
    use lofty::probe::Probe;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const SILENT_DBFS: f64 = -70.0;

    fn tagged_bpm(path: &Path) -> Option<f64> {
        let file = Probe::open(path).ok()?.read().ok()?;
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        tag.get_string(ItemKey::Bpm)?.trim().parse().ok()
    }

    fn files(root: &Path) -> Vec<PathBuf> {
        let mut stack = vec![root.to_path_buf()];
        let mut out = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    #[ignore]
    fn measured_against_a_tagged_library() {
        let Some(root) = std::env::var_os("BPM_CORPUS") else {
            eprintln!("set BPM_CORPUS to a directory of tagged audio");
            return;
        };
        let mut scored: Vec<(PathBuf, f64, Bpm)> = Vec::new();
        let mut unmeasured = 0usize;
        for path in files(Path::new(&root)) {
            let Some(tagged) = tagged_bpm(&path) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(analysis) = waveform::analyze(Arc::from(bytes.into_boxed_slice()), SILENT_DBFS)
            else {
                continue;
            };
            match analysis.bpm {
                Some(measured) => scored.push((path, tagged, measured)),
                None => unmeasured += 1,
            }
        }
        if scored.is_empty() {
            eprintln!("no tagged, decodable audio under {root:?}");
            return;
        }

        let error = |&(_, tagged, measured): &(PathBuf, f64, Bpm)| (measured.bpm - tagged).abs();
        let within = |limit: f64| scored.iter().filter(|s| error(s) <= limit).count();
        let octave = scored
            .iter()
            .filter(|&&(_, tagged, m)| [0.5, 2.0].iter().any(|f| (m.bpm - tagged * f).abs() <= 2.0))
            .count();
        let total = scored.len();
        let mean: f64 = scored.iter().map(error).sum::<f64>() / total as f64;
        let percent = |n: usize| 100.0 * n as f64 / total as f64;

        println!("\n{total} tagged tracks measured, {unmeasured} with no tempo found");
        println!("  within 1 BPM  {:>5.1}%", percent(within(1.0)));
        println!("  within 2 BPM  {:>5.1}%", percent(within(2.0)));
        println!("  half/double   {:>5.1}%", percent(octave));
        println!("  mean error    {mean:>5.2} BPM");

        let mut worst = scored;
        worst.sort_by(|a, b| error(b).total_cmp(&error(a)));
        println!("\nworst disagreements:");
        for (path, tagged, measured) in worst.iter().take(15) {
            println!(
                "  tag {tagged:>6.1}  measured {:>6.1}  confidence {:.2}  {}",
                measured.bpm,
                measured.confidence,
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
}
