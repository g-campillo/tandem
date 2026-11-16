//! The guided build against the scripted fake helper: step guidance at each help level,
//! "Check my work" and the final check. No Node, no Claude.

mod support;

use serde_json::{Value, json};
use std::sync::Arc;
use support::{
    Harness, event, expect, preamble, reply_ok, tool_request, turn_finished, turn_ok_with,
};
use tandem_claude::view::{final_card, step_card};
use tandem_claude::{BufferText, ChatState, Command, TurnKind};
use tandem_core::chat::{
    BreakdownItem, ChatTask, CheckResult, EditAction, EditLocation, Finding, Guidance, HelpLevel,
    ItemKind, Mode, Phase, Placement, Plan, PlanStep, StepKind,
};

const GUIDANCE_SHOWN: &str = "Guidance is on screen. Reply with one short sentence.";
const VERDICT_SHOWN: &str = "The verdict is on screen. Finish with one short sentence.";
const NOT_GUIDED: &str = "Step guidance can only be shown during a guided build.";
const NO_CHECK: &str = "No check is running.";
const ALREADY_REPORTED: &str = "A verdict was already reported for this turn.";
const BUILDING: &str = "You are building this. Claude guides each step.";
const NO_GUIDANCE: &str =
    "Claude did not show guidance for this step. Use \"Guide me through this step\" to try again.";
const NO_VERDICT: &str = "Claude did not give a verdict. Use \"Check my work\" again.";
const TASK_COMPLETE: &str = "Task complete.";
/// `src/cli.rs` before step 1, 7 lines: line 4 is `pub struct Args {`, line 6
/// `    pub name: String,`.
const CLI: &str = "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    /// The name to greet.\n    pub name: String,\n}\n";

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

fn set_level(harness: &Harness, language: &str, level: HelpLevel) {
    harness.with_store(|store| {
        let mut prefs = store.chat_prefs().unwrap();
        prefs.help_levels.insert(language.into(), level);
        store.save_chat_prefs(&prefs).unwrap();
    });
}

/// Valid beginner guidance for step 1.
fn beginner_input() -> Value {
    json!({
        "step": 1,
        "summary": "Add a `json` flag field to the Args struct.",
        "why": "clap turns each field of the struct into a command-line flag.",
        "code": "    #[arg(long)]\n    json: bool,",
        "location": {"action": "insert", "line": 4, "anchor": "pub struct Args {"},
        "breakdown": [
            {"part": "#[arg(long)]", "explanation": "Makes the field a --json flag."},
            {"part": "bool", "explanation": "A flag is either set or not."}
        ]
    })
}

fn beginner_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Beginner,
        summary: "Add a `json` flag field to the Args struct.".into(),
        why: "clap turns each field of the struct into a command-line flag.".into(),
        code: Some("    #[arg(long)]\n    json: bool,".into()),
        pseudocode: None,
        command: None,
        breakdown: vec![
            BreakdownItem {
                part: "#[arg(long)]".into(),
                explanation: "Makes the field a --json flag.".into(),
            },
            BreakdownItem {
                part: "bool".into(),
                explanation: "A flag is either set or not.".into(),
            },
        ],
        path: Some("src/cli.rs".into()),
        anchor: None,
        where_: None,
        location: Some(EditLocation {
            action: EditAction::Insert,
            line: 4,
            end_line: None,
            anchor: "pub struct Args {".into(),
            placement: Placement::After,
        }),
    }
}

/// Valid intermediate guidance for step 1.
fn intermediate_input() -> Value {
    json!({
        "step": 1,
        "summary": "Add a boolean flag to Args.",
        "why": "clap derives flags from the struct's fields.",
        "where": "in src/cli.rs, inside struct Args, after the last field",
        "pseudocode": "in Args: add a long flag named json of type bool",
        "breakdown": [{"part": "#[arg(long)]", "explanation": "An attribute, new in Rust."}]
    })
}

