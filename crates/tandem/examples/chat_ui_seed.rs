//! Offline seed for checking the chat panel by hand. It writes a small project and a state
//! folder with two tasks: one in Plan, and one in a guided Build with stored guidance for
//! its first step (selected). It starts no helper and uses no Claude subscription.
use anyhow::{Result, ensure};
use std::{fs, path::PathBuf};
use tandem_core::chat::{
    ActivityKind, BreakdownItem, Guidance, HelpLevel, ItemKind, Mode, Phase, Plan, PlanStep,
    StepKind,
};
use tandem_core::{Project, Store, new_id};

const CARGO_TOML: &str = "[package]\nname = \"seed-cli\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nclap = { version = \"4\", features = [\"derive\"] }\n";
const CLI_RS: &str = "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    /// The name to greet.\n    pub name: String,\n}\n";
const MAIN_RS: &str = "mod cli;\n\nuse clap::Parser;\n\nfn main() {\n    let args = cli::Args::parse();\n    println!(\"Hello, {}!\", args.name);\n}\n";

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
fn first_step_guidance() -> Guidance {
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
        anchor: Some("pub struct Args {".into()),
    }
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
        ItemKind::Plan { plan: plan.clone() },
        ItemKind::Assistant {
            text: "Here is the plan. Review it and pick how you want to build it.".into(),
        },
    ] {
        store.append_chat_item(&task.id, &kind)?;
    }

    // Created last, so it is the selected task when Tandem opens the project.
    let mut build = store.create_chat_task(project.key())?;
    build.title = "Add a --json flag to the CLI (guided)".into();
    build.phase = Phase::Build;
    build.mode = Mode::Guided;
    build.current_step = 0;
    build.plan = Some(plan.clone());
    store.save_chat_task(&build)?;
    let guidance = first_step_guidance();
    guidance.check(&plan.steps[0])?;
    store.save_chat_guidance(&build.id, &guidance)?;
    for kind in [
        ItemKind::Plan { plan },
        ItemKind::Notice {
            text: "You are building this. Claude guides each step.".into(),
        },
    ] {
        store.append_chat_item(&build.id, &kind)?;
    }

    println!("project={}", project_path.display());
    println!("state_dir={}", state_path.display());
    println!(
        "Launch: TANDEM_CLAUDE_OFFLINE=1 target/debug/tandem {} --state-dir {}",
        project_path.display(),
        state_path.display()
    );
    Ok(())
}
