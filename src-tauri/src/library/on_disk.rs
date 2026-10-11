//! Finding a library file under the name this machine's filesystem knows it by.
//!
//! A path in a shared library was spelled by the machine that scanned it, and
//! two machines can spell one name differently: macOS hands back `ä` as `a`
//! and a combining diaeresis, where Windows and Linux ask a share for exactly
//! the code points they are given. So a name that is not found as stored is
//! asked for again composed, then decomposed, before it counts as missing.
//!
//! The same goes for a character Windows keeps out of a name, `:` among them.
//! macOS allows it, and its SMB client writes it to the share as a private-use
//! code point, which is the name every other machine has to ask for.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use unicode_normalization::{is_nfc, is_nfd, UnicodeNormalization};

/// What the macOS SMB client writes to a share in place of a character Windows
/// keeps out of a file name, or `None` for one it sends as it is. `last` is
/// whether the character ends its name, where a space and a full stop are kept
/// out as well.
fn on_share(c: char, last: bool) -> Option<char> {
    let mapped = match c {
        '\u{1}'..='\u{1f}' => 0xf000 + u32::from(c),
        '"' => 0xf020,
        '*' => 0xf021,
        ':' => 0xf022,
        '<' => 0xf023,
        '>' => 0xf024,
        '?' => 0xf025,
        '|' => 0xf027,
        ' ' if last => 0xf028,
        '.' if last => 0xf029,
        _ => return None,
    };
    char::from_u32(mapped)
}

/// `path` with every name in it spelled as [`on_share`] has it, or `None` when
/// that is how it is spelled already. Only names are respelled: the `?` and `:`
/// of a Windows prefix are not part of one.
fn share_spelling(path: &Path) -> Option<PathBuf> {
    let mut changed = false;
    let mut spelled = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            spelled.push(component);
            continue;
        };
        let name = name.to_str()?;
        let end = name.chars().count() - 1;
        let mapped: String = name
            .chars()
            .enumerate()
            .map(|(i, c)| on_share(c, i == end).unwrap_or(c))
            .collect();
        changed |= mapped != name;
        spelled.push(mapped);
    }
    changed.then_some(spelled)
}

/// The other spellings of `path` worth asking for once the stored one has
/// failed: composed first, then decomposed, then each of the three as a Mac
/// wrote it to a share. Empty for a path that is plain ASCII with nothing
/// Windows forbids, or not Unicode at all.
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
    let on_share: Vec<_> = std::iter::once(path)
        .chain(others.iter().map(PathBuf::as_path))
        .filter_map(share_spelling)
        .collect();
    others.extend(on_share);
    others
}

/// Open a library file for reading, under whichever spelling of its name
/// exists. Only a name that is not found, or that this platform refuses to ask
/// for at all, is asked for again, and the error reported is the stored
/// spelling's.
pub fn open(path: &Path) -> io::Result<File> {
    let not_found = match File::open(path) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidFilename
            ) =>
        {
            e
        }
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
    fn a_name_windows_forbids_is_respelled_as_a_mac_wrote_it() {
        assert_eq!(
            share_spelling(Path::new("Khid/Se tuli : Sushi/05 Osta mut?.ogg")),
            Some(PathBuf::from(
                "Khid/Se tuli \u{f022} Sushi/05 Osta mut\u{f025}.ogg"
            ))
        );
        assert_eq!(
            share_spelling(Path::new("R.E.M./Vol. /a.b.ogg")),
            Some(PathBuf::from("R.E.M\u{f029}/Vol.\u{f028}/a.b.ogg"))
        );
        assert_eq!(share_spelling(Path::new("/mnt/radio/a.ogg")), None);
    }

    #[test]
    fn a_respelled_name_is_tried_under_every_normal_form() {
        assert_eq!(
            respellings(Path::new("a\u{308} : b")),
            [
                PathBuf::from("\u{e4} : b"),
                PathBuf::from("a\u{308} \u{f022} b"),
                PathBuf::from("\u{e4} \u{f022} b"),
            ]
        );
    }

    #[test]
    fn a_file_a_mac_named_with_a_forbidden_character_is_opened() {
        let dir = tempfile::tempdir().unwrap();
        let album = dir.path().join("Se tuli \u{f022} Sushi");
        std::fs::create_dir(&album).unwrap();
        std::fs::write(album.join("05 Osta mut.ogg"), b"audio").unwrap();
        let stored = dir.path().join("Se tuli : Sushi").join("05 Osta mut.ogg");

        let mut read = String::new();
        open(&stored).unwrap().read_to_string(&mut read).unwrap();
        assert_eq!(read, "audio");
        assert!(find(&stored).exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_prefix_is_not_a_name() {
        assert_eq!(
            share_spelling(Path::new(r"\\?\UNC\nas\Public\a : b\c.ogg")),
            Some(PathBuf::from(
                "\\\\?\\UNC\\nas\\Public\\a \u{f022} b\\c.ogg"
            ))
        );
        assert_eq!(share_spelling(Path::new(r"C:\Music\a.ogg")), None);
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
