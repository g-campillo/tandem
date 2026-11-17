//! Tandem's IDE layer over the toolkit editor: the per-language editing rules, and the
//! corrections made right after a keystroke (indentation and tag closing). Every edit this
//! layer makes to a buffer goes through [`Workspace::ide_replace`], one undo step each.
mod rules;

pub(super) use rules::register_language_rules;

use super::*;
use gpui_kit::component::input::{InputEdit, Rope};
use tandem_core::edit::Edit;
use tandem_core::edit::indent::{self, IndentUnit};
use tandem_core::edit::tags;

/// The IDE state of each open tab, by the entity id of its editor.
#[derive(Default)]
pub(super) struct IdeUi {
    tabs: HashMap<EntityId, TabIde>,
}

/// What the IDE layer keeps about one tab's editor.
struct TabIde {
    editor: WeakEntity<EditorState>,
    language: &'static str,
    unit: IndentUnit,
    /// Edits made by `ide_replace` whose `InputEditsEvent` has not been handled yet.
    own_edits: usize,
    _subscriptions: Vec<Subscription>,
}

/// The one edit of a single-cursor keystroke: (start, old_end, inserted text), or None
/// when `edits` holds no edit or several (several cursors).
pub(super) fn single_edit(edits: &[(InputEdit, Rope)]) -> Option<(usize, usize, String)> {
    let [(edit, text)] = edits else {
        return None;
    };
    let inserted = text.try_slice(edit.start_byte..edit.new_end_byte).ok()?;
    Some((edit.start_byte, edit.old_end_byte, inserted.to_string()))
}

/// The correction for a single keystroke that inserted `inserted` at `start` (`text` is
/// after it): the indentation fix if there is one, else the close tag after `>` or the
/// completed close tag after `</`.
fn typing_fix(
    text: &str,
    start: usize,
    inserted: &str,
    language: &str,
    unit: IndentUnit,
) -> Option<Edit> {
    indent::after_typed(text, start, inserted, language, unit).or_else(|| match inserted {
        ">" => tags::close_tag(text, start, language),
        "/" => tags::complete_close_tag(text, start, language),
        _ => None,
    })
}

impl Workspace {
    /// Starts the IDE layer for a tab's editor. Called after the tab's own subscription
    /// exists, so the document snapshot is taken before `ide_edits` sees an edit.
    pub(super) fn attach_ide(
        &mut self,
        editor: &Entity<EditorState>,
        language: &'static str,
        unit: IndentUnit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edits = cx.subscribe_in(
            editor,
            window,
            |view, editor, event: &InputEditsEvent, window, cx| {
                view.ide_edits(editor, event, window, cx)
            },
        );
        self.ide.tabs.insert(
            editor.entity_id(),
            TabIde {
                editor: editor.downgrade(),
                language,
                unit,
                own_edits: 0,
                _subscriptions: vec![edits],
            },
        );
    }

    /// Whether the IDE layer must leave the buffers alone: a project is opening or
    /// Claude is editing the files.
    fn ide_refused(&self) -> bool {
        self.loading || self.takeover_running()
    }

