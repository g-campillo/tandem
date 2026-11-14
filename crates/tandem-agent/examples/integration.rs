use anyhow::{Result, ensure};
use std::{
    collections::BTreeMap,
    fs,
    sync::{Mutex, atomic::AtomicBool},
};
use tandem_agent::*;
use tandem_checks::CheckEnvironment;
use tandem_companion::CompanionSession;
use tandem_core::*;

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--inspect") {
        return inspect_existing();
    }
    let root = std::env::current_dir()?
        .join(".tandem-local/m4/agent-integration")
        .join(new_id());
    let source = root.join("project");
    fs::create_dir_all(source.join("src"))?;
    fs::create_dir(source.join("tests"))?;
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname='tandem-agent-fixture'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    )?;
    fs::write(
        source.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"tandem-agent-fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        source.join("rust-toolchain.toml"),
        "[toolchain]\nchannel='1.100.0'\n",
    )?;
    let disk = "pub fn double(value: u32) -> u32 { todo!() }\n";
    fs::write(source.join("src/lib.rs"), disk)?;
    fs::write(source.join("src/main.rs"), "fn main() {}\n")?;
    fs::write(
        source.join("tests/behavior.rs"),
        r#"#[test] fn values(){for value in [0,1,21,1_000_000]{assert_eq!(tandem_agent_fixture::double(value),value*2);}}
#[test] fn program(){let output=std::process::Command::new(env!("CARGO_BIN_EXE_tandem-agent-fixture")).output().unwrap();assert!(output.status.success());assert_eq!(String::from_utf8(output.stdout).unwrap().trim(),"42");}
"#,
    )?;
    let project = Project::open(&source)?;
    let mut draft = project.open_document("src/lib.rs", false)?;
    draft.snapshot(format!("// Retain this pre-existing user note.\n{disk}"));
    let mut unrelated = project.open_document("notes.txt", true)?;
    unrelated.snapshot("Unrelated unsaved user work\n".into());
    let task = TaskPlan::new(
        project.key().into(),
        "Implement double in a helper module and connect the executable".into(),
        vec![
            "double returns twice inputs from 0 through 1,000,000".into(),
            "main prints 42 using the library function".into(),
            "Existing tests and user notes stay unchanged".into(),
        ],
        vec!["Implement and validate".into()],
    )?;
    let environment = CheckEnvironment::discover(&source, root.join("checks"))?;
    let recipe = environment.recipe("cargo-test")?;
    let grant=DelegationGrant{id:new_id(),project:project.key().into(),task_id:task.id.clone(),plan_revision:task.revision,step_id:Some(task.steps[0].id.clone()),scope:DelegationScope::Step,
        objective:"Implement double in new src/helper.rs, expose it from src/lib.rs without changing its public signature, and make src/main.rs print double(21). Preserve the pre-existing user comment and all tests. Run the approved cargo-test recipe and repair failures within scope.".into(),paths:vec!["src/lib.rs".into(),"src/main.rs".into(),"src/helper.rs".into()],checks:BTreeMap::from([("cargo-test".into(),recipe.grant_fingerprint)]),max_rounds:3,confirmed_at_ms:0};
    let store = Mutex::new(Store::open(&root.join("state"))?);
    store.lock().unwrap().save_task(&task)?;
    let mut run = prepare_delegation(
        &project,
        &[draft.clone(), unrelated.clone()],
        grant,
        &root.join("runs"),
        &AtomicBool::new(false),
    )?;
    store.lock().unwrap().save_agent_run(&run)?;
    run.grant.confirmed_at_ms = now_ms();
    run.sequence += 1;
    let mut session = CompanionSession::connect()?;
    let run = AgentExecution {
        session: &mut session,
        store: &store,
        original: &project,
        task: &task,
        environment: &environment,
        cancel: &AtomicBool::new(false),
    }
    .run(run, |r| {
        println!(
            "{:?}: {}",
            r.status,
            r.activity.last().unwrap_or(&r.summary)
        )
    })?;
    ensure!(
        run.status == AgentStatus::ReadyForReview && run.stage_verified,
        "Delegation failed: {} {:?}",
        run.summary,
        run.limitations
    );
    ensure!(
        run.changed_paths().len() == 3,
        "Multi-file changes were not staged: {:?}",
        run.changed_paths()
    );
    ensure!(
        store
            .lock()
            .unwrap()
            .checks(project.key())?
            .iter()
            .any(|c| c.status == CheckStatus::Passed),
        "No passing approved check"
    );
    ensure!(
        fs::read_to_string(source.join("src/lib.rs"))? == disk
            && !source.join("src/helper.rs").exists(),
        "Delegation wrote live source"
    );
    draft.snapshot(format!(
        "{}// Later non-overlapping user edit.\n",
        draft.text
    ));
    let import = preview_import(&run, &project, &[draft, unrelated.clone()])?;
    ensure!(
        import.conflicts.is_empty(),
        "Unexpected import conflict: {:?}",
        import.conflicts
    );
    let import = import.transaction.unwrap();
    let working = apply_to_working_set(std::slice::from_ref(&unrelated), &import);
    let library = working.iter().find(|d| d.path == "src/lib.rs").unwrap();
    ensure!(
        library.text.contains("pre-existing user note")
            && library.text.contains("Later non-overlapping"),
        "Import lost existing user work"
    );
    let mut workspace = WorkspaceState::new(&project);
    workspace.tabs = working.iter().map(|d| d.path.clone()).collect();
    workspace.sequence = 1;
    store
        .lock()
        .unwrap()
        .commit_import(&workspace, &working, &import)?;
    let undo = preview_undo(&import, &project, &working)?
        .transaction
        .context("Undo conflicted")?;
    let restored = apply_to_working_set(&working, &undo);
    ensure!(
        restored
            .iter()
            .any(|d| d.path == "notes.txt" && d.text == unrelated.text),
        "Undo removed unrelated work"
    );
    ensure!(
        !restored.iter().any(|d| d.path == "src/helper.rs"),
        "Undo retained the unchanged newly created draft"
    );
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"run":run,"import":import,"undo":undo,"original_source_unchanged":true,"unrelated_draft_preserved":true,"non_overlapping_user_change_preserved":true}),
        )?,
    )?;
    fs::write(
        ".tandem-local/m4/agent-integration-receipt.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"directory":root,"status":"passed","changed_files":3,"source_unchanged":true,"import_and_undo":true}),
        )?,
    )?;
    println!("Real subscription delegation, approved validation, import and inverse merge passed.");
    Ok(())
}
use anyhow::Context;

