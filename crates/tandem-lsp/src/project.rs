use crate::companion::{MERGED, merge_replies, relay_request, relay_response};
use crate::legacy::read_frame;
use crate::utf16_position;
use anyhow::{Context, Result, anyhow, bail, ensure};
use lsp_types::{Diagnostic, Position, TextEdit};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tandem_core::languages::language_id_for_path;

/// How to launch one language server and what to tell it. Built by `rust_server`, `python_server`, ...
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Shown in health text: "rust-analyzer", "Pyright", ...
    pub name: String,
    /// Health detail once the server is ready.
    pub ready_detail: String,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Directories put on PATH before /usr/bin:/bin:/usr/sbin:/sbin.
    pub path: Vec<PathBuf>,
    /// Set after the HOME/USER/LOGNAME/LANG/LC_ALL/TMPDIR allow-list.
    pub env: Vec<(String, OsString)>,
    pub project: PathBuf,
    /// Tandem-owned cache folder, created before start; never inside the project.
    pub cache: PathBuf,
    pub initialization_options: Value,
    /// Answers workspace/configuration; also sent as didChangeConfiguration after `initialized` when `push_configuration`.
    pub configuration: Value,
    pub push_configuration: bool,
    /// Send every open document again with a new version whenever the workspace sequence changes (Pyright).
    pub resend_on_change: bool,
    /// Merged over client_capabilities(), objects key by key (Swift asks for dynamic diagnostic registration).
    pub capabilities: Value,
    pub diagnostics: DiagnosticMode,
    /// How long `initialize` may take before the start fails.
    pub start_timeout: Duration,
    /// How long any other request may take before the server is restarted.
    pub request_timeout: Duration,
    /// A second server run beside this one as a pair (Vue's TypeScript companion): it receives the
    /// same documents, answers the tsserver requests this server relays, and its answers are merged
    /// with this server's. Either exiting restarts both.
    pub companion: Option<Box<ServerConfig>>,
}
impl ServerConfig {
    /// Defaults: ready_detail "{name} ready", no args/path/env, initialization_options null,
    /// configuration {}, push_configuration false, resend_on_change false, capabilities {},
    /// diagnostics Versioned, start and request timeouts 20 s, no companion.
    pub fn new(
        name: impl Into<String>,
        program: impl Into<PathBuf>,
        project: impl Into<PathBuf>,
        cache: impl Into<PathBuf>,
    ) -> Self {
        let name = name.into();
        Self {
            ready_detail: format!("{name} ready"),
            name,
            program: program.into(),
            args: vec![],
            path: vec![],
            env: vec![],
            project: project.into(),
            cache: cache.into(),
            initialization_options: Value::Null,
            configuration: json!({}),
            push_configuration: false,
            resend_on_change: false,
            capabilities: json!({}),
            diagnostics: DiagnosticMode::Versioned,
            start_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(20),
            companion: None,
        }
    }
}

/// How a session answers `textDocument/diagnostic` for a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticMode {
    /// Pull when `diagnosticProvider` is advertised, otherwise only pushes carrying the current
    /// document version (rust-analyzer, Pyright: today's behaviour).
    Versioned,
    /// Pull when advertised or dynamically registered. Otherwise the latest push for the file,
    /// with or without a version, once the server has been quiet for SETTLE since the file's
    /// last change and its last push.
    Settled,
}
/// How long a `Settled` server must stay quiet before its latest push counts as its answer.
const SETTLE: Duration = Duration::from_millis(500);

