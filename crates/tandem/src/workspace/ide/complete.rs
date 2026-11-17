//! Tandem's completion list: when it opens, what it offers, how the keys move through it
//! and what accepting an entry does. The toolkit's own list never opens: the tab's
//! [`CompletionSource`] answers the toolkit with nothing and asks the workspace instead.
use super::*;
use gpui_kit::component::input::{CompletionProvider, Rope};
use lsp_types::{CompletionContext, CompletionItem, CompletionResponse, CompletionTriggerKind};
use std::ops::Range;
use std::time::Duration;
use tandem_core::edit::snippet::library::{self, SnippetDef};
use tandem_core::edit::syntax::likely_in_string_or_comment;
use tandem_lsp::features::completion::{Candidate, Ranked, Source};

/// The most entries a list ranks.
const RANK_LIMIT: usize = 200;
/// The rows the list shows at once.
pub(super) const VISIBLE_ROWS: usize = 12;
/// How far PageUp and PageDown move the selection.
const PAGE_ROWS: isize = 12;
/// The height of a row, which is also one row of wheel movement.
pub(super) const ROW_HEIGHT: f32 = 22.;
/// How long the selection rests on a server entry before its details are asked for.
const DETAILS_DELAY: Duration = Duration::from_millis(120);
/// How long accepting a server entry waits for its details (the import it adds) at most.
const ACCEPT_WAIT: Duration = Duration::from_millis(800);
/// The client command of an entry that asks for the list again at the new cursor.
const TRIGGER_SUGGEST: &str = "editor.action.triggerSuggest";

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

/// What a completion list offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListKind {
    /// The language server's entries and the snippets for the word at the cursor.
    Words,
    /// The options of the active snippet stop.
    Choices,
    /// Every snippet of the language, from Insert Snippet….
    Snippets,
}

/// The completion list of one editor: on screen once it has rows, or waiting for the
/// language server's reply.
pub(in crate::workspace) struct CompletionState {
    pub(super) editor_id: EntityId,
    pub(super) editor: WeakEntity<EditorState>,
    pub(super) kind: ListKind,
    /// The selection a list of choices or snippets opened with, and its text. The list
    /// stays as it opened while the editor keeps that selection, and a snippet chosen
    /// from Insert Snippet… gets that text as `TM_SELECTED_TEXT`.
    held: Option<(Range<usize>, String)>,
    /// Where the word being completed starts; the list hangs below it.
    pub(super) word_start: usize,
    /// The cursor when the entries were asked for.
    request_cursor: usize,
    /// The language server's entries.
    items: Vec<CompletionItem>,
    /// Server entries whose details came since the list opened, by their index in `items`.
    resolved: HashMap<usize, CompletionItem>,
    /// The entry whose details are asked for, and that request; dropping it cancels it.
    resolving: Option<(usize, Task<()>)>,
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
            kind: ListKind::Words,
            held: None,
            word_start,
            request_cursor: cursor,
            items: Vec::new(),
            resolved: HashMap::new(),
            resolving: None,
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

    /// The server entry at `index` in `items`, with its details once they came.
    pub(super) fn item(&self, index: usize) -> Option<&CompletionItem> {
        self.resolved.get(&index).or_else(|| self.items.get(index))
    }

