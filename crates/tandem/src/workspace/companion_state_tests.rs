use super::*;

#[test]
fn every_implementation_route_requires_current_approvals_and_start() {
    let session = TaskSession::new("/fixture".into()).unwrap();
    for action in [
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Review,
        CompanionAction::Reveal,
        CompanionAction::Delegate,
        CompanionAction::ImproveClarity,
        CompanionAction::ImproveSpeed,
        CompanionAction::ImproveBalanced,
    ] {
        assert!(
            command_binding(Some(&session), action, false).is_err(),
            "{action:?} bypassed approvals"
        );
    }
    for action in [
        CompanionAction::Analyze,
        CompanionAction::Ask,
        CompanionAction::Discuss,
        CompanionAction::Explain,
        CompanionAction::DraftSpec,
        CompanionAction::DraftPlan,
    ] {
        assert!(command_binding(Some(&session), action, false).is_ok());
        assert!(command_binding(Some(&session), action, true).is_err());
    }
    assert!(command_binding(None, CompanionAction::Guide, false).is_err());
}

#[test]
fn ordinary_chat_never_selects_reveal_and_dirty_approvals_are_refused() {
    assert_eq!(
        workflow_action(CompanionAction::Ask),
        WorkflowAction::Discuss
    );
    let saved = DocumentRef {
        revision: 1,
        hash: "saved".into(),
    };
    assert!(approval_matches(true, Some(&saved), Some(&saved)).is_err());
    assert!(approval_matches(false, Some(&saved), Some(&saved)).is_ok());
    assert!(approval_matches(false, Some(&saved), None).is_err());
}

#[test]
fn unavailable_model_choice_is_retained_and_default_is_hidden() {
    let preference = ModelPreference::Explicit {
        model: "chosen-model".into(),
    };
    assert!(model_label(&preference, None).contains("chosen-model · unavailable"));
    assert_eq!(
        preference,
        ModelPreference::Explicit {
            model: "chosen-model".into()
        }
    );
    let session = TaskSession::new("/fixture".into()).unwrap();
    let guidance = GuidanceSettings::default().effective("/fixture", "rust", None);
    assert!(preview_key(&session, "rust", &guidance).is_none());
}

