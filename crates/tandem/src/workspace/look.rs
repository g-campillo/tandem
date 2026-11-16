//! How the window looks: the saved appearance, and applying its opacity and blur to the
//! native window.
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
}

impl LookUi {
    /// The saved appearance, or the default one when none was saved or it cannot be read.
    pub fn load(store: &Store) -> Self {
        let prefs = store.appearance_prefs().unwrap_or_default();
        Self {
            saved: prefs.clone(),
            prefs,
            blur: BlurBackend::WindowServer,
            background: None,
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
    use super::{BlurBackend, blur_label, opacity_label, window_background};
    use gpui_kit::WindowBackgroundAppearance;

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