fn show_step(id: &str, input: Value) -> Value {
    tool_request(id, "show_step", input)
}

fn report_check(id: &str, input: Value) -> Value {
    tool_request(id, "report_check", input)
}

fn answered(id: &str, ok: bool) -> Value {
    expect(json!({"type": "response", "id": id, "ok": ok}))
}

fn assistant(text: &str) -> Value {
    event(json!({"event": "assistant_text", "text": text}))
}

/// What Tandem writes when it shuts down an idle helper, and nothing after it.
fn closing() -> Vec<Value> {
    vec![
        expect(json!({"type": "close_session"})),
        expect(json!({"type": "shutdown"})),
        json!({"expect_eof": true}),
    ]
}

/// The `response` frame Tandem sent for helper request `id`.
fn response(harness: &Harness, id: &str) -> Value {
    harness
        .sent_of("response")
        .into_iter()
        .find(|frame| frame["id"] == id)
        .unwrap_or_else(|| panic!("no response to {id}"))
}

fn response_text(harness: &Harness, id: &str) -> String {
    response(harness, id)["text"]
        .as_str()
        .expect("a tool answer")
        .to_owned()
}

fn turn_texts(harness: &Harness) -> Vec<String> {
    harness
        .sent_of("turn")
        .iter()
        .map(|frame| frame["text"].as_str().expect("turn text").to_owned())
        .collect()
}

fn has_assistant(state: &ChatState, text: &str) -> bool {
    state
        .items
        .iter()
        .any(|item| item.kind == ItemKind::Assistant { text: text.into() })
}

fn notices(state: &ChatState) -> Vec<&str> {
    state
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Notice { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn checks(state: &ChatState) -> Vec<&CheckResult> {
    state
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Check { result } => Some(result),
            _ => None,
        })
        .collect()
}

/// Waits until the assistant said `text` and no turn runs.
fn finished_with(harness: &Harness, text: &str) -> Arc<ChatState> {
    harness.wait_for(&format!("a finished turn ending with {text:?}"), |state| {
        state.turn.is_none() && has_assistant(state, text)
    })
}

/// Waits until the assistant said `text`, no turn runs and the last item is `notice`.
fn finished_with_notice(harness: &Harness, text: &str, notice: &str) -> Arc<ChatState> {
    harness.wait_for(&format!("{text:?} followed by {notice:?}"), |state| {
        state.turn.is_none()
            && has_assistant(state, text)
            && state.items.last().map(|item| &item.kind)
                == Some(&ItemKind::Notice {
                    text: notice.into(),
                })
    })
}

fn current_step(state: &ChatState) -> usize {
    state.task.as_ref().expect("a task").current_step
}

#[test]
fn choosing_guided_starts_the_build_with_guidance_for_step_one() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                // Long enough for the test to see the running turn.
                json!({"sleep_ms": 500}),
                show_step("r1", beginner_input()),
                answered("r1", true),
                assistant("Step 1 is on screen."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);

    let preparing = harness.wait_for("the guidance turn", |state| state.turn.is_some());
    assert_eq!(
        preparing.turn,
        Some(TurnKind::Guidance {
            step: 0,
            level: HelpLevel::Beginner
        })
    );
    let state = finished_with(&harness, "Step 1 is on screen.");
    let building = state.task.clone().unwrap();
    assert_eq!(
        (building.phase, building.mode, building.current_step),
        (Phase::Build, Mode::Guided, 0)
    );
    assert!(notices(&state).contains(&BUILDING), "{:#?}", state.items);

    let text = &turn_texts(&harness)[0];
    for part in ["Step 1 of 3", "Edit src/cli.rs", "Help level: beginner"] {
        assert!(text.contains(part), "{part:?} is missing from {text}");
    }
    assert_eq!(response_text(&harness, "r1"), GUIDANCE_SHOWN);
    assert_eq!(state.guidance, Some(beginner_guidance()));
    assert_eq!(
        step_card(&state).unwrap().guidance,
        Some(beginner_guidance())
    );
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, Some(beginner_guidance()));
    let saved = harness.with_store(|store| store.chat_task(&task.id).unwrap().unwrap());
    assert_eq!((saved.phase, saved.mode), (Phase::Build, Mode::Guided));
    harness.finish();
}

