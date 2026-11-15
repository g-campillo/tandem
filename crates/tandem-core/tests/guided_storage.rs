use rusqlite::{Connection, params};
use tandem_core::*;

fn spec() -> SpecDocument {
    SpecDocument {
        revision: 1,
        summary: "Read a greeting".into(),
        requirements: vec![Criterion {
            id: "greeting".into(),
            text: "Return a greeting".into(),
        }],
        acceptance: vec![Criterion {
            id: "test".into(),
            text: "A test passes".into(),
        }],
        in_scope: vec![],
        out_of_scope: vec![],
        constraints: vec![],
    }
}
fn task() -> TaskPlan {
    TaskPlan::new(
        "p".into(),
        "Greeting".into(),
        vec!["A greeting".into()],
        vec!["Implement greeting".into()],
    )
    .unwrap()
}
fn question() -> ClarificationQuestion {
    ClarificationQuestion {
        id: "language".into(),
        prompt: "Which language?".into(),
        choices: vec![
            QuestionChoice {
                id: "rust".into(),
                label: "Rust".into(),
                description: "Use Rust".into(),
            },
            QuestionChoice {
                id: "python".into(),
                label: "Python".into(),
                description: "Use Python".into(),
            },
        ],
    }
}
fn apply(store: &mut Store, session: &TaskSession, command: SessionCommand) -> TaskSession {
    store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(new_id(), vec![command]),
        )
        .unwrap()
}
fn planned(store: &mut Store) -> TaskSession {
    let mut session = store.create_session("p").unwrap();
    session = apply(store, &session, SessionCommand::SaveSpec { spec: spec() });
    let plan = PlanDocument {
        revision: 1,
        spec_ref: session.spec.as_ref().unwrap().document_ref(),
        task: task(),
    };
    apply(store, &session, SessionCommand::SavePlan { plan })
}
fn approve_and_start(store: &mut Store, mut session: TaskSession) -> TaskSession {
    let spec_ref = session.spec.as_ref().unwrap().document_ref();
    let plan_ref = session.plan.as_ref().unwrap().document_ref();
    session = apply(
        store,
        &session,
        SessionCommand::ApproveSpec {
            document_ref: spec_ref,
            approved_at_ms: 5,
        },
    );
    session = apply(
        store,
        &session,
        SessionCommand::ApprovePlan {
            document_ref: plan_ref,
            approved_at_ms: 6,
        },
    );
    apply(store, &session, SessionCommand::StartImplementation)
}

#[test]
fn schema_four_migration_preserves_every_legacy_row_and_creates_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.sqlite");
    let old = task();
    let original = serde_json::to_string(&old).unwrap();
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE task(id TEXT PRIMARY KEY,root TEXT NOT NULL,revision INTEGER NOT NULL,updated INTEGER NOT NULL,data TEXT NOT NULL); CREATE TABLE task_revision(id TEXT NOT NULL,revision INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(id,revision)); CREATE TABLE model_run(id TEXT PRIMARY KEY,root TEXT NOT NULL,sequence INTEGER NOT NULL,data TEXT NOT NULL); CREATE TABLE companion_thread(root TEXT NOT NULL,name TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(root,name)); CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL); PRAGMA user_version=4;").unwrap();
        c.execute(
            "INSERT INTO task VALUES(?1,'p',1,1,?2)",
            params![old.id, original],
        )
        .unwrap();
        c.execute(
            "INSERT INTO task_revision VALUES(?1,1,?2)",
            params![old.id, original],
        )
        .unwrap();
        // Historical pre-plan and task model rows must survive migration verbatim.
        c.execute(
            "INSERT INTO model_run VALUES('pre-plan','p',0,?1)",
            [legacy_run(None)],
        )
        .unwrap();
        c.execute(
            "INSERT INTO model_run VALUES('review','p',0,?1)",
            [legacy_run(Some(&old))],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings VALUES('guidance',?1)",
            [serde_json::to_string(&GuidanceSettings::default()).unwrap()],
        )
        .unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.tasks("p").unwrap()[0].id, old.id);
    assert_eq!(store.model_runs("p").unwrap().len(), 2);
    assert!(store.sessions("p").unwrap().is_empty());
    let backup = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("workspace-schema4-backup-")
        })
        .unwrap();
    let backup = Connection::open(backup).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        backup
            .query_row("SELECT data FROM task", [], |r| r.get::<_, String>(0))
            .unwrap(),
        original
    );
    let c = Connection::open(path).unwrap();
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
}
fn legacy_run(task: Option<&TaskPlan>) -> String {
    let request = ModelRequest {
        model_preference: ModelPreference::Automatic,
        resolved_model: None,
        conversation_id: None,
        guided: None,
        action: CompanionAction::Review,
        question: "Check the greeting".into(),
        language: "rust".into(),
        guidance: GuidanceSettings::default().effective("p", "rust", None),
        task: task.cloned(),
        context: ContextSnapshot {
            id: "context".into(),
            project_fingerprint: "fingerprint".into(),
            files: vec![],
            tree: vec![],
            omitted: vec![],
            captured_at_ms: 0,
            validation_environments: Default::default(),
        },
        evidence: vec![],
        prior_decisions: vec![],
        delegation: None,
    };
    let mut run = ModelRun::pending("p".into(), request);
    run.id = if task.is_some() { "review" } else { "pre-plan" }.into();
    run.finish(ModelStatus::Completed, "Historical assessment");
    serde_json::to_string(&run).unwrap()
}

