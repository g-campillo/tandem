//! The font each pane is drawn in: the saved font preferences resolved against the
//! families this Mac has. Kept free of windows and views so it is tested directly.
use gpui_kit::{FontFeatures, SharedString};
use std::sync::Arc;
use tandem_core::appearance::{
    FONT_SIZE, FontPrefs, MONO_FALLBACK, SYSTEM_FONT, font_label, ligature_features, resolve_family,
};

/// A font as GPUI takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub family: SharedString,
    /// The size in pixels.
    pub size: f32,
    pub features: FontFeatures,
}

impl Face {
    /// The height of a line of code or terminal output in this font: one and a half times
    /// its size, as a whole number of pixels.
    pub fn line_height(&self) -> f32 {
        (self.size * 1.5).round()
    }
}

/// The panes whose font the user chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontPane {
    Editor,
    Terminal,
    ChatText,
    ChatCode,
    Explorer,
}

impl FontPane {
    /// The pane as Settings' notes name it.
    pub fn name(self) -> &'static str {
        match self {
            FontPane::Editor => "the editor",
            FontPane::Terminal => "the terminal",
            FontPane::ChatText => "chat text",
            FontPane::ChatCode => "chat code",
            FontPane::Explorer => "the file explorer",
        }
    }
}

/// A saved family that is not installed, and the family drawn in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitution {
    pub pane: FontPane,
    pub missing: String,
    pub used: String,
}

