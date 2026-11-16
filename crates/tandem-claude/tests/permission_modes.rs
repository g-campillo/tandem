//! Permission modes against the scripted fake helper: no Node, no Claude.

mod support;

use serde_json::{Value, json};
use std::sync::Arc;
use support::{
    Harness, event, expect, hello, open_ok, permission_request, preamble, reply_ok, turn_finished,
    turn_ok, turn_ok_with,
};
use tandem_claude::{ChatState, Command, TakeoverRequest};
use tandem_core::chat::{ChatTask, ItemKind, PermissionMode, Phase, Plan, PlanStep, StepKind};

const BYPASS_WHILE_WORKING: &str =
    "Wait for Claude to finish, or press Stop, before switching to or from Bypass.";
const AUTO_REFUSED: &str =
    "Cannot set permission mode to auto: auto mode unavailable for this model";
const BYPASS_REFUSED: &str = "Cannot set permission mode to bypassPermissions because it is disabled by settings or configuration";
const POLICIES: [&str; 4] = ["discuss", "guided", "check", "takeover"];

fn step(kind: StepKind, target: &str, summary: &str) -> PlanStep {
    PlanStep {
        kind,
        target: target.into(),
        summary: summary.into(),
        language: "rust".into(),
    }
}

/// Edit `src/cli.rs`, run `cargo add serde`, create `tests/cli.rs`; all Rust.
fn plan() -> Plan {
    Plan {
        goal: "Print results as JSON when --json is set".into(),
        done_when: vec![
            "`--json` prints valid JSON".into(),
            "The old output is unchanged".into(),
        ],
        steps: vec![
            step(
                StepKind::EditFile,
                "src/cli.rs",
                "Add a `json` flag to Args",
            ),
            step(StepKind::RunCommand, "cargo add serde", "Add serde"),
            step(StepKind::CreateFile, "tests/cli.rs", "Test the flag"),
        ],
    }
}

/// Stores a task with the three-step plan in `phase` at step index `current_step`.
fn store_task(harness: &Harness, phase: Phase, current_step: usize) -> ChatTask {
    let key = harness.key();
    harness.with_store(|store| {
        let mut task = store.create_chat_task(&key).unwrap();
        task.title = "Print results as JSON".into();
        task.phase = phase;
        task.current_step = current_step;
        task.plan = Some(plan());
        store.save_chat_task(&task).unwrap();
        task
    })
}

/// Before `start`: discuss runs on the fresh task Tandem creates, the other policies on a
/// stored task in Build at step 0.
fn prepare_for(harness: &Harness, policy: &str) {
    if policy != "discuss" {
        store_task(harness, Phase::Build, 0);
    }
}

/// Starts a turn in `policy` the way the user does.
fn start_turn_in(harness: &Harness, policy: &str) {
    harness.send(match policy {
        "discuss" | "guided" => Command::Send("hi".into()),
        "check" => Command::CheckMyWork,
        "takeover" => Command::Takeover {
            request: TakeoverRequest::Step,
            saved: Vec::new(),
        },
        _ => panic!("no such policy: {policy}"),
    });
}

/// A permission request for Claude's `Edit` tool on `path`.
fn edit_request(id: &str, path: &str) -> Value {
    json!({"send": {"type": "request", "id": id, "request": "permission", "tool": "Edit",
        "title": "Claude wants to use Edit", "detail": "", "command": null, "path": path}})
}

/// Expects a `turn` in `wanted`, accepts it, and reports that Claude Code refused that mode
/// before the turn started.
fn refused_turn(wanted: &str, reason: &str) -> Vec<Value> {
    vec![
        expect(json!({"type": "turn", "permission_mode": wanted})),
        reply_ok(json!({})),
        event(json!({"event": "permission_mode_fallback", "wanted": wanted, "reason": reason})),
        event(json!({"event": "turn_started", "turn": "$id"})),
    ]
}

