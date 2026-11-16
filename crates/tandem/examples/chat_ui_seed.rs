//! Offline seed for checking the chat panel by hand. It writes a small project and a state
//! folder with five tasks: one in Plan, whose transcript has a reply with fenced code; one
//! in a guided Build with stored guidance for its first step at all three help levels
//! (selected); one guided step whose Beginner guidance replaces a line; one guided command
//! step; and one whose first step Claude did in a takeover, with the three changed files
//! waiting for Keep or Undo. It starts no helper and uses no Claude subscription.
use anyhow::{Result, ensure};
use std::{fs, path::PathBuf};
use tandem_core::chat::{
    ActivityKind, BreakdownItem, ChangeSet, ChangeSetStatus, ChatTask, Decision, EditAction,
    EditLocation, FileChange, Guidance, HelpLevel, ItemKind, Mode, Phase, Placement, Plan,
    PlanStep, StepKind, TakeoverScope,
};
use tandem_core::takeover::Snapshot;
use tandem_core::{Project, Store, new_id, now_ms};

const CARGO_TOML: &str = "[package]\nname = \"seed-cli\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nclap = { version = \"4\", features = [\"derive\"] }\n";
const CLI_RS: &str = "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    /// The name to greet.\n    pub name: String,\n}\n";
const MAIN_RS: &str = "mod cli;\n\nuse clap::Parser;\n\nfn main() {\n    let args = cli::Args::parse();\n    println!(\"Hello, {}!\", args.name);\n}\n";
/// `src/cli.rs` after Claude's takeover of step 1: the same file with the `json` flag.
const CLI_RS_WITH_JSON: &str = "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    /// The name to greet.\n    pub name: String,\n    /// Print the result as JSON.\n    #[arg(long)]\n    pub json: bool,\n}\n";
const JSON_RS: &str = "/// The greeting as a JSON object.\npub fn greeting(name: &str) -> String {\n    format!(\"{{\\\"greeting\\\":\\\"Hello, {name}!\\\"}}\")\n}\n";

fn step(kind: StepKind, target: &str, summary: &str) -> PlanStep {
    PlanStep {
        kind,
        target: target.into(),
        summary: summary.into(),
        language: "rust".into(),
    }
}

fn part(part: &str, explanation: &str) -> BreakdownItem {
    BreakdownItem {
        part: part.into(),
        explanation: explanation.into(),
    }
}

/// Beginner guidance for the first step: add the `json` field to `Args` in src/cli.rs.
fn beginner_step_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Beginner,
        summary: "Add a `json` flag field to the Args struct.".into(),
        why: "clap turns each field of the struct into a command-line flag, so a bool field named json becomes --json.".into(),
        code: Some("    #[arg(long)]\n    json: bool,".into()),
        pseudocode: None,
        command: None,
        breakdown: vec![
            part(
                "#[arg(long)]",
                "An attribute that tells clap to accept this field as a long flag, written --json.",
            ),
            part("json", "The field's name. clap uses it as the flag's name."),
            part(
                "bool",
                "The field's type: false unless --json is given, then true.",
            ),
            part(
                ",",
                "Ends the field. Rust allows a comma after the last field of a struct too.",
            ),
        ],
        path: Some("src/cli.rs".into()),
        anchor: None,
        where_: None,
        location: Some(EditLocation {
            action: EditAction::Insert,
            line: 6,
            end_line: None,
            anchor: "pub name: String,".into(),
            placement: Placement::After,
        }),
    }
}

/// Intermediate guidance for the same step: pseudocode, and the place in words.
fn intermediate_step_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Intermediate,
        summary: "Add a boolean `json` flag to Args.".into(),
        why: "clap derives a command-line flag from each field of the struct.".into(),
        code: None,
        pseudocode: Some(
            "add a field named json of type bool\nmark it as a long flag with the arg attribute"
                .into(),
        ),
        command: None,
        breakdown: vec![
            part("arg", "The attribute clap reads to configure a field."),
            part(
                "#[arg(long)]",
                "Rust's attribute syntax: it annotates the item that follows.",
            ),
        ],
        path: Some("src/cli.rs".into()),
        anchor: None,
        where_: Some("in `src/cli.rs`, inside `struct Args`, after the `name` field".into()),
        location: None,
    }
}

/// Expert guidance for the same step: a short description and the place in words.
fn expert_step_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Expert,
        summary: "Add a `json: bool` long flag to Args.".into(),
        why: "clap derives flags from fields.".into(),
        code: None,
        pseudocode: None,
        command: None,
        breakdown: Vec::new(),
        path: Some("src/cli.rs".into()),
        anchor: None,
        where_: Some("in `src/cli.rs`, inside `struct Args`".into()),
        location: None,
    }
}

