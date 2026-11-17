//! How Tandem looks: the window's opacity and blur, and the font of each pane. One
//! preference for every project, saved in `settings` under `appearance`.
use crate::Store;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// The family name GPUI resolves to the system's interface font.
pub const SYSTEM_FONT: &str = ".SystemUIFont";
/// The monospace family used when a saved code font is not installed.
pub const MONO_FALLBACK: &str = "Menlo";
pub const DEFAULT_CODE_FONT: &str = "FiraCode Nerd Font";
pub const DEFAULT_TERMINAL_FONT: &str = "FiraCode Nerd Font Mono";
/// The lowest and highest window opacity, in percent.
pub const OPACITY: (u8, u8) = (30, 100);
/// The highest blur strength; the lowest is 0, which is no blur.
pub const BLUR_MAX: u8 = 100;
/// The window server's blur radius at blur strength `BLUR_MAX`.
pub const BLUR_RADIUS_MAX: i32 = 64;
/// The smallest and largest font size of any pane, in pixels.
pub const FONT_SIZE: (f32, f32) = (10., 30.);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePrefs {
    /// Window opacity in percent, `OPACITY.0..=OPACITY.1`. It fades everything, text included.
    pub opacity: u8,
    /// Blur strength of what is behind the window, `0..=BLUR_MAX`.
    pub blur: u8,
    pub fonts: FontPrefs,
}

