mod chat;
mod chat_logic;
mod chat_view;
mod code_view;
pub mod commands;
// The terminal, chat and explorer fonts are read from Tasks 5, 6 and 8 on, the
// substitutions from Task 9 on; Task 9 removes this attribute.
#[allow(dead_code)]
mod fonts;
mod hover;
mod look;
mod pane_logic;
mod panels;
mod panes;
mod provider;
mod settings;
mod settings_look;
mod tools;
mod view;
use chat::ChatUi;
use chat_logic::{BottomTab, line_count_label, next_bottom};
use look::LookUi;
use panes::PaneUi;
pub use panes::SwapPanes;
use settings::SettingsUi;
pub use settings::runtime_status;
use tools::Development;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::component::input::{EditorState, InputEditsEvent, InputEvent, InputState, Position};
use gpui_kit::*;
use notify::Watcher as _;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tandem_claude::Command;
use tandem_core::*;
use tandem_terminal::{Terminal, TerminalView};

actions!(
    tandem,
    [
        Quit,
        OpenProject,
        ShowRecent,
        Save,
        SaveAll,
        SaveCopy,
        QuickOpen,
        ProjectSearch,
        FindInFile,
        ReplaceInFile,
        NewFile,
        CloseFile,
        ToggleTerminal,
        Changes,
        ZoomIn,
        ZoomOut,
        FormatFile,
        References,
        Symbols,
        Settings,
        ToggleChat,
        FocusChat,
        NewTask,
        StopClaude,
        ShowProblems,
        CheckMyWork
    ]
);

/// Claude edits the files on disk during a takeover, so the editor writes none of them.
const TAKEOVER_SAVE_PAUSED: &str = "Claude is editing files; saving is paused.";

/// Menu/shortcut commands must work even when the old editor focus handle is
/// temporarily absent from the view tree (for example while showing a diff).
pub fn install_actions(cx: &mut App, window: AnyWindowHandle, view: WeakEntity<Workspace>) {
    fn route<A: Action>(
        cx: &mut App,
        window: AnyWindowHandle,
        view: WeakEntity<Workspace>,
        handler: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        cx.on_action(move |_: &A, cx| {
            let view = view.clone();
            // Dispatch runs while GPUI has borrowed the window. Defer the
            // update until that borrow is released instead of losing the action.
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| handler(view, window, cx))
                });
            });
        });
    }
    route::<Settings>(cx, window, view.clone(), |v, _, cx| v.toggle_settings(cx));
    route::<OpenProject>(cx, window, view.clone(), |v, w, c| v.choose_project(w, c));
    route::<ShowRecent>(cx, window, view.clone(), |v, _, c| {
        v.show_recent = !v.show_recent;
        c.notify();
    });
    route::<Save>(cx, window, view.clone(), |v, _, c| v.save_active(c));
    route::<SaveAll>(cx, window, view.clone(), |v, _, c| v.save_all(c));
    route::<SaveCopy>(cx, window, view.clone(), |v, w, c| v.save_copy(w, c));
    route::<QuickOpen>(cx, window, view.clone(), |v, w, c| {
        v.set_sidebar(Sidebar::Files, w, c)
    });
    route::<ProjectSearch>(cx, window, view.clone(), |v, w, c| {
        v.set_sidebar(Sidebar::Search, w, c)
    });
    route::<FindInFile>(cx, window, view.clone(), |v, w, c| {
        v.find_in_file(false, w, c)
    });
    route::<ReplaceInFile>(cx, window, view.clone(), |v, w, c| {
        v.find_in_file(true, w, c)
    });
    route::<NewFile>(cx, window, view.clone(), |v, w, c| {
        v.set_sidebar(Sidebar::New, w, c)
    });
    route::<CloseFile>(cx, window, view.clone(), |v, w, c| {
        v.close_file(false, w, c)
    });
    route::<Changes>(cx, window, view.clone(), |v, w, c| v.show_diff(w, c));
    route::<ToggleTerminal>(cx, window, view.clone(), |v, _, c| v.toggle_terminal(c));
    route::<ZoomIn>(cx, window, view.clone(), |v, _, c| v.zoom(1.0, c));
    route::<ZoomOut>(cx, window, view.clone(), |v, _, c| v.zoom(-1.0, c));
    route::<ToggleChat>(cx, window, view.clone(), |v, _, c| {
        v.chat.visible = !v.chat.visible;
        c.notify();
    });
    route::<SwapPanes>(cx, window, view.clone(), |v, _, c| v.swap_panes(c));
    route::<FocusChat>(cx, window, view.clone(), |v, w, c| v.focus_chat(w, c));
    route::<FormatFile>(cx, window, view.clone(), |v, w, c| {
        v.language_action("textDocument/formatting", w, c)
    });
    route::<References>(cx, window, view.clone(), |v, w, c| {
        v.language_action("textDocument/references", w, c)
    });
    route::<Symbols>(cx, window, view.clone(), |v, w, c| {
        v.language_action("textDocument/documentSymbol", w, c)
    });
    route::<NewTask>(cx, window, view.clone(), |v, _, _| {
        v.chat_command(Command::NewTask)
    });
    route::<StopClaude>(cx, window, view.clone(), |v, _, _| {
        v.chat_command(Command::Stop)
    });
    route::<CheckMyWork>(cx, window, view.clone(), |v, _, _| {
        v.chat_command(Command::CheckMyWork)
    });
    route::<ShowProblems>(cx, window, view, |v, _, c| {
        v.bottom = Some(BottomTab::Problems);
        c.notify();
    });
}