/// Beginner guidance that replaces one line: the doc comment of `name` in src/cli.rs.
fn update_line_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Beginner,
        summary: "Replace the doc comment above `name`.".into(),
        why: "clap prints a field's doc comment as that flag's help text.".into(),
        code: Some("    /// The name of the person to greet.".into()),
        pseudocode: None,
        command: None,
        breakdown: vec![part(
            "///",
            "Starts a doc comment. clap shows it as the flag's help text.",
        )],
        path: Some("src/cli.rs".into()),
        anchor: None,
        where_: None,
        location: Some(EditLocation {
            action: EditAction::Update,
            line: 5,
            end_line: None,
            anchor: "/// The name to greet.".into(),
            placement: Placement::After,
        }),
    }
}

/// Beginner guidance for a command step: run the tests.
fn command_guidance() -> Guidance {
    Guidance {
        step: 0,
        level: HelpLevel::Beginner,
        summary: "Run the project's tests.".into(),
        why: "The tests show whether the change works before you go on.".into(),
        code: None,
        pseudocode: None,
        command: Some("cargo test --quiet".into()),
        breakdown: vec![
            part("cargo", "Rust's build tool and package manager."),
            part(
                "test",
                "The subcommand that builds the project and runs its tests.",
            ),
            part(
                "--quiet",
                "Prints one line per test binary instead of one per test.",
            ),
        ],
        path: None,
        anchor: None,
        where_: None,
        location: None,
    }
}

/// Creates a guided build at the first step of `plan`, stores `guidance` after checking
/// each against its step, and starts the transcript as a guided build does.
fn seed_guided_task(
    store: &mut Store,
    project: &Project,
    title: &str,
    plan: &Plan,
    guidance: &[Guidance],
) -> Result<ChatTask> {
    let mut task = store.create_chat_task(project.key())?;
    task.title = title.into();
    task.phase = Phase::Build;
    task.mode = Mode::Guided;
    task.current_step = 0;
    task.plan = Some(plan.clone());
    store.save_chat_task(&task)?;
    for guidance in guidance {
        guidance.check(&plan.steps[guidance.step])?;
        store.save_chat_guidance(&task.id, guidance)?;
    }
    for kind in [
        ItemKind::Plan { plan: plan.clone() },
        ItemKind::Notice {
            text: "You are building this. Claude guides each step.".into(),
        },
    ] {
        store.append_chat_item(&task.id, &kind)?;
    }
    Ok(task)
}

/// Plays Claude's takeover of step 1 for real: snapshots the project into the state
/// folder, modifies `src/cli.rs`, creates `src/json.rs` and deletes `src/main.rs`, then
/// records the diff as a change set waiting for review.
fn seed_takeover(store: &mut Store, project: &Project, task_id: &str) -> Result<ChangeSet> {
    let id = new_id();
    let snapshot = Snapshot::capture(project, &store.directory.join("takeover").join(&id))?;
    fs::write(project.root.join("src/cli.rs"), CLI_RS_WITH_JSON)?;
    fs::write(project.root.join("src/json.rs"), JSON_RS)?;
    fs::remove_file(project.root.join("src/main.rs"))?;
    let files: Vec<FileChange> = snapshot
        .diff(project)?
        .into_iter()
        .map(|change| FileChange {
            path: change.path,
            kind: change.kind,
            decision: Decision::Undecided,
            after_hash: change.after_hash,
            undoable: change.undoable,
        })
        .collect();
    ensure!(
        files.len() == 3,
        "The seeded takeover should change three files, not {}.",
        files.len()
    );
    let set = ChangeSet {
        id,
        task_id: task_id.into(),
        scope: TakeoverScope::Step(0),
        status: ChangeSetStatus::Review,
        files,
        created_at_ms: now_ms(),
    };
    store.save_change_set(&set)?;
    Ok(set)
}

fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("tandem-chat-ui-{}", new_id())));
    ensure!(
        !root.exists(),
        "Refusing to overwrite {}; give a folder that does not exist yet.",
        root.display()
    );
    let project_path = root.join("project");
    let state_path = root.join("state");
    fs::create_dir_all(project_path.join("src"))?;
    fs::create_dir_all(&state_path)?;
    fs::write(project_path.join("Cargo.toml"), CARGO_TOML)?;
    fs::write(project_path.join("src/cli.rs"), CLI_RS)?;
    fs::write(project_path.join("src/main.rs"), MAIN_RS)?;

    let project = Project::open(&project_path)?;
    let mut store = Store::open(&state_path)?;
    let mut task = store.create_chat_task(project.key())?;
    let plan = Plan {
        goal: "Print results as JSON when --json is set".into(),
        done_when: vec![
            "Output parses as JSON".into(),
            "Old output is unchanged".into(),
        ],
        steps: vec![
            step(StepKind::EditFile, "src/cli.rs", "Add `json` to Args"),
            step(StepKind::EditFile, "src/main.rs", "Print JSON in run()"),
            step(StepKind::CreateFile, "tests/cli.rs", "Add a test"),
            step(StepKind::RunCommand, "cargo test", "Run the tests"),
        ],
    }
    .normalized()?;
    task.title = "Add a --json flag to the CLI".into();
    task.phase = Phase::Plan;
    task.plan = Some(plan.clone());
    store.save_chat_task(&task)?;
    for kind in [
        ItemKind::User {
            text: "Add a --json flag to the CLI".into(),
        },
        ItemKind::Activity {
            kind: ActivityKind::Read,
            label: "src/cli.rs".into(),
        },
        ItemKind::Activity {
            kind: ActivityKind::Search,
            label: "\"clap bool flag\"".into(),
        },
        ItemKind::Assistant {
            text: "Should --json also change error output?".into(),
        },
        ItemKind::User {
            text: "No, only results.".into(),
        },
        ItemKind::Assistant {
            text: "Then the flag is one field, and you can try it like this:\n\n```rust\n#[arg(long)]\njson: bool,\n```\n\n```shell\ncargo run -- World --json\n```".into(),
        },
        ItemKind::Plan { plan: plan.clone() },
        ItemKind::Assistant {
            text: "Here is the plan. Review it and pick how you want to build it.".into(),
        },
    ] {
        store.append_chat_item(&task.id, &kind)?;
    }

    let beginner = beginner_step_guidance();
    ensure!(
        beginner.located(&plan.steps[0], Some(CLI_RS))? == beginner.location,
        "The seeded location should match src/cli.rs."
    );
    let build = seed_guided_task(
        &mut store,
        &project,
        "Add a --json flag to the CLI (guided)",
        &plan,
        &[
            beginner,
            intermediate_step_guidance(),
            expert_step_guidance(),
        ],
    )?;

    let update_plan = Plan {
        goal: "Make the help text of name clearer".into(),
        done_when: vec!["--help shows the new text".into()],
        steps: vec![step(
            StepKind::EditFile,
            "src/cli.rs",
            "Reword the doc comment of `name`",
        )],
    }
    .normalized()?;
    let update = update_line_guidance();
    ensure!(
        update.located(&update_plan.steps[0], Some(CLI_RS))? == update.location,
        "The seeded location should match src/cli.rs."
    );
    seed_guided_task(
        &mut store,
        &project,
        "Reword the name help text (update a line)",
        &update_plan,
        &[update],
    )?;

    let command_plan = Plan {
        goal: "See the tests pass".into(),
        done_when: vec!["cargo test passes".into()],
        steps: vec![step(
            StepKind::RunCommand,
            "cargo test --quiet",
            "Run the tests",
        )],
    }
    .normalized()?;
    seed_guided_task(
        &mut store,
        &project,
        "Run the tests (a command step)",
        &command_plan,
        &[command_guidance()],
    )?;

    let mut takeover = store.create_chat_task(project.key())?;
    takeover.title = "Add a --json flag to the CLI (Claude did step 1)".into();
    takeover.phase = Phase::Build;
    takeover.mode = Mode::Guided;
    takeover.current_step = 1;
    takeover.plan = Some(plan.clone());
    store.save_chat_task(&takeover)?;
    let set = seed_takeover(&mut store, &project, &takeover.id)?;
    for kind in [
        ItemKind::Plan { plan },
        ItemKind::Notice {
            text: "Step 1 done: Added the json flag to Args.".into(),
        },
        ItemKind::Changes { change_set: set.id },
    ] {
        store.append_chat_item(&takeover.id, &kind)?;
    }
    // Creating a task selects it; the guided build stays the one Tandem opens with.
    store.select_chat_task(project.key(), &build.id)?;

    println!("project={}", project_path.display());
    println!("state_dir={}", state_path.display());
    println!(
        "Launch: TANDEM_CLAUDE_OFFLINE=1 target/debug/tandem {} --state-dir {}",
        project_path.display(),
        state_path.display()
    );
    Ok(())
}
