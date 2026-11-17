//! Tandem's completion list: when it opens, what it offers, how the keys move through it
//! and what accepting an entry does. The toolkit's own list never opens: the tab's
//! [`CompletionSource`] answers the toolkit with nothing and asks the workspace instead.
use super::*;
use gpui_kit::component::input::{CompletionProvider, Rope};
use lsp_types::{CompletionContext, CompletionItem, CompletionResponse, CompletionTriggerKind};
use tandem_core::edit::snippet::library::{self, SnippetDef};
use tandem_lsp::features::completion::{Candidate, Ranked, Source};

/// The most entries a list ranks.
const RANK_LIMIT: usize = 200;
/// The rows the list shows at once.
pub(super) const VISIBLE_ROWS: usize = 12;
/// How far PageUp and PageDown move the selection.
const PAGE_ROWS: isize = 12;
/// The height of a row, which is also one row of wheel movement.
pub(super) const ROW_HEIGHT: f32 = 22.;

/// Answers the toolkit's completion requests for one tab's editor, for the life of the
/// tab: with nothing, so the toolkit's own list never opens, while it asks the workspace
/// to open Tandem's list instead.
pub(in crate::workspace) struct CompletionSource {
    pub workspace: WeakEntity<Workspace>,
    pub editor: WeakEntity<EditorState>,
    pub language: &'static str,
    /// The language server's trigger characters, kept current by `ide_tick`.
    pub triggers: Rc<RefCell<Vec<String>>>,
}

impl CompletionProvider for CompletionSource {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let typed = (offset <= text.len() && text.is_char_boundary(offset))
            .then(|| text.chars_at(offset).reversed().next())
            .flatten();
        let invoked = trigger.trigger_kind == CompletionTriggerKind::INVOKED;
        let (workspace, editor) = (self.workspace.clone(), self.editor.entity_id());
        // The toolkit calls this while it updates the editor, sometimes from inside a
        // workspace update, so the workspace is asked once those updates have ended.
        window
            .spawn(cx, async move |cx| {
                workspace
                    .update_in(cx, |view, window, cx| {
                        view.completion_requested(editor, typed, invoked, window, cx)
                    })
                    .ok();
            })
            .detach();
        Task::ready(Ok(CompletionResponse::Array(Vec::new())))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        opens_list(new_text, self.language, &self.triggers.borrow())
    }
}

/// The completion list of one editor: on screen once it has rows, or waiting for the
/// language server's reply.
pub(in crate::workspace) struct CompletionState {
    pub(super) editor_id: EntityId,
    pub(super) editor: WeakEntity<EditorState>,
    /// Where the word being completed starts; the list hangs below it.
    pub(super) word_start: usize,
    /// The cursor when the entries were asked for.
    request_cursor: usize,
    /// The language server's entries.
    pub(super) items: Vec<CompletionItem>,
    snippets: Vec<SnippetDef>,
    /// Everything the list can offer, from the server's entries and the snippets.
    pub(super) candidates: Vec<Candidate>,
    /// The candidates that match the typed text, best first: the rows of the list.
    pub(super) ranked: Vec<Ranked>,
    /// The selected row.
    pub(super) selected: usize,
    /// The first row on screen.
    pub(super) top: usize,
    /// Whether the server said its entries may change as the user types on.
    incomplete: bool,
    /// The request in flight; dropping it cancels it.
    request: Option<Task<()>>,
    /// The cursor and the typed text the candidates were last ranked for.
    ranked_for: (usize, String),
    /// Wheel movement that has not made up a whole row yet.
    wheel: f32,
    /// Tells this list's documentation apart from an earlier list's.
    pub(super) serial: u64,
}

impl CompletionState {
    /// A list that waits for its first reply.
    fn waiting(editor: WeakEntity<EditorState>, word_start: usize, cursor: usize) -> Self {
        Self {
            editor_id: editor.entity_id(),
            editor,
            word_start,
            request_cursor: cursor,
            items: Vec::new(),
            snippets: Vec::new(),
            candidates: Vec::new(),
            ranked: Vec::new(),
            selected: 0,
            top: 0,
            incomplete: false,
            request: None,
            ranked_for: (cursor, String::new()),
            wheel: 0.,
            serial: 0,
        }
    }

