//! The command palette (⇧⌘P): every registry command with its shortcut, filtered while typing.
use super::*;
use gpui_kit::component::command::{Command as Palette, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex};
use std::cell::Cell;

/// One row per registry command, in registry order, so a row index is a `commands()` index.
fn palette_items() -> Vec<CommandItem> {
    commands()
        .into_iter()
        .map(|spec| {
            let title = spec.title;
            let shortcut = spec.shortcut;
            // No `.action(…)`: the toolkit would dispatch it while the palette's own search
            // field has focus. `run_command` dispatches it once the focus is back.
            CommandItem::new()
                .label(title)
                .keywords([spec.menu.title()])
                .child(move |_, cx| {
                    h_flex()
                        .flex_1()
                        .justify_between()
                        .gap_4()
                        .child(title)
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(shortcut),
                        )
                })
        })
        .collect()
}

impl Workspace {
    /// Opens the command palette, or closes it when it is already open.
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let state = self
            .commands
            .palette
            .get_or_insert_with(|| cx.new(|cx| CommandState::new(window, cx)))
            .clone();
        state.update(cx, |state, cx| state.set_query("", window, cx));
        let weak = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                let focus = state.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }
            let weak = weak.clone();
            dialog.close_button(false).w(px(560.)).p_0().child(
                Palette::new(&state)
                    .bordered(false)
                    .placeholder("Type a command")
                    .items(palette_items())
                    .on_confirm(move |index, window, cx| {
                        window.close_dialog(cx);
                        let _ = weak.update(cx, |view, cx| view.run_command(index.row, window, cx));
                    }),
            )
        });
    }

    /// Runs the registry command at `row`. `close_dialog` has already put the focus back
    /// where it was before the palette opened, and `dispatch_action` targets the focused
    /// element, so toolkit actions reach the editor and Tandem actions their app-level handler.
    fn run_command(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = commands().into_iter().nth(row) else {
            return;
        };
        if spec.scope == KeyScope::Editor {
            let editor = (self.diff.is_none() && !self.settings.open)
                .then(|| self.active.and_then(|index| self.tabs.get(index)))
                .flatten()
                .map(|tab| tab.editor.clone());
            let Some(editor) = editor else {
                self.message("Open a file to use this command.", false, cx);
                return;
            };
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
        window.dispatch_action(spec.action(), cx);
    }
}
