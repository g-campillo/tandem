//! A scripted stand-in for the Node Claude helper, used only by tests.
//!
//! Usage: `tandem-fake-helper <script>`, where the script has one JSON directive per line:
//! `{"expect": {…}}`, `{"send": {…}}`, `{"send_raw": "text"}`, `{"send_env": "NAME"}`,
//! `{"write_file": {"path": "/abs", "text": "…"}}`, `{"delete_file": "/abs"}`,
//! `{"sleep_ms": 50}`, `{"exit": 1}`, `{"hang": true}` or `{"expect_eof": true}`.
//! A failed expectation prints `fake helper: expected …, got …` to stderr and exits with 3.

use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::exit;
use std::time::Duration;

const EXPECTATION_FAILED: i32 = 3;
const SCRIPT_FAILED: i32 = 4;

fn main() {
    let Some(script) = std::env::args_os().nth(1) else {
        fail(SCRIPT_FAILED, "usage: tandem-fake-helper <script>");
    };
    let text = std::fs::read_to_string(&script)
        .unwrap_or_else(|error| fail(SCRIPT_FAILED, &format!("cannot read the script: {error}")));
    let mut fake = Fake {
        input: std::io::stdin().lock(),
        id: Value::Null,
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let directive: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| fail(SCRIPT_FAILED, &format!("bad directive: {error}")));
        fake.run(&directive);
    }
    while fake.next_line().is_some() {}
    exit(0);
}

struct Fake<R> {
    input: R,
    /// The `id` of the last frame an `expect` directive accepted.
    id: Value,
}

impl<R: BufRead> Fake<R> {
    fn run(&mut self, directive: &Value) {
        let bad = || -> ! { fail(SCRIPT_FAILED, &format!("bad directive: {directive}")) };
        let Some((name, argument)) = directive
            .as_object()
            .filter(|object| object.len() == 1)
            .and_then(|object| object.iter().next())
        else {
            bad();
        };
        match (name.as_str(), argument) {
            ("expect", Value::Object(expected)) => self.expect(expected),
            ("send", Value::Object(frame)) => {
                let mut frame = Value::Object(frame.clone());
                replace_id(&mut frame, &self.id);
                send(frame);
            }
            ("send_raw", Value::String(text)) => write_stdout(text.as_bytes()),
            ("send_env", Value::String(name)) => {
                let value = std::env::var(name).unwrap_or_else(|_| "<unset>".into());
                send(json!({"type": "event", "event": "notice", "text": value}));
            }
            ("write_file", Value::Object(file)) => {
                let (Some(path), Some(text)) = (
                    file.get("path").and_then(Value::as_str),
                    file.get("text").and_then(Value::as_str),
                ) else {
                    bad();
                };
                let path = Path::new(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap_or_else(|error| {
                        fail(
                            SCRIPT_FAILED,
                            &format!("cannot create {}: {error}", parent.display()),
                        )
                    });
                }
                std::fs::write(path, text).unwrap_or_else(|error| {
                    fail(
                        SCRIPT_FAILED,
                        &format!("cannot write {}: {error}", path.display()),
                    )
                });
            }
            ("delete_file", Value::String(path)) => {
                std::fs::remove_file(path).unwrap_or_else(|error| {
                    fail(SCRIPT_FAILED, &format!("cannot delete {path}: {error}"))
                });
            }
            ("sleep_ms", Value::Number(ms)) => {
                let ms = ms.as_u64().unwrap_or_else(|| bad());
                std::thread::sleep(Duration::from_millis(ms));
            }
            ("exit", Value::Number(code)) => {
                let code = code.as_i64().and_then(|code| i32::try_from(code).ok());
                exit(code.unwrap_or_else(|| bad()));
            }
            ("hang", Value::Bool(true)) => loop {
                std::thread::sleep(Duration::from_secs(3600));
            },
            ("expect_eof", Value::Bool(true)) => match self.next_line() {
                None => exit(0),
                Some(line) => fail(
                    EXPECTATION_FAILED,
                    &format!("expected end of input, got {}", line.trim_end()),
                ),
            },
            _ => bad(),
        }
    }

    fn expect(&mut self, expected: &Map<String, Value>) {
        let expectation = Value::Object(expected.clone());
        let Some(line) = self.next_line() else {
            fail(
                EXPECTATION_FAILED,
                &format!("expected {expectation}, got end of input"),
            );
        };
        let frame: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if frame.get("v") != Some(&json!(1)) || !contains(&expectation, &frame) {
            fail(
                EXPECTATION_FAILED,
                &format!("expected {expectation}, got {}", line.trim_end()),
            );
        }
        self.id = frame.get("id").cloned().unwrap_or(Value::Null);
    }

    fn next_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }
}

/// Every key of an expected object is present in `actual` with a matching value; anything
/// other than an object must be equal.
fn contains(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|actual| contains(value, actual))
        }),
        _ => expected == actual,
    }
}

fn replace_id(value: &mut Value, id: &Value) {
    match value {
        Value::String(text) if text == "$id" => *value = id.clone(),
        Value::Array(items) => items.iter_mut().for_each(|item| replace_id(item, id)),
        Value::Object(object) => object.values_mut().for_each(|item| replace_id(item, id)),
        _ => {}
    }
}

fn send(frame: Value) {
    let Value::Object(fields) = frame else {
        fail(SCRIPT_FAILED, "a sent frame must be an object");
    };
    let mut line = Map::new();
    line.insert("v".into(), json!(1));
    line.extend(fields);
    let mut bytes = serde_json::to_vec(&line).unwrap_or_default();
    bytes.push(b'\n');
    write_stdout(&bytes);
}

fn write_stdout(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        // Tandem stopped reading; there is nobody left to talk to.
        exit(1);
    }
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("fake helper: {message}");
    exit(code);
}
