//! Pure decisions behind the file tree and chat panes: how wide each is drawn and what a
//! divider drag does. Kept free of GPUI so they are tested directly.
use crate::appearance::WorkspaceGeometry;
use tandem_core::layout::{Pane, PaneLayout, PaneSide};

/// The narrowest and widest the user can make the file tree, in pixels.
pub const FILES_WIDTH: (f32, f32) = (160., 600.);
/// The narrowest and widest the user can make the chat, in pixels.
pub const CHAT_WIDTH: (f32, f32) = (280., 900.);
/// The width the panes always leave for the editor.
pub const EDITOR_MIN: f32 = 360.;

/// The width of the activity bar: the strip of icons on the window's edge, beside the
/// file tree.
pub const ACTIVITY_BAR: f32 = 44.;

/// The width of the row that holds the panes and the editor, in a window `window_width`
/// wide: what the activity bar leaves.
pub fn row_width(window_width: f32) -> f32 {
    if window_width.is_finite() {
        (window_width - ACTIVITY_BAR).max(0.)
    } else {
        0.
    }
}

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

/// The layout after the divider of `pane` is dragged to `pointer_x`, in a row of panes that
/// spans `row_left..row_right` (window coordinates). The pane's new width is the distance
/// from its outer edge to the pointer, rounded to a whole pixel and kept within the pane's
/// limits and within what leaves the editor `EDITOR_MIN` beside the other pane as drawn.
/// Nothing else in the layout changes.
pub fn drag_pane(
    layout: &PaneLayout,
    pane: Pane,
    pointer_x: f32,
    row_left: f32,
    row_right: f32,
    shown: ShownPanes,
) -> PaneLayout {
    let requested = match layout.side(pane) {
        PaneSide::Left => pointer_x - row_left,
        PaneSide::Right => row_right - pointer_x,
    };
    if !requested.is_finite() {
        return *layout;
    }
    let row_width = row_right - row_left;
    let drawn = pane_geometry(row_width, layout, shown);
    let other = match pane {
        Pane::Files => drawn.utility,
        Pane::Chat => drawn.sidebar,
    };
    let (narrowest, widest) = width_limits(pane);
    let widest = widest.min(row_width - EDITOR_MIN - other).max(narrowest);
    let mut next = *layout;
    next.set_width(pane, Some(requested.clamp(narrowest, widest).round()));
    next
}

