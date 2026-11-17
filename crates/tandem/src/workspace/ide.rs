//! Tandem's IDE layer over the toolkit editor: the per-language editing rules, the
//! corrections made right after a keystroke (indentation and tag closing) and Tandem's own
//! completion list. Every edit this layer makes to a buffer goes through
//! [`Workspace::ide_replace`], one undo step each.
mod complete;
mod icons;
mod popup;
mod rules;

pub(super) use rules::register_language_rules;

use super::fonts::Face;
use super::*;
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InputEdit, MoveDown, MovePageDown, MovePageUp, MoveUp,
    OutdentInline, Rope,
};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use tandem_core::edit::Edit;
use tandem_core::edit::comment::comment_tokens;
use tandem_core::edit::indent::{self, IndentUnit};
use tandem_core::edit::snippet::library::UserSnippets;
use tandem_core::edit::tags;
use tandem_lsp::LanguageManager;
use tandem_lsp::features::completion;

/// The IDE state of each open tab, by the entity id of its editor, and of the popups.
#[derive(Default)]
pub(super) struct IdeUi {
    tabs: HashMap<EntityId, TabIde>,
    /// The completion list, on screen or waiting for the server.
    completion: Option<complete::CompletionState>,
    /// The user's snippet folder, set up on the first completion.
    snippets: Option<UserSnippets>,
    /// The snippet file errors already shown in the status bar.
    snippet_errors: HashSet<String>,
    /// Counts the completion lists opened, so each one's documentation starts afresh.
    lists: u64,
    /// An accepted completion waiting for its details; dropping it cancels it.
    accepting: Option<Task<()>>,
    /// What the key listeners may take, kept current with the popups.
    keys: Rc<Cell<KeyClaim>>,
}

impl IdeUi {
    /// Tells the key listeners which popups are on screen now.
    fn sync_keys(&self) {
        self.keys.set(KeyClaim {
            list: self
                .completion
                .as_ref()
                .filter(|list| list.shown())
                .map(|list| list.editor_id),
        });
    }
}

/// What the IDE layer keeps about one tab's editor.
struct TabIde {
    editor: WeakEntity<EditorState>,
    language: &'static str,
    unit: IndentUnit,
    /// Edits made by `ide_replace` whose `InputEditsEvent` has not been handled yet.
    own_edits: usize,
    /// The language server's completion trigger characters, shared with the tab's
    /// completion source and kept current by `ide_tick`.
    triggers: Rc<RefCell<Vec<String>>>,
    _subscriptions: Vec<Subscription>,
}

/// The active tab while its editor is on screen, with what the IDE features need to know
/// about it.
pub(in crate::workspace) struct IdeTarget {
    pub editor: Entity<EditorState>,
    /// The language server of the file, while language tools run.
    pub manager: Option<LanguageManager>,
    /// The file's absolute path.
    pub file: PathBuf,
    /// The file's project-relative path.
    pub relative: String,
    /// The tab's document instance, as the language server knows it.
    pub instance: String,
    pub language: &'static str,
    pub unit: IndentUnit,
}

/// The editor keys the IDE popups use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum IdeKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Tab,
    ShiftTab,
    Escape,
}

/// The popups on screen, as the key listeners see them. The listeners run while an action
/// is dispatched, sometimes from inside a workspace update (Insert Line Below dispatches
/// Enter to the editor), so they decide from this cell whether a key is taken and leave
/// the handling to [`Workspace::ide_key`] once the dispatch is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KeyClaim {
    /// The editor whose completion list is on screen.
    list: Option<EntityId>,
}

impl KeyClaim {
    /// Whether `ide_key` takes `key` pressed in `editor`: the completion list takes every
    /// key but ⇧Tab.
    fn takes(&self, editor: EntityId, key: IdeKey) -> bool {
        self.list == Some(editor) && key != IdeKey::ShiftTab
    }

    /// Whether `ide_key` has a popup to close in an editor other than `editor`.
    fn elsewhere(&self, editor: EntityId) -> bool {
        self.list.is_some_and(|list| list != editor)
    }
}

/// The IDE layer of the editor on screen: the key listeners and the popups drawn over it.
pub(in crate::workspace) struct IdeLayer {
    workspace: WeakEntity<Workspace>,
    editor: EntityId,
    focus: FocusHandle,
    keys: Rc<Cell<KeyClaim>>,
    overlays: Vec<AnyElement>,
}