#[path = "../../../tandem-agent/examples/support/mod.rs"]
mod support;
fn approved() -> (tempfile::TempDir, Store, TaskSession) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut task = TaskPlan::new(
        "/fixture".into(),
        "Add a greeting".into(),
        vec!["Return the greeting".into()],
        vec!["Implement".into()],
    )
    .unwrap();
    task.steps[0].languages = vec!["rust".into()];
    task.steps[0].done_when = vec!["A greeting is available".into()];
    let session = support::approved_session(&mut store, task).unwrap();
    (dir, store, session)
}
fn with_actions(session: &mut TaskSession) {
    let walkthrough = support::fixture_walkthrough(session);
    walkthrough
        .validate(
            session.spec.as_ref().unwrap(),
            session.plan.as_ref().unwrap(),
        )
        .unwrap();
    session.walkthroughs = vec![walkthrough];
}
#[test]
fn command_matrix_tracks_each_approval_start_and_document_replacement() {
    let (_dir, mut store, session) = approved();
    let routes = [
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Review,
        CompanionAction::ReviewAction,
        CompanionAction::Reveal,
        CompanionAction::Delegate,
        CompanionAction::ImproveClarity,
        CompanionAction::ImproveSpeed,
        CompanionAction::ImproveBalanced,
    ];
    for action in routes {
        assert!(command_binding(Some(&session), action, false).is_ok());
    }
    for missing in 0..4 {
        let mut s = session.clone();
        match missing {
            0 => s.spec = None,
            1 => s.spec_approval = None,
            2 => s.plan_approval = None,
            _ => s.started_generation = None,
        };
        for action in routes {
            assert!(command_binding(Some(&s), action, false).is_err());
        }
    }
    let binding = session.authorize(WorkflowAction::Reveal).unwrap();
    let same = store
        .append_user_message(
            &session.id,
            session.sequence,
            &new_id(),
            "Write a function, but discuss it first",
        )
        .unwrap();
    assert!(same.binding_is_current(WorkflowAction::Reveal, &binding));
    let mut changed = same.spec.clone().unwrap();
    changed.summary = "A changed greeting".into();
    let revised = store
        .apply_session_transition(
            &same.id,
            same.sequence,
            &SessionTransition::new(new_id(), vec![SessionCommand::SaveSpec { spec: changed }]),
        )
        .unwrap();
    assert!(revised.spec_approval.is_none() && revised.plan_approval.is_none());
    for action in routes {
        assert!(command_binding(Some(&revised), action, false).is_err());
    }
    assert!(!revised.binding_is_current(WorkflowAction::Reveal, &binding));
}
#[test]
fn preview_stays_on_current_action_language_and_never_exposes_stale_solution() {
    let (_dir, _store, mut session) = approved();
    with_actions(&mut session);
    let mut settings = GuidanceSettings::default();
    settings.defaults.solution = Some(SolutionPolicy::AutomaticPreview);
    let guidance = settings.effective("/fixture", "rust", session.plan.as_ref().map(|p| &p.task));
    let key = preview_key(&session, "rust", &guidance).unwrap();
    assert_eq!(key, preview_key(&session, "rust", &guidance).unwrap());
    assert!(preview_key(&session, "python", &guidance).is_none());
    let scope = SolutionScope {
        workflow: session.authorize(WorkflowAction::Reveal).unwrap(),
        action_scope: DelegationActionScope::current(&session).unwrap(),
        language: "rust".into(),
        guidance: guidance.fingerprint(),
    };
    assert!(solution_visible(&scope, Some(&session), "rust", &guidance));
    assert!(!solution_visible(
        &scope,
        Some(&session),
        "python",
        &guidance
    ));
    session.walkthroughs[0].cursor = 1;
    assert_ne!(key, preview_key(&session, "rust", &guidance).unwrap());
    assert!(!solution_visible(&scope, Some(&session), "rust", &guidance));
    session.started_generation = None;
    assert!(preview_key(&session, "rust", &guidance).is_none());
}
#[test]
fn stable_question_answer_and_composer_survive_session_switch_and_restart() {
    let (dir, mut store, mut session) = approved();
    let question = ClarificationQuestion {
        id: "question".into(),
        prompt: "Which greeting?".into(),
        choices: vec![
            QuestionChoice {
                id: "short".into(),
                label: "Short".into(),
                description: "Brief".into(),
            },
            QuestionChoice {
                id: "long".into(),
                label: "Long".into(),
                description: "Detailed".into(),
            },
        ],
    };
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::AskQuestion {
                    question,
                    run_id: None,
                }],
            ),
        )
        .unwrap();
    store
        .save_composer_draft(&session.id, 0, "Follow-up in progress")
        .unwrap();
    let other = store.create_session("/fixture").unwrap();
    store.select_session("/fixture", &session.id).unwrap();
    drop(store);
    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store.selected_session("/fixture").unwrap(),
        Some(session.id.clone())
    );
    assert_eq!(
        store.composer_draft(&session.id).unwrap().text,
        "Follow-up in progress"
    );
    assert!(store.composer_draft(&other.id).unwrap().text.is_empty());
    let answer = ClarificationAnswer {
        question_id: "question".into(),
        choice_id: None,
        custom_text: Some("A friendly greeting".into()),
    };
    let answered = store
        .record_question_answer(&session.id, session.sequence, &new_id(), answer.clone())
        .unwrap();
    assert!(answered.pending_question.is_none());
    assert!(
        store
            .record_question_answer(&session.id, answered.sequence, &new_id(), answer)
            .is_err()
    );
    assert!(answered.spec_approval.is_some() && answered.plan_approval.is_some());
}

