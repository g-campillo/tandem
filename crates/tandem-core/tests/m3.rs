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
        model_preference: ModelPreference::Automatic,
        resolved_model: None,
        conversation_id: None,
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
                    ..Default::default()
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

#[test]
fn model_choice_and_snapshot_survive_restart_without_changing_guidance_or_transcript() {
    let directory = tempfile::tempdir().unwrap();
    let choice = ModelPreference::Explicit {
        model: "runtime-choice".into(),
    };
    let resolved = ResolvedModel {
        model: "runtime-choice".into(),
        catalog_id: "catalog-entry".into(),
        display_name: "Chosen model".into(),
        effort: Some("high".into()),
    };
    let (session_id, history, completed_id, guidance);
    {
        let mut store = Store::open(directory.path()).unwrap();
        assert_eq!(
            store.model_preference().unwrap(),
            ModelPreference::Automatic
        );
        let session = store.create_session("p").unwrap();
        let session = store
            .append_user_message(
                &session.id,
                session.sequence,
                "message",
                "Retain this conversation across model changes",
            )
            .unwrap();
        session_id = session.id.clone();
        history = serde_json::to_value(store.conversation_events(&session.id, None, 20).unwrap())
            .unwrap();
        guidance = serde_json::to_value(store.guidance().unwrap()).unwrap();
        store.save_model_preference(&choice).unwrap();
        let mut req = request();
        req.conversation_id = Some(session.id.clone());
        req.model_preference = choice.clone();
        req.resolved_model = Some(resolved.clone());
        let mut run = ModelRun::pending("p".into(), req);
        run.finish(ModelStatus::Completed, "Historical explanation");
        completed_id = run.id.clone();
        store.save_model_run(&run).unwrap();
        store
            .save_model_preference(&ModelPreference::Automatic)
            .unwrap();
        assert_eq!(
            serde_json::to_value(store.conversation_events(&session.id, None, 20).unwrap())
                .unwrap(),
            history
        );
        store.save_model_preference(&choice).unwrap();
        let mut changed_guidance = store.guidance().unwrap();
        changed_guidance.sequence += 1;
        store.save_guidance(&changed_guidance).unwrap();
        assert_eq!(store.model_preference().unwrap(), choice);
    }
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.model_preference().unwrap(), choice);
    let current_guidance = store.guidance().unwrap();
    assert_eq!(
        current_guidance.defaults,
        serde_json::from_value::<GuidanceSettings>(guidance)
            .unwrap()
            .defaults
    );
    assert_eq!(
        serde_json::to_value(store.conversation_events(&session_id, None, 20).unwrap()).unwrap(),
        history
    );
    let saved = store
        .model_runs("p")
        .unwrap()
        .into_iter()
        .find(|r| r.id == completed_id)
        .unwrap();
    assert_eq!(saved.request.resolved_model, Some(resolved));
    assert_eq!(
        saved.request.conversation_id.as_deref(),
        Some(session_id.as_str())
    );
}

#[test]
fn legacy_model_metadata_is_unknown_and_never_inherits_current_selection() {
    let binding: ThreadBinding =
        serde_json::from_value(serde_json::json!({"thread_id":"old","manifest":{}})).unwrap();
    assert!(
        binding.resolved_model.is_none()
            && binding.identity.is_none()
            && binding.instructions_hash.is_none()
    );
    let mut old = serde_json::to_value(ModelRun::pending("p".into(), request())).unwrap();
    let object = old["request"].as_object_mut().unwrap();
    object.remove("resolved_model");
    object.remove("model_preference");
    object.remove("conversation_id");
    old.as_object_mut().unwrap().remove("thread_binding");
    let old: ModelRun = serde_json::from_value(old).unwrap();
    assert!(old.request.resolved_model.is_none() && old.thread_binding.is_none());
    assert_eq!(old.request.model_preference, ModelPreference::Automatic);
}

#[test]
fn legacy_plan_acceptance_only_transfers_a_known_matching_model_and_conversation() {
    let mut req = request();
    req.action = CompanionAction::Analyze;
    req.conversation_id = Some("draft".into());
    let resolved = ResolvedModel {
        model: "a".into(),
        catalog_id: "entry-a".into(),
        display_name: "Model A".into(),
        effort: Some("high".into()),
    };
    req.resolved_model = Some(resolved.clone());
    let mut run = ModelRun::pending("p".into(), req);
    run.thread_id = Some("thread".into());
    run.thread_binding = Some(ThreadBinding {
        thread_id: "thread".into(),
        manifest: Default::default(),
        resolved_model: Some(resolved),
        identity: Some(run.request.thread_identity(&run.id)),
        instructions_hash: Some("instructions".into()),
    });
    run.finish(ModelStatus::Completed, "Plan proposal");
    let transferred = run
        .legacy_task_thread("accepted-task", &ModelPreference::Automatic)
        .unwrap();
    assert_eq!(transferred.identity.unwrap().session_id, "accepted-task");
    assert!(
        run.legacy_task_thread(
            "accepted-task",
            &ModelPreference::Explicit {
                model: "different".into()
            }
        )
        .is_none()
    );
    run.thread_binding.as_mut().unwrap().resolved_model = None;
    assert!(
        run.legacy_task_thread("accepted-task", &ModelPreference::Automatic)
            .is_none()
    );
}