impl Substitution {
    /// What Settings says about it.
    pub fn note(&self) -> String {
        format!(
            "{} is not installed, so {} uses {}.",
            font_label(&self.missing),
            self.pane.name(),
            font_label(&self.used)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneFonts {
    pub editor: Face,
    pub terminal: Face,
    /// Messages and cards in the chat.
    pub chat: Face,
    /// Code blocks, inline code and the step card's code in the chat: one pixel smaller
    /// than the chat's text.
    pub chat_code: Face,
    pub explorer: Face,
    pub substitutions: Vec<Substitution>,
}

/// The OpenType features of a code font with ligatures on or off.
pub fn features(ligatures: bool) -> FontFeatures {
    FontFeatures(Arc::new(
        ligature_features(ligatures)
            .into_iter()
            .map(|(tag, value)| (tag.to_owned(), value))
            .collect(),
    ))
}

/// The fonts to draw with for `prefs` on a Mac that has the families `installed`. Code
/// fonts that are not installed fall back to Menlo, text fonts to the system font.
pub fn pane_fonts(prefs: &FontPrefs, installed: &[String]) -> PaneFonts {
    let mut substitutions = Vec::new();
    let mut family = |pane: FontPane, saved: &str, fallback: &str| -> SharedString {
        let resolved = resolve_family(saved, installed, fallback);
        if let Some(missing) = resolved.missing {
            substitutions.push(Substitution {
                pane,
                missing,
                used: resolved.family.clone(),
            });
        }
        resolved.family.into()
    };
    let editor = Face {
        family: family(FontPane::Editor, &prefs.editor.family, MONO_FALLBACK),
        size: prefs.editor.size,
        features: features(prefs.editor.ligatures),
    };
    let terminal = Face {
        family: family(FontPane::Terminal, &prefs.terminal.family, MONO_FALLBACK),
        size: prefs.terminal.size,
        features: features(prefs.terminal.ligatures),
    };
    let chat = Face {
        family: family(FontPane::ChatText, &prefs.chat.family, SYSTEM_FONT),
        size: prefs.chat.size,
        features: FontFeatures::default(),
    };
    let chat_code = Face {
        family: family(FontPane::ChatCode, &prefs.chat.code_family, MONO_FALLBACK),
        size: (prefs.chat.size - 1.).max(FONT_SIZE.0),
        features: features(prefs.chat.code_ligatures),
    };
    let explorer = Face {
        family: family(FontPane::Explorer, &prefs.explorer.family, SYSTEM_FONT),
        size: prefs.explorer.size,
        features: FontFeatures::default(),
    };
    PaneFonts {
        editor,
        terminal,
        chat,
        chat_code,
        explorer,
        substitutions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    const ALL: &[&str] = &["FiraCode Nerd Font", "FiraCode Nerd Font Mono", "Menlo"];

    #[test]
    fn the_default_fonts_are_used_as_saved_when_they_are_installed() {
        let fonts = pane_fonts(&FontPrefs::default(), &installed(ALL));
        assert_eq!(fonts.editor.family.as_ref(), "FiraCode Nerd Font");
        assert_eq!(fonts.editor.size, 14.);
        assert_eq!(fonts.terminal.family.as_ref(), "FiraCode Nerd Font Mono");
        assert_eq!(fonts.terminal.size, 12.);
        assert_eq!(fonts.chat.family.as_ref(), SYSTEM_FONT);
        assert_eq!(fonts.chat.size, 13.);
        assert_eq!(fonts.chat_code.family.as_ref(), "FiraCode Nerd Font");
        assert_eq!(fonts.chat_code.size, 12.);
        assert_eq!(fonts.explorer.family.as_ref(), SYSTEM_FONT);
        assert_eq!(fonts.explorer.size, 13.);
        assert!(fonts.substitutions.is_empty());
    }

    #[test]
    fn missing_code_fonts_fall_back_to_menlo_and_are_reported() {
        let fonts = pane_fonts(&FontPrefs::default(), &installed(&["Menlo"]));
        assert_eq!(fonts.editor.family.as_ref(), "Menlo");
        assert_eq!(fonts.terminal.family.as_ref(), "Menlo");
        assert_eq!(fonts.chat_code.family.as_ref(), "Menlo");
        let panes: Vec<FontPane> = fonts.substitutions.iter().map(|s| s.pane).collect();
        assert_eq!(
            panes,
            [FontPane::Editor, FontPane::Terminal, FontPane::ChatCode]
        );
        assert_eq!(
            fonts.substitutions[0].note(),
            "FiraCode Nerd Font is not installed, so the editor uses Menlo."
        );
    }

    #[test]
    fn missing_text_fonts_fall_back_to_the_system_font() {
        let mut prefs = FontPrefs::default();
        prefs.chat.family = "Gone Sans".into();
        prefs.explorer.family = "Gone Sans".into();
        let fonts = pane_fonts(&prefs, &installed(ALL));
        assert_eq!(fonts.chat.family.as_ref(), SYSTEM_FONT);
        assert_eq!(fonts.explorer.family.as_ref(), SYSTEM_FONT);
        assert_eq!(
            fonts.substitutions[1].note(),
            "Gone Sans is not installed, so the file explorer uses System Font."
        );
    }

    #[test]
    fn ligatures_are_features_of_the_code_fonts_only() {
        let mut prefs = FontPrefs::default();
        prefs.terminal.ligatures = false;
        let fonts = pane_fonts(&prefs, &installed(ALL));
        assert_eq!(fonts.editor.features, features(true));
        assert_eq!(fonts.terminal.features, features(false));
        assert_eq!(fonts.chat_code.features, features(true));
        assert_eq!(fonts.chat.features, FontFeatures::default());
        assert_eq!(fonts.explorer.features, FontFeatures::default());
        assert_eq!(
            features(false).tag_value_list(),
            [("calt".to_owned(), 0), ("liga".to_owned(), 0)]
        );
        assert_eq!(features(true).is_calt_enabled(), Some(true));
    }

    #[test]
    fn chat_code_is_one_pixel_smaller_but_never_below_the_smallest_size() {
        let mut prefs = FontPrefs::default();
        prefs.chat.size = 20.;
        assert_eq!(pane_fonts(&prefs, &installed(ALL)).chat_code.size, 19.);
        prefs.chat.size = 10.;
        assert_eq!(pane_fonts(&prefs, &installed(ALL)).chat_code.size, 10.);
    }

    #[test]
    fn line_height_is_one_and_a_half_times_the_size() {
        let face = |size| Face {
            family: "Menlo".into(),
            size,
            features: FontFeatures::default(),
        };
        assert_eq!(face(12.).line_height(), 18.);
        assert_eq!(face(13.).line_height(), 20.);
        assert_eq!(face(14.).line_height(), 21.);
    }
}
