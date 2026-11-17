//! How the window looks: the saved appearance, and applying its opacity and blur to the
//! native window.
use super::fonts::{self, FontPane, pane_fonts};
use super::*;
use crate::native_window;
use tandem_core::appearance::{AppearancePrefs, BLUR_MAX, OPACITY, blur_radius, window_alpha};

/// How the blur behind the window is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlurBackend {
    /// The window server's blur, at the radius the user chose.
    WindowServer,
    /// GPUI's built-in blur, which has one strength: this system has no call for the radius.
    BuiltIn,
}

pub(super) struct LookUi {
    /// The appearance on screen.
    pub prefs: AppearancePrefs,
    /// The appearance the store holds, so only a change is written.
    saved: AppearancePrefs,
    pub blur: BlurBackend,
    /// The window background last asked of GPUI, so it is only set when it changes.
    background: Option<WindowBackgroundAppearance>,
    /// The installed font families.
    pub installed: Vec<String>,
    /// The fonts on screen.
    pub fonts: fonts::PaneFonts,
}

impl LookUi {
    /// The saved appearance, or the default one when none was saved or it cannot be read,
    /// with its fonts resolved against the `installed` families.
    pub fn load(store: &Store, installed: Vec<String>) -> Self {
        let prefs = store.appearance_prefs().unwrap_or_default();
        Self {
            fonts: pane_fonts(&prefs.fonts, &installed),
            saved: prefs.clone(),
            prefs,
            blur: BlurBackend::WindowServer,
            background: None,
            installed,
        }
    }
}

/// The background GPUI draws the window with. The window server's blur shows through a
/// plain transparent window; without it, GPUI's own blur stands in whenever blur is on.
pub(super) fn window_background(backend: BlurBackend, blur: u8) -> WindowBackgroundAppearance {
    match backend {
        BlurBackend::BuiltIn if blur > 0 => WindowBackgroundAppearance::Blurred,
        _ => WindowBackgroundAppearance::Transparent,
    }
}

/// A change to one pane's font made in Settings.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FontChange {
    Family(FontPane, String),
    /// The chat has one size: `ChatText` and `ChatCode` both set it.
    Size(FontPane, f32),
    /// Only code fonts have ligatures; for `ChatText` and `Explorer` nothing changes.
    Ligatures(FontPane, bool),
}

/// `prefs` after `change`, normalized.
pub(super) fn with_font_change(prefs: &AppearancePrefs, change: FontChange) -> AppearancePrefs {
    let mut changed = prefs.clone();
    let fonts = &mut changed.fonts;
    match change {
        FontChange::Family(pane, family) => match pane {
            FontPane::Editor => fonts.editor.family = family,
            FontPane::Terminal => fonts.terminal.family = family,
            FontPane::ChatText => fonts.chat.family = family,
            FontPane::ChatCode => fonts.chat.code_family = family,
            FontPane::Explorer => fonts.explorer.family = family,
        },
        FontChange::Size(pane, size) => match pane {
            FontPane::Editor => fonts.editor.size = size,
            FontPane::Terminal => fonts.terminal.size = size,
            FontPane::ChatText | FontPane::ChatCode => fonts.chat.size = size,
            FontPane::Explorer => fonts.explorer.size = size,
        },
        FontChange::Ligatures(pane, ligatures) => match pane {
            FontPane::Editor => fonts.editor.ligatures = ligatures,
            FontPane::Terminal => fonts.terminal.ligatures = ligatures,
            FontPane::ChatCode => fonts.chat.code_ligatures = ligatures,
            FontPane::ChatText | FontPane::Explorer => {}
        },
    }
    changed.normalized()
}

/// The saved family of `pane`: the family `FontChange::Family(pane, …)` sets.
pub(super) fn pane_family(prefs: &AppearancePrefs, pane: FontPane) -> &str {
    let fonts = &prefs.fonts;
    match pane {
        FontPane::Editor => &fonts.editor.family,
        FontPane::Terminal => &fonts.terminal.family,
        FontPane::ChatText => &fonts.chat.family,
        FontPane::ChatCode => &fonts.chat.code_family,
        FontPane::Explorer => &fonts.explorer.family,
    }
}

