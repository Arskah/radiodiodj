//! What a decode says about a track: the waveform curve, the integrated
//! loudness, the level envelope behind the automatic cue points, and the tempo.
//!
//! A library that happens to live in this binary. Nothing here opens a device,
//! reads a setting, touches the database or emits an event — a caller hands in
//! bytes and a threshold and receives numbers. That is what lets the same code
//! serve the background pass, a test and (one day) another program, and it is
//! why the pass itself lives in [`crate::library::waveform_scan`]: the pass
//! decides *when and for which rows*, this module answers *what the audio is*.
//!
//! [`waveform::analyze`] is the one decode. Every measurement rides that single
//! walk over the samples rather than opening the file again, because the decode
//! dominates the cost of any one of them. See `docs/audio-measure.md` for the
//! boundary and what may not cross it.

pub mod auto_cue;
pub mod bpm;
pub mod fingerprint;
pub mod formats;
pub mod loudness;
#[cfg(test)]
pub(crate) mod test_audio;
pub mod waveform;
