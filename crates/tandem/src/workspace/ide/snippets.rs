//! Snippet sessions: the tab stops of an inserted snippet, drawn over the text while the
//! user fills them in and moves between them with Tab and ⇧Tab, the options of a stop that
//! has choices, and Insert Snippet…, which lists every snippet of the file's language.
use super::complete::ListKind;
use super::*;
use crate::appearance::palette;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{RangeDecoration, RangeDecorationStyle};
use std::ops::Range;
use tandem_lsp::features::completion::AcceptPlan;

/// How strongly the active tab stop is filled with the accent colour.
const ACTIVE_FILL: f32 = 0.18;

impl Workspace {
    /// Insert Snippet…: opens a list of every snippet of the active file's language at the
    /// cursor. The chosen snippet replaces the selection and gets its text as
    /// `TM_SELECTED_TEXT`.
    pub(in crate::workspace) fn insert_snippet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        if self.loading {
            self.message("Wait for the project to finish opening.", false, cx);
            return;
        }
        if self.takeover_running() {
            self.message(
                "The editor is read-only while Claude edits files.",
                false,
                cx,
            );
            return;
        }
        let snippets = self.completion_snippets(target.language, cx);
        if snippets.is_empty() {
            self.message("There are no snippets for this kind of file.", false, cx);
            return;
        }
        // From the menu bar the focus can be elsewhere; the list's keys go to the editor.
        target
            .editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        let start = target.editor.read(cx).selected_range().start;
        let candidates = completion::snippet_candidates(start, &snippets);
        self.open_fixed_list(
            &target.editor,
            ListKind::Snippets,
            start,
            candidates,
            snippets,
            cx,
        );
    }

    /// Puts a snippet chosen from Insert Snippet…'s list into `editor`: it replaces the text
    /// from `start` to the end of the selection, gets `selected` (the text that was
    /// selected when the list opened) as `TM_SELECTED_TEXT`, and its tab stops start.
    pub(super) fn insert_chosen_snippet(
        &mut self,
        editor: EntityId,
        start: usize,
        selected: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)
        else {
            return;
        };
        let (text, end) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.selected_range().end)
        };
        if start > end {
            return;
        }
        let variables = self.snippet_variables(&target, &text, start, selected, cx);
        let plan = completion::accept_snippet(&text, end, start, body, &variables, target.unit);
        self.apply_accepted(&target.editor, plan, window, cx);
    }

    /// Puts an accepted entry's edit into `editor` as one undo step, selects where the plan
    /// says and starts the plan's snippet session. False when the edit was refused.
    pub(super) fn apply_accepted(
        &mut self,
        editor: &Entity<EditorState>,
        plan: AcceptPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if plan.edit.changes_text() && !self.ide_replace(editor, &plan.edit, window, cx) {
            return false;
        }
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(plan.selection.clone(), cx)
        });
        if let Some(session) = plan.session {
            self.start_snippet(editor, session, cx);
        }
        true
    }

    /// Starts filling in a snippet inserted in `editor`: its tab stops are drawn, and the
    /// options of the first stop are offered when it has some. A snippet still being
    /// filled in elsewhere ends.
    fn start_snippet(
        &mut self,
        editor: &Entity<EditorState>,
        session: SnippetSession,
        cx: &mut Context<Self>,
    ) {
        let id = editor.entity_id();
        let others: Vec<EntityId> = self
            .ide
            .tabs
            .iter()
            .filter(|(other, tab)| **other != id && tab.session.is_some())
            .map(|(other, _)| *other)
            .collect();
        for other in others {
            self.end_snippet(other, cx);
        }
        let Some(tab) = self.ide.tabs.get_mut(&id) else {
            return;
        };
        tab.session = Some(session);
        self.draw_snippet(id, cx);
        self.offer_choices(editor, cx);
    }

    /// Stops filling in the snippet of `editor`: its tab stops are no longer drawn, and a
    /// list of a stop's options closes.
    fn end_snippet(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        if let Some(tab) = self.ide.tabs.get_mut(&editor) {
            tab.session = None;
            if let Some(placeholders) = &tab.placeholders {
                placeholders.clear(cx);
            }
        }
        self.ide.sync_keys();
        self.close_choices(editor, cx);
    }

    /// Draws the tab stops of the snippet in `editor` where they are now: the active stop
    /// filled with the accent colour, the other stops framed.
    fn draw_snippet(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        let p = palette(cx.theme().mode);
        let Some(tab) = self.ide.tabs.get_mut(&editor) else {
            return;
        };
        let (Some(session), Some(state)) = (&tab.session, tab.editor.upgrade()) else {
            return;
        };
        let decorations =
            placeholder_decorations(session.ranges(), p.accent.alpha(ACTIVE_FILL), p.border);
        let placeholders = tab.placeholders.get_or_insert_with(|| {
            state.update(cx, |state, cx| {
                state.create_range_decorations_collection(Vec::new(), cx)
            })
        });
        placeholders.set(decorations, cx);
        self.ide.sync_keys();
    }

    /// Offers the options of the active stop of the snippet in `editor`, when it has some,
    /// in a list hanging below the stop.
    fn offer_choices(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let stop = self
            .ide
            .tabs
            .get(&editor.entity_id())
            .and_then(|tab| tab.session.as_ref())
            .map(|session| session.active_stop().clone());
        let Some((range, stop)) = stop.and_then(|stop| Some((stop.ranges.first()?.clone(), stop)))
        else {
            return;
        };
        if stop.choices.is_empty() {
            return;
        }
        let candidates = completion::choice_candidates(range.start, &stop.choices);
        self.open_fixed_list(
            editor,
            ListKind::Choices,
            range.start,
            candidates,
            Vec::new(),
            cx,
        );
    }

    /// Keeps the snippet session of `editor` in step with edits made there, each a
    /// replacement of `start..old_end` (offsets in the text before it) by `new_len` bytes,
    /// in order. An edit outside the snippet ends the session.
    pub(super) fn snippet_followed(
        &mut self,
        editor: EntityId,
        changes: impl IntoIterator<Item = (usize, usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self
            .ide
            .tabs
            .get_mut(&editor)
            .and_then(|tab| tab.session.as_mut())
        else {
            return;
        };
        if follow(session, changes) {
            self.draw_snippet(editor, cx);
        } else {
            self.end_snippet(editor, cx);
        }
    }

    /// After `editor` changed, ends its snippet session once the cursor has left the
    /// snippet.
    pub(super) fn snippet_moved(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let id = editor.entity_id();
        if !self
            .ide
            .tabs
            .get(&id)
            .is_some_and(|tab| tab.session.is_some())
        {
            return;
        }
        // The editor's notification can come before the event with the edits that move the
        // snippet, so the cursor is compared with it once this update is over.
        let view = cx.entity().downgrade();
        cx.defer(move |cx| {
            view.update(cx, |view, cx| view.end_snippet_left(id, cx))
                .ok();
        });
    }

    /// Ends the snippet session of `editor` when the cursor is outside the snippet.
    fn end_snippet_left(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        let Some(tab) = self.ide.tabs.get(&editor) else {
            return;
        };
        let (Some(session), Some(state)) = (&tab.session, tab.editor.upgrade()) else {
            return;
        };
        if !session.contains(state.read(cx).cursor()) {
            self.end_snippet(editor, cx);
        }
    }

    /// Handles Tab, ⇧Tab and Escape for the snippet being filled in in the `focused`
    /// editor: Tab and ⇧Tab copy the active stop's text into its mirrors and move to the
    /// next or previous stop (past the last one, to the end of the snippet, which ends the
    /// session); Escape copies the mirrors and ends the session. True when the session took
    /// the key. While the editor refuses edits, the keys change nothing.
    pub(super) fn snippet_key(
        &mut self,
        key: IdeKey,
        focused: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let forward = match key {
            IdeKey::Tab => Some(true),
            IdeKey::ShiftTab => Some(false),
            IdeKey::Escape => None,
            _ => return false,
        };
        let Some(id) = focused else {
            return false;
        };
        let Some(tab) = self.ide.tabs.get_mut(&id) else {
            return false;
        };
        let (Some(editor), Some(session)) = (tab.editor.upgrade(), tab.session.take()) else {
            return false;
        };
        let text = editor.read(cx).text().to_string();
        let Some(forward) = forward else {
            if let Some(edit) = session.finish(&text) {
                self.ide_replace(&editor, &edit, window, cx);
            }
            self.end_snippet(id, cx);
            return true;
        };
        let advance = session.advance(&text, forward);
        if let Some(edit) = advance.edit.as_ref().filter(|edit| edit.changes_text())
            && !self.ide_replace(&editor, edit, window, cx)
        {
            self.restore_snippet(id, session);
            return true;
        }
        self.close_choices(id, cx);
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(advance.selection.clone(), cx)
        });
        let Some(next) = advance.next else {
            self.end_snippet(id, cx);
            return true;
        };
        self.restore_snippet(id, next);
        self.draw_snippet(id, cx);
        self.offer_choices(&editor, cx);
        true
    }

    /// Puts option `choice` of the active stop into the snippet of `editor`, with the
    /// cursor after it.
    pub(super) fn accept_choice(
        &mut self,
        editor: EntityId,
        choice: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.ide.tabs.get_mut(&editor) else {
            return;
        };
        let (Some(state), Some(session)) = (tab.editor.upgrade(), tab.session.take()) else {
            return;
        };
        let Some(value) = session.active_stop().choices.get(choice).cloned() else {
            self.restore_snippet(editor, session);
            return;
        };
        let (edit, next, selection) = session.replace_active(&value);
        if edit.changes_text() && !self.ide_replace(&state, &edit, window, cx) {
            self.restore_snippet(editor, session);
            return;
        }
        state.update(cx, |state, cx| state.set_selected_range(selection, cx));
        self.restore_snippet(editor, next);
        self.draw_snippet(editor, cx);
    }

    /// Gives the tab of `editor` its snippet session back.
    fn restore_snippet(&mut self, editor: EntityId, session: SnippetSession) {
        if let Some(tab) = self.ide.tabs.get_mut(&editor) {
            tab.session = Some(session);
        }
    }
}