#[test]
fn beginner_guidance_without_code_or_why_or_for_another_step_is_refused() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Plan, 0);
    let mut no_code = beginner_input();
    no_code.as_object_mut().unwrap().remove("code");
    let mut no_why = beginner_input();
    no_why["why"] = json!("  ");
    let mut step_two = beginner_input();
    step_two["step"] = json!(2);
    let mut no_summary = beginner_input();
    no_summary.as_object_mut().unwrap().remove("summary");
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", no_code),
                answered("r1", false),
                show_step("r2", no_why),
                answered("r2", false),
                show_step("r3", step_two),
                answered("r3", false),
                show_step("r4", no_summary),
                answered("r4", false),
                assistant("I could not show it."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    let state = finished_with_notice(&harness, "I could not show it.", NO_GUIDANCE);

    let no_code = response_text(&harness, "r1");
    assert!(no_code.contains("complete code"), "{no_code}");
    let no_why = response_text(&harness, "r2");
    assert!(no_why.contains("why"), "{no_why}");
    assert_eq!(
        response_text(&harness, "r3"),
        "Step 1 is active. Show guidance for that step only."
    );
    let no_summary = response_text(&harness, "r4");
    assert!(
        no_summary.starts_with("The guidance does not have the expected fields:"),
        "{no_summary}"
    );
    assert_eq!(state.guidance, None);
    let task_id = state.task.as_ref().unwrap().id.clone();
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task_id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, None);
    harness.finish();
}

#[test]
fn intermediate_guidance_with_finished_code_is_refused() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    set_level(&harness, "rust", HelpLevel::Intermediate);
    let mut with_code = intermediate_input();
    with_code["code"] = json!("json: bool,");
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", with_code),
                answered("r1", false),
                show_step("r2", intermediate_input()),
                answered("r2", true),
                assistant("Pseudocode is on screen."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    let state = finished_with(&harness, "Pseudocode is on screen.");

    assert!(turn_texts(&harness)[0].contains("Help level: intermediate"));
    let refused = response_text(&harness, "r1");
    assert!(refused.contains("pseudocode"), "{refused}");
    let guidance = state.guidance.clone().expect("guidance");
    assert_eq!(guidance.level, HelpLevel::Intermediate);
    assert_eq!(guidance.code, None);
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Intermediate)
            .unwrap()
    });
    assert_eq!(stored, Some(guidance));
    harness.finish();
}

/// Writes `src/cli.rs` of the plan's first step into the harness project.
fn write_cli(harness: &Harness) {
    std::fs::write(harness.project.join("src/cli.rs"), CLI).expect("src/cli.rs");
}

#[test]
fn a_beginner_location_is_checked_against_the_file() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    write_cli(&harness);
    let mut wrong_anchor = beginner_input();
    wrong_anchor["location"] = json!({"action": "insert", "line": 6, "anchor": "pub age: u8,"});
    let mut no_location = beginner_input();
    no_location.as_object_mut().unwrap().remove("location");
    let mut moved = beginner_input();
    moved["location"] = json!({"action": "insert", "line": 2, "anchor": "pub struct Args {"});
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", wrong_anchor),
                answered("r1", false),
                show_step("r2", no_location),
                answered("r2", false),
                show_step("r3", moved),
                answered("r3", true),
                assistant("Step 1 is on screen."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    let state = finished_with(&harness, "Step 1 is on screen.");

    let wrong_anchor = response_text(&harness, "r1");
    assert!(
        wrong_anchor.contains("location.anchor is not a line of src/cli.rs"),
        "{wrong_anchor}"
    );
    assert!(
        wrong_anchor.contains("Line 6 is `pub name: String,`."),
        "{wrong_anchor}"
    );
    let no_location = response_text(&harness, "r2");
    assert!(
        no_location.starts_with("At the beginner level, say exactly where the change goes"),
        "{no_location}"
    );
    assert_eq!(response_text(&harness, "r3"), GUIDANCE_SHOWN);
    let shown = state.guidance.clone().expect("guidance");
    assert_eq!(
        shown.location.as_ref().map(|location| location.line),
        Some(4)
    );
    assert_eq!(shown, beginner_guidance());
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, Some(shown));
    harness.finish();
}

