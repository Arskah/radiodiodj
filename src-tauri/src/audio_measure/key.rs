//! Musical key measured from the analysis decode.
//!
//! The estimator works from a **chroma profile** — how much energy the track
//! spent in each of the twelve pitch classes — accumulated while the waveform
//! pass walks the decoded samples ([`super::waveform::analyze`]), the same way
//! [`super::bpm::Collector`] accumulates its onset envelope from that one walk.
//!
//! Unlike every other consumer of that walk, this one needs **frames rather than
//! samples**. Loudness, the waveform curve, the level envelope and the tempo all
//! reduce to sums of squares, for which an interleaved stream is as good as a
//! deinterleaved one. A spectrum is not: analysing `L,R,L,R…` as though it were
//! one signal reads every frequency at half its true value with a mirror image
//! folded on top. So the collector downmixes to mono frames before it transforms
//! anything.
//!
//! What it reports is a key — a pitch class and a mode. Naming that key `8A` for
//! a DJ reading a Camelot wheel is presentation, and lives on the app side; this
//! module spells the note and stops. See `docs/audio-measure.md`.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Mono samples in one analysed frame. At 44.1 kHz this is 186 ms of audio and
/// a bin every 5.4 Hz, which resolves adjacent semitones from [`MIN_HZ`] up —
/// the shorter frames the other collectors use would blur a semitone into its
/// neighbour across the whole low register.
const FRAME: usize = 8192;

/// How far the frame advances between transforms. Half a frame, so a chord
/// struck across a frame boundary is measured whole by the next one.
const HOP: usize = FRAME / 2;

/// The band folded into the profile.
///
/// Below [`MIN_HZ`] a bin is wider than the semitone it would have to resolve,
/// so the bass register votes for its neighbours as readily as for itself. Above
/// [`MAX_HZ`] what is left is mostly the upper harmonics of notes already
/// counted lower down, plus cymbals, which belong to no pitch class at all.
const MIN_HZ: f64 = 130.0;
const MAX_HZ: f64 = 2100.0;

/// How much audio past the start is measured.
///
/// A track establishes its key in its opening minutes, and reading further buys
/// nothing on a song while costing real time on a two-hour recording — the same
/// trade [`super::bpm::CAP_MS`] makes, for the same reason.
pub const CAP_MS: i64 = 300_000;

/// Frames whose band energy is below this are skipped rather than normalised.
///
/// Every frame contributes a unit-sum profile so a loud chorus cannot outvote a
/// quiet verse, and normalising a frame that holds only dither would smear that
/// dither across all twelve pitch classes as though it were music.
const FRAME_FLOOR: f32 = 1e-6;

/// Fewest frames worth judging. Below a couple of seconds a profile describes
/// one chord rather than a key.
const MIN_FRAMES: usize = 20;

/// What generation of this module produced a stored measurement.
///
/// Bumped when the estimator's output could differ for audio it has already
/// read, which puts the library back in the analysis queue — the mechanism
/// [`super::fingerprint::VERSION`] and [`super::bpm::VERSION`] use.
pub const VERSION: i64 = 1;

/// Krumhansl–Kessler profiles: how strongly each scale degree is expected to be
/// heard, as listener ratings rather than as a count of notes in the scale. The
/// difference matters — a scale mask scores a major key and its own relative
/// minor identically, because they hold the same seven notes.
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Whether a key is major or minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

/// One track's measured key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Semitones above C: `0` is C, `9` is A.
    pub pitch_class: u8,
    pub mode: Mode,
    /// 0.0..=1.0. Low is a real answer, not an error: a drum loop has no key to
    /// find, and the row says so rather than holding a name nothing vouches for.
    pub confidence: f32,
}

/// Note names, chosen to match the spelling a Camelot wheel uses for the same
/// key, so one table serves both notations rather than two disagreeing about
/// whether pitch class 1 is D flat or C sharp.
const MAJOR_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];
const MINOR_NAMES: [&str; 12] = [
    "Cm", "C#m", "Dm", "D#m", "Em", "Fm", "F#m", "Gm", "G#m", "Am", "Bbm", "Bm",
];

impl Key {
    /// The key as a note name — `Am`, `F#m`, `Db`. This is what the library
    /// stores, because it is the vocabulary a tagger writes into a file's own key
    /// field, which is what makes a measured key and a tagged one comparable.
    pub fn name(&self) -> &'static str {
        let i = (self.pitch_class % 12) as usize;
        match self.mode {
            Mode::Major => MAJOR_NAMES[i],
            Mode::Minor => MINOR_NAMES[i],
        }
    }
}

