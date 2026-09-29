/// The extensions a scan accepts, which is exactly what a deck can play.
///
/// A file the app cannot decode has no business in a library: it would be
/// listed, take a row, fail the analysis pass and then fail on air. So this list
/// and the decoders in the build are two halves of one thing, and both halves
/// live in `Cargo.toml`.
///
/// **There are two copies of symphonia, and they answer different questions.**
/// The direct dependency is 0.6 and serves the demuxing this crate does — tags
/// and fingerprints. `rodio` brings its own 0.5, and that is the one that
/// *decodes*, so a format is only playable once it is enabled on rodio's copy
/// too. Adding `symphonia/aiff` alone makes a file fingerprint and go no
/// further; `rodio/symphonia-aiff` is what lets a deck play it. Both are set.
///
/// Not here, and why: `opus` demuxes (the Ogg Opus mapper exists) but neither
/// copy ships an Opus decoder at any feature setting. `mka` and `webm` would
/// need `mkv`, and `WebM` audio is overwhelmingly Opus, so most of it still
/// could not be decoded.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "aiff", "aif", "ogg", "oga", "aac", "m4a", "mp2",
];

pub fn is_audio_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTENSIONS.iter().any(|e| *e == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list is what a deck can play, so a format the app cannot decode is
    /// not merely unlisted — it must not be scanned either, or it takes a row
    /// and fails on air.
    #[test]
    fn a_format_that_cannot_be_decoded_is_not_audio() {
        for ext in ["opus", "webm", "mka", "wma", "mid", "txt"] {
            assert!(!is_audio_extension(ext), "{ext} is accepted");
        }
    }

    #[test]
    fn an_extension_is_matched_whatever_its_case() {
        for ext in ["MP3", "Flac", "aIfF", "MP2"] {
            assert!(is_audio_extension(ext), "{ext} is not accepted");
        }
    }
}
