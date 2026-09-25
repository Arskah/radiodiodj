//! Where themes come from: the two built-ins, and the operator's `themes/`
//! directory. Resolves the active theme into a complete token map.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::theme::{Base, Theme, ThemeError, THEMEABLE_TOKENS};

pub const MIDNIGHT: &str = "midnight";
pub const DAYLIGHT: &str = "daylight";

/// Built-in ids. A theme directory using one is refused rather than allowed to
/// shadow it.
pub const RESERVED_IDS: &[&str] = &[MIDNIGHT, DAYLIGHT];

const MIDNIGHT_JSON: &str = include_str!("../../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../../themes/daylight.json");

/// The seeded example: a copy of Midnight, named so its purpose is obvious.
/// Written once, only when `themes/` does not exist at all.
const EXAMPLE_ID: &str = "example";

/// One row of the theme picker. A row with `error` set cannot be selected.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ThemeListing {
    pub id: String,
    pub name: String,
    pub author: Option<String>,
    pub base: Option<String>,
    /// `"built-in"`, or the theme's path relative to the data directory.
    pub source: String,
    pub error: Option<String>,
}

/// A built-in, parsed. Panics only if a shipped theme file is broken, which a
/// test catches long before a build ships.
pub fn builtin(id: &str) -> Theme {
    let source = match id {
        DAYLIGHT => DAYLIGHT_JSON,
        _ => MIDNIGHT_JSON,
    };
    Theme::parse(source).expect("built-in theme must parse and validate")
}

pub fn is_builtin(id: &str) -> bool {
    RESERVED_IDS.contains(&id)
}

/// `{app_data_dir}/themes`.
pub fn themes_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("themes")
}

/// Create `themes/` with a copy-me example inside, but only when the directory
/// is absent. An operator who deletes the example does not get it back.
pub fn seed_if_absent(data_dir: &Path) -> std::io::Result<()> {
    let dir = themes_dir(data_dir);
    if dir.exists() {
        return Ok(());
    }
    let example = dir.join(EXAMPLE_ID);
    fs::create_dir_all(&example)?;

    let seeded = MIDNIGHT_JSON.replacen(
        "\"name\": \"Midnight\"",
        "\"name\": \"Example (copy me)\"",
        1,
    );
    fs::write(example.join("theme.json"), seeded)
}

/// Read a theme directory's `theme.json`.
fn load_one(dir: &Path, id: &str) -> Result<Theme, ThemeError> {
    if is_builtin(id) {
        return Err(ThemeError::ReservedName(id.to_string()));
    }
    let source = fs::read_to_string(dir.join("theme.json"))
        .map_err(|e| ThemeError::Unreadable(e.to_string()))?;
    let theme = Theme::parse(&source)?;
    check_asset(dir, "logo", theme.logo.as_deref())?;
    check_asset(dir, "label", theme.label.as_deref())?;
    Ok(theme)
}

const ASSET_EXTENSIONS: &[&str] = &["svg", "png", "jpg", "jpeg", "webp"];

/// An asset must resolve inside its own theme directory. Same root-membership
/// rule as a library path: canonicalize, then `starts_with`.
fn check_asset(dir: &Path, field: &'static str, name: Option<&str>) -> Result<(), ThemeError> {
    let Some(name) = name else { return Ok(()) };
    let fail = |reason: String| ThemeError::Asset { field, reason };

    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !ASSET_EXTENSIONS.contains(&ext.as_str()) {
        return Err(fail(format!("{name:?} is not an image the app can read")));
    }

    let root = dir
        .canonicalize()
        .map_err(|e| fail(format!("theme directory is unreadable: {e}")))?;
    let path = root
        .join(name)
        .canonicalize()
        .map_err(|e| fail(format!("{name:?} could not be read: {e}")))?;
    if !path.starts_with(&root) {
        return Err(fail(format!("{name:?} is outside the theme folder")));
    }
    Ok(())
}

