//! The theme model: what a theme file may say, and what makes one valid.
//!
//! A theme sets colours and nothing else. Validation runs here, once, when a
//! theme is loaded — the renderer never sees an unvalidated token.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Every token a theme may set. The same names are the `:root` block of
/// `src/styles.css` and the key set of each built-in theme; `tools/themeContract.test.ts`
/// and [`super::builtin`]'s tests keep the three in step.
pub const THEMEABLE_TOKENS: &[&str] = &[
    "--surface",
    "--surface-dim",
    "--surface-bright",
    "--surface-container-lowest",
    "--surface-container-low",
    "--surface-container",
    "--surface-container-high",
    "--surface-container-highest",
    "--surface-variant",
    "--background",
    "--on-surface",
    "--on-surface-variant",
    "--outline",
    "--outline-variant",
    "--primary",
    "--on-primary",
    "--primary-container",
    "--on-primary-container",
    "--inverse-primary",
    "--secondary",
    "--on-secondary",
    "--secondary-container",
    "--on-secondary-container",
    "--tertiary",
    "--tertiary-container",
    "--error",
    "--on-error",
    "--error-container",
    "--on-error-container",
    "--signal-green",
    "--led-highlight",
    "--shadow-color",
    "--inner-highlight",
    "--scrim",
    "--on-warning-container",
    "--on-toggle-knob",
    "--on-marker",
    "--cue-in-color",
    "--fade-in-color",
    "--fade-out-color",
    "--cue-out-color",
    "--next-start-color",
];

/// Whether a theme paints on light or dark. Names the built-in an incomplete
/// theme falls back to, and the `color-scheme` the OS draws native chrome with.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    Light,
    Dark,
}

impl Base {
    pub fn as_str(self) -> &'static str {
        match self {
            Base::Light => "light",
            Base::Dark => "dark",
        }
    }
}

/// A theme file, as written. Unknown top-level keys are ignored, which is what
/// lets a theme carry `notes` for whoever edits it.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    pub base: Base,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Why a theme was refused. Carried into the theme list so the operator can fix
/// it, rather than being logged and forgotten.
#[derive(Debug, PartialEq, Eq)]
pub enum ThemeError {
    Unreadable(String),
    Malformed(String),
    UnknownToken(String),
    BadValue { token: String, value: String },
    ReservedName(String),
    Asset { field: &'static str, reason: String },
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeError::Unreadable(e) => write!(f, "could not be read: {e}"),
            ThemeError::Malformed(e) => write!(f, "is not valid JSON: {e}"),
            ThemeError::UnknownToken(t) => write!(f, "\"{t}\" is not a theme token"),
            ThemeError::BadValue { token, value } => write!(
                f,
                "\"{token}\" is not a colour: {value:?}. Use #rgb, #rrggbb, #rrggbbaa, \
                 rgb(), rgba(), hsl(), hsla() or transparent"
            ),
            ThemeError::ReservedName(n) => write!(f, "\"{n}\" is a built-in theme name"),
            ThemeError::Asset { field, reason } => write!(f, "{field}: {reason}"),
        }
    }
}

impl Theme {
    /// Parse and validate one theme file's text.
    pub fn parse(source: &str) -> Result<Theme, ThemeError> {
        let theme: Theme =
            serde_json::from_str(source).map_err(|e| ThemeError::Malformed(e.to_string()))?;
        theme.validate()?;
        Ok(theme)
    }

    /// Two rules: every token name is in the contract, and every value parses as
    /// a colour. Both are refusals — a theme is never partly applied.
    pub fn validate(&self) -> Result<(), ThemeError> {
        for (token, value) in &self.tokens {
            if !THEMEABLE_TOKENS.contains(&token.as_str()) {
                return Err(ThemeError::UnknownToken(token.clone()));
            }
            if !is_colour(value) {
                return Err(ThemeError::BadValue {
                    token: token.clone(),
                    value: value.clone(),
                });
            }
        }
        Ok(())
    }

    /// Whether the theme sets every token in the contract. What the built-ins
    /// and the seeded example are held to; an operator's theme is free to set
    /// one token and inherit the rest.
    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        THEMEABLE_TOKENS
            .iter()
            .all(|t| self.tokens.contains_key(*t))
    }
}

