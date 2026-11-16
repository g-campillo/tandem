//! The companion controller against the scripted fake helper: no Node, no Claude.

mod support;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use support::{
    Harness, event, expect, hello, open_ok, permission_request, preamble, reply_error, reply_ok,
    session_data, tool_request, turn_finished, turn_ok, turn_ok_with,
};
use tandem_claude::prompts::SYSTEM_APPEND;
use tandem_claude::view::plan_actions_visible;
use tandem_claude::{BufferText, ChatState, Command, Connection, Launch, TurnKind};
use tandem_core::chat::{
    ActivityKind, ChatItem, ItemKind, NEW_TASK_TITLE, Phase, Plan, PlanStep, StepKind,
};

const PLAN_SHOWN: &str = "The plan is on screen with two choices: \"I'll write it\" and \"You do it\". Stop here and do not implement anything. Finish with one short sentence asking the user to review the plan.";
const PLAN_LOCKED: &str = "The plan cannot change during Build. Adapt inside the current step, or tell the user to start a new task for a different plan.";

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

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(index, character)| match index {
            8 | 13 | 18 | 23 => character == '-',
            _ => character.is_ascii_hexdigit(),
        })
}

/// The `response` frame Tandem sent for helper request `id`.
fn response(harness: &Harness, id: &str) -> Value {
    harness
        .sent_of("response")
        .into_iter()
        .find(|frame| frame["id"] == id)
        .unwrap_or_else(|| panic!("no response to {id}"))
}

fn kinds(state: &ChatState) -> Vec<&ItemKind> {
    state.items.iter().map(|item| &item.kind).collect()
}

fn plan_items(state: &ChatState) -> Vec<&ChatItem> {
    state
        .items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Plan { .. }))
        .collect()
}

fn plan_input(goal: &str, target: &str) -> Value {
    json!({
        "goal": goal,
        "done_when": ["The output parses as JSON", "The old output is unchanged"],
        "steps": [
            {"kind": "edit_file", "target": target, "summary": "Add `json` to Args",
             "language": "python"},
            {"kind": "run_command", "target": "cargo test", "summary": "Run the tests",
             "language": "shell"}
        ]
    })
}

fn build_plan() -> Plan {
    Plan {
        goal: "Print results as JSON".into(),
        done_when: vec!["The output parses as JSON".into()],
        steps: vec![PlanStep {
            kind: StepKind::EditFile,
            target: "src/cli.rs".into(),
            summary: "Add `json` to Args".into(),
            language: "rust".into(),
        }],
    }
}

#[test]
fn start_up_opens_a_new_session_for_a_new_task() {
    let mut harness = Harness::new();
    let open = json!({
        "cwd": harness.key(), "resume": null, "use_my_setup": true, "model": "default",
        "effort": null, "system_append": SYSTEM_APPEND
    });
    harness.start(&[hello(), open_ok(open)].concat());

    let state = harness.wait_ready();
    assert_eq!(
        state.connection,
        Connection::Ready {
            subscription: "Claude Ultra".into(),
            claude_code: "2.1.325".into()
        }
    );
    let models: Vec<&str> = state
        .models
        .iter()
        .map(|model| model.value.as_str())
        .collect();
    assert_eq!(models, ["default", "opus", "haiku"]);
    assert!(
        state.install.is_none(),
        "a custom launch has no Node install"
    );

    let task = state.task.clone().expect("a task");
    assert_eq!(task.title, NEW_TASK_TITLE);
    assert_eq!(task.phase, Phase::Discuss);
    assert_eq!(state.tasks.len(), 1);
    assert_eq!(state.tasks[0].id, task.id);
    let selected = harness.with_store(|store| store.selected_chat_task(&harness.key()).unwrap());
    assert_eq!(selected.as_deref(), Some(task.id.as_str()));

    let opened = harness.sent_of("open_session");
    assert_eq!(opened.len(), 1);
    let session_id = opened[0]["session_id"].as_str().expect("a session id");
    assert!(is_uuid(session_id), "{session_id}");
    assert_eq!(session_id, task.session_id);
    assert!(!task.session_started);
    harness.finish();
}

