//! A track's identity independent of where its file lives and what its tags
//! say.
//!
//! The fingerprint hashes the codec parameters plus the first
//! [`HEAD_BYTES`] of demuxed packet payload. Demuxers already step over the
//! tag blocks (ID3v2/APE, FLAC metadata, RIFF `LIST`, MP4 `udta`) and
//! reassemble Ogg packets, so a tag edit — even one that re-pages an Ogg
//! stream — leaves it unchanged. Nothing is decoded, so a symphonia upgrade
//! cannot shift it either, and only the head of the file is read, which keeps
//! the cost on a network share to about a megabyte.
//!
//! Not hashed: the frame count and the tail. On an MP3 without a Xing header
//! both are estimated from the file length, which includes the tags. The
//! accepted cost is that two masters sharing their first megabyte of audio
//! collide.
//!
//! Reads are capped at [`READ_LIMIT`]: a file symphonia has no reader for
//! (say an ASF file named `.mp3`) would otherwise be scanned for a sync word
//! to its end.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Packet payload hashed per track.
const HEAD_BYTES: usize = 1 << 20;

/// Bytes read from a source before fingerprinting gives up. Leaves room for
/// embedded cover art ahead of the audio.
const READ_LIMIT: u64 = 16 << 20;

/// Bumped whenever the hashed input changes, so values from two algorithms
/// never compare equal.
const VERSION: &str = "v1";

/// Fingerprint the file at `path`, reading only its head.
pub fn of_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    of_source(Box::new(file), path.extension().and_then(|e| e.to_str()))
}

/// Fingerprint audio already in memory or behind any seekable source.
pub fn of_source(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<String> {
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    let source = Capped {
        inner: source,
        left: READ_LIMIT,
    };
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("probe")?;
    let mut format = probed.format;
    let track = format.default_track().context("no audio track")?;
    let track_id = track.id;
    let params = &track.codec_params;

    let mut hasher = Sha256::new();
    hasher.update(params.codec.to_string().as_bytes());
    hasher.update(params.sample_rate.unwrap_or(0).to_le_bytes());
    hasher.update((params.channels.map_or(0, |c| c.count()) as u32).to_le_bytes());

    let mut hashed = 0usize;
    while hashed < HEAD_BYTES {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(e) => return Err(e).context("read packet"),
        };
        if packet.track_id() != track_id {
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

/// A source that fails once [`READ_LIMIT`] bytes have been read from it.
struct Capped {
    inner: Box<dyn MediaSource>,
    left: u64,
}

impl Read for Capped {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 && !buf.is_empty() {
            return Err(std::io::Error::other("read limit reached"));
        }
        let want = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        self.left -= n as u64;
        Ok(n)
    }
}

impl Seek for Capped {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for Capped {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
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
        assert!(fp.starts_with("v1:"), "{fp}");
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

    #[test]
    fn a_file_that_is_not_audio_has_no_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mp3");
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(of_file(&path).is_err());
    }

    #[test]
    fn a_file_with_no_reader_is_not_read_to_its_end() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        struct Counting(std::io::Cursor<Vec<u8>>, Arc<AtomicU64>);
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buf)?;
                self.1.fetch_add(n as u64, Ordering::Relaxed);
                Ok(n)
            }
        }
        impl Seek for Counting {
            fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
                self.0.seek(pos)
            }
        }
        impl MediaSource for Counting {
            fn is_seekable(&self) -> bool {
                true
            }
            fn byte_len(&self) -> Option<u64> {
                Some(self.0.get_ref().len() as u64)
            }
        }

        let read = Arc::new(AtomicU64::new(0));
        let mut seed = 1u32;
        let junk: Vec<u8> = (0..4 * READ_LIMIT)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let source = Counting(std::io::Cursor::new(junk), read.clone());
        assert!(of_source(Box::new(source), Some("mp3")).is_err());
        assert!(read.load(Ordering::Relaxed) <= READ_LIMIT);
    }
}