/// PATH for a server: the configured directories, then the system ones. Never an empty entry,
/// which would put the project folder (the server's working directory) on PATH.
fn search_path(directories: &[PathBuf]) -> OsString {
    let mut path = OsString::new();
    for directory in directories {
        path.push(directory);
        path.push(":");
    }
    path.push("/usr/bin:/bin:/usr/sbin:/sbin");
    path
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageDocument {
    pub path: PathBuf,
    pub instance: String,
    pub text: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct LanguageStamp {
    pub sequence: u64,
    pub documents: Vec<LanguageDocument>,
}

#[derive(Clone, Debug)]
pub struct LanguageReply {
    pub stamp: LanguageStamp,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct LanguageHealth {
    pub state: String,
    pub detail: String,
    pub restarts: u32,
}

struct Shared {
    documents: BTreeMap<PathBuf, LanguageDocument>,
    sequence: u64,
    health: LanguageHealth,
    capabilities: Value,
}
struct Request {
    method: String,
    file: PathBuf,
    params: Value,
    stamp: LanguageStamp,
    reply: async_channel::Sender<Result<LanguageReply>>,
    cancelled: Arc<AtomicBool>,
}
enum Work {
    Request(Request),
    Restart,
    Stop,
}
struct Inner {
    sender: mpsc::SyncSender<Work>,
    shared: Arc<Mutex<Shared>>,
    stopped: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct LanguageManager(Arc<Inner>);

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = self.sender.try_send(Work::Stop);
    }
}

impl LanguageManager {
    pub fn start(config: ServerConfig) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            documents: BTreeMap::new(),
            sequence: 0,
            health: LanguageHealth {
                state: "starting".into(),
                detail: format!("Starting {}", config.name),
                restarts: 0,
            },
            capabilities: Value::Null,
        }));
        let stopped = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(32);
        let worker_shared = shared.clone();
        let worker_stop = stopped.clone();
        std::thread::spawn(move || worker(config, worker_shared, worker_stop, receiver));
        Self(Arc::new(Inner {
            sender,
            shared,
            stopped,
        }))
    }
    pub fn set_documents(&self, documents: Vec<LanguageDocument>) {
        let documents = documents.into_iter().map(|d| (d.path.clone(), d)).collect();
        let mut shared = self.0.shared.lock().unwrap();
        if shared.documents != documents {
            shared.documents = documents;
            shared.sequence += 1;
        }
    }
    pub fn invalidate(&self) {
        self.0.shared.lock().unwrap().sequence += 1;
    }
    pub fn health(&self) -> LanguageHealth {
        self.0.shared.lock().unwrap().health.clone()
    }
    /// The running server's capabilities; unknown before it is ready and after it stops.
    pub fn capabilities(&self) -> crate::features::capabilities::ServerFeatures {
        crate::features::capabilities::ServerFeatures(
            self.0.shared.lock().unwrap().capabilities.clone(),
        )
    }
    pub fn is_current(&self, stamp: &LanguageStamp) -> bool {
        !self.0.stopped.load(Ordering::Relaxed)
            && self.0.shared.lock().unwrap().sequence == stamp.sequence
    }
    pub fn restart(&self) {
        let _ = self.0.sender.try_send(Work::Restart);
    }
    pub fn stop(&self) {
        self.0.stopped.store(true, Ordering::Relaxed);
        let _ = self.0.sender.try_send(Work::Stop);
    }

    pub async fn request(
        &self,
        method: &str,
        file: &Path,
        instance: &str,
        text: &str,
        params: Value,
    ) -> Result<LanguageReply> {
        ensure!(
            !self.0.stopped.load(Ordering::Relaxed),
            "Language tools are stopped"
        );
        let stamp = {
            let shared = self.0.shared.lock().unwrap();
            let doc = shared
                .documents
                .get(file)
                .context("Document is no longer open")?;
            ensure!(
                doc.instance == instance,
                "Document was reopened; request is stale"
            );
            ensure!(
                doc.text.as_ref() == text,
                "Language request superseded or buffer synchronization pending"
            );
            LanguageStamp {
                sequence: shared.sequence,
                documents: shared.documents.values().cloned().collect(),
            }
        };
        let (reply, receiver) = async_channel::bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(cancelled.clone());
        self.0
            .sender
            .try_send(Work::Request(Request {
                method: method.into(),
                file: file.to_path_buf(),
                params,
                stamp,
                reply,
                cancelled,
            }))
            .map_err(|_| anyhow!("Language request queue is full or stopped"))?;
        receiver.recv().await.context("Language worker stopped")?
    }
}

fn health(shared: &Mutex<Shared>, state: &str, detail: impl Into<String>, restarts: u32) {
    shared.lock().unwrap().health = LanguageHealth {
        state: state.into(),
        detail: detail.into(),
        restarts,
    };
}

