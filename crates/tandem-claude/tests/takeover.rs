//! Takeover against the scripted fake helper: the snapshot taken before Claude works,
//! takeover turns for one step or the rest, the changed files with Keep and Undo, and
//! recovery after Tandem closed during a takeover. The fake's `write_file` and
//! `delete_file` directives play Claude's edits and shell commands. No Node, no Claude.

mod support;

use serde_json::{Value, json};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use support::{
    Harness, event, expect, preamble, reply_ok, tool_request, turn_finished, turn_ok_with,
};
use tandem_claude::view::{changes_card, takeover_running};
use tandem_claude::{BufferText, ChatState, Command, Connection, TakeoverRequest, TurnKind};
use tandem_core::chat::{
    ChangeKind, ChangeSet, ChangeSetStatus, ChatTask, CheckResult, Decision, ItemKind, Mode, Phase,
    Plan, PlanStep, StepKind, TakeoverScope,
};

const CLI: &str = "pub struct Args {}\n";
const MAIN: &str = "fn main() {}\n";
const CLAUDE_CLI: &str = "pub struct Args {\n    pub json: bool,\n}\n";
const STEP_RECORDED: &str = "Recorded. Stop here; the user continues with the next step.";
const NO_TAKEOVER: &str = "No takeover is running.";
const NO_CHECK: &str = "No check is running.";
const VERDICT_SHOWN: &str = "The verdict is on screen. Finish with one short sentence.";
const EARLIER_KEPT: &str = "Earlier changes from Claude were kept.";
const CLOSED_DURING: &str = "Tandem closed during a takeover. Review what Claude changed.";
const ROLLED_BACK: &str = "Claude's changes were undone. The plan is back at step 1.";
const TASK_COMPLETE: &str = "Task complete.";

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

/// A project with `src/cli.rs`, `src/main.rs` and a `.gitignore` that ignores `build/`,
/// and a stored task with the three-step plan in `phase` at step index `current_step`.
fn setup(phase: Phase, current_step: usize) -> (Harness, ChatTask) {
    let harness = Harness::new();
    write(&harness, "src/cli.rs", CLI);
    write(&harness, "src/main.rs", MAIN);
    write(&harness, ".gitignore", "build/\n");
    let key = harness.key();
    let task = harness.with_store(|store| {
        let mut task = store.create_chat_task(&key).unwrap();
        task.title = "Print results as JSON".into();
        task.phase = phase;
        task.current_step = current_step;
        task.plan = Some(plan());
        store.save_chat_task(&task).unwrap();
        task
    });
    (harness, task)
}

fn absolute(harness: &Harness, relative: &str) -> String {
    harness.project.join(relative).to_str().unwrap().to_owned()
}

fn write(harness: &Harness, relative: &str, text: &str) {
    let path = harness.project.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(harness: &Harness, relative: &str) -> Option<String> {
    std::fs::read_to_string(harness.project.join(relative)).ok()
}

/// Claude (or a command it runs) writes a project file.
fn claude_writes(harness: &Harness, relative: &str, text: &str) -> Value {
    json!({"write_file": {"path": absolute(harness, relative), "text": text}})
}

fn claude_deletes(harness: &Harness, relative: &str) -> Value {
    json!({"delete_file": absolute(harness, relative)})
}

fn step_done(id: &str, step: u64, summary: &str) -> Value {
    tool_request(id, "step_done", json!({"step": step, "summary": summary}))
}

fn report_check(id: &str, input: Value) -> Value {
    tool_request(id, "report_check", input)
}

fn task_pass(id: &str) -> Value {
    report_check(
        id,
        json!({"scope": "task", "verdict": "pass", "explanation": "Every criterion is met."}),
    )
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

/// The change set ids of the `Changes` items, in transcript order.
fn changes_items(state: &ChatState) -> Vec<&str> {
    state
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Changes { change_set } => Some(change_set.as_str()),
            _ => None,
        })
        .collect()
}