#[test]
fn pending_answer_draft_is_retained_during_chat_failure_and_new_typing() {
    assert!(!clear_submitted_input(true, None, "My custom answer"));
    assert!(!clear_submitted_input(
        false,
        Some("My custom answer"),
        "My custom answer"
    ));
    assert!(!clear_submitted_input(
        true,
        Some("My custom answer"),
        "A revised answer"
    ));
    assert!(clear_submitted_input(
        true,
        Some("My custom answer"),
        "My custom answer"
    ));
}

#[test]
fn document_refresh_preserves_other_document_and_preexisting_edits() {
    for (edited, affected) in [
        (WorkflowDocumentKind::Plan, WorkflowDocumentKind::Spec),
        (WorkflowDocumentKind::Spec, WorkflowDocumentKind::Plan),
    ] {
        for approve in [false, true] {
            let (_dir, mut store, session) = approved();
            let mut draft = DocumentEditorDraft::from_session(&session, &[]);
            let key = if edited == WorkflowDocumentKind::Plan {
                "goal"
            } else {
                "summary"
            };
            draft
                .fields
                .iter_mut()
                .find(|f| f.key == key)
                .unwrap()
                .value = "Unsaved human work".into();
            let command = match (affected, approve) {
                (WorkflowDocumentKind::Spec, true) => SessionCommand::ApproveSpec {
                    document_ref: session.spec.as_ref().unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                },
                (WorkflowDocumentKind::Plan, true) => SessionCommand::ApprovePlan {
                    document_ref: session.plan.as_ref().unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                },
                (WorkflowDocumentKind::Spec, false) => {
                    let mut spec = session.spec.clone().unwrap();
                    spec.summary = "Changed saved spec".into();
                    SessionCommand::SaveSpec { spec }
                }
                (WorkflowDocumentKind::Plan, false) => {
                    let mut plan = session.plan.clone().unwrap();
                    plan.task.goal = "Changed saved plan".into();
                    SessionCommand::SavePlan { plan }
                }
            };
            let current = store.load_session(&session.id).unwrap().unwrap();
            let current = store
                .apply_session_transition(
                    &session.id,
                    current.sequence,
                    &SessionTransition::new(new_id(), vec![command]),
                )
                .unwrap();
            draft.refresh(&current, &[], Some(affected));
            assert_eq!(
                draft.fields.iter().find(|f| f.key == key).unwrap().value,
                "Unsaved human work"
            );
        }
    }
    let (_dir, store, session) = approved();
    let mut draft = DocumentEditorDraft::from_session(&session, &[]);
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "summary")
        .unwrap()
        .value = "Already dirty before request".into();
    let mut proposal = session.spec.clone().unwrap();
    proposal.revision += 1;
    proposal.summary = "Model proposal".into();
    let mut proposed = session.clone();
    let event = proposed.propose_spec(proposal, None).unwrap();
    draft.refresh(&session, &[event], None);
    assert_eq!(
        draft.read_spec().unwrap().summary,
        "Already dirty before request"
    );
    let mut sessions = SessionDocumentDrafts::default();
    sessions.preserve(&session.id, draft);
    let other = TaskSession::new("/fixture".into()).unwrap();
    let mut other_draft = sessions.restore(&other, &[]);
    other_draft
        .fields
        .iter_mut()
        .find(|f| f.key == "requirements")
        .unwrap()
        .value = "invalid raw criterion".into();
    sessions.preserve(&other.id, other_draft);
    let restored = sessions.restore(&session, &[]);
    assert_eq!(
        restored.read_spec().unwrap().summary,
        "Already dirty before request"
    );
    assert_eq!(
        store.load_session(&session.id).unwrap().unwrap().spec,
        session.spec
    );
    assert_eq!(
        sessions
            .restore(&other, &[])
            .fields
            .iter()
            .find(|f| f.key == "requirements")
            .unwrap()
            .value,
        "invalid raw criterion"
    );
}

