//! Bounded stdio transport for the pinned Codex app-server feasibility probe.
//! Raw protocol/authentication payloads are never logged by this library.

mod installation;
pub use installation::*;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub const EXPECTED_RUNTIME: &str = "codex-cli 0.169.0";
const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_PENDING: usize = 256;

/// Reads one complete JSONL frame without allocating an unbounded line.
/// A partial frame at EOF is a transport failure, not a valid last message.
fn read_frame(reader: &mut impl BufRead, limit: usize) -> Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().context("read protocol bytes")?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            bail!("protocol closed with an incomplete frame");
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let length = newline.map_or(available.len(), |p| p + 1);
        if bytes.len() + length > limit {
            bail!("protocol frame exceeds configured byte limit");
        }
        bytes.extend_from_slice(&available[..length]);
        reader.consume(length);
        if newline.is_some() {
            // Do not include malformed content in the diagnostic.
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| anyhow!("invalid JSON protocol frame"));
        }
    }
}

fn response_result(message: Value) -> Result<Value> {
    if let Some(error) = message.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let reason = match error.get("message").and_then(Value::as_str) {
            Some("no active turn to interrupt") => "no_active_turn",
            Some(text) if text.starts_with("expected active turn id ") => "active_turn_mismatch",
            _ => "unclassified",
        };
        bail!("app-server rejected request (code {code}, {reason}; payload withheld)");
    }
    message
        .get("result")
        .cloned()
        .context("response has no result")
}