fn set<'a>(state: &'a ChatState, id: &str) -> &'a ChangeSet {
    state
        .change_sets
        .iter()
        .find(|set| set.id == id)
        .unwrap_or_else(|| panic!("no change set {id}: {:#?}", state.change_sets))
}

fn files(set: &ChangeSet) -> Vec<(&str, ChangeKind, Decision)> {
    set.files
        .iter()
        .map(|file| (file.path.as_str(), file.kind, file.decision))
        .collect()
}

fn decision(state: &ChatState, id: &str, path: &str) -> Option<Decision> {
    state
        .change_sets
        .iter()
        .find(|set| set.id == id)?
        .files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.decision)
}

fn task(state: &ChatState) -> &ChatTask {
    state.task.as_ref().expect("a task")
}

fn take_over(harness: &Harness, request: TakeoverRequest) {
    harness.send(Command::Takeover {
        request,
        saved: Vec::new(),
    });
}

/// Waits until a takeover turn runs and returns its change set id.
fn running_takeover(harness: &Harness) -> (Arc<ChatState>, TakeoverScope, String) {
    let state = harness.wait_for("a takeover turn", |state| {
        matches!(state.turn, Some(TurnKind::Takeover { .. }))
    });
    let Some(TurnKind::Takeover { scope, change_set }) = state.turn.clone() else {
        unreachable!()
    };
    (state, scope, change_set)
}

/// Waits until the assistant said `text` and no turn runs.
fn finished_with(harness: &Harness, text: &str) -> Arc<ChatState> {
    harness.wait_for(&format!("a finished turn ending with {text:?}"), |state| {
        state.turn.is_none() && has_assistant(state, text)
    })
}

