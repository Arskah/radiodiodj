//! A track's identity independent of where its file lives and what its tags
//! say.
//!
//! The fingerprint hashes the codec parameters plus the first
//! [`HEAD_BYTES`] of demuxed packet payload. Demuxers already step over the
//! tag blocks (ID3v2/APE, FLAC metadata, RIFF `LIST`, MP4 `udta`) and
//! reassemble Ogg packets, so a tag edit — even one that re-pages an Ogg
//! stream — leaves it unchanged. Nothing is decoded, so no decoder change can
//! shift it either. Only the head of the file is read, which keeps the cost on
//! a network share to about a megabyte.
//!
//! The codec goes in as the id symphonia gives it, which a symphonia upgrade
//! may renumber — 0.6 renumbered MP3, AAC and thirteen others. That is what
//! [`VERSION`] is for: bump it, and the background analysis pass re-reads the
//! head of every present track and stores the new value.
//!
//! Not hashed: the frame count and the tail. On an MP3 without a Xing header
//! both are estimated from the file length, which includes the tags. The
//! accepted cost is that two masters sharing their first megabyte of audio
//! collide.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

/// Packet payload hashed per track.
const HEAD_BYTES: usize = 1 << 20;

/// Bumped whenever the hashed input changes, so values from two algorithms
/// never compare equal. A track whose stored fingerprint carries an older
/// version is re-fingerprinted by the background analysis pass.
pub const VERSION: &str = "v2";

/// Fingerprint the file at `path`, reading only its head.
pub fn of_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    of_source(Box::new(file), path.extension().and_then(|e| e.to_str()))
}

/// The failure came from reading the file rather than from what it holds, so
/// a later attempt may succeed. An early end of stream is the file's fault.
pub fn is_read_error(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        let io = match cause.downcast_ref::<SymphoniaError>() {
            Some(SymphoniaError::IoError(io)) => Some(io),
            _ => cause.downcast_ref::<std::io::Error>(),
        };
        io.is_some_and(|io| io.kind() != std::io::ErrorKind::UnexpectedEof)
    })
}