    /// Whether the list is on screen.
    pub(super) fn shown(&self) -> bool {
        !self.ranked.is_empty()
    }

    /// Where the text the candidates are matched against starts: the word, or earlier
    /// where a server entry's own range starts.
    fn start(&self) -> usize {
        list_start(&self.candidates, self.word_start)
    }
}

/// What a list is asked for: the editor's text and cursor at that moment, and how.
struct Asked {
    editor: WeakEntity<EditorState>,
    text: String,
    cursor: usize,
    word_start: usize,
    /// The character typed right before, when typing asked.
    typed: Option<char>,
    /// Whether the user asked for the list (⌃Space) instead of typing.
    invoked: bool,
    /// Whether the open list is asked again because the server said it was incomplete.
    retrigger: bool,
}

impl Workspace {
    /// Opens or refines the completion list of `editor` after the toolkit asked for
    /// completions: typing `typed` asked, or the user did (`invoked`). A word character
    /// typed into a complete list for the same word only ranks it again; anything else
    /// asks again.
    pub(in crate::workspace) fn completion_requested(
        &mut self,
        editor: EntityId,
        typed: Option<char>,
        invoked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)
        else {
            return;
        };
        if self.ide_refused() {
            return;
        }
        let (text, cursor) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        let word_start = completion::word_start(&text, cursor, target.language);
        let word_char = typed.is_some_and(|typed| completion::is_word_char(typed, target.language));
        let open = self.ide.completion.as_ref().filter(|list| {
            list.editor_id == editor && list.word_start == word_start && list.request.is_none()
        });
        let (refine, retrigger) = match open {
            Some(list) if word_char && !invoked => (!list.incomplete, list.incomplete),
            _ => (false, false),
        };
        if refine {
            self.refresh_completion(cx);
            return;
        }
        let asked = Asked {
            editor: target.editor.downgrade(),
            text,
            cursor,
            word_start,
            typed,
            invoked,
            retrigger,
        };
        self.request_completion(target, asked, window, cx);
    }

    /// Asks for the list: the language server's entries plus the snippets when the server
    /// completes, else the snippets alone at once.
    fn request_completion(
        &mut self,
        target: IdeTarget,
        asked: Asked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_language(cx);
        let snippets = self.completion_snippets(target.language, cx);
        let server = target.manager.filter(|manager| {
            manager.health().state == "ready" && manager.capabilities().has("completionProvider")
        });
        let Some(manager) = server else {
            self.open_completion(asked, Vec::new(), false, snippets, cx);
            return;
        };
        let editor = asked.editor.entity_id();
        let triggers = self
            .ide
            .tabs
            .get(&editor)
            .map(|tab| tab.triggers.borrow().clone())
            .unwrap_or_default();
        let params = completion::completion_params(
            &asked.text,
            asked.cursor,
            asked.typed,
            &triggers,
            asked.retrigger,
        );
        let Ok(params) = params else {
            self.close_completion(cx);
            return;
        };
        let reply = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, asked.text.clone());
            async move {
                let reply = manager
                    .request("textDocument/completion", &file, &instance, &text, params)
                    .await?;
                anyhow::ensure!(manager.is_current(&reply.stamp), "Stale completion");
                completion::completion_items(reply.value)
            }
        });
        let (word_start, cursor, weak) = (asked.word_start, asked.cursor, asked.editor.clone());
        let request = cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update(cx, |view, cx| {
                view.completion_replied(asked, snippets, reply, cx)
            })
            .ok();
        });
        // The list on screen stays, ranked as the user types, until the reply replaces it,
        // as long as it is for the same word.
        let mut list = match self.ide.completion.take() {
            Some(list) if list.editor_id == editor && list.word_start == word_start => list,
            _ => CompletionState::waiting(weak, word_start, cursor),
        };
        list.request = Some(request);
        self.ide.completion = Some(list);
        self.completion_changed(cx);
    }

    /// Opens the list with the server's reply while it is still about the editor's text
    /// and cursor. A failed request offers the snippets alone.
    fn completion_replied(
        &mut self,
        asked: Asked,
        snippets: Vec<SnippetDef>,
        reply: Result<(Vec<CompletionItem>, bool)>,
        cx: &mut Context<Self>,
    ) {
        let editor = asked.editor.entity_id();
        let Some(list) = self
            .ide
            .completion
            .as_mut()
            .filter(|list| list.editor_id == editor)
        else {
            return;
        };
        // This runs inside the request's own task; dropping its handle lets it finish.
        list.request = None;
        let current = asked.editor.upgrade().is_some_and(|editor| {
            let state = editor.read(cx);
            state.cursor() == asked.cursor && *state.text() == asked.text
        });
        if current {
            let (items, incomplete) = reply.unwrap_or_default();
            self.open_completion(asked, items, incomplete, snippets, cx);
        } else if list.shown() {
            self.completion_changed(cx);
        } else {
            self.close_completion(cx);
        }
    }

    /// Shows the candidates that match the typed text, best first, with the first one
    /// selected (`rank` puts a preselected entry first among the best scores), or closes
    /// the list when none matches. Snippets join once something is typed, or when the
    /// user asked for the list.
    fn open_completion(
        &mut self,
        asked: Asked,
        items: Vec<CompletionItem>,
        incomplete: bool,
        snippets: Vec<SnippetDef>,
        cx: &mut Context<Self>,
    ) {
        let Asked {
            editor,
            text,
            cursor,
            word_start,
            invoked,
            ..
        } = asked;
        let mut candidates = completion::server_candidates(&text, cursor, word_start, &items);
        if word_start < cursor || invoked {
            candidates.extend(completion::snippet_candidates(word_start, &snippets));
        }
        let ranked = completion::rank(&text, cursor, &candidates, RANK_LIMIT);
        if ranked.is_empty() {
            self.close_completion(cx);
            return;
        }
        self.ide.lists += 1;
        let mut list = CompletionState::waiting(editor, word_start, cursor);
        list.ranked_for = (
            cursor,
            text.get(list_start(&candidates, word_start)..cursor)
                .unwrap_or_default()
                .to_string(),
        );
        list.items = items;
        list.snippets = snippets;
        list.candidates = candidates;
        list.ranked = ranked;
        list.incomplete = incomplete;
        list.serial = self.ide.lists;
        self.ide.completion = Some(list);
        self.completion_changed(cx);
    }

    /// The open-list rule, after an edit or a cursor move: the list closes when the cursor
    /// is before where its candidates start, or the text from there to the cursor holds
    /// whitespace or a line break; otherwise it is ranked again for that text, so
    /// characters the word rules leave out (`-`, `@`, `:` inside an entry's range) refine
    /// it instead of closing it. A list that still waits for its reply is only closed or
    /// kept. False when the list closed.
    fn refresh_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(list) = self.ide.completion.as_mut() else {
            return false;
        };
        let Some(editor) = list.editor.upgrade() else {
            self.close_completion(cx);
            return false;
        };
        let state = editor.read(cx);
        let cursor = state.cursor();
        let Some(typed) = typed_since(state.text(), list.start(), cursor) else {
            self.close_completion(cx);
            return false;
        };
        if list.candidates.is_empty() || (list.ranked_for.0 == cursor && list.ranked_for.1 == typed)
        {
            return true;
        }
        let text = state.text().slice(0..cursor).to_string();
        let ranked = completion::rank(&text, cursor, &list.candidates, RANK_LIMIT);
        list.ranked_for = (cursor, typed);
        if ranked.is_empty() && list.request.is_none() {
            self.close_completion(cx);
            return false;
        }
        list.ranked = ranked;
        list.selected = 0;
        list.top = 0;
        self.completion_changed(cx);
        true
    }

    /// Keeps the list of `editor` in step after a user edit there; a deletion in an
    /// incomplete list asks the server again.
    pub(super) fn completion_edited(
        &mut self,
        editor: &Entity<EditorState>,
        deleted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ours = |list: &CompletionState| list.editor_id == editor.entity_id();
        if !self.ide.completion.as_ref().is_some_and(ours) || !self.refresh_completion(cx) {
            return;
        }
        let again = deleted
            && self
                .ide
                .completion
                .as_ref()
                .is_some_and(|list| list.incomplete && list.request.is_none());
        let Some(target) = self
            .ide_target()
            .filter(|target| again && target.editor == *editor)
        else {
            return;
        };
        let (text, cursor) = {
            let state = editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        let asked = Asked {
            editor: editor.downgrade(),
            word_start: completion::word_start(&text, cursor, target.language),
            text,
            cursor,
            typed: None,
            invoked: false,
            retrigger: true,
        };
        self.request_completion(target, asked, window, cx);
    }

    /// Keeps the list of `editor` in step after the editor changed, so moving the cursor
    /// ranks it again or closes it.
    pub(super) fn completion_moved(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        if self
            .ide
            .completion
            .as_ref()
            .is_some_and(|list| list.editor_id == editor.entity_id())
        {
            self.refresh_completion(cx);
        }
    }

    /// Closes the list of `editor` when the editor loses the focus.
    pub(super) fn completion_blurred(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        if self
            .ide
            .completion
            .as_ref()
            .is_some_and(|list| list.editor_id == editor.entity_id())
        {
            self.close_completion(cx);
        }
    }

    /// Closes the list and cancels its request.
    pub(super) fn close_completion(&mut self, cx: &mut Context<Self>) {
        if self.ide.completion.take().is_some() {
            self.completion_changed(cx);
        }
    }

    /// Tells the key listeners whether the list is on screen, and redraws.
    fn completion_changed(&mut self, cx: &mut Context<Self>) {
        self.ide.sync_keys();
        cx.notify();
    }

    /// Handles a key for the completion list of the `focused` editor: Up and Down move the
    /// selection around the ends, PageUp and PageDown by a page, Enter and Tab accept,
    /// Escape closes. True when the list took the key. A list left open in another editor
    /// is closed and takes nothing.
    pub(super) fn completion_key(
        &mut self,
        key: IdeKey,
        focused: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(list) = self.ide.completion.as_mut() else {
            return false;
        };
        if Some(list.editor_id) != focused {
            self.close_completion(cx);
            return false;
        }
        let len = list.ranked.len();
        if len == 0 {
            return false;
        }
        let selected = match key {
            IdeKey::Up => move_selection(list.selected, len, -1, true),
            IdeKey::Down => move_selection(list.selected, len, 1, true),
            IdeKey::PageUp => move_selection(list.selected, len, -PAGE_ROWS, false),
            IdeKey::PageDown => move_selection(list.selected, len, PAGE_ROWS, false),
            IdeKey::Enter | IdeKey::Tab => {
                let selected = list.selected;
                self.accept_completion(selected, window, cx);
                return true;
            }
            IdeKey::Escape => {
                self.close_completion(cx);
                return true;
            }
            IdeKey::ShiftTab => return false,
        };
        list.selected = selected;
        list.top = visible_top(selected, list.top, len, VISIBLE_ROWS);
        cx.notify();
        true
    }

    /// Moves the rows on screen by the wheel's movement `dy` (toward the top when positive).
    pub(super) fn scroll_completion(&mut self, dy: Pixels, cx: &mut Context<Self>) {
        let Some(list) = self.ide.completion.as_mut() else {
            return;
        };
        list.wheel -= f32::from(dy) / ROW_HEIGHT;
        let rows = list.wheel.trunc();
        list.wheel -= rows;
        let top = scrolled_top(list.top, list.ranked.len(), VISIBLE_ROWS, rows as isize);
        if top != list.top {
            list.top = top;
            cx.notify();
        }
    }

    /// Accepts the row at `index`: a server entry through `completion::accept_item`, a
    /// snippet through `accept_snippet`. The edit is one undo step the toolkit does not
    /// see as typing; the selection then goes where the entry says.
    pub(super) fn accept_completion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = self.ide.completion.take() else {
            return;
        };
        self.completion_changed(cx);
        let Some(candidate) = list
            .ranked
            .get(index)
            .and_then(|row| list.candidates.get(row.candidate))
        else {
            return;
        };
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == list.editor_id)
        else {
            return;
        };
        let (text, cursor, selection) = {
            let state = target.editor.read(cx);
            let text = state.text().to_string();
            let selection = text
                .get(state.selected_range())
                .unwrap_or_default()
                .to_string();
            (text, state.cursor(), selection)
        };
        let variables = self.snippet_variables(&target, &text, cursor, &selection, cx);
        let plan = match candidate.source {
            Source::Server(item) => completion::accept_item(
                &text,
                cursor,
                list.request_cursor,
                list.word_start,
                &list.items[item],
                &variables,
                target.unit,
            ),
            Source::Snippet(snippet) => Ok(completion::accept_snippet(
                &text,
                cursor,
                list.word_start,
                &list.snippets[snippet].body,
                &variables,
                target.unit,
            )),
            // A snippet's choices are offered only while its session runs.
            Source::Choice(_) => return,
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.message(
                    format!("Could not insert the completion: {error:#}"),
                    true,
                    cx,
                );
                return;
            }
        };
        if plan.edit.changes_text() && !self.ide_replace(&target.editor, &plan.edit, window, cx) {
            return;
        }
        target.editor.update(cx, |editor, cx| {
            editor.set_selected_range(plan.selection.clone(), cx)
        });
    }

    /// The snippets of `language`: the user's first, then the built-in ones they leave. A
    /// snippet file that cannot be read is reported once in the status bar.
    fn completion_snippets(&mut self, language: &str, cx: &mut Context<Self>) -> Vec<SnippetDef> {
        let store = &self.store;
        let user = self.ide.snippets.get_or_insert_with(|| {
            UserSnippets::new(store.lock().unwrap().directory.join("snippets"))
        });
        let (user, error) = user.for_language(language);
        if let Some(error) = error
            && self.ide.snippet_errors.insert(error.clone())
        {
            self.message(format!("Could not read your snippets: {error}"), true, cx);
        }
        library::merged(library::builtin(language), user)
    }
}

