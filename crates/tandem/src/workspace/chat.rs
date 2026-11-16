//! The chat panel's state in the workspace and its link to the companion controller.
use super::chat_logic::{SavePlan, TabFacts, anchor_line, takeover_save_plan};
use super::*;
use gpui_kit::component::input::TextareaState;
use tandem_claude::view::{self, ComposerMode};
use tandem_claude::{
    BufferText, ChatState, Companion, CompanionConfig, HostAnswer, HostAsk, HostQuestion, Signal,
    TakeoverRequest,
};
use tandem_core::chat::{ChangeSet, Decision, Mode, Phase};

/// What waits for the open buffers to be saved before Claude edits the files on disk.
pub(super) enum PendingAfterSave {
    Takeover {
        request: TakeoverRequest,
        saved: Vec<String>,
    },
    /// A message to Claude while it builds the remaining steps.
    Send { text: String, saved: Vec<String> },
}

pub(super) struct ChatUi {
    pub visible: bool,
    pub companion: Option<Companion>,
    pub composer: Entity<TextareaState>,
    pub scroll: ScrollHandle,
    /// The item count when the transcript last scrolled, so new items scroll it again.
    pub seen_items: usize,
    /// Whether a turn ran at the last state change, so its end can be noticed.
    pub was_running: bool,
    /// The decided files of each change set at the last state change, so a Keep or Undo
    /// (which can restore or remove files) reloads the editor and the file tree.
    pub decided: Vec<(String, usize)>,
    pub pending: Option<PendingAfterSave>,
    _composer_events: Subscription,
}

impl ChatUi {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        super::code_view::register_fence_aliases();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Message Claude…")
                .submit_on_enter(true)
        });
        let events = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                // Shift+Enter inserts a newline; submit_on_enter handles that.
                InputEvent::PressEnter { shift: false, .. } => this.chat_send(window, cx),
                // The Send button depends on whether the draft is empty.
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        Self {
            visible: true,
            companion: None,
            composer,
            scroll: ScrollHandle::default(),
            seen_items: 0,
            was_running: false,
            decided: Vec::new(),
            pending: None,
            _composer_events: events,
        }
    }
}

/// The number of decided (kept or undone) files of each change set.
fn decided_files(sets: &[ChangeSet]) -> Vec<(String, usize)> {
    sets.iter()
        .map(|set| {
            let decided = set
                .files
                .iter()
                .filter(|file| file.decision != Decision::Undecided)
                .count();
            (set.id.clone(), decided)
        })
        .collect()
}

impl Workspace {
    pub(super) fn chat_state(&self) -> Option<Arc<ChatState>> {
        self.chat.companion.as_ref().map(Companion::state)
    }

    pub(super) fn chat_turn_running(&self) -> bool {
        self.chat_state().is_some_and(|state| state.turn.is_some())
    }

    /// Whether Claude is editing the project's files, so the editor must not write them.
    pub(super) fn takeover_running(&self) -> bool {
        self.chat_state()
            .is_some_and(|state| view::takeover_running(&state))
    }

    pub(super) fn chat_command(&self, command: Command) {
        if let Some(companion) = &self.chat.companion {
            companion.send(command);
        }
    }

