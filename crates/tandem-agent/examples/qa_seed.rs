use anyhow::{Context, Result, ensure};
use std::{fs, path::PathBuf};
use tandem_core::*;
fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--improvement") {
        return seed_improvement();
    }
    let base = std::env::current_dir()?.join(".tandem-local/m4");
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(base.join("agent-integration-receipt.json"))?)?;
    let root = PathBuf::from(marker["directory"].as_str().context("Missing directory")?);
    let project = Project::open(&root.join("project"))?;
    let source = Store::open(&root.join("state"))?;
    let run = source.agent_runs(project.key())?.remove(0);
    let mut task = source
        .model_runs(project.key())?
        .into_iter()
        .find(|m| m.request.action == CompanionAction::Delegate)
        .context("No real run")?
        .request
        .task
        .context("No accepted task")?;
    if task.active_step.is_none() {
        let id = task.steps[0].id.clone();
        task.start_step(&id)?;
    }
    let state = base.join("qa-state");
    ensure!(
        !state.join("workspace.sqlite").exists(),
        "QA state already exists; do not overwrite it"
    );
    let mut store = Store::open(&state)?;
    store.save_task(&task)?;
    store.save_agent_run(&run)?;
    for check in source.checks(project.key())? {
        store.save_check(&check)?;
    }
    let docs = run
        .baseline
        .values()
        .filter_map(Clone::clone)
        .collect::<Vec<_>>();
    let mut workspace = WorkspaceState::new(&project);
    workspace.tabs = docs.iter().map(|d| d.path.clone()).collect();
    workspace.active = Some("src/lib.rs".into());
    workspace.sequence = 1;
    store.checkpoint(&workspace, &docs)?;
    fs::write(base.join("qa-project.txt"), project.key())?;
    println!(
        "Seeded separate M4 QA profile with retained real result and unsaved working baseline."
    );
    Ok(())
}

fn seed_improvement() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m4");
    let run: ModelRun = serde_json::from_slice(&fs::read(root.join("improvement-receipt.json"))?)?;
    let project = Project::open(std::path::Path::new(&run.project))?;
    let state = root.join("improvement-qa-state");
    ensure!(
        !state.join("workspace.sqlite").exists(),
        "Improvement QA already exists"
    );
    let mut store = Store::open(&state)?;
    store.save_model_run(&run)?;
    let doc = project.open_document("benchmarks/tandem_compare.py", false)?;
    let mut workspace = WorkspaceState::new(&project);
    workspace.tabs = vec![doc.path.clone()];
    workspace.active = Some(doc.path.clone());
    workspace.sequence = 1;
    store.checkpoint(&workspace, &[doc])?;
    println!("Prepared separate improvement QA with the real measured proposal.");
    Ok(())
}
