//! Theming: the operator's colour scheme, resolved and handed to the renderer.
//!
//! See `docs/theming.md`. Three rules hold here: a theme only paints, a bad
//! theme is reported rather than half-applied, and nothing repaints unasked.

pub mod store;
pub mod theme;

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use serde::Serialize;

pub use store::{ThemeListing, MIDNIGHT};

/// What the renderer paints: a complete token map, plus whatever identity the
/// station has. Assembled fresh on every read, and by every mutator.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub theme_id: String,
    pub base: String,
    pub tokens: BTreeMap<String, String>,
    pub station_name: Option<String>,
    pub logo: Option<String>,
    pub label: Option<String>,
    /// Set when the active theme could not be used, so the UI can say why
    /// without a second command.
    pub problem: Option<String>,
}

/// Resolve the configured theme, falling back to Midnight if it cannot be used.
///
/// A failure is never fatal: the app must start even with a broken theme file,
/// and the operator is told through `problem` rather than an empty window.
pub fn resolve(data_dir: &Path, theme_id: &str, station_name: Option<String>) -> Appearance {
    let (resolved, problem) = match store::resolve(data_dir, theme_id) {
        Ok(resolved) => (resolved, None),
        Err(err) => {
            log::warn!("theme {theme_id:?} could not be loaded: {err}; falling back to Midnight");
            let problem = format!("{theme_id} could not be loaded: {err}");
            (
                store::resolve(data_dir, MIDNIGHT).expect("Midnight must always resolve"),
                Some(problem),
            )
        }
    };

    Appearance {
        theme_id: resolved.id,
        base: resolved.base.as_str().to_string(),
        tokens: resolved.tokens,
        station_name,
        logo: resolved.logo.as_deref().and_then(read_image),
        label: resolved.label.as_deref().and_then(read_image),
        problem,
    }
}

/// The largest image a theme or the station may ship.
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

/// Read an image as a `data:` URL.
///
/// Images cross the boundary as data URLs because `asset://` is not enabled —
/// the same route `get_cover_art` already takes. A theme asset is never inlined
/// into the DOM, so an SVG here is inert.
pub fn read_image(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_IMAGE_BYTES {
        log::warn!("image {} is larger than 2 MiB; ignored", path.display());
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = mime_of(path, &bytes)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{mime};base64,{encoded}"))
}

/// Type by magic bytes, never by extension alone — except for SVG, which is
/// text and has none.
fn mime_of(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    is_svg.then_some("image/svg+xml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::theme::THEMEABLE_TOKENS;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_a_built_in_into_a_complete_map() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(tmp.path(), store::DAYLIGHT, None);

        assert_eq!(appearance.theme_id, store::DAYLIGHT);
        assert_eq!(appearance.base, "light");
        assert_eq!(appearance.tokens.len(), THEMEABLE_TOKENS.len());
        assert_eq!(appearance.problem, None);
    }

    /// A theme that cannot be used must never stop the app from starting: it
    /// falls back to Midnight and reports why.
    #[test]
    fn falls_back_to_midnight_and_says_why() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(tmp.path(), "gone", None);

        assert_eq!(appearance.theme_id, MIDNIGHT);
        assert_eq!(appearance.base, "dark");
        assert!(appearance.problem.unwrap().contains("gone"));
    }

    #[test]
    fn carries_the_station_name_through_untouched() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(tmp.path(), MIDNIGHT, Some("Radio Foo".into()));
        assert_eq!(appearance.station_name.as_deref(), Some("Radio Foo"));
    }

    #[test]
    fn encodes_a_theme_image_as_a_data_url() {
        let tmp = tempdir().unwrap();
        let png = tmp.path().join("logo.png");
        fs::write(&png, [0x89, b'P', b'N', b'G', 0x0D]).unwrap();

        let url = read_image(&png).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }

    #[test]
    fn refuses_an_image_over_the_cap() {
        let tmp = tempdir().unwrap();
        let big = tmp.path().join("big.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G'];
        bytes.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        fs::write(&big, bytes).unwrap();

        assert_eq!(read_image(&big), None);
    }

    /// Type comes from the bytes, not the extension — except SVG, which is text.
    #[test]
    fn types_an_image_by_its_bytes() {
        let tmp = tempdir().unwrap();
        let liar = tmp.path().join("liar.png");
        fs::write(&liar, b"not a png at all").unwrap();
        assert_eq!(read_image(&liar), None);

        let svg = tmp.path().join("mark.svg");
        fs::write(&svg, "<svg/>").unwrap();
        assert!(read_image(&svg).unwrap().starts_with("data:image/svg+xml;"));
    }
}
