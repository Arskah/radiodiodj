/// The extensions a scan accepts, which is exactly what a deck can play.
///
/// Enabling a format takes a feature on *both* copies of symphonia in the build,
/// and `opus`, `webm` and `mka` are left out on purpose. See
/// `docs/library.md#unsupported-formats`.
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