fn propose_document(
    session: &mut TaskSession,
    kind: WorkflowDocumentKind,
    title: &str,
) -> ConversationEvent {
    match kind {
        WorkflowDocumentKind::Spec => {
            let mut spec = session.spec.clone().unwrap();
            spec.summary = title.into();
            session.propose_spec(spec, None).unwrap()
        }
        WorkflowDocumentKind::Plan => {
            let mut task = session.plan.as_ref().unwrap().task.clone();
            task.goal = title.into();
            session.propose_plan(task, None).unwrap()
        }
    }
}

#[test]
fn successive_proposals_replace_pristine_drafts_but_preserve_human_raw_edits() {
    for kind in [WorkflowDocumentKind::Spec, WorkflowDocumentKind::Plan] {
        let (_dir, _store, mut session) = approved();
        let mut events = vec![propose_document(&mut session, kind, "First proposal")];
        let mut draft = DocumentEditorDraft::from_session(&session, &events);
        events.push(propose_document(&mut session, kind, "Second proposal"));
        draft.refresh(&session, &events, None);
        let key = if kind == WorkflowDocumentKind::Spec {
            "summary"
        } else {
            "goal"
        };
        assert_eq!(
            draft.fields.iter().find(|f| f.key == key).unwrap().value,
            "Second proposal",
            "{kind:?}"
        );
        assert!(draft.dirty(kind, &session));
        let (displayed, saved) = match kind {
            WorkflowDocumentKind::Spec => (
                draft.spec.document_ref(),
                session.spec.as_ref().unwrap().document_ref(),
            ),
            WorkflowDocumentKind::Plan => (
                draft.plan.as_ref().unwrap().document_ref(),
                session.plan.as_ref().unwrap().document_ref(),
            ),
        };
        assert!(
            approval_matches(draft.dirty(kind, &session), Some(&displayed), Some(&saved)).is_err()
        );
        draft
            .fields
            .iter_mut()
            .find(|f| f.key == key)
            .unwrap()
            .value = "Human-edited proposal".into();
        events.push(propose_document(&mut session, kind, "Third proposal"));
        draft.refresh(&session, &events, None);
        assert_eq!(
            draft.fields.iter().find(|f| f.key == key).unwrap().value,
            "Human-edited proposal",
            "{kind:?}"
        );
        let other_kind = if kind == WorkflowDocumentKind::Spec {
            WorkflowDocumentKind::Plan
        } else {
            WorkflowDocumentKind::Spec
        };
        draft.refresh(&session, &events, Some(other_kind));
        assert_eq!(
            draft.fields.iter().find(|f| f.key == key).unwrap().value,
            "Human-edited proposal"
        );
    }
}

#[test]
fn restart_hydrates_unsaved_documents_outside_the_transcript_page() {
    let (dir, mut store, mut session) = approved();
    let spec_event = propose_document(
        &mut session.clone(),
        WorkflowDocumentKind::Spec,
        "Older spec proposal",
    );
    let plan_event = propose_document(
        &mut session.clone(),
        WorkflowDocumentKind::Plan,
        "Older plan proposal",
    );
    let ConversationPayload::DocumentProposed {
        proposal: DocumentProposal::Spec(spec),
    } = spec_event.payload
    else {
        unreachable!()
    };
    let ConversationPayload::DocumentProposed {
        proposal: DocumentProposal::Plan(plan),
    } = plan_event.payload
    else {
        unreachable!()
    };
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::ProposeSpec { spec, run_id: None },
                    SessionCommand::ProposePlan {
                        task: plan.task,
                        run_id: None,
                    },
                ],
            ),
        )
        .unwrap();
    for n in 0..105 {
        session = store
            .append_user_message(
                &session.id,
                session.sequence,
                &new_id(),
                &format!("Later message {n}"),
            )
            .unwrap();
    }
    let id = session.id.clone();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let session = store.load_session(&id).unwrap().unwrap();
    let page = store.conversation_events(&id, None, 100).unwrap();
    assert_eq!(page.len(), 100);
    assert!(
        !page
            .iter()
            .any(|e| matches!(e.payload, ConversationPayload::DocumentProposed { .. }))
    );
    let document_events = store.document_editor_events(&id).unwrap();
    assert_eq!(document_events.len(), 2);
    let draft = DocumentEditorDraft::from_session(&session, &document_events);
    assert_eq!(draft.read_spec().unwrap().summary, "Older spec proposal");
    assert_eq!(draft.read_plan().unwrap().task.goal, "Older plan proposal");
}