#[test]
fn transcript_is_append_only_paginated_and_event_retries_require_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut s = store.create_session("p").unwrap();
    let id = s.id.clone();
    for n in 0..85 {
        s = store
            .append_user_message(
                &id,
                s.sequence,
                &format!("message-{n}"),
                &format!("Message {n}"),
            )
            .unwrap();
    }
    let generation = s.execution_generation;
    assert_eq!(
        store
            .append_user_message(&id, 0, "message-0", "Message 0")
            .unwrap()
            .sequence,
        85
    );
    assert!(
        store
            .append_user_message(&id, 85, "message-0", "Changed")
            .is_err()
    );
    assert!(store.append_user_message(&id, 0, "late", "Stale").is_err());
    let mut all = vec![];
    let mut before = None;
    loop {
        let page = store.conversation_events(&id, before, 17).unwrap();
        if page.is_empty() {
            break;
        }
        before = Some(page[0].sequence);
        all.extend(page);
    }
    all.sort_by_key(|e| e.sequence);
    assert_eq!(all.len(), 85);
    assert_eq!(all[0].sequence, 1);
    assert_eq!(all[84].sequence, 85);
    assert_eq!(s.execution_generation, generation);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.sessions("p").unwrap()[0].id, id);
    assert_eq!(store.load_session(&id).unwrap().unwrap().sequence, 85);
}

#[test]
fn restart_preserves_question_composer_selection_and_interrupts_operation_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    {
        let mut store = Store::open(dir.path()).unwrap();
        let mut s = store.create_session("p").unwrap();
        id = s.id.clone();
        s = apply(
            &mut store,
            &s,
            SessionCommand::AskQuestion {
                question: question(),
                run_id: Some("run".into()),
            },
        );
        store.save_composer_draft(&id, 0, "My draft").unwrap();
        assert!(store.save_composer_draft(&id, 0, "Stale draft").is_err());
        let op = SessionOperation {
            id: "operation".into(),
            session_id: id.clone(),
            kind: SessionOperationKind::Research,
            started_at_ms: 1,
        };
        apply(
            &mut store,
            &s,
            SessionCommand::BeginOperation { operation: op },
        );
    }
    for _ in 0..2 {
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.selected_session("p").unwrap().as_deref(),
            Some(id.as_str())
        );
        assert_eq!(store.composer_draft(&id).unwrap().text, "My draft");
        let s = store.load_session(&id).unwrap().unwrap();
        assert_eq!(s.pending_question.unwrap().id, "language");
        assert_eq!(s.sequence, 3);
        assert!(store.active_session_operation(&id).unwrap().is_none());
        assert!(matches!(
            store.session_operation_outcome("operation").unwrap(),
            Some(OperationOutcome::Interrupted { .. })
        ));
        assert_eq!(store.conversation_events(&id, None, 200).unwrap().len(), 3);
    }
    let mut store = Store::open(dir.path()).unwrap();
    let answer = ClarificationAnswer {
        question_id: "language".into(),
        choice_id: None,
        custom_text: Some("Python please".into()),
    };
    let s = store
        .record_question_answer(&id, 3, "answer", answer.clone())
        .unwrap();
    assert!(s.pending_question.is_none());
    assert_eq!(s.answered_questions.len(), 1);
    assert_eq!(
        store
            .record_question_answer(&id, 3, "answer", answer.clone())
            .unwrap()
            .sequence,
        4
    );
    assert!(
        store
            .record_question_answer(&id, 4, "second-answer", answer)
            .is_err()
    );
    assert!(
        store
            .apply_session_transition(
                &id,
                4,
                &SessionTransition::new(
                    "late-outcome",
                    vec![SessionCommand::FinishOperation {
                        operation_id: "operation".into(),
                        outcome: OperationOutcome::Completed {
                            summary: "Late".into()
                        }
                    }]
                )
            )
            .is_err()
    );
}