/// Where the text the `candidates` are matched against starts: `word_start`, or earlier
/// where a candidate's own range starts.
fn list_start(candidates: &[Candidate], word_start: usize) -> usize {
    candidates
        .iter()
        .map(|candidate| candidate.start)
        .fold(word_start, usize::min)
}

/// The selection after moving it by `delta` rows in a list of `len` entries: around the
/// ends when `wrap`, else stopping at them.
fn move_selection(selected: usize, len: usize, delta: isize, wrap: bool) -> usize {
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    let moved = selected.min(last) as isize + delta;
    if wrap {
        moved.rem_euclid(len as isize) as usize
    } else {
        moved.clamp(0, last as isize) as usize
    }
}

/// The first row on screen once `selected` is shown, moving `top` as little as possible,
/// with `rows` rows on screen and no empty rows below the last entry.
fn visible_top(selected: usize, top: usize, len: usize, rows: usize) -> usize {
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    let selected = selected.min(last);
    let top = if selected < top {
        selected
    } else if selected >= top + rows {
        selected + 1 - rows
    } else {
        top
    };
    top.min(len.saturating_sub(rows))
}

/// The first row on screen after the wheel moved the list by `by` rows.
fn scrolled_top(top: usize, len: usize, rows: usize, by: isize) -> usize {
    let last_top = len.saturating_sub(rows) as isize;
    (top as isize + by).clamp(0, last_top) as usize
}