/// Polls the project until `relative` holds `text`, for up to 5 s.
fn wait_for_file(harness: &Harness, relative: &str, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while read(harness, relative).as_deref() != Some(text) {
        assert!(
            Instant::now() < deadline,
            "{relative} never became {text:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_step_takeover_records_claudes_changes_and_guides_the_next_step() {
    let (mut harness, stored) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                // Long enough for the test to see the running turn.
                json!({"sleep_ms": 300}),
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                claude_writes(&harness, "src/json.rs", "pub fn print() {}\n"),
                claude_deletes(&harness, "src/main.rs"),
                claude_writes(&harness, "build/out.txt", "built\n"),
                step_done("r1", 1, "Added a json field, because clap makes it a flag."),
                answered("r1", true),
                assistant("Step 1 is done."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided"})),
            vec![assistant("Step 2 next."), turn_finished(4, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);

    let (running, scope, id) = running_takeover(&harness);
    assert_eq!(scope, TakeoverScope::Step(0));
    assert!(takeover_running(&running));
    assert!(!running.preparing_takeover);
    assert!(
        harness.snapshot_dir(&id).join("manifest.json").is_file(),
        "the snapshot is taken before the turn"
    );
    assert_eq!(set(&running, &id).status, ChangeSetStatus::Open);

    let state = finished_with(&harness, "Step 2 next.");
    let texts = turn_texts(&harness);
    assert!(texts[0].contains("step 1 of 3 only"), "{}", texts[0]);
    assert!(texts[1].contains("Step 2 of 3"), "{}", texts[1]);
    assert_eq!(harness.sent_of("turn")[1]["policy"], "guided");
    assert_eq!(response_text(&harness, "r1"), STEP_RECORDED);

    let changes = set(&state, &id);
    assert_eq!(changes.status, ChangeSetStatus::Review);
    assert_eq!(changes.scope, TakeoverScope::Step(0));
    assert_eq!(
        files(changes),
        [
            ("src/cli.rs", ChangeKind::Modified, Decision::Undecided),
            ("src/json.rs", ChangeKind::Created, Decision::Undecided),
            ("src/main.rs", ChangeKind::Deleted, Decision::Undecided),
        ]
    );
    assert_eq!(changes_items(&state), [id.as_str()]);
    let card = changes_card(&state, &id).expect("a changes card");
    assert_eq!(
        (card.rows.len(), card.undecided, card.can_act),
        (3, 3, true)
    );
    let building = task(&state);
    assert_eq!(
        (building.phase, building.mode, building.current_step),
        (Phase::Build, Mode::Guided, 1)
    );
    assert!(
        notices(&state).contains(&"Step 1 done: Added a json field, because clap makes it a flag."),
        "{:#?}",
        state.items
    );
    assert!(harness.snapshot_dir(&id).is_dir(), "kept for Undo");

    let saved_sets = harness.with_store(|store| store.change_sets(&stored.id).unwrap());
    assert_eq!(saved_sets, state.change_sets);
    let saved = harness.with_store(|store| store.chat_task(&stored.id).unwrap().unwrap());
    assert_eq!((saved.current_step, saved.mode), (1, Mode::Guided));
    harness.finish();
}

#[test]
fn keep_and_undo_decide_each_file() {
    let (mut harness, stored) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                claude_writes(&harness, "src/json.rs", "pub fn print() {}\n"),
                claude_deletes(&harness, "src/main.rs"),
                claude_writes(&harness, "notes.txt", "Claude's notes\n"),
                assistant("Changed four files."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Changed four files.");
    let id = state.change_sets[0].id.clone();
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Review);
    assert_eq!(set(&state, &id).files.len(), 4);

    let undo = |path: &str| {
        harness.send(Command::Undo {
            change_set: id.clone(),
            path: Some(path.into()),
        });
        harness.wait_for(&format!("{path} undone"), |state| {
            decision(state, &id, path) == Some(Decision::Undone)
        })
    };
    undo("src/cli.rs");
    assert_eq!(read(&harness, "src/cli.rs").as_deref(), Some(CLI));
    undo("src/json.rs");
    assert_eq!(read(&harness, "src/json.rs"), None);
    let state = undo("src/main.rs");
    assert_eq!(read(&harness, "src/main.rs").as_deref(), Some(MAIN));
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Review);
    assert_eq!(changes_card(&state, &id).unwrap().undecided, 1);
    assert!(harness.snapshot_dir(&id).is_dir());

    harness.send(Command::Keep {
        change_set: id.clone(),
        path: Some("notes.txt".into()),
    });
    let state = harness.wait_for("the resolved set", |state| {
        set(state, &id).status == ChangeSetStatus::Resolved
    });
    assert_eq!(
        files(set(&state, &id)),
        [
            ("notes.txt", ChangeKind::Created, Decision::Kept),
            ("src/cli.rs", ChangeKind::Modified, Decision::Undone),
            ("src/json.rs", ChangeKind::Created, Decision::Undone),
            ("src/main.rs", ChangeKind::Deleted, Decision::Undone),
        ]
    );
    assert_eq!(
        read(&harness, "notes.txt").as_deref(),
        Some("Claude's notes\n")
    );
    assert!(
        !harness.snapshot_dir(&id).exists(),
        "the snapshot is discarded"
    );
    assert_eq!(changes_card(&state, &id).unwrap().undecided, 0);
    assert!(!notices(&state).contains(&ROLLED_BACK));
    assert_eq!(task(&state).current_step, 0);
    let saved_sets = harness.with_store(|store| store.change_sets(&stored.id).unwrap());
    assert_eq!(saved_sets, state.change_sets);
    harness.finish();
}

#[test]
fn keep_all_and_undo_all_act_on_every_undecided_file() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                claude_writes(&harness, "src/json.rs", "pub fn print() {}\n"),
                claude_deletes(&harness, "src/main.rs"),
                claude_writes(&harness, "notes.txt", "Claude's notes\n"),
                assistant("First."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "a.txt", "a\n"),
                claude_writes(&harness, "b.txt", "b\n"),
                assistant("Second."),
                turn_finished(4, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "First.");
    let first = state.change_sets[0].id.clone();

    harness.send(Command::Keep {
        change_set: first.clone(),
        path: Some("notes.txt".into()),
    });
    harness.wait_for("notes.txt kept", |state| {
        decision(state, &first, "notes.txt") == Some(Decision::Kept)
    });
    harness.send(Command::Undo {
        change_set: first.clone(),
        path: None,
    });
    let state = harness.wait_for("the first set resolved", |state| {
        set(state, &first).status == ChangeSetStatus::Resolved
    });
    assert_eq!(
        files(set(&state, &first)),
        [
            ("notes.txt", ChangeKind::Created, Decision::Kept),
            ("src/cli.rs", ChangeKind::Modified, Decision::Undone),
            ("src/json.rs", ChangeKind::Created, Decision::Undone),
            ("src/main.rs", ChangeKind::Deleted, Decision::Undone),
        ]
    );
    assert_eq!(read(&harness, "src/cli.rs").as_deref(), Some(CLI));
    assert_eq!(read(&harness, "src/json.rs"), None);
    assert_eq!(read(&harness, "src/main.rs").as_deref(), Some(MAIN));
    assert!(!harness.snapshot_dir(&first).exists());
    assert!(
        !notices(&state).contains(&ROLLED_BACK),
        "a kept file means the takeover was not undone"
    );

    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Second.");
    assert!(
        !notices(&state).contains(&EARLIER_KEPT),
        "nothing was left to keep"
    );
    let second = state.change_sets[1].id.clone();
    assert_eq!(set(&state, &second).status, ChangeSetStatus::Review);

    harness.send(Command::Keep {
        change_set: second.clone(),
        path: None,
    });
    let state = harness.wait_for("the second set resolved", |state| {
        set(state, &second).status == ChangeSetStatus::Resolved
    });
    assert_eq!(
        files(set(&state, &second)),
        [
            ("a.txt", ChangeKind::Created, Decision::Kept),
            ("b.txt", ChangeKind::Created, Decision::Kept),
        ]
    );
    assert_eq!(read(&harness, "a.txt").as_deref(), Some("a\n"));
    assert!(!harness.snapshot_dir(&second).exists());
    assert_eq!(changes_items(&state), [first.as_str(), second.as_str()]);
    harness.finish();
}

#[test]
fn undo_refuses_a_file_changed_again_or_unsaved_in_the_editor() {
    let (mut harness, _) = setup(Phase::Build, 0);
    let claude_main = "fn main() {\n    run();\n}\n";
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                claude_writes(&harness, "src/main.rs", claude_main),
                assistant("Changed."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Changed.");
    let id = state.change_sets[0].id.clone();

    write(&harness, "src/cli.rs", "pub struct Args { mine: u8 }\n");
    harness.send(Command::Undo {
        change_set: id.clone(),
        path: Some("src/cli.rs".into()),
    });
    let state = harness.wait_for("the refusal", |state| {
        state.banner.as_deref()
            == Some("src/cli.rs changed after Claude's edit, so it was not restored.")
    });
    assert_eq!(
        read(&harness, "src/cli.rs").as_deref(),
        Some("pub struct Args { mine: u8 }\n")
    );
    assert_eq!(
        decision(&state, &id, "src/cli.rs"),
        Some(Decision::Undecided)
    );

    harness.buffers.lock().unwrap().insert(
        "src/main.rs".into(),
        BufferText {
            text: "fn main() { /* typing */ }\n".into(),
            dirty: true,
        },
    );
    harness.send(Command::Undo {
        change_set: id.clone(),
        path: Some("src/main.rs".into()),
    });
    let state = harness.wait_for("the unsaved refusal", |state| {
        state.banner.as_deref() == Some("Save or close src/main.rs before undoing Claude's change.")
    });
    assert_eq!(read(&harness, "src/main.rs").as_deref(), Some(claude_main));
    assert_eq!(
        decision(&state, &id, "src/main.rs"),
        Some(Decision::Undecided)
    );
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Review);
    assert!(harness.snapshot_dir(&id).is_dir());
    harness.finish();
}

#[test]
fn a_rest_takeover_from_the_plan_card_builds_every_step_and_finishes() {
    let (mut harness, stored) = setup(Phase::Plan, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                json!({"sleep_ms": 300}),
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                step_done("r1", 1, "Added the flag."),
                answered("r1", true),
                step_done("r2", 2, "Added serde."),
                answered("r2", true),
                // Too early: step 3 is still open.
                task_pass("r3"),
                answered("r3", false),
                claude_writes(&harness, "tests/cli.rs", "#[test]\nfn json() {}\n"),
                step_done("r4", 3, "Added a test."),
                answered("r4", true),
                task_pass("r5"),
                answered("r5", true),
                assistant("All done."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Rest);

    let (running, scope, id) = running_takeover(&harness);
    assert_eq!(scope, TakeoverScope::Rest(0));
    let building = task(&running);
    assert_eq!(
        (building.phase, building.mode, building.current_step),
        (Phase::Build, Mode::Takeover, 0)
    );

    let state = finished_with(&harness, "All done.");
    let text = &turn_texts(&harness)[0];
    for part in [
        "all remaining steps, starting at step 1 of 3",
        "1. `--json` prints valid JSON",
        "2. The old output is unchanged",
    ] {
        assert!(text.contains(part), "{part:?} is missing from {text}");
    }
    assert_eq!(
        response_text(&harness, "r1"),
        "Recorded. Continue with step 2."
    );
    assert_eq!(
        response_text(&harness, "r2"),
        "Recorded. Continue with step 3."
    );
    assert_eq!(response_text(&harness, "r3"), NO_CHECK);
    assert_eq!(
        response_text(&harness, "r4"),
        "Recorded. Now check every \"done when\" criterion and call report_check with scope \"task\"."
    );
    assert_eq!(response_text(&harness, "r5"), VERDICT_SHOWN);

    let done = task(&state);
    assert_eq!(
        (done.phase, done.mode, done.current_step),
        (Phase::Done, Mode::Guided, 3)
    );
    let changes = set(&state, &id);
    assert_eq!(changes.status, ChangeSetStatus::Review);
    assert_eq!(
        files(changes),
        [
            ("src/cli.rs", ChangeKind::Modified, Decision::Undecided),
            ("tests/cli.rs", ChangeKind::Created, Decision::Undecided),
        ]
    );
    assert_eq!(changes_items(&state), [id.as_str()]);
    let notices = notices(&state);
    for notice in [
        "Step 1 done: Added the flag.",
        "Step 2 done: Added serde.",
        "Step 3 done: Added a test.",
        TASK_COMPLETE,
    ] {
        assert!(notices.contains(&notice), "{notice:?} in {notices:#?}");
    }
    let verdicts: Vec<&CheckResult> = state
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Check { result } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts.len(), 1);
    assert_eq!((verdicts[0].step, verdicts[0].passed), (None, true));
    let saved = harness.with_store(|store| store.chat_task(&stored.id).unwrap().unwrap());
    assert_eq!((saved.phase, saved.mode), (Phase::Done, Mode::Guided));
    harness.finish();
}

#[test]
fn step_done_must_name_the_active_step_once() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "guided"})),
            vec![
                step_done("r1", 1, "Not a takeover."),
                answered("r1", false),
                assistant("Just chatting."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                step_done("r2", 2, "Wrong step."),
                answered("r2", false),
                step_done("r3", 1, "Added the flag."),
                answered("r3", true),
                step_done("r4", 2, "Kept going."),
                answered("r4", false),
                step_done("r5", 1, "Again."),
                answered("r5", false),
                assistant("Done."),
                turn_finished(4, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided"})),
            vec![assistant("Next."), turn_finished(5, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Send("Can you do it?".into()));
    finished_with(&harness, "Just chatting.");
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Next.");

    assert_eq!(response_text(&harness, "r1"), NO_TAKEOVER);
    assert_eq!(response_text(&harness, "r2"), "Step 1 is the active step.");
    assert_eq!(response_text(&harness, "r3"), STEP_RECORDED);
    assert_eq!(response(&harness, "r4")["ok"], false);
    assert_eq!(response(&harness, "r5")["ok"], false);
    assert_eq!(task(&state).current_step, 1);
    assert_eq!(
        notices(&state)
            .iter()
            .filter(|text| text.starts_with("Step 1 done:"))
            .count(),
        1
    );
    harness.finish();
}

#[test]
fn stop_during_a_takeover_ends_it_for_review() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                expect(json!({"type": "interrupt"})),
                reply_ok(json!({})),
                turn_finished(3, "interrupted"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Rest);
    let (running, _, id) = running_takeover(&harness);
    assert_eq!(task(&running).mode, Mode::Takeover);
    wait_for_file(&harness, "src/cli.rs", CLAUDE_CLI);
    harness.send(Command::Stop);

    let state = harness.wait_for("the stopped takeover", |state| {
        state.turn.is_none() && notices(state).contains(&"Stopped.")
    });
    assert_eq!(changes_items(&state), [id.as_str()]);
    let changes = set(&state, &id);
    assert_eq!(changes.status, ChangeSetStatus::Review);
    assert_eq!(
        files(changes),
        [("src/cli.rs", ChangeKind::Modified, Decision::Undecided)]
    );
    assert_eq!(task(&state).mode, Mode::Guided);
    assert_eq!(task(&state).current_step, 0);
    assert!(!takeover_running(&state));
    harness.finish();
}

#[test]
fn unsaved_buffers_block_a_takeover() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.buffers.lock().unwrap().insert(
        "src/cli.rs".into(),
        BufferText {
            text: "pub struct Args { typing }\n".into(),
            dirty: true,
        },
    );
    harness.start(&[preamble(), closing()].concat());
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);

    let state = harness.wait_for("the banner", |state| state.banner.is_some());
    assert_eq!(
        state.banner.as_deref(),
        Some("Save or close these files first: src/cli.rs")
    );
    assert!(state.change_sets.is_empty());
    assert_eq!(task(&state).mode, Mode::Guided);
    assert!(!harness.snapshot_dir("").exists(), "no snapshot was taken");
    assert!(harness.sent_of("turn").is_empty());
    harness.finish();
}

