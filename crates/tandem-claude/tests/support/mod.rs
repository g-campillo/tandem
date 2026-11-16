//! Runs `HelperClient` and `Companion` against the scripted fake helper
//! (`src/bin/fake_helper.rs`).
#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tandem_claude::companion::Companion;
use tandem_claude::{
    BufferText, ChatState, Command, CompanionConfig, Connection, HelperClient, HostAnswer,
    HostQuestion, Launch, Received, Signal,
};
use tandem_core::Store;

pub fn fake_helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tandem-fake-helper"))
}

/// Writes one directive per line to a new script file in `dir`.
pub fn write_script(dir: &Path, lines: &[Value]) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let path = dir.join(format!(
        "script-{}.jsonl",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).expect("write the fake helper script");
    path
}

pub fn fake_launch(script: &Path) -> Launch {
    Launch::custom(fake_helper_path(), vec![script.into()])
}

/// Spawns the helper with a sink that forwards everything it receives to a channel.
pub fn collect(launch: &Launch) -> (HelperClient, Receiver<Received>) {
    let (sender, receiver) = mpsc::channel();
    let client = HelperClient::spawn(
        launch,
        Box::new(move |received| {
            let _ = sender.send(received);
        }),
    )
    .expect("spawn the helper");
    (client, receiver)
}

/// A `Companion` on a temporary project and state folder, talking to the fake helper.
///
/// The helper runs under a small `bash` wrapper that records every frame Tandem writes in
/// a log (`sent()`) and the fake helper's exit status (`stop()`), so a test can check the
/// exact frames and that the script saw everything it expected.
pub struct Harness {
    pub temp: tempfile::TempDir,
    /// The canonical project folder; its text is the project key.
    pub project: PathBuf,
    pub store: Arc<Mutex<Store>>,
    /// Open editor buffers by project-relative path, as the host thread reports them.
    pub buffers: Arc<Mutex<BTreeMap<String, BufferText>>>,
    companion: Option<Companion>,
    host: Option<thread::JoinHandle<()>>,
    run: Option<Run>,
}

/// The files of one helper run.
struct Run {
    log: PathBuf,
    status: PathBuf,
}

const WAIT_LIMIT: Duration = Duration::from_secs(5);