#[test]
fn a_chat_turn_streams_records_and_names_the_task() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({
                "policy": "discuss", "text": "Add a --json flag", "model": "default", "effort": null
            })),
            vec![
                event(json!({"event": "text_delta", "text": "Let me "})),
                event(json!({"event": "text_delta", "text": "look."})),
                json!({"sleep_ms": 1000}),
                event(json!({"event": "activity", "kind": "read", "label": "src/cli.rs"})),
                assistant("Should --json also change error output?"),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("  Add a --json flag\n".into()));

    let streaming = harness.wait_for("streaming text", |state| {
        state.streaming.as_deref() == Some("Let me look.")
    });
    assert_eq!(streaming.turn, Some(TurnKind::Chat));

    let state = finished_with(&harness, "Should --json also change error output?");
    assert_eq!(
        kinds(&state),
        [
            &ItemKind::User {
                text: "Add a --json flag".into()
            },
            &ItemKind::Activity {
                kind: ActivityKind::Read,
                label: "src/cli.rs".into()
            },
            &ItemKind::Assistant {
                text: "Should --json also change error output?".into()
            },
        ]
    );
    assert_eq!(state.streaming, None);
    assert_eq!(state.banner, None);
    let task = state.task.clone().unwrap();
    assert_eq!(task.title, "Add a --json flag");
    assert!(task.session_started);
    assert_eq!(state.tasks[0].title, "Add a --json flag");

    let stored = harness.with_store(|store| store.chat_items(&task.id).unwrap());
    assert_eq!(stored, state.items);
    let saved = harness.with_store(|store| store.chat_task(&task.id).unwrap().unwrap());
    assert_eq!(saved.title, "Add a --json flag");
    assert!(saved.session_started);
    harness.finish();
}

#[test]
fn a_long_first_line_is_cut_for_the_title() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![assistant("ok"), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    let first_line = "é".repeat(70);
    harness.send(Command::Send(format!("{first_line}\nsecond line")));
    let state = finished_with(&harness, "ok");
    assert_eq!(state.task.as_ref().unwrap().title, "é".repeat(60));
    harness.finish();
}

#[test]
fn a_proposed_plan_moves_the_task_to_plan_and_can_be_replaced() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "discuss"})),
            vec![
                tool_request(
                    "r1",
                    "propose_plan",
                    plan_input("Print results as JSON when --json is set", "src/cli.rs"),
                ),
                expect(json!({"type": "response", "id": "r1", "ok": true})),
                assistant("Please review the plan."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "discuss", "text": "Smaller, please"})),
            vec![
                tool_request(
                    "r2",
                    "propose_plan",
                    plan_input("Print JSON for --json", "src/main.rs"),
                ),
                expect(json!({"type": "response", "id": "r2", "ok": true})),
                tool_request("r3", "propose_plan", plan_input("Bad plan", "/etc/hosts")),
                expect(json!({"type": "response", "id": "r3", "ok": false})),
                tool_request("r4", "propose_plan", json!({"goal": "No steps"})),
                expect(json!({"type": "response", "id": "r4", "ok": false})),
                assistant("Here is the smaller plan."),
                turn_finished(4, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Plan a --json flag".into()));
    let state = finished_with(&harness, "Please review the plan.");

    assert_eq!(response(&harness, "r1")["text"], PLAN_SHOWN);
    let task = state.task.clone().unwrap();
    assert_eq!(task.phase, Phase::Plan);
    assert_eq!(task.title, "Print results as JSON when --json is set");
    let plan = task.plan.clone().expect("a plan");
    assert_eq!(
        plan.steps[0].language, "rust",
        "the path decides the language"
    );
    let first = plan_items(&state);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, ItemKind::Plan { plan: plan.clone() });
    assert!(plan_actions_visible(&state, first[0].id));
    let saved = harness.with_store(|store| store.chat_task(&task.id).unwrap().unwrap());
    assert_eq!(saved.phase, Phase::Plan);
    assert_eq!(saved.plan.as_ref(), Some(&plan));

    harness.send(Command::Send("Smaller, please".into()));
    let state = finished_with(&harness, "Here is the smaller plan.");
    let plans = plan_items(&state);
    assert_eq!(plans.len(), 2);
    assert!(!plan_actions_visible(&state, plans[0].id));
    assert!(plan_actions_visible(&state, plans[1].id));
    let task = state.task.clone().unwrap();
    assert_eq!(task.plan.as_ref().unwrap().goal, "Print JSON for --json");
    assert_eq!(task.title, "Print JSON for --json");

    let rejected = response(&harness, "r3");
    assert_eq!(rejected["ok"], false);
    assert_eq!(
        rejected["text"],
        "Step 1: the target must be a path inside the project, not \"/etc/hosts\"."
    );
    let malformed = response(&harness, "r4");
    assert!(
        malformed["text"]
            .as_str()
            .unwrap()
            .starts_with("The plan does not have the expected fields:"),
        "{malformed}"
    );
    harness.finish();
}

