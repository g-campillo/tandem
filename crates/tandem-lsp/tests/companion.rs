//! A server paired with a TypeScript companion, as Vue runs: the companion receives the same
//! documents, answers the tsserver requests the main server relays through Tandem, and the two
//! servers' answers are merged.
mod support;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::wait_for;
use tandem_lsp::*;

/// The Vue server's part: hover relays a tsserver request and shows the response it gets back;
/// completion and pull diagnostics answer with "fromVue" and "vue".
const MAIN_SERVER: &str = r#"#!/usr/bin/python3
import sys, json
def send(v):
    b = json.dumps(v).encode(); sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b); sys.stdout.buffer.flush()
def receive():
    line = sys.stdin.buffer.readline()
    if not line: sys.exit(0)
    n = int(line.decode().split(':')[1]); sys.stdin.buffer.readline(); return json.loads(sys.stdin.buffer.read(n))
item = lambda m: {'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 1}}, 'message': m}
while True:
    msg = receive()
    method, ident = msg.get('method'), msg.get('id')
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'capabilities': {'diagnosticProvider': {}}}})
    elif method == 'textDocument/hover':
        path = msg['params']['textDocument']['uri'][len('file://'):]
        send({'jsonrpc': '2.0', 'method': 'tsserver/request', 'params': [[7, '_vue:quickinfo', {'file': path}]]})
        reply = receive()
        while reply.get('method') != 'tsserver/response': reply = receive()
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'contents': {'kind': 'markdown', 'value': 'relayed ' + json.dumps(reply['params'])}}})
    elif method == 'textDocument/completion':
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'isIncomplete': False, 'items': [{'label': 'fromVue'}]}})
    elif method == 'textDocument/diagnostic':
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'kind': 'full', 'items': [item('vue')]}})
    elif method is not None and ident is not None:
        send({'jsonrpc': '2.0', 'id': ident, 'result': None})
"#;

/// typescript-language-server's part: answers the relayed tsserver request, completes with
/// "fromTs", pushes an unversioned "ts" diagnostic on didOpen, and records what it receives.
/// With `exit-on-open` it exits instead of opening a document.
const COMPANION_SERVER: &str = r#"#!/usr/bin/python3
import sys, json, pathlib
log = pathlib.Path(sys.argv[1]); modes = set(sys.argv[2:]); opened = {}
def send(v):
    b = json.dumps(v).encode(); sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b); sys.stdout.buffer.flush()
def record(name, value): (log / name).write_text(json.dumps(value))
item = lambda m: {'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 1}}, 'message': m}
while True:
    line = sys.stdin.buffer.readline()
    if not line: break
    n = int(line.decode().split(':')[1]); sys.stdin.buffer.readline(); msg = json.loads(sys.stdin.buffer.read(n))
    method, ident = msg.get('method'), msg.get('id')
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'capabilities': {}}})
    elif method == 'workspace/executeCommand':
        record('companion-command', msg['params'])
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'body': {'displayString': 'x'}}})
    elif method == 'textDocument/completion':
        send({'jsonrpc': '2.0', 'id': ident, 'result': [{'label': 'fromTs'}]})
    elif method == 'textDocument/didOpen':
        if 'exit-on-open' in modes: sys.exit(0)
        doc = msg['params']['textDocument']; opened[doc['uri']] = doc['languageId']; record('companion-language-ids', opened)
        send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': {'uri': doc['uri'], 'diagnostics': [item('ts')]}})
    elif method is not None and ident is not None:
        send({'jsonrpc': '2.0', 'id': ident, 'result': None})
"#;

const TEXT: &str = "<script setup lang=\"ts\">\nconst n: number = 1;\n</script>\n";

fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join(name);
    std::fs::write(&script, text).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The fake Vue server alone.
fn main_server(dir: &Path) -> ServerConfig {
    let program = script(dir, "vue.py", MAIN_SERVER);
    let mut config = ServerConfig::new("vue", program, dir, dir.join("cache"));
    config.diagnostics = DiagnosticMode::Settled;
    config
}

/// The fake Vue server paired with the fake TypeScript companion, which records into `dir`.
fn paired(dir: &Path, companion_modes: &[&str]) -> ServerConfig {
    let program = script(dir, "typescript.py", COMPANION_SERVER);
    let mut companion = ServerConfig::new(
        "typescript-language-server (Vue)",
        program,
        dir,
        dir.join("cache/typescript"),
    );
    companion.args = std::iter::once(dir.as_os_str().to_owned())
        .chain(companion_modes.iter().map(|mode| mode.into()))
        .collect();
    companion.diagnostics = DiagnosticMode::Settled;
    let mut config = main_server(dir);
    config.companion = Some(Box::new(companion));
    config
}

