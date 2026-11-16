//! Pure decisions behind the file tree and chat panes: how wide each is drawn. Kept free of
//! GPUI so they are tested directly.
use crate::appearance::WorkspaceGeometry;
use tandem_core::layout::{Pane, PaneLayout};

/// The narrowest and widest the user can make the file tree, in pixels.
pub const FILES_WIDTH: (f32, f32) = (160., 600.);
/// The narrowest and widest the user can make the chat, in pixels.
pub const CHAT_WIDTH: (f32, f32) = (280., 900.);
/// The width the panes always leave for the editor.
pub const EDITOR_MIN: f32 = 360.;

/// The (narrowest, widest) width of `pane`.
pub fn width_limits(pane: Pane) -> (f32, f32) {
    match pane {
        Pane::Files => FILES_WIDTH,
        Pane::Chat => CHAT_WIDTH,
    }
}

/// Which panes are on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownPanes {
    pub files: bool,
    pub chat: bool,
}

/// The widths to draw in a window `window_width` wide. A pane the user never resized keeps
/// the window's default width; a resized pane keeps its width within its limits. When the
/// panes would leave the editor less than `EDITOR_MIN`, the chat gives up width first, then
/// the file tree, neither below its narrowest. The saved widths are not changed, so the
/// panes grow back when the window does. A hidden pane takes no width.
pub fn pane_geometry(
    window_width: f32,
    layout: &PaneLayout,
    shown: ShownPanes,
) -> WorkspaceGeometry {
    let window_width = if window_width.is_finite() {
        window_width.max(0.)
    } else {
        0.
    };
    let defaults = WorkspaceGeometry::for_width(window_width, shown.chat);
    let width = |pane: Pane, visible: bool, default: f32| {
        if !visible {
            return 0.;
        }
        let (narrowest, widest) = width_limits(pane);
        layout
            .width(pane)
            .filter(|width| width.is_finite())
            .map_or(default, |width| width.clamp(narrowest, widest))
    };
    let mut files = width(Pane::Files, shown.files, defaults.sidebar);
    let mut chat = width(Pane::Chat, shown.chat, defaults.utility);
    let mut excess = files + chat - (window_width - EDITOR_MIN);
    if excess > 0. && shown.chat {
        let given = excess.min((chat - CHAT_WIDTH.0).max(0.));
        chat -= given;
        excess -= given;
    }
    if excess > 0. && shown.files {
        files -= excess.min((files - FILES_WIDTH.0).max(0.));
    }
    WorkspaceGeometry {
        sidebar: files,
        utility: chat,
        content: (window_width - files - chat).max(0.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: ShownPanes = ShownPanes {
        files: true,
        chat: true,
    };
    const FILES_ONLY: ShownPanes = ShownPanes {
        files: true,
        chat: false,
    };
    const CHAT_ONLY: ShownPanes = ShownPanes {
        files: false,
        chat: true,
    };

    fn layout(files_width: Option<f32>, chat_width: Option<f32>) -> PaneLayout {
        PaneLayout {
            swapped: false,
            files_width,
            chat_width,
        }
    }

    fn widths(geometry: WorkspaceGeometry) -> (f32, f32, f32) {
        (geometry.sidebar, geometry.utility, geometry.content)
    }

    #[test]
    fn pane_geometry_of_the_default_layout_is_the_windows_default() {
        for width in [900., 1090., 1280., 1600.] {
            for shown in [BOTH, FILES_ONLY] {
                assert_eq!(
                    widths(pane_geometry(width, &PaneLayout::default(), shown)),
                    widths(WorkspaceGeometry::for_width(width, shown.chat)),
                    "width {width}, {shown:?}"
                );
            }
        }
    }

    #[test]
    fn pane_geometry_uses_the_saved_widths() {
        assert_eq!(
            widths(pane_geometry(1600., &layout(Some(300.), Some(500.)), BOTH)),
            (300., 500., 800.)
        );
        assert_eq!(
            widths(pane_geometry(1280., &layout(None, Some(500.)), BOTH)),
            (250., 500., 530.)
        );
    }

    #[test]
    fn pane_geometry_is_the_same_on_either_side() {
        let swapped = PaneLayout {
            swapped: true,
            ..layout(Some(300.), Some(500.))
        };
        assert_eq!(
            widths(pane_geometry(1600., &swapped, BOTH)),
            (300., 500., 800.)
        );
    }

    #[test]
    fn pane_geometry_keeps_saved_widths_within_their_limits() {
        assert_eq!(
            widths(pane_geometry(3000., &layout(Some(50.), Some(100.)), BOTH)),
            (160., 280., 2560.)
        );
        assert_eq!(
            widths(pane_geometry(
                3000.,
                &layout(Some(5000.), Some(5000.)),
                BOTH
            )),
            (600., 900., 1500.)
        );
    }

    #[test]
    fn pane_geometry_takes_width_from_the_chat_then_the_file_tree_to_keep_the_editor() {
        let wide = layout(Some(600.), Some(900.));
        let cases = [
            (1280., (600., 320., 360.)),
            (1000., (360., 280., 360.)),
            (900., (260., 280., 360.)),
        ];
        for (width, expected) in cases {
            assert_eq!(
                widths(pane_geometry(width, &wide, BOTH)),
                expected,
                "width {width}"
            );
        }
    }

    #[test]
    fn pane_geometry_gives_a_hidden_pane_no_width() {
        let saved = layout(Some(300.), Some(500.));
        let neither = ShownPanes {
            files: false,
            chat: false,
        };
        assert_eq!(
            widths(pane_geometry(1280., &saved, FILES_ONLY)),
            (300., 0., 980.)
        );
        assert_eq!(
            widths(pane_geometry(1280., &saved, CHAT_ONLY)),
            (0., 500., 780.)
        );
        assert_eq!(
            widths(pane_geometry(1280., &saved, neither)),
            (0., 0., 1280.)
        );
        assert_eq!(
            widths(pane_geometry(
                900.,
                &layout(Some(600.), Some(900.)),
                FILES_ONLY
            )),
            (540., 0., 360.)
        );
    }

    #[test]
    fn pane_geometry_ignores_a_saved_width_that_is_not_a_number() {
        for width in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                widths(pane_geometry(
                    1280.,
                    &layout(Some(width), Some(width)),
                    BOTH
                )),
                (250., 390., 640.),
                "saved width {width}"
            );
        }
    }

    #[test]
    fn pane_geometry_never_goes_negative_in_a_window_too_small_for_the_panes() {
        for window_width in [f32::NAN, f32::INFINITY, -10., 0., 400., 799.] {
            for saved in [PaneLayout::default(), layout(Some(600.), Some(900.))] {
                let geometry = pane_geometry(window_width, &saved, BOTH);
                let case = format!("window {window_width}, {saved:?}");
                assert_eq!(geometry.sidebar, 160., "{case}");
                assert_eq!(geometry.utility, 280., "{case}");
                assert!(
                    geometry.content.is_finite() && geometry.content >= 0.,
                    "{case}"
                );
                assert!(geometry.terminal_available_width() >= 0., "{case}");
            }
        }
    }
}