#[test]
fn a_plan_proposal_during_build_is_refused() {
    let mut harness = Harness::new();
    let key = harness.key();
    harness.with_store(|store| {
        let mut task = store.create_chat_task(&key).unwrap();
        task.title = "Print results as JSON".into();
        task.phase = Phase::Build;
        task.plan = Some(build_plan());
        store.save_chat_task(&task).unwrap();
    });
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                tool_request(
                    "r1",
                    "propose_plan",
                    plan_input("Something else", "src/other.rs"),
                ),
                expect(json!({"type": "response", "id": "r1", "ok": false, "text": PLAN_LOCKED})),
                assistant("The plan is fixed."),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Can we change the plan?".into()));
    let state = finished_with(&harness, "The plan is fixed.");

    let task = state.task.clone().unwrap();
    assert_eq!(task.phase, Phase::Build);
    assert_eq!(task.plan, Some(build_plan()));
    assert!(plan_items(&state).is_empty());
    let turn = &harness.sent_of("turn")[0];
    assert_eq!(
        turn["text"],
        "[Tandem] Guided build. Step 1 of 1 is active (Edit src/cli.rs). The user writes the code: answer their question and do not edit files.\n\nCan we change the plan?"
    );
    harness.finish();
}

#[test]
fn an_unknown_tool_is_not_available() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![
                tool_request("r1", "show_step", json!({"step": 1})),
                expect(json!({"type": "response", "id": "r1", "ok": false,
                    "text": "This tool is not available right now."})),
                assistant("ok"),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "ok");
    harness.finish();
}

