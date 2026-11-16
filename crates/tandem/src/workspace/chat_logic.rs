//! Pure decisions behind the workspace's chat and bottom panels, kept free of GPUI so they
//! are tested directly.

/// The tabs of the panel below the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomTab {
    Terminal,
    Problems,
}

/// The bottom panel after a click on `clicked`: clicking the visible tab hides the panel,
/// clicking any other tab (or any tab while the panel is hidden) shows that tab.
pub fn next_bottom(current: Option<BottomTab>, clicked: BottomTab) -> Option<BottomTab> {
    if current == Some(clicked) {
        None
    } else {
        Some(clicked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_bottom_hides_the_visible_tab_and_shows_any_other() {
        use BottomTab::{Problems, Terminal};
        let cases = [
            (None, Terminal, Some(Terminal)),
            (None, Problems, Some(Problems)),
            (Some(Terminal), Terminal, None),
            (Some(Problems), Problems, None),
            (Some(Terminal), Problems, Some(Problems)),
            (Some(Problems), Terminal, Some(Terminal)),
        ];
        for (current, clicked, expected) in cases {
            assert_eq!(
                next_bottom(current, clicked),
                expected,
                "{current:?} clicked {clicked:?}"
            );
        }
    }
}