impl IdeLayer {
    /// Adds `capture_action` listeners for MoveUp, MoveDown, MovePageUp, MovePageDown,
    /// Enter, IndentInline, OutdentInline and Escape that hand the key to `ide_key` and
    /// stop it from reaching the editor when an IDE popup takes it; then the overlays.
    /// Enter counts only without ⌘, ⇧ or the secondary flag, so ⌘↵ stays Insert Line Below.
    pub fn apply(self, container: Div) -> Div {
        let route = KeyRoute {
            workspace: self.workspace,
            editor: self.editor,
            focus: self.focus,
            keys: self.keys,
            replaying: Rc::default(),
        };
        container
            .capture_action(route.listener(|_: &MoveUp, _| Some(IdeKey::Up)))
            .capture_action(route.listener(|_: &MoveDown, _| Some(IdeKey::Down)))
            .capture_action(route.listener(|_: &MovePageUp, _| Some(IdeKey::PageUp)))
            .capture_action(route.listener(|_: &MovePageDown, _| Some(IdeKey::PageDown)))
            .capture_action(route.listener(|enter: &Enter, window| {
                (!enter.secondary && !enter.shift && !window.modifiers().platform)
                    .then_some(IdeKey::Enter)
            }))
            .capture_action(route.listener(|_: &IndentInline, _| Some(IdeKey::Tab)))
            .capture_action(route.listener(|_: &OutdentInline, _| Some(IdeKey::ShiftTab)))
            .capture_action(route.listener(|_: &Escape, _| Some(IdeKey::Escape)))
            .children(self.overlays)
    }
}

/// Hands the editor keys of one editor to the workspace.
#[derive(Clone)]
struct KeyRoute {
    workspace: WeakEntity<Workspace>,
    editor: EntityId,
    focus: FocusHandle,
    keys: Rc<Cell<KeyClaim>>,
    /// Set while a key `ide_key` did not take after all goes back to the editor.
    replaying: Rc<Cell<bool>>,
}

impl KeyRoute {
    /// A capture listener for `A`, which `key` maps to an IDE key or to none.
    fn listener<A: Action>(
        &self,
        key: impl Fn(&A, &Window) -> Option<IdeKey> + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let route = self.clone();
        move |action, window, cx| {
            if let Some(key) = key(action, window)
                && !route.replaying.get()
            {
                route.route(key, action.boxed_clone(), window, cx);
            }
        }
    }

    /// Stops `key` from reaching the editor when a popup takes it, and has `ide_key`
    /// handle it, or close a popup left open in another editor, once the dispatch is over.
    /// A stopped key that `ide_key` does not take then (the popup closed meanwhile) goes
    /// back to the editor as `action`.
    fn route(&self, key: IdeKey, action: Box<dyn Action>, window: &mut Window, cx: &mut App) {
        let claim = self.keys.get();
        if claim == KeyClaim::default() {
            return;
        }
        let focused = self.focus.is_focused(window);
        let stopped = focused && claim.takes(self.editor, key);
        if stopped {
            cx.stop_propagation();
        } else if focused && !claim.elsewhere(self.editor) {
            return;
        }
        let route = self.clone();
        window.defer(cx, move |window, cx| {
            let taken = route
                .workspace
                .update(cx, |view, cx| view.ide_key(key, window, cx))
                .unwrap_or(false);
            if stopped && !taken {
                route.replaying.set(true);
                route.focus.dispatch_action(action.as_ref(), window, cx);
                route.replaying.set(false);
            }
        });
    }
}