#[test]
fn stop_interrupts_the_turn() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![
                expect(json!({"type": "interrupt"})),
                reply_ok(json!({})),
                turn_finished(3, "interrupted"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Start a long job".into()));
    harness.wait_for("the turn to start", |state| state.turn.is_some());
    harness.send(Command::Stop);

    let state = harness.wait_for("the Stopped notice", |state| {
        state.turn.is_none()
            && state.items.last().map(|item| &item.kind)
                == Some(&ItemKind::Notice {
                    text: "Stopped.".into(),
                })
    });
    assert!(!state.stopping);
    assert_eq!(state.banner, None);
    harness.finish();
}

#[test]
fn a_turn_error_becomes_a_banner_and_a_notice() {
    let limit = "Your Claude plan's usage limit was reached. Try again later.";
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![event(
                json!({"event": "turn_finished", "turn": 3, "status": "error",
                "error": limit}),
            )],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    let state = harness.wait_for("the error banner", |state| {
        state.turn.is_none() && state.banner.as_deref() == Some(limit)
    });
    assert_eq!(
        state.items.last().unwrap().kind,
        ItemKind::Notice { text: limit.into() }
    );
    harness.finish();
}

#[test]
fn a_refused_account_never_starts_a_turn() {
    let detail =
        "Claude Code would bill an API key (ANTHROPIC_API_KEY) instead of your subscription.";
    let mut session = session_data();
    session["auth"] = json!({"ok": false, "reason": "not_subscription", "detail": detail});
    session["models"] = json!([]);
    let harness = Harness::started(
        &[
            hello(),
            vec![
                expect(json!({"type": "open_session"})),
                reply_ok(session),
                expect(json!({"type": "close_session"})),
                expect(json!({"type": "shutdown"})),
                json!({"expect_eof": true}),
            ],
        ]
        .concat(),
    );
    let state = harness.wait_for("the refusal", |state| {
        matches!(state.connection, Connection::Refused { .. })
    });
    assert_eq!(
        state.connection,
        Connection::Refused {
            reason: detail.into()
        }
    );

    harness.send(Command::Send("hi".into()));
    let state = harness.wait_for("the banner", |state| state.banner.is_some());
    assert_eq!(state.banner.as_deref(), Some("Claude is not connected."));
    assert!(state.items.is_empty());
    assert!(harness.sent_of("turn").is_empty());
    harness.finish();
}

#[test]
fn a_signed_out_account_shows_the_sign_in_command() {
    let mut session = session_data();
    session["auth"] = json!({"ok": false, "reason": "signed_out",
        "detail": "Claude Code is not signed in."});
    let harness = Harness::started(
        &[
            hello(),
            vec![expect(json!({"type": "open_session"})), reply_ok(session)],
        ]
        .concat(),
    );
    let state = harness.wait_for("signed out", |state| {
        matches!(state.connection, Connection::SignedOut { .. })
    });
    let Connection::SignedOut { hint } = &state.connection else {
        unreachable!()
    };
    assert!(hint.contains("auth login"), "{hint}");
    harness.finish();
}

#[test]
fn auth_refused_during_a_turn_ends_it() {
    let detail = "Claude Code started with an API key (ANTHROPIC_API_KEY).";
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![event(
                json!({"event": "auth_refused", "reason": "not_subscription",
                "detail": detail}),
            )],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    let state = harness.wait_for("the refusal", |state| {
        matches!(state.connection, Connection::Refused { .. })
    });
    assert_eq!(
        state.connection,
        Connection::Refused {
            reason: detail.into()
        }
    );
    assert_eq!(state.turn, None);
    harness.finish();
}

#[test]
fn permission_cards_are_answered_cancelled_and_denied_by_stop() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![
                permission_request("r1", "npm install left-pad"),
                expect(json!({"type": "response", "id": "r1", "allow": true})),
                permission_request("r2", "rm -rf build"),
                expect(json!({"type": "response", "id": "r2", "allow": false})),
                permission_request("r3", "make deploy"),
                event(json!({"event": "permission_cancelled", "id": "r3"})),
                assistant("Installed."),
                turn_finished(3, "completed"),
            ],
            turn_ok(),
            vec![
                permission_request("r4", "npm publish"),
                expect(json!({"type": "response", "id": "r4", "allow": false})),
                expect(json!({"type": "interrupt"})),
                reply_ok(json!({})),
                turn_finished(4, "interrupted"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Install left-pad".into()));

    let state = harness.wait_for("the first card", |state| state.pending_permission.is_some());
    let card = state.items.last().unwrap();
    assert_eq!(state.pending_permission, Some(card.id));
    assert_eq!(
        card.kind,
        ItemKind::Permission {
            title: "Claude wants to run npm install left-pad".into(),
            detail: String::new(),
            command: Some("npm install left-pad".into()),
            allowed: None,
        }
    );
    let first = card.id;
    harness.send(Command::Permission { allow: true });

    let state = harness.wait_for("the second card", |state| {
        state.pending_permission.is_some_and(|id| id != first)
    });
    let second = state.pending_permission.unwrap();
    harness.send(Command::Permission { allow: false });

    let state = finished_with(&harness, "Installed.");
    assert_eq!(state.pending_permission, None);
    let answers: Vec<(String, Option<bool>)> = state
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Permission {
                command, allowed, ..
            } => Some((command.clone().unwrap(), *allowed)),
            _ => None,
        })
        .collect();
    assert_eq!(
        answers,
        [
            ("npm install left-pad".to_owned(), Some(true)),
            ("rm -rf build".to_owned(), Some(false)),
            ("make deploy".to_owned(), Some(false)),
        ]
    );
    assert!(state.items.iter().any(|item| item.id == second));
    let task_id = state.task.as_ref().unwrap().id.clone();
    let stored = harness.with_store(|store| store.chat_items(&task_id).unwrap());
    assert_eq!(stored, state.items);

    harness.send(Command::Send("Publish it".into()));
    harness.wait_for("the publish card", |state| {
        state.pending_permission.is_some()
    });
    harness.send(Command::Stop);
    let state = harness.wait_for("the stopped turn", |state| {
        state.turn.is_none()
            && state.items.last().map(|item| &item.kind)
                == Some(&ItemKind::Notice {
                    text: "Stopped.".into(),
                })
    });
    assert_eq!(state.pending_permission, None);
    let publish = state
        .items
        .iter()
        .rev()
        .find(|item| matches!(item.kind, ItemKind::Permission { .. }))
        .unwrap();
    assert!(matches!(
        publish.kind,
        ItemKind::Permission {
            allowed: Some(false),
            ..
        }
    ));
    let sent = harness.sent();
    let deny = sent
        .iter()
        .position(|frame| frame["type"] == "response" && frame["id"] == "r4")
        .unwrap();
    let interrupt = sent
        .iter()
        .position(|frame| frame["type"] == "interrupt")
        .unwrap();
    assert!(deny < interrupt);
    harness.finish();
}