impl Harness {
    /// A project folder (with an empty `src`) and a store, with nothing started yet.
    pub fn new() -> Harness {
        let temp = tempfile::tempdir().expect("temp dir");
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join("src")).expect("project folder");
        let project = project.canonicalize().expect("canonical project");
        let store = Store::open(&temp.path().join("state")).expect("open the store");
        Harness {
            temp,
            project,
            store: Arc::new(Mutex::new(store)),
            buffers: Arc::new(Mutex::new(BTreeMap::new())),
            companion: None,
            host: None,
            run: None,
        }
    }

    /// Starts a companion whose helper runs `lines`.
    pub fn started(lines: &[Value]) -> Harness {
        let mut harness = Harness::new();
        harness.start(lines);
        harness
    }

    pub fn key(&self) -> String {
        self.project.to_str().expect("UTF-8 project").to_owned()
    }

    pub fn with_store<T>(&self, change: impl FnOnce(&mut Store) -> T) -> T {
        change(&mut self.store.lock().unwrap())
    }

    /// Starts a companion (stopping any earlier one) whose helper runs `lines`.
    pub fn start(&mut self, lines: &[Value]) {
        let script = write_script(self.temp.path(), lines);
        let log = script.with_extension("sent");
        let status = script.with_extension("status");
        let launch = recorded_launch(&script, &log, &status);
        self.run = Some(Run { log, status });
        self.start_with(Some(launch), false);
    }

    pub fn start_with(&mut self, launch: Option<Launch>, offline: bool) {
        self.stop();
        let (companion, signals) = Companion::start(CompanionConfig {
            project: self.project.clone(),
            store: Arc::clone(&self.store),
            launch,
            offline,
        });
        let buffers = Arc::clone(&self.buffers);
        self.host = Some(thread::spawn(move || answer_host(&signals, &buffers)));
        self.companion = Some(companion);
    }

    pub fn companion(&self) -> &Companion {
        self.companion.as_ref().expect("a started companion")
    }

    pub fn send(&self, command: Command) {
        self.companion().send(command);
    }

    pub fn state(&self) -> Arc<ChatState> {
        self.companion().state()
    }

    /// Polls the state until `done` holds, for up to 5 s; panics with the last state.
    pub fn wait_for(&self, what: &str, done: impl Fn(&ChatState) -> bool) -> Arc<ChatState> {
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            let state = self.state();
            if done(&state) {
                return state;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for {what}; last state: {state:#?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_ready(&self) -> Arc<ChatState> {
        self.wait_for("a ready connection", |state| {
            matches!(state.connection, Connection::Ready { .. })
        })
    }

    /// Every frame Tandem has written to the current helper so far, in order.
    pub fn sent(&self) -> Vec<Value> {
        let Some(run) = &self.run else {
            return Vec::new();
        };
        let text = std::fs::read_to_string(&run.log).unwrap_or_default();
        text.split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|line| serde_json::from_str(line).expect("a JSON frame"))
            .collect()
    }

    /// The frames of one `type` Tandem has written so far.
    pub fn sent_of(&self, kind: &str) -> Vec<Value> {
        self.sent()
            .into_iter()
            .filter(|frame| frame["type"] == kind)
            .collect()
    }

    /// Polls the recorded frames until `done` holds, for up to 5 s.
    pub fn wait_sent(&self, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            let sent = self.sent();
            if done(&sent) {
                return sent;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for {what}; sent: {sent:#?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Shuts the companion down and returns the fake helper's exit status, if it exited
    /// on its own.
    pub fn stop(&mut self) -> Option<i32> {
        if let Some(companion) = self.companion.take() {
            companion.shutdown();
        }
        if let Some(host) = self.host.take() {
            let _ = host.join();
        }
        let status = self.run.as_ref()?.status.clone();
        std::fs::read_to_string(status).ok()?.trim().parse().ok()
    }

    /// Drops the companion without waiting for it, as when Tandem quits: its thread and
    /// helper wind down in the background.
    pub fn abandon(&mut self) {
        drop(self.companion.take());
        // The host thread ends with the old controller.
        drop(self.host.take());
    }

    /// The folder that holds the snapshot of change set `id`.
    pub fn snapshot_dir(&self, id: &str) -> PathBuf {
        self.with_store(|store| store.directory.join("takeover").join(id))
    }

    /// Shuts down and checks that the fake helper ran its whole script without a mismatch.
    pub fn finish(mut self) {
        assert_eq!(
            self.stop(),
            Some(0),
            "the fake helper did not finish its script; sent: {:#?}",
            self.sent()
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(companion) = self.companion.take() {
            companion.shutdown();
        }
    }
}

/// Runs the fake helper through `bash`, appending everything Tandem writes to `log` and
/// the fake's exit status to `status`. Only the fake holds the helper's stdout, so its exit
/// still ends the stream at once.
pub fn recorded_launch(script: &Path, log: &Path, status: &Path) -> Launch {
    Launch::custom(
        PathBuf::from("/bin/bash"),
        vec![
            "-c".into(),
            r#""$0" "$1" < <(exec /usr/bin/tee -a "$2" 2>/dev/null); echo $? > "$3""#.into(),
            fake_helper_path().into(),
            script.into(),
            log.into(),
            status.into(),
        ],
    )
}

/// Answers the companion's host questions from `buffers` until the companion ends.
fn answer_host(
    signals: &async_channel::Receiver<Signal>,
    buffers: &Mutex<BTreeMap<String, BufferText>>,
) {
    while let Ok(signal) = signals.recv_blocking() {
        let Signal::Host(ask) = signal else {
            continue;
        };
        let buffers = buffers.lock().unwrap();
        let answer = match ask.question {
            HostQuestion::ReadBuffer { path } => HostAnswer::Buffer(buffers.get(&path).cloned()),
            HostQuestion::DirtyPaths => HostAnswer::DirtyPaths(
                buffers
                    .iter()
                    .filter(|(_, buffer)| buffer.dirty)
                    .map(|(path, _)| path.clone())
                    .collect(),
            ),
        };
        let _ = ask.reply.send(answer);
    }
}

pub fn reply_ok(data: Value) -> Value {
    json!({"send": {"type": "reply", "id": "$id", "ok": true, "data": data}})
}

pub fn reply_error(code: &str, message: &str) -> Value {
    json!({"send": {"type": "reply", "id": "$id", "ok": false,
        "error": {"code": code, "message": message}}})
}

pub fn expect(frame: Value) -> Value {
    json!({ "expect": frame })
}

pub fn event(event: Value) -> Value {
    let mut frame = json!({"type": "event"});
    frame
        .as_object_mut()
        .unwrap()
        .extend(event.as_object().expect("an event object").clone());
    json!({ "send": frame })
}

pub fn hello() -> Vec<Value> {
    vec![
        expect(json!({"type": "hello"})),
        reply_ok(json!({
            "protocol": 1, "helper": "1.0.0", "sdk": "0.3.325",
            "claude_code": "2.1.325", "node": "v24.22.1"
        })),
    ]
}

/// The `open_session` reply data for a signed-in subscription with three models.
pub fn session_data() -> Value {
    json!({
        "auth": {"ok": true},
        "account": {"subscription": "Claude Ultra", "provider": "firstParty"},
        "claude_code_version": "2.1.325",
        "models": [
            {"value": "default", "name": "Default (recommended)", "description": "d",
             "efforts": ["low", "high"]},
            {"value": "opus", "name": "Opus", "description": "o",
             "efforts": ["low", "high", "max"]},
            {"value": "haiku", "name": "Haiku", "description": "h", "efforts": []}
        ]
    })
}

/// Expects an `open_session` frame matching `expected` and accepts it.
pub fn open_ok(expected: Value) -> Vec<Value> {
    let mut frame = json!({"type": "open_session"});
    frame
        .as_object_mut()
        .unwrap()
        .extend(expected.as_object().expect("an object").clone());
    vec![expect(frame), reply_ok(session_data())]
}

/// A successful `hello` and `open_session`.
pub fn preamble() -> Vec<Value> {
    [hello(), open_ok(json!({}))].concat()
}

/// Expects a `turn`, accepts it and reports it started.
pub fn turn_ok() -> Vec<Value> {
    turn_ok_with(json!({}))
}

/// Like `turn_ok`, requiring the `turn` frame to match `expected`.
pub fn turn_ok_with(expected: Value) -> Vec<Value> {
    let mut frame = json!({"type": "turn"});
    frame
        .as_object_mut()
        .unwrap()
        .extend(expected.as_object().expect("an object").clone());
    vec![
        expect(frame),
        reply_ok(json!({})),
        event(json!({"event": "turn_started", "turn": "$id"})),
    ]
}

/// Ends turn `turn` with `status` ("completed", "interrupted" or "error").
pub fn turn_finished(turn: u64, status: &str) -> Value {
    event(json!({"event": "turn_finished", "turn": turn, "status": status, "error": null}))
}

/// A tool request from Claude with the helper's request id `id`.
pub fn tool_request(id: &str, name: &str, input: Value) -> Value {
    json!({"send": {"type": "request", "id": id, "request": "tool", "name": name, "input": input}})
}

/// A permission request for running `command`.
pub fn permission_request(id: &str, command: &str) -> Value {
    json!({"send": {"type": "request", "id": id, "request": "permission", "tool": "Bash",
        "title": format!("Claude wants to run {command}"), "detail": "",
        "command": command, "path": null}})
}