fn has_permission_item(state: &ChatState) -> bool {
    state
        .items
        .iter()
        .any(|item| matches!(item.kind, ItemKind::Permission { .. }))
}

fn has_notice(state: &ChatState, text: &str) -> bool {
    state
        .items
        .iter()
        .any(|item| item.kind == ItemKind::Notice { text: text.into() })
}

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

#[test]
fn bypass_never_shows_a_card_in_any_policy() {
    for policy in POLICIES {
        let mut harness = Harness::new();
        save_mode(&harness, PermissionMode::Bypass);
        prepare_for(&harness, policy);
        let inside = harness.project.join("src/main.rs");
        harness.start(
            &[
                hello(),
                open_ok(json!({"permission_mode": "bypass"})),
                turn_ok_with(json!({"policy": policy, "permission_mode": "bypass"})),
                vec![
                    permission_request("r1", "npm install left-pad"),
                    expect(json!({"type": "response", "id": "r1", "allow": true})),
                    edit_request("r2", inside.to_str().unwrap()),
                    expect(json!({"type": "response", "id": "r2", "allow": true})),
                    assistant("done"),
                    turn_finished(3, "completed"),
                ],
            ]
            .concat(),
        );
        harness.wait_ready();
        start_turn_in(&harness, policy);

        let state = finished_with(&harness, "done");
        assert!(!has_permission_item(&state), "{policy}: {:#?}", state.items);
        assert_eq!(state.pending_permission, None, "{policy}");
        harness.finish();
    }
}

#[test]
fn ask_and_auto_still_show_the_card() {
    for (mode, wire) in [(PermissionMode::Ask, "ask"), (PermissionMode::Auto, "auto")] {
        let mut harness = Harness::new();
        save_mode(&harness, mode);
        harness.start(
            &[
                hello(),
                open_ok(json!({"permission_mode": wire})),
                turn_ok_with(json!({"policy": "discuss", "permission_mode": wire})),
                vec![
                    permission_request("r1", "npm install left-pad"),
                    expect(json!({"type": "response", "id": "r1", "allow": false})),
                    assistant("done"),
                    turn_finished(3, "completed"),
                ],
            ]
            .concat(),
        );
        harness.wait_ready();
        harness.send(Command::Send("hi".into()));

        let state = harness.wait_for("the card", |state| state.pending_permission.is_some());
        let card = state.items.last().unwrap();
        assert_eq!(state.pending_permission, Some(card.id), "{wire}");
        assert!(
            matches!(card.kind, ItemKind::Permission { allowed: None, .. }),
            "{wire}: {card:#?}"
        );
        harness.send(Command::Permission { allow: false });
        let state = finished_with(&harness, "done");
        assert_eq!(state.pending_permission, None, "{wire}");
        harness.finish();
    }
}

#[test]
fn a_refused_mode_falls_back_to_ask_and_says_so() {
    let mut harness = Harness::new();
    save_mode(&harness, PermissionMode::Auto);
    harness.start(
        &[
            hello(),
            open_ok(json!({"permission_mode": "auto"})),
            refused_turn("auto", AUTO_REFUSED),
            vec![
                permission_request("r1", "npm install left-pad"),
                expect(json!({"type": "response", "id": "r1", "allow": true})),
                assistant("first"),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"permission_mode": "ask"})),
            vec![assistant("second"), turn_finished(4, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));

    // The turn runs in Ask, so the card is shown.
    harness.wait_for("the card", |state| state.pending_permission.is_some());
    harness.send(Command::Permission { allow: true });
    let state = finished_with(&harness, "first");
    assert!(
        has_notice(
            &state,
            &format!(
                "Auto is not available, so Tandem is using Ask. Claude Code said: {AUTO_REFUSED}"
            )
        ),
        "{:#?}",
        state.items
    );
    assert_eq!(state.prefs.permission_mode, PermissionMode::Ask);
    assert_eq!(saved_mode(&harness), PermissionMode::Ask);

    // The next turn asks for Ask, in the same session.
    harness.send(Command::Send("again".into()));
    finished_with(&harness, "second");
    assert_eq!(harness.sent_of("turn")[1]["permission_mode"], "ask");
    assert_eq!(harness.sent_of("open_session").len(), 1);
    harness.finish();
}