#[test]
fn model_and_effort_are_saved_and_carried_by_the_next_turn() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"model": "opus", "effort": "max"})),
            vec![assistant("ok"), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::SetModel("opus".into()));
    harness.send(Command::SetEffort(Some("max".into())));
    let state = harness.wait_for("opus at max", |state| {
        state.prefs.model == "opus" && state.prefs.effort.as_deref() == Some("max")
    });
    assert!(state.banner.is_none());
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!(
        (saved.model.as_str(), saved.effort.as_deref()),
        ("opus", Some("max"))
    );
    harness.send(Command::Send("hi".into()));
    finished_with(&harness, "ok");

    harness.send(Command::SetModel("haiku".into()));
    let state = harness.wait_for("haiku", |state| state.prefs.model == "haiku");
    assert_eq!(state.prefs.effort, None);
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!((saved.model.as_str(), saved.effort), ("haiku", None));

    harness.send(Command::SetModel("nope".into()));
    let state = harness.wait_for("a banner", |state| state.banner.is_some());
    assert_eq!(
        state.banner.as_deref(),
        Some("That model is not available.")
    );
    assert_eq!(state.prefs.model, "haiku");

    harness.send(Command::SetModel("default".into()));
    harness.send(Command::SetEffort(Some("max".into())));
    let state = harness.wait_for("the effort refusal", |state| {
        state.banner.as_deref() == Some("That effort is not available for this model.")
    });
    assert_eq!(state.prefs.model, "default");
    assert_eq!(state.prefs.effort, None);
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!((saved.model.as_str(), saved.effort), ("default", None));
    harness.finish();
}

#[test]
fn a_saved_model_that_disappeared_falls_back_to_default() {
    let mut harness = Harness::new();
    harness.with_store(|store| {
        let mut prefs = store.chat_prefs().unwrap();
        prefs.model = "retired".into();
        prefs.effort = Some("high".into());
        store.save_chat_prefs(&prefs).unwrap();
    });
    harness.start(
        &[
            hello(),
            open_ok(json!({"model": "retired", "effort": "high"})),
        ]
        .concat(),
    );
    let state = harness.wait_ready();
    assert_eq!(
        (state.prefs.model.as_str(), state.prefs.effort.as_deref()),
        ("default", None)
    );
    assert_eq!(
        state.banner.as_deref(),
        Some("The saved model is no longer available; using Default.")
    );
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!((saved.model.as_str(), saved.effort), ("default", None));
    harness.finish();
}