    /// Starts the companion for the open project, replacing any earlier one.
    pub(super) fn start_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dropping a companion asks its controller to shut down; its signal loop ends
        // when the controller does.
        self.chat.companion = None;
        self.chat.seen_items = 0;
        self.chat.was_running = false;
        self.chat.decided.clear();
        self.chat.pending = None;
        let Some(project) = &self.project else {
            return;
        };
        let (companion, signals) = Companion::start(CompanionConfig {
            project: project.root.clone(),
            store: self.store.clone(),
            launch: None,
            offline: std::env::var_os("TANDEM_CLAUDE_OFFLINE").is_some_and(|value| value == "1"),
        });
        self.chat.companion = Some(companion);
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                while let Ok(signal) = signals.recv().await {
                    if weak
                        .update_in(cx, |view, window, cx| {
                            view.on_chat_signal(signal, window, cx)
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        cx.notify();
    }

    pub(super) fn on_chat_signal(
        &mut self,
        signal: Signal,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match signal {
            Signal::Changed => {
                let Some(state) = self.chat_state() else {
                    return;
                };
                if state.items.len() > self.chat.seen_items || state.streaming.is_some() {
                    self.chat.scroll.scroll_to_bottom();
                }
                self.chat.seen_items = state.items.len();
                let running = state.turn.is_some();
                let decided = decided_files(&state.change_sets);
                if (self.chat.was_running && !running) || decided != self.chat.decided {
                    // Claude, or a Keep or Undo, may have changed files: reload open buffers
                    // and rescan the tree.
                    self.disk_events.store(true, Ordering::Relaxed);
                }
                self.chat.was_running = running;
                self.chat.decided = decided;
                cx.notify();
            }
            Signal::Host(ask) => self.answer_host(ask, cx),
        }
    }

    fn answer_host(&mut self, ask: HostAsk, cx: &mut Context<Self>) {
        self.snapshot_live(cx);
        let answer = match ask.question {
            HostQuestion::DirtyPaths => HostAnswer::DirtyPaths(
                self.tabs
                    .iter()
                    .filter(|tab| tab.document.dirty())
                    .map(|tab| tab.document.path.clone())
                    .collect(),
            ),
            HostQuestion::ReadBuffer { path } => {
                HostAnswer::Buffer(self.tabs.iter().find(|tab| tab.document.path == path).map(
                    |tab| BufferText {
                        text: tab.editor.read(cx).text().to_string(),
                        dirty: tab.document.dirty(),
                    },
                ))
            }
        };
        // The controller waits for at most one answer; never block the UI on it.
        let _ = ask.reply.try_send(answer);
    }

    pub(super) fn chat_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.chat_state() else {
            return;
        };
        let text = self.chat.composer.read(cx).value().to_string();
        if view::composer_mode(&state, text.trim().is_empty()) != ComposerMode::Send {
            return;
        }
        let building_the_rest = state
            .task
            .as_ref()
            .is_some_and(|task| task.phase == Phase::Build && task.mode == Mode::Takeover);
        if building_the_rest {
            // The answer lets Claude go on editing files on disk, and the controller refuses
            // while buffers are unsaved: save them first.
            if self.chat.pending.is_some() {
                return;
            }
            let Some(saved) = self.save_for_takeover(window, cx) else {
                // Blocked: the message stays in the composer.
                return;
            };
            if saved.is_empty() {
                self.chat_command(Command::Send(text));
            } else {
                self.chat.pending = Some(PendingAfterSave::Send { text, saved });
            }
        } else {
            self.chat_command(Command::Send(text));
        }
        self.chat
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }

    /// "Do it for me", "Build the rest for me", "Fix it for me" and the plan card's "You
    /// do it": saves the open buffers, because Claude edits the files on disk, then asks
    /// the controller to take over (at the next tick when a save had to run first).
    pub(super) fn begin_takeover(
        &mut self,
        request: TakeoverRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat.pending.is_some() {
            return;
        }
        let Some(saved) = self.save_for_takeover(window, cx) else {
            return;
        };
        if saved.is_empty() {
            self.chat_command(Command::Takeover { request, saved });
        } else {
            self.chat.pending = Some(PendingAfterSave::Takeover { request, saved });
        }
        cx.notify();
    }

    /// Carries out the takeover save plan for the open tabs: closes the new empty ones and
    /// starts saving the other unsaved ones. Returns the paths being saved, or `None` (with
    /// the reason in the status bar) when Claude cannot start.
    fn save_for_takeover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<String>> {
        self.snapshot_live(cx);
        let tabs: Vec<TabFacts> = self
            .tabs
            .iter()
            .map(|tab| TabFacts {
                path: tab.document.path.clone(),
                dirty: tab.document.dirty(),
                never_saved: tab.document.base_hash.is_none(),
                empty: tab.document.text.is_empty(),
                conflict: tab.document.conflict,
                read_only: tab.document.read_only,
                saving: tab.saving,
            })
            .collect();
        match takeover_save_plan(&tabs) {
            SavePlan::Ready => Some(Vec::new()),
            SavePlan::Blocked(message) => {
                self.message(message, true, cx);
                None
            }
            SavePlan::Save { save, close } => {
                if let Err(error) = self.close_empty_new_tabs(&close, window, cx) {
                    self.message(
                        format!(
                            "Could not forget the drafts of the empty new files, so Claude did not start: {error:#}"
                        ),
                        true,
                        cx,
                    );
                    return None;
                }
                for path in &save {
                    if let Some(index) = self.tabs.iter().position(|tab| tab.document.path == *path)
                    {
                        self.save_index(index, cx);
                    }
                }
                if !save.is_empty() {
                    self.message(
                        format!(
                            "Saving {} file(s) before Claude takes over: {}",
                            save.len(),
                            save.join(", ")
                        ),
                        false,
                        cx,
                    );
                }
                Some(save)
            }
        }
    }

    /// Closes the never-saved, empty tabs in `paths` and forgets their drafts. Saving them
    /// would create empty files, and a kept draft would reopen empty over the file Claude
    /// may create at that path.
    fn close_empty_new_tabs(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let active = self
            .active
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.document.path.clone());
        let old_active = self.active.unwrap_or(0);
        let mut closed = Vec::new();
        self.tabs.retain(|tab| {
            let close = paths.contains(&tab.document.path);
            if close {
                closed.push(tab.document.clone());
            }
            !close
        });
        self.active = active
            .and_then(|path| self.tabs.iter().position(|tab| tab.document.path == path))
            .or_else(|| {
                (!self.tabs.is_empty()).then(|| old_active.min(self.tabs.len().saturating_sub(1)))
            });
        self.invalidate_language(cx);
        self.close_pending = false;
        self.diff = None;
        if self.active.is_some() {
            self.go_to_line(None, window, cx);
        } else {
            self.query.update(cx, |query, cx| query.focus(window, cx));
        }
        self.changed(cx);
        // A checkpoint removes the draft of every document it is given that is not dirty,
        // so the closed buffers are passed as clean copies.
        let forgotten = closed.into_iter().map(|mut document| {
            document.base_hash = Some(content_hash(document.text.as_bytes()));
            document.base_text = document.text.clone();
            document
        });
        let documents: Vec<Document> = self
            .tabs
            .iter()
            .map(|tab| tab.document.clone())
            .chain(forgotten)
            .collect();
        if let Some(state) = self.state() {
            self.store
                .lock()
                .map_err(|_| anyhow!("Recovery store unavailable"))?
                .checkpoint(&state, &documents)?;
            self.checkpointed = self.generation;
        }
        Ok(())
    }

