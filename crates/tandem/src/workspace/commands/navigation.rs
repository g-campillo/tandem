//! Commands that move between tabs, problems and search matches.
use super::*;

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