#[test]
fn a_beginner_location_is_checked_against_unsaved_changes() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    write_cli(&harness);
    harness.buffers.lock().unwrap().insert(
        "src/cli.rs".into(),
        BufferText {
            text: format!("// new first line\n{CLI}"),
            dirty: true,
        },
    );
    let mut input = beginner_input();
    // Line 7 of the buffer; on disk that text is line 6.
    input["location"] = json!({"action": "insert", "line": 7, "anchor": "pub name: String,"});
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", input),
                answered("r1", true),
                assistant("Step 1 is on screen."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    let state = finished_with(&harness, "Step 1 is on screen.");

    assert_eq!(response_text(&harness, "r1"), GUIDANCE_SHOWN);
    let location = state
        .guidance
        .as_ref()
        .and_then(|guidance| guidance.location.clone())
        .expect("a location");
    assert_eq!(location.line, 7);
    assert_eq!(location.anchor, "pub name: String,");
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, state.guidance);
    harness.finish();
}

#[test]
fn an_intermediate_edit_needs_where_and_keeps_no_location() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    set_level(&harness, "rust", HelpLevel::Intermediate);
    write_cli(&harness);
    let mut no_where = intermediate_input();
    no_where.as_object_mut().unwrap().remove("where");
    let mut with_location = intermediate_input();
    with_location["location"] =
        json!({"action": "insert", "line": 4, "anchor": "pub struct Args {"});
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", no_where),
                answered("r1", false),
                show_step("r2", with_location),
                answered("r2", true),
                assistant("Pseudocode is on screen."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    let state = finished_with(&harness, "Pseudocode is on screen.");

    let refused = response_text(&harness, "r1");
    assert!(
        refused.starts_with("Say where the change goes in `where`, in words"),
        "{refused}"
    );
    assert_eq!(response_text(&harness, "r2"), GUIDANCE_SHOWN);
    let guidance = state.guidance.clone().expect("guidance");
    assert_eq!(guidance.level, HelpLevel::Intermediate);
    assert_eq!(
        guidance.where_.as_deref(),
        Some("in src/cli.rs, inside struct Args, after the last field")
    );
    assert_eq!(guidance.location, None);
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Intermediate)
            .unwrap()
    });
    assert_eq!(stored, Some(guidance));
    harness.finish();
}

#[test]
fn command_guidance_needs_the_command_and_its_breakdown() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Build, 1);
    let command = json!({
        "step": 2,
        "summary": "Add serde as a dependency.",
        "why": "serde turns Rust values into JSON.",
        "command": "cargo add serde",
        "breakdown": [
            {"part": "cargo", "explanation": "Rust's build tool."},
            {"part": "add", "explanation": "Adds a dependency to Cargo.toml."},
            {"part": "serde", "explanation": "The crate to add."}
        ]
    });
    let mut no_command = command.clone();
    no_command.as_object_mut().unwrap().remove("command");
    let mut no_breakdown = command.clone();
    no_breakdown.as_object_mut().unwrap().remove("breakdown");
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", no_command),
                answered("r1", false),
                show_step("r2", no_breakdown),
                answered("r2", false),
                show_step("r3", command),
                answered("r3", true),
                assistant("Run it."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::GuideStep);
    let state = finished_with(&harness, "Run it.");

    assert!(turn_texts(&harness)[0].contains("Step 2 of 3"));
    let no_command = response_text(&harness, "r1");
    assert!(no_command.contains("command"), "{no_command}");
    let no_breakdown = response_text(&harness, "r2");
    assert!(no_breakdown.contains("every token"), "{no_breakdown}");
    let guidance = state.guidance.clone().expect("guidance");
    assert_eq!(guidance.step, 1);
    assert_eq!(guidance.command.as_deref(), Some("cargo add serde"));
    assert_eq!(guidance.path, None, "a command step has no file");
    harness.finish();
}