#[test]
fn a_used_session_is_resumed_and_a_lost_one_is_replaced_with_a_recap() {
    let mut harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![assistant("Use clap."), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Add a --json flag".into()));
    let state = finished_with(&harness, "Use clap.");
    let task_id = state.task.as_ref().unwrap().id.clone();
    assert_eq!(harness.stop(), Some(0));
    let stored = harness.with_store(|store| store.chat_task(&task_id).unwrap().unwrap());
    assert!(stored.session_started);
    let old_session = stored.session_id.clone();

    harness.start(
        &[
            hello(),
            vec![
                expect(json!({"type": "open_session", "resume": old_session, "session_id": null})),
                reply_error(
                    "resume_failed",
                    &format!("No conversation found with session ID: {old_session}"),
                ),
            ],
            open_ok(json!({"resume": null})),
            turn_ok(),
            vec![assistant("Back on track."), turn_finished(4, "completed")],
        ]
        .concat(),
    );
    let state = harness.wait_ready();
    let task = state.task.clone().unwrap();
    assert_eq!(task.id, task_id);
    assert_ne!(task.session_id, old_session);
    assert!(!task.session_started);
    let opened = harness.sent_of("open_session");
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1]["session_id"], task.session_id.as_str());
    let saved = harness.with_store(|store| store.chat_task(&task_id).unwrap().unwrap());
    assert_eq!(saved.session_id, task.session_id);

    harness.send(Command::Send("Where were we?".into()));
    let state = finished_with(&harness, "Back on track.");
    let text = harness.sent_of("turn")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.starts_with("[Tandem] The earlier conversation"),
        "{text}"
    );
    assert!(
        text.contains("You: Add a --json flag\nClaude: Use clap.\n"),
        "{text}"
    );
    assert!(text.ends_with("\n\nWhere were we?"), "{text}");
    assert!(state.task.as_ref().unwrap().session_started);
    harness.finish();
}