#[test]
fn documents_receipts_and_projection_commit_atomically_and_cannot_be_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let s = planned(&mut store);
    let saved_ref = s.plan.as_ref().unwrap().document_ref();
    let saved = store
        .document_revision(&s.id, WorkflowDocumentKind::Plan, saved_ref.revision)
        .unwrap()
        .unwrap();
    let saved_data = serde_json::to_string(&saved).unwrap();
    let s = approve_and_start(&mut store, s);
    let task = &s.plan.as_ref().unwrap().task;
    assert_eq!(
        store.tasks("p").unwrap()[0].state_sequence,
        task.state_sequence
    );
    assert!(store.save_task(task).is_err());
    assert_eq!(
        serde_json::to_string(
            &store
                .document_revision(&s.id, WorkflowDocumentKind::Plan, saved_ref.revision)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        saved_data
    );
    let binding = s.authorize(WorkflowAction::ReviewAction).unwrap();
    let mut s = store
        .append_user_message(&s.id, s.sequence, "chat", "A follow up")
        .unwrap();
    assert!(s.binding_is_current(WorkflowAction::ReviewAction, &binding));
    let receipt = ResearchReceipt::new(
        ResearchRequest::new(ResearchRequestKind::ListFiles {
            prefix: None,
            limit: 10,
        })
        .unwrap(),
        vec![],
        ResearchReceiptStatus::Unavailable,
        "No files".into(),
        vec![],
    )
    .unwrap();
    s = apply(
        &mut store,
        &s,
        SessionCommand::RecordResearchReceipt {
            receipt: receipt.clone(),
        },
    );
    assert_eq!(
        store.research_receipt(&s.id, &receipt.id).unwrap(),
        Some(receipt.clone())
    );
    let mut altered = receipt.clone();
    altered.content = "Different".into();
    altered.content_hash = content_hash(altered.content.as_bytes());
    altered.limits.returned_bytes = altered.content.len();
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    "rewrite",
                    vec![SessionCommand::RecordResearchReceipt { receipt: altered }]
                )
            )
            .is_err()
    );
    let mut changed_spec = spec();
    changed_spec.summary = "A new greeting".into();
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveSpec { spec: changed_spec },
    );
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    "stale-binding",
                    vec![SessionCommand::AssistantMessage {
                        text: "Late review".into(),
                        run_id: "late-run".into()
                    }]
                )
                .with_authorization(WorkflowAction::ReviewAction, binding)
            )
            .is_err()
    );
    assert_eq!(
        store.research_receipt(&s.id, &receipt.id).unwrap(),
        Some(receipt)
    );
}