#[test]
fn files_saved_before_the_takeover_are_named_in_a_notice() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                assistant("Nothing to change."),
                turn_finished(3, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    harness.send(Command::Takeover {
        request: TakeoverRequest::Step,
        saved: vec!["src/cli.rs".into(), "src/main.rs".into()],
    });
    let state = finished_with(&harness, "Nothing to change.");

    assert!(
        notices(&state).contains(
            &"Tandem saved src/cli.rs, src/main.rs before the takeover, because Claude edits the files on disk."
        ),
        "{:#?}",
        state.items
    );
    let id = state.change_sets[0].id.clone();
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Resolved);
    assert!(set(&state, &id).files.is_empty());
    assert!(changes_items(&state).is_empty(), "nothing changed");
    assert!(!harness.snapshot_dir(&id).exists());
    harness.finish();
}

#[test]
fn a_second_takeover_keeps_the_first_ones_undecided_files() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                assistant("First."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "takeover"})),
            vec![assistant("Second."), turn_finished(4, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "First.");
    let first = state.change_sets[0].id.clone();
    assert!(harness.snapshot_dir(&first).is_dir());

    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Second.");
    let earlier = set(&state, &first);
    assert_eq!(earlier.status, ChangeSetStatus::Resolved);
    assert_eq!(
        files(earlier),
        [("src/cli.rs", ChangeKind::Modified, Decision::Kept)]
    );
    assert!(!harness.snapshot_dir(&first).exists());
    assert_eq!(read(&harness, "src/cli.rs").as_deref(), Some(CLAUDE_CLI));
    assert_eq!(
        notices(&state)
            .iter()
            .filter(|text| **text == EARLIER_KEPT)
            .count(),
        1
    );
    assert_eq!(state.change_sets.len(), 2);
    harness.finish();
}

