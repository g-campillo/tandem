//! The headless companion controller. It runs on its own thread, owns the helper process
//! and the active task, persists through the shared `Store`, and publishes an immutable
//! `ChatState` snapshot for the UI. Only `Command`s from the UI and frames from the helper
//! drive it; questions about the editor go to the UI as `Signal::Host`.

use crate::client::{HelperClient, Received};
use crate::install::{Launch, sign_in_hint};
use crate::prompts::{self, cut};
use crate::protocol::{
    Event, HelloData, HostRequest, Incoming, OpenSession, Outgoing, PROTOCOL_VERSION, Policy,
    ResponseBody, SessionData, Turn, TurnStatus, WireError,
};
use crate::state::{
    BufferText, ChatState, Command, CompanionConfig, Connection, HostAnswer, HostAsk, HostQuestion,
    InstallInfo, Signal, TurnKind,
};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tandem_core::chat::{
    ActivityKind, ChatItem, ChatPrefs, ChatTask, DEFAULT_MODEL, ItemKind, NEW_TASK_TITLE, Phase,
    Plan, TaskSummary,
};
use tandem_core::{Project, Store, new_id, normalized_relative};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const OPEN_TIMEOUT: Duration = Duration::from_secs(60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const HOST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long Shutdown waits for an interrupted turn to finish.
const SHUTDOWN_TURN_WAIT: Duration = Duration::from_secs(2);
/// How long `Companion::shutdown` waits for the controller thread.
const SHUTDOWN_JOIN: Duration = Duration::from_secs(4);

const MAX_MESSAGE_CHARS: usize = 100_000;
const TITLE_CHARS: usize = 60;

const OFFLINE_REASON: &str = "Claude is switched off for this run.";
const OFFLINE_HINT: &str = "Unset TANDEM_CLAUDE_OFFLINE and restart Tandem.";
const PROTOCOL_HINT: &str = "Reinstall Tandem, or run npm ci in the helper folder.";
const NOT_CONNECTED: &str = "Claude is not connected.";
const HELPER_STOPPED: &str = "The Claude helper stopped.";
const HELPER_TIMED_OUT: &str = "The Claude helper did not answer in time.";
const CLOSING: &str = "Tandem is closing.";
const UNEXPECTED_REPLY: &str = "The Claude helper sent a reply Tandem does not understand.";
const STOPPED_UNEXPECTEDLY: &str = "Claude stopped unexpectedly.";
const TURN_FAILED: &str = "Claude could not finish this turn.";
const STOPPED_NOTICE: &str = "Stopped.";
const STALE_MODEL: &str = "The saved model is no longer available; using Default.";
const MODEL_UNAVAILABLE: &str = "That model is not available.";
const EFFORT_UNAVAILABLE: &str = "That effort is not available for this model.";
const PLAN_SHOWN: &str = "The plan is on screen with two choices: \"I'll write it\" and \"You do it\". Stop here and do not implement anything. Finish with one short sentence asking the user to review the plan.";
const PLAN_LOCKED: &str = "The plan cannot change during Build. Adapt inside the current step, or tell the user to start a new task for a different plan.";
const TOOL_UNAVAILABLE: &str = "This tool is not available right now.";
const PATH_OUTSIDE: &str = "Give a path inside the project.";
const ANSWER_FAILED: &str = "Tandem could not send this answer.";

/// The UI's handle on the controller thread.
pub struct Companion {
    inbox: Sender<Inbound>,
    shared: Arc<Mutex<Arc<ChatState>>>,
    thread: Option<JoinHandle<()>>,
    /// Disconnects when the controller thread ends.
    finished: Option<Receiver<()>>,
}

impl Companion {
    /// Starts the controller thread. The receiver carries `Signal::Changed` after every
    /// state change and the controller's questions about the editor.
    pub fn start(config: CompanionConfig) -> (Companion, async_channel::Receiver<Signal>) {
        let (signals, signal_receiver) = async_channel::unbounded();
        let (inbox, inbox_receiver) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Arc::new(ChatState::starting())));
        let (finished_sender, finished) = mpsc::channel::<()>();
        let publisher = Publisher {
            shared: Arc::clone(&shared),
            signals,
        };
        let controller_inbox = inbox.clone();
        let thread = thread::Builder::new()
            .name("tandem-companion".into())
            .spawn(move || {
                let _finished = finished_sender;
                run(config, publisher, controller_inbox, inbox_receiver);
            })
            .expect("start the companion thread");
        (
            Companion {
                inbox,
                shared,
                thread: Some(thread),
                finished: Some(finished),
            },
            signal_receiver,
        )
    }

    /// The latest snapshot; cheap, it clones an `Arc`.
    pub fn state(&self) -> Arc<ChatState> {
        Arc::clone(&lock(&self.shared))
    }

    pub fn send(&self, command: Command) {
        let _ = self.inbox.send(Inbound::Command(command));
    }

    /// Sends Shutdown and waits up to about 4 s for the controller to end.
    pub fn shutdown(mut self) {
        let _ = self.inbox.send(Inbound::Command(Command::Shutdown));
        let ended = self.finished.take().is_some_and(|finished| {
            matches!(
                finished.recv_timeout(SHUTDOWN_JOIN),
                Err(RecvTimeoutError::Disconnected)
            )
        });
        if let Some(thread) = self.thread.take()
            && ended
        {
            let _ = thread.join();
        }
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        let _ = self.inbox.send(Inbound::Command(Command::Shutdown));
    }
}

