//! Commands that change the selection or add cursors in the focused code editor.
use super::*;
use gpui_kit::component::input::Rope;
use std::ops::Range;
use tandem_core::edit::{self, select};

/// At most this many occurrences are selected at once (⇧⌘L).
const MAX_OCCURRENCES: usize = 1_000;

/// The selections ⌘D has added so far, valid while the editor's text and primary selection
/// are unchanged.
pub(super) struct OccurrenceSession {
    editor: EntityId,
    text: Rope,
    needle: String,
    whole_word: bool,
    /// The primary selection first, then each added occurrence in the order it was added.
    taken: Vec<Range<usize>>,
}

/// The selections Expand Selection has passed through, so Shrink Selection can go back.
pub(super) struct ExpansionSession {
    editor: EntityId,
    text: Rope,
    stack: Vec<Range<usize>>,
    current: Range<usize>,
}

/// The focused editor with its path, its text and its primary selection. The toolkit cannot
/// list its cursors, so the commands below remember what they added themselves.
struct Focused {
    editor: Entity<EditorState>,
    path: String,
    rope: Rope,
    text: String,
    current: Range<usize>,
}

impl Workspace {
    /// The active tab's editor when it has keyboard focus. Selection commands also work
    /// while the editor is read-only.
    fn focused_selection(&self, window: &Window, cx: &App) -> Option<Focused> {
        let (editor, path) = self.focused_editor(window, cx)?;
        let (rope, current) = {
            let state = editor.read(cx);
            (state.text().clone(), state.selected_range())
        };
        Some(Focused {
            editor,
            path,
            text: rope.to_string(),
            rope,
            current,
        })
    }

    /// Selects the word under the cursor, then adds its next occurrence on every further
    /// press (⌘D).
    pub(super) fn add_next_occurrence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            rope,
            text,
            current,
            ..
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        let id = editor.entity_id();
        let continues = self.commands.occurrences.as_ref().is_some_and(|session| {
            session.editor == id && session.text == rope && session.taken.first() == Some(&current)
        });
        if !continues {
            if current.is_empty() {
                // The first press only selects the word.
                let Some(range) = edit::word_range_at(&text, current.start) else {
                    return;
                };
                editor.update(cx, |editor, cx| {
                    editor.set_selected_range(range.clone(), cx)
                });
                self.commands.occurrences = Some(OccurrenceSession {
                    editor: id,
                    text: rope,
                    needle: text[range.clone()].to_string(),
                    whole_word: true,
                    taken: vec![range],
                });
                return;
            }
            self.commands.occurrences = Some(OccurrenceSession {
                editor: id,
                text: rope,
                needle: text[current.clone()].to_string(),
                whole_word: false,
                taken: vec![current],
            });
        }
        let Some(session) = self.commands.occurrences.as_mut() else {
            return;
        };
        let found = select::occurrences(&text, &session.needle, session.whole_word);
        let after = session.taken.last().map_or(0, |range| range.end);
        match select::next_occurrence(&found, &session.taken, after) {
            Some(range) => {
                editor.update(cx, |editor, cx| editor.add_selection(range.clone(), cx));
                session.taken.push(range);
            }
            None => self.message("No more occurrences.", false, cx),
        }
    }

    /// Selects every occurrence of the selected text, or of the word under the cursor (⇧⌘L).
    pub(super) fn select_all_occurrences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            text,
            current,
            ..
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        let (target, whole_word) = if current.is_empty() {
            let Some(word) = edit::word_range_at(&text, current.start) else {
                return;
            };
            (word, true)
        } else {
            (current, false)
        };
        let mut found = select::occurrences(&text, &text[target.clone()], whole_word);
        if found.is_empty() {
            return;
        }
        let total = found.len();
        found.truncate(MAX_OCCURRENCES);
        let primary = found
            .iter()
            .find(|range| range.contains(&target.start))
            .unwrap_or(&found[0])
            .clone();
        let mut others: Vec<Range<usize>> = found
            .iter()
            .filter(|range| **range != primary)
            .cloned()
            .collect();
        // Each added selection scrolls the view to itself, so the nearest come last.
        others.sort_by_key(|range| std::cmp::Reverse(range.start.abs_diff(primary.start)));
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(primary, cx);
            for range in others {
                editor.add_selection(range, cx);
            }
        });
        self.commands.occurrences = None;
        let message = match found.len() {
            selected if selected < total => {
                format!("Selected the first 1,000 of {total} occurrences.")
            }
            1 => "1 occurrence selected.".to_string(),
            selected => format!("{selected} occurrences selected."),
        };
        self.message(message, false, cx);
    }

    /// Grows the selection to the next larger word, string, bracket pair, line or the
    /// whole text (⌃⇧→).
    pub(super) fn expand_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            path,
            rope,
            text,
            current,
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        // In Rust an apostrophe starts a lifetime more often than a char literal.
        let quotes: &[char] = if path.ends_with(".rs") {
            &['"', '`']
        } else {
            &['"', '\'', '`']
        };
        let Some(next) = select::expand_selection(&text, &current, quotes) else {
            return;
        };
        let id = editor.entity_id();
        let mut session = self
            .commands
            .expansion
            .take()
            .filter(|session| {
                session.editor == id && session.text == rope && session.current == current
            })
            .unwrap_or_else(|| ExpansionSession {
                editor: id,
                text: rope,
                stack: Vec::new(),
                current: current.clone(),
            });
        session.stack.push(current);
        session.current = next.clone();
        editor.update(cx, |editor, cx| editor.set_selected_range(next, cx));
        self.commands.expansion = Some(session);
    }

    /// Goes back to the selection before the last Expand Selection (⌃⇧←).
    pub(super) fn shrink_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            rope,
            current,
            ..
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        let id = editor.entity_id();
        let Some(session) = self.commands.expansion.as_mut().filter(|session| {
            session.editor == id && session.text == rope && session.current == current
        }) else {
            return;
        };
        let Some(previous) = session.stack.pop() else {
            return;
        };
        session.current = previous.clone();
        editor.update(cx, |editor, cx| editor.set_selected_range(previous, cx));
    }

    /// Selects the lines the selection touches with their line break, or adds the next line
    /// when they are already selected.
    pub(super) fn select_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            text,
            current,
            ..
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        let lines = select::select_line(&text, &current);
        editor.update(cx, |editor, cx| editor.set_selected_range(lines, cx));
    }

    /// Puts a cursor at the end of every selected line (⇧⌥I).
    pub(super) fn add_cursors_to_line_ends(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused {
            editor,
            text,
            current,
            ..
        }) = self.focused_selection(window, cx)
        else {
            return;
        };
        let ends = select::line_end_cursors(&text, &current);
        if ends.len() < 2 {
            return;
        }
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(ends[0]..ends[0], cx);
            for &end in &ends[1..] {
                editor.add_selection(end..end, cx);
            }
        });
    }

    /// Moves the cursor to the bracket matching the one next to it, or to the closer of the
    /// pair around it (⇧⌘\).
    pub(super) fn go_to_bracket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Focused { editor, text, .. }) = self.focused_selection(window, cx) else {
            return;
        };
        let cursor = editor.read(cx).cursor();
        match select::matching_bracket(&text, cursor) {
            Some(target) => editor.update(cx, |editor, cx| {
                editor.set_selected_range(target..target, cx)
            }),
            None => self.message("No matching bracket.", false, cx),
        }
    }
}