#[test]
fn undoing_everything_rolls_the_plan_back() {
    let (mut harness, stored) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                claude_writes(&harness, "src/json.rs", "pub fn print() {}\n"),
                step_done("r1", 1, "Added the flag."),
                answered("r1", true),
                assistant("Step 1 is done."),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "guided"})),
            vec![assistant("Step 2 next."), turn_finished(4, "completed")],
            turn_ok_with(json!({"policy": "guided"})),
            vec![assistant("Step 1 again."), turn_finished(5, "completed")],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Step);
    let state = finished_with(&harness, "Step 2 next.");
    assert_eq!(task(&state).current_step, 1);
    let id = state.change_sets[0].id.clone();

    harness.send(Command::Undo {
        change_set: id.clone(),
        path: None,
    });
    let state = finished_with(&harness, "Step 1 again.");
    assert_eq!(read(&harness, "src/cli.rs").as_deref(), Some(CLI));
    assert_eq!(read(&harness, "src/json.rs"), None);
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Resolved);
    assert!(
        set(&state, &id)
            .files
            .iter()
            .all(|file| file.decision == Decision::Undone)
    );
    let back = task(&state);
    assert_eq!(
        (back.phase, back.mode, back.current_step),
        (Phase::Build, Mode::Guided, 0)
    );
    assert!(notices(&state).contains(&ROLLED_BACK), "{:#?}", state.items);
    let texts = turn_texts(&harness);
    assert_eq!(texts.len(), 3);
    assert!(texts[2].contains("Step 1 of 3"), "{}", texts[2]);
    let saved = harness.with_store(|store| store.chat_task(&stored.id).unwrap().unwrap());
    assert_eq!(saved.current_step, 0);
    harness.finish();
}

