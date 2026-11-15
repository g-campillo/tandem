//! Offline Task 8 native-view seed. Supplies deterministic history, never model turns.
//! Also reusable by later Task 9/10 QA; it does not claim live model behavior.
mod support;
use anyhow::{Context, Result, ensure};
use std::{fs, path::PathBuf};
use tandem_core::*;
fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("Pass a new disposable directory")?,
    );
    ensure!(
        !root.exists(),
        "Refusing to overwrite an existing QA directory"
    );
    fs::create_dir_all(root.join("project"))?;
    fs::write(
        root.join("project/greeting.rs"),
        "// Offline editor fixture\n",
    )?;
    let project = Project::open(&root.join("project"))?;
    let mut store = Store::open(&root.join("state"))?;
    store.save_model_preference(&ModelPreference::Explicit {
        model: "tandem-ui-unavailable-fixture".into(),
    })?;
    let mut task = TaskPlan::new(
        project.key().into(),
        "Add a friendly greeting".into(),
        vec!["Return the expected greeting".into()],
        vec!["Implement the greeting".into()],
    )?;
    task.steps[0].files = vec!["greeting.rs".into()];
    task.steps[0].languages = vec!["rust".into()];
    task.steps[0].why = "Make the greeting available to callers".into();
    task.steps[0].done_when = task.acceptance.clone();
    let ready = support::approved_session(&mut store, task.clone())?;
    let walkthrough = support::fixture_walkthrough(&ready);
    store.apply_session_transition(
        &ready.id,
        ready.sequence,
        &SessionTransition::new(
            new_id(),
            vec![SessionCommand::SaveWalkthrough { walkthrough }],
        )
        .with_authorization(
            WorkflowAction::PrepareWalkthrough,
            ready.authorize(WorkflowAction::PrepareWalkthrough)?,
        ),
    )?;
    let mut legacy = task.clone();
    legacy.id = new_id();
    legacy.goal = "Legacy greeting history".into();
    store.save_task(&legacy)?;
    let mut discussion = store.create_session(project.key())?;
    for i in 0..105 {
        discussion = store.append_user_message(
            &discussion.id,
            discussion.sequence,
            &new_id(),
            &format!("Saved discussion message {}", i + 1),
        )?;
    }
    store.save_composer_draft(&discussion.id, 0, "A saved follow-up draft")?;
    let approved = store.load_session(&ready.id)?.unwrap();
    let mut current = store.create_session(project.key())?;
    let spec = approved.spec.unwrap();
    let mut plan = approved.plan.unwrap();
    plan.task.id = new_id();
    current=store.apply_session_transition(&current.id,current.sequence,&SessionTransition::new(new_id(),vec![SessionCommand::HumanMessage{text:"Research a friendly greeting and prepare documents. Keep implementation hidden.".into()},SessionCommand::AssistantMessage{text:"The saved documents are ready to inspect. Approval and Start are separate actions.".into(),run_id:"deterministic-seed".into()},SessionCommand::SaveSpec{spec},SessionCommand::SavePlan{plan},SessionCommand::AskQuestion{question:ClarificationQuestion{id:"greeting-choice".into(),prompt:"Which greeting style should we discuss?".into(),choices:vec![QuestionChoice{id:"brief".into(),label:"Brief".into(),description:"A short greeting".into()},QuestionChoice{id:"warm".into(),label:"Warm".into(),description:"A friendly greeting".into()}]},run_id:None},SessionCommand::ResearchActivity{activity:ResearchActivity::PublicBriefPrepared{brief:PublicResearchBrief{id:"public-brief".into(),topics:vec!["Rust string documentation".into()],queries:vec!["Rust String official documentation".into()],urls:vec!["https://doc.rust-lang.org/std/string/struct.String.html".into()]},model:ResolvedModel{model:"seed-only".into(),catalog_id:"seed-only".into(),display_name:"Deterministic fixture".into(),effort:None}}},SessionCommand::ResearchActivity{activity:ResearchActivity::Stopped{reason:ResearchStopReason::Unavailable,detail:"Seeded limitation: no live search was run.".into(),continue_available:true}}]))?;
    store.save_composer_draft(&current.id, 0, "Follow-up preserved across restart")?;
    let mut state = WorkspaceState::new(&project);
    state.tabs = vec!["greeting.rs".into()];
    state.active = Some("greeting.rs".into());
    state.sequence = 1;
    store.checkpoint(&state, &[project.open_document("greeting.rs", false)?])?;
    println!(
        "{}\n{}\nDiscussion: {}\nSaved documents: {}\nApproved current action: {}",
        project.root.display(),
        root.join("state").display(),
        discussion.id,
        current.id,
        ready.id
    );
    Ok(())
}