impl Default for AppearancePrefs {
    fn default() -> Self {
        Self {
            opacity: 100,
            blur: 50,
            fonts: FontPrefs::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontPrefs {
    pub editor: EditorFont,
    pub terminal: TerminalFont,
    pub chat: ChatFont,
    pub explorer: ExplorerFont,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorFont {
    pub family: String,
    pub size: f32,
    pub ligatures: bool,
}

impl Default for EditorFont {
    fn default() -> Self {
        Self {
            family: DEFAULT_CODE_FONT.into(),
            size: 14.,
            ligatures: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalFont {
    pub family: String,
    pub size: f32,
    pub ligatures: bool,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self {
            family: DEFAULT_TERMINAL_FONT.into(),
            size: 12.,
            ligatures: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatFont {
    /// The font of messages and cards.
    pub family: String,
    /// The font of code blocks, inline code and the step card's code.
    pub code_family: String,
    pub size: f32,
    pub code_ligatures: bool,
}

impl Default for ChatFont {
    fn default() -> Self {
        Self {
            family: SYSTEM_FONT.into(),
            code_family: DEFAULT_CODE_FONT.into(),
            size: 13.,
            code_ligatures: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExplorerFont {
    pub family: String,
    pub size: f32,
}

impl Default for ExplorerFont {
    fn default() -> Self {
        Self {
            family: SYSTEM_FONT.into(),
            size: 13.,
        }
    }
}

/// `size` as a whole number of pixels within `FONT_SIZE`; `default` when it is not a number.
pub fn font_size(size: f32, default: f32) -> f32 {
    if size.is_finite() {
        size.round().clamp(FONT_SIZE.0, FONT_SIZE.1)
    } else {
        default
    }
}

/// `family` trimmed, or `default` when nothing is left.
fn family(family: &str, default: &str) -> String {
    let family = family.trim();
    if family.is_empty() { default } else { family }.to_owned()
}

impl AppearancePrefs {
    /// The same preferences with every value inside its limits: what Tandem applies and saves.
    pub fn normalized(&self) -> Self {
        let defaults = FontPrefs::default();
        let fonts = &self.fonts;
        Self {
            opacity: self.opacity.clamp(OPACITY.0, OPACITY.1),
            blur: self.blur.min(BLUR_MAX),
            fonts: FontPrefs {
                editor: EditorFont {
                    family: family(&fonts.editor.family, &defaults.editor.family),
                    size: font_size(fonts.editor.size, defaults.editor.size),
                    ligatures: fonts.editor.ligatures,
                },
                terminal: TerminalFont {
                    family: family(&fonts.terminal.family, &defaults.terminal.family),
                    size: font_size(fonts.terminal.size, defaults.terminal.size),
                    ligatures: fonts.terminal.ligatures,
                },
                chat: ChatFont {
                    family: family(&fonts.chat.family, &defaults.chat.family),
                    code_family: family(&fonts.chat.code_family, &defaults.chat.code_family),
                    size: font_size(fonts.chat.size, defaults.chat.size),
                    code_ligatures: fonts.chat.code_ligatures,
                },
                explorer: ExplorerFont {
                    family: family(&fonts.explorer.family, &defaults.explorer.family),
                    size: font_size(fonts.explorer.size, defaults.explorer.size),
                },
            },
        }
    }
}

/// The alpha of the window's content for an opacity in percent: 0.3 to 1.0.
pub fn window_alpha(opacity: u8) -> f32 {
    f32::from(opacity.clamp(OPACITY.0, OPACITY.1)) / 100.
}

/// The window server's blur radius for a blur strength: 0 (no blur) to `BLUR_RADIUS_MAX`.
pub fn blur_radius(blur: u8) -> i32 {
    (f32::from(blur.min(BLUR_MAX)) * BLUR_RADIUS_MAX as f32 / f32::from(BLUR_MAX)).round() as i32
}

/// The family a pane is drawn in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFamily {
    pub family: String,
    /// The saved family when it is not installed and `family` stands in for it.
    pub missing: Option<String>,
}

/// The family to draw `saved` in. The system font is always available; any other family
/// must be in `installed`. A family that is not installed is replaced by `fallback`, or by
/// the system font when `fallback` is not installed either.
pub fn resolve_family(saved: &str, installed: &[String], fallback: &str) -> ResolvedFamily {
    let available = |family: &str| family == SYSTEM_FONT || installed.iter().any(|f| f == family);
    if available(saved) {
        return ResolvedFamily {
            family: saved.to_owned(),
            missing: None,
        };
    }
    ResolvedFamily {
        family: if available(fallback) {
            fallback
        } else {
            SYSTEM_FONT
        }
        .to_owned(),
        missing: Some(saved.to_owned()),
    }
}

/// The families a font picker lists: the system font first, then the installed families
/// in alphabetical order. Hidden families (their names start with a dot) are left out.
pub fn font_choices(installed: &[String]) -> Vec<String> {
    let mut families: Vec<String> = installed
        .iter()
        .filter(|family| !family.starts_with('.') && !family.trim().is_empty())
        .cloned()
        .collect();
    families.sort_by_key(|family| family.to_lowercase());
    families.dedup();
    families.insert(0, SYSTEM_FONT.to_owned());
    families
}

/// The name a font picker shows for `family`.
pub fn font_label(family: &str) -> &str {
    if family == SYSTEM_FONT {
        "System Font"
    } else {
        family
    }
}

/// The family a font picker's `label` stands for: the inverse of `font_label`.
pub fn family_of_label(label: &str) -> &str {
    if label == "System Font" {
        SYSTEM_FONT
    } else {
        label
    }
}

/// The OpenType features that turn a code font's ligatures on or off, as VS Code's
/// `editor.fontLigatures` does: contextual alternates and standard ligatures.
pub fn ligature_features(enabled: bool) -> [(&'static str, u32); 2] {
    let value = u32::from(enabled);
    [("calt", value), ("liga", value)]
}

/// The editor font size after ⌘= (`delta` 1) or ⌘− (`delta` -1).
pub fn zoomed(size: f32, delta: f32) -> f32 {
    font_size(size + delta, EditorFont::default().size)
}

impl Store {
    /// The saved appearance, normalized, or the defaults when none was saved.
    pub fn appearance_prefs(&self) -> Result<AppearancePrefs> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='appearance'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let prefs: Option<AppearancePrefs> = value
            .map(|value| {
                serde_json::from_str(&value).context("The saved appearance settings are damaged")
            })
            .transpose()?;
        Ok(prefs.unwrap_or_default().normalized())
    }

    pub fn save_appearance_prefs(&mut self, prefs: &AppearancePrefs) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key,value) VALUES('appearance',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(&prefs.normalized())?],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_defaults_are_an_opaque_blurred_window_in_fira_code() {
        let prefs = AppearancePrefs::default();
        assert_eq!((prefs.opacity, prefs.blur), (100, 50));
        assert_eq!(prefs.fonts.editor.family, "FiraCode Nerd Font");
        assert_eq!(prefs.fonts.editor.size, 14.);
        assert!(prefs.fonts.editor.ligatures);
        assert_eq!(prefs.fonts.terminal.family, "FiraCode Nerd Font Mono");
        assert_eq!(prefs.fonts.terminal.size, 12.);
        assert!(prefs.fonts.terminal.ligatures);
        assert_eq!(prefs.fonts.chat.family, SYSTEM_FONT);
        assert_eq!(prefs.fonts.chat.code_family, "FiraCode Nerd Font");
        assert_eq!(prefs.fonts.chat.size, 13.);
        assert!(prefs.fonts.chat.code_ligatures);
        assert_eq!(prefs.fonts.explorer.family, SYSTEM_FONT);
        assert_eq!(prefs.fonts.explorer.size, 13.);
        assert_eq!(prefs.normalized(), prefs);
    }

    #[test]
    fn missing_fields_take_their_defaults_and_unknown_fields_are_ignored() {
        let prefs: AppearancePrefs = serde_json::from_str(
            r#"{"opacity":80,"later":1,"fonts":{"terminal":{"size":15},"chat":{"code_ligatures":false}}}"#,
        )
        .unwrap();
        assert_eq!(prefs.opacity, 80);
        assert_eq!(prefs.blur, 50);
        assert_eq!(prefs.fonts.terminal.family, "FiraCode Nerd Font Mono");
        assert_eq!(prefs.fonts.terminal.size, 15.);
        assert!(prefs.fonts.terminal.ligatures);
        assert!(!prefs.fonts.chat.code_ligatures);
        assert_eq!(prefs.fonts.chat.code_family, "FiraCode Nerd Font");
        assert_eq!(prefs.fonts.editor, EditorFont::default());
    }

    #[test]
    fn normalized_keeps_every_value_inside_its_limits() {
        let mut prefs = AppearancePrefs {
            opacity: 5,
            blur: 250,
            ..AppearancePrefs::default()
        };
        prefs.fonts.editor.size = 2.;
        prefs.fonts.terminal.size = 99.;
        prefs.fonts.chat.size = f32::NAN;
        prefs.fonts.explorer.size = 13.4;
        prefs.fonts.editor.family = "  ".into();
        prefs.fonts.chat.code_family = " JetBrains Mono ".into();
        let normalized = prefs.normalized();
        assert_eq!((normalized.opacity, normalized.blur), (30, 100));
        assert_eq!(normalized.fonts.editor.size, 10.);
        assert_eq!(normalized.fonts.terminal.size, 30.);
        assert_eq!(normalized.fonts.chat.size, 13.);
        assert_eq!(normalized.fonts.explorer.size, 13.);
        assert_eq!(normalized.fonts.editor.family, "FiraCode Nerd Font");
        assert_eq!(normalized.fonts.chat.code_family, "JetBrains Mono");
        assert_eq!(normalized.normalized(), normalized);
    }

    #[test]
    fn window_alpha_runs_from_the_floor_to_opaque() {
        assert_eq!(window_alpha(100), 1.);
        assert_eq!(window_alpha(65), 0.65);
        assert_eq!(window_alpha(30), 0.3);
        assert_eq!(window_alpha(0), 0.3);
        assert_eq!(window_alpha(255), 1.);
    }

    #[test]
    fn blur_radius_is_zero_without_blur_and_grows_to_the_maximum() {
        assert_eq!(blur_radius(0), 0);
        assert_eq!(blur_radius(1), 1);
        assert_eq!(blur_radius(50), 32);
        assert_eq!(blur_radius(100), 64);
        assert_eq!(blur_radius(200), 64);
        for blur in 1..=100u8 {
            assert!(blur_radius(blur) >= blur_radius(blur - 1));
            assert!(blur_radius(blur) > 0, "blur {blur} must blur");
        }
    }

    #[test]
    fn resolve_family_keeps_an_installed_family_and_the_system_font() {
        let fonts = installed(&["FiraCode Nerd Font", "Menlo"]);
        assert_eq!(
            resolve_family("FiraCode Nerd Font", &fonts, MONO_FALLBACK),
            ResolvedFamily {
                family: "FiraCode Nerd Font".into(),
                missing: None
            }
        );
        assert_eq!(
            resolve_family(SYSTEM_FONT, &[], MONO_FALLBACK),
            ResolvedFamily {
                family: SYSTEM_FONT.into(),
                missing: None
            }
        );
    }

    #[test]
    fn resolve_family_falls_back_and_names_the_missing_family() {
        let fonts = installed(&["Menlo", "Helvetica"]);
        assert_eq!(
            resolve_family("FiraCode Nerd Font", &fonts, MONO_FALLBACK),
            ResolvedFamily {
                family: "Menlo".into(),
                missing: Some("FiraCode Nerd Font".into())
            }
        );
        assert_eq!(
            resolve_family("Gone Sans", &fonts, SYSTEM_FONT),
            ResolvedFamily {
                family: SYSTEM_FONT.into(),
                missing: Some("Gone Sans".into())
            }
        );
        // Family names are matched exactly.
        assert_eq!(
            resolve_family("menlo", &fonts, MONO_FALLBACK)
                .missing
                .as_deref(),
            Some("menlo")
        );
    }

    #[test]
    fn resolve_family_uses_the_system_font_when_the_fallback_is_missing_too() {
        assert_eq!(
            resolve_family(
                "FiraCode Nerd Font",
                &installed(&["Helvetica"]),
                MONO_FALLBACK
            ),
            ResolvedFamily {
                family: SYSTEM_FONT.into(),
                missing: Some("FiraCode Nerd Font".into())
            }
        );
    }

    #[test]
    fn font_choices_start_with_the_system_font_and_are_sorted_without_hidden_families() {
        let fonts = installed(&["menlo", ".SF NS", "Zapfino", "Arial", "Arial", " "]);
        assert_eq!(
            font_choices(&fonts),
            [SYSTEM_FONT, "Arial", "menlo", "Zapfino"]
        );
        assert_eq!(font_choices(&[]), [SYSTEM_FONT]);
        assert_eq!(font_label(SYSTEM_FONT), "System Font");
        assert_eq!(font_label("Menlo"), "Menlo");
        assert_eq!(family_of_label("System Font"), SYSTEM_FONT);
        assert_eq!(family_of_label("Menlo"), "Menlo");
    }

    #[test]
    fn ligature_features_switch_calt_and_liga_together() {
        assert_eq!(ligature_features(true), [("calt", 1), ("liga", 1)]);
        assert_eq!(ligature_features(false), [("calt", 0), ("liga", 0)]);
    }

    #[test]
    fn zoomed_moves_one_pixel_and_stops_at_the_limits() {
        assert_eq!(zoomed(14., 1.), 15.);
        assert_eq!(zoomed(14., -1.), 13.);
        assert_eq!(zoomed(30., 1.), 30.);
        assert_eq!(zoomed(10., -1.), 10.);
    }
}
