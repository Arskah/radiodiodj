//! Finding a library file under the name this machine's filesystem knows it by.
//!
//! A path in a shared library was spelled by the machine that scanned it, and
//! two machines can spell one name differently: macOS hands back `ä` as `a`
//! and a combining diaeresis, where Windows and Linux ask a share for exactly
//! the code points they are given. So a name that is not found as stored is
//! asked for again composed, then decomposed, before it counts as missing.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use unicode_normalization::{is_nfc, is_nfd, UnicodeNormalization};

/// The other spellings of `path` worth asking for once the stored one has
/// failed: composed first, then decomposed. Empty for a path that is ASCII or
/// not Unicode at all.
fn respellings(path: &Path) -> Vec<PathBuf> {
    let Some(stored) = path.to_str() else {
        return Vec::new();
    };
    let mut others = Vec::new();
    if !is_nfc(stored) {
        others.push(PathBuf::from(stored.nfc().collect::<String>()));
    }
    if !is_nfd(stored) {
        others.push(PathBuf::from(stored.nfd().collect::<String>()));
    }
    others
}

/// Open a library file for reading, under whichever spelling of its name
/// exists. Only a name that is not found is asked for again, and the error
/// reported is the stored spelling's.
pub fn open(path: &Path) -> io::Result<File> {
    let not_found = match File::open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => e,
        other => return other,
    };
    respellings(path)
        .iter()
        .find_map(|other| File::open(other).ok())
        .ok_or(not_found)
}

/// The spelling of `path` that exists here, or `path` itself when none does,
/// for a reader that takes a path rather than an open file. Costs a `stat`
/// where [`open`] costs nothing, so a read that can use that one should.
pub fn find(path: &Path) -> Cow<'_, Path> {
    if path.exists() {
        return Cow::Borrowed(path);
    }
    respellings(path)
        .into_iter()
        .find(|other| other.exists())
        .map_or(Cow::Borrowed(path), Cow::Owned)
}

/// `path` as the platform's file manager accepts it. A library path's folder is
/// canonical, which on Windows is the verbatim form (`\\?\C:\…`,
/// `\\?\UNC\server\share\…`) that the filesystem takes and Explorer refuses.
pub fn for_shell(path: &Path) -> PathBuf {
    let Some(verbatim) = path.to_str().and_then(|p| p.strip_prefix(r"\\?\")) else {
        return path.to_path_buf();
    };
    match verbatim.strip_prefix(r"UNC\") {
        Some(share) => PathBuf::from(format!(r"\\{share}")),
        None => PathBuf::from(verbatim),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const COMPOSED: &str = "Sa\u{e4}st\u{f6}.ogg";
    const DECOMPOSED: &str = "Sa\u{61}\u{308}sto\u{308}.ogg";

    #[test]
    fn a_name_is_respelled_composed_then_decomposed() {
        assert_eq!(
            respellings(Path::new(DECOMPOSED)),
            [PathBuf::from(COMPOSED)]
        );
        assert_eq!(
            respellings(Path::new(COMPOSED)),
            [PathBuf::from(DECOMPOSED)]
        );
        assert!(respellings(Path::new("plain.ogg")).is_empty());
    }

    #[test]
    fn a_file_scanned_under_another_spelling_is_opened() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(COMPOSED), b"audio").unwrap();

        let mut read = String::new();
        open(&dir.path().join(DECOMPOSED))
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "audio");
    }

    #[test]
    fn a_file_scanned_under_another_spelling_is_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DECOMPOSED), b"audio").unwrap();

        let found = find(&dir.path().join(COMPOSED)).into_owned();
        assert!(found.exists());
    }

    #[test]
    fn a_file_that_is_nowhere_reports_the_stored_name() {
        let dir = tempfile::tempdir().unwrap();
        let stored = dir.path().join(DECOMPOSED);

        assert_eq!(open(&stored).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(find(&stored), stored);
    }

    #[test]
    fn a_verbatim_path_is_handed_to_the_shell_plain() {
        assert_eq!(
            for_shell(Path::new(r"\\?\UNC\nas\Public\Music\a.ogg")),
            PathBuf::from(r"\\nas\Public\Music\a.ogg")
        );
        assert_eq!(
            for_shell(Path::new(r"\\?\C:\Music\a.ogg")),
            PathBuf::from(r"C:\Music\a.ogg")
        );
        assert_eq!(
            for_shell(Path::new("/mnt/radio/a.ogg")),
            PathBuf::from("/mnt/radio/a.ogg")
        );
    }
}