/// Everything the controller thread waits for, in one queue.
enum Inbound {
    Command(Command),
    /// `generation` names the helper process that sent it; older ones are ignored.
    Helper {
        generation: u64,
        received: Received,
    },
}

/// Publishes snapshots: changes are made under the lock with `Arc::make_mut`, and the lock
/// is never held for anything else.
struct Publisher {
    shared: Arc<Mutex<Arc<ChatState>>>,
    signals: async_channel::Sender<Signal>,
}

impl Publisher {
    fn snapshot(&self) -> Arc<ChatState> {
        Arc::clone(&lock(&self.shared))
    }

    fn update(&self, change: impl FnOnce(&mut ChatState)) {
        {
            let mut shared = lock(&self.shared);
            change(Arc::make_mut(&mut shared));
        }
        // An error means the UI is gone.
        let _ = self.signals.try_send(Signal::Changed);
    }
}

fn run(
    config: CompanionConfig,
    publisher: Publisher,
    inbox_sender: Sender<Inbound>,
    inbox: Receiver<Inbound>,
) {
    match load(&config) {
        Ok((project, state)) => {
            publisher.update(|shared| *shared = state);
            let mut controller = Controller {
                config,
                project,
                publisher,
                inbox,
                inbox_sender,
                queue: VecDeque::new(),
                client: None,
                generation: 0,
                launch: None,
                hello: None,
                recap: false,
                permission: None,
                waiting_permissions: VecDeque::new(),
                closing: false,
            };
            controller.connect();
            controller.serve();
        }
        Err(error) => {
            publisher.update(|state| {
                state.connection = Connection::Unavailable {
                    reason: format!("{error:#}"),
                    hint: String::new(),
                }
            });
            while let Ok(inbound) = inbox.recv() {
                if matches!(inbound, Inbound::Command(Command::Shutdown)) {
                    return;
                }
            }
        }
    }
}

/// Opens the project and loads the preferences, the task list and the selected task (a
/// new one when the project has none) with its items.
fn load(config: &CompanionConfig) -> Result<(Project, ChatState)> {
    let project = Project::open(&config.project)?;
    let key = project.key().to_owned();
    let mut store = lock(&config.store);
    let prefs = store.chat_prefs()?;
    let selected = match store.selected_chat_task(&key)? {
        Some(id) => store.chat_task(&id)?,
        None => None,
    };
    let task = match selected {
        Some(task) => task,
        None => match store.chat_tasks(&key)?.first() {
            Some(latest) => {
                let task = store
                    .chat_task(&latest.id)?
                    .context("A chat task disappeared while loading")?;
                store.select_chat_task(&key, &task.id)?;
                task
            }
            None => store.create_chat_task(&key)?,
        },
    };
    let items = store.chat_items(&task.id)?;
    let tasks = store.chat_tasks(&key)?;
    drop(store);
    let mut state = ChatState::starting();
    state.prefs = prefs;
    state.tasks = tasks;
    state.task = Some(task);
    state.items = items;
    Ok((project, state))
}

/// A helper request that failed: the helper's error code, if it sent one, and a message
/// for the user.
struct HelperError {
    code: Option<String>,
    message: String,
}

impl HelperError {
    fn local(message: impl Into<String>) -> HelperError {
        HelperError {
            code: None,
            message: message.into(),
        }
    }
}

impl From<WireError> for HelperError {
    fn from(error: WireError) -> HelperError {
        HelperError {
            code: Some(error.code),
            message: error.message,
        }
    }
}

/// A permission card waiting for the user.
struct PendingPermission {
    request: String,
    item: i64,
}

/// A permission request that arrived while another card was waiting.
struct PermissionAsk {
    request: String,
    title: String,
    detail: String,
    command: Option<String>,
}

struct Controller {
    config: CompanionConfig,
    project: Project,
    publisher: Publisher,
    inbox: Receiver<Inbound>,
    inbox_sender: Sender<Inbound>,
    /// Messages that arrived while a request waited for its reply, oldest first.
    queue: VecDeque<Inbound>,
    client: Option<HelperClient>,
    generation: u64,
    /// The launch of the current helper.
    launch: Option<Launch>,
    /// Set once the current helper answered `hello`.
    hello: Option<HelloData>,
    /// The next chat turn starts with a recap, because Claude lost the conversation.
    recap: bool,
    permission: Option<PendingPermission>,
    waiting_permissions: VecDeque<PermissionAsk>,
    /// Set by Shutdown: new helper requests are refused instead of reaching the UI.
    closing: bool,
}