/// The saved size of `pane`: the size `FontChange::Size(pane, …)` sets.
pub(super) fn pane_size(prefs: &AppearancePrefs, pane: FontPane) -> f32 {
    let fonts = &prefs.fonts;
    match pane {
        FontPane::Editor => fonts.editor.size,
        FontPane::Terminal => fonts.terminal.size,
        FontPane::ChatText | FontPane::ChatCode => fonts.chat.size,
        FontPane::Explorer => fonts.explorer.size,
    }
}

/// A font size as Settings shows it: `14 px`.
pub(super) fn font_size_label(size: f32) -> String {
    format!("{size:.0} px")
}

/// The opacity as Settings shows it: `80%`.
pub(super) fn opacity_label(opacity: u8) -> String {
    format!("{opacity}%")
}

/// The blur strength as Settings shows it: `Off` for 0, else the number.
pub(super) fn blur_label(blur: u8) -> String {
    if blur == 0 {
        "Off".to_owned()
    } else {
        blur.to_string()
    }
}

impl Workspace {
    /// Sets the window opacity to the slider's `percent` and applies it. Saving is left to
    /// `persist_look`.
    pub(super) fn set_opacity(
        &mut self,
        percent: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opacity = (percent.round() as u8).clamp(OPACITY.0, OPACITY.1);
        if opacity == self.look.prefs.opacity {
            return;
        }
        self.look.prefs.opacity = opacity;
        self.apply_window_look(window);
        cx.notify();
    }

    /// Sets the blur strength to the slider's `strength` and applies it. Saving is left to
    /// `persist_look`.
    pub(super) fn set_blur(&mut self, strength: f32, window: &mut Window, cx: &mut Context<Self>) {
        let blur = (strength.round() as u8).min(BLUR_MAX);
        if blur == self.look.prefs.blur {
            return;
        }
        self.look.prefs.blur = blur;
        self.apply_window_look(window);
        cx.notify();
    }

    /// Applies the opacity and blur of `self.look.prefs` to the window.
    pub(super) fn apply_window_look(&mut self, window: &mut Window) {
        let (opacity, blur) = (self.look.prefs.opacity, self.look.prefs.blur);
        native_window::set_content_opacity(window, window_alpha(opacity));
        self.look.blur = if native_window::set_blur_radius(window, blur_radius(blur)) {
            BlurBackend::WindowServer
        } else {
            BlurBackend::BuiltIn
        };
        let background = window_background(self.look.blur, blur);
        if self.look.background != Some(background) {
            window.set_background_appearance(background);
            self.look.background = Some(background);
        }
    }

