//! Disposable native QA seed, no runtime connection or subscription usage.
#[path = "../../tandem-agent/examples/support/mod.rs"]
mod support;
use anyhow::{Result, ensure};
use std::{fs, path::PathBuf};
use tandem_core::*;

fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("tandem-guided-ui-{}", new_id())));
    ensure!(!root.exists(), "Refusing to overwrite a native QA fixture");
    let project_path = root.join("project");
    let state_path = root.join("state");
    fs::create_dir_all(&project_path)?;
    fs::write(
        project_path.join("greeting.rs"),
        "// Offline Rust editor fixture\n",
    )?;
    fs::write(
        project_path.join("greeting.py"),
        "# Offline Python editor fixture\n",
    )?;
    fs::write(
        project_path.join("README.md"),
        "Disposable deterministic UI fixture. No model, command or public search was run.\n",
    )?;
    let project = Project::open(&project_path)?;
    let mut store = Store::open(&state_path)?;
    store.save_model_preference(&ModelPreference::Explicit {
        model: "tandem-ui-unavailable-fixture".into(),
    })?;
    let mut session_ids = vec![];
    for language in ["rust", "python"] {
        let path = if language == "rust" {
            "greeting.rs"
        } else {
            "greeting.py"
        };
        let mut task = TaskPlan::new(
            project.key().into(),
            format!("Add a friendly {language} greeting"),
            vec!["The named greeting returns the expected text".into()],
            vec!["Implement the greeting".into()],
        )?;
        task.steps[0].files = vec![path.into()];
        task.steps[0].languages = vec![language.into()];
        task.steps[0].done_when = task.acceptance.clone();
        let session = support::approved_session(&mut store, task)?;
        let walk = support::fixture_walkthrough(&session);
        store.apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            )
            .with_authorization(
                WorkflowAction::PrepareWalkthrough,
                session.authorize(WorkflowAction::PrepareWalkthrough)?,
            ),
        )?;
        session_ids.push((format!("approved-{language}"), session.id));
    }
    let approved = store.load_session(&session_ids[0].1)?.unwrap();
    let mut current = store.create_session(project.key())?;
    let mut plan = approved.plan.unwrap();
    plan.task.id = new_id();
    current=store.apply_session_transition(&current.id,current.sequence,&SessionTransition::new(new_id(),vec![
        SessionCommand::HumanMessage{text:"Discuss the desired greeting before implementation. Keep the solution hidden.".into()},
        SessionCommand::AssistantMessage{text:"These are deterministic QA documents and sources. Edit and approve each document, then explicitly Start.".into(),run_id:"ui-seed".into()},
        SessionCommand::SaveSpec{spec:approved.spec.unwrap()},SessionCommand::SavePlan{plan},
        SessionCommand::AskQuestion{question:ClarificationQuestion{id:"greeting-style".into(),prompt:"Which greeting style should we discuss?".into(),choices:vec![QuestionChoice{id:"brief".into(),label:"Brief".into(),description:"A short greeting".into()},QuestionChoice{id:"warm".into(),label:"Warm".into(),description:"A warmer greeting".into()}]},run_id:None},
        SessionCommand::ResearchActivity{activity:ResearchActivity::Stopped{reason:ResearchStopReason::Unavailable,detail:"Seeded limitation: no live research was attempted. Sources in a real session require immutable receipts.".into(),continue_available:true}},
    ]))?;
    store.save_composer_draft(&current.id, 0, "Saved composer text survives restart")?;
    session_ids.push((
        "discussion-documents-pending-question".into(),
        current.id.clone(),
    ));
    let mut history = store.create_session(project.key())?;
    for index in 0..105 {
        history = store.append_user_message(
            &history.id,
            history.sequence,
            &new_id(),
            &format!("Deterministic saved discussion message {}", index + 1),
        )?;
    }
    session_ids.push(("paged-history".into(), history.id));
    let request = ModelRequest {
        model_preference: ModelPreference::Explicit {
            model: "tandem-ui-unavailable-fixture".into(),
        },
        resolved_model: None,
        conversation_id: Some(current.id.clone()),
        guided: None,
        action: CompanionAction::Discuss,
        question: "Deterministic offline history fixture".into(),
        language: "rust".into(),
        guidance: GuidanceSettings::default().effective(project.key(), "rust", None),
        task: None,
        context: tandem_companion::capture_context(
            &project,
            &[],
            &["README.md".into()],
            &root.join("captures"),
            &std::sync::atomic::AtomicBool::new(false),
        )?,
        evidence: vec![],
        prior_decisions: vec![],
        delegation: None,
    };
    let mut run = ModelRun::pending(project.key().into(), request);
    run.response = Some(CompanionResponse::Discussion(ConversationReply {
        text: "Deterministic saved discussion; no AI was connected.".into(),
        question: None,
        findings: vec![],
        research_requests: vec![],
    }));
    run.finish(
        ModelStatus::Completed,
        "Seeded offline history only; not a model result",
    );
    store.save_model_run(&run)?;
    let mut failed = ModelRun::pending(project.key().into(), run.request.clone());
    failed.finish(
        ModelStatus::Failed,
        "Seeded unavailable-model error. Editing and saved work remain available.",
    );
    store.save_model_run(&failed)?;
    let mut workspace = WorkspaceState::new(&project);
    workspace.sequence = 1;
    workspace.tabs = vec!["greeting.rs".into(), "greeting.py".into()];
    workspace.active = Some("greeting.rs".into());
    store.checkpoint(
        &workspace,
        &[
            project.open_document("greeting.rs", false)?,
            project.open_document("greeting.py", false)?,
        ],
    )?;
    store.select_session(project.key(), &current.id)?;
    fs::write(
        root.join("seed-receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"class":"D/S","project":project_path,"state_dir":state_path,"sessions":session_ids,"subscription_turns":0,"limitations":"Native UI not launched or verified by this seed. All approvals are explicit fixture Store commands, never inferred from conversation."}),
        )?,
    )?;
    println!(
        "project={}\nstate_dir={}\nreceipt={}\nLaunch: target/debug/tandem {} --state-dir {}",
        project_path.display(),
        state_path.display(),
        root.join("seed-receipt.json").display(),
        project_path.display(),
        state_path.display()
    );
    Ok(())
}