impl Controller {
    fn snapshot(&self) -> Arc<ChatState> {
        self.publisher.snapshot()
    }

    fn update(&self, change: impl FnOnce(&mut ChatState)) {
        self.publisher.update(change);
    }

    fn set_banner(&self, text: impl Into<String>) {
        let text = text.into();
        self.update(|state| state.banner = Some(text));
    }

    fn with_store<T>(&self, work: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        work(&mut lock(&self.config.store))
    }

    fn serve(&mut self) {
        loop {
            let inbound = match self.queue.pop_front() {
                Some(inbound) => inbound,
                None => match self.inbox.recv() {
                    Ok(inbound) => inbound,
                    Err(_) => Inbound::Command(Command::Shutdown),
                },
            };
            match inbound {
                Inbound::Command(Command::Shutdown) => {
                    self.shutdown();
                    return;
                }
                Inbound::Command(command) => self.command(command),
                Inbound::Helper {
                    generation,
                    received,
                } if generation == self.generation && self.client.is_some() => {
                    self.helper(received);
                }
                Inbound::Helper { .. } => {}
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::NewTask => self.new_task(),
            Command::SelectTask(id) => self.select_task(&id),
            Command::Send(text) => self.send_message(&text),
            Command::Stop => self.stop(),
            Command::Permission { allow } => self.answer_permission(allow),
            Command::SetModel(value) => self.set_model(value),
            Command::SetEffort(value) => self.set_effort(value),
            Command::SetUseMySetup(value) => self.set_use_my_setup(value),
            Command::Reconnect => {
                if self.snapshot().turn.is_none() {
                    self.connect();
                }
            }
            // `serve` ends the loop on Shutdown before it gets here.
            Command::Shutdown => {}
        }
    }

    // ----- Connection -----

    fn connect(&mut self) {
        self.drop_client();
        self.update(|state| state.connection = Connection::Starting);
        if self.config.offline {
            self.update(|state| {
                state.connection = Connection::Unavailable {
                    reason: OFFLINE_REASON.into(),
                    hint: OFFLINE_HINT.into(),
                }
            });
            return;
        }
        let launch = match self.config.launch.clone() {
            Some(launch) => launch,
            None => match Launch::discover() {
                Ok(launch) => launch,
                Err(error) => {
                    self.update(|state| {
                        state.connection = Connection::Unavailable {
                            reason: format!("{error:#}"),
                            hint: String::new(),
                        }
                    });
                    return;
                }
            },
        };
        let install = install_info(&launch);
        self.generation += 1;
        let generation = self.generation;
        let sender = self.inbox_sender.clone();
        let sink = Box::new(move |received| {
            let _ = sender.send(Inbound::Helper {
                generation,
                received,
            });
        });
        match HelperClient::spawn(&launch, sink) {
            Ok(client) => self.client = Some(client),
            Err(error) => {
                self.update(|state| {
                    state.install = install;
                    state.connection = Connection::Failed {
                        message: format!("{error:#}"),
                    };
                });
                return;
            }
        }
        self.launch = Some(launch);
        self.update(|state| state.install = install);

        let hello = match self
            .request(|id| Outgoing::Hello { id }, HELLO_TIMEOUT)
            .and_then(|data| {
                serde_json::from_value::<HelloData>(data)
                    .map_err(|_| HelperError::local(UNEXPECTED_REPLY))
            }) {
            Ok(hello) => hello,
            Err(error) => return self.fail(error.message),
        };
        if hello.protocol != PROTOCOL_VERSION {
            self.drop_client();
            let reason = format!(
                "The Claude helper speaks protocol {}; this Tandem needs {PROTOCOL_VERSION}.",
                hello.protocol
            );
            self.update(|state| {
                state.connection = Connection::Unavailable {
                    reason,
                    hint: PROTOCOL_HINT.into(),
                }
            });
            return;
        }
        self.hello = Some(hello);
        self.open_session();
    }

    /// Opens the Claude session of the active task: resumed when it was used before, new
    /// otherwise. A conversation that cannot be resumed is replaced by a fresh session once.
    /// Whenever the task has a conversation that its session never saw, the next turn
    /// carries a recap.
    fn open_session(&mut self) {
        let mut retried = false;
        loop {
            let state = self.snapshot();
            let Some(task) = state.task.clone() else {
                return;
            };
            self.recap = !task.session_started
                && state.items.iter().any(|item| {
                    matches!(
                        item.kind,
                        ItemKind::User { .. } | ItemKind::Assistant { .. }
                    )
                });
            let (session_id, resume) = if task.session_started {
                (None, Some(task.session_id.clone()))
            } else {
                (Some(task.session_id.clone()), None)
            };
            let open = OpenSession {
                cwd: self.project.key().to_owned(),
                session_id,
                resume,
                use_my_setup: state.prefs.use_my_setup,
                model: state.prefs.model.clone(),
                effort: state.prefs.effort.clone(),
                system_append: prompts::SYSTEM_APPEND.to_owned(),
            };
            match self.request(|id| Outgoing::OpenSession { id, open }, OPEN_TIMEOUT) {
                Ok(data) => return self.session_opened(data),
                Err(error) if error.code.as_deref() == Some("resume_failed") && !retried => {
                    retried = true;
                    let mut task = task;
                    task.session_id = new_id();
                    task.session_started = false;
                    self.save_task(task);
                }
                Err(error) => return self.fail(error.message),
            }
        }
    }

    fn session_opened(&mut self, data: Value) {
        let Ok(session) = serde_json::from_value::<SessionData>(data) else {
            return self.fail(UNEXPECTED_REPLY.into());
        };
        if !session.auth.ok {
            let connection = if session.auth.reason.as_deref() == Some("signed_out") {
                Connection::SignedOut {
                    hint: sign_in_hint(
                        self.launch
                            .as_ref()
                            .and_then(|launch| launch.helper.as_ref()),
                    ),
                }
            } else {
                Connection::Refused {
                    reason: session.auth.detail.unwrap_or_else(|| {
                        "This Claude account cannot be used with Tandem.".into()
                    }),
                }
            };
            self.update(|state| state.connection = connection);
            return;
        }
        let claude_code = session
            .claude_code_version
            .or_else(|| self.hello.as_ref().map(|hello| hello.claude_code.clone()))
            .unwrap_or_default();
        let subscription = session
            .account
            .subscription
            .unwrap_or_else(|| "Claude subscription".into());
        let mut prefs = self.snapshot().prefs.clone();
        let stale = prefs.model != DEFAULT_MODEL
            && !session
                .models
                .iter()
                .any(|model| model.value == prefs.model);
        if stale {
            prefs.model = DEFAULT_MODEL.into();
            prefs.effort = None;
            self.save_prefs(&prefs);
        }
        let models = session.models;
        self.update(|state| {
            state.connection = Connection::Ready {
                subscription,
                claude_code,
            };
            state.models = models;
            state.prefs = prefs;
            if stale {
                state.banner = Some(STALE_MODEL.into());
            }
        });
    }

    /// Closes the current session and opens the active task's one, when the helper is up.
    fn reopen_session(&mut self) {
        if self.client.is_none() || self.hello.is_none() {
            return;
        }
        // Opening a session replaces any open one, so a failed close changes nothing.
        let _ = self.request(|id| Outgoing::CloseSession { id }, COMMAND_TIMEOUT);
        self.open_session();
    }

    /// The connection failed. A helper that already stopped is reported by its `Closed`
    /// message instead, which carries the end of its stderr.
    fn fail(&mut self, message: String) {
        self.update(|state| state.connection = Connection::Failed { message });
        if !self.closed_queued() {
            self.drop_client();
        }
    }

    fn closed_queued(&self) -> bool {
        self.queue.iter().any(|inbound| {
            matches!(inbound, Inbound::Helper { generation, received: Received::Closed { .. } }
                if *generation == self.generation)
        })
    }

    fn drop_client(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.shutdown();
        }
        // Whatever the old helper still sends is ignored.
        self.generation += 1;
        self.hello = None;
    }

