//! Commands that move between tabs, problems, search matches, lines and earlier locations.
use super::history::{Location, NavHistory, parse_line_target};
use super::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::{RopeExt as _, WindowExt as _};
use std::cell::Cell;

/// The tab after (or before) `active` among `count` tabs, wrapping around.
fn cycle(active: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(active) = active else {
        return Some(0);
    };
    Some(if forward {
        (active + 1) % count
    } else {
        (active + count - 1) % count
    })
}

/// The next problem line after `current` (or the previous one before it), wrapping around.
/// Lines are 1-based.
fn next_problem(lines: &[usize], current: usize, forward: bool) -> Option<usize> {
    let lines = lines.iter().copied();
    if forward {
        lines
            .clone()
            .filter(|&line| line > current)
            .min()
            .or_else(|| lines.min())
    } else {
        lines
            .clone()
            .filter(|&line| line < current)
            .max()
            .or_else(|| lines.max())
    }
}

impl Workspace {
    /// Selects the next (or previous) tab in strip order, wrapping around (⌃Tab, ⌃⇧Tab).
    pub(super) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = cycle(self.active, self.tabs.len(), forward) {
            self.select_tab(index, window, cx);
        }
    }

    /// Selects the tab with this 1-based number, when there is one (⌘1 … ⌘9).
    pub(super) fn select_tab_number(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (1..=self.tabs.len()).contains(&number) {
            self.select_tab(number - 1, window, cx);
        }
    }

    /// Moves the cursor to the next (or previous) problem line of the active file and shows
    /// that problem's message (F8, ⇧F8).
    pub(super) fn go_to_problem(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) else {
            return;
        };
        let path = tab.document.path.clone();
        let current = tab.editor.read(cx).cursor_position().line as usize + 1;
        let lines: Vec<usize> = self
            .dev
            .problems
            .iter()
            .filter(|problem| problem.path == path)
            .map(|problem| problem.line)
            .collect();
        let Some(line) = next_problem(&lines, current, forward) else {
            self.message("No problems in this file.", false, cx);
            return;
        };
        let label = self
            .dev
            .problems
            .iter()
            .find(|problem| problem.path == path && problem.line == line)
            .map(|problem| problem.label.clone())
            .unwrap_or_default();
        self.go_to_line(Some(line), window, cx);
        self.message(label, false, cx);
    }

    /// Selects the next (or previous) match of the editor's last search (⌘G, ⇧⌘G).
    pub(super) fn find_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.settings.open {
            return;
        }
        let Some(editor) = self.diff.clone().or_else(|| {
            self.active
                .and_then(|i| self.tabs.get(i))
                .map(|tab| tab.editor.clone())
        }) else {
            return;
        };
        let found = editor.update(cx, |editor, cx| {
            let range = if forward {
                editor.next_search_match(cx)
            } else {
                editor.previous_search_match(cx)
            };
            if let Some(range) = &range {
                editor.set_selected_range(range.clone(), cx);
            }
            range.is_some()
        });
        if !found {
            self.message(
                "Nothing to find. Press ⌘F and type a search first.",
                false,
                cx,
            );
        }
    }

    /// The active tab's file and cursor.
    fn current_location(&self, cx: &App) -> Option<Location> {
        let tab = self.active.and_then(|i| self.tabs.get(i))?;
        let position = tab.editor.read(cx).cursor_position();
        Some(Location {
            path: tab.document.path.clone(),
            line: position.line,
            column: position.character,
        })
    }

    /// Forgets the history of a project that is no longer open.
    pub(super) fn sync_history(&mut self) {
        if self.commands.history_epoch != self.epoch {
            self.commands.history = NavHistory::default();
            self.commands.closed.clear();
            self.commands.history_epoch = self.epoch;
        }
    }

    /// Remembers where the cursor is before a jump, so Back can return there.
    pub(in crate::workspace) fn record_jump(&mut self, cx: &App) {
        // While a project opens, the tabs still belong to the previous one.
        if self.commands.history_paused || self.loading {
            return;
        }
        self.sync_history();
        if let Some(location) = self.current_location(cx) {
            self.commands.history.record(location);
        }
    }

    /// Returns to the location before the last jump, or to where Back left (⌃-, ⌃⇧-).
    pub(super) fn go_history(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_history();
        let current = self.current_location(cx);
        let target = if forward {
            self.commands.history.forward(current)
        } else {
            self.commands.history.back(current)
        };
        let Some(target) = target else {
            self.message(
                if forward {
                    "No later location."
                } else {
                    "No earlier location."
                },
                false,
                cx,
            );
            return;
        };
        self.commands.history_paused = true;
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.document.path == target.path)
        {
            self.select_tab(index, window, cx);
            self.tabs[index].editor.update(cx, |editor, cx| {
                let position = Position::new(target.line, target.column);
                editor.unfold_at(position, cx);
                editor.set_cursor_position(position, window, cx);
            });
        } else {
            self.open_file(
                target.path,
                false,
                Some(target.line as usize + 1),
                window,
                cx,
            );
        }
        self.commands.history_paused = false;
    }

    /// Opens the Go to Line dialog for the active file, or closes it when it is open (⌃G).
    pub(super) fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = (self.diff.is_none() && !self.settings.open)
            .then(|| self.active.and_then(|i| self.tabs.get(i)))
            .flatten()
            .map(|tab| tab.editor.clone());
        let Some(editor) = editor else {
            self.message("Open a file first.", false, cx);
            return;
        };
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        // The dialog gives the focus back to the editor when it closes.
        editor.update(cx, |editor, cx| editor.focus(window, cx));
        let lines = editor.read(cx).text().lines_len();
        let input = self
            .commands
            .line_input
            .get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx)))
            .clone();
        input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(format!("Line or line:column (1–{lines})"), window, cx);
        });
        let weak = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let weak = weak.clone();
            // Enter in the single-line input reaches the dialog's Confirm binding, which
            // calls `on_ok`; returning `true` closes the dialog.
            dialog
                .title("Go to Line")
                .w(px(360.))
                .child(Input::new(&input))
                .on_ok(move |_, window, cx| {
                    let _ = weak.update(cx, |view, cx| view.confirm_go_to_line(window, cx));
                    true
                })
        });
    }

    /// Moves the cursor to the line (and column) typed into the Go to Line dialog.
    fn confirm_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.commands.line_input.clone() else {
            return;
        };
        let Some(editor) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|tab| tab.editor.clone())
        else {
            return;
        };
        let value = input.read(cx).value();
        let lines = editor.read(cx).text().lines_len();
        let Some((line, column)) = parse_line_target(&value, lines) else {
            self.message("Enter a line number.", false, cx);
            return;
        };
        self.record_jump(cx);
        editor.update(cx, |editor, cx| {
            let position = Position::new(line - 1, column - 1);
            editor.unfold_at(position, cx);
            editor.set_cursor_position(position, window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{cycle, next_problem};

    #[test]
    fn cycle_moves_forward_and_backward_in_the_middle() {
        assert_eq!(cycle(Some(1), 4, true), Some(2));
        assert_eq!(cycle(Some(2), 4, false), Some(1));
    }

    #[test]
    fn cycle_wraps_at_both_ends() {
        assert_eq!(cycle(Some(3), 4, true), Some(0));
        assert_eq!(cycle(Some(0), 4, false), Some(3));
    }

    #[test]
    fn cycle_stays_on_a_single_tab() {
        assert_eq!(cycle(Some(0), 1, true), Some(0));
        assert_eq!(cycle(Some(0), 1, false), Some(0));
    }

    #[test]
    fn cycle_without_an_active_tab_selects_the_first() {
        assert_eq!(cycle(None, 3, true), Some(0));
        assert_eq!(cycle(None, 3, false), Some(0));
    }

    #[test]
    fn cycle_without_tabs_selects_nothing() {
        assert_eq!(cycle(None, 0, true), None);
        assert_eq!(cycle(Some(0), 0, false), None);
    }

    #[test]
    fn next_problem_finds_the_nearest_line_in_each_direction() {
        let lines = [3, 10, 20];
        assert_eq!(next_problem(&lines, 5, true), Some(10));
        assert_eq!(next_problem(&lines, 15, false), Some(10));
    }

    #[test]
    fn next_problem_wraps_around() {
        let lines = [3, 10, 20];
        assert_eq!(next_problem(&lines, 25, true), Some(3));
        assert_eq!(next_problem(&lines, 20, true), Some(3));
        assert_eq!(next_problem(&lines, 1, false), Some(20));
        assert_eq!(next_problem(&lines, 3, false), Some(20));
    }

    #[test]
    fn next_problem_skips_the_current_line() {
        let lines = [3, 10, 20];
        assert_eq!(next_problem(&lines, 10, true), Some(20));
        assert_eq!(next_problem(&lines, 10, false), Some(3));
    }

    #[test]
    fn next_problem_accepts_unsorted_lines() {
        let lines = [20, 3, 10, 3];
        assert_eq!(next_problem(&lines, 4, true), Some(10));
        assert_eq!(next_problem(&lines, 15, false), Some(10));
        assert_eq!(next_problem(&lines, 21, true), Some(3));
        assert_eq!(next_problem(&lines, 2, false), Some(20));
    }

    #[test]
    fn next_problem_without_problems_is_none() {
        assert_eq!(next_problem(&[], 1, true), None);
        assert_eq!(next_problem(&[], 1, false), None);
    }
}