/// The whole accepted colour grammar.
///
/// Deliberately closed: it cannot express `;`, `}`, a second declaration, a
/// `var()`, or a fetch, so the injection surface shuts here rather than in an
/// escaping function downstream. `oklch()` and named colours are excluded — see
/// `docs/theming.md`.
pub fn is_colour(value: &str) -> bool {
    let v = value.trim();
    if v == "transparent" {
        return true;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    for func in ["rgb", "rgba", "hsl", "hsla"] {
        if let Some(rest) = v.strip_prefix(func) {
            if let Some(args) = rest.strip_prefix('(').and_then(|a| a.strip_suffix(')')) {
                return are_numeric_args(args);
            }
        }
    }
    false
}

/// Arguments to a colour function: numbers, optionally with `%`, separated by
/// commas, spaces or a `/` alpha delimiter. Nothing else may appear.
fn are_numeric_args(args: &str) -> bool {
    let parts: Vec<&str> = args
        .split([',', ' ', '/'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    if !(3..=4).contains(&parts.len()) {
        return false;
    }
    parts.iter().all(|p| {
        let n = p.strip_suffix('%').unwrap_or(p);
        !n.is_empty() && n.parse::<f64>().is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_with(token: &str, value: &str) -> String {
        format!(r#"{{"name":"T","base":"dark","tokens":{{"{token}":"{value}"}}}}"#)
    }

    #[test]
    fn accepts_every_form_in_the_grammar() {
        for value in [
            "#f33",
            "#f33a",
            "#ff5b5b",
            "#ff5b5b80",
            "rgb(255 91 91)",
            "rgba(255, 91, 91, 0.4)",
            "hsl(0 100% 68%)",
            "hsla(0, 100%, 68%, 0.5)",
            "transparent",
            "  #ff5b5b  ",
        ] {
            assert!(is_colour(value), "{value:?} should be a colour");
        }
    }

    #[test]
    fn refuses_everything_else() {
        for value in [
            "chocolate",
            "var(--primary)",
            "color-mix(in srgb, red, blue)",
            "oklch(0.7 0.2 20)",
            "url(http://example.com/x.png)",
            "#ff",
            "#ff555",
            "#ff5b5b5b5b",
            "#gggggg",
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4, 5)",
            "rgb(red, green, blue)",
            "",
        ] {
            assert!(!is_colour(value), "{value:?} should not be a colour");
        }
    }

    /// The grammar is the injection boundary: a value that closes the
    /// declaration and opens another must not parse.
    #[test]
    fn refuses_a_smuggled_second_declaration() {
        for value in [
            "#fff; position: fixed",
            "#fff} body {display:none",
            "red; background: url(http://example.com/beacon)",
        ] {
            assert!(!is_colour(value), "{value:?} must not parse");
        }
    }

    #[test]
    fn refuses_an_unknown_token_name() {
        let err = Theme::parse(&theme_with("--surfase", "#fff")).unwrap_err();
        assert_eq!(err, ThemeError::UnknownToken("--surfase".to_string()));
        assert!(err.to_string().contains("--surfase"));
    }

    #[test]
    fn refuses_a_bad_value_and_names_the_token() {
        let err = Theme::parse(&theme_with("--primary", "chartreuse")).unwrap_err();
        assert_eq!(
            err,
            ThemeError::BadValue {
                token: "--primary".to_string(),
                value: "chartreuse".to_string(),
            }
        );
        assert!(err.to_string().contains("--primary"));
    }

    #[test]
    fn a_partial_theme_is_valid() {
        let theme = Theme::parse(&theme_with("--primary", "#ff5b5b")).unwrap();
        assert_eq!(theme.tokens.len(), 1);
        assert!(!theme.is_complete());
    }

    /// Unknown top-level keys are ignored, which is what lets the seeded example
    /// carry `notes` for whoever edits it.
    #[test]
    fn ignores_unknown_top_level_keys() {
        let theme =
            Theme::parse(r#"{"name":"T","base":"light","notes":["hi"],"whatever":1,"tokens":{}}"#)
                .unwrap();
        assert_eq!(theme.base, Base::Light);
    }

    #[test]
    fn refuses_malformed_json() {
        assert!(matches!(
            Theme::parse("{not json").unwrap_err(),
            ThemeError::Malformed(_)
        ));
    }

    #[test]
    fn base_is_required() {
        assert!(Theme::parse(r#"{"name":"T","tokens":{}}"#).is_err());
    }
}