    /// Sends a command and waits for its reply, queueing everything else that arrives
    /// meanwhile to be handled afterwards in arrival order.
    fn request(
        &mut self,
        build: impl FnOnce(u64) -> Outgoing,
        timeout: Duration,
    ) -> Result<Value, HelperError> {
        if self.closed_queued() {
            return Err(HelperError::local(HELPER_STOPPED));
        }
        if self
            .queue
            .iter()
            .any(|inbound| matches!(inbound, Inbound::Command(Command::Shutdown)))
        {
            return Err(HelperError::local(CLOSING));
        }
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HelperError::local(NOT_CONNECTED))?;
        let id = client
            .command(build)
            .map_err(|error| HelperError::local(format!("{error:#}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(inbound) = self.inbox.recv_timeout(remaining) else {
                return Err(HelperError::local(HELPER_TIMED_OUT));
            };
            match inbound {
                Inbound::Helper { generation, .. } if generation != self.generation => {}
                Inbound::Helper {
                    received:
                        Received::Frame(Incoming::Reply {
                            id: reply,
                            ok,
                            data,
                            error,
                        }),
                    ..
                } if reply == id => {
                    return if ok {
                        Ok(data)
                    } else {
                        Err(error.map_or_else(
                            || HelperError::local("The Claude helper refused the request."),
                            HelperError::from,
                        ))
                    };
                }
                closed @ Inbound::Helper {
                    received: Received::Closed { .. },
                    ..
                } => {
                    self.queue.push_back(closed);
                    return Err(HelperError::local(HELPER_STOPPED));
                }
                shutdown @ Inbound::Command(Command::Shutdown) => {
                    self.queue.push_back(shutdown);
                    return Err(HelperError::local(CLOSING));
                }
                other => self.queue.push_back(other),
            }
        }
    }