/// Every theme the operator can see: the built-ins, then each directory under
/// `themes/` holding a `theme.json`, valid or not.
pub fn list(data_dir: &Path) -> Vec<ThemeListing> {
    let mut out: Vec<ThemeListing> = RESERVED_IDS
        .iter()
        .map(|id| {
            let theme = builtin(id);
            ThemeListing {
                id: (*id).to_string(),
                name: theme.name,
                author: theme.author,
                base: Some(theme.base.as_str().to_string()),
                source: "built-in".to_string(),
                error: None,
            }
        })
        .collect();

    let dir = themes_dir(data_dir);
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };

    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir() && e.path().join("theme.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));

    for (id, path) in found {
        let source = format!("themes/{id}");
        out.push(match load_one(&path, &id) {
            Ok(theme) => ThemeListing {
                id,
                name: theme.name,
                author: theme.author,
                base: Some(theme.base.as_str().to_string()),
                source,
                error: None,
            },
            Err(err) => ThemeListing {
                name: id.clone(),
                id,
                author: None,
                base: None,
                source,
                error: Some(err.to_string()),
            },
        });
    }
    out
}

/// A theme resolved for painting: every token set, base decided.
#[derive(Debug)]
pub struct Resolved {
    pub id: String,
    pub base: Base,
    pub tokens: BTreeMap<String, String>,
    pub logo: Option<PathBuf>,
    pub label: Option<PathBuf>,
}

