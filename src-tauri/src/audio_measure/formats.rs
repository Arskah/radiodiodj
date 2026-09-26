pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "aac", "m4a", "opus", "webm", "aiff", "aif", "mka", "mp2",
];

pub fn is_audio_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTENSIONS.iter().any(|e| *e == lower)
}