fn worker(
    config: ServerConfig,
    shared: Arc<Mutex<Shared>>,
    stopped: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Work>,
) {
    let mut session: Option<ProjectSession> = None;
    let mut retries = 0;
    let mut next_restart = Instant::now();
    while !stopped.load(Ordering::Relaxed) {
        if session
            .as_mut()
            .is_some_and(|s| s.closed || s.child.try_wait().ok().flatten().is_some())
        {
            session = None;
            retries += 1;
            next_restart = Instant::now() + Duration::from_secs(retries.min(3) as u64);
            shared.lock().unwrap().capabilities = Value::Null;
            health(
                &shared,
                "restarting",
                "Language server exited; current diagnostics are unavailable",
                retries,
            );
        }
        if session.is_none() && retries < 3 && Instant::now() >= next_restart {
            match ProjectSession::start(&config, &stopped) {
                Ok(mut started) => {
                    let documents = shared
                        .lock()
                        .unwrap()
                        .documents
                        .values()
                        .cloned()
                        .collect::<Vec<_>>();
                    let sequence = shared.lock().unwrap().sequence;
                    if started.sync_documents(&documents, sequence).is_ok() {
                        shared.lock().unwrap().capabilities = started.capabilities.clone();
                        session = Some(started);
                        health(&shared, "ready", config.ready_detail.as_str(), retries);
                    } else {
                        retries += 1;
                        next_restart = Instant::now() + Duration::from_secs(1);
                    }
                }
                Err(error) => {
                    retries += 1;
                    next_restart = Instant::now() + Duration::from_secs(retries as u64);
                    shared.lock().unwrap().capabilities = Value::Null;
                    health(
                        &shared,
                        if retries < 3 { "restarting" } else { "failed" },
                        error.to_string(),
                        retries,
                    );
                }
            }
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Work::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Work::Restart) => {
                session = None;
                retries = 0;
                next_restart = Instant::now();
                shared.lock().unwrap().capabilities = Value::Null;
                health(&shared, "starting", "Restarting language tools", 0);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(session) = &mut session {
                    let documents = shared
                        .lock()
                        .unwrap()
                        .documents
                        .values()
                        .cloned()
                        .collect::<Vec<_>>();
                    let sequence = shared.lock().unwrap().sequence;
                    if session.sync_documents(&documents, sequence).is_err()
                        || session.drain().is_err()
                    {
                        session.closed = true;
                    }
                    if session.closed {
                        session.stop();
                    }
                }
            }
            Ok(Work::Request(request)) => {
                if request.cancelled.load(Ordering::Relaxed) || request.reply.is_closed() {
                    continue;
                }
                let result = (|| -> Result<LanguageReply> {
                    ensure!(
                        shared.lock().unwrap().sequence == request.stamp.sequence,
                        "Language request superseded by edits"
                    );
                    let session = session
                        .as_mut()
                        .context("Language server is not ready; retry when connected")?;
                    session.sync_documents(&request.stamp.documents, request.stamp.sequence)?;
                    let mut params = request.params.clone();
                    if request.method.starts_with("textDocument/") {
                        params["textDocument"] = json!({"uri": file_uri(&request.file)?});
                    }
                    let value = if request.method == "workspace/executeCommand" {
                        // The command was chosen by the user, so the edits it asks for while
                        // it runs are collected and returned with its result.
                        session.collected_edits = Some(Vec::new());
                        let value = session.answer(
                            &request.method,
                            &request.file,
                            params,
                            &request.cancelled,
                            &stopped,
                        );
                        let edits = session.collected_edits.take().unwrap_or_default();
                        json!({"result": value?, "edits": edits})
                    } else {
                        session.answer(
                            &request.method,
                            &request.file,
                            params,
                            &request.cancelled,
                            &stopped,
                        )?
                    };
                    ensure!(
                        shared.lock().unwrap().sequence == request.stamp.sequence,
                        "Language response superseded by edits"
                    );
                    Ok(LanguageReply {
                        stamp: request.stamp.clone(),
                        value,
                    })
                })();
                let _ = request.reply.try_send(result);
            }
        }
    }
    drop(session);
    shared.lock().unwrap().capabilities = Value::Null;
    health(&shared, "stopped", "Language tools stopped", retries);
}

/// A request the server did not answer in time; the session is closed and restarts.
#[derive(Debug)]
struct TimedOut;
impl std::fmt::Display for TimedOut {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Language request timed out; restarting server")
    }
}
impl std::error::Error for TimedOut {}

