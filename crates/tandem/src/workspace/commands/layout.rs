//! Commands that show or hide parts of the window and change how code editors display text.
use super::*;

impl Workspace {
    /// Shows or hides the file explorer (⌘B).
    pub(super) fn toggle_explorer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commands.explorer_visible = !self.commands.explorer_visible;
        // The explorer's input disappears with it, so the keyboard focus moves to the editor.
        if !self.commands.explorer_visible
            && self.query.read(cx).focus_handle(cx).is_focused(window)
        {
            self.go_to_line(None, window, cx);
        }
        cx.notify();
    }

    /// Shows the file explorer and focuses its input (⇧⌘E).
    pub(super) fn show_explorer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar(Sidebar::Files, window, cx);
    }

    /// Hides the bottom panel, or shows it again on the tab it was hidden on (⌘J).
    pub(super) fn toggle_bottom_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.bottom {
            self.commands.last_bottom = tab;
            self.bottom = None;
            cx.notify();
        } else if self.commands.last_bottom == BottomTab::Terminal {
            // Starts the terminal when needed.
            self.toggle_terminal(cx);
        } else {
            self.bottom = Some(BottomTab::Problems);
            cx.notify();
        }
    }

    /// Turns wrapping of long lines on or off in every code editor (⌥Z).
    pub(super) fn toggle_word_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Without this guard the menu's key equivalent would take ⌥Z from the chat, the
        // explorer and the terminal.
        if self.focused_editor(window, cx).is_none() {
            return;
        }
        let wrap = !self.commands.word_wrap;
        self.commands.word_wrap = wrap;
        for tab in &self.tabs {
            tab.editor
                .update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
        }
        self.message(
            if wrap {
                "Word wrap on."
            } else {
                "Word wrap off."
            },
            false,
            cx,
        );
    }

    /// Opens the fold at the cursor (⌥⌘]).
    pub(super) fn unfold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, _)) = self.focused_editor(window, cx) else {
            return;
        };
        editor.update(cx, |editor, cx| {
            let position = editor.cursor_position();
            if editor.display_map().is_folded_at(position.line as usize) {
                // A fold keeps its first line visible, so the line after the cursor's line
                // is inside it.
                editor.unfold_at(Position::new(position.line + 1, 0), cx);
            } else {
                editor.unfold_at(position, cx);
            }
        });
    }

    /// Opens every fold of the focused editor (⌘K ⌘J).
    pub(super) fn unfold_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, _)) = self.focused_editor(window, cx) else {
            return;
        };
        // Disabling folding clears every fold and keeps the fold candidates.
        editor.update(cx, |editor, cx| {
            editor.set_folding(false, window, cx);
            editor.set_folding(true, window, cx);
        });
    }
}