/// Accumulates a chroma profile from a sample stream.
pub struct Collector {
    /// Interleaved samples per channel-frame, from the decoder header.
    channels: usize,
    /// Partial channel-frame: samples seen and their sum, averaged to one mono
    /// sample once the frame is whole.
    pending: f32,
    pending_count: usize,
    /// Mono samples awaiting a transform. Holds at most [`FRAME`].
    frame: Vec<f32>,
    /// Pitch class each output bin votes for, or `None` for one outside the
    /// analysed band. Computed once, since the rate cannot change mid-decode.
    bins: Vec<Option<u8>>,
    /// Hann window, applied before every transform.
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Summed per-frame profiles, one entry per pitch class.
    chroma: [f64; 12],
    frames: usize,
    /// How many frames are measured before the rest of the decode is ignored.
    cap: usize,
}

impl Collector {
    /// `rate` and `channels` come from the decoder header; a degenerate header
    /// (either zero) yields a collector that produces no frames and therefore no
    /// measurement, rather than a bogus one.
    pub fn new(rate: u32, channels: u16) -> Self {
        Self::with_cap(rate, channels, CAP_MS)
    }

    /// The span a collector reads is a parameter only so a test can compare what
    /// two parts of one track measure. The app always uses [`CAP_MS`].
    fn with_cap(rate: u32, channels: u16, cap_ms: i64) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME);
        let hops_per_ms = f64::from(rate) / (HOP as f64 * 1000.0);
        Self {
            channels: channels as usize,
            pending: 0.0,
            pending_count: 0,
            frame: Vec::with_capacity(FRAME),
            bins: bin_classes(rate),
            window: hann(FRAME),
            scratch: fft.make_output_vec(),
            fft,
            chroma: [0.0; 12],
            frames: 0,
            cap: ((cap_ms as f64 * hops_per_ms).round() as usize).max(1),
        }
    }

    /// Whether the cap has been reached, after which the rest of the decode
    /// costs this collector nothing.
    fn full(&self) -> bool {
        self.frames >= self.cap
    }

    pub fn push(&mut self, sample: f32) {
        if self.channels == 0 || self.bins.is_empty() || self.full() {
            return;
        }
        self.pending += sample;
        self.pending_count += 1;
        if self.pending_count < self.channels {
            return;
        }
        let mono = self.pending / self.channels as f32;
        self.pending = 0.0;
        self.pending_count = 0;
        self.frame.push(mono);
        if self.frame.len() == FRAME {
            self.transform();
        }
    }

    /// Transform the buffered frame into the profile and advance by [`HOP`].
    fn transform(&mut self) {
        let mut input: Vec<f32> = self
            .frame
            .iter()
            .zip(&self.window)
            .map(|(s, w)| s * w)
            .collect();
        self.frame.drain(..HOP);
        if self.fft.process(&mut input, &mut self.scratch).is_err() {
            return;
        }
        let mut profile = [0.0f32; 12];
        let mut total = 0.0f32;
        for (bin, class) in self.scratch.iter().zip(&self.bins) {
            if let Some(c) = class {
                // Magnitude, not power: squaring hands the loudest partial in the
                // frame a vote several times the size of the chord under it.
                let magnitude = bin.norm();
                profile[*c as usize] += magnitude;
                total += magnitude;
            }
        }
        self.frames += 1;
        if total <= FRAME_FLOOR {
            return;
        }
        for (sum, value) in self.chroma.iter_mut().zip(&profile) {
            *sum += f64::from(*value / total);
        }
    }

    /// The measured key, or `None` when the audio held none to find.
    pub fn finish(mut self) -> Option<Key> {
        if !self.full() && self.frame.len() > HOP {
            // A trailing part-frame is zero-padded rather than dropped, so a
            // track shorter than one frame is still measured.
            self.frame.resize(FRAME, 0.0);
            self.transform();
        }
        if self.frames < MIN_FRAMES {
            return None;
        }
        estimate(&self.chroma)
    }
}