struct Tab {
    instance: String,
    language_bound: bool,
    document: Document,
    editor: Entity<EditorState>,
    saving: bool,
    _changes: Subscription,
}
#[derive(Clone, Copy, PartialEq)]
enum Sidebar {
    Files,
    Search,
    New,
}
struct LoadedProject {
    project: Project,
    index: FileIndex,
    state: WorkspaceState,
    documents: Vec<Document>,
    warnings: Vec<String>,
    tools: ProjectTools,
}

pub struct Workspace {
    store: Arc<Mutex<Store>>,
    project: Option<Project>,
    index: FileIndex,
    tabs: Vec<Tab>,
    active: Option<usize>,
    query: Entity<InputState>,
    sidebar: Sidebar,
    expanded: HashSet<String>,
    search_hits: Vec<SearchHit>,
    search_cancel: Arc<AtomicBool>,
    search_id: u64,
    status: String,
    error: bool,
    recent: Vec<String>,
    show_recent: bool,
    panes: PaneUi,
    look: LookUi,
    look_controls: settings_look::LookControls,
    generation: u64,
    checkpointed: u64,
    checkpoint_inflight: bool,
    reconcile_inflight: bool,
    last_reconcile: Instant,
    last_index: Instant,
    epoch: u64,
    loading: bool,
    copy_inflight: bool,
    disk_events: Arc<AtomicBool>,
    source_generation: Arc<AtomicU64>,
    _watcher: Option<notify::RecommendedWatcher>,
    _subscriptions: Vec<Subscription>,
    terminal: Option<Entity<TerminalView>>,
    /// The visible tab of the panel below the editor; `None` hides the panel.
    bottom: Option<BottomTab>,
    diff: Option<Entity<EditorState>>,
    close_pending: bool,
    hover: Entity<hover::HoverTip>,
    dev: Development,
    chat: ChatUi,
    settings: SettingsUi,
    commands: commands::CommandsUi,
}