#[test]
fn a_paused_rest_takeover_continues_after_the_users_answer() {
    let (mut harness, _) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                step_done("r1", 1, "Added the flag."),
                answered("r1", true),
                assistant("Should errors be JSON too?"),
                turn_finished(3, "completed"),
            ],
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/json.rs", "pub fn print() {}\n"),
                step_done("r2", 2, "Added serde."),
                answered("r2", true),
                claude_writes(&harness, "tests/cli.rs", "#[test]\nfn json() {}\n"),
                step_done("r3", 3, "Added a test."),
                answered("r3", true),
                task_pass("r4"),
                answered("r4", true),
                assistant("All done."),
                turn_finished(4, "completed"),
            ],
            closing(),
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Rest);
    let state = finished_with(&harness, "Should errors be JSON too?");
    let id = state.change_sets[0].id.clone();
    let paused = task(&state);
    assert_eq!(
        (paused.phase, paused.mode, paused.current_step),
        (Phase::Build, Mode::Takeover, 1)
    );
    assert_eq!(set(&state, &id).status, ChangeSetStatus::Open);
    assert_eq!(
        files(set(&state, &id)),
        [("src/cli.rs", ChangeKind::Modified, Decision::Undecided)]
    );
    assert_eq!(changes_items(&state), [id.as_str()]);
    assert!(harness.snapshot_dir(&id).is_dir());

    harness.send(Command::Send("yes, go on".into()));
    let state = finished_with(&harness, "All done.");
    let turns = harness.sent_of("turn");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1]["policy"], "takeover");
    let text = turns[1]["text"].as_str().unwrap();
    assert!(
        text.starts_with("[Tandem] Takeover is still active"),
        "{text}"
    );
    assert!(text.contains("(next: step 2 of 3)"), "{text}");
    assert!(text.ends_with("\n\nyes, go on"), "{text}");
    assert!(state.items.iter().any(|item| item.kind
        == ItemKind::User {
            text: "yes, go on".into()
        }));

    assert_eq!(state.change_sets.len(), 1, "the open set is reused");
    let changes = set(&state, &id);
    assert_eq!(changes.status, ChangeSetStatus::Review);
    assert_eq!(changes.scope, TakeoverScope::Rest(0));
    assert_eq!(
        files(changes),
        [
            ("src/cli.rs", ChangeKind::Modified, Decision::Undecided),
            ("src/json.rs", ChangeKind::Created, Decision::Undecided),
            ("tests/cli.rs", ChangeKind::Created, Decision::Undecided),
        ]
    );
    assert_eq!(changes_items(&state), [id.as_str()]);
    let done = task(&state);
    assert_eq!((done.phase, done.mode), (Phase::Done, Mode::Guided));
    harness.finish();
}