    /// Applies one IDE edit as one undo step and selects `edit.selection` afterwards;
    /// counts it so `ide_edits` skips its event. False when refused (loading or takeover)
    /// or when the edit changes nothing.
    pub(in crate::workspace) fn ide_replace(
        &mut self,
        editor: &Entity<EditorState>,
        edit: &Edit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A read-only editor drops the edit without an event, which would leave the count
        // one too high and swallow the user's next keystroke.
        if self.ide_refused() || !edit.changes_text() || !editor.read(cx).is_editable() {
            return false;
        }
        if let Some(tab) = self.ide.tabs.get_mut(&editor.entity_id()) {
            tab.own_edits += 1;
        }
        // The tab's `InputEditsEvent` subscription records the change in the document.
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(edit.range.clone(), cx);
            editor.replace(edit.text.clone(), window, cx);
            editor.set_selected_range(edit.selection.clone(), cx);
        });
        true
    }

    /// Corrects a single keystroke in the focused editor: the indentation after Enter, a
    /// closing bracket alone on its line, a Python block keyword's colon, the close tag
    /// after an open tag's `>` and the name after `</`. The correction is its own undo
    /// step. Edits made by `ide_replace` are skipped.
    fn ide_edits(
        &mut self,
        editor: &Entity<EditorState>,
        event: &InputEditsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.ide.tabs.get_mut(&editor.entity_id()) else {
            return;
        };
        if tab.own_edits > 0 {
            tab.own_edits -= 1;
            return;
        }
        let (language, unit) = (tab.language, tab.unit);
        let Some(((start, _, inserted), (_, after))) =
            single_edit(event.edits()).zip(event.edits().last())
        else {
            return;
        };
        if self.ide_refused() || !editor.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        // The correction is computed on the text after this edit, so it applies only while
        // the editor still shows that text with the cursor right after the inserted text.
        let text = after.to_string();
        let cursor = start + inserted.len();
        let state = editor.read(cx);
        if *state.text() != text || state.selected_range() != (cursor..cursor) {
            return;
        }
        if let Some(fix) = typing_fix(&text, start, &inserted, language, unit) {
            self.ide_replace(editor, &fix, window, cx);
        }
    }

    /// Forgets the tabs whose editor is gone.
    pub(in crate::workspace) fn ide_tick(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ide
            .tabs
            .retain(|_, tab| tab.editor.upgrade().is_some());
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{single_edit, typing_fix};
    use gpui_kit::component::input::{InputEdit, Point as TextPoint, Rope};
    use tandem_core::edit::indent::IndentUnit;

    const FOUR: IndentUnit = IndentUnit {
        width: 4,
        hard_tabs: false,
    };

    /// Types `inserted` at the `|` of `marked` and returns the text after the typing fix
    /// with `|` at the cursor; `None` when there is no fix.
    fn fixed(marked: &str, inserted: &str, language: &str) -> Option<String> {
        let start = marked.find('|').expect("a cursor marker");
        let text = format!("{}{inserted}{}", &marked[..start], &marked[start + 1..]);
        let edit = typing_fix(&text, start, inserted, language, FOUR)?;
        let mut fixed = edit.apply(&text);
        fixed.insert(edit.selection.start, '|');
        Some(fixed)
    }

    #[test]
    fn typing_fix_closes_a_tag_after_its_greater_than_sign() {
        assert_eq!(fixed("<div|", ">", "html").as_deref(), Some("<div>|</div>"));
        assert_eq!(fixed("<div|", ">", "rust"), None);
    }

    #[test]
    fn typing_fix_completes_a_close_tag_after_a_slash() {
        assert_eq!(
            fixed("<section><p>a</p><|", "/", "html").as_deref(),
            Some("<section><p>a</p></section>|")
        );
    }

    #[test]
    fn typing_fix_still_corrects_indentation() {
        assert_eq!(
            fixed("fn f() {\n    x();\n    |", "}", "rust").as_deref(),
            Some("fn f() {\n    x();\n}|")
        );
        assert_eq!(fixed("<div>|", "x", "html"), None);
    }

    /// The edit that turned `before` into `after` by replacing `start..old_end`.
    fn edit(before: &str, after: &str, start: usize, old_end: usize) -> (InputEdit, Rope) {
        let new_end = old_end + after.len() - before.len();
        let point = |offset: usize, text: &str| {
            let row = text[..offset].matches('\n').count();
            let column = offset - text[..offset].rfind('\n').map_or(0, |line| line + 1);
            TextPoint::new(row, column)
        };
        let input = InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(start, before),
            old_end_position: point(old_end, before),
            new_end_position: point(new_end, after),
        };
        (input, Rope::from_str(after))
    }

    #[test]
    fn single_edit_returns_the_inserted_text() {
        let edits = [edit("fn f() {\n    \n}", "fn f() {\n    }\n}", 13, 13)];
        assert_eq!(single_edit(&edits), Some((13, 13, "}".to_string())));
    }

    #[test]
    fn single_edit_returns_a_replacement_and_a_deletion() {
        let replaced = [edit("let ab = 1;", "let xyz = 1;", 4, 6)];
        assert_eq!(single_edit(&replaced), Some((4, 6, "xyz".to_string())));
        let deleted = [edit("let ab = 1;", "let  = 1;", 4, 6)];
        assert_eq!(single_edit(&deleted), Some((4, 6, String::new())));
    }

    #[test]
    fn single_edit_returns_an_enter_with_its_indentation() {
        let edits = [edit("    x", "    x\n    ", 5, 5)];
        assert_eq!(single_edit(&edits), Some((5, 5, "\n    ".to_string())));
    }

    #[test]
    fn single_edit_ignores_no_edit_and_several_edits() {
        assert_eq!(single_edit(&[]), None);
        let first = edit("a\nb", "ax\nb", 1, 1);
        let second = edit("ax\nb", "ax\nbx", 4, 4);
        assert_eq!(single_edit(&[first, second]), None);
    }
}
