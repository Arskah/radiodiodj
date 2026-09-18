//! Theming: the operator's colour scheme, resolved and handed to the renderer.
//!
//! See `docs/theming.md`. Three rules hold here: a theme only paints, a bad
//! theme is reported rather than half-applied, and nothing repaints unasked.

pub mod store;
pub mod theme;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

use crate::persist::config::AppearanceConfig;

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

/// Resolve the configured theme and the station's identity into what the
/// renderer paints.
///
/// A theme failure is never fatal: the app must start even with a broken theme
/// file, and the operator is told through `problem` rather than an empty window.
///
/// The station's own images win over whatever the theme ships, so switching
/// theme never costs the operator their logo.
pub fn resolve(data_dir: &Path, config: &AppearanceConfig) -> Appearance {
    let theme_id = &config.theme_id;
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

    let branding = branding_dir(data_dir);
    let identity = |name: &Option<String>| name.as_ref().map(|n| branding.join(n));

    Appearance {
        theme_id: resolved.id,
        base: resolved.base.as_str().to_string(),
        tokens: resolved.tokens,
        station_name: config.station_name.clone(),
        logo: image_for(identity(&config.logo), resolved.logo),
        label: image_for(identity(&config.label), resolved.label),
        problem,
    }
}

/// The station's own image, else the theme's. A slot the operator set but whose
/// file has gone missing falls through to the theme rather than showing nothing.
fn image_for(identity: Option<PathBuf>, theme: Option<PathBuf>) -> Option<String> {
    identity
        .as_deref()
        .and_then(read_image)
        .or_else(|| theme.as_deref().and_then(read_image))
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

/// `{app_data_dir}/branding` — where the station's own images are copied to, so
/// they are present at every launch regardless of where the original moved.
pub fn branding_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("branding")
}

/// Which station image a command is acting on.
#[derive(serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ImageSlot {
    Logo,
    Label,
}