/// Match a chroma profile against all twenty-four keys.
///
/// Each candidate is the profile correlated against [`MAJOR`] or [`MINOR`]
/// rotated to that tonic. Pearson rather than a dot product, because a profile's
/// own mean and spread say nothing about which key it is — an untuned
/// correlation would simply prefer whichever template is largest.
fn estimate(chroma: &[f64; 12]) -> Option<Key> {
    let mut scores = Vec::with_capacity(24);
    for pitch_class in 0..12u8 {
        for (mode, profile) in [(Mode::Major, &MAJOR), (Mode::Minor, &MINOR)] {
            scores.push((pitch_class, mode, correlate(chroma, profile, pitch_class)));
        }
    }
    let &(pitch_class, mode, best) = scores
        .iter()
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .expect("twenty-four candidates");
    if best <= 0.0 {
        return None;
    }
    Some(Key {
        pitch_class,
        mode,
        confidence: confidence(&scores, best),
    })
}

/// Pearson correlation of a profile against `template` rotated so that
/// `tonic` is its first degree.
fn correlate(chroma: &[f64; 12], template: &[f64; 12], tonic: u8) -> f64 {
    let rotated: Vec<f64> = (0..12)
        .map(|i| template[(i + 12 - tonic as usize) % 12])
        .collect();
    let mean_c = chroma.iter().sum::<f64>() / 12.0;
    let mean_t = rotated.iter().sum::<f64>() / 12.0;
    let mut covariance = 0.0;
    let mut var_c = 0.0;
    let mut var_t = 0.0;
    for (c, t) in chroma.iter().zip(&rotated) {
        let (dc, dt) = (c - mean_c, t - mean_t);
        covariance += dc * dt;
        var_c += dc * dc;
        var_t += dt * dt;
    }
    let spread = (var_c * var_t).sqrt();
    if spread <= 0.0 {
        return 0.0;
    }
    covariance / spread
}

/// How far the winning key stands out from the field.
///
/// Measured against the mean of all twenty-four candidates rather than the
/// runner-up, which is usually the winner's own relative major or minor — those
/// two share seven notes, so their scores track each other and say nothing about
/// whether the track is in a key at all. The same reasoning as
/// [`super::bpm`]'s confidence, where the runner-up is a harmonic.
///
/// Normalised by the headroom above that mean, so a profile no template fits
/// well cannot report a high confidence merely by fitting one slightly better.
fn confidence(scores: &[(u8, Mode, f64)], best: f64) -> f32 {
    let mean = scores.iter().map(|s| s.2).sum::<f64>() / scores.len() as f64;
    if mean >= 1.0 {
        return 0.0;
    }
    (((best - mean) / (1.0 - mean)).clamp(0.0, 1.0)) as f32
}

/// Which pitch class each output bin votes for, or `None` outside the band.
///
/// Precomputed because the alternative is a logarithm per bin per frame, and
/// there are `FRAME / 2 + 1` bins in every one of them.
fn bin_classes(rate: u32) -> Vec<Option<u8>> {
    if rate == 0 {
        return Vec::new();
    }
    let bins = FRAME / 2 + 1;
    (0..bins)
        .map(|bin| {
            let hz = bin as f64 * f64::from(rate) / FRAME as f64;
            if !(MIN_HZ..=MAX_HZ).contains(&hz) {
                return None;
            }
            // MIDI number of the nearest semitone, folded to a pitch class where
            // 0 is C: A4 = 440 Hz is MIDI 69, and 69 mod 12 is 9.
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            Some((midi.round().rem_euclid(12.0)) as u8)
        })
        .collect()
}

