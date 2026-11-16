//! Runs the helper as a child process and exchanges protocol frames with it. A reader
//! thread hands every frame to a sink in order; a second thread keeps the end of stderr for
//! diagnostics.

use crate::install::{Launch, child_env};
use crate::protocol::{self, Incoming, MAX_FRAME_BYTES, Outgoing, ResponseBody};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const STDERR_TAIL_BYTES: usize = 16 * 1024;
const INVALID_FRAME: &str = "invalid frame from the Claude helper";
const OVERSIZED_FRAME: &str = "frame from the Claude helper exceeds 8 MiB";
const READ_FAILED: &str = "could not read from the Claude helper";
/// How long a closed stdout waits for the rest of stderr, so a crash message makes it into
/// `stderr_tail`.
const STDERR_SETTLE: Duration = Duration::from_secs(1);
const EXIT_GRACE: Duration = Duration::from_secs(2);
const TERM_GRACE: Duration = Duration::from_secs(1);

/// What the reader thread hands to the sink: every frame in order, then `Closed` once.
#[derive(Debug)]
pub enum Received {
    Frame(Incoming),
    /// `error` is `None` when stdout ended cleanly.
    Closed {
        error: Option<String>,
        stderr_tail: String,
    },
}

pub struct HelperClient {
    child: Child,
    writer: Mutex<Writer>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    reaped: bool,
}

struct Writer {
    stdin: Option<ChildStdin>,
    next_id: u64,
}

impl Writer {
    fn write(&mut self, line: &[u8]) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("The Claude helper is not running")?;
        stdin
            .write_all(line)
            .and_then(|()| stdin.flush())
            .context("Could not write to the Claude helper")
    }
}

impl HelperClient {
    /// Starts the helper with a scrubbed environment in its own process group.
    pub fn spawn(launch: &Launch, sink: Box<dyn Fn(Received) + Send>) -> Result<HelperClient> {
        let mut extra_path: Vec<PathBuf> = Vec::new();
        if let Some(dir) = launch.node.as_ref().and_then(|node| node.path.parent()) {
            extra_path.push(dir.to_path_buf());
        }
        let home = launch
            .env
            .iter()
            .find(|(name, _)| name == "HOME")
            .map(|(_, home)| PathBuf::from(home))
            .or_else(std::env::home_dir);
        if let Some(home) = home {
            extra_path.push(home.join(".cargo/bin"));
        }
        extra_path.push("/opt/homebrew/bin".into());
        extra_path.push("/usr/local/bin".into());

        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(child_env(&launch.env, &extra_path))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // macOS dispatch workers can block SIGCHLD. Carrying that mask into the helper
        // prevents Node from reaping the processes it starts. Change only the child's mask,
        // using async-signal-safe calls.
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
        let mut child = command.spawn().with_context(|| {
            format!(
                "Could not start the Claude helper ({})",
                launch.program.display()
            )
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Could not connect to the Claude helper's standard streams");
        };

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)));
        let (stderr_done, stderr_drained) = mpsc::channel::<()>();
        let tail = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            drain_stderr(stderr, &tail);
            drop(stderr_done);
        });
        let tail = Arc::clone(&stderr_tail);
        thread::spawn(move || read_frames(stdout, sink, &tail, &stderr_drained));

        Ok(HelperClient {
            child,
            writer: Mutex::new(Writer {
                stdin: Some(stdin),
                next_id: 1,
            }),
            stderr_tail,
            reaped: false,
        })
    }

    /// Sends a command built with the next id (1, 2, 3, …) and returns that id.
    pub fn command(&self, build: impl FnOnce(u64) -> Outgoing) -> Result<u64> {
        let mut writer = lock(&self.writer);
        let id = writer.next_id;
        writer.next_id += 1;
        let line = protocol::encode(&build(id))?;
        writer.write(&line)?;
        Ok(id)
    }

    /// Answers the helper request `id`.
    pub fn respond(&self, id: &str, body: ResponseBody) -> Result<()> {
        let line = protocol::encode(&Outgoing::Response {
            id: id.to_owned(),
            body,
        })?;
        lock(&self.writer).write(&line)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The last 16 KiB the helper wrote to stderr.
    pub fn stderr_tail(&self) -> String {
        tail_text(&self.stderr_tail)
    }

    /// Closes stdin and waits up to 2 s for the helper to exit, then terminates its process
    /// group (1 s grace) and finally kills it. Safe to call more than once.
    pub fn shutdown(&mut self) {
        lock(&self.writer).stdin.take();
        if self.reaped || self.wait_for_exit(EXIT_GRACE) {
            return;
        }
        self.signal_group(libc::SIGTERM);
        if self.wait_for_exit(TERM_GRACE) {
            return;
        }
        self.signal_group(libc::SIGKILL);
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
    }

    fn wait_for_exit(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            match self.child.try_wait() {
                // An error means the child cannot be waited for any more; never signal its
                // process id again, as it may belong to another process by now.
                Ok(Some(_)) | Err(_) => {
                    self.reaped = true;
                    return true;
                }
                Ok(None) if Instant::now() >= deadline => return false,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    fn signal_group(&self, signal: libc::c_int) {
        // The helper leads its own process group, so this also reaches Claude Code and any
        // command it started.
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), signal);
        }
    }
}