/// Resolve `id` into a complete token map, filling whatever it omits from the
/// built-in its `base` names.
pub fn resolve(data_dir: &Path, id: &str) -> Result<Resolved, ThemeError> {
    let (theme, dir) = if is_builtin(id) {
        (builtin(id), None)
    } else {
        let dir = themes_dir(data_dir).join(id);
        (load_one(&dir, id)?, Some(dir))
    };

    let fallback = builtin(match theme.base {
        Base::Light => DAYLIGHT,
        Base::Dark => MIDNIGHT,
    });

    let mut tokens = BTreeMap::new();
    for name in THEMEABLE_TOKENS {
        let value = theme
            .tokens
            .get(*name)
            .or_else(|| fallback.tokens.get(*name))
            .cloned();
        if let Some(value) = value {
            tokens.insert((*name).to_string(), value);
        }
    }

    // A built-in has no directory to resolve a filename against, so an image it
    // named resolves to `None` rather than panicking.
    let image = |file: Option<String>| Some(dir.as_ref()?.join(file?));

    Ok(Resolved {
        id: id.to_string(),
        base: theme.base,
        tokens,
        logo: image(theme.logo),
        label: image(theme.label),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_theme(data_dir: &Path, id: &str, body: &str) -> PathBuf {
        let dir = themes_dir(data_dir).join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("theme.json"), body).unwrap();
        dir
    }

    fn find<'a>(list: &'a [ThemeListing], id: &str) -> &'a ThemeListing {
        list.iter().find(|l| l.id == id).expect("listed")
    }

    /// A built-in is what an incomplete theme falls back to, so it may not have
    /// holes — and it must survive the same validator as anyone else's.
    #[test]
    fn built_ins_parse_validate_and_are_complete() {
        for id in RESERVED_IDS {
            let theme = builtin(id);
            theme.validate().expect("built-in validates");
            assert!(theme.is_complete(), "{id} is missing tokens");
        }
        assert_eq!(builtin(MIDNIGHT).base, Base::Dark);
        assert_eq!(builtin(DAYLIGHT).base, Base::Light);
    }

    #[test]
    fn seeds_an_example_when_the_directory_is_absent() {
        let tmp = tempdir().unwrap();
        seed_if_absent(tmp.path()).unwrap();

        let seeded = themes_dir(tmp.path()).join("example").join("theme.json");
        let theme = Theme::parse(&fs::read_to_string(&seeded).unwrap()).unwrap();
        assert_eq!(theme.name, "Example (copy me)");
        assert!(theme.is_complete(), "the example shows every token");
    }

    /// Deleting the example must not bring it back.
    #[test]
    fn does_not_seed_when_the_directory_already_exists() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(themes_dir(tmp.path())).unwrap();
        seed_if_absent(tmp.path()).unwrap();

        assert!(!themes_dir(tmp.path()).join("example").exists());
    }

    #[test]
    fn lists_built_ins_and_file_themes() {
        let tmp = tempdir().unwrap();
        let body = r##"{"name":"Station Red","author":"Radio Foo","base":"dark",
            "tokens":{"--primary":"#ff5b5b"}}"##;
        write_theme(tmp.path(), "station-red", body);

        let list = list(tmp.path());
        assert_eq!(find(&list, MIDNIGHT).source, "built-in");

        let red = find(&list, "station-red");
        assert_eq!(red.name, "Station Red");
        assert_eq!(red.author.as_deref(), Some("Radio Foo"));
        assert_eq!(red.source, "themes/station-red");
        assert_eq!(red.error, None);
    }

    /// An invalid theme is listed with its reason, never hidden — a theme that
    /// "didn't show up" is the worst failure mode for a drop-in-a-folder feature.
    #[test]
    fn lists_an_invalid_theme_with_its_reason() {
        let tmp = tempdir().unwrap();
        let body = r##"{"name":"Broken","base":"dark","tokens":{"--surfase":"#fff"}}"##;
        write_theme(tmp.path(), "broken-blue", body);

        let listing = list(tmp.path());
        let broken = find(&listing, "broken-blue");
        assert!(broken.error.as_ref().unwrap().contains("--surfase"));
        assert!(resolve(tmp.path(), "broken-blue").is_err());
    }

    #[test]
    fn refuses_a_theme_using_a_built_in_name() {
        let tmp = tempdir().unwrap();
        write_theme(
            tmp.path(),
            MIDNIGHT,
            r#"{"name":"Impostor","base":"dark","tokens":{}}"#,
        );

        // The built-in still holds the id; the directory is listed and refused.
        let listing = list(tmp.path());
        assert_eq!(listing.iter().filter(|l| l.id == MIDNIGHT).count(), 2);
        assert!(listing.iter().any(|l| {
            l.id == MIDNIGHT && l.error.as_deref().is_some_and(|e| e.contains("built-in"))
        }));
    }

    #[test]
    fn a_directory_without_a_theme_file_is_not_a_theme() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(themes_dir(tmp.path()).join("not-a-theme")).unwrap();

        assert!(!list(tmp.path()).iter().any(|l| l.id == "not-a-theme"));
    }

    #[test]
    fn fills_omitted_tokens_from_the_base() {
        let tmp = tempdir().unwrap();
        let body = r##"{"name":"Sparse","base":"light","tokens":{"--primary":"#ff5b5b"}}"##;
        write_theme(tmp.path(), "sparse", body);

        let resolved = resolve(tmp.path(), "sparse").unwrap();
        assert_eq!(resolved.base, Base::Light);
        assert_eq!(resolved.tokens.len(), THEMEABLE_TOKENS.len());
        assert_eq!(resolved.tokens["--primary"], "#ff5b5b");
        assert_eq!(
            resolved.tokens["--background"],
            builtin(DAYLIGHT).tokens["--background"],
            "the rest comes from the base, not from Midnight"
        );
    }

    #[test]
    fn refuses_an_asset_outside_the_theme_directory() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("outside.png"), [0x89, b'P', b'N', b'G']).unwrap();
        write_theme(
            tmp.path(),
            "escapee",
            r#"{"name":"Escapee","base":"dark","tokens":{},"logo":"../../outside.png"}"#,
        );

        let err = resolve(tmp.path(), "escapee").unwrap_err().to_string();
        assert!(err.contains("logo"), "{err}");
    }

    #[test]
    fn refuses_an_asset_that_is_not_an_image() {
        let tmp = tempdir().unwrap();
        let dir = write_theme(
            tmp.path(),
            "scripted",
            r#"{"name":"Scripted","base":"dark","tokens":{},"logo":"payload.html"}"#,
        );
        fs::write(dir.join("payload.html"), "<script>").unwrap();

        let err = resolve(tmp.path(), "scripted").unwrap_err().to_string();
        assert!(err.contains("logo"), "{err}");
    }

    #[test]
    fn resolves_an_asset_that_sits_beside_the_theme() {
        let tmp = tempdir().unwrap();
        let dir = write_theme(
            tmp.path(),
            "with-logo",
            r#"{"name":"With logo","base":"dark","tokens":{},"logo":"logo.svg"}"#,
        );
        fs::write(dir.join("logo.svg"), "<svg/>").unwrap();

        let resolved = resolve(tmp.path(), "with-logo").unwrap();
        assert_eq!(resolved.logo.unwrap().file_name().unwrap(), "logo.svg");
    }

    #[test]
    fn a_missing_theme_is_an_error_rather_than_a_panic() {
        let tmp = tempdir().unwrap();
        assert!(resolve(tmp.path(), "gone").is_err());
    }
}