#[test]
fn a_help_level_change_reuses_stored_guidance_or_asks_again() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Plan, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", beginner_input()),
                answered("r1", true),
                assistant("Beginner guidance."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r2", intermediate_input()),
                answered("r2", true),
                assistant("Intermediate guidance."),
                turn_finished(4, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    finished_with(&harness, "Beginner guidance.");

    harness.send(Command::SetHelpLevel {
        language: "rust".into(),
        level: HelpLevel::Intermediate,
    });
    let state = finished_with(&harness, "Intermediate guidance.");
    assert_eq!(
        state.guidance.as_ref().map(|guidance| guidance.level),
        Some(HelpLevel::Intermediate)
    );
    let texts = turn_texts(&harness);
    assert_eq!(texts.len(), 2);
    assert!(
        texts[1].contains("Help level: intermediate"),
        "{}",
        texts[1]
    );
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!(saved.help_level("rust"), HelpLevel::Intermediate);

    harness.send(Command::SetHelpLevel {
        language: "Rust".into(),
        level: HelpLevel::Beginner,
    });
    let state = harness.wait_for("the stored beginner guidance", |state| {
        state.guidance.as_ref().map(|guidance| guidance.level) == Some(HelpLevel::Beginner)
    });
    assert_eq!(state.guidance, Some(beginner_guidance()));
    assert_eq!(state.turn, None);
    assert_eq!(state.prefs.help_level("rust"), HelpLevel::Beginner);
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!(saved.help_level("rust"), HelpLevel::Beginner);

    harness.send(Command::SetHelpLevel {
        language: "python".into(),
        level: HelpLevel::Expert,
    });
    let state = harness.wait_for("the python level", |state| {
        state.prefs.help_level("python") == HelpLevel::Expert
    });
    assert_eq!(state.turn, None);
    assert_eq!(state.guidance, Some(beginner_guidance()));
    let saved = harness.with_store(|store| store.chat_prefs().unwrap());
    assert_eq!(saved.help_level("python"), HelpLevel::Expert);
    assert_eq!(turn_texts(&harness).len(), 2);
    assert!(
        harness
            .with_store(|store| store.chat_guidance(&task.id, 0, HelpLevel::Expert).unwrap())
            .is_none()
    );
    harness.finish();
}

#[test]
fn guide_step_forgets_the_stored_guidance_and_asks_again() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Build, 0);
    harness.with_store(|store| {
        store
            .save_chat_guidance(&task.id, &beginner_guidance())
            .unwrap()
    });
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![assistant("Let me think."), turn_finished(3, "completed")],
            closing(),
        ]
        .concat(),
    );
    let state = harness.wait_ready();
    assert_eq!(state.guidance, Some(beginner_guidance()));

    harness.send(Command::GuideStep);
    let state = finished_with_notice(&harness, "Let me think.", NO_GUIDANCE);
    assert_eq!(state.guidance, None);
    assert!(turn_texts(&harness)[0].contains("Step 1 of 3"));
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, None);
    harness.finish();
}