/// Configures an allowlisted environment. In particular, no API credentials,
/// provider overrides, inherited Codex configuration, or shell hooks propagate.
fn runtime_command(binary: &Path, home: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(binary);
    command.env_clear();
    for name in [
        "HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "LANG", "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("CODEX_HOME", home).current_dir(cwd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // macOS dispatch workers can block SIGCHLD. Carrying that mask into
        // app-server prevents its async process watcher from reaping commands.
        // Change only the child's mask, using async-signal-safe calls.
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
    command
}

pub struct Client {
    child: Child,
    input: Option<ChildStdin>,
    incoming: Receiver<Result<Value>>,
    pending: VecDeque<Value>,
    next_id: u64,
    pub notifications: BTreeMap<String, usize>,
    pub rejected_server_requests: usize,
}

impl Client {
    pub fn start(binary: &Path, home: &Path, cwd: &Path) -> Result<Self> {
        Self::start_with_overrides(binary, home, cwd, &[])
    }

    pub fn start_with_overrides(
        binary: &Path,
        home: &Path,
        cwd: &Path,
        overrides: &[String],
    ) -> Result<Self> {
        let version = runtime_command(binary, home, cwd)
            .arg("--version")
            .output()
            .context("inspect runtime version")?;
        if !version.status.success()
            || String::from_utf8_lossy(&version.stdout).trim() != EXPECTED_RUNTIME
        {
            bail!("unsupported Codex runtime; this probe requires {EXPECTED_RUNTIME}");
        }
        let mut command = runtime_command(binary, home, cwd);
        command
            .args(["app-server", "--strict-config", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for setting in overrides {
            command.args(["-c", setting]);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context("start app-server")?;
        let input = child.stdin.take().context("open protocol input")?;
        let stdout = child.stdout.take().context("open protocol output")?;
        let stderr = child.stderr.take().context("open process diagnostics")?;
        let (sender, incoming) = mpsc::sync_channel(64);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader, MAX_FRAME) {
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
        // Drain diagnostics without ever exposing or persisting raw content.
        thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        });
        let mut client = Self {
            child,
            input: Some(input),
            incoming,
            pending: VecDeque::new(),
            next_id: 1,
            notifications: BTreeMap::new(),
            rejected_server_requests: 0,
        };
        client.request(
            "initialize",
            json!({
                "clientInfo": {"name": "tandem", "title": "Tandem", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }),
            Duration::from_secs(30),
        )?;
        client.send(&json!({"method": "initialized"}))?;
        Ok(client)
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        let input = self.input.as_mut().context("protocol input closed")?;
        serde_json::to_writer(&mut *input, message).context("encode request")?;
        input.write_all(b"\n").context("write request separator")?;
        input.flush().context("flush request")
    }

    fn receive(&mut self, deadline: Instant) -> Result<Value> {
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            let message = self
                .incoming
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => {
                        anyhow!("app-server response timeout; outcome unknown")
                    }
                    mpsc::RecvTimeoutError::Disconnected => anyhow!("app-server protocol closed"),
                })??;
            if message.get("method").is_some() && message.get("id").is_some() {
                self.rejected_server_requests += 1;
                // No server-initiated action is authorized by the M0-A probe.
                self.send(&json!({"id": message["id"], "error": {
                    "code": -32601, "message": "This client does not authorize server requests"
                }}))?;
                continue;
            }
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                // Retain a bounded vocabulary of metadata, never arbitrary values.
                let label = match method {
                    "turn/started"
                    | "turn/completed"
                    | "item/started"
                    | "item/completed"
                    | "item/agentMessage/delta"
                    | "thread/started"
                    | "account/updated"
                    | "account/login/completed"
                    | "error" => method,
                    _ => "other",
                };
                *self.notifications.entry(label.into()).or_default() += 1;
            }
            return Ok(message);
        }
    }

    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.begin_request(method, params)?;
        self.finish_request(id, timeout)
    }

    pub fn begin_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        Ok(id)
    }

    pub fn finish_request(&mut self, id: u64, timeout: Duration) -> Result<Value> {
        if let Some(index) = self
            .pending
            .iter()
            .position(|message| message["id"].as_u64() == Some(id))
        {
            return response_result(
                self.pending
                    .remove(index)
                    .expect("located pending response"),
            );
        }
        let deadline = Instant::now() + timeout;
        loop {
            let message = self.receive(deadline)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return response_result(message);
            }
            if self.pending.len() >= MAX_PENDING {
                bail!("pending protocol messages exceeded limit");
            }
            self.pending.push_back(message);
        }
    }

    pub fn next_event(&mut self, timeout: Duration) -> Result<Value> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(message);
        }
        self.receive(Instant::now() + timeout)
    }

    /// A quiet stream is not a failed operation. Execution clients use this to
    /// observe cancellation while the server is waiting for a process to exit.
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<Value>> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(Some(message));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let message = match self
                .incoming
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(message) => message?,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("app-server protocol closed"),
            };
            if message.get("method").is_some() && message.get("id").is_some() {
                self.rejected_server_requests += 1;
                self.send(&json!({"id":message["id"],"error":{"code":-32601,"message":"Action is outside the granted execution scope"}}))?;
                continue;
            }
            return Ok(Some(message));
        }
    }

    /// Close input first, then terminate this child's process group if needed.
    /// Does not infer rollback or turn completion from process shutdown.
    pub fn shutdown(&mut self) -> Result<()> {
        self.input.take();
        for _ in 0..20 {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        for _ in 0..20 {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        self.child.wait().context("reap app-server")?;
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fragmented_utf8_and_multiple_frames() {
        let input = "{\"text\":\"🦀\"}\n{\"id\":2,\"result\":{}}\n";
        let mut reader = BufReader::with_capacity(2, Cursor::new(input));
        assert_eq!(read_frame(&mut reader, 100).unwrap().unwrap()["text"], "🦀");
        assert_eq!(read_frame(&mut reader, 100).unwrap().unwrap()["id"], 2);
        assert!(read_frame(&mut reader, 100).unwrap().is_none());
    }

    #[test]
    fn truncated_oversized_and_invalid_frames_fail_without_echoing_contents() {
        for input in [
            "{\"secret\":\"private\"}",
            "not-json-private\n",
            "private-private-private\n",
        ] {
            let error = read_frame(&mut Cursor::new(input), 20)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private"));
        }
    }

    #[test]
    fn rpc_errors_never_echo_private_data() {
        let error = response_result(
            json!({"error": {"code": -32000, "message": "secret", "data": "secret"}}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("-32000"));
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn environment_is_explicit_and_api_keys_are_not_forwarded() {
        let command = runtime_command(
            Path::new("codex"),
            Path::new("/private-home"),
            Path::new("/scratch"),
        );
        let env: BTreeMap<_, _> = command.get_envs().collect();
        assert!(env.contains_key(std::ffi::OsStr::new("CODEX_HOME")));
        for key in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "OPENAI_BASE_URL",
            "BASH_ENV",
            "ENV",
        ] {
            assert!(!env.contains_key(std::ffi::OsStr::new(key)));
        }
    }

    #[cfg(unix)]
    fn peer(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peer");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'codex-cli 0.169.0\\n'; exit 0; fi\nIFS= read -r initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nIFS= read -r initialized\n{body}\n"
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (directory, path)
    }

    #[test]
    #[cfg(unix)]
    fn child_signal_mask_is_reset_without_changing_the_parent_thread() {
        let worker = std::thread::spawn(|| unsafe {
            let mut blocked: libc::sigset_t = std::mem::zeroed();
            let mut original: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGCHLD);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut original),
                0
            );
            let result=runtime_command(Path::new("/usr/bin/python3"),Path::new("/tmp"),Path::new("/tmp"))
                .args(["-c","import signal; assert signal.SIGCHLD not in signal.pthread_sigmask(signal.SIG_BLOCK, [])"]).status();
            let mut current: libc::sigset_t = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut current);
            let parent_blocked = libc::sigismember(&current, libc::SIGCHLD) == 1;
            libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut());
            assert!(result.unwrap().success());
            assert!(parent_blocked);
        });
        worker.join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn deferred_responses_and_notifications_survive_interleaving() {
        let (directory, binary) = peer(
            r#"
IFS= read -r first
IFS= read -r second
printf '%s\n' '{"id":"approval","method":"unexpected/approval","params":{"secret":"private"}}'
IFS= read -r rejection
printf '%s\n' '{"method":"turn/completed","params":{"turn":{"status":"interrupted"}}}'
printf '%s\n' '{"id":2,"result":{"exitCode":130}}'
printf '%s\n' '{"id":3,"result":{"terminated":true}}'
IFS= read -r eof
"#,
        );
        let mut client = Client::start(&binary, directory.path(), directory.path()).unwrap();
        let running = client.begin_request("command/exec", json!({})).unwrap();
        let cancel = client
            .begin_request("command/exec/terminate", json!({}))
            .unwrap();
        assert_eq!(
            client
                .finish_request(cancel, Duration::from_secs(2))
                .unwrap()["terminated"],
            true
        );
        assert_eq!(
            client
                .finish_request(running, Duration::from_secs(2))
                .unwrap()["exitCode"],
            130
        );
        assert_eq!(
            client.next_event(Duration::from_secs(1)).unwrap()["method"],
            "turn/completed"
        );
        assert_eq!(client.rejected_server_requests, 1);
    }

    #[test]
    #[cfg(unix)]
    fn disconnect_is_not_reported_as_a_successful_response() {
        let (directory, binary) = peer("IFS= read -r request\nexit 0");
        let mut client = Client::start(&binary, directory.path(), directory.path()).unwrap();
        let error = client
            .request("turn/start", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert!(error.to_string().contains("closed"));
    }

    #[test]
    #[cfg(unix)]
    fn timeout_reports_unknown_outcome_without_automatic_retry() {
        let (directory, binary) = peer("IFS= read -r request\n/bin/sleep 0.2\nexit 0");
        let mut client = Client::start(&binary, directory.path(), directory.path()).unwrap();
        let error = client
            .request("turn/start", json!({}), Duration::from_millis(10))
            .unwrap_err();
        assert!(error.to_string().contains("outcome unknown"));
        assert_eq!(client.next_id, 3);
    }
}
