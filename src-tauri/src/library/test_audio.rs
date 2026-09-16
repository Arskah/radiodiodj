//! Audio files for library tests, written on the fly so the repo carries no
//! binary fixtures.

use std::path::Path;

const SAMPLE_RATE: u32 = 8_000;

/// Write a mono 16-bit PCM WAV of `secs` seconds whose samples are derived
/// from `seed`, so two seeds give two different recordings.
pub fn write_wav(path: &Path, seed: u32, secs: u32) {
    let samples = SAMPLE_RATE * secs;
    let data_len = samples * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    for _ in 0..samples {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        bytes.extend_from_slice(&((state >> 16) as i16 / 4).to_le_bytes());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// Tag `path` in place with `title` and `artist`, as an external tagger would,
/// and move its mtime forward so a scan sees the change.
pub fn retag_externally(path: &Path, title: &str, artist: &str) {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{ItemKey, Tag};

    let mut tagged = Probe::open(path).unwrap().read().unwrap();
    let mut tag = Tag::new(tagged.primary_tag_type());
    tag.insert_text(ItemKey::TrackTitle, title.into());
    tag.insert_text(ItemKey::TrackArtist, artist.into());
    tagged.insert_tag(tag);
    tagged.save_to_path(path, WriteOptions::default()).unwrap();
    let later =
        std::fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}