    /// Answers a helper request; an answer that cannot be sent becomes a short refusal so
    /// Claude is never left waiting.
    fn respond(&self, request: &str, body: ResponseBody) {
        let Some(client) = &self.client else {
            return;
        };
        let is_tool = matches!(body, ResponseBody::Tool { .. });
        if client.respond(request, body).is_err() && is_tool {
            let _ = client.respond(
                request,
                ResponseBody::Tool {
                    ok: false,
                    text: ANSWER_FAILED.into(),
                },
            );
        }
    }

    // ----- Helper messages -----

    fn helper(&mut self, received: Received) {
        match received {
            Received::Closed { error, stderr_tail } => self.helper_stopped(&stderr_tail, error),
            Received::Frame(Incoming::Event(event)) => self.event(event),
            Received::Frame(Incoming::Request { id, request }) if self.closing => {
                let refusal = match request {
                    HostRequest::Tool { .. } => ResponseBody::Tool {
                        ok: false,
                        text: CLOSING.into(),
                    },
                    HostRequest::Permission { .. } => ResponseBody::Permission { allow: false },
                };
                self.respond(&id, refusal);
            }
            Received::Frame(Incoming::Request { id, request }) => match request {
                HostRequest::Tool { name, input } => {
                    let (ok, text) = match name.as_str() {
                        "propose_plan" => self.propose_plan(input),
                        "read_editor_buffer" => self.read_editor_buffer(&input),
                        _ => (false, TOOL_UNAVAILABLE.to_owned()),
                    };
                    self.respond(&id, ResponseBody::Tool { ok, text });
                }
                HostRequest::Permission {
                    title,
                    detail,
                    command,
                    ..
                } => self.permission_requested(PermissionAsk {
                    request: id,
                    title,
                    detail,
                    command,
                }),
            },
            // A reply nobody waits for any more.
            Received::Frame(Incoming::Reply { .. }) => {}
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::TurnStarted { .. } | Event::Unknown => {}
            Event::TextDelta { text } => self.update(|state| {
                state
                    .streaming
                    .get_or_insert_with(String::new)
                    .push_str(&text)
            }),
            Event::AssistantText { text } => {
                let item = self.store_item(ItemKind::Assistant { text });
                self.update(|state| {
                    state.items.extend(item);
                    state.streaming = None;
                });
            }
            Event::Activity { kind, label } => self.append_item(ItemKind::Activity {
                kind: ActivityKind::from_wire(&kind),
                label,
            }),
            Event::Notice { text } => self.append_item(ItemKind::Notice { text }),
            Event::AuthRefused { detail, .. } => {
                self.end_turn(None);
                self.drop_client();
                self.update(|state| state.connection = Connection::Refused { reason: detail });
            }
            Event::PermissionCancelled { id } => self.permission_cancelled(&id),
            Event::TurnFinished { status, error, .. } => {
                if self.snapshot().turn.is_none() {
                    return;
                }
                match status {
                    TurnStatus::Completed => self.end_turn(None),
                    TurnStatus::Interrupted => self.end_turn(Some(STOPPED_NOTICE.into())),
                    TurnStatus::Error => self.end_turn_with_error(error),
                }
            }
            Event::SessionClosed { reason } => {
                let tail = self
                    .client
                    .as_ref()
                    .map(HelperClient::stderr_tail)
                    .unwrap_or_default();
                self.helper_stopped(&tail, Some(reason).filter(|reason| !reason.is_empty()));
            }
        }
    }

    /// Ends the running turn: drops streaming text, marks an unanswered permission card
    /// denied and appends `notice` when given.
    fn end_turn(&mut self, notice: Option<String>) {
        self.deny_permissions(false);
        let item = notice.and_then(|text| self.store_item(ItemKind::Notice { text }));
        self.update(|state| {
            state.streaming = None;
            state.turn = None;
            state.stopping = false;
            state.items.extend(item);
        });
    }

    fn end_turn_with_error(&mut self, error: Option<String>) {
        let text = error
            .filter(|error| !error.trim().is_empty())
            .unwrap_or_else(|| TURN_FAILED.into());
        self.end_turn(Some(text.clone()));
        self.set_banner(text);
    }