#[test]
fn injected_projection_failure_rolls_back_session_events_documents_and_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let s = store.create_session("p").unwrap();
    let c = Connection::open(dir.path().join("workspace.sqlite")).unwrap();
    c.execute_batch("CREATE TRIGGER fail_projection BEFORE INSERT ON task BEGIN SELECT RAISE(ABORT,'injected projection failure'); END;").unwrap();
    let receipt = ResearchReceipt::new(
        ResearchRequest::new(ResearchRequestKind::ListFiles {
            prefix: None,
            limit: 10,
        })
        .unwrap(),
        vec![],
        ResearchReceiptStatus::Unavailable,
        "Unavailable".into(),
        vec![],
    )
    .unwrap();
    let transition = SessionTransition::new(
        "atomic",
        vec![
            SessionCommand::SaveSpec { spec: spec() },
            SessionCommand::RecordResearchReceipt {
                receipt: receipt.clone(),
            },
            SessionCommand::SavePlan {
                plan: PlanDocument {
                    revision: 1,
                    spec_ref: spec().document_ref(),
                    task: task(),
                },
            },
        ],
    );
    let error = store
        .apply_session_transition(&s.id, 0, &transition)
        .unwrap_err();
    assert!(
        error.to_string().contains("injected projection failure"),
        "{error:#}"
    );
    assert_eq!(store.load_session(&s.id).unwrap().unwrap().sequence, 0);
    assert!(
        store
            .conversation_events(&s.id, None, 200)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .document_revision(&s.id, WorkflowDocumentKind::Spec, 1)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .research_receipt(&s.id, &receipt.id)
            .unwrap()
            .is_none()
    );
    assert!(store.tasks("p").unwrap().is_empty());
    c.execute_batch("DROP TRIGGER fail_projection;").unwrap();
    let committed = store
        .apply_session_transition(&s.id, 0, &transition)
        .unwrap();
    assert_eq!(committed.sequence, 3);
    assert_eq!(
        store
            .apply_session_transition(&s.id, 0, &transition)
            .unwrap()
            .sequence,
        3
    );
}

#[test]
fn legacy_adoption_is_idempotent_preserves_history_and_never_invents_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut old = task();
    old.revise("Revised greeting".into(), vec![], vec!["Implement".into()])
        .unwrap();
    old.start_step(&old.steps[0].id.clone()).unwrap();
    old.complete_active_manually(vec![]).unwrap();
    store.save_task(&old).unwrap();
    let run: ModelRun = serde_json::from_str(&legacy_run(Some(&old))).unwrap();
    store.save_model_run(&run).unwrap();
    let mut s = store.adopt_legacy_task(&old.id).unwrap();
    assert_eq!(store.adopt_legacy_task(&old.id).unwrap().id, s.id);
    assert!(s.plan.is_none() && s.spec.is_none());
    assert!(s.spec_approval.is_none() && s.plan_approval.is_none());
    assert!(s.authorize(WorkflowAction::ReviewAction).is_err());
    assert_eq!(s.legacy_task.as_ref().unwrap().review_ids, vec![run.id]);
    assert_eq!(
        store.legacy_task(&s.id).unwrap().unwrap().steps[0].status,
        StepStatus::Complete
    );
    assert!(store.save_task(&old).is_err());
    s = apply(&mut store, &s, SessionCommand::SaveSpec { spec: spec() });
    let plan = PlanDocument {
        revision: 1,
        spec_ref: s.spec.as_ref().unwrap().document_ref(),
        task: old.clone(),
    };
    s = apply(&mut store, &s, SessionCommand::SavePlan { plan });
    let current = &s.plan.as_ref().unwrap().task;
    assert_eq!(current.id, old.id);
    assert!(current.revision > old.revision);
    assert!(current.state_sequence > old.state_sequence);
    assert_eq!(current.steps[0].status, StepStatus::Pending);
    assert_eq!(
        store.legacy_task(&s.id).unwrap().unwrap().steps[0].status,
        StepStatus::Complete
    );
}

fn walkthrough(session: &TaskSession) -> Walkthrough {
    let spec = session.spec.as_ref().unwrap();
    let plan = session.plan.as_ref().unwrap();
    let step_id = plan.task.steps[0].id.clone();
    Walkthrough {
        id: "walkthrough".into(),
        generation: session.execution_generation,
        step_id: step_id.clone(),
        plan_ref: plan.document_ref(),
        cursor: 0,
        actions: vec![
            GuidedAction {
                id: "create-file".into(),
                revision: 1,
                kind: GuidedActionKind::Edit,
                what: "Create a greeting file".into(),
                target: ActionTarget::File {
                    path: "greeting.rs".into(),
                    symbol: None,
                    change_kind: FileChangeKind::Create,
                },
                why: "The greeting needs a module".into(),
                expected_result: "The module exists".into(),
                required_evidence: vec![RequiredActionEvidence::FileSnapshot {
                    path: "greeting.rs".into(),
                }],
            },
            GuidedAction {
                id: "validate".into(),
                revision: 1,
                kind: GuidedActionKind::ValidateStep,
                what: "Assess the greeting".into(),
                target: ActionTarget::Validation {
                    spec_ref: spec.document_ref(),
                    plan_ref: plan.document_ref(),
                    step_id: Some(step_id),
                    criterion_refs: vec![CriterionRef::SpecAcceptance { id: "test".into() }],
                    paths: vec!["greeting.rs".into()],
                },
                why: "Check the cumulative result".into(),
                expected_result: "The greeting meets the criteria".into(),
                required_evidence: vec![RequiredActionEvidence::CriterionAssessment {
                    criterion_ref: CriterionRef::SpecAcceptance { id: "test".into() },
                }],
            },
        ],
    }
}