/// How many columns a terminal `available` pixels wide has in a font whose characters
/// are `cell` pixels wide: as many as fit, between 40 and 300.
pub fn terminal_columns(available: f32, cell: f32) -> u16 {
    let fit = if available.is_finite() && cell.is_finite() && cell > 0. {
        (available / cell).floor()
    } else {
        0.
    };
    fit.clamp(40., 300.) as u16
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

    #[test]
    fn terminal_width_leaves_an_inset_only_where_no_pane_is_beside_the_editor() {
        let none = ShownPanes {
            files: false,
            chat: false,
        };
        for (shown, inset) in [(BOTH, 0.), (FILES_ONLY, 6.), (CHAT_ONLY, 6.), (none, 12.)] {
            let geometry = pane_geometry(1280., &PaneLayout::default(), shown);
            assert_eq!(
                geometry.terminal_available_width(),
                geometry.content - inset - 18.,
                "{shown:?}"
            );
        }
    }

    #[test]
    fn drag_pane_measures_a_left_pane_from_the_rows_left_edge() {
        let expected = layout(Some(300.), None);
        assert_eq!(
            drag_pane(&PaneLayout::default(), Pane::Files, 300., 0., 1280., BOTH),
            expected
        );
        assert_eq!(
            drag_pane(&PaneLayout::default(), Pane::Files, 310., 10., 1290., BOTH),
            expected
        );
    }

    #[test]
    fn drag_pane_measures_a_right_pane_from_the_rows_right_edge() {
        let next = drag_pane(&PaneLayout::default(), Pane::Chat, 800., 0., 1280., BOTH);
        assert_eq!(next.chat_width, Some(480.));
        assert_eq!(next.files_width, None);
    }

    #[test]
    fn drag_pane_follows_the_panes_side_when_swapped() {
        let swapped = PaneLayout {
            swapped: true,
            ..PaneLayout::default()
        };
        let files = drag_pane(&swapped, Pane::Files, 1000., 0., 1280., BOTH);
        assert_eq!(files.files_width, Some(280.));
        assert!(files.swapped);
        let chat = drag_pane(&swapped, Pane::Chat, 420., 0., 1280., BOTH);
        assert_eq!(chat.chat_width, Some(420.));
        assert!(chat.swapped);
    }

    #[test]
    fn drag_pane_changes_only_the_dragged_panes_width() {
        let saved = PaneLayout {
            swapped: true,
            ..layout(Some(300.), Some(480.))
        };
        assert_eq!(
            drag_pane(&saved, Pane::Chat, 400., 0., 1600., BOTH),
            PaneLayout {
                chat_width: Some(400.),
                ..saved
            }
        );
    }

    #[test]
    fn drag_pane_stops_at_the_narrowest_width() {
        let default = PaneLayout::default();
        assert_eq!(
            drag_pane(&default, Pane::Files, 20., 0., 1280., BOTH).files_width,
            Some(160.)
        );
        assert_eq!(
            drag_pane(&default, Pane::Chat, 1270., 0., 1280., BOTH).chat_width,
            Some(280.)
        );
        assert_eq!(
            drag_pane(&default, Pane::Files, -50., 0., 1280., BOTH).files_width,
            Some(160.)
        );
    }

    #[test]
    fn drag_pane_stops_at_the_widest_width() {
        let default = PaneLayout::default();
        assert_eq!(
            drag_pane(&default, Pane::Files, 1000., 0., 2400., BOTH).files_width,
            Some(600.)
        );
        assert_eq!(
            drag_pane(&default, Pane::Chat, 100., 0., 2400., BOTH).chat_width,
            Some(900.)
        );
    }

    #[test]
    fn drag_pane_stops_where_the_editor_would_get_too_narrow() {
        let default = PaneLayout::default();
        assert_eq!(
            drag_pane(&default, Pane::Files, 700., 0., 1280., BOTH).files_width,
            Some(530.)
        );
        assert_eq!(
            drag_pane(&default, Pane::Files, 700., 0., 1280., FILES_ONLY).files_width,
            Some(600.)
        );
        assert_eq!(
            drag_pane(&layout(Some(400.), None), Pane::Chat, 100., 0., 1280., BOTH).chat_width,
            Some(520.)
        );
    }

    #[test]
    fn drag_pane_keeps_the_narrowest_width_when_there_is_no_room_at_all() {
        assert_eq!(
            drag_pane(&PaneLayout::default(), Pane::Files, 400., 0., 700., BOTH).files_width,
            Some(160.)
        );
    }

    #[test]
    fn drag_pane_rounds_to_a_whole_pixel() {
        let default = PaneLayout::default();
        assert_eq!(
            drag_pane(&default, Pane::Files, 300.4, 0., 1280., BOTH).files_width,
            Some(300.)
        );
        assert_eq!(
            drag_pane(&default, Pane::Files, 300.6, 0., 1280., BOTH).files_width,
            Some(301.)
        );
    }

    #[test]
    fn drag_pane_ignores_a_pointer_that_is_not_a_number() {
        let saved = layout(Some(300.), Some(480.));
        for pointer in [f32::NAN, f32::INFINITY] {
            for pane in [Pane::Files, Pane::Chat] {
                assert_eq!(
                    drag_pane(&saved, pane, pointer, 0., 1280., BOTH),
                    saved,
                    "pointer {pointer}, {pane:?}"
                );
            }
        }
    }

    #[test]
    fn row_width_is_what_the_activity_bar_leaves() {
        assert_eq!(row_width(1280.), 1236.);
        assert_eq!(row_width(900.), 856.);
        assert_eq!(row_width(44.), 0.);
        for width in [0., 20., -5., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(row_width(width), 0., "window {width}");
        }
    }

    #[test]
    fn the_smallest_window_keeps_the_editor_beside_the_activity_bar_and_both_panes() {
        // Tandem's window is at least 900 wide.
        let geometry = pane_geometry(row_width(900.), &PaneLayout::default(), BOTH);
        assert_eq!(widths(geometry), (210., 286., 360.));
        assert_eq!(
            ACTIVITY_BAR + geometry.sidebar + geometry.utility + geometry.content,
            900.
        );
        let wide = pane_geometry(row_width(1324.), &PaneLayout::default(), BOTH);
        assert_eq!(widths(wide), (250., 390., 640.));
    }

    #[test]
    fn terminal_columns_fit_the_width_between_40_and_300() {
        assert_eq!(terminal_columns(720., 7.2), 100);
        assert_eq!(terminal_columns(719., 7.2), 99);
        assert_eq!(terminal_columns(100., 7.2), 40);
        assert_eq!(terminal_columns(10_000., 7.2), 300);
        for (available, cell) in [
            (720., 0.),
            (720., -1.),
            (720., f32::NAN),
            (f32::NAN, 7.2),
            (f32::INFINITY, 7.2),
            (-50., 7.2),
        ] {
            assert_eq!(
                terminal_columns(available, cell),
                40,
                "{available} / {cell}"
            );
        }
    }
}