#[test]
fn editing_a_proposal_back_to_saved_content_consumes_it_even_on_noop_save() {
    for kind in [WorkflowDocumentKind::Spec, WorkflowDocumentKind::Plan] {
        let (dir, mut store, mut session) = approved();
        let event = propose_document(&mut session.clone(), kind, "Proposal to discard");
        let ConversationPayload::DocumentProposed { proposal } = event.payload else {
            unreachable!()
        };
        let command = match proposal {
            DocumentProposal::Spec(spec) => SessionCommand::ProposeSpec { spec, run_id: None },
            DocumentProposal::Plan(plan) => SessionCommand::ProposePlan {
                task: plan.task,
                run_id: None,
            },
        };
        session = store
            .apply_session_transition(
                &session.id,
                session.sequence,
                &SessionTransition::new(new_id(), vec![command]),
            )
            .unwrap();
        let events = store.conversation_events(&session.id, None, 100).unwrap();
        let mut draft = DocumentEditorDraft::from_session(&session, &events);
        let (key, saved_value) = match kind {
            WorkflowDocumentKind::Spec => {
                ("summary", session.spec.as_ref().unwrap().summary.clone())
            }
            WorkflowDocumentKind::Plan => {
                ("goal", session.plan.as_ref().unwrap().task.goal.clone())
            }
        };
        draft
            .fields
            .iter_mut()
            .find(|f| f.key == key)
            .unwrap()
            .value = saved_value.clone();
        let command = match kind {
            WorkflowDocumentKind::Spec => SessionCommand::SaveSpec {
                spec: draft.read_spec().unwrap(),
            },
            WorkflowDocumentKind::Plan => SessionCommand::SavePlan {
                plan: draft.read_plan().unwrap(),
            },
        };
        let saved = store
            .apply_session_transition(
                &session.id,
                session.sequence,
                &SessionTransition::new(new_id(), vec![command]),
            )
            .unwrap();
        assert_eq!(saved.sequence, session.sequence);
        assert_eq!(saved.spec_approval, session.spec_approval);
        assert_eq!(saved.plan_approval, session.plan_approval);
        let events = store.document_editor_events(&session.id).unwrap();
        draft.refresh(&saved, &events, Some(kind));
        assert_eq!(
            draft.fields.iter().find(|f| f.key == key).unwrap().value,
            saved_value
        );
        assert!(!draft.dirty(kind, &saved));
        let id = saved.id.clone();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let saved = store.load_session(&id).unwrap().unwrap();
        let events = store.document_editor_events(&id).unwrap();
        let draft = DocumentEditorDraft::from_session(&saved, &events);
        assert_eq!(
            draft.fields.iter().find(|f| f.key == key).unwrap().value,
            saved_value
        );
    }
}

