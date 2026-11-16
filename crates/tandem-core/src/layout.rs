//! The window's pane layout: which side the file tree and the chat sit on, and the widths
//! the user dragged them to. It is one preference for every project, saved in `settings`
//! under `pane_layout`.
use crate::Store;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// The two panes beside the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The sidebar with the file tree and project search.
    Files,
    Chat,
}

/// The side of the editor a pane sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneLayout {
    /// `false`: file tree on the left, chat on the right. `true`: the other way round.
    pub swapped: bool,
    /// The width the user dragged the file tree to, in pixels. `None` follows the window.
    pub files_width: Option<f32>,
    /// The width the user dragged the chat to, in pixels. `None` follows the window.
    pub chat_width: Option<f32>,
}

impl PaneLayout {
    pub fn side(&self, pane: Pane) -> PaneSide {
        match (pane, self.swapped) {
            (Pane::Files, false) | (Pane::Chat, true) => PaneSide::Left,
            (Pane::Files, true) | (Pane::Chat, false) => PaneSide::Right,
        }
    }

    /// The width the user chose for `pane`, if any. A width belongs to its pane, not to a
    /// side, so swapping keeps it.
    pub fn width(&self, pane: Pane) -> Option<f32> {
        match pane {
            Pane::Files => self.files_width,
            Pane::Chat => self.chat_width,
        }
    }

    /// Sets the width the user chose for `pane`; `None` returns it to the default.
    pub fn set_width(&mut self, pane: Pane, width: Option<f32>) {
        match pane {
            Pane::Files => self.files_width = width,
            Pane::Chat => self.chat_width = width,
        }
    }
}

impl Store {
    /// The saved pane layout, or the default layout when none was saved.
    pub fn pane_layout(&self) -> Result<PaneLayout> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key='pane_layout'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).context("The saved pane layout is damaged"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn save_pane_layout(&mut self, layout: &PaneLayout) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key,value) VALUES('pane_layout',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(layout)?],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_layout_has_the_file_tree_left_and_the_chat_right() {
        let layout = PaneLayout::default();
        assert!(!layout.swapped);
        assert_eq!(layout.side(Pane::Files), PaneSide::Left);
        assert_eq!(layout.side(Pane::Chat), PaneSide::Right);
        assert_eq!(layout.width(Pane::Files), None);
        assert_eq!(layout.width(Pane::Chat), None);
    }

    #[test]
    fn swapping_moves_both_panes_and_keeps_each_panes_width() {
        let mut layout = PaneLayout::default();
        layout.set_width(Pane::Files, Some(300.));
        layout.set_width(Pane::Chat, Some(480.));
        layout.swapped = true;
        assert_eq!(layout.side(Pane::Files), PaneSide::Right);
        assert_eq!(layout.side(Pane::Chat), PaneSide::Left);
        assert_eq!(layout.width(Pane::Files), Some(300.));
        assert_eq!(layout.width(Pane::Chat), Some(480.));
    }

    #[test]
    fn set_width_changes_only_that_pane_and_none_clears_it() {
        let mut layout = PaneLayout::default();
        layout.set_width(Pane::Chat, Some(480.));
        assert_eq!(layout.chat_width, Some(480.));
        assert_eq!(layout.files_width, None);
        layout.set_width(Pane::Chat, None);
        assert_eq!(layout, PaneLayout::default());
    }

    #[test]
    fn missing_fields_take_their_defaults_and_unknown_fields_are_ignored() {
        let layout: PaneLayout = serde_json::from_str(r#"{"swapped":true,"later":1}"#).unwrap();
        assert_eq!(
            layout,
            PaneLayout {
                swapped: true,
                files_width: None,
                chat_width: None,
            }
        );
    }
}
