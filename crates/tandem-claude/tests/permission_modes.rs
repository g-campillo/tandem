//! Permission modes against the scripted fake helper: no Node, no Claude.

mod support;

use serde_json::{Value, json};
use std::sync::Arc;
use support::{
    Harness, event, expect, hello, open_ok, preamble, reply_ok, turn_finished, turn_ok,
    turn_ok_with,
};
use tandem_claude::{ChatState, Command};
use tandem_core::chat::{ItemKind, PermissionMode};

const BYPASS_WHILE_WORKING: &str =
    "Wait for Claude to finish, or press Stop, before switching to or from Bypass.";

fn assistant(text: &str) -> Value {
    event(json!({"event": "assistant_text", "text": text}))
}

/// Waits until the assistant said `text` and the turn is over.
fn finished_with(harness: &Harness, text: &str) -> Arc<ChatState> {
    harness.wait_for(&format!("a finished turn ending with {text:?}"), |state| {
        state.turn.is_none()
            && state
                .items
                .iter()
                .any(|item| item.kind == ItemKind::Assistant { text: text.into() })
    })
}

fn saved_mode(harness: &Harness) -> PermissionMode {
    harness.with_store(|store| store.chat_prefs().unwrap().permission_mode)
}

fn save_mode(harness: &Harness, mode: PermissionMode) {
    harness.with_store(|store| {
        let mut prefs = store.chat_prefs().unwrap();
        prefs.permission_mode = mode;
        store.save_chat_prefs(&prefs).unwrap();
    });
}

/// Waits until Tandem has sent `count` `open_session` frames and returns them.
fn wait_opened(harness: &Harness, count: usize) -> Vec<Value> {
    harness
        .wait_sent(&format!("{count} open_session frames"), |sent| {
            sent.iter()
                .filter(|frame| frame["type"] == "open_session")
                .count()
                == count
        })
        .into_iter()
        .filter(|frame| frame["type"] == "open_session")
        .collect()
}

/// Expects Stop's `interrupt` and ends turn `turn` as interrupted.
fn stopped(turn: u64) -> Vec<Value> {
    vec![
        expect(json!({"type": "interrupt"})),
        reply_ok(json!({})),
        turn_finished(turn, "interrupted"),
    ]
}

#[test]
fn ask_is_the_default_on_open_session_and_turn() {
    let harness = Harness::started(
        &[
            hello(),
            open_ok(json!({"permission_mode": "ask"})),
            turn_ok_with(json!({"policy": "discuss", "permission_mode": "ask"})),
            vec![assistant("ok"), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    let state = harness.wait_ready();
    assert_eq!(state.prefs.permission_mode, PermissionMode::Ask);
    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "ok");
    harness.finish();
}

#[test]
fn a_saved_mode_is_used_at_start_up() {
    let mut harness = Harness::new();
    save_mode(&harness, PermissionMode::Bypass);
    harness.start(&[hello(), open_ok(json!({"permission_mode": "bypass"}))].concat());
    let state = harness.wait_ready();
    assert_eq!(state.prefs.permission_mode, PermissionMode::Bypass);
    harness.finish();
}

#[test]
fn auto_is_saved_and_applies_from_the_next_turn_without_reopening() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"permission_mode": "auto"})),
            vec![assistant("ok"), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::SetPermissionMode(PermissionMode::Auto));
    let state = harness.wait_for("Auto saved", |state| {
        state.prefs.permission_mode == PermissionMode::Auto
    });
    assert_eq!(saved_mode(&harness), PermissionMode::Auto);
    assert_eq!(state.banner, None);

    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "ok");
    assert_eq!(harness.sent_of("open_session").len(), 1);
    assert!(harness.sent_of("close_session").is_empty());
    harness.finish();
}

#[test]
fn switching_to_or_from_bypass_reopens_and_resumes_the_session() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"permission_mode": "ask"})),
            vec![
                assistant("first"),
                turn_finished(3, "completed"),
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"permission_mode": "bypass", "session_id": null})),
            turn_ok_with(json!({"permission_mode": "bypass"})),
            vec![
                assistant("second"),
                turn_finished(6, "completed"),
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"permission_mode": "auto", "session_id": null})),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "first");

    harness.send(Command::SetPermissionMode(PermissionMode::Bypass));
    wait_opened(&harness, 2);
    let state = harness.wait_ready();
    assert_eq!(state.prefs.permission_mode, PermissionMode::Bypass);
    assert_eq!(saved_mode(&harness), PermissionMode::Bypass);

    harness.send(Command::Send("again".into()));
    finished_with(&harness, "second");

    harness.send(Command::SetPermissionMode(PermissionMode::Auto));
    let opened = wait_opened(&harness, 3);
    let state = harness.wait_ready();
    assert_eq!(state.prefs.permission_mode, PermissionMode::Auto);
    assert_eq!(saved_mode(&harness), PermissionMode::Auto);

    let session = opened[0]["session_id"].as_str().expect("a new session");
    assert_eq!(opened[1]["resume"], session);
    assert_eq!(opened[2]["resume"], session);
    harness.finish();
}

#[test]
fn bypass_is_not_switched_while_claude_works() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"permission_mode": "ask"})),
            stopped(3),
            vec![
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"permission_mode": "bypass", "session_id": null})),
            turn_ok(),
            stopped(7),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    harness.wait_for("a running turn", |state| state.turn.is_some());

    // Into Bypass while Claude works: refused, nothing saved, nothing reopened.
    harness.send(Command::SetPermissionMode(PermissionMode::Bypass));
    let state = harness.wait_for("the refusal", |state| {
        state.banner.as_deref() == Some(BYPASS_WHILE_WORKING)
    });
    assert_eq!(state.prefs.permission_mode, PermissionMode::Ask);
    assert_eq!(saved_mode(&harness), PermissionMode::Ask);
    assert!(harness.sent_of("close_session").is_empty());

    // Ask to Auto needs no reopen, so it is saved at once.
    harness.send(Command::SetPermissionMode(PermissionMode::Auto));
    harness.wait_for("Auto saved", |state| {
        state.prefs.permission_mode == PermissionMode::Auto
    });
    assert_eq!(saved_mode(&harness), PermissionMode::Auto);
    assert!(harness.sent_of("close_session").is_empty());

    harness.send(Command::Stop);
    harness.wait_for("the stopped turn", |state| state.turn.is_none());

    // Idle, Bypass is switched on and the session reopened for it.
    harness.send(Command::SetPermissionMode(PermissionMode::Bypass));
    wait_opened(&harness, 2);
    harness.wait_for("Bypass saved", |state| {
        state.prefs.permission_mode == PermissionMode::Bypass
    });
    harness.send(Command::Send("again".into()));
    harness.wait_for("a running turn", |state| state.turn.is_some());

    // Out of Bypass while Claude works: refused the same way.
    harness.send(Command::SetPermissionMode(PermissionMode::Ask));
    let state = harness.wait_for("the refusal", |state| {
        state.banner.as_deref() == Some(BYPASS_WHILE_WORKING)
    });
    assert_eq!(state.prefs.permission_mode, PermissionMode::Bypass);
    assert_eq!(saved_mode(&harness), PermissionMode::Bypass);
    assert_eq!(harness.sent_of("close_session").len(), 1);

    harness.send(Command::Stop);
    harness.wait_for("the stopped turn", |state| state.turn.is_none());
    harness.finish();
}