/// Whether typing `typed` asks for completions: one word character of `language`, or one
/// of the server's trigger characters.
fn opens_list(typed: &str, language: &str, triggers: &[String]) -> bool {
    let mut chars = typed.chars();
    match (chars.next(), chars.next()) {
        (Some(typed_char), None) => {
            completion::is_word_char(typed_char, language)
                || triggers.iter().any(|trigger| trigger == typed)
        }
        _ => false,
    }
}

/// The text from `start` to the cursor while the list stays open for it: `None` when the
/// cursor is before `start` or that text holds whitespace or a line break.
fn typed_since(text: &Rope, start: usize, cursor: usize) -> Option<String> {
    if start > cursor
        || cursor > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(cursor)
    {
        return None;
    }
    let typed = text.slice(start..cursor).to_string();
    (!typed.chars().any(char::is_whitespace)).then_some(typed)
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{move_selection, opens_list, scrolled_top, typed_since, visible_top};
    use gpui_kit::component::input::Rope;

    #[test]
    fn up_and_down_wrap_around_the_ends() {
        assert_eq!(move_selection(2, 5, 1, true), 3);
        assert_eq!(move_selection(2, 5, -1, true), 1);
        assert_eq!(move_selection(4, 5, 1, true), 0);
        assert_eq!(move_selection(0, 5, -1, true), 4);
        assert_eq!(move_selection(0, 1, 1, true), 0);
    }

    #[test]
    fn paging_stops_at_the_ends() {
        assert_eq!(move_selection(3, 40, 12, false), 15);
        assert_eq!(move_selection(30, 40, 12, false), 39);
        assert_eq!(move_selection(15, 40, -12, false), 3);
        assert_eq!(move_selection(5, 40, -12, false), 0);
        assert_eq!(move_selection(39, 40, 12, false), 39);
    }

    #[test]
    fn an_empty_list_keeps_the_selection_at_zero() {
        assert_eq!(move_selection(0, 0, 1, true), 0);
        assert_eq!(move_selection(3, 0, -12, false), 0);
        assert_eq!(visible_top(0, 4, 0, 12), 0);
        assert_eq!(scrolled_top(4, 0, 12, 3), 0);
    }

    #[test]
    fn a_selection_past_a_shorter_list_comes_back_to_its_end() {
        assert_eq!(move_selection(9, 5, 0, false), 4);
        assert_eq!(move_selection(9, 5, 1, true), 0);
    }

    #[test]
    fn the_window_follows_the_selection() {
        // Inside the window: nothing moves.
        assert_eq!(visible_top(5, 0, 40, 12), 0);
        assert_eq!(visible_top(11, 0, 40, 12), 0);
        // Below it: the selection becomes the last row.
        assert_eq!(visible_top(12, 0, 40, 12), 1);
        assert_eq!(visible_top(39, 0, 40, 12), 28);
        // Above it: the selection becomes the first row.
        assert_eq!(visible_top(3, 10, 40, 12), 3);
        // Wrapping from the last entry to the first.
        assert_eq!(visible_top(0, 28, 40, 12), 0);
    }

    #[test]
    fn the_window_never_shows_empty_rows_below_the_list() {
        assert_eq!(visible_top(3, 2, 5, 12), 0);
        assert_eq!(visible_top(14, 10, 15, 12), 3);
    }

    #[test]
    fn the_wheel_moves_the_window_within_the_list() {
        assert_eq!(scrolled_top(0, 40, 12, 3), 3);
        assert_eq!(scrolled_top(3, 40, 12, -2), 1);
        assert_eq!(scrolled_top(1, 40, 12, -5), 0);
        assert_eq!(scrolled_top(25, 40, 12, 10), 28);
        assert_eq!(scrolled_top(0, 5, 12, 3), 0);
    }

    #[test]
    fn word_characters_and_trigger_characters_open_the_list() {
        let triggers = [".".to_string(), "::".to_string()];
        assert!(opens_list("a", "rust", &triggers));
        assert!(opens_list("_", "python", &[]));
        assert!(opens_list(".", "rust", &triggers));
        assert!(opens_list("-", "css", &[]));
        assert!(opens_list("$", "typescript", &[]));
        assert!(!opens_list("-", "rust", &triggers));
        assert!(!opens_list(" ", "rust", &triggers));
        assert!(!opens_list("", "rust", &triggers));
        // Pastes and multi-character triggers are not one typed character.
        assert!(!opens_list("ab", "rust", &triggers));
        assert!(!opens_list("::", "rust", &triggers));
    }

    #[test]
    fn the_list_stays_open_while_the_typed_text_has_no_whitespace() {
        let text = Rope::from_str("let x = background-c\nnext");
        assert_eq!(typed_since(&text, 8, 20).as_deref(), Some("background-c"));
        assert_eq!(typed_since(&text, 8, 8).as_deref(), Some(""));
        assert_eq!(typed_since(&text, 8, 5), None);
        assert_eq!(typed_since(&text, 4, 8), None);
        assert_eq!(typed_since(&text, 8, 22), None);
        assert_eq!(typed_since(&text, 8, 99), None);
    }

    #[test]
    fn the_typed_text_keeps_to_character_boundaries() {
        let text = Rope::from_str("é€x");
        assert_eq!(typed_since(&text, 0, 6).as_deref(), Some("é€x"));
        assert_eq!(typed_since(&text, 1, 6), None);
    }
}