#[test]
fn identical_proposal_noop_save_restores_exact_saved_refs_and_can_start() {
    let (_dir, mut store, fixture) = approved();
    let mut session = store.create_session("/fixture").unwrap();
    let mut plan = fixture.plan.clone().unwrap();
    plan.task.id = new_id();
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::SaveSpec {
                        spec: fixture.spec.unwrap(),
                    },
                    SessionCommand::SavePlan { plan },
                ],
            ),
        )
        .unwrap();
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::ApprovePlan {
                    document_ref: session.plan.as_ref().unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                }],
            ),
        )
        .unwrap();
    let mut proposal = session.spec.clone().unwrap();
    proposal.revision += 1;
    let proposed = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::ProposeSpec {
                    spec: proposal,
                    run_id: None,
                }],
            ),
        )
        .unwrap();
    let events = store.conversation_events(&session.id, None, 100).unwrap();
    let mut draft = DocumentEditorDraft::from_session(&proposed, &events);
    let saved = store
        .apply_session_transition(
            &session.id,
            proposed.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveSpec {
                    spec: draft.read_spec().unwrap(),
                }],
            ),
        )
        .unwrap();
    draft.refresh(&saved, &events, Some(WorkflowDocumentKind::Spec));
    assert!(!draft.dirty(WorkflowDocumentKind::Spec, &saved));
    assert_eq!(
        draft.spec.document_ref(),
        session.spec.as_ref().unwrap().document_ref()
    );
    let reloaded = DocumentEditorDraft::from_session(&saved, &events);
    assert!(!reloaded.dirty(WorkflowDocumentKind::Spec, &saved));
    let saved = store
        .apply_session_transition(
            &session.id,
            saved.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::ApproveSpec {
                        document_ref: reloaded.spec.document_ref(),
                        approved_at_ms: now_ms(),
                    },
                    SessionCommand::StartImplementation,
                ],
            ),
        )
        .unwrap();
    assert!(saved.authorize(WorkflowAction::PrepareAction).is_ok());
}

fn history_run(response: CompanionResponse) -> ModelRun {
    let mut run = ModelRun::pending(
        "/fixture".into(),
        ModelRequest {
            model_preference: ModelPreference::Automatic,
            resolved_model: None,
            conversation_id: None,
            guided: None,
            action: CompanionAction::Review,
            question: "Historical review".into(),
            language: "rust".into(),
            guidance: GuidanceSettings::default().effective("/fixture", "rust", None),
            task: None,
            context: ContextSnapshot {
                id: "capture".into(),
                project_fingerprint: "snapshot".into(),
                files: vec![],
                tree: vec![],
                omitted: vec![],
                captured_at_ms: 0,
                validation_environments: Default::default(),
            },
            evidence: vec![ReviewEvidence {
                id: "check-1".into(),
                kind: "test".into(),
                status: "Failed".into(),
                summary: "Edge case failed".into(),
                verified_failure: true,
                benchmark: None,
            }],
            prior_decisions: vec![],
            delegation: None,
        },
    );
    run.response = Some(response);
    run
}
#[test]
fn historical_review_and_task_definition_include_findings_sources_and_criteria() {
    let mut run = history_run(CompanionResponse::Review(ReviewResponse {
        conclusion: ReviewConclusion::NeedsChanges,
        summary: "Boundary failure".into(),
        findings: vec![ReviewFinding {
            category: FindingCategory::DemonstratedProblem,
            blocking: true,
            problem: "Empty input fails".into(),
            why: "Caller accepts empty values".into(),
            next_action: "Handle the boundary".into(),
            path: "lib.rs".into(),
            line: 7,
            criterion_indices: vec![0],
            evidence_ids: vec!["check-1".into()],
        }],
        evidence_used: vec!["check-1".into()],
        validation_needed: vec!["Run broader cases".into()],
        requested_context: vec![],
    }));
    run.status = ModelStatus::Completed;
    let rows = historical_run(&run);
    let text = rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Historical model assessment",
        "Empty input fails",
        "Caller accepts empty values",
        "Handle the boundary",
        "Run broader cases",
        "Historical check evidence",
        "check-1",
        "Failed",
        "Edge case failed",
    ] {
        assert!(text.contains(expected), "Missing {expected}");
    }
    assert!(
        rows.iter()
            .any(|r| r.source.as_ref() == Some(&("lib.rs".into(), 7)))
    );
    let (_dir, _store, session) = approved();
    let mut task = session.plan.unwrap().task;
    task.constraints = vec!["Preserve interface".into()];
    task.steps[0].why = "Expose the greeting".into();
    let text = historical_task(&task)
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Historical task definition",
        "Return the greeting",
        "Implement",
        "A greeting is available",
        "Expose the greeting",
        "Preserve interface",
    ] {
        assert!(text.contains(expected), "Missing {expected}");
    }
}

