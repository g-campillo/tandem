use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tandem_lsp::*;

/// A scripted language server that records what Tandem sends it, with the manager
/// started on it and one open document.
fn fixture() -> (tempfile::TempDir, LanguageManager, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.py");
    std::fs::write(&script,r#"#!/usr/bin/python3
import sys,json,pathlib
root=pathlib.Path(__file__).parent
def send(value):
    body=json.dumps(value).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body);sys.stdout.buffer.flush()
def record(name,value):
    (root/(name+'.tmp')).write_text(json.dumps(value));(root/(name+'.tmp')).rename(root/name)
while True:
    line=sys.stdin.buffer.readline()
    if not line: break
    length=int(line.decode().split(':')[1]);sys.stdin.buffer.readline();msg=json.loads(sys.stdin.buffer.read(length))
    method=msg.get('method');ident=msg.get('id')
    if method=='initialize':
        record('client-capabilities.json',msg['params']['capabilities'])
        send({'jsonrpc':'2.0','id':ident,'result':{'capabilities':{'renameProvider':{'prepareProvider':True},'completionProvider':{'triggerCharacters':['.',':'],'resolveProvider':True},'executeCommandProvider':{'commands':['test.apply']}}}})
    elif method=='workspace/symbol':
        record('symbol-params.json',msg['params'])
        send({'jsonrpc':'2.0','id':ident,'result':[]})
    elif method=='textDocument/hover':
        record('hover-params.json',msg['params'])
        send({'jsonrpc':'2.0','id':ident,'result':None})
    elif ident is not None and method is not None:
        send({'jsonrpc':'2.0','id':ident,'result':None})
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = root.path().join("main.rs");
    let manager = LanguageManager::start(ServerConfig {
        python: None,
        binary: script,
        toolchain: PathBuf::from("/usr"),
        project: root.path().into(),
        cache: root.path().join("cache"),
    });
    manager.set_documents(vec![LanguageDocument {
        path: file.clone(),
        instance: "test".into(),
        text: Arc::from("fn main() {}"),
    }]);
    wait_for(|| manager.health().state == "ready");
    (root, manager, file)
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..150 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(condition(), "condition timed out");
}

fn recorded(root: &tempfile::TempDir, name: &str) -> Value {
    let path = root.path().join(name);
    wait_for(|| path.exists());
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn server_capabilities_are_exposed_while_the_server_runs() {
    let (_root, manager, _file) = fixture();
    let features = manager.capabilities();
    assert!(features.is_known());
    assert!(features.rename_prepare());
    assert!(features.completion_resolve());
    assert_eq!(features.completion_triggers(), [".", ":"]);
    assert_eq!(features.commands(), ["test.apply"]);
    manager.stop();
    wait_for(|| !manager.capabilities().is_known());
}

#[test]
fn the_new_client_capabilities_are_sent() {
    let (root, manager, _file) = fixture();
    let sent = recorded(&root, "client-capabilities.json");
    assert_eq!(
        sent.pointer("/textDocument/completion/completionItem/snippetSupport"),
        Some(&json!(true))
    );
    assert_eq!(
        sent.pointer("/textDocument/hover/contentFormat"),
        Some(&json!(["markdown", "plaintext"]))
    );
    manager.stop();
}

#[test]
fn only_text_document_requests_name_the_document() {
    let (root, manager, file) = fixture();
    let symbols = smol::block_on(manager.request(
        "workspace/symbol",
        &file,
        "test",
        "fn main() {}",
        json!({"query": "main"}),
    ))
    .unwrap();
    assert_eq!(symbols.value, json!([]));
    let params = recorded(&root, "symbol-params.json");
    assert_eq!(params, json!({"query": "main"}));
    assert!(params.get("textDocument").is_none());

    let hover = smol::block_on(manager.request(
        "textDocument/hover",
        &file,
        "test",
        "fn main() {}",
        position_params("fn main() {}", 3).unwrap(),
    ))
    .unwrap();
    assert_eq!(hover.value, Value::Null);
    let params = recorded(&root, "hover-params.json");
    assert_eq!(
        params.pointer("/textDocument/uri"),
        Some(&json!(file_uri(&file).unwrap()))
    );
    manager.stop();
}
