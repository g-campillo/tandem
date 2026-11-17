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
command=None
while True:
    line=sys.stdin.buffer.readline()
    if not line: break
    length=int(line.decode().split(':')[1]);sys.stdin.buffer.readline();msg=json.loads(sys.stdin.buffer.read(length))
    method=msg.get('method');ident=msg.get('id')
    if method=='initialize':
        record('client-capabilities.json',msg['params']['capabilities'])
        record('roots.json',{'rootUri':msg['params'].get('rootUri'),'rootPath':msg['params'].get('rootPath')})
        send({'jsonrpc':'2.0','id':ident,'result':{'capabilities':{'renameProvider':{'prepareProvider':True},'completionProvider':{'triggerCharacters':['.',':'],'resolveProvider':True},'executeCommandProvider':{'commands':['test.apply']}}}})
    elif method=='workspace/symbol':
        record('symbol-params.json',msg['params'])
        send({'jsonrpc':'2.0','id':ident,'result':[]})
    elif method=='textDocument/hover':
        record('hover-params.json',msg['params'])
        send({'jsonrpc':'2.0','id':ident,'result':None})
    elif method=='workspace/executeCommand':
        command=ident;uri=msg['params']['arguments'][0]
        edit_id=9100 if msg['params']['command']=='test.apply' else 9200
        send({'jsonrpc':'2.0','id':edit_id,'method':'workspace/applyEdit','params':{'edit':{'changes':{uri:[{'range':{'start':{'line':0,'character':3},'end':{'line':0,'character':7}},'newText':'x'}]}}}})
    elif ident==9100 and method is None:
        record('applied.json',msg['result']['applied'])
        send({'jsonrpc':'2.0','id':command,'result':{'done':True}})
    elif ident==9200 and method is None:
        send({'jsonrpc':'2.0','id':command,'error':{'code':-32603,'message':'Command failed'}})
        send({'jsonrpc':'2.0','id':9201,'method':'workspace/applyEdit','params':{'edit':{}}})
    elif ident==9201 and method is None:
        record('late-applied.json',msg['result']['applied'])
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

/// Pyright makes a workspace of the project only from `rootPath` (or workspace folders),
/// not from `rootUri`; without one it renames in the open file alone.
#[test]
fn the_project_root_is_sent_as_a_uri_and_as_a_path() {
    let (root, manager, _file) = fixture();
    let sent = recorded(&root, "roots.json");
    assert_eq!(
        sent["rootUri"],
        json!(file_uri(root.path()).unwrap()),
        "{sent}"
    );
    assert_eq!(sent["rootPath"], json!(root.path()), "{sent}");
    manager.stop();
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

#[test]
fn a_running_command_may_apply_edits_and_they_come_back_with_its_result() {
    let (root, manager, file) = fixture();
    let uri = file_uri(&file).unwrap();
    let reply = smol::block_on(manager.request(
        "workspace/executeCommand",
        &file,
        "test",
        "fn main() {}",
        json!({"command": "test.apply", "arguments": [uri]}),
    ))
    .unwrap();
    let edit = json!({"range": {"start": {"line": 0, "character": 3},
        "end": {"line": 0, "character": 7}}, "newText": "x"});
    assert_eq!(
        reply.value,
        json!({"result": {"done": true}, "edits": [{"changes": {uri: [edit]}}]})
    );
    assert_eq!(recorded(&root, "applied.json"), json!(true));
    let files = features::edits::command_edits(&reply.value, &file).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, file);
    assert_eq!(
        apply_text_edits("fn main() {}", &files[0].edits).unwrap(),
        "fn x() {}"
    );
    manager.stop();
}

#[test]
fn edits_after_a_failed_command_are_denied() {
    let (root, manager, file) = fixture();
    let failed = smol::block_on(manager.request(
        "workspace/executeCommand",
        &file,
        "test",
        "fn main() {}",
        json!({"command": "test.fail", "arguments": [file_uri(&file).unwrap()]}),
    ));
    assert!(failed.is_err());
    assert_eq!(recorded(&root, "late-applied.json"), json!(false));
    manager.stop();
}
