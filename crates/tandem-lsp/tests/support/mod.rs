//! Shared by the integration tests: a scripted fake language server and a polling helper.
// Each test file uses only part of this module.
#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;
use tandem_lsp::ServerConfig;

/// A fake server that records what Tandem sends as JSON files in `dir` and behaves as `modes` say:
/// `slow-init`, `static-pull`, `dynamic-pull`, `unregister` and `push`.
const FAKE_SERVER: &str = r#"#!/usr/bin/python3
import sys, json, time, pathlib
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
        record('initialize', msg['params'])
        if 'slow-init' in modes: time.sleep(5)
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'capabilities': {'diagnosticProvider': {}} if 'static-pull' in modes else {}}})
    elif method == 'initialized':
        send({'jsonrpc': '2.0', 'id': 9001, 'method': 'custom/unknown', 'params': {}})
        if 'dynamic-pull' in modes:
            send({'jsonrpc': '2.0', 'id': 9002, 'method': 'client/registerCapability', 'params': {'registrations': [{'id': 'd', 'method': 'textDocument/diagnostic', 'registerOptions': {}}]}})
    elif ident == 9001 and method is None: record('unknown-reply', msg)
    elif ident == 9002 and method is None:
        record('registered', msg)
        if 'unregister' in modes:
            send({'jsonrpc': '2.0', 'id': 9003, 'method': 'client/unregisterCapability', 'params': {'unregisterations': [{'id': 'd', 'method': 'textDocument/diagnostic'}]}})
    elif ident == 9003 and method is None: record('unregistered', msg)
    elif method == 'textDocument/didOpen':
        doc = msg['params']['textDocument']; opened[doc['uri']] = doc['languageId']; record('language-ids', opened)
        if 'push' in modes:
            send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': {'uri': doc['uri'], 'diagnostics': [item('pushed')]}})
    elif method == 'textDocument/diagnostic':
        send({'jsonrpc': '2.0', 'id': ident, 'result': {'kind': 'full', 'items': [item('pulled')]}})
"#;

/// Writes `dir/server.py` and returns a config that runs it in `dir` with `modes`.
pub fn fake_server(dir: &Path, modes: &[&str]) -> ServerConfig {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("server.py");
    std::fs::write(&script, FAKE_SERVER).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = ServerConfig::new("fake", script, dir, dir.join("cache"));
    config.args = std::iter::once(dir.as_os_str().to_owned())
        .chain(modes.iter().map(|mode| mode.into()))
        .collect();
    config
}

/// Polls `condition` every 20 ms for up to 3 s and fails the test if it never holds.
pub fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..150 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(condition(), "condition timed out");
}