#[test]
fn a_takeover_interrupted_by_closing_tandem_is_recovered_for_review() {
    let (mut harness, stored) = setup(Phase::Build, 0);
    harness.start(
        &[
            preamble(),
            turn_ok_with(json!({"policy": "takeover"})),
            vec![
                claude_writes(&harness, "src/cli.rs", CLAUDE_CLI),
                json!({"hang": true}),
            ],
        ]
        .concat(),
    );
    harness.wait_ready();
    take_over(&harness, TakeoverRequest::Rest);
    let (_, _, id) = running_takeover(&harness);
    wait_for_file(&harness, "src/cli.rs", CLAUDE_CLI);
    // An open set whose snapshot folder is gone.
    harness.with_store(|store| {
        store
            .save_change_set(&ChangeSet {
                id: "lost".into(),
                task_id: stored.id.clone(),
                scope: TakeoverScope::Step(0),
                status: ChangeSetStatus::Open,
                files: Vec::new(),
                created_at_ms: 0,
            })
            .unwrap()
    });
    harness.abandon();

    // Claude is switched off, so the helper never starts: recovery needs none of it.
    harness.start_with(None, true);
    let state = harness.wait_for("the recovered takeover", |state| {
        state
            .change_sets
            .iter()
            .any(|set| set.id == id && set.status == ChangeSetStatus::Review)
    });
    assert!(matches!(state.connection, Connection::Unavailable { .. }));
    assert_eq!(
        files(set(&state, &id)),
        [("src/cli.rs", ChangeKind::Modified, Decision::Undecided)]
    );
    assert_eq!(set(&state, "lost").status, ChangeSetStatus::Resolved);
    assert_eq!(changes_items(&state), [id.as_str()]);
    assert_eq!(
        notices(&state)
            .iter()
            .filter(|text| **text == CLOSED_DURING)
            .count(),
        1
    );
    assert_eq!(task(&state).mode, Mode::Guided);
    let saved = harness.with_store(|store| store.chat_task(&stored.id).unwrap().unwrap());
    assert_eq!(saved.mode, Mode::Guided);
    let open = harness.with_store(|store| store.open_change_sets(&harness.key()).unwrap());
    assert!(open.is_empty(), "{open:#?}");

    // Undo works without Claude.
    assert!(changes_card(&state, &id).unwrap().can_act);
    harness.send(Command::Undo {
        change_set: id.clone(),
        path: None,
    });
    let state = harness.wait_for("the undone takeover", |state| {
        set(state, &id).status == ChangeSetStatus::Resolved
    });
    assert_eq!(read(&harness, "src/cli.rs").as_deref(), Some(CLI));
    assert!(notices(&state).contains(&ROLLED_BACK), "{:#?}", state.items);
    assert_eq!(state.turn, None);
}