#[test]
fn a_refused_bypass_shows_cards_again() {
    let mut harness = Harness::new();
    save_mode(&harness, PermissionMode::Bypass);
    harness.start(
        &[
            hello(),
            open_ok(json!({"permission_mode": "bypass"})),
            refused_turn("bypass", BYPASS_REFUSED),
            vec![
                permission_request("r1", "npm install left-pad"),
                expect(json!({"type": "response", "id": "r1", "allow": false})),
                assistant("done"),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));

    let state = harness.wait_for("the card", |state| state.pending_permission.is_some());
    let card = state.items.last().unwrap();
    assert!(
        matches!(card.kind, ItemKind::Permission { allowed: None, .. }),
        "{card:#?}"
    );
    harness.send(Command::Permission { allow: false });
    let state = finished_with(&harness, "done");
    assert!(
        has_notice(
            &state,
            &format!(
                "Bypass is not available, so Tandem is using Ask. Claude Code said: {BYPASS_REFUSED}"
            )
        ),
        "{:#?}",
        state.items
    );
    assert_eq!(state.prefs.permission_mode, PermissionMode::Ask);
    assert_eq!(saved_mode(&harness), PermissionMode::Ask);
    assert_eq!(harness.sent_of("open_session").len(), 1);
    harness.finish();
}

#[test]
fn a_bypass_chat_turn_says_when_claude_may_edit() {
    let mut harness = Harness::new();
    save_mode(&harness, PermissionMode::Bypass);
    harness.start(
        &[
            hello(),
            open_ok(json!({"permission_mode": "bypass"})),
            turn_ok_with(json!({"permission_mode": "bypass"})),
            vec![assistant("first"), turn_finished(3, "completed")],
            turn_ok_with(json!({"permission_mode": "bypass"})),
            vec![assistant("second"), turn_finished(4, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "first");
    harness.send(Command::Send("/review".into()));
    finished_with(&harness, "second");

    let turns = harness.sent_of("turn");
    assert_eq!(
        turns[0]["text"],
        format!("hi\n\n{}", tandem_claude::prompts::BYPASS_EDITS)
    );
    assert_eq!(turns[1]["text"], "/review");
    harness.finish();
}

#[test]
fn ask_and_auto_chat_turns_are_unchanged() {
    for (mode, wire) in [(PermissionMode::Ask, "ask"), (PermissionMode::Auto, "auto")] {
        let mut harness = Harness::new();
        save_mode(&harness, mode);
        harness.start(
            &[
                hello(),
                open_ok(json!({"permission_mode": wire})),
                turn_ok_with(json!({"permission_mode": wire})),
                vec![assistant("ok"), turn_finished(3, "completed")],
            ]
            .concat(),
        );
        harness.wait_ready();
        harness.send(Command::Send("hi".into()));
        finished_with(&harness, "ok");

        assert_eq!(harness.sent_of("turn")[0]["text"], "hi", "{wire}");
        harness.finish();
    }
}

#[test]
fn a_bypass_check_turn_still_forbids_edits() {
    let mut harness = Harness::new();
    save_mode(&harness, PermissionMode::Bypass);
    store_task(&harness, Phase::Build, 0);
    harness.start(
        &[
            hello(),
            open_ok(json!({"permission_mode": "bypass"})),
            turn_ok_with(json!({"policy": "check", "permission_mode": "bypass"})),
            vec![assistant("checked"), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);
    finished_with(&harness, "checked");

    let turn = &harness.sent_of("turn")[0];
    let text = turn["text"].as_str().expect("turn text");
    assert_eq!(turn["policy"], "check");
    assert!(text.contains("Do not edit files."), "{text}");
    assert!(!text.contains("Permissions are set to Bypass"), "{text}");
    harness.finish();
}