impl Drop for HelperClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn tail_text(tail: &Mutex<VecDeque<u8>>) -> String {
    String::from_utf8_lossy(lock(tail).make_contiguous()).into_owned()
}

fn drain_stderr(mut stderr: ChildStderr, tail: &Mutex<VecDeque<u8>>) {
    let mut buffer = [0u8; 8192];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                let mut tail = lock(tail);
                tail.extend(&buffer[..read]);
                let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..excess);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn read_frames(
    stdout: ChildStdout,
    sink: Box<dyn Fn(Received) + Send>,
    stderr_tail: &Mutex<VecDeque<u8>>,
    stderr_drained: &Receiver<()>,
) {
    let mut reader = BufReader::new(stdout);
    let (error, stdout_ended) = loop {
        match read_line(&mut reader, MAX_FRAME_BYTES) {
            // The helper's own reader skips empty lines; do the same.
            Ok(Some(line)) if line == b"\n" => {}
            Ok(Some(line)) => match protocol::decode(&line) {
                Ok(frame) => sink(Received::Frame(frame)),
                Err(_) => break (Some(INVALID_FRAME), false),
            },
            Ok(None) => break (None, true),
            Err(LineError::Incomplete) => break (Some(INVALID_FRAME), true),
            Err(LineError::TooLong) => break (Some(OVERSIZED_FRAME), false),
            Err(LineError::Read) => break (Some(READ_FAILED), false),
        }
    };
    // Close our end at once, so a helper still writing fails instead of blocking.
    drop(reader);
    if stdout_ended {
        // Usually the helper has exited; give stderr a moment to deliver its last words.
        let _ = stderr_drained.recv_timeout(STDERR_SETTLE);
    }
    sink(Received::Closed {
        error: error.map(str::to_owned),
        stderr_tail: tail_text(stderr_tail),
    });
}

#[derive(Debug, PartialEq, Eq)]
enum LineError {
    /// The stream ended in the middle of a line.
    Incomplete,
    /// The line, newline included, is longer than the limit.
    TooLong,
    Read,
}

/// Reads one newline-terminated line of at most `limit` bytes (newline included) without
/// ever holding more than `limit` bytes for it. `Ok(None)` is a clean end of stream.
fn read_line(reader: &mut impl BufRead, limit: usize) -> Result<Option<Vec<u8>>, LineError> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return Err(LineError::Read),
        };
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(LineError::Incomplete)
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let length = newline.map_or(available.len(), |position| position + 1);
        let needed = line.len() + length;
        if needed > limit {
            return Err(LineError::TooLong);
        }
        if needed > line.capacity() {
            let target = (line.capacity() * 2).clamp(needed, limit);
            line.reserve_exact(target - line.len());
        }
        line.extend_from_slice(&available[..length]);
        reader.consume(length);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn reader(bytes: &[u8]) -> BufReader<Cursor<Vec<u8>>> {
        // A tiny buffer makes every line span several reads.
        BufReader::with_capacity(3, Cursor::new(bytes.to_vec()))
    }

    #[test]
    fn read_line_returns_whole_lines_then_a_clean_end() {
        let mut input = reader(b"{\"a\":1}\n\n{\"b\":2}\n");
        assert_eq!(read_line(&mut input, 64), Ok(Some(b"{\"a\":1}\n".to_vec())));
        assert_eq!(read_line(&mut input, 64), Ok(Some(b"\n".to_vec())));
        assert_eq!(read_line(&mut input, 64), Ok(Some(b"{\"b\":2}\n".to_vec())));
        assert_eq!(read_line(&mut input, 64), Ok(None));
    }

    #[test]
    fn read_line_limit_counts_the_newline() {
        let at_limit = read_line(&mut reader(b"123456789\n"), 10).unwrap().unwrap();
        assert_eq!(at_limit.len(), 10);
        assert!(at_limit.capacity() <= 10);
        assert_eq!(
            read_line(&mut reader(b"1234567890\n"), 10),
            Err(LineError::TooLong)
        );
        assert_eq!(
            read_line(&mut reader(b"12345678901234"), 10),
            Err(LineError::TooLong)
        );
    }

    #[test]
    fn read_line_reports_a_partial_last_line() {
        let mut input = reader(b"{\"a\":1}\n{\"b\"");
        assert!(read_line(&mut input, 64).unwrap().is_some());
        assert_eq!(read_line(&mut input, 64), Err(LineError::Incomplete));
    }
}