#[test]
fn adopted_legacy_history_keeps_original_definition_reviews_and_evidence_after_restart() {
    let (dir, mut store, fixture) = approved();
    let mut original = TaskPlan::new(
        "/fixture".into(),
        "Retained original legacy goal".into(),
        vec!["Original acceptance".into()],
        vec!["Original completed step".into()],
    )
    .unwrap();
    original.analysis = "Original analysis".into();
    original.start_step(&original.steps[0].id.clone()).unwrap();
    original
        .complete_active_manually(vec!["Original completion evidence".into()])
        .unwrap();
    store.save_task(&original).unwrap();
    let mut review = history_run(CompanionResponse::Review(ReviewResponse {
        conclusion: ReviewConclusion::NeedsChanges,
        summary: "Original historical assessment".into(),
        findings: vec![ReviewFinding {
            category: FindingCategory::DemonstratedProblem,
            blocking: true,
            problem: "Original boundary finding".into(),
            why: "Original caller contract".into(),
            next_action: "Inspect the boundary".into(),
            path: "lib.rs".into(),
            line: 7,
            criterion_indices: vec![0],
            evidence_ids: vec!["check-1".into()],
        }],
        evidence_used: vec!["check-1".into()],
        validation_needed: vec![],
        requested_context: vec![],
    }));
    review.status = ModelStatus::Completed;
    review.request.task = Some(original.clone());
    review.binding = review.request.binding();
    store.save_model_run(&review).unwrap();
    // These are the same read-only row builders used before adoption.
    let mut before = historical_task(&original);
    before.extend(historical_run(&review));
    let before_text = before
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(before_text.contains("Original boundary finding"));
    assert!(before_text.contains("Original completion evidence"));

    let mut session = store.adopt_legacy_task(&original.id).unwrap();
    let adopted = linked_legacy_history(&store, &session).unwrap();
    let adopted_text = adopted
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Linked legacy history · read-only",
        "Retained original legacy goal",
        "Original completed step",
        "Original analysis",
        "Original boundary finding",
        "Original completion evidence",
        "Original historical assessment",
        &review.id,
    ] {
        assert!(adopted_text.contains(expected), "Missing {expected}");
    }
    assert!(
        adopted
            .iter()
            .any(|r| r.source == Some(("lib.rs".into(), 7)))
    );
    assert!(
        adopted
            .iter()
            .any(|r| r.check_id.as_deref() == Some("check-1"))
    );
    assert!(session.authorize(WorkflowAction::ReviewAction).is_err());

    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveSpec {
                    spec: fixture.spec.unwrap(),
                }],
            ),
        )
        .unwrap();
    let mut replacement = original.clone();
    replacement.goal = "Current replacement plan goal".into();
    replacement.steps[0].title = "Current replacement step".into();
    let plan = PlanDocument {
        revision: 1,
        spec_ref: session.spec.as_ref().unwrap().document_ref(),
        task: replacement,
    };
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(new_id(), vec![SessionCommand::SavePlan { plan }]),
        )
        .unwrap();
    assert_eq!(
        session.plan.as_ref().unwrap().task.steps[0].status,
        StepStatus::Pending
    );
    for _ in 0..45 {
        let mut unrelated = review.clone();
        unrelated.id = new_id();
        unrelated.request.task = None;
        unrelated.binding = unrelated.request.binding();
        store.save_model_run(&unrelated).unwrap();
    }
    assert!(
        !store
            .model_runs("/fixture")
            .unwrap()
            .iter()
            .any(|r| r.id == review.id)
    );
    assert!(
        store
            .model_run("/other-project", &review.id)
            .unwrap()
            .is_none()
    );
    let id = session.id.clone();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let session = store.load_session(&id).unwrap().unwrap();
    let rows = linked_legacy_history(&store, &session).unwrap();
    let restarted_text = rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(restarted_text, adopted_text);
    assert!(!restarted_text.contains("Current replacement"));
    assert!(session.spec_approval.is_none() && session.plan_approval.is_none());
    assert!(session.authorize(WorkflowAction::ReviewAction).is_err());
}
#[test]
fn historical_improvement_keeps_both_decisions_and_never_exposes_solution_body() {
    for decision in [
        ImprovementDecision::KeepCurrent,
        ImprovementDecision::ProposeChange,
    ] {
        let proposal: ImprovementProposal = serde_json::from_value(serde_json::json!({"objective":"Balanced","decision":decision,"summary":"Consider the tradeoff","invariants":["Preserve duplicate behavior"],"preconditions":["Sorted random access"],"expected_complexity":"Expected logarithmic lookup","practical_costs":"Sorting first costs time","tradeoffs":["More memory"],"suggested_steps":["Inspect caller usage"],"evidence_ids":["check-1"],"validation_needed":["No measured benchmark yet"],"benchmark_plan":"Measure repeated and one-shot use"})).unwrap();
        let rows = historical_run(&history_run(CompanionResponse::Improvement(proposal)));
        let text = rows
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            format!("{decision:?}"),
            "Preserve duplicate behavior".into(),
            "Sorted random access".into(),
            "Sorting first costs time".into(),
            "More memory".into(),
            "Inspect caller usage".into(),
            "No measured benchmark yet".into(),
            "Measure repeated and one-shot use".into(),
            "Historical check evidence".into(),
        ] {
            assert!(text.contains(&expected), "Missing {expected}");
        }
    }
    let rows = historical_run(&history_run(CompanionResponse::Solution(
        SuggestedSolution {
            summary: "Hidden example".into(),
            language: "rust".into(),
            path: "lib.rs".into(),
            code: "PRIVATE_SOLUTION_BODY".into(),
            sections: vec![],
            tradeoffs: vec![],
        },
    )));
    assert!(
        !rows
            .iter()
            .any(|r| r.text.contains("PRIVATE_SOLUTION_BODY"))
    );
}