#[test]
fn a_passed_check_advances_and_guides_the_next_step() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "check"})),
            vec![
                json!({"sleep_ms": 500}),
                report_check(
                    "r1",
                    json!({"scope": "step", "step": 1, "verdict": "pass",
                        "explanation": "The flag is declared correctly."}),
                ),
                answered("r1", true),
                assistant("Step 1 is done."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step(
                    "r2",
                    json!({"step": 2, "summary": "Add serde.", "why": "It writes JSON.",
                        "command": "cargo add serde",
                        "breakdown": [{"part": "cargo add", "explanation": "Adds a crate."}]}),
                ),
                answered("r2", true),
                assistant("Step 2 is on screen."),
                turn_finished(4, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);

    let checking = harness.wait_for("the check turn", |state| state.turn.is_some());
    assert_eq!(checking.turn, Some(TurnKind::Check { step: 0 }));
    let state = finished_with(&harness, "Step 2 is on screen.");

    let texts = turn_texts(&harness);
    assert_eq!(harness.sent_of("turn")[0]["policy"], "check");
    assert!(texts[0].contains("Check my work"), "{}", texts[0]);
    assert!(texts[0].contains("step 1 of 3"), "{}", texts[0]);
    assert!(texts[1].contains("Step 2 of 3"), "{}", texts[1]);
    assert_eq!(response_text(&harness, "r1"), VERDICT_SHOWN);
    assert_eq!(
        checks(&state),
        [&CheckResult {
            step: Some(0),
            passed: true,
            explanation: "The flag is declared correctly.".into(),
            findings: Vec::new(),
        }]
    );
    assert_eq!(current_step(&state), 1);
    assert_eq!(
        state.guidance.as_ref().map(|guidance| guidance.step),
        Some(1)
    );
    let card = step_card(&state).unwrap();
    assert_eq!((card.index, card.kind), (1, StepKind::RunCommand));
    let saved = harness.with_store(|store| store.chat_task(&task.id).unwrap().unwrap());
    assert_eq!(saved.current_step, 1);
    harness.finish();
}

#[test]
fn a_check_that_needs_changes_keeps_the_step() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r1",
                    json!({"scope": "step", "verdict": "needs_changes",
                        "explanation": "The flag is missing.",
                        "findings": [{"path": "src/cli.rs", "line": 4,
                            "problem": "Args has no json field.",
                            "why": "clap only creates flags for fields.",
                            "hint": "Add a bool field."}]}),
                ),
                answered("r1", true),
                assistant("Fix the flag."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);
    let state = finished_with(&harness, "Fix the flag.");

    assert_eq!(
        checks(&state),
        [&CheckResult {
            step: Some(0),
            passed: false,
            explanation: "The flag is missing.".into(),
            findings: vec![Finding {
                path: Some("src/cli.rs".into()),
                line: Some(4),
                problem: "Args has no json field.".into(),
                why: "clap only creates flags for fields.".into(),
                hint: Some("Add a bool field.".into()),
            }],
        }]
    );
    assert_eq!(current_step(&state), 0);
    assert_eq!(step_card(&state).unwrap().index, 0);
    assert!(!notices(&state).contains(&NO_VERDICT));
    harness.finish();
}

#[test]
fn unsaved_files_are_named_in_the_check() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Build, 0);
    harness.buffers.lock().unwrap().insert(
        "src/cli.rs".into(),
        BufferText {
            text: "pub struct Args {}\n".into(),
            dirty: true,
        },
    );
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "check"})),
            vec![assistant("First."), turn_finished(3, "completed")],
            turn_ok_with(json!({"policy": "check"})),
            vec![assistant("Second."), turn_finished(4, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);
    finished_with_notice(&harness, "First.", NO_VERDICT);

    harness.buffers.lock().unwrap().clear();
    harness.send(Command::CheckMyWork);
    let state = finished_with_notice(&harness, "Second.", NO_VERDICT);

    let texts = turn_texts(&harness);
    assert!(
        texts[0].contains(
            "These files have unsaved changes in the editor: src/cli.rs. Read them with read_editor_buffer"
        ),
        "{}",
        texts[0]
    );
    assert!(!texts[1].contains("unsaved changes"), "{}", texts[1]);
    assert!(!texts[1].contains("read_editor_buffer"), "{}", texts[1]);
    assert_eq!(
        notices(&state)
            .iter()
            .filter(|text| **text == NO_VERDICT)
            .count(),
        2
    );
    harness.finish();
}