/// Hann window of `len` points, so a note's energy stays in its own bins rather
/// than leaking across the spectrum from the frame's own edges.
fn hann(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / len as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// Feed `collector` a mono signal built by `f`, sample by sample.
    fn feed(collector: &mut Collector, samples: usize, mut f: impl FnMut(f64) -> f32) {
        for i in 0..samples {
            collector.push(f(i as f64 / f64::from(RATE)));
        }
    }

    /// One note as an instrument sounds it: a fundamental plus falling partials.
    ///
    /// Timbre is not decoration here. A partial series reinforces its own pitch
    /// class at the octave and its fifth two partials up, which is a large part
    /// of why a listener — and this estimator — hears a root as a root. A chord
    /// of bare equal-amplitude sines carries none of that and is genuinely
    /// ambiguous: `Db F Ab` sounded that way has no more claim to being D flat
    /// major than to being the upper voices of F minor.
    fn note(midi: i32, t: f64) -> f64 {
        let hz = 440.0 * 2f64.powf(f64::from(midi - 69) / 12.0);
        (1..=4)
            .map(|h| {
                let partial = 2.0 * std::f64::consts::PI * hz * f64::from(h) * t;
                partial.sin() / f64::from(h)
            })
            .sum()
    }

    /// Sum of `notes`, scaled to stay inside full scale.
    fn chord(notes: &[i32]) -> impl Fn(f64) -> f32 + '_ {
        move |t| {
            let sum: f64 = notes.iter().map(|&n| note(n, t)).sum();
            (sum / (2.0 * notes.len() as f64)) as f32
        }
    }

    /// MIDI numbers for a triad rooted at `root`, voiced as music voices one:
    /// the root in the bass under a close triad above it. The bass note is what
    /// tells a listener which of a chord's notes is the root, so leaving it out
    /// would be testing the estimator against a stimulus no track contains.
    fn triad(root: i32, mode: Mode) -> [i32; 4] {
        let third = match mode {
            Mode::Major => 4,
            Mode::Minor => 3,
        };
        [48 + root, 60 + root, 60 + root + third, 60 + root + 7]
    }

    fn measure(notes: &[i32], seconds: usize) -> Option<Key> {
        let mut c = Collector::new(RATE, 1);
        feed(&mut c, RATE as usize * seconds, chord(notes));
        c.finish()
    }

    #[test]
    fn a_c_major_triad_reads_as_c_major() {
        let key = measure(&triad(0, Mode::Major), 5).expect("a key");
        assert_eq!(key.pitch_class, 0, "read as {}", key.name());
        assert_eq!(key.mode, Mode::Major, "read as {}", key.name());
    }

    #[test]
    fn an_a_minor_triad_reads_as_a_minor() {
        let key = measure(&triad(9, Mode::Minor), 5).expect("a key");
        assert_eq!(key.name(), "Am");
    }

    /// The whole point of the Krumhansl weights over a scale mask: C major and
    /// A minor hold the same seven notes, so only the weighting can tell a
    /// progression centred on one from one centred on the other.
    #[test]
    fn the_tonic_decides_between_relative_keys() {
        // Both progressions use only white notes. The first leans on C, the
        // second on A.
        let c_major = measure(&[60, 64, 67, 60, 64, 67, 72], 5).expect("a key");
        let a_minor = measure(&[57, 60, 64, 57, 60, 64, 69], 5).expect("a key");
        assert_eq!(c_major.name(), "C");
        assert_eq!(a_minor.name(), "Am");
    }

    #[test]
    fn every_pitch_class_is_found_in_major() {
        for root in 0..12i32 {
            let key = measure(&triad(root, Mode::Major), 4).expect("a key");
            assert_eq!(
                key.pitch_class,
                root as u8,
                "root {root} read as {}",
                key.name()
            );
        }
    }

    #[test]
    fn silence_has_no_key() {
        let mut c = Collector::new(RATE, 1);
        feed(&mut c, RATE as usize * 5, |_| 0.0);
        assert!(c.finish().is_none());
    }

    /// A profile with no tonal centre must report low confidence rather than
    /// whichever of twenty-four templates happened to fit a flat spectrum best.
    #[test]
    fn noise_is_reported_as_unsure() {
        let mut c = Collector::new(RATE, 1);
        // Deterministic pseudo-noise: no pitch class is favoured.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        feed(&mut c, RATE as usize * 5, |_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state as f32 / u64::MAX as f32) * 2.0 - 1.0
        });
        let key = c.finish().expect("noise still correlates with something");
        assert!(key.confidence < 0.5, "confidence {}", key.confidence);
    }

    #[test]
    fn a_clip_too_short_to_judge_has_no_key() {
        let mut c = Collector::new(RATE, 1);
        feed(&mut c, RATE as usize / 2, chord(&triad(0, Mode::Major)));
        assert!(c.finish().is_none());
    }

    /// The reason this collector downmixes at all: a stereo file must measure
    /// what the same audio measures in mono, not an interleaved artefact.
    #[test]
    fn a_stereo_track_measures_what_its_mono_does() {
        let notes = triad(7, Mode::Major);
        let mono = measure(&notes, 5).expect("a key");

        let mut stereo = Collector::new(RATE, 2);
        let signal = chord(&notes);
        for i in 0..RATE as usize * 5 {
            let s = signal(i as f64 / f64::from(RATE));
            stereo.push(s);
            stereo.push(s);
        }
        let key = stereo.finish().expect("a key");
        assert_eq!(key.name(), mono.name());
    }

    #[test]
    fn a_degenerate_header_measures_nothing() {
        let mut no_rate = Collector::new(0, 2);
        feed(&mut no_rate, RATE as usize, chord(&triad(0, Mode::Major)));
        assert!(no_rate.finish().is_none());

        let mut no_channels = Collector::new(RATE, 0);
        feed(
            &mut no_channels,
            RATE as usize,
            chord(&triad(0, Mode::Major)),
        );
        assert!(no_channels.finish().is_none());
    }

    /// The cap is what stops a two-hour recording paying for a transform per hop
    /// all the way through.
    #[test]
    fn the_cap_stops_the_measurement() {
        let mut c = Collector::with_cap(RATE, 1, 1_000);
        feed(&mut c, RATE as usize * 5, chord(&triad(0, Mode::Major)));
        let hops = f64::from(RATE) / HOP as f64;
        assert!(
            c.frames <= hops.round() as usize + 1,
            "{} frames for a one-second cap",
            c.frames
        );
    }

    #[test]
    fn every_key_has_a_name() {
        for pitch_class in 0..12u8 {
            for mode in [Mode::Major, Mode::Minor] {
                let key = Key {
                    pitch_class,
                    mode,
                    confidence: 1.0,
                };
                assert!(!key.name().is_empty());
            }
        }
    }

    /// Bins outside the band vote for nothing, and the band is where semitones
    /// are resolvable.
    #[test]
    fn only_the_analysed_band_votes() {
        let bins = bin_classes(RATE);
        let hz = |bin: usize| bin as f64 * f64::from(RATE) / FRAME as f64;
        for (bin, class) in bins.iter().enumerate() {
            let inside = (MIN_HZ..=MAX_HZ).contains(&hz(bin));
            assert_eq!(class.is_some(), inside, "bin {bin} at {:.1} Hz", hz(bin));
        }
    }

    #[test]
    fn a_440_hz_bin_votes_for_a() {
        let bins = bin_classes(RATE);
        let bin = (440.0 * FRAME as f64 / f64::from(RATE)).round() as usize;
        assert_eq!(bins[bin], Some(9), "A4 is pitch class 9");
    }
}