/// Starts `config` with `App.vue` open and waits until it is ready.
fn started(dir: &Path, config: ServerConfig) -> (LanguageManager, PathBuf) {
    let file = dir.join("App.vue");
    let manager = LanguageManager::start(config);
    manager.set_documents(vec![LanguageDocument {
        path: file.clone(),
        instance: "test".into(),
        text: Arc::from(TEXT),
    }]);
    wait_for(|| manager.health().state == "ready");
    (manager, file)
}

fn ask(manager: &LanguageManager, file: &Path, method: &str, params: Value) -> Value {
    smol::block_on(manager.request(method, file, "test", TEXT, params))
        .unwrap_or_else(|error| panic!("{method}: {error:#}"))
        .value
}

/// The JSON the fake companion recorded under `name`, once it is complete.
fn record(dir: &Path, name: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(name)).ok()?).ok()
}

fn hover_text(manager: &LanguageManager, file: &Path) -> String {
    let hover = ask(
        manager,
        file,
        "textDocument/hover",
        json!({"position": {"line": 1, "character": 6}}),
    );
    hover["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("{hover}"))
        .to_string()
}

#[test]
fn relayed_tsserver_requests_reach_the_companion_and_come_back() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, file) = started(dir.path(), paired(dir.path(), &[]));

    let text = hover_text(&manager, &file);
    assert!(text.contains(r#"[[7, {"displayString": "x"}]]"#), "{text}");
    let command = record(dir.path(), "companion-command").unwrap();
    assert_eq!(command["command"], json!("typescript.tsserverRequest"));
    assert_eq!(
        command["arguments"],
        json!([
            "_vue:quickinfo",
            {"file": file.to_str().unwrap()},
            {"isAsync": true, "lowPriority": true}
        ])
    );
    manager.stop();
}

#[test]
fn completion_and_diagnostics_merge_both_servers() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, file) = started(dir.path(), paired(dir.path(), &[]));

    let completion = ask(
        &manager,
        &file,
        "textDocument/completion",
        json!({"position": {"line": 1, "character": 6}}),
    );
    let labels: Vec<_> = completion["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{completion}"))
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["fromVue", "fromTs"]);

    let diagnostics = ask(&manager, &file, "textDocument/diagnostic", json!({}));
    let messages: Vec<_> = diagnostic_items(diagnostics)
        .unwrap()
        .into_iter()
        .map(|item| item.message)
        .collect();
    assert_eq!(messages, ["vue", "ts"]);
    manager.stop();
}

#[test]
fn the_companion_opens_vue_documents_as_vue() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, file) = started(dir.path(), paired(dir.path(), &[]));
    wait_for(|| record(dir.path(), "companion-language-ids").is_some());
    let ids = record(dir.path(), "companion-language-ids").unwrap();
    assert_eq!(ids, json!({file_uri(&file).unwrap(): "vue"}));
    manager.stop();
}

#[test]
fn without_a_companion_relayed_requests_get_a_null_body() {
    let dir = tempfile::tempdir().unwrap();
    let (manager, file) = started(dir.path(), main_server(dir.path()));
    let text = hover_text(&manager, &file);
    assert!(text.contains("[[7, null]]"), "{text}");
    manager.stop();
}

#[test]
fn the_pair_restarts_when_the_companion_exits() {
    let dir = tempfile::tempdir().unwrap();
    let manager = LanguageManager::start(paired(dir.path(), &["exit-on-open"]));
    manager.set_documents(vec![LanguageDocument {
        path: dir.path().join("App.vue"),
        instance: "test".into(),
        text: Arc::from(TEXT),
    }]);
    wait_for(|| manager.health().state == "restarting");
    assert!(manager.health().restarts >= 1);
    manager.stop();
}

#[test]
fn a_companion_that_cannot_start_fails_the_pair() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = paired(dir.path(), &[]);
    if let Some(companion) = config.companion.as_mut() {
        companion.program = dir.path().join("missing");
    }
    let manager = LanguageManager::start(config);
    wait_for(|| {
        let health = manager.health();
        health.state == "restarting"
            && health
                .detail
                .contains("Could not start the installed language server")
    });
    manager.stop();
}