#[test]
fn immutable_action_receipt_dedup_is_atomic_and_does_not_itself_change_progress() {
    // Task 2 exercises the journal primitive. Task 6 owns review validation and Next.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let s = planned(&mut store);
    let mut s = approve_and_start(&mut store, s);
    let walk = walkthrough(&s);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walk.clone(),
        },
    );
    let record = ConsumedActionTransition {
        id: "consumption".into(),
        review_id: "review-1".into(),
        from_cursor: 0,
        to_cursor: 1,
        consumed_at_ms: 10,
        binding: ActionReviewBinding {
            workflow: s.authorize(WorkflowAction::AdvanceAction).unwrap(),
            walkthrough_id: walk.id.clone(),
            step_id: walk.step_id.clone(),
            action_id: walk.actions[0].id.clone(),
            action_revision: 1,
            action_hash: walk.actions[0].definition_hash(),
            context_id: "context".into(),
            project_fingerprint: "snapshot".into(),
            language: "rust".into(),
            guidance_fingerprint: "guidance".into(),
            evidence: vec![],
        },
    };
    s = apply(
        &mut store,
        &s,
        SessionCommand::RecordActionTransition {
            transition: record.clone(),
        },
    );
    let sequence = s.sequence;
    s = apply(
        &mut store,
        &s,
        SessionCommand::RecordActionTransition {
            transition: record.clone(),
        },
    );
    assert_eq!(s.sequence, sequence);
    assert_eq!(s.walkthroughs[0].cursor, 0);
    let mut changed = record.clone();
    changed.consumed_at_ms += 1;
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    "changed-record",
                    vec![SessionCommand::RecordActionTransition {
                        transition: changed
                    }]
                )
            )
            .is_err()
    );
    let mut duplicate_action = record.clone();
    duplicate_action.id = "other-transition".into();
    duplicate_action.review_id = "review-2".into();
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    "duplicate-action",
                    vec![SessionCommand::RecordActionTransition {
                        transition: duplicate_action
                    }]
                )
            )
            .is_err()
    );
    let mut duplicate_review = record.clone();
    duplicate_review.id = "other-transition".into();
    duplicate_review.binding.action_id = "validate".into();
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    "duplicate-review",
                    vec![SessionCommand::RecordActionTransition {
                        transition: duplicate_review
                    }]
                )
            )
            .is_err()
    );
    let id = s.id.clone();
    let original_events = store.conversation_events(&id, None, 200).unwrap().len();
    drop(store);
    for _ in 0..2 {
        let store = Store::open(dir.path()).unwrap();
        let loaded = store.load_session(&id).unwrap().unwrap();
        assert_eq!(loaded.walkthroughs[0].id, "walkthrough");
        assert_eq!(loaded.walkthroughs[0].actions[0].id, "create-file");
        assert_eq!(loaded.walkthroughs[0].cursor, 0);
        assert_eq!(
            store
                .action_transition(&id, "consumption")
                .unwrap()
                .unwrap()
                .review_id,
            record.review_id
        );
        assert_eq!(
            store.conversation_events(&id, None, 200).unwrap().len(),
            original_events
        );
    }
}

