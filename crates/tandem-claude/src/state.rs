//! What the companion controller publishes for the UI, and what the UI sends it.

use crate::install::Launch;
use crate::protocol::ModelChoice;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use tandem_core::Store;
use tandem_core::chat::{ChatItem, ChatPrefs, ChatTask, TaskSummary};

/// How far Tandem got in reaching Claude.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    Starting,
    /// Node or the helper is missing, the protocol does not match, or Claude is switched off.
    Unavailable {
        reason: String,
        hint: String,
    },
    /// `hint` is the command that signs Claude Code in.
    SignedOut {
        hint: String,
    },
    /// Not a Claude subscription, or another provider.
    Refused {
        reason: String,
    },
    Ready {
        subscription: String,
        claude_code: String,
    },
    /// The helper or Claude Code stopped.
    Failed {
        message: String,
    },
}

/// Which Node and helper the current connection uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallInfo {
    pub node_path: String,
    pub node_version: String,
    pub helper_dir: String,
    pub bundled: bool,
}

/// What the running turn was started for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnKind {
    Chat,
}

/// An immutable snapshot of everything the chat panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatState {
    pub connection: Connection,
    pub install: Option<InstallInfo>,
    pub models: Vec<ModelChoice>,
    pub prefs: ChatPrefs,
    pub tasks: Vec<TaskSummary>,
    pub task: Option<ChatTask>,
    pub items: Vec<ChatItem>,
    /// Assistant text still arriving.
    pub streaming: Option<String>,
    /// `Some` while a turn runs.
    pub turn: Option<TurnKind>,
    pub stopping: bool,
    /// The id of the Permission item awaiting an answer.
    pub pending_permission: Option<i64>,
    /// The latest problem, shown above the composer.
    pub banner: Option<String>,
}

impl ChatState {
    /// The state before the project and its tasks are loaded.
    pub fn starting() -> ChatState {
        ChatState {
            connection: Connection::Starting,
            install: None,
            models: Vec::new(),
            prefs: ChatPrefs::default(),
            tasks: Vec::new(),
            task: None,
            items: Vec::new(),
            streaming: None,
            turn: None,
            stopping: false,
            pending_permission: None,
            banner: None,
        }
    }
}

/// What the UI asks the controller to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    NewTask,
    SelectTask(String),
    Send(String),
    Stop,
    Permission { allow: bool },
    SetModel(String),
    SetEffort(Option<String>),
    SetUseMySetup(bool),
    Reconnect,
    Shutdown,
}

/// What the controller needs to know from the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostQuestion {
    ReadBuffer { path: String },
    DirtyPaths,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAnswer {
    Buffer(Option<BufferText>),
    DirtyPaths(Vec<String>),
}

/// An open editor buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferText {
    pub text: String,
    pub dirty: bool,
}

/// A question for the UI thread; answer it through `reply`.
#[derive(Clone, Debug)]
pub struct HostAsk {
    pub question: HostQuestion,
    pub reply: SyncSender<HostAnswer>,
}

/// What the controller tells the UI.
#[derive(Clone, Debug)]
pub enum Signal {
    /// `Companion::state` has a new snapshot.
    Changed,
    Host(HostAsk),
}

pub struct CompanionConfig {
    pub project: PathBuf,
    pub store: Arc<Mutex<Store>>,
    /// `None` discovers Node and the helper with `Launch::discover()` at every connect.
    pub launch: Option<Launch>,
    /// Never start the helper (`TANDEM_CLAUDE_OFFLINE=1`).
    pub offline: bool,
}