fn inspect_existing() -> Result<()> {
    let base = std::env::current_dir()?.join(".tandem-local/m4/agent-integration");
    let mut roots = fs::read_dir(base)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("state/workspace.sqlite").is_file())
        .collect::<Vec<_>>();
    roots.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
    let root = roots.pop().context("No retained delegation fixture")?;
    let project = Project::open(&root.join("project"))?;
    let mut store = Store::open(&root.join("state"))?;
    let mut run = store.agent_runs(project.key())?.remove(0);
    inspect_delegation(&mut run)?;
    let mut draft = run.baseline["src/lib.rs"].clone().unwrap();
    draft.snapshot(format!(
        "{}// Later non-overlapping user edit.\n",
        draft.text
    ));
    let mut unrelated = project.open_document("notes.txt", true)?;
    unrelated.snapshot("Unrelated unsaved user work\n".into());
    let preview = preview_import(&run, &project, &[draft, unrelated.clone()])?;
    ensure!(
        preview.conflicts.is_empty(),
        "Unexpected conflict: {:?}",
        preview.conflicts
    );
    let import = preview.transaction.unwrap();
    let working = apply_to_working_set(std::slice::from_ref(&unrelated), &import);
    let text = &working
        .iter()
        .find(|d| d.path == "src/lib.rs")
        .unwrap()
        .text;
    ensure!(
        text.contains("pre-existing user note") && text.contains("Later non-overlapping"),
        "User work lost"
    );
    let mut workspace = WorkspaceState::new(&project);
    workspace.tabs = working.iter().map(|d| d.path.clone()).collect();
    workspace.sequence = store.sequence()? + 1;
    store.commit_import(&workspace, &working, &import)?;
    let undo = preview_undo(&import, &project, &working)?
        .transaction
        .context("Undo conflicted")?;
    let restored = apply_to_working_set(&working, &undo);
    ensure!(
        restored
            .iter()
            .any(|d| d.path == "notes.txt" && d.text == unrelated.text),
        "Unrelated work lost"
    );
    ensure!(
        !restored.iter().any(|d| d.path == "src/helper.rs"),
        "Created draft not removed"
    );
    workspace.sequence += 1;
    workspace.tabs = restored.iter().map(|d| d.path.clone()).collect();
    store.commit_import(&workspace, &restored, &undo)?;
    ensure!(
        fs::read_to_string(project.root.join("src/lib.rs"))?
            == "pub fn double(value: u32) -> u32 { todo!() }\n"
            && !project.root.join("src/helper.rs").exists(),
        "Original source changed"
    );
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"run":run,"import":import,"undo":undo,"original_source_unchanged":true,"unrelated_draft_preserved":true,"non_overlapping_user_change_preserved":true}),
        )?,
    )?;
    fs::write(
        ".tandem-local/m4/agent-integration-receipt.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"directory":root,"status":"passed","changed_files":3,"source_unchanged":true,"import_and_undo":true,"overlapping_insertion_blocked_in_first_attempt":true}),
        )?,
    )?;
    println!(
        "Retained real delegation passed non-overlapping import and journaled undo; originals unchanged."
    );
    Ok(())
}
