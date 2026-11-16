//! What the companion controller publishes for the UI, and what the UI sends it.

use crate::install::Launch;
use crate::protocol::ModelChoice;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use tandem_core::Store;
use tandem_core::chat::{
    ChangeSet, ChatItem, ChatPrefs, ChatTask, Guidance, HelpLevel, TakeoverScope, TaskSummary,
};

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

/// What the running turn was started for. Step numbers are indexes into the plan's steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnKind {
    Chat,
    /// Claude prepares guidance for `step` at `level`.
    Guidance {
        step: usize,
        level: HelpLevel,
    },
    /// Claude checks the user's work on `step`.
    Check {
        step: usize,
    },
    /// Claude checks the whole task against its "done when" criteria.
    FinalCheck,
    /// Claude does the work itself; `change_set` records what it changes.
    Takeover {
        scope: TakeoverScope,
        change_set: String,
    },
}

/// What the user asks Claude to do: the active step only, or every remaining step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeoverRequest {
    Step,
    Rest,
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
    /// Guidance for the active step at the current help level, if any.
    pub guidance: Option<Guidance>,
    /// The active task's takeover change sets, oldest first.
    pub change_sets: Vec<ChangeSet>,
    /// Tandem is getting a takeover ready: taking the snapshot of the project and starting
    /// the turn. The editor stays read-only, as it does while the takeover turn runs.
    pub preparing_takeover: bool,
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
            guidance: None,
            change_sets: Vec::new(),
            preparing_takeover: false,
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
    Permission {
        allow: bool,
    },
    SetModel(String),
    SetEffort(Option<String>),
    SetUseMySetup(bool),
    /// "I'll write it" on the plan card: start the guided build.
    ChooseGuided,
    /// Ask Claude again for guidance on the active step.
    GuideStep,
    CheckMyWork,
    SetHelpLevel {
        language: String,
        level: HelpLevel,
    },
    /// Let Claude do the work. `saved` lists the files the UI saved just before.
    Takeover {
        request: TakeoverRequest,
        saved: Vec<String>,
    },
    /// Accept Claude's change to `path`, or to every undecided file when `None`.
    Keep {
        change_set: String,
        path: Option<String>,
    },
    /// Restore the file from before Claude's change, or every undecided file when `None`.
    Undo {
        change_set: String,
        path: Option<String>,
    },
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
