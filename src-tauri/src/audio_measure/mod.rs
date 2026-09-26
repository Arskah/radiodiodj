//! What a decode says about a track: the waveform curve, the integrated
//! loudness, the level envelope behind the automatic cue points, the tempo,
//! the musical key and the content fingerprint.
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

pub mod bpm;
pub mod fingerprint;
pub mod formats;
pub mod key;
pub mod level_envelope;
pub mod loudness;
#[cfg(test)]
pub(crate) mod test_audio;
pub mod waveform;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    /// The file the guard lives in, and so the one file allowed to name what it
    /// bans.
    const GUARD: &str = "mod.rs";

    /// What the module may not depend on anywhere, and why. Stated as a test
    /// rather than as a comment because a comment is only read by someone who
    /// already suspects the answer.
    const BANNED: &[(&str, &str)] = &[
        ("tauri::", "no events, no `AppHandle`, no command handlers"),
        ("crate::library", "it does not know a database exists"),
        (
            "crate::persist",
            "a setting arrives as an argument or not at all",
        ),
        ("crate::playlist", "it does not know what a station is"),
        ("crate::broadcast", "it does not know what a station is"),
        (
            "crate::audio::",
            "playback depends on measurement, never the reverse",
        ),
        ("rodio::Sink", "it never plays anything"),
        ("cpal", "it never opens a device"),
    ];

    /// What the shipped half may not do, checked only above a file's
    /// `#[cfg(test)]`. Choosing concurrency is the caller's business, and the
    /// pass has already chosen; a corpus test fanning out over a library of
    /// files is measuring, not deciding.
    const BANNED_IN_SHIPPED: &[(&str, &str)] = &[
        ("std::thread", "the caller owns the concurrency"),
        ("std::sync::mpsc", "the caller owns the concurrency"),
    ];

    /// Every dependency this module is meant to be free of, checked against the
    /// source rather than left to a reviewer to notice.
    ///
    /// Doc comments are exempt: the link to [`crate::library::waveform_scan`]
    /// above names a caller, which is not a dependency.
    #[test]
    fn nothing_here_depends_on_the_app() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/audio_measure");
        let mut checked = 0;
        for entry in fs::read_dir(&dir).expect("audio_measure is a directory") {
            let path = entry.expect("readable entry").path();
            if path.extension().is_none_or(|e| e != "rs")
                || path.file_name().is_some_and(|n| n == GUARD)
            {
                continue;
            }
            let source = fs::read_to_string(&path).expect("readable source");
            let shipped = source
                .lines()
                .position(|l| l == "#[cfg(test)]")
                .unwrap_or(usize::MAX);
            for (line_no, line) in source.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let also: &[(&str, &str)] = if line_no < shipped {
                    BANNED_IN_SHIPPED
                } else {
                    &[]
                };
                for (needle, why) in BANNED.iter().chain(also) {
                    assert!(
                        !line.contains(needle),
                        "{}:{} names `{needle}` — {why}. See docs/audio-measure.md.",
                        path.display(),
                        line_no + 1,
                    );
                }
            }
            checked += 1;
        }
        // A guard that silently found nothing to read would pass forever.
        assert!(checked > 0, "no sources found under {}", dir.display());
    }
}
