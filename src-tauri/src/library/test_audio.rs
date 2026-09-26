//! Tag writes for library tests, applied to audio
//! [written on the fly](crate::audio_measure::test_audio::write_wav) so the repo
//! carries no binary fixtures.

use std::path::Path;

/// Write one tag of the caller's choosing, for a test about how a value is read
/// rather than about a retag. Leaves the modification time alone.
///
/// ID3v2 explicitly, not the file's own primary tag type: a WAV's is RIFF INFO,
/// which has no field for a BPM at all, so a test written against it would assert
/// nothing.
pub fn write_tag(path: &Path, key: lofty::tag::ItemKey, value: &str, title: &str) {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::prelude::ItemKey;
    use lofty::probe::Probe;
    use lofty::tag::{Tag, TagType};

    let mut tagged = Probe::open(path).unwrap().read().unwrap();
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::TrackTitle, title.into());
    // Not `insert_text`: for a key whose frame lofty types as an integer, that
    // call refuses the item and says so only in its return value.
    assert!(
        tag.insert(lofty::tag::TagItem::new(
            key,
            lofty::tag::ItemValue::Text(value.into())
        )),
        "lofty refused the {key:?} tag"
    );
    tagged.insert_tag(tag);
    tagged.save_to_path(path, WriteOptions::default()).unwrap();
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
