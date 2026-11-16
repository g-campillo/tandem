//! The chat panel's state in the workspace and its link to the companion controller.
use super::*;
use gpui_kit::component::input::TextareaState;
use tandem_claude::view::{self, ComposerMode};
use tandem_claude::{
    BufferText, ChatState, Companion, CompanionConfig, HostAnswer, HostAsk, HostQuestion, Signal,
};

pub(super) struct ChatUi {
    pub visible: bool,
    pub companion: Option<Companion>,
    pub composer: Entity<TextareaState>,
    pub scroll: ScrollHandle,
    /// The item count when the transcript last scrolled, so new items scroll it again.
    pub seen_items: usize,
    /// Whether a turn ran at the last state change, so its end can be noticed.
    pub was_running: bool,
    _composer_events: Subscription,
}

impl ChatUi {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
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
            _composer_events: events,
        }
    }
}

impl Workspace {
    pub(super) fn chat_state(&self) -> Option<Arc<ChatState>> {
        self.chat.companion.as_ref().map(Companion::state)
    }

    pub(super) fn chat_turn_running(&self) -> bool {
        self.chat_state().is_some_and(|state| state.turn.is_some())
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
                if self.chat.was_running && !running {
                    // Claude may have changed files: reload open buffers and rescan the tree.
                    self.disk_events.store(true, Ordering::Relaxed);
                }
                self.chat.was_running = running;
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
        self.chat_command(Command::Send(text));
        self.chat
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn focus_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.visible = true;
        self.chat
            .composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        cx.notify();
    }
}