/// Copy an operator-chosen image into `{app_data_dir}/branding`, and return the
/// file name to store in config.
///
/// The app owns the copy: a logo is a few kilobytes and must be present at every
/// launch, unlike a library, so depending on a path the operator may later move
/// would be the more fragile choice.
pub fn adopt_image(data_dir: &Path, slot: ImageSlot, source: &Path) -> Result<String, String> {
    let size = std::fs::metadata(source)
        .map_err(|e| format!("could not read the image: {e}"))?
        .len();
    if size > MAX_IMAGE_BYTES {
        return Err("images must be 2 MiB or smaller".to_string());
    }

    let bytes = std::fs::read(source).map_err(|e| format!("could not read the image: {e}"))?;
    let mime = mime_of(source, &bytes)
        .ok_or_else(|| "that file is not a PNG, JPEG, WebP or SVG".to_string())?;

    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "svg",
    };
    let name = match slot {
        ImageSlot::Logo => format!("logo.{extension}"),
        ImageSlot::Label => format!("label.{extension}"),
    };

    let dir = branding_dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    // Replacing a slot leaves the previous file behind if the type changed, so
    // clear the other extensions this slot could have used.
    for old in ["png", "jpg", "webp", "svg"] {
        let stale = dir.join(match slot {
            ImageSlot::Logo => format!("logo.{old}"),
            ImageSlot::Label => format!("label.{old}"),
        });
        if stale.file_name() != Path::new(&name).file_name() {
            let _ = std::fs::remove_file(stale);
        }
    }

    std::fs::write(dir.join(&name), &bytes)
        .map_err(|e| format!("could not save the image: {e}"))?;
    Ok(name)
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

    fn config(theme_id: &str) -> AppearanceConfig {
        AppearanceConfig {
            theme_id: theme_id.to_string(),
            ..AppearanceConfig::default()
        }
    }

    fn png(path: &std::path::Path) {
        fs::write(path, [0x89, b'P', b'N', b'G', 0x0D]).unwrap();
    }

    #[test]
    fn resolves_a_built_in_into_a_complete_map() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(tmp.path(), &config(store::DAYLIGHT));

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
        let appearance = resolve(tmp.path(), &config("gone"));

        assert_eq!(appearance.theme_id, MIDNIGHT);
        assert_eq!(appearance.base, "dark");
        assert!(appearance.problem.unwrap().contains("gone"));
    }

    #[test]
    fn carries_the_station_name_through_untouched() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(
            tmp.path(),
            &AppearanceConfig {
                station_name: Some("Radio Foo".into()),
                ..config(MIDNIGHT)
            },
        );
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

    /// The operator's own image wins over whatever the theme ships, so trying a
    /// different theme never costs them their logo.
    #[test]
    fn station_identity_wins_over_a_themes_image() {
        let tmp = tempdir().unwrap();

        // A theme that ships its own logo.
        let theme_dir = store::themes_dir(tmp.path()).join("branded");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(
            theme_dir.join("theme.json"),
            r#"{"name":"Branded","base":"dark","tokens":{},"logo":"logo.svg"}"#,
        )
        .unwrap();
        fs::write(theme_dir.join("logo.svg"), "<svg/>").unwrap();

        let themed = resolve(tmp.path(), &config("branded"));
        assert!(themed.logo.unwrap().starts_with("data:image/svg+xml;"));

        // The station's own logo, which must win.
        fs::create_dir_all(branding_dir(tmp.path())).unwrap();
        png(&branding_dir(tmp.path()).join("logo.png"));

        let owned = resolve(
            tmp.path(),
            &AppearanceConfig {
                logo: Some("logo.png".into()),
                ..config("branded")
            },
        );
        assert!(owned.logo.unwrap().starts_with("data:image/png;"));
    }

    /// A slot pointing at a file that has gone falls through to the theme rather
    /// than showing nothing.
    #[test]
    fn a_missing_station_image_falls_through() {
        let tmp = tempdir().unwrap();
        let appearance = resolve(
            tmp.path(),
            &AppearanceConfig {
                logo: Some("vanished.png".into()),
                ..config(MIDNIGHT)
            },
        );
        assert_eq!(appearance.logo, None);
    }

    #[test]
    fn adopts_an_image_into_the_branding_directory() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("from-desktop.png");
        png(&source);

        let name = adopt_image(tmp.path(), ImageSlot::Logo, &source).unwrap();
        assert_eq!(name, "logo.png");
        assert!(branding_dir(tmp.path()).join("logo.png").is_file());
    }

    /// Replacing a slot with a different file type must not leave the old one
    /// behind for the next resolve to find.
    #[test]
    fn replacing_a_slot_clears_the_previous_file() {
        let tmp = tempdir().unwrap();
        let png_source = tmp.path().join("a.png");
        png(&png_source);
        adopt_image(tmp.path(), ImageSlot::Logo, &png_source).unwrap();

        let svg_source = tmp.path().join("b.svg");
        fs::write(&svg_source, "<svg/>").unwrap();
        let name = adopt_image(tmp.path(), ImageSlot::Logo, &svg_source).unwrap();

        assert_eq!(name, "logo.svg");
        assert!(!branding_dir(tmp.path()).join("logo.png").exists());
    }

    #[test]
    fn the_two_slots_do_not_collide() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("art.png");
        png(&source);

        assert_eq!(
            adopt_image(tmp.path(), ImageSlot::Logo, &source).unwrap(),
            "logo.png"
        );
        assert_eq!(
            adopt_image(tmp.path(), ImageSlot::Label, &source).unwrap(),
            "label.png"
        );
    }

    #[test]
    fn refuses_to_adopt_a_file_that_is_not_an_image() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("payload.png");
        fs::write(&source, b"<script>alert(1)</script>").unwrap();

        let err = adopt_image(tmp.path(), ImageSlot::Logo, &source).unwrap_err();
        assert!(err.contains("not a PNG"), "{err}");
    }

    #[test]
    fn refuses_to_adopt_an_oversized_image() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("huge.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G'];
        bytes.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        fs::write(&source, bytes).unwrap();

        let err = adopt_image(tmp.path(), ImageSlot::Logo, &source).unwrap_err();
        assert!(err.contains("2 MiB"), "{err}");
    }
}