#[test]
fn session_selection_and_saved_document_versions_are_isolated_and_noop_saves_keep_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let s = planned(&mut store);
    let mut s = approve_and_start(&mut store, s);
    let binding = s.authorize(WorkflowAction::ReviewAction).unwrap();
    let sequence = s.sequence;
    let current_spec = s.spec.clone().unwrap();
    let plan = s.plan.clone().unwrap();
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveSpec { spec: current_spec },
    );
    s = apply(&mut store, &s, SessionCommand::SavePlan { plan });
    assert_eq!(s.sequence, sequence);
    store
        .save_composer_draft(&s.id, 0, "Still editing this draft")
        .unwrap();
    assert!(s.binding_is_current(WorkflowAction::ReviewAction, &binding));
    let other = store.create_session("other-project").unwrap();
    assert!(store.select_session("p", &other.id).is_err());
    assert_eq!(
        store.selected_session("p").unwrap().as_deref(),
        Some(s.id.as_str())
    );
    assert!(
        store
            .research_receipt(&other.id, "missing")
            .unwrap()
            .is_none()
    );
    let mut new_spec = s.spec.clone().unwrap();
    new_spec.summary = "New definition".into();
    s = apply(&mut store, &s, SessionCommand::SaveSpec { spec: new_spec });
    assert_eq!(s.spec.as_ref().unwrap().revision, 2);
    assert!(s.spec_approval.is_none() && s.plan_approval.is_none());
    assert!(matches!(
        store
            .document_revision(&s.id, WorkflowDocumentKind::Spec, 1)
            .unwrap(),
        Some(DocumentProposal::Spec(SpecDocument { revision: 1, .. }))
    ));
    assert!(matches!(
        store
            .document_revision(&s.id, WorkflowDocumentKind::Spec, 2)
            .unwrap(),
        Some(DocumentProposal::Spec(SpecDocument { revision: 2, .. }))
    ));
    let second = store.create_session("p").unwrap();
    let foreign = PlanDocument {
        revision: 1,
        spec_ref: spec().document_ref(),
        task: s.plan.as_ref().unwrap().task.clone(),
    };
    assert!(
        store
            .apply_session_transition(
                &second.id,
                0,
                &SessionTransition::new(
                    "foreign-task",
                    vec![
                        SessionCommand::SaveSpec { spec: spec() },
                        SessionCommand::SavePlan { plan: foreign }
                    ]
                )
            )
            .is_err()
    );
    assert_eq!(store.load_session(&second.id).unwrap().unwrap().sequence, 0);
}

#[test]
fn terminal_operation_fence_rejects_late_artifacts_and_preserves_current_action() {
    for cancelled in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let s = planned(&mut store);
        let mut s = approve_and_start(&mut store, s);
        s = apply(
            &mut store,
            &s,
            SessionCommand::SaveWalkthrough {
                walkthrough: walkthrough(&s),
            },
        );
        s = apply(
            &mut store,
            &s,
            SessionCommand::BeginOperation {
                operation: SessionOperation {
                    id: "review-operation".into(),
                    session_id: s.id.clone(),
                    kind: SessionOperationKind::ReviewAction,
                    started_at_ms: 0,
                },
            },
        );
        let record = ConsumedActionTransition {
            id: "late-consumption".into(),
            review_id: "late-review".into(),
            from_cursor: 0,
            to_cursor: 1,
            consumed_at_ms: 1,
            binding: ActionReviewBinding {
                workflow: s.authorize(WorkflowAction::AdvanceAction).unwrap(),
                walkthrough_id: "walkthrough".into(),
                step_id: s.walkthroughs[0].step_id.clone(),
                action_id: "create-file".into(),
                action_revision: 1,
                action_hash: s.walkthroughs[0].actions[0].definition_hash(),
                context_id: "context".into(),
                project_fingerprint: "snapshot".into(),
                language: "rust".into(),
                guidance_fingerprint: "guidance".into(),
                evidence: vec![],
            },
        };
        if cancelled {
            s = apply(
                &mut store,
                &s,
                SessionCommand::FinishOperation {
                    operation_id: "review-operation".into(),
                    outcome: OperationOutcome::Cancelled {
                        detail: "Cancelled by user".into(),
                    },
                },
            );
        }
        let id = s.id.clone();
        drop(store);
        let mut store = Store::open(dir.path()).unwrap();
        let s = store.load_session(&id).unwrap().unwrap();
        let late = SessionTransition::new(
            "late-result",
            vec![SessionCommand::RecordActionTransition { transition: record }],
        )
        .for_operation("review-operation");
        let error = store
            .apply_session_transition(&id, s.sequence, &late)
            .unwrap_err();
        assert!(error.to_string().contains("already terminal"), "{error:#}");
        assert!(
            store
                .action_transition(&id, "late-consumption")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.load_session(&id).unwrap().unwrap().walkthroughs[0].cursor,
            0
        );
        assert_eq!(
            store.load_session(&id).unwrap().unwrap().sequence,
            s.sequence
        );
    }
}
