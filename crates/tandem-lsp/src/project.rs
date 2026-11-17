use crate::legacy::read_frame;
use crate::utf16_position;
use anyhow::{Context, Result, anyhow, bail, ensure};
use lsp_types::{Diagnostic, Position, TextEdit};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub binary: PathBuf,
    pub toolchain: PathBuf,
    pub project: PathBuf,
    pub cache: PathBuf,
    pub python: Option<PythonServer>,
}
#[derive(Clone, Debug)]
pub struct PythonServer {
    pub script: PathBuf,
    pub interpreter: PathBuf,
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
                detail: if config.python.is_some() {
                    "Starting Pyright"
                } else {
                    "Starting rust-analyzer"
                }
                .into(),
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
                        health(
                            &shared,
                            "ready",
                            if config.python.is_some() {
                                "Pyright ready · selected local interpreter"
                            } else {
                                "rust-analyzer ready · build scripts and proc macros off"
                            },
                            retries,
                        );
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
                    let value = if request.method == "textDocument/diagnostic"
                        && !session.pull_diagnostics
                    {
                        session.versioned_diagnostics(
                            &request.file,
                            &request.cancelled,
                            &stopped,
                        )?
                    } else if request.method == "workspace/executeCommand" {
                        // The command was chosen by the user, so the edits it asks for while
                        // it runs are collected and returned with its result.
                        session.collected_edits = Some(Vec::new());
                        let value =
                            session.request(&request.method, params, &request.cancelled, &stopped);
                        let edits = session.collected_edits.take().unwrap_or_default();
                        json!({"result": value?, "edits": edits})
                    } else {
                        session.request(&request.method, params, &request.cancelled, &stopped)?
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

struct OpenDocument {
    text: Arc<str>,
    instance: String,
    version: i32,
}
struct ProjectSession {
    child: Child,
    input: Option<ChildStdin>,
    incoming: mpsc::Receiver<Result<Value>>,
    next_id: u64,
    documents: BTreeMap<PathBuf, OpenDocument>,
    pull_diagnostics: bool,
    closed: bool,
    language: &'static str,
    configuration: Value,
    diagnostic_versions: BTreeMap<PathBuf, (i32, Value)>,
    version_counter: i32,
    synced_sequence: u64,
    capabilities: Value,
    /// The `workspace/applyEdit` edits received while a `workspace/executeCommand` runs;
    /// `None` when no command runs, and then such edits are denied.
    collected_edits: Option<Vec<Value>>,
}

impl ProjectSession {
    fn start(config: &ServerConfig, stopped: &AtomicBool) -> Result<Self> {
        std::fs::create_dir_all(&config.cache)?;
        let mut command = Command::new(&config.binary);
        if let Some(python) = &config.python {
            command.arg(&python.script).arg("--stdio");
        }
        command.env_clear();
        for name in ["HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    config.toolchain.join("bin").display()
                ),
            )
            .env("RUSTC", config.toolchain.join("bin/rustc"))
            .env("RUSTDOC", config.toolchain.join("bin/rustdoc"))
            .env("CARGO_NET_OFFLINE", "true")
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
            closed: false,
            language:if config.python.is_some(){"python"}else{"rust"},
            configuration:config.python.as_ref().map(|p|json!({"python":{"pythonPath":p.interpreter,"analysis":{"diagnosticMode":"openFilesOnly","typeCheckingMode":"basic","autoImportCompletions":true}},"pyright":{"disableOrganizeImports":true}})).unwrap_or(json!({})),
            diagnostic_versions:BTreeMap::new(),version_counter:0,synced_sequence:u64::MAX,
            capabilities: Value::Null,
            collected_edits: None,
        };
        let init = session.request("initialize", json!({"processId":std::process::id(),"rootUri":file_uri(&config.project)?,"rootPath":config.project,
            "capabilities":client_capabilities(),
            "initializationOptions":{"checkOnSave":false,"cargo":{"extraArgs":["--locked","--offline"],"buildScripts":{"enable":false},"targetDir":config.cache},
                "procMacro":{"enable":false},"files":{"watcher":"server"},"completion":{"autoimport":{"enable":true}},"rustfmt":{"extraArgs":[]}}
        }), &AtomicBool::new(false), stopped)?;
        session.capabilities = init.get("capabilities").cloned().unwrap_or(Value::Null);
        ensure!(
            init.pointer("/capabilities/positionEncoding")
                .is_none_or(|v| v == "utf-16"),
            "Unsupported language-server position encoding"
        );
        session.pull_diagnostics = init
            .pointer("/capabilities/diagnosticProvider")
            .is_some_and(|v| !v.is_null());
        session.notify("initialized", json!({}))?;
        if config.python.is_some() {
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
        let changed = self.documents.len() != documents.len()
            || documents.iter().any(|d| {
                self.documents
                    .get(&d.path)
                    .is_none_or(|old| old.text != d.text || old.instance != d.instance)
            });
        let force_versions =
            self.language == "python" && (changed || self.synced_sequence != sequence);
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
            }
            match self.documents.get_mut(&document.path) {
                Some(old) if old.text != document.text || force_versions => {
                    self.version_counter = self
                        .version_counter
                        .checked_add(1)
                        .context("Language document version exhausted")?;
                    old.version = self.version_counter;
                    old.text = document.text.clone();
                    let version = old.version;
                    self.notify("textDocument/didChange",json!({"textDocument":{"uri":file_uri(&document.path)?,"version":version},"contentChanges":[{"text":document.text.as_ref()}]}))?;
                }
                None => {
                    self.version_counter = self
                        .version_counter
                        .checked_add(1)
                        .context("Language document version exhausted")?;
                    self.notify("textDocument/didOpen",json!({"textDocument":{"uri":file_uri(&document.path)?,"languageId":self.language,"version":self.version_counter,"text":document.text.as_ref()}}))?;
                    self.documents.insert(
                        document.path.clone(),
                        OpenDocument {
                            text: document.text.clone(),
                            instance: document.instance.clone(),
                            version: self.version_counter,
                        },
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn versioned_diagnostics(
        &mut self,
        path: &Path,
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        let version = self.documents.get(path).context("Document closed")?.version;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((received, items)) = self.diagnostic_versions.get(path)
                && *received == version
            {
                return Ok(json!({"kind":"full","items":items}));
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
        cancel: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() >= deadline {
                self.closed = true;
                bail!("Language request timed out; restarting server");
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
            // Unversioned push diagnostics never become current findings.
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
        Ok(())
    }
    fn handle_message(&mut self, message: Value) -> Result<()> {
        if message["method"] == "textDocument/publishDiagnostics"
            && let Some(uri) = message["params"]["uri"].as_str()
            && let Ok(path) = uri_path(uri)
            && let Some(version) = message["params"]["version"].as_i64()
            && let Some(doc) = self.documents.get(&path)
            && i64::from(doc.version) == version
        {
            self.diagnostic_versions.insert(
                path,
                (doc.version, message["params"]["diagnostics"].clone()),
            );
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
                    Some("window/workDoneProgress/create"|"client/registerCapability")=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?,
                    Some("workspace/configuration")=>{
                        let values=message["params"]["items"].as_array().map(|items|items.iter().map(|item|item["section"].as_str().map(|s|s.split('.').fold(&self.configuration,|value,key|&value[key]).clone()).unwrap_or_else(||self.configuration.clone())).collect::<Vec<_>>()).unwrap_or_default();
                        self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":values}))?;
                    },
                    _=>self.send(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Unsupported client method"}}))?,
                }
        }
        Ok(())
    }
    fn stop(&mut self) {
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

/// What Tandem tells a language server it supports. Hover text is asked for as Markdown,
/// so that signatures arrive in fenced code blocks the editor can highlight.
fn client_capabilities() -> Value {
    let mut capabilities = json!({"general":{"positionEncodings":["utf-16"]},"workspace":{"configuration":true},
        "textDocument":{"completion":{"completionItem":{"snippetSupport":false}},"definition":{"linkSupport":true},"hover":{"contentFormat":["markdown","plaintext"]},"publishDiagnostics":{"versionSupport":true},"diagnostic":{},"documentSymbol":{"hierarchicalDocumentSymbolSupport":true}}});
    crate::features::capabilities::extend_client_capabilities(&mut capabilities);
    capabilities
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