    /// Resolves the saved fonts against the installed families. The panes read
    /// `self.look.fonts` when they are drawn; the toolkit's own code font (hover
    /// signatures, Markdown code, the diff view) follows the editor's.
    pub(super) fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        self.look.fonts = pane_fonts(&self.look.prefs.fonts, &self.look.installed);
        crate::appearance::set_code_font(cx, self.look.fonts.editor.family.clone());
        if let Some(terminal) = self.terminal.clone() {
            let font = self.terminal_font();
            terminal.update(cx, |terminal, cx| terminal.set_font(font, cx));
        }
        cx.notify();
    }

    /// Applies a font change made in Settings or with ⌘= and ⌘−, and moves Settings'
    /// sliders to it. Saving is left to `persist_look`.
    pub(super) fn change_font(
        &mut self,
        change: FontChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefs = with_font_change(&self.look.prefs, change);
        if prefs == self.look.prefs {
            return;
        }
        self.look.prefs = prefs;
        self.apply_fonts(cx);
        self.look_controls.sync(&self.look.prefs, window, cx);
    }

    /// The terminal's font as the terminal view takes it.
    pub(super) fn terminal_font(&self) -> tandem_terminal::TerminalFont {
        let face = &self.look.fonts.terminal;
        tandem_terminal::TerminalFont {
            family: face.family.clone(),
            size: face.size,
            features: face.features.clone(),
        }
    }

    /// Writes the appearance to the store when it differs from what the store holds. A
    /// failure is reported once and not retried until the appearance changes again.
    pub(super) fn persist_look(&mut self, cx: &mut Context<Self>) {
        if self.look.prefs == self.look.saved {
            return;
        }
        self.look.saved = self.look.prefs.clone();
        let result = self
            .store
            .lock()
            .map_err(|_| anyhow!("Recovery store unavailable"))
            .and_then(|mut store| store.save_appearance_prefs(&self.look.prefs));
        if let Err(error) = result {
            self.message(
                format!("Could not save the appearance: {error:#}"),
                true,
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlurBackend, FontChange, blur_label, font_size_label, opacity_label, pane_family,
        pane_size, window_background, with_font_change,
    };
    use crate::workspace::fonts::FontPane;
    use gpui_kit::WindowBackgroundAppearance;
    use tandem_core::appearance::{AppearancePrefs, SYSTEM_FONT};

    #[test]
    fn a_family_change_sets_exactly_the_named_panes_family() {
        let prefs = AppearancePrefs::default();
        let family = |pane| with_font_change(&prefs, FontChange::Family(pane, "Menlo".into()));

        let mut expected = prefs.clone();
        expected.fonts.editor.family = "Menlo".into();
        assert_eq!(family(FontPane::Editor), expected);

        let mut expected = prefs.clone();
        expected.fonts.terminal.family = "Menlo".into();
        assert_eq!(family(FontPane::Terminal), expected);

        let mut expected = prefs.clone();
        expected.fonts.chat.family = "Menlo".into();
        assert_eq!(family(FontPane::ChatText), expected);

        let mut expected = prefs.clone();
        expected.fonts.chat.code_family = "Menlo".into();
        assert_eq!(family(FontPane::ChatCode), expected);

        let mut expected = prefs.clone();
        expected.fonts.explorer.family = "Menlo".into();
        assert_eq!(family(FontPane::Explorer), expected);
    }

    #[test]
    fn a_blank_family_becomes_that_panes_default() {
        let mut prefs = AppearancePrefs::default();
        prefs.fonts.terminal.family = "Menlo".into();
        prefs.fonts.chat.code_family = "Menlo".into();
        let changed = with_font_change(&prefs, FontChange::Family(FontPane::Terminal, " ".into()));
        assert_eq!(changed.fonts.terminal.family, "FiraCode Nerd Font Mono");
        let changed = with_font_change(
            &prefs,
            FontChange::Family(FontPane::ChatCode, String::new()),
        );
        assert_eq!(changed.fonts.chat.code_family, "FiraCode Nerd Font");
        assert_eq!(changed.fonts.terminal.family, "Menlo");
    }

    #[test]
    fn a_size_change_sets_the_panes_size_rounded_and_clamped() {
        let prefs = AppearancePrefs::default();
        let changed = with_font_change(&prefs, FontChange::Size(FontPane::Editor, 99.));
        let mut expected = prefs.clone();
        expected.fonts.editor.size = 30.;
        assert_eq!(changed, expected);

        let changed = with_font_change(&prefs, FontChange::Size(FontPane::Terminal, 13.6));
        let mut expected = prefs.clone();
        expected.fonts.terminal.size = 14.;
        assert_eq!(changed, expected);

        let changed = with_font_change(&prefs, FontChange::Size(FontPane::Explorer, 2.));
        assert_eq!(changed.fonts.explorer.size, 10.);
    }

    #[test]
    fn both_chat_panes_set_the_chats_one_size() {
        let prefs = AppearancePrefs::default();
        let mut expected = prefs.clone();
        expected.fonts.chat.size = 16.;
        assert_eq!(
            with_font_change(&prefs, FontChange::Size(FontPane::ChatCode, 16.)),
            expected
        );
        assert_eq!(
            with_font_change(&prefs, FontChange::Size(FontPane::ChatText, 16.)),
            expected
        );
    }

    #[test]
    fn ligatures_change_only_the_code_fonts() {
        let prefs = AppearancePrefs::default();
        let off = |pane| with_font_change(&prefs, FontChange::Ligatures(pane, false));

        let mut expected = prefs.clone();
        expected.fonts.editor.ligatures = false;
        assert_eq!(off(FontPane::Editor), expected);

        let mut expected = prefs.clone();
        expected.fonts.terminal.ligatures = false;
        assert_eq!(off(FontPane::Terminal), expected);

        let mut expected = prefs.clone();
        expected.fonts.chat.code_ligatures = false;
        assert_eq!(off(FontPane::ChatCode), expected);

        assert_eq!(off(FontPane::ChatText), prefs);
        assert_eq!(off(FontPane::Explorer), prefs);
    }

    #[test]
    fn each_panes_family_and_size_read_back_what_a_change_set() {
        let prefs = AppearancePrefs::default();
        for pane in [
            FontPane::Editor,
            FontPane::Terminal,
            FontPane::ChatText,
            FontPane::ChatCode,
            FontPane::Explorer,
        ] {
            let changed = with_font_change(&prefs, FontChange::Family(pane, "Menlo".into()));
            assert_eq!(pane_family(&changed, pane), "Menlo", "{pane:?}");
            let changed = with_font_change(&prefs, FontChange::Size(pane, 21.));
            assert_eq!(pane_size(&changed, pane), 21., "{pane:?}");
        }
        assert_eq!(pane_family(&prefs, FontPane::ChatText), SYSTEM_FONT);
        assert_eq!(
            pane_family(&prefs, FontPane::ChatCode),
            "FiraCode Nerd Font"
        );
        assert_eq!(pane_size(&prefs, FontPane::ChatCode), 13.);
    }

    #[test]
    fn a_font_size_shows_in_whole_pixels() {
        assert_eq!(font_size_label(14.), "14 px");
        assert_eq!(font_size_label(10.), "10 px");
        assert_eq!(font_size_label(30.), "30 px");
    }

    #[test]
    fn a_change_to_the_current_value_changes_nothing() {
        let mut prefs = AppearancePrefs::default();
        prefs.fonts.explorer.family = "Helvetica".into();
        prefs.fonts.terminal.size = 15.;
        for change in [
            FontChange::Family(FontPane::Explorer, "Helvetica".into()),
            FontChange::Size(FontPane::Terminal, 15.),
            FontChange::Size(FontPane::ChatCode, 13.),
            FontChange::Ligatures(FontPane::Editor, true),
        ] {
            assert_eq!(with_font_change(&prefs, change), prefs);
        }
    }

    #[test]
    fn opacity_shows_as_a_percentage() {
        assert_eq!(opacity_label(30), "30%");
        assert_eq!(opacity_label(100), "100%");
    }

    #[test]
    fn blur_shows_off_at_zero_and_the_strength_otherwise() {
        assert_eq!(blur_label(0), "Off");
        assert_eq!(blur_label(1), "1");
        assert_eq!(blur_label(100), "100");
    }

    #[test]
    fn the_window_servers_blur_always_uses_a_transparent_window() {
        for blur in [0, 1, 50, 100] {
            assert_eq!(
                window_background(BlurBackend::WindowServer, blur),
                WindowBackgroundAppearance::Transparent
            );
        }
    }

    #[test]
    fn the_built_in_blur_is_on_exactly_when_blur_is_on() {
        assert_eq!(
            window_background(BlurBackend::BuiltIn, 0),
            WindowBackgroundAppearance::Transparent
        );
        for blur in [1, 50, 100] {
            assert_eq!(
                window_background(BlurBackend::BuiltIn, blur),
                WindowBackgroundAppearance::Blurred
            );
        }
    }
}