    /// The index in `items` of the selected row's entry, when it is the server's.
    fn selected_item(&self) -> Option<usize> {
        let row = self.ranked.get(self.selected)?;
        match self.candidates.get(row.candidate)?.source {
            Source::Server(index) => Some(index),
            Source::Snippet(_) | Source::Choice(_) => None,
        }
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
    /// asks again, except a word character typed inside a string or a comment, which
    /// opens nothing there.
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
        // A list asked for again because it was incomplete goes on wherever it opened.
        if !retrigger
            && !should_auto_request(
                invoked,
                typed,
                word_char,
                likely_in_string_or_comment(&text, cursor, target.language),
            )
        {
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

    /// Opens a list of `candidates` hanging below `start` in `editor`, every one of them in
    /// their own order (an empty query): the options of a snippet stop, or every snippet
    /// from Insert Snippet…. The list stays as it opened while the editor keeps the
    /// selection it has now; after that, the open-list rule applies from `start`.
    pub(super) fn open_fixed_list(
        &mut self,
        editor: &Entity<EditorState>,
        kind: ListKind,
        start: usize,
        candidates: Vec<Candidate>,
        snippets: Vec<SnippetDef>,
        cx: &mut Context<Self>,
    ) {
        let (text, cursor, selection, typed) = {
            let state = editor.read(cx);
            let cursor = state.cursor();
            (
                state.text().to_string(),
                cursor,
                state.selected_range(),
                typed_since(state.text(), start, cursor).unwrap_or_default(),
            )
        };
        // Nothing ranks when `start` is not a position in the text.
        let ranked = completion::rank(&text, start, &candidates, RANK_LIMIT);
        if ranked.is_empty() {
            self.close_completion(cx);
            return;
        }
        self.ide.lists += 1;
        let mut list = CompletionState::waiting(editor.downgrade(), start, start);
        list.kind = kind;
        let selected = text.get(selection.clone()).unwrap_or_default().to_string();
        list.held = Some((selection, selected));
        list.ranked_for = (cursor, typed);
        list.snippets = snippets;
        list.candidates = candidates;
        list.ranked = ranked;
        list.serial = self.ide.lists;
        self.ide.completion = Some(list);
        self.completion_changed(cx);
    }

    /// Closes the list of `editor` when it offers the options of a snippet stop.
    pub(super) fn close_choices(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        if self
            .ide
            .completion
            .as_ref()
            .is_some_and(|list| list.editor_id == editor && list.kind == ListKind::Choices)
        {
            self.close_completion(cx);
        }
    }

    /// The open-list rule, after an edit or a cursor move: the list closes when the cursor
    /// is before where its candidates start, or the text from there to the cursor holds
    /// whitespace or a line break; otherwise it is ranked again for that text, so
    /// characters the word rules leave out (`-`, `@`, `:` inside an entry's range) refine
    /// it instead of closing it. A list that still waits for its reply is only closed or
    /// kept, and a list of choices or snippets stays as it opened while the editor keeps
    /// the selection it opened with. False when the list closed.
    fn refresh_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(list) = self.ide.completion.as_mut() else {
            return false;
        };
        let Some(editor) = list.editor.upgrade() else {
            self.close_completion(cx);
            return false;
        };
        let state = editor.read(cx);
        if list
            .held
            .as_ref()
            .is_some_and(|held| selection_holds(held, state.text(), &state.selected_range()))
        {
            return true;
        }
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

    /// Tells the key listeners whether the list is on screen, asks for the selected
    /// entry's details, and redraws.
    fn completion_changed(&mut self, cx: &mut Context<Self>) {
        self.ide.sync_keys();
        self.resolve_selected(cx);
        cx.notify();
    }

    /// Asks for the details of the selected server entry once the selection has rested on
    /// it for a moment, when it came without documentation and the server resolves
    /// entries. A newer selection cancels the older request.
    fn resolve_selected(&mut self, cx: &mut Context<Self>) {
        let resolves = self
            .ide_target()
            .and_then(|target| target.manager)
            .is_some_and(|manager| resolves_entries(&manager));
        let Some(list) = self.ide.completion.as_mut() else {
            return;
        };
        let wanted = list.selected_item().filter(|&index| {
            selection_wants_details(
                &list.items[index],
                list.resolved.contains_key(&index),
                resolves,
            )
        });
        let Some(index) = wanted else {
            list.resolving = None;
            return;
        };
        if list
            .resolving
            .as_ref()
            .is_some_and(|(resolving, _)| *resolving == index)
        {
            return;
        }
        let serial = list.serial;
        let task = cx.spawn(async move |view, cx| {
            smol::Timer::after(DETAILS_DELAY).await;
            let request = view.update(cx, |view, cx| view.resolve_listed(serial, index, cx));
            let Ok(Some(request)) = request else {
                return;
            };
            let reply = request.await;
            view.update(cx, |view, cx| {
                view.completion_resolved(serial, index, reply, cx)
            })
            .ok();
        });
        list.resolving = Some((index, task));
    }

    /// The request for the details of entry `index` of the list numbered `serial`, while
    /// that list is open.
    fn resolve_listed(
        &mut self,
        serial: u64,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<CompletionItem>>> {
        let list = self
            .ide
            .completion
            .as_ref()
            .filter(|list| list.serial == serial)?;
        let (editor, item) = (list.editor_id, list.items.get(index)?.clone());
        self.resolve_completion(editor, &item, cx)
    }

    /// Keeps the details of entry `index` for the list numbered `serial`, so the
    /// documentation beside the list shows them and accepting the entry uses them. A
    /// failed request leaves the entry as it was, to be asked for again.
    fn completion_resolved(
        &mut self,
        serial: u64,
        index: usize,
        reply: Result<CompletionItem>,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = self
            .ide
            .completion
            .as_mut()
            .filter(|list| list.serial == serial)
        else {
            return;
        };
        // This runs inside the request's own task; dropping its handle lets it finish.
        if list
            .resolving
            .as_ref()
            .is_some_and(|(resolving, _)| *resolving == index)
        {
            list.resolving = None;
        }
        if let Ok(item) = reply {
            list.resolved.insert(index, item);
            cx.notify();
        }
    }

    /// Asks the language server of the active tab for the details of `item` listed in
    /// `editor` (its documentation, and the edits it adds, such as an import); `None` when
    /// that tab is not on screen or its server does not resolve entries. The details
    /// describe the entry rather than the text, so they hold after an edit made while
    /// they were asked for.
    fn resolve_completion(
        &mut self,
        editor: EntityId,
        item: &CompletionItem,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<CompletionItem>>> {
        let target = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)?;
        let manager = target.manager.filter(resolves_entries)?;
        let params = serde_json::to_value(item).ok()?;
        self.sync_language(cx);
        let text = target.editor.read(cx).text().to_string();
        let (file, instance) = (target.file, target.instance);
        Some(cx.background_spawn(async move {
            let reply = manager
                .request("completionItem/resolve", &file, &instance, &text, params)
                .await?;
            Ok(serde_json::from_value(reply.value)?)
        }))
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
        self.resolve_selected(cx);
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
    /// snippet through `accept_snippet`, a snippet stop's option into that stop. A server
    /// entry that may still lack the edits it adds (an import) waits up to `ACCEPT_WAIT`
    /// for its details when the server resolves entries, and goes in as it is when they do
    /// not come in time. Accepting again replaces an entry still waiting.
    pub(super) fn accept_completion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = self.ide.completion.take() else {
            return;
        };
        self.ide.accepting = None;
        self.completion_changed(cx);
        let Some(source) = list
            .ranked
            .get(index)
            .and_then(|row| list.candidates.get(row.candidate))
            .map(|candidate| candidate.source)
        else {
            return;
        };
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == list.editor_id)
        else {
            return;
        };
        let (insertion, wait) = match (list.kind, source) {
            (_, Source::Choice(choice)) => {
                self.accept_choice(list.editor_id, choice, window, cx);
                return;
            }
            (ListKind::Snippets, Source::Snippet(snippet)) => {
                let selected = list.held.as_ref().map_or("", |(_, text)| text.as_str());
                let body = &list.snippets[snippet].body;
                self.insert_chosen_snippet(
                    list.editor_id,
                    list.word_start,
                    selected,
                    body,
                    window,
                    cx,
                );
                return;
            }
            (_, Source::Server(index)) => {
                let cached = list.resolved.get(&index);
                let item = resolved_item(cached, &list.items[index]);
                let resolves = target.manager.as_ref().is_some_and(resolves_entries);
                let wait = accept_waits_for_details(&item, cached.is_some(), resolves);
                (Insertion::Item(Box::new(item)), wait)
            }
            (_, Source::Snippet(snippet)) => (
                Insertion::Snippet(list.snippets[snippet].body.clone()),
                false,
            ),
        };
        let start = list.start();
        let at = {
            let state = target.editor.read(cx);
            AcceptedAt {
                start,
                request_cursor: list.request_cursor,
                cursor: state.cursor(),
                prefix: text_before(state.text(), start),
            }
        };
        let mut accepted = Accepted {
            editor: list.editor_id,
            word_start: list.word_start,
            at,
            insertion,
        };
        let details = match &accepted.insertion {
            Insertion::Item(item) if wait => self.resolve_completion(accepted.editor, item, cx),
            _ => None,
        };
        let Some(details) = details else {
            self.insert_accepted(accepted, window, cx);
            return;
        };
        self.ide.accepting = Some(cx.spawn_in(window, async move |view, cx| {
            let timeout = async {
                smol::Timer::after(ACCEPT_WAIT).await;
                None
            };
            // On a timeout the request is dropped, which cancels it.
            let resolved = smol::future::or(async { details.await.ok() }, timeout).await;
            if let (Some(resolved), Insertion::Item(item)) = (resolved, &mut accepted.insertion) {
                **item = resolved;
            }
            view.update_in(cx, |view, window, cx| {
                view.insert_accepted(accepted, window, cx)
            })
            .ok();
        }));
    }

    /// Puts an accepted entry into its editor, planned with the editor's text and cursor
    /// at this moment. Nothing happens when that editor is no longer the active tab's on
    /// screen or the accepted word is gone (see `AcceptedAt::still_applies`). The edit is
    /// one undo step the toolkit does not see as typing; the selection then goes where
    /// the entry says, a snippet's tab stops start, and an entry that asks for the list
    /// again gets it at the new cursor.
    fn insert_accepted(&mut self, accepted: Accepted, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == accepted.editor)
        else {
            return;
        };
        let (text, cursor, selection) = {
            let state = target.editor.read(cx);
            if !accepted.at.still_applies(state.text(), state.cursor()) {
                return;
            }
            let text = state.text().to_string();
            let selection = text
                .get(state.selected_range())
                .unwrap_or_default()
                .to_string();
            (text, state.cursor(), selection)
        };
        // A list opened while the entry waited is about the word the entry replaces.
        if self
            .ide
            .completion
            .as_ref()
            .is_some_and(|list| list.editor_id == accepted.editor)
        {
            self.close_completion(cx);
        }
        let variables = self.snippet_variables(&target, &text, cursor, &selection, cx);
        let plan = match &accepted.insertion {
            Insertion::Item(item) => completion::accept_item(
                &text,
                cursor,
                accepted.at.request_cursor,
                accepted.word_start,
                item,
                &variables,
                target.unit,
            ),
            Insertion::Snippet(body) => Ok(completion::accept_snippet(
                &text,
                cursor,
                accepted.word_start,
                body,
                &variables,
                target.unit,
            )),
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
        let command = plan.command.clone();
        if !self.apply_accepted(&target.editor, plan, window, cx) {
            return;
        }
        if command.is_some_and(|command| command.command == TRIGGER_SUGGEST) {
            self.suggest_again(accepted.editor, window, cx);
        }
    }

    /// Opens the list of `editor` at its cursor as ⌃Space does, once the edit just made
    /// there has been handled, as the toolkit's own requests are.
    fn suggest_again(&mut self, editor: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |view, cx| {
            view.update_in(cx, |view, window, cx| {
                view.completion_requested(editor, None, true, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// The snippets of `language`: the user's first, then the built-in ones they leave. A
    /// snippet file that cannot be read is reported once in the status bar.
    pub(super) fn completion_snippets(
        &mut self,
        language: &str,
        cx: &mut Context<Self>,
    ) -> Vec<SnippetDef> {
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

/// An entry accepted from the list, on its way into the editor.
struct Accepted {
    editor: EntityId,
    word_start: usize,
    at: AcceptedAt,
    insertion: Insertion,
}

/// What an accepted entry inserts.
enum Insertion {
    /// A server entry, with its details when they came.
    Item(Box<CompletionItem>),
    /// A snippet's body.
    Snippet(String),
}

/// Where an entry was accepted, to tell whether it still goes in once its details came.
struct AcceptedAt {
    /// Where the text the list matched starts.
    start: usize,
    /// The cursor when the list was asked for.
    request_cursor: usize,
    /// The cursor when the entry was accepted.
    cursor: usize,
    /// The text before `start` then.
    prefix: String,
}

impl AcceptedAt {
    /// Whether the accepted entry still goes into `text` with the cursor at `cursor`: the
    /// cursor is not before where it was both when the list was asked for and when the
    /// entry was accepted (a deletion made before accepting is the entry's to replace,
    /// one made while it waited is not), the text before the list's start is unchanged,
    /// and the text from there to the cursor is still one word, as for an open list.
    fn still_applies(&self, text: &Rope, cursor: usize) -> bool {
        cursor >= self.request_cursor.min(self.cursor)
            && typed_since(text, self.start, cursor).is_some()
            && text.slice(0..self.start) == self.prefix
    }
}

/// The text of `text` before `offset`, or nothing when `offset` is not a position in it.
fn text_before(text: &Rope, offset: usize) -> String {
    if offset <= text.len() && text.is_char_boundary(offset) {
        text.slice(0..offset).to_string()
    } else {
        String::new()
    }
}

/// Whether `manager` is ready and resolves completion entries.
fn resolves_entries(manager: &LanguageManager) -> bool {
    manager.health().state == "ready" && manager.capabilities().completion_resolve()
}

/// Whether a completion request goes out: always when the user asked for the list or
/// `typed` a character that is not a word character (a trigger character); for a typed
/// word character, or nothing typed, only outside strings and comments.
fn should_auto_request(
    invoked: bool,
    typed: Option<char>,
    word_char: bool,
    in_string_or_comment: bool,
) -> bool {
    invoked || (typed.is_some() && !word_char) || !in_string_or_comment
}

/// The entry to accept: its details when they came (`cached`), else the entry as listed.
fn resolved_item(cached: Option<&CompletionItem>, original: &CompletionItem) -> CompletionItem {
    cached.unwrap_or(original).clone()
}

/// Whether accepting `item` waits for its details: the server resolves entries, they have
/// not come yet (`cached`), and the item adds no edits of its own so far, so an import it
/// needs may still be missing.
fn accept_waits_for_details(item: &CompletionItem, cached: bool, resolves: bool) -> bool {
    resolves
        && !cached
        && item
            .additional_text_edits
            .as_ref()
            .is_none_or(Vec::is_empty)
}

/// Whether selecting `item` asks for its details: the server resolves entries, they have
/// not come yet (`cached`), and the item has no documentation to show.
fn selection_wants_details(item: &CompletionItem, cached: bool, resolves: bool) -> bool {
    resolves && !cached && item.documentation.is_none()
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

/// Whether the editor still has the selection a list opened with (`held`: its range and
/// text), with the same text in it.
fn selection_holds(held: &(Range<usize>, String), text: &Rope, selection: &Range<usize>) -> bool {
    let (range, selected) = held;
    range == selection
        && range.end <= text.len()
        && text.is_char_boundary(range.start)
        && text.is_char_boundary(range.end)
        && text.slice(range.clone()) == selected.as_str()
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
    use super::{
        AcceptedAt, accept_waits_for_details, move_selection, opens_list, resolved_item,
        scrolled_top, selection_holds, selection_wants_details, should_auto_request, typed_since,
        visible_top,
    };
    use gpui_kit::component::input::Rope;
    use lsp_types::{CompletionItem, Documentation, Position, Range, TextEdit};

    #[test]
    fn typing_asks_for_the_list_unless_in_a_string_or_comment() {
        // (invoked, typed, word_char, in_string_or_comment) → asks
        let table = [
            ((false, Some('a'), true, false), true),
            ((false, Some('a'), true, true), false),
            ((true, Some('a'), true, true), true),
            ((true, None, false, true), true),
            ((true, None, false, false), true),
            // Trigger characters always ask, even inside a string (`"./` in an import).
            ((false, Some('.'), false, true), true),
            ((false, Some('/'), false, false), true),
            ((false, None, false, true), false),
            ((false, None, false, false), true),
        ];
        for ((invoked, typed, word_char, in_string), asks) in table {
            assert_eq!(
                should_auto_request(invoked, typed, word_char, in_string),
                asks,
                "invoked {invoked}, typed {typed:?}, word {word_char}, string {in_string}"
            );
        }
    }

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            ..CompletionItem::default()
        }
    }

    fn import_edit() -> TextEdit {
        TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            new_text: "use std::collections::HashMap;\n".into(),
        }
    }

    #[test]
    fn the_resolved_item_is_preferred_over_the_original() {
        let original = item("HashMap");
        let mut resolved = item("HashMap");
        resolved.additional_text_edits = Some(vec![import_edit()]);
        assert_eq!(resolved_item(Some(&resolved), &original), resolved);
        assert_eq!(resolved_item(None, &original), original);
    }

    #[test]
    fn accepting_waits_for_details_only_when_the_import_may_be_missing() {
        let plain = item("HashMap");
        assert!(accept_waits_for_details(&plain, false, true));
        // Already resolved, or a server that does not resolve: nothing to wait for.
        assert!(!accept_waits_for_details(&plain, true, true));
        assert!(!accept_waits_for_details(&plain, false, false));
        let mut with_import = item("HashMap");
        with_import.additional_text_edits = Some(vec![import_edit()]);
        assert!(!accept_waits_for_details(&with_import, false, true));
        let mut empty = item("HashMap");
        empty.additional_text_edits = Some(Vec::new());
        assert!(accept_waits_for_details(&empty, false, true));
    }

    #[test]
    fn a_selected_item_without_documentation_asks_for_it_once() {
        let plain = item("total_area");
        assert!(selection_wants_details(&plain, false, true));
        assert!(!selection_wants_details(&plain, true, true));
        assert!(!selection_wants_details(&plain, false, false));
        let mut documented = item("total_area");
        documented.documentation = Some(Documentation::String("Adds up the areas.".into()));
        assert!(!selection_wants_details(&documented, false, true));
    }

    /// What was accepted in `marked` (`[` the list's start, `|` the cursor when it was
    /// asked for and accepted).
    fn accepted(marked: &str) -> (AcceptedAt, String) {
        let start = marked.find('[').expect("a start marker");
        let text = marked.replace('[', "");
        let cursor = text.find('|').expect("a cursor marker");
        let text = text.replace('|', "");
        let at = AcceptedAt {
            start,
            request_cursor: cursor,
            cursor,
            prefix: text[..start].to_string(),
        };
        (at, text)
    }

    #[test]
    fn an_accepted_entry_goes_in_while_its_word_is_still_there() {
        let (at, text) = accepted("let map = [HashMa|\n");
        let cursor = text.find('\n').unwrap();
        assert!(at.still_applies(&Rope::from_str(&text), cursor));
        // Typed on during the wait.
        let typed = text.replace("HashMa", "HashMap");
        assert!(at.still_applies(&Rope::from_str(&typed), cursor + 1));
    }

    #[test]
    fn an_accepted_entry_is_dropped_once_its_word_is_gone() {
        let (at, text) = accepted("let map = [HashMa|\n");
        let cursor = text.find('\n').unwrap();
        // Deleted back past the cursor of the request.
        let deleted = text.replace("HashMa", "Hash");
        assert!(!at.still_applies(&Rope::from_str(&deleted), cursor - 2));
        // Moved on past a space.
        let spaced = text.replace("HashMa", "HashMa x");
        assert!(!at.still_applies(&Rope::from_str(&spaced), cursor + 2));
        // The text before the word changed, the word itself did not move.
        let edited = text.replace("map", "mop");
        assert!(!at.still_applies(&Rope::from_str(&edited), cursor));
        // The cursor went before the word.
        assert!(!at.still_applies(&Rope::from_str(&text), 2));
    }

    #[test]
    fn a_deletion_made_before_accepting_still_goes_in() {
        // Asked for at `HashMa|`, then one character deleted before accepting.
        let (mut at, text) = accepted("let map = [HashMa|\n");
        let text = text.replace("HashMa", "HashM");
        at.cursor -= 1;
        assert!(at.still_applies(&Rope::from_str(&text), at.cursor));
        assert!(!at.still_applies(&Rope::from_str(&text), at.cursor - 1));
    }

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
    fn a_held_selection_holds_while_the_editor_keeps_it_unchanged() {
        let text = Rope::from_str("let x = a + b;\n");
        let held = (8..13, "a + b".to_string());
        assert!(selection_holds(&held, &text, &(8..13)));
        // The selection moved or shrank.
        assert!(!selection_holds(&held, &text, &(13..13)));
        assert!(!selection_holds(&held, &text, &(8..12)));
        // Same range, other text in it.
        let edited = Rope::from_str("let x = a - b;\n");
        assert!(!selection_holds(&held, &edited, &(8..13)));
        // The text became shorter than the range.
        assert!(!selection_holds(&held, &Rope::from_str("let"), &(8..13)));
        // An empty selection holds while the cursor stays.
        let cursor = (4..4, String::new());
        assert!(selection_holds(&cursor, &text, &(4..4)));
        assert!(!selection_holds(&cursor, &text, &(5..5)));
    }

    #[test]
    fn the_typed_text_keeps_to_character_boundaries() {
        let text = Rope::from_str("é€x");
        assert_eq!(typed_since(&text, 0, 6).as_deref(), Some("é€x"));
        assert_eq!(typed_since(&text, 1, 6), None);
    }
}