    /// The helper exited, or its Claude session ended on its own.
    fn helper_stopped(&mut self, stderr_tail: &str, error: Option<String>) {
        if self.snapshot().turn.is_some() {
            self.end_turn_with_error(None);
        }
        let detail = stderr_tail
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
            .or(error);
        let message = match detail {
            Some(detail) => format!("{STOPPED_UNEXPECTEDLY} {detail}"),
            None => STOPPED_UNEXPECTEDLY.to_owned(),
        };
        self.drop_client();
        self.update(|state| state.connection = Connection::Failed { message });
    }

    // ----- Tools -----

    fn propose_plan(&mut self, input: Value) -> (bool, String) {
        let state = self.snapshot();
        let Some(mut task) = state.task.clone() else {
            return (false, PLAN_LOCKED.into());
        };
        if !matches!(task.phase, Phase::Discuss | Phase::Plan) || state.turn != Some(TurnKind::Chat)
        {
            return (false, PLAN_LOCKED.into());
        }
        let plan = serde_json::from_value::<Plan>(input)
            .map_err(|error| anyhow!("The plan does not have the expected fields: {error}."))
            .and_then(Plan::normalized);
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => return (false, format!("{error:#}")),
        };
        task.title = title_from(&plan.goal);
        task.plan = Some(plan.clone());
        task.phase = Phase::Plan;
        self.save_task(task);
        self.append_item(ItemKind::Plan { plan });
        (true, PLAN_SHOWN.into())
    }

    fn read_editor_buffer(&self, input: &Value) -> (bool, String) {
        let raw = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Ok(path) = normalized_relative(Path::new(raw)) else {
            return (false, PATH_OUTSIDE.into());
        };
        let buffer = match self.ask_host(HostQuestion::ReadBuffer { path: path.clone() }) {
            Some(HostAnswer::Buffer(buffer)) => buffer,
            _ => None,
        };
        match buffer {
            Some(BufferText { text, dirty: true }) => {
                (true, format!("Unsaved editor contents of {path}:\n{text}"))
            }
            Some(BufferText { text, dirty: false }) => {
                (true, format!("Editor contents of {path} (saved):\n{text}"))
            }
            None => match self.project.open_document(&path, false) {
                Ok(document) => (
                    true,
                    format!("Saved contents of {path}:\n{}", document.text),
                ),
                Err(error) => (false, format!("Cannot read {path}: {error:#}")),
            },
        }
    }

    /// Asks the UI thread and waits up to 5 s; no answer counts as "nothing open".
    fn ask_host(&self, question: HostQuestion) -> Option<HostAnswer> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.publisher
            .signals
            .try_send(Signal::Host(HostAsk { question, reply }))
            .ok()?;
        answer.recv_timeout(HOST_TIMEOUT).ok()
    }

    // ----- Permissions -----

    fn permission_requested(&mut self, ask: PermissionAsk) {
        if self.permission.is_some() {
            self.waiting_permissions.push_back(ask);
        } else {
            self.show_permission(ask);
        }
    }

    fn show_permission(&mut self, ask: PermissionAsk) {
        let kind = ItemKind::Permission {
            title: ask.title,
            detail: ask.detail,
            command: ask.command,
            allowed: None,
        };
        let Some(item) = self.store_item(kind) else {
            // A card that cannot be shown cannot be allowed.
            self.respond(&ask.request, ResponseBody::Permission { allow: false });
            return;
        };
        self.permission = Some(PendingPermission {
            request: ask.request,
            item: item.id,
        });
        self.update(|state| {
            state.pending_permission = Some(item.id);
            state.items.push(item);
        });
    }

    fn show_next_permission(&mut self) {
        if let Some(next) = self.waiting_permissions.pop_front() {
            self.show_permission(next);
        }
    }

    fn answer_permission(&mut self, allow: bool) {
        let Some(pending) = self.permission.take() else {
            return;
        };
        self.respond(&pending.request, ResponseBody::Permission { allow });
        self.decide_permission(pending.item, allow);
        self.show_next_permission();
    }

    fn permission_cancelled(&mut self, request: &str) {
        if self
            .permission
            .as_ref()
            .is_some_and(|pending| pending.request == request)
        {
            if let Some(pending) = self.permission.take() {
                self.decide_permission(pending.item, false);
            }
            self.show_next_permission();
        } else {
            self.waiting_permissions
                .retain(|ask| ask.request != request);
        }
    }

    /// Denies the card on screen and every waiting request, telling the helper when
    /// `respond` is set.
    fn deny_permissions(&mut self, respond: bool) {
        let waiting: Vec<PermissionAsk> = self.waiting_permissions.drain(..).collect();
        if let Some(pending) = self.permission.take() {
            if respond {
                self.respond(&pending.request, ResponseBody::Permission { allow: false });
            }
            self.decide_permission(pending.item, false);
        }
        if respond {
            for ask in waiting {
                self.respond(&ask.request, ResponseBody::Permission { allow: false });
            }
        }
    }

    /// Records the answer on the Permission item and clears `pending_permission`.
    fn decide_permission(&mut self, item_id: i64, allow: bool) {
        let state = self.snapshot();
        let decided =
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .and_then(|item| match &item.kind {
                    ItemKind::Permission {
                        title,
                        detail,
                        command,
                        ..
                    } => Some(ItemKind::Permission {
                        title: title.clone(),
                        detail: detail.clone(),
                        command: command.clone(),
                        allowed: Some(allow),
                    }),
                    _ => None,
                });
        if let Some(kind) = &decided
            && let Err(error) = self.with_store(|store| store.update_chat_item(item_id, kind))
        {
            self.set_banner(format!("Could not save the chat: {error:#}"));
        }
        self.update(|state| {
            if let Some(kind) = decided
                && let Some(item) = state.items.iter_mut().find(|item| item.id == item_id)
            {
                item.kind = kind;
            }
            state.pending_permission = None;
        });
    }

    // ----- Commands -----

    fn send_message(&mut self, text: &str) {
        let state = self.snapshot();
        let text = text.trim();
        let problem = if !matches!(state.connection, Connection::Ready { .. }) {
            Some(NOT_CONNECTED)
        } else if state.turn.is_some() {
            Some("Claude is still working. Wait for it, or press Stop.")
        } else if state.task.is_none() {
            Some("There is no task to send this to.")
        } else if text.is_empty() {
            Some("Type a message.")
        } else if text.chars().count() > MAX_MESSAGE_CHARS {
            Some("That message is too long: the limit is 100,000 characters.")
        } else {
            None
        };
        if let Some(problem) = problem {
            return self.set_banner(problem);
        }
        let Some(mut task) = state.task.clone() else {
            return;
        };
        let prompt = prompts::chat(&task, &state.items, text, self.recap);
        let policy = match task.phase {
            Phase::Discuss | Phase::Plan => Policy::Discuss,
            Phase::Build | Phase::Done => Policy::Guided,
        };
        self.append_item(ItemKind::User {
            text: text.to_owned(),
        });
        if task.title == NEW_TASK_TITLE {
            task.title = title_from(text.lines().next().unwrap_or(text));
            self.save_task(task);
        }
        self.start_turn(TurnKind::Chat, prompt, policy);
    }

    fn start_turn(&mut self, kind: TurnKind, text: String, policy: Policy) {
        let prefs = self.snapshot().prefs.clone();
        let turn = Turn {
            text,
            policy,
            model: prefs.model,
            effort: prefs.effort,
        };
        match self.request(|id| Outgoing::Turn { id, turn }, COMMAND_TIMEOUT) {
            Ok(_) => {
                self.recap = false;
                self.update(|state| {
                    state.turn = Some(kind);
                    state.banner = None;
                });
                if let Some(mut task) = self.snapshot().task.clone() {
                    task.session_started = true;
                    self.save_task(task);
                }
            }
            Err(error) => self.set_banner(error.message),
        }
    }

    fn stop(&mut self) {
        let state = self.snapshot();
        if state.turn.is_none() || state.stopping {
            return;
        }
        self.deny_permissions(true);
        self.update(|state| state.stopping = true);
        if let Err(error) = self.request(|id| Outgoing::Interrupt { id }, COMMAND_TIMEOUT)
            && !self.closed_queued()
        {
            self.update(|state| {
                state.stopping = false;
                state.banner = Some(error.message);
            });
        }
    }

    fn new_task(&mut self) {
        let state = self.snapshot();
        if state.turn.is_some() || state.items.is_empty() {
            return;
        }
        let key = self.project.key().to_owned();
        let created = self.with_store(|store| {
            let task = store.create_chat_task(&key)?;
            Ok((task, store.chat_tasks(&key)?))
        });
        match created {
            Ok((task, tasks)) => self.switch_task(task, Vec::new(), tasks),
            Err(error) => self.set_banner(format!("Could not create a task: {error:#}")),
        }
    }

    fn select_task(&mut self, id: &str) {
        let state = self.snapshot();
        if state.turn.is_some() || state.task.as_ref().is_some_and(|task| task.id == id) {
            return;
        }
        let key = self.project.key().to_owned();
        let loaded = self.with_store(|store| {
            let task = store.chat_task(id)?.context("That task no longer exists")?;
            store.select_chat_task(&key, id)?;
            Ok((task, store.chat_items(id)?, store.chat_tasks(&key)?))
        });
        match loaded {
            Ok((task, items, tasks)) => self.switch_task(task, items, tasks),
            Err(error) => self.set_banner(format!("Could not open that task: {error:#}")),
        }
    }

    fn switch_task(&mut self, task: ChatTask, items: Vec<ChatItem>, tasks: Vec<TaskSummary>) {
        self.permission = None;
        self.waiting_permissions.clear();
        self.update(|state| {
            state.task = Some(task);
            state.items = items;
            state.tasks = tasks;
            state.streaming = None;
            state.pending_permission = None;
            state.banner = None;
        });
        self.reopen_session();
    }

    fn set_model(&mut self, value: String) {
        let state = self.snapshot();
        let model = state.models.iter().find(|model| model.value == value);
        if value != DEFAULT_MODEL && model.is_none() {
            return self.set_banner(MODEL_UNAVAILABLE);
        }
        let mut prefs = state.prefs.clone();
        let effort_fits = prefs
            .effort
            .as_ref()
            .is_some_and(|effort| model.is_some_and(|model| model.efforts.contains(effort)));
        if !effort_fits {
            prefs.effort = None;
        }
        prefs.model = value;
        self.save_prefs(&prefs);
        self.update(|state| state.prefs = prefs);
    }

    fn set_effort(&mut self, value: Option<String>) {
        let state = self.snapshot();
        if let Some(effort) = &value {
            let listed = state
                .models
                .iter()
                .find(|model| model.value == state.prefs.model)
                .is_some_and(|model| model.efforts.contains(effort));
            if !listed {
                return self.set_banner(EFFORT_UNAVAILABLE);
            }
        }
        let mut prefs = state.prefs.clone();
        prefs.effort = value;
        self.save_prefs(&prefs);
        self.update(|state| state.prefs = prefs);
    }

    fn set_use_my_setup(&mut self, value: bool) {
        let state = self.snapshot();
        if state.turn.is_some() {
            return;
        }
        let mut prefs = state.prefs.clone();
        prefs.use_my_setup = value;
        self.save_prefs(&prefs);
        self.update(|state| state.prefs = prefs);
        self.reopen_session();
    }

    /// Interrupts a running turn (waiting up to 2 s for it to finish), then closes the
    /// session and the helper.
    fn shutdown(&mut self) {
        self.closing = true;
        if self.snapshot().turn.is_some() && self.client.is_some() {
            self.deny_permissions(true);
            if let Some(client) = &self.client {
                let _ = client.command(|id| Outgoing::Interrupt { id });
            }
            self.update(|state| state.stopping = true);
            let deadline = Instant::now() + SHUTDOWN_TURN_WAIT;
            while self.snapshot().turn.is_some()
                && self.client.is_some()
                && Instant::now() < deadline
            {
                let inbound = match self.queue.pop_front() {
                    Some(inbound) => inbound,
                    None => match self
                        .inbox
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(inbound) => inbound,
                        Err(_) => break,
                    },
                };
                if let Inbound::Helper {
                    generation,
                    received,
                } = inbound
                    && generation == self.generation
                {
                    self.helper(received);
                }
            }
            self.deny_permissions(false);
        }
        if let Some(client) = &self.client {
            let _ = client.command(|id| Outgoing::CloseSession { id });
            let _ = client.command(|id| Outgoing::Shutdown { id });
        }
        self.drop_client();
    }

    // ----- Persistence -----

    /// Saves the task and publishes it with the refreshed task list.
    fn save_task(&mut self, task: ChatTask) {
        let key = self.project.key().to_owned();
        let saved = self.with_store(|store| {
            store.save_chat_task(&task)?;
            store.chat_tasks(&key)
        });
        self.update(|state| {
            match saved {
                Ok(tasks) => state.tasks = tasks,
                Err(error) => state.banner = Some(format!("Could not save the task: {error:#}")),
            }
            state.task = Some(task);
        });
    }

    fn save_prefs(&self, prefs: &ChatPrefs) {
        if let Err(error) = self.with_store(|store| store.save_chat_prefs(prefs)) {
            self.set_banner(format!("Could not save the preferences: {error:#}"));
        }
    }

    /// Stores an item for the active task; a failure becomes the banner.
    fn store_item(&self, kind: ItemKind) -> Option<ChatItem> {
        let task_id = self.snapshot().task.as_ref()?.id.clone();
        match self.with_store(|store| store.append_chat_item(&task_id, &kind)) {
            Ok(item) => Some(item),
            Err(error) => {
                self.set_banner(format!("Could not save the chat: {error:#}"));
                None
            }
        }
    }

    fn append_item(&self, kind: ItemKind) {
        if let Some(item) = self.store_item(kind) {
            self.update(|state| state.items.push(item));
        }
    }
}

fn install_info(launch: &Launch) -> Option<InstallInfo> {
    let (node, helper) = (launch.node.as_ref()?, launch.helper.as_ref()?);
    Some(InstallInfo {
        node_path: node.path.display().to_string(),
        node_version: node.version.clone(),
        helper_dir: helper.dir.display().to_string(),
        bundled: helper.bundled,
    })
}

/// A task title: the first 60 characters.
fn title_from(text: &str) -> String {
    cut(text.trim(), TITLE_CHARS).trim_end().to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
