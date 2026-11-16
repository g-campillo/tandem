//! Commands that change the text of the focused code editor.
use super::*;
use std::ops::Range;
use tandem_core::edit::{self, Edit, comment::comment_tokens};

impl Workspace {
    /// The active tab's editor and path, when that editor is shown and has keyboard focus.
    pub(super) fn focused_editor(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(Entity<EditorState>, String)> {
        if self.diff.is_some() || self.settings.open {
            return None;
        }
        let tab = self.active.and_then(|index| self.tabs.get(index))?;
        tab.editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            .then(|| (tab.editor.clone(), tab.document.path.clone()))
    }

    /// `focused_editor`, unless the editor is read-only right now.
    pub(super) fn editable_editor(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<EditorState>, String)> {
        let found = self.focused_editor(window, cx)?;
        if self.loading {
            self.message("Wait for the project to finish opening.", false, cx);
            return None;
        }
        if self.takeover_running() {
            self.message(
                "The editor is read-only while Claude edits files.",
                false,
                cx,
            );
            return None;
        }
        Some(found)
    }

    /// Runs `compute(text, selection, path)` for the focused, editable editor and applies
    /// the edit it returns as one undo step.
    pub(super) fn apply_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        compute: impl FnOnce(&str, &Range<usize>, &str) -> Option<Edit>,
    ) {
        let Some((editor, path)) = self.editable_editor(window, cx) else {
            return;
        };
        let (text, selection, reversed) = {
            let editor = editor.read(cx);
            (
                editor.text().to_string(),
                editor.selected_range(),
                editor.is_selection_reversed(),
            )
        };
        let Some(edit) = compute(&text, &selection, &path) else {
            return;
        };
        // The tab's `InputEditsEvent` subscription records the change in the document.
        editor.update(cx, |editor, cx| {
            if edit.changes_text() {
                editor.set_selected_range(edit.range.clone(), cx);
                editor.replace(edit.text.clone(), window, cx);
            }
            editor.set_selected_range_with_direction(edit.selection.clone(), reversed, cx);
        });
    }

    /// Adds or removes the file type's line comment marker on the selected lines (⌘/,
    /// ⌘K ⌘C, ⌘K ⌘U).
    pub(super) fn comment_lines(
        &mut self,
        action: LineComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, path)) = self.editable_editor(window, cx) else {
            return;
        };
        let tokens = comment_tokens(&path);
        if tokens.line.is_none() && tokens.block.is_none() {
            self.message("This file type has no comment syntax.", false, cx);
            return;
        }
        self.apply_edit(window, cx, |text, selection, _| {
            edit::comment::comment_lines(text, selection, &tokens, action)
        });
    }

    /// Wraps the selection in the file type's block comment, or removes it (⇧⌥A).
    pub(super) fn toggle_block_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, path)) = self.editable_editor(window, cx) else {
            return;
        };
        let Some((open, close)) = comment_tokens(&path).block else {
            self.message("This file type has no block comment.", false, cx);
            return;
        };
        self.apply_edit(window, cx, |text, selection, _| {
            edit::comment::toggle_block_comment(text, selection, open, close)
        });
    }
}