impl Workspace {
    pub fn new(
        store: Arc<Mutex<Store>>,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let appearance = crate::appearance::install(window, cx);
        window.set_window_title("Tandem");
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find a file · ⌘P"));
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if this.sidebar == Sidebar::Search {
                        this.search_id += 1;
                        this.search_cancel.store(true, Ordering::Relaxed);
                        this.search_hits.clear();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.submit_query(window, cx),
                _ => {}
            },
        );
        let (recent, sequence) = {
            let store = store.lock().unwrap();
            (
                store.recent().unwrap_or_default(),
                store.sequence().unwrap_or(0),
            )
        };
        let panes = PaneUi::load(&store.lock().unwrap());
        let installed = cx.text_system().all_font_names();
        let look = LookUi::load(&store.lock().unwrap(), installed);
        let look_controls = settings_look::LookControls::new(&look.prefs, window, cx);
        let mut view = Self {
            chat: ChatUi::new(window, cx),
            settings: SettingsUi::default(),
            commands: commands::CommandsUi::default(),
            dev: Development::new(window, cx),
            store,
            project: None,
            index: FileIndex::default(),
            tabs: Vec::new(),
            active: None,
            query,
            sidebar: Sidebar::Files,
            expanded: HashSet::new(),
            search_hits: Vec::new(),
            search_cancel: Arc::new(AtomicBool::new(false)),
            search_id: 0,
            status: "Open a folder to start. Editing works without AI.".into(),
            error: false,
            recent,
            show_recent: false,
            panes,
            look,
            look_controls,
            generation: sequence,
            checkpointed: sequence,
            checkpoint_inflight: false,
            reconcile_inflight: false,
            last_reconcile: Instant::now(),
            last_index: Instant::now(),
            epoch: 0,
            loading: false,
            copy_inflight: false,
            disk_events: Arc::new(AtomicBool::new(false)),
            source_generation: Arc::new(AtomicU64::new(0)),
            _watcher: None,
            _subscriptions: vec![appearance, subscription],
            terminal: None,
            bottom: None,
            diff: None,
            close_pending: false,
            hover: cx.new(hover::HoverTip::new),
        };
        view.apply_window_look(window);
        view.apply_fonts(cx);
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                loop {
                    smol::Timer::after(Duration::from_millis(500)).await;
                    if weak
                        .update_in(cx, |view, window, cx| {
                            view.checkpoint_async(cx);
                            view.development_tick(window, cx);
                            view.chat_tick(window, cx);
                            view.pane_tick(cx);
                            view.persist_look(cx);
                            let rescan = view.disk_events.swap(false, Ordering::Relaxed)
                                || view.last_index.elapsed() > Duration::from_secs(30);
                            if !view.loading
                                && (rescan
                                    || view.last_reconcile.elapsed() > Duration::from_secs(2))
                            {
                                view.reconcile(rescan, window, cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        if let Some(path) = initial {
            view.open_project(path, window, cx);
        }
        view
    }

    fn message(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.error = error;
        cx.notify();
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.saturating_add(1);
        self.hover.update(cx, |tip, cx| tip.hide(cx));
        cx.notify();
    }
    fn snapshot_live(&mut self, cx: &App) {
        let mut changed = false;
        for tab in &mut self.tabs {
            let editor = tab.editor.read(cx);
            if *editor.text() != tab.document.text {
                tab.document.snapshot(editor.text().to_string());
                changed = true;
            }
        }
        // A synchronous flush may see an edit before its queued event. Give
        // that snapshot a fresh sequence so an older background write loses.
        if changed {
            self.generation = self.generation.saturating_add(1);
            if let Some(manager) = &self.dev.language {
                manager.invalidate();
            }
            if let Some(manager) = &self.dev.python_language {
                manager.invalidate();
            }
        }
    }
    fn state(&self) -> Option<WorkspaceState> {
        Some(WorkspaceState {
            root: self.project.as_ref()?.key().into(),
            tabs: self.tabs.iter().map(|t| t.document.path.clone()).collect(),
            active: self
                .active
                .and_then(|i| self.tabs.get(i))
                .map(|t| t.document.path.clone()),
            font_size: self.look.prefs.fonts.editor.size,
            sequence: self.generation,
        })
    }
    fn flush(&mut self, cx: &App) -> Result<()> {
        self.snapshot_live(cx);
        if let Some(state) = self.state() {
            self.store
                .lock()
                .map_err(|_| anyhow!("Recovery store unavailable"))?
                .checkpoint(
                    &state,
                    &self
                        .tabs
                        .iter()
                        .map(|t| t.document.clone())
                        .collect::<Vec<_>>(),
                )?;
            self.checkpointed = self.generation;
        }
        Ok(())
    }
    pub fn flush_for_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.chat_turn_running() {
            self.message(
                "Claude is still working. Stop it (⌘.) before closing.",
                true,
                cx,
            );
            return false;
        }
        if self.tabs.iter().any(|t| t.saving) || self.loading || self.copy_inflight {
            self.message(
                "Wait for the current file operation before closing.",
                true,
                cx,
            );
            return false;
        }
        self.persist_layout(cx);
        self.persist_look(cx);
        match self.flush(cx) {
            Ok(()) => true,
            Err(error) => {
                self.message(
                    format!("Could not preserve drafts: {error:#}. The window remains open."),
                    true,
                    cx,
                );
                false
            }
        }
    }
    fn checkpoint_async(&mut self, cx: &mut Context<Self>) {
        if self.checkpoint_inflight || self.checkpointed == self.generation {
            return;
        }
        self.snapshot_live(cx);
        let Some(state) = self.state() else {
            return;
        };
        let documents: Vec<_> = self.tabs.iter().map(|t| t.document.clone()).collect();
        let store = self.store.clone();
        let generation = self.generation;
        self.checkpoint_inflight = true;
        let operation = cx.background_spawn(async move {
            store
                .lock()
                .map_err(|_| anyhow!("Recovery store unavailable"))?
                .checkpoint(&state, &documents)
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                view.checkpoint_inflight = false;
                match result {
                    Ok(()) => view.checkpointed = view.checkpointed.max(generation),
                    Err(error) => view.message(format!("Draft backup failed: {error:#}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_inflight {
            self.message("Finish the file-copy operation first.", false, cx);
            return;
        }
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project folder".into()),
        });
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = selected.await;
                let _ = weak.update_in(cx, |view, window, cx| match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            view.open_project(path, window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => view.message("Could not open the folder picker.", true, cx),
                });
            })
            .detach();
    }

    fn open_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat_turn_running() {
            self.message(
                "Claude is still working. Stop it (⌘.) before switching projects.",
                true,
                cx,
            );
            return;
        }
        if self
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).is_busy())
        {
            self.message(
                "A terminal command is running. Interrupt it before switching projects.",
                true,
                cx,
            );
            return;
        }
        if self.tabs.iter().any(|t| t.saving) || self.copy_inflight {
            self.message(
                "Wait for saving to finish before switching projects.",
                true,
                cx,
            );
            return;
        }
        if let Err(error) = self.flush(cx) {
            self.message(
                format!("Could not preserve current drafts: {error:#}"),
                true,
                cx,
            );
            return;
        }
        self.epoch += 1;
        let epoch = self.epoch;
        self.loading = true;
        // Move input away from the old editor immediately. A late keyboard
        // event must not become a source edit in a workspace being replaced.
        self.query.update(cx, |query, cx| query.focus(window, cx));
        self.search_cancel.store(true, Ordering::Relaxed);
        self.message("Opening project and recovering workspace…", false, cx);
        let store = self.store.clone();
        let operation = cx.background_spawn(async move { load_project(path, store) });
        let weak = cx.entity().downgrade();
        window.spawn(cx, async move |cx| {
            let result = operation.await;
            let _ = weak.update_in(cx, |view, window, cx| {
                if view.epoch != epoch { return; } view.loading = false;
                match result {
                    Err(error) => view.message(format!("Cannot open project: {error:#}"), true, cx),
                    Ok(loaded) => {
                        if view.project.as_ref().is_some_and(|current| current.root == loaded.project.root) {
                            view.index = loaded.index; view.show_recent = false;
                            view.go_to_line(None, window, cx);
                            view.message("Project already open; existing tabs and undo history kept.", false, cx);
                            return;
                        }
                        // Capture any edit that arrived between the initial
                        // checkpoint and the loading-state render.
                        if let Err(error) = view.flush(cx) {
                            view.message(format!("Project switch stopped; drafts could not be preserved: {error:#}"), true, cx);
                            return;
                        }
                        view.stop_language(cx);
                        view.tabs.clear(); view.active = None; view.diff = None; view.close_pending = false;
                        view.terminal = None; view.bottom = None; view._watcher = None;
                        view.project = Some(loaded.project.clone()); view.index = loaded.index; view.show_recent = false;
                        view.dev.reset(loaded.tools, window, cx);
                        view.start_chat(window, cx);
                        let name = loaded.project.root.file_name().map(|name| name.to_string_lossy()).unwrap_or_else(|| "Project".into());
                        window.set_window_title(&format!("{name} — Tandem"));
                        view.expanded.clear(); view.search_hits.clear();
                        view.set_sidebar(Sidebar::Files, window, cx);
                        for document in loaded.documents { view.add_tab(document, window, cx); }
                        view.active = loaded.state.active.and_then(|path| view.tabs.iter().position(|t| t.document.path == path)).or_else(|| (!view.tabs.is_empty()).then_some(0));
                        let signal = view.disk_events.clone(); let source_generation=view.source_generation.clone(); let watched_root = loaded.project.root.clone();
                        if let Ok(mut watcher) = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                            if let Ok(event) = event && event.paths.iter().any(|path| !path.strip_prefix(&watched_root).unwrap_or(path).components().any(|part| matches!(part.as_os_str().to_str(), Some("target" | ".git" | "node_modules" | ".tandem-local")))) { source_generation.fetch_add(1,Ordering::Relaxed); signal.store(true, Ordering::Relaxed); }
                        }) && watcher.watch(&loaded.project.root, notify::RecursiveMode::Recursive).is_ok() { view._watcher = Some(watcher); }
                        view.recent.retain(|root| root != loaded.project.key()); view.recent.insert(0, loaded.project.key().into()); view.recent.truncate(12);
                        let recovered = view.tabs.iter().filter(|t| t.document.recovered).count();
                        let message = if !loaded.warnings.is_empty() { loaded.warnings.join(" · ") } else if recovered > 0 { format!("Recovered {recovered} unsaved draft(s). Project files were not changed.") } else { format!("{} file{} · local editing ready", view.index.files.len(), if view.index.files.len() == 1 { "" } else { "s" }) };
                        view.last_index = Instant::now(); view.go_to_line(None, window, cx); view.changed(cx); view.message(message, !loaded.warnings.is_empty(), cx);
                        view.discover_tools(cx);
                    }
                }
            });
        }).detach();
    }

    fn add_tab(&mut self, document: Document, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.document.path == document.path)
        {
            self.select_tab(index, window, cx);
            return;
        }
        let word_wrap = self.commands.word_wrap;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(document.language())
                .default_value(document.text.clone())
                .soft_wrap(word_wrap)
        });
        let changes = cx.subscribe(&editor, |view, entity, event: &InputEditsEvent, cx| {
            if let Some((_, text)) = event.edits().last()
                && let Some(tab) = view
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.editor.entity_id() == entity.entity_id())
            {
                let text = text.to_string();
                if tab.document.text != text {
                    tab.document.snapshot(text);
                    view.diff = None;
                    view.changed(cx);
                    view.invalidate_language(cx);
                }
            }
        });
        self.tabs.push(Tab {
            instance: new_id(),
            language_bound: false,
            document,
            editor,
            saving: false,
            _changes: changes,
        });
        self.active = Some(self.tabs.len() - 1);
        self.settings.open = false;
        self.diff = None;
        self.close_pending = false;
        self.changed(cx);
        self.sync_language(cx);
    }
    fn open_file(
        &mut self,
        path: String,
        new: bool,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_jump(cx);
        if self.loading {
            self.message(
                "Wait for the selected project to finish opening.",
                false,
                cx,
            );
            return;
        }
        let Some(project) = self.project.clone() else {
            self.message("Open a project folder first.", true, cx);
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| tab.document.path == path) {
            self.select_tab(index, window, cx);
            self.go_to_line(line, window, cx);
            return;
        }
        if self.tabs.len() >= 32 {
            self.message("Close a tab before opening more than 32 files.", true, cx);
            return;
        }
        let epoch = self.epoch;
        let store = self.store.clone();
        let operation = cx.background_spawn(async move {
            validate_new_name(&path)?;
            if let Some(mut draft) = store
                .lock()
                .map_err(|_| anyhow!("Recovery store unavailable"))?
                .drafts(project.key())?
                .into_iter()
                .find(|d| d.path == path)
            {
                validate_import_path(&project, &draft.path)?;
                draft.recovered = true;
                draft.conflict = !(draft.base_hash.is_none()
                    && !project.root.join(&draft.path).exists())
                    && !matches!(project.disk_change(&draft), DiskChange::Unchanged);
                return Ok(draft);
            }
            project.open_document(&path, new)
        });
        let weak = cx.entity().downgrade();
        window.spawn(cx, async move |cx| {
            let result = operation.await;
            let _ = weak.update_in(cx, |view, window, cx| {
                if view.epoch != epoch { return; }
                match result {
                    Ok(document) => { let recovered = document.recovered; view.add_tab(document, window, cx); view.go_to_line(line, window, cx); view.apply_step_reveal(window, cx); if recovered { view.message("Recovered an unsaved draft; inspect disk differences before saving.", false, cx); } }
                    Err(error) => view.message(format!("Cannot open file: {error:#}"), true, cx),
                }
            });
        }).detach();
    }
    fn go_to_line(&self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) {
            tab.editor.update(cx, |editor, cx| {
                if let Some(line) = line {
                    editor.set_cursor_position(
                        Position::new(line.saturating_sub(1) as u32, 0),
                        window,
                        cx,
                    );
                } else {
                    editor.focus(window, cx);
                }
            });
        }
    }
    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.record_jump(cx);
        self.active = Some(index);
        self.settings.open = false;
        self.invalidate_language(cx);
        self.diff = None;
        self.close_pending = false;
        self.go_to_line(None, window, cx);
        self.changed(cx);
    }

    fn save_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.takeover_running() {
            self.message(TAKEOVER_SAVE_PAUSED, true, cx);
            return;
        }
        if self.loading || self.copy_inflight {
            self.message("Wait for the current file operation to finish.", false, cx);
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        self.snapshot_live(cx);
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if tab.saving {
            return;
        }
        tab.saving = true;
        let snapshot = tab.document.clone();
        let id = tab.editor.entity_id();
        let epoch = self.epoch;
        let operation = cx.background_spawn(async move {
            let saved = project.save(&snapshot);
            let conflict = saved.is_err()
                && matches!(
                    project.disk_change(&snapshot),
                    DiskChange::Changed(_) | DiskChange::Missing | DiskChange::Unavailable(_)
                );
            (saved, conflict)
        });
        self.message("Saving…", false, cx);
        cx.spawn(async move |view, cx| {
            let (result, conflict) = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                let Some(tab) = view.tabs.iter_mut().find(|t| t.editor.entity_id() == id) else {
                    return;
                };
                tab.saving = false;
                match result {
                    Ok(saved) => {
                        tab.document.base_text = saved.base_text;
                        tab.document.base_hash = saved.base_hash;
                        tab.document.conflict = false;
                        tab.document.recovered = false;
                        tab.document.read_only = saved.read_only;
                        let name = tab.document.path.clone();
                        let message = if tab.document.dirty() {
                            format!("Saved {name}; newer edits remain unsaved.")
                        } else {
                            format!("Saved {name}")
                        };
                        view.changed(cx);
                        view.message(message, false, cx);
                    }
                    Err(error) => {
                        tab.document.conflict |= conflict;
                        view.message(format!("Save stopped: {error:#}"), true, cx);
                    }
                }
            });
        })
        .detach();
    }
    fn save_active(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.save_index(index, cx);
        }
    }
    fn save_all(&mut self, cx: &mut Context<Self>) {
        if self.takeover_running() {
            self.message(TAKEOVER_SAVE_PAUSED, true, cx);
            return;
        }
        for index in 0..self.tabs.len() {
            if self.tabs[index].document.dirty() {
                self.save_index(index, cx);
            }
        }
    }
    fn save_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.copy_inflight || self.tabs.iter().any(|tab| tab.saving) {
            self.message("Wait for the current file operation to finish.", false, cx);
            return;
        }
        self.snapshot_live(cx);
        let Some(snapshot) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.document.clone())
        else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let name = PathBuf::from(&snapshot.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        let selected = cx.prompt_for_new_path(&project.root, name.as_deref());
        self.copy_inflight = true;
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(path))) = selected.await else {
                    let _ = weak.update(cx, |view, cx| {
                        view.copy_inflight = false;
                        view.message("Copy cancelled; the draft is preserved.", false, cx);
                    });
                    return;
                };
                let result = smol::unblock(move || -> Result<PathBuf> {
                    let copy_project = Project::open(path.parent().context("Copy has no parent")?)?;
                    let name = path
                        .file_name()
                        .and_then(|p| p.to_str())
                        .context("Invalid copy file name")?;
                    let mut copy = copy_project.open_document(name, true)?;
                    copy.format = snapshot.format;
                    copy.snapshot(snapshot.text);
                    copy_project.save(&copy)?;
                    Ok(path)
                })
                .await;
                let _ = weak.update(cx, |view, cx| {
                    view.copy_inflight = false;
                    match result {
                        Ok(path) => view.message(
                            format!(
                                "Saved a copy to {}. Your draft was preserved.",
                                path.display()
                            ),
                            false,
                            cx,
                        ),
                        Err(error) => view.message(format!("Copy not saved: {error:#}"), true, cx),
                    }
                });
            })
            .detach();
    }
    fn close_file(&mut self, keep_draft: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.copy_inflight {
            return;
        }
        let Some(index) = self.active else {
            return;
        };
        if self.tabs[index].saving {
            self.message("Wait for this save to finish.", true, cx);
            return;
        }
        self.snapshot_live(cx);
        if self.tabs[index].document.dirty() && !keep_draft {
            self.close_pending = true;
            cx.notify();
            return;
        }
        if let Err(error) = self.flush(cx) {
            self.message(format!("Could not preserve draft: {error:#}"), true, cx);
            return;
        }
        self.note_closed(self.tabs[index].document.path.clone());
        self.tabs.remove(index);
        self.invalidate_language(cx);
        self.active =
            (!self.tabs.is_empty()).then_some(index.min(self.tabs.len().saturating_sub(1)));
        self.close_pending = false;
        self.diff = None;
        if self.active.is_some() {
            self.go_to_line(None, window, cx);
        } else {
            self.query.update(cx, |query, cx| query.focus(window, cx));
        }
        self.changed(cx);
    }
    fn show_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.is_some() {
            self.diff = None;
            self.go_to_line(None, window, cx);
            cx.notify();
            return;
        }
        // The comparison takes the editor area, where Settings would cover it.
        self.settings.open = false;
        self.snapshot_live(cx);
        let Some(project) = self.project.clone() else {
            return;
        };
        let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) else {
            return;
        };
        let snapshot = tab.document.clone();
        let id = tab.editor.entity_id();
        let revision = snapshot.revision;
        let operation = cx.background_spawn(async move { project.diff(&snapshot) });
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = operation.await;
                let _ = weak.update_in(cx, |view, window, cx| {
                    if view.active.and_then(|i| view.tabs.get(i)).is_none_or(|t| {
                        t.editor.entity_id() != id || t.document.revision != revision
                    }) {
                        return;
                    }
                    match result {
                        Ok(diff) => {
                            let editor = cx.new(|cx| {
                                EditorState::new(window, cx).language("diff").default_value(
                                    if diff.is_empty() {
                                        "No differences from disk.\n".into()
                                    } else {
                                        diff
                                    },
                                )
                            });
                            editor.update(cx, |editor, cx| editor.focus(window, cx));
                            view.diff = Some(editor);
                        }
                        Err(error) => view.message(format!("Cannot compare: {error:#}"), true, cx),
                    }
                    cx.notify();
                });
            })
            .detach();
    }

    fn use_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.flush(cx) {
            self.message(
                format!("Cannot preserve current draft: {error:#}"),
                true,
                cx,
            );
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) else {
            return;
        };
        if tab.saving {
            return;
        }
        let path = tab.document.path.clone();
        let revision = tab.document.revision;
        let id = tab.editor.entity_id();
        let operation = cx.background_spawn(async move { project.open_document(&path, false) });
        let weak = cx.entity().downgrade();
        window.spawn(cx, async move |cx| {
            let result = operation.await;
            let _ = weak.update_in(cx, |view, window, cx| {
                let Some(tab) = view.tabs.iter_mut().find(|t| t.editor.entity_id() == id) else { return; };
                if tab.document.revision != revision { view.message("The buffer changed while reading disk; reload was stopped.", true, cx); return; }
                match result {
                    Ok(mut document) => { document.revision = revision.saturating_add(1); let text = document.text.clone(); tab.document = document;
                        tab.editor.update(cx, |editor, cx| editor.replace_all(text, window, cx)); view.diff = None; view.changed(cx);
                        view.message("Loaded disk contents. Undo restores the prior draft in this session.", false, cx); }
                    Err(error) => view.message(format!("Could not reload disk: {error:#}"), true, cx),
                }
            });
        }).detach();
    }

    fn reconcile(&mut self, rescan: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.reconcile_inflight {
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        self.last_reconcile = Instant::now();
        self.reconcile_inflight = true;
        let epoch = self.epoch;
        let snapshots: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| !t.saving)
            .map(|t| (t.editor.entity_id(), t.document.clone()))
            .collect();
        let operation = cx.background_spawn(async move {
            let changes: Vec<_> = snapshots
                .into_iter()
                .map(|(id, document)| {
                    let change = project.disk_change(&document);
                    (id, document, change)
                })
                .collect();
            (rescan.then(|| project.index()), changes)
        });
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let (index, changes) = operation.await;
                let _ = weak.update_in(cx, |view, window, cx| {
                    view.reconcile_inflight = false;
                    if view.epoch != epoch {
                        return;
                    }
                    let mut changed = false;
                    if let Some(index) = index {
                        changed |= view.index.files != index.files
                            || view.index.truncated != index.truncated;
                        view.index = index;
                        view.last_index = Instant::now();
                        if view.status == "Refreshing project files…" {
                            view.status =
                                format!("Refreshed {} indexed files.", view.index.files.len());
                            view.error = false;
                            changed = true;
                        }
                    }
                    for (id, inspected, change) in changes {
                        let Some(tab) = view
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.editor.entity_id() == id)
                        else {
                            continue;
                        };
                        if tab.saving || tab.document.base_hash != inspected.base_hash {
                            continue;
                        }
                        match change {
                            DiskChange::Unchanged => {
                                changed |= tab.document.conflict;
                                tab.document.conflict = false;
                            }
                            DiskChange::MetadataChanged { read_only } => {
                                changed = true;
                                tab.document.read_only = read_only;
                                tab.document.conflict = false;
                            }
                            DiskChange::Changed(current) if current.text == tab.document.text => {
                                changed = true;
                                tab.document.base_text = current.base_text;
                                tab.document.base_hash = current.base_hash;
                                tab.document.format = current.format;
                                tab.document.conflict = false;
                                tab.document.recovered = false;
                                view.generation = view.generation.saturating_add(1);
                            }
                            DiskChange::Changed(mut current)
                                if !tab.document.dirty()
                                    && tab.document.revision == inspected.revision =>
                            {
                                changed = true;
                                current.revision = tab.document.revision.saturating_add(1);
                                tab.document = current;
                                let text = tab.document.text.clone();
                                tab.editor
                                    .update(cx, |editor, cx| editor.replace_all(text, window, cx));
                                view.generation = view.generation.saturating_add(1);
                            }
                            DiskChange::Changed(_)
                            | DiskChange::Missing
                            | DiskChange::Unavailable(_) => {
                                changed |= !tab.document.conflict;
                                tab.document.conflict = true;
                            }
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
            })
            .detach();
    }
    fn find_in_file(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        if replace {
            self.diff = None;
        }
        let editor = self.diff.clone().or_else(|| {
            self.active
                .and_then(|i| self.tabs.get(i))
                .map(|t| t.editor.clone())
        });
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| {
                editor.focus(window, cx);
                editor.open_search(replace, cx);
            });
        }
        cx.notify();
    }

    fn set_sidebar(&mut self, sidebar: Sidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.commands.explorer_visible = true;
        self.sidebar = sidebar;
        self.query.update(cx, |query, cx| {
            query.set_value("", window, cx);
            query.set_placeholder(
                match sidebar {
                    Sidebar::Files => "Find a file · ⌘P",
                    Sidebar::Search => "Search text · Enter to run",
                    Sidebar::New => "New file: relative/path.rs",
                },
                window,
                cx,
            );
            query.focus(window, cx);
        });
        cx.notify();
    }
    fn submit_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            self.message(
                "Wait for the selected project to finish opening.",
                false,
                cx,
            );
            return;
        }
        let query = self.query.read(cx).text().to_string();
        match self.sidebar {
            Sidebar::New => self.open_file(query.trim().to_string(), true, None, window, cx),
            Sidebar::Files => {
                if let Some(path) = self
                    .index
                    .files
                    .iter()
                    .find(|path| path.to_lowercase().contains(&query.to_lowercase()))
                    .cloned()
                {
                    self.open_file(path, false, None, window, cx);
                } else if !query.trim().is_empty() {
                    // Exact paths can open deliberately selected ignored files;
                    // automatic navigation/search still honors ignore rules.
                    self.open_file(query.trim().into(), false, None, window, cx);
                }
            }
            Sidebar::Search => {
                let Some(project) = self.project.clone() else {
                    return;
                };
                self.search_cancel.store(true, Ordering::Relaxed);
                self.search_cancel = Arc::new(AtomicBool::new(false));
                self.search_id += 1;
                self.snapshot_live(cx);
                let overlays: Vec<_> = self.tabs.iter().map(|tab| tab.document.clone()).collect();
                let cancelled = self.search_cancel.clone();
                let files = self.index.files.clone();
                let epoch = self.epoch;
                let search_id = self.search_id;
                let operation = cx.background_spawn(async move {
                    project.search_with_overlays(&files, &query, &overlays, &cancelled)
                });
                self.message("Searching project text…", false, cx);
                cx.spawn(async move |view, cx| { let hits = operation.await; let _ = view.update(cx, |view, cx| {
                    if view.epoch != epoch || view.search_id != search_id { return; } view.search_hits = hits;
                    view.message(format!("{} matching lines (up to {MAX_SEARCH_RESULTS}); includes captured unsaved buffers.", view.search_hits.len()), false, cx);
                }); }).detach();
            }
        }
    }
    fn toggle_terminal(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.message(
                "Wait for the selected project to finish opening.",
                false,
                cx,
            );
            return;
        }
        if self.terminal.is_none() {
            let Some(project) = &self.project else {
                self.message("Open a project before starting a terminal.", true, cx);
                return;
            };
            match Terminal::new(&project.root) {
                Ok(terminal) => self.terminal = Some(cx.new(|cx| TerminalView::new(terminal, cx))),
                Err(error) => {
                    self.message(format!("Cannot start terminal: {error:#}"), true, cx);
                    return;
                }
            }
        }
        self.bottom = next_bottom(self.bottom, BottomTab::Terminal);
        cx.notify();
    }
    /// ⌘= and ⌘−: the editor's font size, for every project.
    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = &mut self.look.prefs.fonts.editor.size;
        *size = tandem_core::appearance::zoomed(*size, delta);
        self.apply_fonts(cx);
    }
    fn navigation(&self, cx: &App) -> Vec<(String, String, usize, bool)> {
        let query = self.query.read(cx).text().to_string().to_lowercase();
        if !query.is_empty() {
            return self
                .index
                .files
                .iter()
                .filter(|path| path.to_lowercase().contains(&query))
                .take(250)
                .map(|path| (path.clone(), path.clone(), 0, false))
                .collect();
        }
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for path in &self.index.files {
            let parts: Vec<_> = path.split('/').collect();
            let mut prefix = String::new();
            for (depth, part) in parts.iter().enumerate() {
                if depth > 0 {
                    prefix.push('/');
                }
                prefix.push_str(part);
                let directory = depth + 1 < parts.len();
                if seen.insert(prefix.clone()) {
                    rows.push((prefix.clone(), part.to_string(), depth, directory));
                }
                if directory && !self.expanded.contains(&prefix) {
                    break;
                }
            }
            if rows.len() >= 250 {
                break;
            }
        }
        rows
    }
}

