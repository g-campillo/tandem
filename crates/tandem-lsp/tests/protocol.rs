//! How a session speaks to servers other than rust-analyzer and Pyright: per-document language
//! ids, capability overlays, server requests, diagnostic modes and start timeouts.
mod support;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{fake_server, wait_for};
use tandem_lsp::*;

const TEXT: &str = "x";

fn open(manager: &LanguageManager, files: &[PathBuf]) {
    manager.set_documents(
        files
            .iter()
            .map(|path| LanguageDocument {
                path: path.clone(),
                instance: "test".into(),
                text: Arc::from(TEXT),
            })
            .collect(),
    );
}

/// The JSON the fake server recorded under `name`, once it is complete.
fn record(dir: &Path, name: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(name)).ok()?).ok()
}

fn wait_for_record(dir: &Path, name: &str) -> Value {
    wait_for(|| record(dir, name).is_some());
    record(dir, name).unwrap()
}

#[test]
fn documents_open_with_their_own_language_ids() {
    let dir = tempfile::tempdir().unwrap();
    let manager = LanguageManager::start(fake_server(dir.path(), &[]));
    let expected = [
        ("a.tsx", "typescriptreact"),
        ("b.jsx", "javascriptreact"),
        ("c.ts", "typescript"),
        ("d.json", "json"),
        ("tsconfig.json", "jsonc"),
        ("e.scss", "scss"),
    ];
    let files: Vec<_> = expected
        .iter()
        .map(|(name, _)| dir.path().join(name))
        .collect();
    open(&manager, &files);
    wait_for(|| {
        record(dir.path(), "language-ids").and_then(|ids| ids.as_object().map(|ids| ids.len()))
            == Some(expected.len())
    });
    let ids = record(dir.path(), "language-ids").unwrap();
    for (file, (name, id)) in files.iter().zip(expected) {
        assert_eq!(ids[file_uri(file).unwrap()], json!(id), "{name}");
    }
    manager.stop();
}

#[test]
fn unknown_server_requests_get_a_null_result() {
    let dir = tempfile::tempdir().unwrap();
    let manager = LanguageManager::start(fake_server(dir.path(), &[]));
    let reply = wait_for_record(dir.path(), "unknown-reply");
    assert_eq!(reply["id"], json!(9001));
    assert_eq!(reply.get("result"), Some(&Value::Null), "{reply}");
    assert!(reply.get("error").is_none(), "{reply}");
    manager.stop();
}

#[test]
fn capability_overlay_adds_to_the_client_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &[]);
    config.capabilities = json!({"textDocument":{"diagnostic":{"dynamicRegistration":true}}});
    let manager = LanguageManager::start(config);
    let initialize = wait_for_record(dir.path(), "initialize");
    assert_eq!(
        initialize.pointer("/capabilities/textDocument/diagnostic/dynamicRegistration"),
        Some(&json!(true))
    );
    assert_eq!(
        initialize.pointer("/capabilities/textDocument/hover/contentFormat"),
        Some(&json!(["markdown", "plaintext"]))
    );
    manager.stop();
}

/// Starts the fake server with one open document and waits until it is ready.
fn started(dir: &Path, config: ServerConfig) -> (LanguageManager, PathBuf) {
    let file = dir.join("main.swift");
    let manager = LanguageManager::start(config);
    open(&manager, std::slice::from_ref(&file));
    wait_for(|| manager.health().state == "ready");
    (manager, file)
}

fn diagnostics(manager: &LanguageManager, file: &Path) -> anyhow::Result<Value> {
    smol::block_on(manager.request("textDocument/diagnostic", file, "test", TEXT, json!({})))
        .map(|reply| reply.value)
}

fn messages(value: &Value) -> Vec<&str> {
    value["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["message"].as_str().unwrap())
        .collect()
}

#[test]
fn dynamically_registered_pull_is_used_in_settled_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &["dynamic-pull"]);
    config.diagnostics = DiagnosticMode::Settled;
    let (manager, file) = started(dir.path(), config);
    let registered = wait_for_record(dir.path(), "registered");
    assert_eq!(registered.get("result"), Some(&Value::Null), "{registered}");
    let value = diagnostics(&manager, &file).unwrap();
    assert_eq!(messages(&value), ["pulled"]);
    manager.stop();
}

#[test]
fn unregistered_pull_falls_back_to_pushes_in_settled_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &["dynamic-pull", "unregister", "push"]);
    config.diagnostics = DiagnosticMode::Settled;
    let (manager, file) = started(dir.path(), config);
    wait_for_record(dir.path(), "unregistered");
    let value = diagnostics(&manager, &file).unwrap();
    assert_eq!(messages(&value), ["pushed"]);
    manager.stop();
}

#[test]
fn versioned_mode_ignores_registration_and_unversioned_pushes() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &["dynamic-pull", "push"]);
    config.diagnostics = DiagnosticMode::Versioned;
    config.request_timeout = Duration::from_millis(500);
    let (manager, file) = started(dir.path(), config);
    wait_for_record(dir.path(), "registered");
    let error = diagnostics(&manager, &file).unwrap_err();
    assert!(error.to_string().contains("not yet available"), "{error:#}");
    manager.stop();
}

#[test]
fn settled_mode_accepts_the_latest_unversioned_push() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &["push"]);
    config.diagnostics = DiagnosticMode::Settled;
    let (manager, file) = started(dir.path(), config);
    let asked = Instant::now();
    let value = diagnostics(&manager, &file).unwrap();
    assert_eq!(messages(&value), ["pushed"]);
    assert!(
        asked.elapsed() < Duration::from_secs(2),
        "{:?}",
        asked.elapsed()
    );
    manager.stop();
}

#[test]
fn a_server_that_does_not_answer_initialize_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_server(dir.path(), &["slow-init"]);
    config.start_timeout = Duration::from_millis(300);
    let manager = LanguageManager::start(config);
    wait_for(|| {
        let health = manager.health();
        matches!(health.state.as_str(), "restarting" | "failed")
            && health.detail.contains("did not start")
    });
    assert_eq!(manager.health().detail, "fake did not start within 0.3 s");
    manager.stop();
}