#[test]
fn the_last_step_leads_to_the_final_check() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Build, 2);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r1",
                    json!({"scope": "step", "step": 3, "verdict": "pass",
                        "explanation": "The test covers the flag."}),
                ),
                answered("r1", true),
                assistant("Step 3 is done."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r2",
                    json!({"scope": "task", "verdict": "needs_changes",
                        "explanation": "The old output changed."}),
                ),
                answered("r2", true),
                assistant("Not done yet."),
                turn_finished(4, "completed"),
            ],
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r3",
                    json!({"scope": "task", "verdict": "pass",
                        "explanation": "Every criterion is met."}),
                ),
                answered("r3", true),
                assistant("Done."),
                turn_finished(5, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);
    let state = finished_with(&harness, "Not done yet.");

    let texts = turn_texts(&harness);
    assert_eq!(texts.len(), 2);
    assert!(
        texts[1].contains("All 3 steps are finished"),
        "{}",
        texts[1]
    );
    assert!(texts[1].contains("1. `--json` prints valid JSON\n2. The old output is unchanged\n"));
    let task_now = state.task.clone().unwrap();
    assert_eq!((task_now.phase, task_now.current_step), (Phase::Build, 3));
    assert!(final_card(&state));
    assert_eq!(step_card(&state), None);
    let results = checks(&state);
    assert_eq!(results.len(), 2);
    assert_eq!((results[0].step, results[0].passed), (Some(2), true));
    assert_eq!((results[1].step, results[1].passed), (None, false));

    harness.send(Command::CheckMyWork);
    let state = harness.wait_for("the finished task", |state| {
        state.turn.is_none() && notices(state).contains(&TASK_COMPLETE)
    });
    assert!(has_assistant(&state, "Done."));
    assert!(turn_texts(&harness)[2].contains("All 3 steps are finished"));
    assert_eq!(state.task.as_ref().unwrap().phase, Phase::Done);
    assert!(!final_card(&state));
    let last = *checks(&state).last().unwrap();
    assert_eq!(
        last,
        &CheckResult {
            step: None,
            passed: true,
            explanation: "Every criterion is met.".into(),
            findings: Vec::new(),
        }
    );
    let saved = harness.with_store(|store| store.chat_task(&task.id).unwrap().unwrap());
    assert_eq!(saved.phase, Phase::Done);
    harness.finish();
}

#[test]
fn stop_after_a_passed_check_keeps_the_step_and_starts_nothing() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r1",
                    json!({"scope": "step", "step": 1, "verdict": "pass",
                        "explanation": "Right."}),
                ),
                answered("r1", true),
                expect(json!({"type": "interrupt"})),
                reply_ok(json!({})),
                turn_finished(3, "interrupted"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::CheckMyWork);
    harness.wait_for("the passed step", |state| {
        state
            .task
            .as_ref()
            .is_some_and(|task| task.current_step == 1)
    });
    harness.send(Command::Stop);
    let state = harness.wait_for("the stopped turn", |state| {
        state.turn.is_none() && notices(state).contains(&"Stopped.")
    });

    assert_eq!(current_step(&state), 1);
    assert_eq!(state.guidance, None);
    let card = step_card(&state).unwrap();
    assert_eq!(card.index, 1);
    assert_eq!(card.guidance, None);
    assert!(!card.preparing);
    harness.finish();
}