/// Fingerprint audio already in memory or behind any seekable source.
pub fn of_source(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<String> {
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    let stream = MediaSourceStream::new(source, Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("probe")?;
    let track = format
        .default_track(TrackType::Audio)
        .context("no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .context("no audio codec parameters")?;

    let mut hasher = Sha256::new();
    hasher.update(params.codec.to_string().as_bytes());
    hasher.update(params.sample_rate.unwrap_or(0).to_le_bytes());
    hasher.update((params.channels.as_ref().map_or(0, Channels::count) as u32).to_le_bytes());

    let mut hashed = 0usize;
    while hashed < HEAD_BYTES {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(e) => return Err(e).context("read packet"),
        };
        if packet.track_id != track_id {
            continue;
        }
        let take = packet.data.len().min(HEAD_BYTES - hashed);
        hasher.update(&packet.data[..take]);
        hashed += take;
    }
    if hashed == 0 {
        bail!("no audio packets");
    }

    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("{VERSION}:{digest}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_audio::write_wav;
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::*;
    use lofty::probe::Probe;
    use lofty::tag::{ItemKey, Tag, TagType};

    #[test]
    fn the_fingerprint_of_a_known_recording_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 2);
        assert_eq!(
            of_file(&path).unwrap(),
            "v2:2b8824ce42c1a4e43a8cde23d6c31d31f8d0f2163fe7d786cded49a0d6030e7d"
        );
    }

    #[test]
    fn different_audio_gives_different_fingerprints() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        write_wav(&a, 1, 2);
        write_wav(&b, 2, 2);
        assert_ne!(of_file(&a).unwrap(), of_file(&b).unwrap());
    }

    #[test]
    fn the_same_audio_anywhere_gives_the_same_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("sub/b.wav"));
        write_wav(&a, 1, 2);
        write_wav(&b, 1, 2);
        let fp = of_file(&a).unwrap();
        assert!(fp.starts_with("v2:"), "{fp}");
        assert_eq!(fp, of_file(&b).unwrap());
        let bytes = std::fs::read(&a).unwrap();
        let in_memory = of_source(Box::new(std::io::Cursor::new(bytes)), Some("wav")).unwrap();
        assert_eq!(fp, in_memory);
    }

    #[test]
    fn a_tag_edit_keeps_the_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 2);
        let before = of_file(&path).unwrap();
        let len_before = std::fs::metadata(&path).unwrap().len();

        let mut tagged = Probe::open(&path).unwrap().read().unwrap();
        for tag_type in [TagType::RiffInfo, TagType::Id3v2] {
            let mut tag = Tag::new(tag_type);
            tag.insert_text(ItemKey::TrackTitle, "Retitled".into());
            tag.insert_text(ItemKey::TrackArtist, "Someone".into());
            tagged.insert_tag(tag);
        }
        tagged.save_to_path(&path, WriteOptions::default()).unwrap();

        assert_ne!(std::fs::metadata(&path).unwrap().len(), len_before);
        let reread = Probe::open(&path).unwrap().read().unwrap();
        assert_eq!(
            reread
                .primary_tag()
                .unwrap()
                .get_string(ItemKey::TrackTitle),
            Some("Retitled")
        );
        assert_eq!(of_file(&path).unwrap(), before);
    }

    /// `[ID3v2.3 tag holding one APIC frame][payload]`, the layout a tagger
    /// writes when a track carries cover art. Built by hand because lofty puts
    /// a WAV's ID3v2 in a RIFF chunk, and it is the leading tag that the probe
    /// has to step over.
    fn behind_cover_art(payload: &[u8], image_len: usize) -> Vec<u8> {
        let mut frame = vec![0u8]; // latin-1 text encoding
        frame.extend_from_slice(b"image/jpeg\0");
        frame.push(3); // front cover
        frame.push(0); // empty description
        frame.extend(std::iter::repeat_n(0x5A, image_len));

        let mut body = Vec::new();
        body.extend_from_slice(b"APIC");
        body.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        body.extend_from_slice(&[0, 0]); // frame flags
        body.extend_from_slice(&frame);

        let n = body.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"ID3");
        out.extend_from_slice(&[3, 0, 0]); // v2.3, no flags
        for shift in [21, 14, 7, 0] {
            out.push(((n >> shift) & 0x7F) as u8); // synchsafe length
        }
        out.extend_from_slice(&body);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn cover_art_past_the_probe_window_keeps_the_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.wav");
        write_wav(&plain, 1, 2);
        let audio = std::fs::read(&plain).unwrap();
        let bare = of_file(&plain).unwrap();

        // The probe searches about a megabyte for a format marker. A small
        // cover leaves the audio inside that window whether or not the ID3v2
        // reader is registered; one past it is only found because the reader
        // steps over the tag, so this is what fails when `id3v2` is not among
        // symphonia's features.
        for image_len in [4 * 1024, 2 * 1024 * 1024] {
            let path = dir.path().join(format!("cover-{image_len}.wav"));
            std::fs::write(&path, behind_cover_art(&audio, image_len)).unwrap();
            let got = of_file(&path).unwrap_or_else(|e| panic!("{image_len} byte cover: {e:#}"));
            assert_eq!(got, bare, "{image_len} byte cover");
        }
    }

    #[test]
    fn a_file_that_is_not_audio_has_no_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mp3");
        std::fs::write(&path, b"not audio at all").unwrap();
        let err = of_file(&path).unwrap_err();
        assert!(!is_read_error(&err), "{err:#}");
    }

    #[test]
    fn a_file_that_cannot_be_opened_is_a_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = of_file(&dir.path().join("gone.mp3")).unwrap_err();
        assert!(is_read_error(&err), "{err:#}");
    }
}