struct OpenDocument {
    text: Arc<str>,
    instance: String,
    version: i32,
    /// When didOpen or the latest didChange was sent.
    changed: Instant,
}
struct ProjectSession {
    child: Child,
    input: Option<ChildStdin>,
    incoming: mpsc::Receiver<Result<Value>>,
    next_id: u64,
    documents: BTreeMap<PathBuf, OpenDocument>,
    pull_diagnostics: bool,
    /// Whether `initialize` advertised `diagnosticProvider`; restored when a dynamic registration is withdrawn.
    static_pull: bool,
    diagnostic_mode: DiagnosticMode,
    request_timeout: Duration,
    closed: bool,
    resend_on_change: bool,
    configuration: Value,
    diagnostic_versions: BTreeMap<PathBuf, (i32, Value)>,
    /// `Settled` only: the latest usable push for each open document and when it arrived.
    latest_push: BTreeMap<PathBuf, (Value, Instant)>,
    version_counter: i32,
    synced_sequence: u64,
    /// The server paired with this one (Vue's TypeScript companion), started and stopped with it.
    companion: Option<Box<ProjectSession>>,
    /// On a companion: the main server's tsserver request id for each forwarded request's id.
    relay: BTreeMap<u64, Value>,
    /// On a companion: tsserver/response params waiting to be sent to the main server.
    outbox: Vec<Value>,
    capabilities: Value,
    /// The `workspace/applyEdit` edits received while a `workspace/executeCommand` runs;
    /// `None` when no command runs, and then such edits are denied.
    collected_edits: Option<Vec<Value>>,
}