/// What the estimator says about real music, run against a directory of audio.
///
/// Reports rather than asserts, like [`super::bpm`]'s survey: a tag is poor
/// ground truth for a key — most libraries carry none, and the taggers that do
/// write `TKEY` disagree about notation before they disagree about the key — so
/// what is useful is the list, read by someone who knows the songs.
///
/// `KEY_CORPUS` is read relative to the package root, since that is the working
/// directory cargo gives a test binary — not the repo root the command is typed
/// from.
///
/// ```text
/// KEY_CORPUS=src/audio_measure/local-audio/known cargo test --release \
///   --manifest-path src-tauri/Cargo.toml \
///   -- --ignored --nocapture survey_a_library
/// ```
#[cfg(test)]
mod corpus {
    use super::*;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::*;
    use lofty::probe::Probe;
    use std::path::{Path, PathBuf};

    /// Extensions this app decodes, so a survey is not skewed by the cover art
    /// sitting beside the audio.
    const AUDIO: &[&str] = &["mp3", "flac", "ogg", "m4a", "wav", "aac", "oga", "opus"];

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
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
                    .unwrap_or(false)
                {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// What a tagger claimed, in whatever notation it chose.
    fn tagged_key(path: &Path) -> Option<String> {
        let file = Probe::open(path).ok()?.read().ok()?;
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        let value = tag.get_string(ItemKey::InitialKey)?.trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    fn measure(path: &Path) -> Option<Key> {
        use rodio::{Decoder, Source};
        use std::io::Cursor;
        use std::sync::Arc;

        let bytes: Arc<[u8]> = Arc::from(std::fs::read(path).ok()?.into_boxed_slice());
        let decoder = Decoder::new(Cursor::new(bytes)).ok()?;
        let mut collector = Collector::new(decoder.sample_rate(), decoder.channels());
        for sample in decoder {
            collector.push(sample);
        }
        collector.finish()
    }

    #[test]
    #[ignore]
    fn survey_a_library() {
        let Some(root) = std::env::var_os("KEY_CORPUS") else {
            eprintln!("set KEY_CORPUS to a directory of audio");
            return;
        };
        let all = files(Path::new(&root));
        if all.is_empty() {
            eprintln!("no audio under {root:?}");
            return;
        }
        let mut measured = 0;
        for path in &all {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            match measure(path) {
                Some(key) => {
                    measured += 1;
                    eprintln!(
                        "{:<44} {:<4} conf {:.2}  tag {:?}",
                        name,
                        key.name(),
                        key.confidence,
                        tagged_key(path).unwrap_or_else(|| "-".into()),
                    );
                }
                None => eprintln!("{name:<44} no key"),
            }
        }
        eprintln!("\n{measured} of {} measured", all.len());
    }
}