    /// Runs from the workspace's 500 ms loop: once the saves started for a takeover have
    /// finished, starts the takeover or sends the parked message, or reports that a file
    /// could not be saved.
    pub(super) fn chat_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.pending.is_none() || self.tabs.iter().any(|tab| tab.saving) {
            return;
        }
        let Some(pending) = self.chat.pending.take() else {
            return;
        };
        self.snapshot_live(cx);
        let saved = match &pending {
            PendingAfterSave::Takeover { saved, .. } | PendingAfterSave::Send { saved, .. } => {
                saved
            }
        };
        let unsaved = self
            .tabs
            .iter()
            .any(|tab| tab.document.dirty() && saved.contains(&tab.document.path));
        if unsaved {
            if let PendingAfterSave::Send { text, .. } = pending
                && self.chat.composer.read(cx).value().is_empty()
            {
                // Give the unsent message back.
                self.chat
                    .composer
                    .update(cx, |composer, cx| composer.set_value(text, window, cx));
            }
            self.message(
                "Could not save every file, so Claude did not start. Your drafts are kept.",
                true,
                cx,
            );
            return;
        }
        match pending {
            PendingAfterSave::Takeover { request, saved } => {
                self.chat_command(Command::Takeover { request, saved })
            }
            PendingAfterSave::Send { text, .. } => self.chat_command(Command::Send(text)),
        }
        cx.notify();
    }

    /// "Open file" on the step card: opens the step's file (a new buffer when it does not
    /// exist yet) with the cursor on the guidance's anchor line when one is found.
    pub(super) fn open_step_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.chat_state().as_deref().and_then(view::step_card) else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let guidance = card.guidance.as_ref();
        let path = guidance
            .and_then(|guidance| guidance.path.clone())
            .unwrap_or(card.target);
        let disk = project.root.join(&path);
        let new = !disk.exists();
        let line = guidance
            .and_then(|guidance| guidance.anchor.as_deref())
            .and_then(|anchor| {
                let open = self.tabs.iter().find(|tab| tab.document.path == path);
                match open {
                    Some(tab) => anchor_line(&tab.editor.read(cx).text().to_string(), anchor),
                    None => small_file_text(&disk).and_then(|text| anchor_line(&text, anchor)),
                }
            });
        self.settings.open = false;
        self.open_file(path, new, line, window, cx);
    }

    /// "Open Terminal" on the step card: shows the bottom panel on the Terminal tab,
    /// starting the terminal when none runs.
    pub(super) fn show_terminal(&mut self, cx: &mut Context<Self>) {
        if self.bottom == Some(BottomTab::Terminal) {
            if self.terminal.is_some() {
                return;
            }
            self.bottom = None;
        }
        self.toggle_terminal(cx);
    }

    pub(super) fn focus_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.visible = true;
        self.chat
            .composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        cx.notify();
    }
}

/// The text of a file on disk when it is at most `MAX_FILE_BYTES`, otherwise `None`.
fn small_file_text(path: &std::path::Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}