/// Snippet variables that come from the document: the file's names and comment markers,
/// and the line, word and selection at `offset` in `text`. Comment markers the file type
/// lacks are empty.
fn document_variables(
    file: &Path,
    relative: &str,
    language: &str,
    text: &str,
    offset: usize,
    selection: &str,
) -> HashMap<String, String> {
    let offset = text.floor_char_boundary(offset);
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    let line_index = text[..offset].matches('\n').count();
    let word_start = completion::word_start(text, offset, language);
    let word_end = offset
        + text[offset..]
            .chars()
            .take_while(|c| completion::is_word_char(*c, language))
            .map(char::len_utf8)
            .sum::<usize>();
    let name = relative.rsplit('/').next().unwrap_or(relative);
    let base = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let directory = file
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let tokens = comment_tokens(relative);
    let (block_start, block_end) = tokens.block.unwrap_or_default();
    [
        ("TM_SELECTED_TEXT", selection.to_string()),
        (
            "TM_CURRENT_LINE",
            text[line_start..line_end]
                .trim_end_matches('\r')
                .to_string(),
        ),
        ("TM_CURRENT_WORD", text[word_start..word_end].to_string()),
        ("TM_LINE_INDEX", line_index.to_string()),
        ("TM_LINE_NUMBER", (line_index + 1).to_string()),
        ("TM_FILENAME", name.to_string()),
        ("TM_FILENAME_BASE", base),
        ("TM_DIRECTORY", directory),
        ("TM_FILEPATH", file.display().to_string()),
        ("RELATIVE_FILEPATH", relative.to_string()),
        ("LINE_COMMENT", tokens.line.unwrap_or_default().to_string()),
        ("BLOCK_COMMENT_START", block_start.to_string()),
        ("BLOCK_COMMENT_END", block_end.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// Snippet variables for the date and time `now`.
fn date_variables<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>) -> HashMap<String, String>
where
    Tz::Offset: std::fmt::Display,
{
    let formats = [
        ("CURRENT_YEAR", "%Y"),
        ("CURRENT_YEAR_SHORT", "%y"),
        ("CURRENT_MONTH", "%m"),
        ("CURRENT_MONTH_NAME", "%B"),
        ("CURRENT_MONTH_NAME_SHORT", "%b"),
        ("CURRENT_DATE", "%d"),
        ("CURRENT_DAY_NAME", "%A"),
        ("CURRENT_DAY_NAME_SHORT", "%a"),
        ("CURRENT_HOUR", "%H"),
        ("CURRENT_MINUTE", "%M"),
        ("CURRENT_SECOND", "%S"),
    ];
    formats
        .into_iter()
        .map(|(name, format)| (name.to_string(), now.format(format).to_string()))
        .chain([(
            "CURRENT_SECONDS_UNIX".to_string(),
            now.timestamp().to_string(),
        )])
        .collect()
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
    /// exists, so the document snapshot is taken before `ide_edits` sees an edit. The
    /// tab's completion source stays for the life of the tab; it offers the language's
    /// snippets while language tools are off.
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
        let blur = cx.subscribe_in(editor, window, |view, editor, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Blur) {
                view.completion_blurred(editor, cx);
            }
        });
        let moved = cx.observe(editor, |view, editor, cx| {
            view.completion_moved(&editor, cx)
        });
        let triggers = Rc::new(RefCell::new(Vec::new()));
        let source = Rc::new(complete::CompletionSource {
            workspace: cx.entity().downgrade(),
            editor: editor.downgrade(),
            language,
            triggers: triggers.clone(),
        });
        editor.update(cx, |editor, _| {
            editor.lsp_mut().completion_provider = Some(source);
        });
        self.ide.tabs.insert(
            editor.entity_id(),
            TabIde {
                editor: editor.downgrade(),
                language,
                unit,
                own_edits: 0,
                triggers,
                _subscriptions: vec![edits, blur, moved],
            },
        );
    }

    /// Whether the IDE layer must leave the buffers alone: a project is opening or
    /// Claude is editing the files.
    fn ide_refused(&self) -> bool {
        self.loading || self.takeover_running()
    }

    /// The active tab when its editor is on screen (no diff, settings closed). `manager`
    /// is `self.language_manager()`.
    fn ide_target(&self) -> Option<IdeTarget> {
        if self.diff.is_some() || self.settings.open {
            return None;
        }
        let tab = self.tabs.get(self.active?)?;
        let ide = self.ide.tabs.get(&tab.editor.entity_id())?;
        let project = self.project.as_ref()?;
        Some(IdeTarget {
            editor: tab.editor.clone(),
            manager: self.language_manager().cloned(),
            file: project.root.join(&tab.document.path),
            relative: tab.document.path.clone(),
            instance: tab.instance.clone(),
            language: ide.language,
            unit: ide.unit,
        })
    }

    /// Snippet variables for an insertion at `offset` with `selection` text: file and line
    /// variables from the target, CLIPBOARD from `cx.read_from_clipboard()`, dates from
    /// `chrono::Local::now()`, UUID from `tandem_core::new_id()`, comment tokens from
    /// `tandem_core::edit::comment::comment_tokens(relative)`.
    fn snippet_variables(
        &self,
        target: &IdeTarget,
        text: &str,
        offset: usize,
        selection: &str,
        cx: &App,
    ) -> HashMap<String, String> {
        let mut variables = document_variables(
            &target.file,
            &target.relative,
            target.language,
            text,
            offset,
            selection,
        );
        variables.extend(date_variables(&chrono::Local::now()));
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        variables.insert("CLIPBOARD".into(), clipboard.unwrap_or_default());
        variables.insert("UUID".into(), new_id());
        variables
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

    /// Handles a key while the editor container has focus; true when consumed. The
    /// popups are asked in order: the completion list first.
    pub(in crate::workspace) fn ide_key(
        &mut self,
        key: IdeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let focused = self
            .ide_target()
            .map(|target| target.editor)
            .filter(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
            .map(|editor| editor.entity_id());
        self.completion_key(key, focused, window, cx)
    }

    /// The IDE layer of `editor`, the active tab's editor on screen: its key listeners and
    /// the popups drawn over it.
    pub(in crate::workspace) fn ide_layer(
        &self,
        editor: &Entity<EditorState>,
        face: &Face,
        cx: &mut Context<Self>,
    ) -> IdeLayer {
        let list = self
            .ide
            .completion
            .as_ref()
            .filter(|list| list.editor_id == editor.entity_id() && list.shown());
        let overlays = list
            .and_then(|list| popup::completion_list(list, editor, face, cx))
            .into_iter()
            .collect();
        IdeLayer {
            workspace: cx.entity().downgrade(),
            editor: editor.entity_id(),
            focus: editor.read(cx).focus_handle(cx),
            keys: self.ide.keys.clone(),
            overlays,
        }
    }

    /// Follows a user edit: corrects a single keystroke (see `typing_correction`) as its
    /// own undo step, then keeps the completion list in step with the text. Edits made by
    /// `ide_replace` are skipped.
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
        if let Some(fix) = self.typing_correction(editor, event, window, cx) {
            self.ide_replace(editor, &fix, window, cx);
        }
        let deleted = single_edit(event.edits())
            .is_some_and(|(start, old_end, inserted)| inserted.is_empty() && old_end > start);
        self.completion_edited(editor, deleted, window, cx);
    }

    /// The correction of a single keystroke in the focused editor: the indentation after
    /// Enter, a closing bracket alone on its line, a Python block keyword's colon, the
    /// close tag after an open tag's `>` and the name after `</`.
    fn typing_correction(
        &self,
        editor: &Entity<EditorState>,
        event: &InputEditsEvent,
        window: &Window,
        cx: &App,
    ) -> Option<Edit> {
        let tab = self.ide.tabs.get(&editor.entity_id())?;
        let ((start, _, inserted), (_, after)) =
            single_edit(event.edits()).zip(event.edits().last())?;
        if self.ide_refused() || !editor.read(cx).focus_handle(cx).is_focused(window) {
            return None;
        }
        // The correction is computed on the text after this edit, so it applies only while
        // the editor still shows that text with the cursor right after the inserted text.
        let text = after.to_string();
        let cursor = start + inserted.len();
        let state = editor.read(cx);
        if *state.text() != text || state.selected_range() != (cursor..cursor) {
            return None;
        }
        typing_fix(&text, start, &inserted, tab.language, tab.unit)
    }

    /// Forgets the tabs whose editor is gone, closes a completion list whose editor is
    /// gone, and refreshes the active tab's completion trigger characters from its
    /// language server (none while language tools are off).
    pub(in crate::workspace) fn ide_tick(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ide
            .tabs
            .retain(|_, tab| tab.editor.upgrade().is_some());
        if self
            .ide
            .completion
            .as_ref()
            .is_some_and(|list| list.editor.upgrade().is_none())
        {
            self.close_completion(cx);
        }
        let triggers = self
            .language_manager()
            .map(|manager| manager.capabilities().completion_triggers())
            .unwrap_or_default();
        let active = self.active.and_then(|index| self.tabs.get(index));
        if let Some(tab) = active.and_then(|tab| self.ide.tabs.get(&tab.editor.entity_id()))
            && *tab.triggers.borrow() != triggers
        {
            *tab.triggers.borrow_mut() = triggers;
        }
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{IdeKey, KeyClaim, date_variables, document_variables, single_edit, typing_fix};
    use chrono::TimeZone as _;
    use gpui_kit::EntityId;
    use gpui_kit::component::input::{InputEdit, Point as TextPoint, Rope};
    use std::path::Path;
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

    #[test]
    fn document_variables_describe_the_file_and_the_cursor() {
        let text = "fn main() {\n    let total = 1;\n}\n";
        let offset = text.find("total").unwrap() + 3;
        let file = Path::new("/project/src/lib.rs");
        let variables = document_variables(file, "src/lib.rs", "rust", text, offset, "x");
        let expected = [
            ("TM_SELECTED_TEXT", "x"),
            ("TM_CURRENT_LINE", "    let total = 1;"),
            ("TM_CURRENT_WORD", "total"),
            ("TM_LINE_INDEX", "1"),
            ("TM_LINE_NUMBER", "2"),
            ("TM_FILENAME", "lib.rs"),
            ("TM_FILENAME_BASE", "lib"),
            ("TM_DIRECTORY", "/project/src"),
            ("TM_FILEPATH", "/project/src/lib.rs"),
            ("RELATIVE_FILEPATH", "src/lib.rs"),
            ("LINE_COMMENT", "//"),
            ("BLOCK_COMMENT_START", "/*"),
            ("BLOCK_COMMENT_END", "*/"),
        ];
        for (name, value) in expected {
            assert_eq!(
                variables.get(name).map(String::as_str),
                Some(value),
                "{name}"
            );
        }
    }

    #[test]
    fn document_variables_at_the_ends_of_the_text() {
        let file = Path::new("/project/app.py");
        let start = document_variables(file, "app.py", "python", "value = 1", 0, "");
        assert_eq!(start["TM_CURRENT_WORD"], "value");
        assert_eq!(start["TM_LINE_NUMBER"], "1");
        assert_eq!(start["LINE_COMMENT"], "#");
        assert_eq!(start["BLOCK_COMMENT_START"], "");
        let end = document_variables(file, "app.py", "python", "a\nb_c\n", 7, "");
        assert_eq!(end["TM_CURRENT_LINE"], "");
        assert_eq!(end["TM_CURRENT_WORD"], "");
        assert_eq!(end["TM_LINE_INDEX"], "2");
        let word_end = document_variables(file, "app.py", "python", "a\nb_c\n", 5, "");
        assert_eq!(word_end["TM_CURRENT_WORD"], "b_c");
        assert_eq!(word_end["TM_CURRENT_LINE"], "b_c");
    }

    #[test]
    fn date_variables_spell_out_the_date_and_time() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 3, 5, 7, 8, 9).unwrap();
        let variables = date_variables(&now);
        let expected = [
            ("CURRENT_YEAR", "2026"),
            ("CURRENT_YEAR_SHORT", "26"),
            ("CURRENT_MONTH", "03"),
            ("CURRENT_MONTH_NAME", "March"),
            ("CURRENT_MONTH_NAME_SHORT", "Mar"),
            ("CURRENT_DATE", "05"),
            ("CURRENT_DAY_NAME", "Thursday"),
            ("CURRENT_DAY_NAME_SHORT", "Thu"),
            ("CURRENT_HOUR", "07"),
            ("CURRENT_MINUTE", "08"),
            ("CURRENT_SECOND", "09"),
        ];
        for (name, value) in expected {
            assert_eq!(
                variables.get(name).map(String::as_str),
                Some(value),
                "{name}"
            );
        }
        assert_eq!(
            variables["CURRENT_SECONDS_UNIX"],
            now.timestamp().to_string()
        );
    }

    #[test]
    fn an_open_list_takes_every_key_but_shift_tab_in_its_own_editor() {
        let (ours, other) = (EntityId::from(1), EntityId::from(2));
        let open = KeyClaim { list: Some(ours) };
        for key in [
            IdeKey::Up,
            IdeKey::Down,
            IdeKey::PageUp,
            IdeKey::PageDown,
            IdeKey::Enter,
            IdeKey::Tab,
            IdeKey::Escape,
        ] {
            assert!(open.takes(ours, key), "{key:?}");
            assert!(!open.takes(other, key), "{key:?}");
        }
        assert!(!open.takes(ours, IdeKey::ShiftTab));
        assert!(!open.elsewhere(ours));
        assert!(open.elsewhere(other));
    }

    #[test]
    fn nothing_open_takes_no_key() {
        let closed = KeyClaim::default();
        assert!(!closed.takes(EntityId::from(1), IdeKey::Enter));
        assert!(!closed.elsewhere(EntityId::from(1)));
    }
}