#[test]
fn a_replaced_session_still_gets_a_recap_after_a_restart() {
    let mut harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![assistant("Use clap."), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Add a --json flag".into()));
    finished_with(&harness, "Use clap.");
    assert_eq!(harness.stop(), Some(0));

    harness.start(
        &[
            hello(),
            vec![
                expect(json!({"type": "open_session", "session_id": null})),
                reply_error("resume_failed", "No conversation found"),
            ],
            open_ok(json!({"resume": null})),
        ]
        .concat(),
    );
    harness.wait_ready();
    // Closed before anything was sent to the replacement session.
    assert_eq!(harness.stop(), Some(0));

    harness.start(
        &[
            hello(),
            open_ok(json!({"resume": null})),
            turn_ok(),
            vec![assistant("Back on track."), turn_finished(3, "completed")],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Where were we?".into()));
    finished_with(&harness, "Back on track.");
    let text = harness.sent_of("turn")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.starts_with("[Tandem] The earlier conversation"),
        "{text}"
    );
    harness.finish();
}

#[test]
fn shutdown_interrupts_a_running_turn_then_closes_the_helper() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![
                expect(json!({"type": "interrupt"})),
                turn_finished(3, "interrupted"),
                expect(json!({"type": "close_session"})),
                expect(json!({"type": "shutdown"})),
                json!({"expect_eof": true}),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Start a long job".into()));
    let state = harness.wait_for("the turn to start", |state| state.turn.is_some());
    let task_id = state.task.as_ref().unwrap().id.clone();
    let store = Arc::clone(&harness.store);
    harness.finish();

    let items = store.lock().unwrap().chat_items(&task_id).unwrap();
    assert_eq!(
        items.last().unwrap().kind,
        ItemKind::Notice {
            text: "Stopped.".into()
        }
    );
}

#[test]
fn another_protocol_version_is_unavailable() {
    let harness = Harness::started(&[
        expect(json!({"type": "hello"})),
        reply_ok(json!({
            "protocol": 2, "helper": "2.0.0", "sdk": "0.4.0",
            "claude_code": "3.0.0", "node": "v24.22.1"
        })),
    ]);
    let state = harness.wait_for("the protocol refusal", |state| {
        matches!(state.connection, Connection::Unavailable { .. })
    });
    assert_eq!(
        state.connection,
        Connection::Unavailable {
            reason: "The Claude helper speaks protocol 2; this Tandem needs 1.".into(),
            hint: "Reinstall Tandem, or run npm ci in the helper folder.".into(),
        }
    );
    assert!(harness.sent_of("open_session").is_empty());
    harness.finish();
}

#[test]
fn tasks_are_created_and_selected_with_their_own_sessions() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok(),
            vec![
                assistant("First answer."),
                turn_finished(3, "completed"),
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"resume": null})),
            vec![
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"session_id": null})),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::NewTask);
    harness.send(Command::Send("First task".into()));
    let state = finished_with(&harness, "First answer.");
    assert_eq!(
        state.tasks.len(),
        1,
        "NewTask on an empty task does nothing"
    );
    assert!(harness.sent_of("close_session").is_empty());
    let first = state.task.clone().unwrap();

    harness.send(Command::NewTask);
    let state = harness.wait_for("the second task", |state| {
        state.task.as_ref().is_some_and(|task| task.id != first.id)
    });
    assert!(state.items.is_empty());
    assert_eq!(state.tasks.len(), 2);
    let second = state.task.clone().unwrap();
    let selected = harness.with_store(|store| store.selected_chat_task(&harness.key()).unwrap());
    assert_eq!(selected.as_deref(), Some(second.id.as_str()));
    let sent = harness.wait_sent("the second open_session", |sent| {
        sent.iter()
            .filter(|frame| frame["type"] == "open_session")
            .count()
            == 2
    });
    let opened: Vec<&Value> = sent
        .iter()
        .filter(|frame| frame["type"] == "open_session")
        .collect();
    assert_eq!(opened[1]["session_id"], second.session_id.as_str());

    harness.send(Command::SelectTask(first.id.clone()));
    let state = harness.wait_for("the first task again", |state| {
        state.task.as_ref().is_some_and(|task| task.id == first.id)
    });
    assert_eq!(state.items.len(), 2);
    let sent = harness.wait_sent("the third open_session", |sent| {
        sent.iter()
            .filter(|frame| frame["type"] == "open_session")
            .count()
            == 3
    });
    let reopened = sent
        .iter()
        .rfind(|frame| frame["type"] == "open_session")
        .unwrap();
    assert_eq!(reopened["resume"], first.session_id.as_str());
    let state = harness.wait_ready();
    assert_eq!(state.banner, None);
    harness.finish();
}

