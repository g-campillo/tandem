use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tandem_lsp::*;

fn fixture() -> (tempfile::TempDir, LanguageManager, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.py");
    std::fs::write(&script,r#"#!/usr/bin/python3
import sys,json,time,pathlib
root=pathlib.Path(__file__).parent
def send(value):
    body=json.dumps(value).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body);sys.stdout.buffer.flush()
while True:
    line=sys.stdin.buffer.readline()
    if not line: break
    length=int(line.decode().split(':')[1]);sys.stdin.buffer.readline();msg=json.loads(sys.stdin.buffer.read(length))
    method=msg.get('method');ident=msg.get('id')
    if method=='initialize':
        send({'jsonrpc':'2.0','id':ident,'result':{'capabilities':{'diagnosticProvider':{}}}})
        send({'jsonrpc':'2.0','id':9000,'method':'workspace/applyEdit','params':{'edit':{}}})
    elif ident==9000:
        if msg.get('result',{}).get('applied')==False: (root/'edit-denied').write_text('yes')
    elif method=='textDocument/diagnostic':
        (root/'request-started').write_text('yes');time.sleep(.3)
        send({'jsonrpc':'2.0','id':ident,'result':{'kind':'full','items':[]}})
    elif method=='textDocument/documentSymbol':
        if not (root/'crashed').exists(): (root/'crashed').write_text('yes');sys.exit(1)
        send({'jsonrpc':'2.0','id':ident,'result':[]})
    elif method=='$/cancelRequest': (root/'cancelled').write_text('yes')
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

#[test]
fn stale_response_is_rejected_and_unsolicited_edits_are_denied() {
    let (root, manager, file) = fixture();
    let worker = manager.clone();
    let requested = file.clone();
    let response = std::thread::spawn(move || {
        smol::block_on(worker.request(
            "textDocument/diagnostic",
            &requested,
            "test",
            "fn main() {}",
            json!({}),
        ))
    });
    wait_for(|| root.path().join("request-started").exists());
    manager.set_documents(vec![LanguageDocument {
        path: file,
        instance: "test".into(),
        text: Arc::from("fn changed() {}"),
    }]);
    assert!(
        response
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("superseded")
    );
    wait_for(|| root.path().join("edit-denied").exists());
    let stale = smol::block_on(manager.request(
        "textDocument/diagnostic",
        &requested_path(&root),
        "test",
        "fn main() {}",
        json!({}),
    ));
    assert!(stale.unwrap_err().to_string().contains("superseded"));
    let current = smol::block_on(manager.request(
        "textDocument/diagnostic",
        &requested_path(&root),
        "test",
        "fn changed() {}",
        json!({}),
    ))
    .unwrap();
    assert_eq!(current.stamp.documents[0].text.as_ref(), "fn changed() {}");
    manager.stop();
}

fn requested_path(root: &tempfile::TempDir) -> PathBuf {
    root.path().join("main.rs")
}
#[test]
fn cancelled_requests_signal_server_and_server_crashes_recover() {
    let (root, manager, file) = fixture();
    smol::block_on(async {
        let response = manager.request(
            "textDocument/diagnostic",
            &file,
            "test",
            "fn main() {}",
            json!({}),
        );
        smol::future::race(
            async {
                let _ = response.await;
            },
            async {
                smol::Timer::after(Duration::from_millis(100)).await;
            },
        )
        .await;
    });
    wait_for(|| root.path().join("cancelled").exists());
    let failed = smol::block_on(manager.request(
        "textDocument/documentSymbol",
        &file,
        "test",
        "fn main() {}",
        json!({}),
    ));
    assert!(failed.is_err());
    // Observe one atomic health snapshot: separate reads can combine the old
    // ready state with the newly incremented restart count during transition.
    wait_for(|| {
        let health = manager.health();
        health.state == "ready" && health.restarts > 0
    });
    let recovered = smol::block_on(manager.request(
        "textDocument/documentSymbol",
        &file,
        "test",
        "fn main() {}",
        json!({}),
    ));
    assert!(
        recovered.is_ok(),
        "Recovery failed: {recovered:?}; health: {:?}",
        manager.health()
    );
    manager.stop();
}