#[test]
fn tools_outside_a_guided_build_are_refused() {
    let harness = Harness::started(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "discuss"})),
            vec![
                show_step("r1", beginner_input()),
                answered("r1", false),
                report_check(
                    "r2",
                    json!({"scope": "step", "step": 1, "verdict": "pass", "explanation": "ok"}),
                ),
                answered("r2", false),
                report_check(
                    "r3",
                    json!({"scope": "task", "verdict": "pass", "explanation": "ok"}),
                ),
                answered("r3", false),
                assistant("Nothing to show."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Show me step 1".into()));
    let state = finished_with(&harness, "Nothing to show.");

    assert_eq!(response_text(&harness, "r1"), NOT_GUIDED);
    assert_eq!(response_text(&harness, "r2"), NO_CHECK);
    assert_eq!(response_text(&harness, "r3"), NO_CHECK);
    assert_eq!(state.guidance, None);
    assert!(checks(&state).is_empty());
    assert_eq!(state.task.as_ref().unwrap().phase, Phase::Discuss);
    harness.finish();
}

#[test]
fn a_verdict_must_match_the_running_check_and_comes_once() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Build, 0);
    let pass =
        |step: u64| json!({"scope": "step", "step": step, "verdict": "pass", "explanation": "ok"});
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                report_check("r1", pass(1)),
                answered("r1", false),
                assistant("Just chatting."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "check"})),
            vec![
                report_check(
                    "r2",
                    json!({"scope": "task", "verdict": "pass", "explanation": "ok"}),
                ),
                answered("r2", false),
                report_check("r3", pass(2)),
                answered("r3", false),
                report_check(
                    "r4",
                    json!({"scope": "step", "step": 1, "verdict": "needs_changes",
                        "explanation": "Not yet."}),
                ),
                answered("r4", true),
                report_check("r5", pass(1)),
                answered("r5", false),
                assistant("Checked."),
                turn_finished(4, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Is this right?".into()));
    finished_with(&harness, "Just chatting.");
    harness.send(Command::CheckMyWork);
    let state = finished_with(&harness, "Checked.");

    assert_eq!(response_text(&harness, "r1"), NO_CHECK);
    assert_eq!(response_text(&harness, "r2"), NO_CHECK);
    assert_eq!(
        response_text(&harness, "r3"),
        "This check is for step 1. Report the verdict for that step only."
    );
    assert_eq!(response_text(&harness, "r5"), ALREADY_REPORTED);
    let results = checks(&state);
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert_eq!(current_step(&state), 0);
    harness.finish();
}

#[test]
fn chat_during_the_build_names_the_active_step() {
    let mut harness = Harness::new();
    let task = store_task(&harness, Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", beginner_input()),
                answered("r1", true),
                assistant("A bool is a switch."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided", "text": "/help"})),
            vec![assistant("Help."), turn_finished(4, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("why a bool?".into()));
    let state = finished_with(&harness, "A bool is a switch.");

    let text = &turn_texts(&harness)[0];
    assert!(
        text.starts_with("[Tandem] Guided build. Step 1 of 3"),
        "{text}"
    );
    assert!(text.ends_with("\n\nwhy a bool?"), "{text}");
    assert_eq!(state.guidance, Some(beginner_guidance()));
    let stored = harness.with_store(|store| {
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap()
    });
    assert_eq!(stored, Some(beginner_guidance()));

    harness.send(Command::Send("/help".into()));
    finished_with(&harness, "Help.");
    assert_eq!(turn_texts(&harness)[1], "/help");
    harness.finish();
}

#[test]
fn a_restart_shows_the_same_step_and_guidance_without_a_turn() {
    let mut harness = Harness::new();
    store_task(&harness, Phase::Plan, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                show_step("r1", beginner_input()),
                answered("r1", true),
                assistant("On screen."),
                turn_finished(3, "completed"),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::ChooseGuided);
    finished_with(&harness, "On screen.");
    assert_eq!(harness.stop(), Some(0));

    harness.start(&[preamble(), closing()].concat());
    let state = harness.wait_ready();
    assert_eq!(state.guidance, Some(beginner_guidance()));
    let card = step_card(&state).unwrap();
    assert_eq!(card.index, 0);
    assert_eq!(card.guidance, Some(beginner_guidance()));
    assert!(card.can_act);
    assert!(harness.sent_of("turn").is_empty());
    harness.finish();
}