#[test]
fn use_my_setup_is_saved_and_reopens_the_session() {
    let harness = Harness::started(
        &[
            preamble(),
            vec![
                expect(json!({"type": "close_session"})),
                reply_ok(json!({})),
            ],
            open_ok(json!({"use_my_setup": false, "resume": null})),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::SetUseMySetup(false));
    harness.wait_sent("the reopened session", |sent| {
        sent.iter()
            .filter(|frame| frame["type"] == "open_session")
            .count()
            == 2
    });
    assert!(!harness.with_store(|store| store.chat_prefs().unwrap().use_my_setup));
    assert!(!harness.state().prefs.use_my_setup);
    harness.finish();
}

#[test]
fn a_helper_crash_fails_the_turn_and_reconnect_starts_again() {
    let mut harness = Harness::started(&[preamble(), turn_ok(), vec![json!({"exit": 1})]].concat());
    harness.wait_ready();
    harness.send(Command::Send("hi".into()));
    let state = harness.wait_for("the failure", |state| {
        matches!(state.connection, Connection::Failed { .. })
    });
    let Connection::Failed { message } = &state.connection else {
        unreachable!()
    };
    assert!(
        message.starts_with("Claude stopped unexpectedly."),
        "{message}"
    );
    assert_eq!(state.turn, None);
    assert_eq!(
        state.banner.as_deref(),
        Some("Claude could not finish this turn.")
    );

    harness.send(Command::Send("again".into()));
    let state = harness.wait_for("the not-connected banner", |state| {
        state.banner.as_deref() == Some("Claude is not connected.")
    });
    assert_eq!(state.turn, None);

    harness.send(Command::Reconnect);
    harness.wait_ready();
    // The script waits for a turn again, so it does not end cleanly.
    harness.stop();
}

#[test]
fn read_editor_buffer_prefers_the_editor_then_the_disk() {
    let mut harness = Harness::new();
    std::fs::write(harness.project.join("src/b.rs"), "fn b() {}\n").unwrap();
    {
        let mut buffers = harness.buffers.lock().unwrap();
        buffers.insert(
            "src/a.rs".into(),
            BufferText {
                text: "fn a() { todo!() }\n".into(),
                dirty: true,
            },
        );
        buffers.insert(
            "src/c.rs".into(),
            BufferText {
                text: "fn c() {}\n".into(),
                dirty: false,
            },
        );
    }
    let read = |id: &str, path: &str| tool_request(id, "read_editor_buffer", json!({"path": path}));
    harness.start(
        &[
            preamble(),
            turn_ok(),
            vec![
                read("r1", "src/a.rs"),
                expect(json!({"type": "response", "id": "r1", "ok": true})),
                read("r2", "src/c.rs"),
                expect(json!({"type": "response", "id": "r2", "ok": true})),
                read("r3", "src/b.rs"),
                expect(json!({"type": "response", "id": "r3", "ok": true})),
                read("r4", "../x"),
                expect(json!({"type": "response", "id": "r4", "ok": false})),
                read("r5", "src/missing.rs"),
                expect(json!({"type": "response", "id": "r5", "ok": false})),
                assistant("Read them."),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Look at my code".into()));
    finished_with(&harness, "Read them.");

    assert_eq!(
        response(&harness, "r1")["text"],
        "Unsaved editor contents of src/a.rs:\nfn a() { todo!() }\n"
    );
    assert_eq!(
        response(&harness, "r2")["text"],
        "Editor contents of src/c.rs (saved):\nfn c() {}\n"
    );
    assert_eq!(
        response(&harness, "r3")["text"],
        "Saved contents of src/b.rs:\nfn b() {}\n"
    );
    assert_eq!(
        response(&harness, "r4")["text"],
        "Give a path inside the project."
    );
    let missing = response(&harness, "r5")["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        missing.starts_with("Cannot read src/missing.rs: "),
        "{missing}"
    );
    harness.finish();
}

#[test]
fn offline_never_starts_the_helper() {
    let mut harness = Harness::new();
    let missing = Launch::custom(PathBuf::from("/nonexistent/tandem-claude-helper"), vec![]);
    harness.start_with(Some(missing), true);
    let state = harness.wait_for("the offline notice", |state| {
        matches!(state.connection, Connection::Unavailable { .. })
    });
    assert_eq!(
        state.connection,
        Connection::Unavailable {
            reason: "Claude is switched off for this run.".into(),
            hint: "Unset TANDEM_CLAUDE_OFFLINE and restart Tandem.".into(),
        }
    );
    assert!(state.task.is_some(), "the task still loads offline");

    harness.send(Command::Reconnect);
    harness.send(Command::Send("hi".into()));
    let state = harness.wait_for("the banner", |state| state.banner.is_some());
    assert!(matches!(state.connection, Connection::Unavailable { .. }));
    harness.stop();
}