#[test]
fn clean_initial_document_accepts_a_proposal_and_history_shows_actual_measurements() {
    let mut session = TaskSession::new("/fixture".into()).unwrap();
    let mut draft = DocumentEditorDraft::from_session(&session, &[]);
    let (_dir, _store, fixture) = approved();
    let event = session.propose_spec(fixture.spec.unwrap(), None).unwrap();
    draft.refresh(&session, &[event], None);
    assert_eq!(draft.read_spec().unwrap().summary, "Add a greeting");
    assert!(session.spec.is_none());
    let proposal = ImprovementProposal {
        objective: ImprovementObjective::Speed,
        decision: ImprovementDecision::ProposeChange,
        summary: "Measured on supplied cases only".into(),
        invariants: vec!["Preserve outputs".into()],
        preconditions: vec![],
        expected_complexity: "Constant".into(),
        practical_costs: "No allocation".into(),
        tradeoffs: vec![],
        suggested_steps: vec![],
        evidence_ids: vec!["check-1".into()],
        validation_needed: vec![],
        benchmark_plan: "Broaden input sizes".into(),
    };
    let mut run = history_run(CompanionResponse::Improvement(proposal));
    run.request.evidence[0].status = "Passed".into();
    run.request.evidence[0].benchmark = Some(BenchmarkMeasurement {
        method: "perf_counter_ns; alternating order; deepcopy/setup excluded".into(),
        warmups: 2,
        samples: 7,
        cases: vec![BenchmarkCase {
            label: "Representative inputs".into(),
            baseline_ns: vec![100; 7],
            candidate_ns: vec![25; 7],
        }],
    });
    let text = historical_run(&run)
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Historical check evidence",
        "Measured on this snapshot only",
        "Representative inputs",
        "median 25",
        "7 samples",
    ] {
        assert!(text.contains(expected), "Missing {expected}");
    }
}