impl ProjectSession {
    fn start(config: &ServerConfig, stopped: &AtomicBool) -> Result<Self> {
        std::fs::create_dir_all(&config.cache)?;
        // The companion is ready before the main server can relay its first tsserver request.
        let companion = config
            .companion
            .as_deref()
            .map(|companion| Self::start(companion, stopped).map(Box::new))
            .transpose()?;
        let mut command = Command::new(&config.program);
        command.args(&config.args);
        command.env_clear();
        for name in ["HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("PATH", search_path(&config.path));
        for (name, value) in &config.env {
            command.env(name, value);
        }
        command
            .current_dir(&config.project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            unsafe {
                command.pre_exec(|| {
                    let mut mask: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut mask);
                    if libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command
            .spawn()
            .context("Could not start the installed language server")?;
        let input = child.stdin.take().context("LSP input missing")?;
        let output = child.stdout.take().context("LSP output missing")?;
        let (sender, incoming) = mpsc::sync_channel(128);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(value)) => {
                        if sender.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        let mut session = Self {
            child,
            input: Some(input),
            incoming,
            next_id: 1,
            documents: BTreeMap::new(),
            pull_diagnostics: false,
            static_pull: false,
            diagnostic_mode: config.diagnostics,
            request_timeout: config.request_timeout,
            closed: false,
            resend_on_change: config.resend_on_change,
            configuration: config.configuration.clone(),
            diagnostic_versions: BTreeMap::new(),
            latest_push: BTreeMap::new(),
            version_counter: 0,
            synced_sequence: u64::MAX,
            companion,
            relay: BTreeMap::new(),
            outbox: vec![],
            capabilities: Value::Null,
            collected_edits: None,
        };
        let mut capabilities = client_capabilities();
        merge_json(&mut capabilities, &config.capabilities);
        let mut initialize = json!({"processId":std::process::id(),"rootUri":file_uri(&config.project)?,"rootPath":config.project,
            "capabilities":capabilities});
        if !config.initialization_options.is_null() {
            initialize["initializationOptions"] = config.initialization_options.clone();
        }
        let init = session
            .request(
                "initialize",
                initialize,
                config.start_timeout,
                &AtomicBool::new(false),
                stopped,
            )
            .map_err(|error| match error.downcast::<TimedOut>() {
                Ok(TimedOut) => anyhow!(
                    "{} did not start within {} s",
                    config.name,
                    config.start_timeout.as_secs_f32()
                ),
                Err(error) => error,
            })?;
        session.capabilities = init.get("capabilities").cloned().unwrap_or(Value::Null);
        ensure!(
            init.pointer("/capabilities/positionEncoding")
                .is_none_or(|v| v == "utf-16"),
            "Unsupported language-server position encoding"
        );
        session.static_pull = init
            .pointer("/capabilities/diagnosticProvider")
            .is_some_and(|v| !v.is_null());
        session.pull_diagnostics = session.static_pull;
        session.notify("initialized", json!({}))?;
        if config.push_configuration {
            session.notify(
                "workspace/didChangeConfiguration",
                json!({"settings":session.configuration}),
            )?;
        }
        Ok(session)
    }
    fn send(&mut self, value: Value) -> Result<()> {
        let body = serde_json::to_vec(&value)?;
        let input = self.input.as_mut().context("LSP input closed")?;
        write!(input, "Content-Length: {}\r\n\r\n", body.len())?;
        input.write_all(&body)?;
        input.flush()?;
        Ok(())
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    fn sync_documents(&mut self, documents: &[LanguageDocument], sequence: u64) -> Result<()> {
        if let Some(companion) = &mut self.companion {
            companion.sync_documents(documents, sequence)?;
        }
        let changed = self.documents.len() != documents.len()
            || documents.iter().any(|d| {
                self.documents
                    .get(&d.path)
                    .is_none_or(|old| old.text != d.text || old.instance != d.instance)
            });
        let force_versions = self.resend_on_change && (changed || self.synced_sequence != sequence);
        if changed || force_versions {
            self.diagnostic_versions.clear();
        }
        self.synced_sequence = sequence;
        let closed: Vec<_> = self
            .documents
            .keys()
            .filter(|path| !documents.iter().any(|d| &d.path == *path))
            .cloned()
            .collect();
        for path in closed {
            self.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":file_uri(&path)?}}),
            )?;
            self.documents.remove(&path);
            self.latest_push.remove(&path);
        }
        for document in documents {
            if self
                .documents
                .get(&document.path)
                .is_some_and(|old| old.instance != document.instance)
            {
                self.notify(
                    "textDocument/didClose",
                    json!({"textDocument":{"uri":file_uri(&document.path)?}}),
                )?;
                self.documents.remove(&document.path);
                self.latest_push.remove(&document.path);
            }
            match self.documents.get_mut(&document.path) {
                Some(old) if old.text != document.text || force_versions => {
                    self.version_counter = self
                        .version_counter
                        .checked_add(1)
                        .context("Language document version exhausted")?;
                    old.version = self.version_counter;
                    old.text = document.text.clone();
                    old.changed = Instant::now();
                    let version = old.version;
                    self.notify("textDocument/didChange",json!({"textDocument":{"uri":file_uri(&document.path)?,"version":version},"contentChanges":[{"text":document.text.as_ref()}]}))?;
                }
                None => {
                    self.version_counter = self
                        .version_counter
                        .checked_add(1)
                        .context("Language document version exhausted")?;
                    self.notify("textDocument/didOpen",json!({"textDocument":{"uri":file_uri(&document.path)?,"languageId":language_id_for_path(&document.path.to_string_lossy()),"version":self.version_counter,"text":document.text.as_ref()}}))?;
                    self.documents.insert(
                        document.path.clone(),
                        OpenDocument {
                            text: document.text.clone(),
                            instance: document.instance.clone(),
                            version: self.version_counter,
                            changed: Instant::now(),
                        },
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Answers `textDocument/diagnostic`: the server's pull when it offers one, otherwise
    /// the pushes the diagnostic mode accepts.
    fn diagnostics(
        &mut self,
        path: &Path,
        params: Value,
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        let deadline = Instant::now() + self.request_timeout.min(Duration::from_secs(10));
        loop {
            self.pump_companion()?;
            // A dynamic registration can arrive while waiting.
            if self.pull_diagnostics {
                return self.request(
                    "textDocument/diagnostic",
                    params,
                    self.request_timeout,
                    cancel,
                    stop,
                );
            }
            let document = self.documents.get(path).context("Document closed")?;
            if let Some((received, items)) = self.diagnostic_versions.get(path)
                && *received == document.version
            {
                return Ok(json!({"kind":"full","items":items}));
            }
            if self.diagnostic_mode == DiagnosticMode::Settled {
                let push = self.latest_push.get(path);
                let quiet_since =
                    push.map_or(document.changed, |(_, at)| document.changed.max(*at));
                if quiet_since.elapsed() >= SETTLE {
                    let items = push.map_or(json!([]), |(items, _)| items.clone());
                    return Ok(json!({"kind":"full","items":items}));
                }
            }
            ensure!(
                !cancel.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed),
                "Diagnostic request cancelled"
            );
            ensure!(
                Instant::now() < deadline,
                "Version-associated diagnostics are not yet available"
            );
            match self.incoming.recv_timeout(Duration::from_millis(50)) {
                Ok(message) => self.handle_message(message?)?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    bail!("Language server disconnected");
                }
            }
        }
    }
    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        let id = self.start_request(method, params)?;
        let deadline = Instant::now() + timeout;
        loop {
            // The server may be waiting for a tsserver request relayed to the companion.
            self.pump_companion()?;
            if Instant::now() >= deadline {
                self.closed = true;
                return Err(TimedOut.into());
            }
            if cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                let _ = self.notify("$/cancelRequest", json!({"id":id}));
                bail!("Language request cancelled");
            }
            let message = match self.incoming.recv_timeout(Duration::from_millis(50)) {
                Ok(message) => message?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    bail!("Language server disconnected");
                }
            };
            if message["id"] == id && message.get("method").is_none() {
                if let Some(error) = message.get("error") {
                    bail!("Language request {method} failed (code {})", error["code"]);
                }
                return message
                    .get("result")
                    .cloned()
                    .context("Language result missing");
            }
            self.handle_message(message)?;
        }
    }
    fn drain(&mut self) -> Result<()> {
        for _ in 0..128 {
            match self.incoming.try_recv() {
                Ok(message) => self.handle_message(message?)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.closed = true;
                    break;
                }
            }
        }
        self.pump_companion()
    }
    fn handle_message(&mut self, message: Value) -> Result<()> {
        if self.relay(&message)? {
            return Ok(());
        }
        if message["method"] == "textDocument/publishDiagnostics"
            && let Some(uri) = message["params"]["uri"].as_str()
            && let Ok(path) = uri_path(uri)
            && let Some(doc) = self.documents.get(&path)
        {
            let version = message["params"]["version"].as_i64();
            let current = version == Some(i64::from(doc.version));
            let diagnostics = &message["params"]["diagnostics"];
            if current {
                self.diagnostic_versions
                    .insert(path.clone(), (doc.version, diagnostics.clone()));
            }
            if self.diagnostic_mode == DiagnosticMode::Settled && (current || version.is_none()) {
                self.latest_push
                    .insert(path, (diagnostics.clone(), Instant::now()));
            }
        }
        if message.get("method").is_some() && message.get("id").is_some() {
            match message["method"].as_str() {
                    Some("workspace/applyEdit")=>match &mut self.collected_edits {
                        Some(list)=>{
                            list.push(message["params"]["edit"].clone());
                            self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":{"applied":true}}))?
                        }
                        None=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":{"applied":false,"failureReason":"Source edits require an explicit editor action"}}))?,
                    },
                    Some("window/workDoneProgress/create")=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?,
                    Some("window/showDocument")=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":false}}))?,
                    Some("client/registerCapability")=>{
                        if self.diagnostic_mode==DiagnosticMode::Settled&&names_diagnostic_pull(&message["params"]["registrations"]){
                            self.pull_diagnostics=true;
                        }
                        self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?;
                    },
                    Some("client/unregisterCapability")=>{
                        // The spec spells it "unregisterations".
                        if self.diagnostic_mode==DiagnosticMode::Settled&&names_diagnostic_pull(&message["params"]["unregisterations"]){
                            self.pull_diagnostics=self.static_pull;
                        }
                        self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?;
                    },
                    Some("workspace/configuration")=>{
                        let values=message["params"]["items"].as_array().map(|items|items.iter().map(|item|item["section"].as_str().map(|s|s.split('.').fold(&self.configuration,|value,key|&value[key]).clone()).unwrap_or_else(||self.configuration.clone())).collect::<Vec<_>>()).unwrap_or_default();
                        self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":values}))?;
                    },
                    _=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?,
                }
        }
        Ok(())
    }
    fn stop(&mut self) {
        if let Some(companion) = &mut self.companion {
            companion.stop();
        }
        if self.input.take().is_none() {
            return;
        }
        if self.child.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGTERM);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.closed = true;
    }
}
impl Drop for ProjectSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Answering Tandem's requests, and running a companion beside the main server.
impl ProjectSession {
    /// Answers one request from Tandem. With a companion, the methods in MERGED are asked of both
    /// servers and their answers merged; a companion error leaves the main server's answer alone.
    fn answer(
        &mut self,
        method: &str,
        file: &Path,
        params: Value,
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        if self.companion.is_none() || !MERGED.contains(&method) {
            return self.answer_alone(method, file, params, cancel, stop);
        }
        let main = self.answer_alone(method, file, params.clone(), cancel, stop)?;
        let companion = self
            .companion
            .as_mut()
            .map(|companion| companion.answer_alone(method, file, params, cancel, stop));
        // Answers to relayed requests the companion received while answering.
        self.pump_companion()?;
        Ok(match companion {
            Some(Ok(companion)) => merge_replies(method, main, companion),
            _ => main,
        })
    }
    /// This server's own answer: diagnostics through `diagnostics`, anything else through `request`.
    fn answer_alone(
        &mut self,
        method: &str,
        file: &Path,
        params: Value,
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        if method == "textDocument/diagnostic" {
            self.diagnostics(file, params, cancel, stop)
        } else {
            self.request(method, params, self.request_timeout, cancel, stop)
        }
    }
    /// Sends a request without waiting for its answer and returns its id.
    fn start_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        Ok(id)
    }
    /// Relays tsserver requests between the main server and its companion; returns whether
    /// `message` was part of a relay. On the main session, a `tsserver/request` notification is
    /// forwarded to the companion as typescript-language-server's `typescript.tsserverRequest`
    /// command, or answered at once with a null body when there is no companion to ask. On the
    /// companion, the answer to a forwarded request waits in the outbox for `pump_companion`.
    fn relay(&mut self, message: &Value) -> Result<bool> {
        if message["method"] == "tsserver/request" && message.get("id").is_none() {
            let Some((id, command, args)) = relay_request(&message["params"]) else {
                return Ok(true);
            };
            let forwarded = self.companion.as_mut().is_some_and(|companion| {
                let params = json!({"command":"typescript.tsserverRequest",
                    "arguments":[command,args,{"isAsync":true,"lowPriority":true}]});
                companion
                    .start_request("workspace/executeCommand", params)
                    .map(|forwarded| companion.relay.insert(forwarded, id.clone()))
                    .is_ok()
            });
            if !forwarded {
                self.notify("tsserver/response", relay_response(id, Value::Null))?;
            }
            return Ok(true);
        }
        if message.get("method").is_none()
            && let Some(id) = message["id"].as_u64().and_then(|id| self.relay.remove(&id))
        {
            // An error answer has no result, and so no body.
            let body = message["result"]["body"].clone();
            self.outbox.push(relay_response(id, body));
            return Ok(true);
        }
        Ok(false)
    }
    /// Handles what the companion sent, without waiting, then sends its answers to relayed
    /// requests to the main server. A companion that exited or broke closes this session too, so
    /// the pair restarts together.
    fn pump_companion(&mut self) -> Result<()> {
        let Some(companion) = self.companion.as_mut() else {
            return Ok(());
        };
        if companion.drain().is_err() {
            companion.closed = true;
        }
        let exited = companion.closed || companion.child.try_wait().ok().flatten().is_some();
        let answers = std::mem::take(&mut companion.outbox);
        if exited {
            self.closed = true;
        }
        for answer in answers {
            self.notify("tsserver/response", answer)?;
        }
        Ok(())
    }
}

/// What Tandem tells a language server it supports. Hover text is asked for as Markdown,
/// so that signatures arrive in fenced code blocks the editor can highlight.
fn client_capabilities() -> Value {
    let mut capabilities = json!({"general":{"positionEncodings":["utf-16"]},"workspace":{"configuration":true},
        "textDocument":{"completion":{"completionItem":{"snippetSupport":false}},"definition":{"linkSupport":true},"hover":{"contentFormat":["markdown","plaintext"]},"publishDiagnostics":{"versionSupport":true},"diagnostic":{},"documentSymbol":{"hierarchicalDocumentSymbolSupport":true}}});
    crate::features::capabilities::extend_client_capabilities(&mut capabilities);
    capabilities
}
/// Whether a (un)registration list names pull diagnostics.
fn names_diagnostic_pull(registrations: &Value) -> bool {
    registrations.as_array().is_some_and(|registrations| {
        registrations
            .iter()
            .any(|registration| registration["method"] == "textDocument/diagnostic")
    })
}
/// Merges `overlay` into `base`: objects key by key, any other overlay value replaces the base value.
fn merge_json(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json(base.entry(key.as_str()).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}
pub fn file_uri(path: &Path) -> Result<String> {
    Ok(url::Url::from_file_path(path)
        .map_err(|_| anyhow!("Invalid local file URI"))?
        .to_string())
}
pub fn uri_path(uri: &str) -> Result<PathBuf> {
    url::Url::parse(uri)?
        .to_file_path()
        .map_err(|_| anyhow!("Only local file definitions are supported"))
}
pub fn position_params(text: &str, byte: usize) -> Result<Value> {
    Ok(json!({"position":utf16_position(text,byte)?}))
}
pub fn diagnostic_items(value: Value) -> Result<Vec<Diagnostic>> {
    Ok(serde_json::from_value(
        value.get("items").cloned().unwrap_or(json!([])),
    )?)
}

pub fn utf16_offset(text: &str, position: Position) -> Result<usize> {
    if text.is_empty() && position == Position::new(0, 0) {
        return Ok(0);
    }
    let mut offset = 0usize;
    let line = text.split_inclusive('\n').nth(position.line as usize);
    if line.is_none()
        && position.line as usize == text.matches('\n').count()
        && text.ends_with('\n')
        && position.character == 0
    {
        return Ok(text.len());
    }
    let line = line.context("Edit refers to a missing line")?;
    for part in text.split_inclusive('\n').take(position.line as usize) {
        offset += part.len();
    }
    let line = line.trim_end_matches(['\r', '\n']);
    let mut units = 0;
    for ch in line.chars() {
        if units == position.character {
            return Ok(offset);
        }
        units += ch.len_utf16() as u32;
        ensure!(
            units <= position.character,
            "Edit splits a UTF-16 surrogate pair"
        );
        offset += ch.len_utf8();
    }
    ensure!(units == position.character, "Edit extends past line end");
    Ok(offset)
}
pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> Result<String> {
    let mut ranges = edits
        .iter()
        .map(|edit| {
            Ok((
                utf16_offset(text, edit.range.start)?,
                utf16_offset(text, edit.range.end)?,
                edit.new_text.as_str(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    ranges.sort_by_key(|r| (r.0, r.1));
    let mut end = 0;
    let mut previous = None;
    for (start, next, _) in &ranges {
        ensure!(
            start <= next && *start >= end && previous != Some((*start, *next)),
            "Overlapping or reversed language edits"
        );
        end = *next;
        previous = Some((*start, *next));
    }
    let mut result = text.to_string();
    for (start, end, replacement) in ranges.into_iter().rev() {
        result.replace_range(start..end, replacement);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_is_requested_as_markdown_with_a_plain_text_fallback() {
        let capabilities = client_capabilities();
        assert_eq!(
            capabilities.pointer("/textDocument/hover/contentFormat"),
            Some(&json!(["markdown", "plaintext"]))
        );
        // The capabilities the other language features rely on are still declared.
        assert_eq!(
            capabilities.pointer("/general/positionEncodings"),
            Some(&json!(["utf-16"]))
        );
        assert_eq!(
            capabilities.pointer("/textDocument/definition/linkSupport"),
            Some(&json!(true))
        );
        assert_eq!(
            capabilities.pointer("/textDocument/completion/completionItem/snippetSupport"),
            Some(&json!(true))
        );
    }

    #[test]
    fn capability_overlays_merge_objects_and_replace_other_values() {
        let mut capabilities = json!({"textDocument":{"diagnostic":{},"hover":{"contentFormat":["markdown"]}},"general":true});
        merge_json(
            &mut capabilities,
            &json!({"textDocument":{"diagnostic":{"dynamicRegistration":true},"hover":{"contentFormat":["plaintext"]}},"general":{"a":1},"window":{"showDocument":{"support":false}}}),
        );
        assert_eq!(
            capabilities,
            json!({"textDocument":{"diagnostic":{"dynamicRegistration":true},"hover":{"contentFormat":["plaintext"]}},"general":{"a":1},"window":{"showDocument":{"support":false}}})
        );
        // The default overlay changes nothing.
        let mut unchanged = client_capabilities();
        merge_json(&mut unchanged, &json!({}));
        assert_eq!(unchanged, client_capabilities());
    }

    #[test]
    fn search_path_puts_configured_folders_before_the_system_ones() {
        assert_eq!(
            search_path(&[PathBuf::from("/a/bin"), PathBuf::from("/b/bin")]),
            OsString::from("/a/bin:/b/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        );
        // No leading empty entry, which would mean the server's working directory.
        assert_eq!(
            search_path(&[]),
            OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")
        );
    }

    #[test]
    fn formatting_rejects_invalid_unicode_ranges_and_overlap() {
        let text = "a🦀b\n";
        let edit = TextEdit {
            range: lsp_types::Range::new(Position::new(0, 1), Position::new(0, 3)),
            new_text: "x".into(),
        };
        assert_eq!(
            apply_text_edits(text, std::slice::from_ref(&edit)).unwrap(),
            "axb\n"
        );
        let mut invalid = edit.clone();
        invalid.range.end.character = 2;
        assert!(apply_text_edits(text, &[invalid]).is_err());
        assert!(apply_text_edits(text, &[edit.clone(), edit]).is_err());
    }
}
