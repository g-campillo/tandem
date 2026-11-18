//! Commands that change the text of the focused code editor.
use super::*;
use std::ops::Range;
use tandem_core::edit::lines;
use tandem_core::edit::{self, Edit, comment::comment_tokens};

impl Workspace {
    /// The active tab's editor and path, when that editor is shown and has keyboard focus.
    pub(super) fn focused_editor(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(Entity<EditorState>, String)> {
        if self.diff.is_some() {
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

    /// Copies the selected lines above or below them (⇧⌥↑, ⇧⌥↓).
    pub(super) fn duplicate_lines(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::duplicate_lines(text, selection, direction)
        });
    }

    /// Swaps the selected lines with the line above or below them (⌥↑, ⌥↓).
    pub(super) fn move_lines(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::move_lines(text, selection, direction)
        });
    }

    /// Deletes the selected lines (⇧⌘K).
    pub(super) fn delete_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::delete_lines(text, selection)
        });
    }

    /// Opens an empty line below or above the cursor's line without splitting it (⌘↵,
    /// ⇧⌘↵). The new line comes from the editor's own Enter, so it gets its smart
    /// indentation.
    pub(super) fn insert_line(&mut self, above: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, _)) = self.editable_editor(window, cx) else {
            return;
        };
        let (text, cursor) = {
            let editor = editor.read(cx);
            (editor.text().to_string(), editor.cursor())
        };
        let start = edit::line_start(&text, cursor);
        if above && start == 0 {
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(0..0, cx);
                editor.replace("\n", window, cx);
                editor.set_selected_range(0..0, cx);
            });
            return;
        }
        let at = if above {
            edit::line_end(&text, start - 1)
        } else {
            edit::line_end(&text, cursor)
        };
        editor.update(cx, |editor, cx| editor.set_selected_range(at..at, cx));
        // Runs the editor's Enter handler synchronously.
        let focus = editor.read(cx).focus_handle(cx);
        focus.dispatch_action(
            &gpui_kit::component::input::Enter {
                secondary: false,
                shift: false,
            },
            window,
            cx,
        );
    }

    /// Joins the selected lines, or the cursor's line with the next one (⌃J).
    pub(super) fn join_lines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::join_lines(text, selection)
        });
    }

    /// Removes the spaces and tabs at the end of every line of the file (⌘K ⌘X).
    pub(super) fn trim_trailing_whitespace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::trim_trailing_whitespace(text, selection)
        });
    }

    /// Sorts the selected lines, or the whole file when one line is selected.
    pub(super) fn sort_lines(
        &mut self,
        descending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::sort_lines(text, selection, descending)
        });
    }

    /// Upper- or lower-cases the selection, or the word under the cursor.
    pub(super) fn transform_case(
        &mut self,
        upper: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_edit(window, cx, |text, selection, _| {
            lines::transform_case(text, selection, upper)
        });
    }
}
