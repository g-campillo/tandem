use tandem_core::*;
#[test]
fn captured_working_version_keeps_closed_drafts_and_prefers_live_buffers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let project = Project::open(&root).unwrap();
    let mut closed = project.open_document("closed.rs", true).unwrap();
    closed.snapshot("retained draft".into());
    let mut live = project.open_document("live.rs", true).unwrap();
    live.snapshot("checkpoint".into());
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    store
        .checkpoint(&WorkspaceState::new(&project), &[closed, live.clone()])
        .unwrap();
    live.snapshot("latest live version".into());
    let docs = store.working_documents(project.key(), &[live]).unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(
        docs.iter().find(|d| d.path == "closed.rs").unwrap().text,
        "retained draft"
    );
    assert_eq!(
        docs.iter().find(|d| d.path == "live.rs").unwrap().text,
        "latest live version"
    );
}
fn request() -> ModelRequest {
    ModelRequest {
        delegation: None,
        action: CompanionAction::Guide,
        question: "Explain the next step".into(),
        language: "rust".into(),
        guidance: GuidanceSettings::default().effective("p", "rust", None),
        task: None,
        context: ContextSnapshot {
            id: "context".into(),
            project_fingerprint: "project".into(),
            files: vec![],
            tree: vec![],
            omitted: vec![],
            captured_at_ms: 0,
            validation_environments: Default::default(),
        },
        evidence: vec![],
        prior_decisions: vec![],
    }
}

#[test]
fn restart_preserves_completed_responses_and_interrupts_pending_work_without_replaying_threads() {
    let temp = tempfile::tempdir().unwrap();
    let mut running = ModelRun::pending("p".into(), request());
    running.status = ModelStatus::Running;
    running.thread_id = Some("thread-active".into());
    running.turn_id = Some("turn-active".into());
    let preparing = ModelRun::pending("p".into(), request());
    let mut done = ModelRun::pending("p".into(), request());
    done.response = Some(CompanionResponse::Guidance(GuidanceResponse {
        summary: "Saved explanation".into(),
        sections: vec![],
        next_action: "Implement".into(),
        requested_context: vec![],
    }));
    done.finish(ModelStatus::Completed, "Complete");
    {
        let mut store = Store::open(temp.path()).unwrap();
        for run in [&running, &preparing, &done] {
            store.save_model_run(run).unwrap();
        }
        store
            .save_companion_thread(
                "p",
                "task:main:rust",
                &ThreadBinding {
                    thread_id: "thread-active".into(),
                    manifest: Default::default(),
                },
            )
            .unwrap();
    }
    let mut store = Store::open(temp.path()).unwrap();
    let runs = store.model_runs("p").unwrap();
    for id in [&running.id, &preparing.id] {
        assert_eq!(
            runs.iter().find(|r| &r.id == id).unwrap().status,
            ModelStatus::Interrupted
        );
    }
    assert_eq!(
        runs.iter().find(|r| r.id == done.id).unwrap().status,
        ModelStatus::Completed
    );
    assert!(
        store
            .companion_thread("p", "task:main:rust")
            .unwrap()
            .is_none()
    );
    store.save_model_run(&running).unwrap();
    assert_eq!(
        store
            .model_runs("p")
            .unwrap()
            .iter()
            .find(|r| r.id == running.id)
            .unwrap()
            .status,
        ModelStatus::Interrupted
    );
}
#[test]
fn schema_two_migration_backs_up_tasks_and_settings_reject_stale_updates() {
    let temp = tempfile::tempdir().unwrap();
    let task = TaskPlan::new(
        "p".into(),
        "Retain the task".into(),
        vec!["Criterion".into()],
        vec!["Step".into()],
    )
    .unwrap();
    {
        let mut store = Store::open(temp.path()).unwrap();
        store.save_task(&task).unwrap();
    }
    {
        let c = rusqlite::Connection::open(temp.path().join("workspace.sqlite")).unwrap();
        c.execute_batch(
            "DROP TABLE model_run; DROP TABLE companion_thread; PRAGMA user_version=2;",
        )
        .unwrap();
    }
    let mut store = Store::open(temp.path()).unwrap();
    assert_eq!(store.tasks("p").unwrap()[0].goal, task.goal);
    assert!(std::fs::read_dir(temp.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("workspace-schema2-backup-")
    }));
    let mut settings = store.guidance().unwrap();
    let stale = settings.clone();
    settings.sequence += 1;
    settings.defaults.explanation = Some(ExplanationDepth::Detailed);
    store.save_guidance(&settings).unwrap();
    assert!(store.save_guidance(&stale).is_err());
}
#[test]
fn unsupported_demonstrated_findings_become_concerns_and_optional_preferences_do_not_block() {
    let mut input = request();
    input.action = CompanionAction::Review;
    let raw = serde_json::json!({"conclusion":"NeedsChanges","summary":"Check these concerns","findings":[
        {"category":"DemonstratedProblem","blocking":true,"problem":"Possible issue","why":"Needs verification","next_action":"Run a focused test","path":"src/lib.rs","line":1,"criterion_indices":[],"evidence_ids":[]},
        {"category":"StylePreference","blocking":true,"problem":"Name preference","why":"Personal style","next_action":"Optionally rename","path":"src/lib.rs","line":1,"criterion_indices":[],"evidence_ids":[]}],"evidence_used":[],"validation_needed":["Tests"],"requested_context":[]});
    let CompanionResponse::Review(result) = input.parse_response(raw).unwrap() else {
        panic!()
    };
    assert_eq!(
        result.findings[0].category,
        FindingCategory::NeedsVerification
    );
    assert!(!result.findings[1].blocking);
    assert!(input.parse_response(serde_json::json!({"conclusion":"RequirementsSatisfied","summary":"Done","findings":[],"evidence_used":["invented"],"validation_needed":[],"requested_context":[]})).is_err());
}

#[test]
fn schema_four_migration_backs_up_before_upgrading_to_five() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO settings VALUES('checkpoint_sequence','17'); PRAGMA user_version=4;").unwrap();
    }
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.sequence().unwrap(), 17);
    let connection = rusqlite::Connection::open(path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
    assert!(std::fs::read_dir(directory.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("workspace-schema4-backup-")
    }));
}