fn load_project(path: PathBuf, store: Arc<Mutex<Store>>) -> Result<LoadedProject> {
    let project = Project::open(&path)?;
    let (state, drafts, tools) = {
        let store = store
            .lock()
            .map_err(|_| anyhow!("Recovery store unavailable"))?;
        (
            store
                .workspace(project.key())?
                .unwrap_or_else(|| WorkspaceState::new(&project)),
            store.drafts(project.key())?,
            store.project_tools(project.key())?,
        )
    };
    let mut drafts: HashMap<_, _> = drafts
        .into_iter()
        .map(|document| (document.path.clone(), document))
        .collect();
    let mut paths = state.tabs.clone();
    for path in drafts.keys() {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    let mut documents = Vec::new();
    let mut warnings = Vec::new();
    for path in paths.into_iter().take(32) {
        if let Some(mut draft) = drafts.remove(&path) {
            validate_import_path(&project, &draft.path)?;
            draft.recovered = true;
            draft.conflict = !(draft.base_hash.is_none()
                && !project.root.join(&draft.path).exists())
                && !matches!(project.disk_change(&draft), DiskChange::Unchanged);
            documents.push(draft);
        } else {
            match project.open_document(&path, false) {
                Ok(document) => documents.push(document),
                Err(_) => warnings.push(format!("Could not restore {path}")),
            }
        }
    }
    let index = project.index();
    if index.truncated {
        warnings.push("File listing reached its size limit".into());
    }
    Ok(LoadedProject {
        project,
        index,
        state,
        documents,
        warnings,
        tools,
    })
}