/// Follows `changes` in `session` in order, each a replacement of `start..old_end` by
/// `new_len` bytes. False when one of them was outside the snippet, which ends it.
fn follow(
    session: &mut SnippetSession,
    changes: impl IntoIterator<Item = (usize, usize, usize)>,
) -> bool {
    changes
        .into_iter()
        .all(|(start, old_end, new_len)| session.adjust(start, old_end, new_len))
}

/// The decorations of a snippet's tab stops (`ranges`, each with whether it belongs to the
/// active stop): the active stop filled with `fill`, the other stops framed with `frame`.
fn placeholder_decorations(
    ranges: Vec<(Range<usize>, bool)>,
    fill: Hsla,
    frame: Hsla,
) -> Vec<RangeDecoration> {
    ranges
        .into_iter()
        .map(|(range, active)| {
            let (style, color) = if active {
                (RangeDecorationStyle::Fill, fill)
            } else {
                (RangeDecorationStyle::Frame, frame)
            };
            RangeDecoration::new(range)
                .with_style(style)
                .with_color(color)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{follow, placeholder_decorations};
    use gpui_kit::Hsla;
    use gpui_kit::component::input::RangeDecorationStyle;
    use std::collections::HashMap;
    use tandem_core::edit::snippet::session::SnippetSession;
    use tandem_core::edit::snippet::{SnippetIndent, expand};

    /// The session of `body` inserted at `at`.
    fn session(body: &str, at: usize) -> SnippetSession {
        let indent = SnippetIndent {
            line: String::new(),
            unit: "    ".into(),
        };
        let expansion = expand(body, &HashMap::new(), &indent);
        SnippetSession::start(&expansion, at)
            .expect("a snippet with tab stops")
            .0
    }

    #[test]
    fn typing_over_the_active_stop_moves_the_stops_after_it() {
        // "fn name() {}": `name` at 3..7, `$2` at 8.
        let mut snippet = session("fn ${1:name}($2) {}", 0);
        assert!(follow(&mut snippet, [(3, 7, 1)]));
        assert_eq!(snippet.ranges(), [(3..4, true), (5..5, false)]);
    }

    #[test]
    fn several_edits_are_followed_in_order() {
        // After "ab": `name` at 5..9, `$2` at 10. An insertion before the snippet, then
        // typing over `name` where it is after that insertion.
        let mut snippet = session("fn ${1:name}($2) {}", 2);
        assert!(follow(&mut snippet, [(0, 0, 2), (7, 11, 1)]));
        assert_eq!(snippet.ranges(), [(7..8, true), (9..9, false)]);
    }

    #[test]
    fn an_edit_after_the_snippet_ends_it() {
        let mut snippet = session("fn ${1:name}($2) {}", 0);
        assert!(!follow(&mut snippet, [(13, 13, 1)]));
        let mut snippet = session("fn ${1:name}($2) {}", 0);
        assert!(!follow(&mut snippet, [(13, 13, 1), (3, 7, 1)]));
    }

    #[test]
    fn the_active_stop_is_filled_and_the_others_framed() {
        let (fill, frame) = (Hsla::white(), Hsla::black());
        let decorations = placeholder_decorations(
            vec![(3..7, true), (8..8, false), (12..15, true)],
            fill,
            frame,
        );
        let drawn: Vec<_> = decorations
            .iter()
            .map(|decoration| {
                (
                    decoration.range().clone(),
                    decoration.style(),
                    decoration.color(),
                )
            })
            .collect();
        assert_eq!(
            drawn,
            [
                (3..7, RangeDecorationStyle::Fill, Some(fill)),
                (8..8, RangeDecorationStyle::Frame, Some(frame)),
                (12..15, RangeDecorationStyle::Fill, Some(fill)),
            ]
        );
    }
}
