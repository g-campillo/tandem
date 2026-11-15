use super::*;

#[test]
fn every_implementation_route_requires_current_approvals_and_start() {
    let session = TaskSession::new("/fixture".into()).unwrap();
    for action in [
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Explain,
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

fn lossless_documents() -> (tempfile::TempDir, Store, TaskSession) {
    let (dir, mut store, fixture) = approved();
    let mut spec = fixture.spec.unwrap();
    spec.summary = "  Preserve the API\nand its behavior | exactly  \n".into();
    spec.requirements[0].text = spec.summary.clone();
    spec.acceptance[0].text = "\tSame inputs | same outputs\r\nincluding boundaries \n".into();
    spec.in_scope = vec![" first\nsecond | third ".into(), "separate entry\n".into()];
    spec.out_of_scope = vec![" \nNo removals\t ".into()];
    spec.constraints = vec!["\tKeep whitespace\r\nand separators | intact\n".into()];
    spec.validate().unwrap();
    let mut plan = fixture.plan.unwrap();
    plan.task.id = new_id();
    plan.spec_ref = spec.document_ref();
    plan.task.goal = spec.summary.clone();
    plan.task.analysis = " Analysis\nwith | separators \n".into();
    // TaskPlan permits empty entries too: the editor must preserve them.
    plan.task.acceptance = vec![" first\nsecond | third ".into(), "".into(), "\n".into()];
    plan.task.constraints = vec![" constraint\r\nnext | part ".into(), "".into()];
    plan.task.unknowns = vec![" question\ncontinued | value \n".into()];
    let step = &mut plan.task.steps[0];
    step.title = " Step\ncontinued | title ".into();
    step.why = " Reason\ncontinued | reason \n".into();
    step.files = vec![
        " src/with | separator.rs ".into(),
        "src/line\nbreak.rs".into(),
    ];
    step.symbols = vec![" symbol\ncontinued | name ".into(), "".into()];
    step.languages = vec![" rust\npython | other ".into(), "".into()];
    step.done_when = vec![
        " Done\nwhen | exact \n".into(),
        " Another condition ".into(),
    ];
    let mut second = step.clone();
    second.id = new_id();
    second.depends_on = vec![step.id.clone()];
    plan.task.steps.push(second);
    plan.validate().unwrap();
    let session = store.create_session("/fixture").unwrap();
    let session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::SaveSpec { spec },
                    SessionCommand::SavePlan { plan },
                ],
            ),
        )
        .unwrap();
    (dir, store, session)
}

#[test]
fn untouched_structured_documents_preserve_exact_refs_and_allow_save_approve_start() {
    let (_dir, mut store, session) = lossless_documents();
    let draft = DocumentEditorDraft::from_session(&session, &[]);
    let spec = draft.read_spec().unwrap();
    let plan = draft.read_plan().unwrap();
    assert_eq!(spec, *session.spec.as_ref().unwrap());
    assert_eq!(
        serde_json::to_value(&plan).unwrap(),
        serde_json::to_value(session.plan.as_ref().unwrap()).unwrap()
    );
    assert_eq!(
        spec.document_ref(),
        session.spec.as_ref().unwrap().document_ref()
    );
    assert_eq!(
        plan.document_ref(),
        session.plan.as_ref().unwrap().document_ref()
    );
    assert!(!draft.dirty(WorkflowDocumentKind::Spec, &session));
    assert!(!draft.dirty(WorkflowDocumentKind::Plan, &session));
    let saved = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::SaveSpec { spec },
                    SessionCommand::SavePlan { plan },
                ],
            ),
        )
        .unwrap();
    assert_eq!(saved.sequence, session.sequence);
    let started = store
        .apply_session_transition(
            &saved.id,
            saved.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::ApproveSpec {
                        document_ref: draft.spec.document_ref(),
                        approved_at_ms: now_ms(),
                    },
                    SessionCommand::ApprovePlan {
                        document_ref: draft.plan.as_ref().unwrap().document_ref(),
                        approved_at_ms: now_ms(),
                    },
                    SessionCommand::StartImplementation,
                ],
            ),
        )
        .unwrap();
    assert!(started.authorize(WorkflowAction::PrepareAction).is_ok());
    let draft = DocumentEditorDraft::from_session(&started, &[]);
    let saved = store
        .apply_session_transition(
            &started.id,
            started.sequence,
            &SessionTransition::new(
                new_id(),
                vec![
                    SessionCommand::SaveSpec {
                        spec: draft.read_spec().unwrap(),
                    },
                    SessionCommand::SavePlan {
                        plan: draft.read_plan().unwrap(),
                    },
                ],
            ),
        )
        .unwrap();
    assert_eq!(saved.sequence, started.sequence);
    assert_eq!(saved.spec_approval, started.spec_approval);
    assert_eq!(saved.plan_approval, started.plan_approval);
    assert_eq!(saved.started_generation, started.started_generation);
}

#[test]
fn unrelated_document_field_edits_preserve_every_other_value_and_entry_boundary() {
    let (_dir, _store, session) = lossless_documents();
    let mut draft = DocumentEditorDraft::from_session(&session, &[]);
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "summary")
        .unwrap()
        .value = "Edited summary".into();
    let mut expected = session.spec.clone().unwrap();
    expected.summary = "Edited summary".into();
    assert_eq!(draft.read_spec().unwrap(), expected);
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "goal")
        .unwrap()
        .value = "Edited goal".into();
    let mut expected = session.plan.clone().unwrap();
    expected.task.goal = "Edited goal".into();
    assert_eq!(
        serde_json::to_value(draft.read_plan().unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn structured_entry_add_remove_and_invalid_human_input_remain_lossless() {
    let (_dir, _store, session) = lossless_documents();
    let mut draft = DocumentEditorDraft::from_session(&session, &[]);
    let mut expected = session.spec.clone().unwrap();
    draft.add_entry("requirements").unwrap();
    let text = "  New requirement\r\nwith | separators\n  ";
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "requirements.1.id")
        .unwrap()
        .value = "extra-requirement".into();
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "requirements.1.text")
        .unwrap()
        .value = text.into();
    expected.requirements.push(Criterion {
        id: "extra-requirement".into(),
        text: text.into(),
    });
    assert_eq!(draft.read_spec().unwrap(), expected);
    draft.remove_entry("requirements", 0).unwrap();
    expected.requirements.remove(0);
    assert_eq!(draft.read_spec().unwrap(), expected);
    draft.remove_entry("in_scope", 0).unwrap();
    expected.in_scope.remove(0);
    draft.add_entry("in_scope").unwrap();
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "in_scope.1")
        .unwrap()
        .value = text.into();
    expected.in_scope.push(text.into());
    assert_eq!(draft.read_spec().unwrap(), expected);

    let mut expected_plan = session.plan.clone().unwrap();
    draft.add_entry("unknowns").unwrap();
    expected_plan.task.unknowns.push(String::new());
    draft.remove_entry("step.0.files", 0).unwrap();
    expected_plan.task.steps[0].files.remove(0);
    assert_eq!(
        serde_json::to_value(draft.read_plan().unwrap()).unwrap(),
        serde_json::to_value(expected_plan).unwrap()
    );

    let invalid = "  invalid | ID\nretained verbatim  ";
    draft
        .fields
        .iter_mut()
        .find(|f| f.key == "requirements.0.id")
        .unwrap()
        .value = invalid.into();
    assert!(draft.read_spec().is_err());
    assert!(draft.dirty(WorkflowDocumentKind::Spec, &session));
    draft.refresh(&session, &[], Some(WorkflowDocumentKind::Plan));
    let mut drafts = SessionDocumentDrafts::default();
    drafts.preserve(&session.id, draft);
    let restored = drafts.restore(&session, &[]);
    assert!(restored.read_spec().is_err());
    assert_eq!(
        restored
            .fields
            .iter()
            .find(|f| f.key == "requirements.0.id")
            .unwrap()
            .value,
        invalid
    );
    assert_eq!(
        restored
            .fields
            .iter()
            .find(|f| f.key == "requirements.0.text")
            .unwrap()
            .value,
        text
    );
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
    let (_dir, mut store, mut session) = approved();
    with_actions(&mut session);
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
        .find(|f| f.key == "requirements.0.id")
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
            .find(|f| f.key == "requirements.0.id")
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

#[test]
fn local_operation_loading_belongs_only_to_its_initiator_and_cleanup_is_fenced() {
    let mut operation =
        ForegroundOperation::new("one".into(), "session".into(), CompanionAction::Review);
    for phase in [
        OperationPhase::PreservingInput,
        OperationPhase::CapturingContext,
        OperationPhase::CheckingAccount,
        OperationPhase::CheckingModels,
        OperationPhase::ReadingSource,
        OperationPhase::SearchingSource,
        OperationPhase::InspectingSource,
        OperationPhase::PublicSearch,
        OperationPhase::PublicRetrieval,
        OperationPhase::Discussing,
        OperationPhase::DraftingSpec,
        OperationPhase::DraftingPlan,
        OperationPhase::PreparingAction,
        OperationPhase::Reviewing,
        OperationPhase::Revealing,
        OperationPhase::Stopping,
    ] {
        operation.phase = phase;
        let own = operation.button(CompanionAction::ReviewAction, "Review");
        assert!(own.loading && own.disabled);
        assert!(own.label.contains(phase_label(phase)));
        let other = operation.button(CompanionAction::DraftSpec, "Draft Spec");
        assert!(!other.loading && other.disabled);
        assert!(other.label.contains("Another operation is active"));
    }
    operation.stop();
    assert_eq!(operation.phase, OperationPhase::Stopping);
    assert!(!operation.owns("old", "session"));
    assert!(!operation.owns("one", "other-session"));
    assert!(operation.owns("one", "session"));
    assert!(
        operation
            .button(CompanionAction::Review, "Review")
            .label
            .contains("Stopping")
    );
    operation.control = Some("answer:choice".into());
    assert!(operation.control_button("answer:choice", "Answer").loading);
    assert!(!operation.control_button("answer:other", "Answer").loading);
    assert!(!operation.button(CompanionAction::Review, "Review").loading);
    let send = ForegroundOperation::new("send".into(), "session".into(), CompanionAction::Ask);
    assert!(send.button(CompanionAction::Ask, "Send").loading);
    assert!(
        !send
            .button(CompanionAction::Analyze, "Analyze / discuss")
            .loading
    );
}

#[test]
fn exact_recipe_matching_never_substitutes_the_models_command() {
    let (_, _, session) = approved();
    let mut action = support::fixture_walkthrough(&session).actions.remove(0);
    let recipe = tandem_checks::Recipe {
        id: "cargo-check".into(),
        title: "Check".into(),
        argv: vec!["/tools/cargo".into(), "check".into()],
        fingerprint: "recipe".into(),
        grant_fingerprint: "grant".into(),
        language: "rust".into(),
        version: "test".into(),
        read_roots: vec![],
    };
    action.kind = GuidedActionKind::Command;
    action.target = ActionTarget::Command {
        argv: recipe.argv.clone(),
        cwd: ".".into(),
        expected_exit_codes: vec![0],
    };
    assert_eq!(
        matching_action_recipe(&action, std::slice::from_ref(&recipe)),
        Some("cargo-check")
    );
    for argv in [
        vec!["cargo".into(), "check".into()],
        vec!["/tools/cargo".into(), "test".into()],
        vec!["/tools/cargo".into(), "check".into(), "{artifacts}".into()],
    ] {
        action.target = ActionTarget::Command {
            argv,
            cwd: ".".into(),
            expected_exit_codes: vec![0],
        };
        assert!(matching_action_recipe(&action, std::slice::from_ref(&recipe)).is_none());
    }
    action.target = ActionTarget::Command {
        argv: recipe.argv.clone(),
        cwd: "subdir".into(),
        expected_exit_codes: vec![0],
    };
    assert!(matching_action_recipe(&action, &[recipe]).is_none());
}

#[test]
fn attachment_draft_requires_explicit_exact_argv_and_preserves_incomplete_input() {
    let draft = OutputAttachmentDraft {
        command: "[\"cargo\",\"test\"]".into(),
        cwd: ".".into(),
        exit: "unknown".into(),
        stdout: "retained output\n".into(),
        stderr: "retained error\n".into(),
    };
    assert!(draft.attachment().is_err());
    assert_eq!(draft.stdout, "retained output\n");
    let mut valid = draft.clone();
    valid.exit = "1".into();
    let attachment = valid.attachment().unwrap();
    assert_eq!(attachment.argv, ["cargo", "test"]);
    assert_eq!(attachment.exit_code, 1);
    assert_eq!(attachment.stderr, "retained error\n");
    valid.command = "cargo test".into();
    assert!(
        valid.attachment().is_err(),
        "no implicit shell parsing or execution"
    );
}

fn ui_walk_fixture() -> (tempfile::TempDir, Store, TaskSession, Project) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("greeting.rs"),
        "// existing user's implementation\n",
    )
    .unwrap();
    let project = Project::open(&root).unwrap();
    let mut store = Store::open(&dir.path().join("state")).unwrap();
    let mut task = TaskPlan::new(
        project.key().into(),
        "Greeting".into(),
        vec!["Greeting works".into()],
        vec!["Implement".into(), "Exercise".into()],
    )
    .unwrap();
    for step in &mut task.steps {
        step.done_when = vec!["Greeting is available".into()];
        step.files = vec!["greeting.rs".into()];
        step.languages = vec!["rust".into()];
    }
    let session = support::approved_session(&mut store, task).unwrap();
    (dir, store, session, project)
}
fn save_ui_walk(store: &mut Store, session: &TaskSession) -> TaskSession {
    let mut walk = support::fixture_walkthrough(session);
    let active = session
        .plan
        .as_ref()
        .unwrap()
        .task
        .active_step
        .as_ref()
        .unwrap();
    walk.step_id = active.clone();
    if let ActionTarget::Validation {
        step_id,
        criterion_refs,
        ..
    } = &mut walk.actions[1].target
    {
        *step_id = Some(active.clone());
        *criterion_refs = vec![CriterionRef::StepDoneWhen {
            step_id: active.clone(),
            index: 0,
        }];
        walk.actions[1].required_evidence = vec![RequiredActionEvidence::CriterionAssessment {
            criterion_ref: criterion_refs[0].clone(),
        }];
    }
    store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            ),
        )
        .unwrap()
}
fn ui_capture(
    dir: &tempfile::TempDir,
    store: &Store,
    session: &TaskSession,
    project: &Project,
) -> (ActionReviewInput, EffectiveGuidance) {
    use tandem_companion::ConversationCapture;
    let guidance = GuidanceSettings::default().effective(
        project.key(),
        "rust",
        session.plan.as_ref().map(|p| &p.task),
    );
    let mut capture = tandem_companion::ProjectConversationCapture {
        project: project.clone(),
        overlays: vec![],
        selected: vec![],
        cache: dir.path().join("capture"),
        check_environment: None,
        recipe_ids: vec![],
        attachments: vec![],
    };
    let input = capture
        .capture(
            session,
            CompanionAction::ReviewAction,
            "rust",
            &guidance,
            &[],
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .review
        .unwrap();
    (
        super::super::companion::authenticate_capture(store, session, input, "rust", &guidance)
            .unwrap(),
        guidance,
    )
}
fn ui_review(
    store: &mut Store,
    session: &TaskSession,
    input: &ActionReviewInput,
    outcome: Option<OperationOutcome>,
) -> (TaskSession, ActionReview, String) {
    let operation_id = new_id();
    let session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::BeginOperation {
                    operation: SessionOperation {
                        id: operation_id.clone(),
                        session_id: session.id.clone(),
                        kind: SessionOperationKind::ReviewAction,
                        started_at_ms: now_ms(),
                    },
                }],
            ),
        )
        .unwrap();
    let assessments = session
        .current_action()
        .unwrap()
        .1
        .required_evidence
        .iter()
        .filter_map(|r| match r {
            RequiredActionEvidence::CriterionAssessment { criterion_ref } => {
                Some(ActionCriterionAssessment {
                    criterion_ref: criterion_ref.clone(),
                    satisfied: true,
                    explanation: "Captured implementation meets the named criterion".into(),
                    evidence_ids: vec![],
                })
            }
            _ => None,
        })
        .collect();
    let review = ActionReview {
        id: new_id(),
        binding: input.binding.clone(),
        outcome: ActionReviewOutcome::Passed,
        summary: "Captured inputs satisfy this action".into(),
        findings: vec![],
        evidence_used: vec![],
        limitations: vec![],
        assessments,
        reviewed_at_ms: now_ms(),
    };
    let mut session = store
        .record_action_review(&session.id, &review, input, Some(&operation_id))
        .unwrap();
    if let Some(outcome) = outcome {
        session = store
            .apply_session_transition(
                &session.id,
                session.sequence,
                &SessionTransition::new(
                    new_id(),
                    vec![SessionCommand::FinishOperation {
                        operation_id: operation_id.clone(),
                        outcome,
                    }],
                ),
            )
            .unwrap();
    }
    let review = store.current_action_review(&session).unwrap().unwrap().0;
    (session, review, operation_id)
}
#[test]
fn start_queues_first_action_next_is_atomic_and_dependency_ready_step_queues_automatically() {
    let (dir, mut store, mut session, project) = ui_walk_fixture();
    assert_eq!(
        walkthrough_continuation(&session),
        WalkthroughContinuation::PrepareAction,
        "the production Start continuation queues a preparation"
    );
    session = save_ui_walk(&mut store, &session);
    let first = session.current_action().unwrap().1.id.clone();
    assert_eq!(
        walkthrough_continuation(&session),
        WalkthroughContinuation::DisplayAction
    );
    assert!(
        current_action_summary(&session)
            .join("\n")
            .contains("Implement the named behavior")
    );
    assert!(
        !current_action_summary(&session)
            .join("\n")
            .contains("Assess the implementation"),
        "future action omitted from ordinary/accessibility rows"
    );
    let (input, guidance) = ui_capture(&dir, &store, &session, &project);
    let (mut session, review, op) = ui_review(&mut store, &session, &input, None);
    let (stored, pending) = store.current_action_review(&session).unwrap().unwrap();
    assert!(!current_review_candidate(
        &session,
        &stored,
        pending.as_ref(),
        "rust",
        &guidance
    ));
    assert!(
        advance_review(&mut store, &session, &review, &input).is_err(),
        "a stored result is not yet a completed operation"
    );
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::FinishOperation {
                    operation_id: op,
                    outcome: OperationOutcome::Completed {
                        summary: "Reviewed".into(),
                    },
                }],
            ),
        )
        .unwrap();
    let (stored, completed) = store.current_action_review(&session).unwrap().unwrap();
    assert!(current_review_candidate(
        &session,
        &stored,
        completed.as_ref(),
        "rust",
        &guidance
    ));
    assert!(!current_review_candidate(
        &session,
        &stored,
        completed.as_ref(),
        "python",
        &guidance
    ));
    let (fresh, _) = ui_capture(&dir, &store, &session, &project);
    let next = advance_review(&mut store, &session, &review, &fresh).unwrap();
    let duplicate = advance_review(&mut store, &session, &review, &fresh).unwrap();
    assert_eq!(next.current_action().unwrap().0.cursor, 1);
    assert_eq!(duplicate.current_action().unwrap().0.cursor, 1);
    assert_ne!(duplicate.current_action().unwrap().1.id, first);
    let (input, _) = ui_capture(&dir, &store, &next, &project);
    let (session, review, _) = ui_review(
        &mut store,
        &next,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Step reviewed".into(),
        }),
    );
    let next = advance_review(&mut store, &session, &review, &input).unwrap();
    assert_eq!(
        next.plan.as_ref().unwrap().task.steps[0].status,
        StepStatus::Complete
    );
    assert_eq!(
        next.plan.as_ref().unwrap().task.active_step,
        Some(next.plan.as_ref().unwrap().task.steps[1].id.clone())
    );
    assert_eq!(
        walkthrough_continuation(&next),
        WalkthroughContinuation::PrepareAction
    );
    let next = save_ui_walk(&mut store, &next);
    assert_eq!(next.current_action().unwrap().0.cursor, 0);
    let id = next.id.clone();
    let action = next.current_action().unwrap().1.id.clone();
    drop(store);
    let store = Store::open(&dir.path().join("state")).unwrap();
    let restored = store.load_session(&id).unwrap().unwrap();
    assert_eq!(restored.current_action().unwrap().1.id, action);
    assert_eq!(
        walkthrough_continuation(&restored),
        WalkthroughContinuation::DisplayAction
    );
    assert!(
        !restored.implementation_complete(),
        "final task validation is still required"
    );
}
#[test]
fn next_recaptures_real_source_and_blocks_old_review_even_without_ui_notifications() {
    let (dir, mut store, session, project) = ui_walk_fixture();
    let session = save_ui_walk(&mut store, &session);
    let (input, _) = ui_capture(&dir, &store, &session, &project);
    let (session, review, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Reviewed".into(),
        }),
    );
    std::fs::write(
        project.root.join("greeting.rs"),
        "// changed externally after the review\n",
    )
    .unwrap();
    let (fresh, _) = ui_capture(&dir, &store, &session, &project);
    assert!(advance_review(&mut store, &session, &review, &fresh).is_err());
    assert_eq!(
        store
            .load_session(&session.id)
            .unwrap()
            .unwrap()
            .current_action()
            .unwrap()
            .0
            .cursor,
        0
    );
}
#[test]
fn cancelled_failed_interrupted_reviews_never_enable_next_and_restart_preserves_action() {
    for outcome in [
        OperationOutcome::Cancelled {
            detail: "Cancelled".into(),
        },
        OperationOutcome::Interrupted {
            detail: "Lost connection".into(),
        },
        OperationOutcome::Failed {
            detail: "Malformed response or unavailable model".into(),
        },
    ] {
        let (dir, mut store, session, project) = ui_walk_fixture();
        let session = save_ui_walk(&mut store, &session);
        let (input, guidance) = ui_capture(&dir, &store, &session, &project);
        let (session, review, _) = ui_review(&mut store, &session, &input, Some(outcome));
        let (stored, outcome) = store.current_action_review(&session).unwrap().unwrap();
        assert!(!current_review_candidate(
            &session,
            &stored,
            outcome.as_ref(),
            "rust",
            &guidance
        ));
        assert!(advance_review(&mut store, &session, &review, &input).is_err());
    }
}

#[test]
fn deliberate_output_is_unvalidated_exact_scoped_immutable_evidence_and_replacement_invalidates_next()
 {
    let (dir, mut store, mut session, project) = ui_walk_fixture();
    let mut walk = support::fixture_walkthrough(&session);
    let command = &mut walk.actions[0];
    command.kind = GuidedActionKind::Command;
    command.target = ActionTarget::Command {
        argv: vec!["cargo".into(), "test".into()],
        cwd: ".".into(),
        expected_exit_codes: vec![0],
    };
    command.required_evidence = vec![RequiredActionEvidence::CommandReceipt];
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            ),
        )
        .unwrap();
    let (empty, guidance) = ui_capture(&dir, &store, &session, &project);
    let draft = OutputAttachmentDraft {
        command: "[\"cargo\",\"test\"]".into(),
        cwd: ".".into(),
        exit: "0".into(),
        stdout: "complete output\n".into(),
        stderr: "".into(),
    };
    let capture = || {
        tandem_companion::capture_action_context(
            &session,
            &project,
            &[],
            &[],
            &dir.path().join("evidence"),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
    };
    let receipt = tandem_companion::attach_action_output(
        empty.scope(),
        &capture(),
        draft.attachment().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        receipt.provenance,
        CommandProvenance::UserAttachedOutput
    ));
    assert_eq!(receipt.status, CheckStatus::Unvalidated);
    assert!(receipt.recipe_id.is_none() && receipt.recipe_fingerprint.is_none());
    let input_with = |receipt: ActionCommandEvidence| {
        let c = capture();
        ActionReviewInput::capture(
            &session,
            c.context,
            c.targets,
            vec![receipt],
            "rust",
            &guidance,
        )
        .unwrap()
    };
    let input = input_with(receipt.clone());
    let action = session.current_action().unwrap().1;
    assert_eq!(input.evidence_gate(action).0, ActionReviewOutcome::Passed);
    assert_eq!(
        empty.evidence_gate(action).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    let mut wrong = receipt.clone();
    wrong.argv = vec!["cargo".into(), "check".into()];
    assert_eq!(
        input_with(wrong).evidence_gate(action).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    let mut wrong = receipt.clone();
    wrong.cwd = "subdir".into();
    assert_eq!(
        input_with(wrong).evidence_gate(action).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    let mut failed = receipt.clone();
    failed.exit_code = Some(1);
    assert_eq!(
        input_with(failed).evidence_gate(action).0,
        ActionReviewOutcome::NeedsChanges
    );
    let mut truncated = receipt.clone();
    truncated.truncated = true;
    assert_eq!(
        input_with(truncated).evidence_gate(action).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    let mut running = receipt.clone();
    running.status = CheckStatus::Running;
    running.completed_at_ms = None;
    assert_eq!(
        input_with(running).evidence_gate(action).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    let replacement = tandem_companion::attach_action_output(
        empty.scope(),
        &capture(),
        draft.attachment().unwrap(),
    )
    .unwrap();
    assert_ne!(receipt.id, replacement.id);
    let replaced_input = input_with(replacement.clone());
    let (session, review, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Attached output assessed".into(),
        }),
    );
    assert!(advance_review(&mut store, &session, &review, &replaced_input).is_err());
    store
        .record_action_attachment(&session.id, &replacement, &input.binding)
        .unwrap();
    assert!(
        advance_review(&mut store, &session, &review, &input).is_err(),
        "a replacement saved after fresh capture is rechecked by the Next transaction"
    );
    let mut late_receipt = receipt.clone();
    late_receipt.id = new_id();
    let late_input = input_with(late_receipt);
    let mut late_review = review.clone();
    late_review.id = new_id();
    late_review.binding = late_input.binding.clone();
    assert!(
        store
            .record_action_review(&session.id, &late_review, &late_input, None)
            .is_err(),
        "a late model result cannot select older output over an explicit replacement"
    );
    assert_eq!(
        store
            .current_action_attachment(&session)
            .unwrap()
            .unwrap()
            .id,
        replacement.id
    );
    std::fs::write(
        project.root.join("greeting.rs"),
        "// new inputs require rerun\n",
    )
    .unwrap();
    let c = tandem_companion::capture_action_context(
        &session,
        &project,
        &[],
        &[],
        &dir.path().join("evidence"),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let stale = ActionReviewInput::capture(
        &session,
        c.context,
        c.targets,
        vec![receipt],
        "rust",
        &guidance,
    )
    .unwrap();
    assert_eq!(
        stale.evidence_gate(session.current_action().unwrap().1).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    assert_eq!(
        draft.stdout, "complete output\n",
        "review failures never consume the attachment draft"
    );
}

#[test]
fn local_check_capture_retains_original_action_and_record_version_for_cumulative_review() {
    let (dir, mut store, session, project) = ui_walk_fixture();
    let session = save_ui_walk(&mut store, &session);
    let (mut input, guidance) = ui_capture(&dir, &store, &session, &project);
    let mut record =
        CheckRecord::pending(project.key().into(), "cargo-check".into(), "recipe".into());
    record.task_id = Some(session.plan.as_ref().unwrap().task.id.clone());
    record.step_id = session.plan.as_ref().unwrap().task.active_step.clone();
    record.plan_revision = Some(session.plan.as_ref().unwrap().revision);
    store.save_check(&record).unwrap();
    assert_eq!(
        store.check_action_scope(&record.id).unwrap(),
        Some(input.scope())
    );
    let unrelated =
        CheckRecord::pending(project.key().into(), "cargo-check".into(), "recipe".into());
    store.save_check(&unrelated).unwrap();
    let checks =
        super::super::companion::action_checks(&store, &session, &[record.clone(), unrelated])
            .unwrap();
    assert_eq!(
        checks.len(),
        1,
        "unbound checks cannot consume the action evidence budget"
    );
    let captured = tandem_companion::capture_action_context(
        &session,
        &project,
        &[],
        &[],
        &dir.path().join("receipt"),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    record.sequence += 1;
    record.command = vec!["/tools/cargo".into(), "check".into()];
    record.snapshot = Some(captured.snapshot);
    record.snapshot_unchanged = Some(true);
    record.status = CheckStatus::Passed;
    record.exit_code = Some(0);
    record.output = "complete local result".into();
    record.completed_at_ms = Some(now_ms());
    store.save_check(&record).unwrap();
    input
        .context
        .validation_environments
        .insert("cargo-check".into(), "recipe".into());
    let scope = input.scope();
    input = ActionReviewInput::capture(
        &session,
        input.context,
        input.targets,
        vec![ActionCommandEvidence::from_local_check(scope, &record).unwrap()],
        "rust",
        &guidance,
    )
    .unwrap();
    let (session, review, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Reviewed".into(),
        }),
    );
    let next = advance_review(&mut store, &session, &review, &input).unwrap();
    let (mut current, _) = ui_capture(&dir, &store, &next, &project);
    current
        .context
        .validation_environments
        .insert("cargo-check".into(), "recipe".into());
    let temporarily_scoped =
        ActionCommandEvidence::from_local_check(current.scope(), &record).unwrap();
    let input = ActionReviewInput::capture(
        &next,
        current.context,
        current.targets,
        vec![temporarily_scoped],
        "rust",
        &guidance,
    )
    .unwrap();
    let authenticated =
        super::super::companion::authenticate_capture(&store, &next, input, "rust", &guidance)
            .unwrap();
    assert_eq!(authenticated.commands[0].scope, review.binding.scope());
    assert_ne!(authenticated.commands[0].scope, authenticated.scope());
    assert!(
        matches!(&authenticated.commands[0].provenance,CommandProvenance::LocalCheck {check_id,check_hash,..} if check_id==&record.id && *check_hash==content_hash(&serde_json::to_vec(&record).unwrap()))
    );
    assert_eq!(
        session.walkthroughs[0].cursor, 0,
        "completed history is stored by Next, not rewritten by later capture"
    );
    assert_eq!(next.walkthroughs[0].cursor, 1);
}

#[test]
fn restart_reuses_completed_review_after_fresh_local_capture_without_replaying_model_work() {
    let (dir, mut store, session, project) = ui_walk_fixture();
    let session = save_ui_walk(&mut store, &session);
    let (input, _) = ui_capture(&dir, &store, &session, &project);
    let (session, _, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Reviewed before restart".into(),
        }),
    );
    let id = session.id.clone();
    drop(store);
    let store = Store::open(&dir.path().join("state")).unwrap();
    let session = store.load_session(&id).unwrap().unwrap();
    let (review, outcome) = store.current_action_review(&session).unwrap().unwrap();
    let (fresh, guidance) = ui_capture(&dir, &store, &session, &project);
    assert!(current_review_candidate(
        &session,
        &review,
        outcome.as_ref(),
        "rust",
        &guidance
    ));
    review_matches_capture(&session, &review, &fresh).unwrap();
    assert_eq!(
        session.current_action().unwrap().0.cursor,
        0,
        "freshness does not consume Next"
    );
    assert!(
        store.model_runs(project.key()).unwrap().is_empty(),
        "only local capture occurred"
    );
    std::fs::write(
        project.root.join("greeting.rs"),
        "// changed between restart and Next\n",
    )
    .unwrap();
    let (fresh, _) = ui_capture(&dir, &store, &session, &project);
    assert!(review_matches_capture(&session, &review, &fresh).is_err());
}

fn ui_final_validation_fixture() -> (tempfile::TempDir, Store, TaskSession, Project) {
    let (dir, mut store, mut session, project) = ui_walk_fixture();
    for _ in 0..2 {
        assert_eq!(
            walkthrough_continuation(&session),
            WalkthroughContinuation::PrepareAction
        );
        session = save_ui_walk(&mut store, &session);
        for _ in 0..2 {
            let (input, _) = ui_capture(&dir, &store, &session, &project);
            let (reviewed, review, _) = ui_review(
                &mut store,
                &session,
                &input,
                Some(OperationOutcome::Completed {
                    summary: "Reviewed current action".into(),
                }),
            );
            session = advance_review(&mut store, &reviewed, &review, &input).unwrap();
        }
    }
    assert!(
        session
            .plan
            .as_ref()
            .unwrap()
            .task
            .steps
            .iter()
            .all(|s| s.status == StepStatus::Complete)
    );
    assert!(!session.implementation_complete());
    assert_eq!(
        walkthrough_continuation(&session),
        WalkthroughContinuation::PrepareAction
    );
    let spec = session.spec.as_ref().unwrap();
    let plan = session.plan.as_ref().unwrap();
    let criteria = spec
        .acceptance
        .iter()
        .map(|c| CriterionRef::SpecAcceptance { id: c.id.clone() })
        .collect::<Vec<_>>();
    let action = GuidedAction {
        id: "final-task-validation".into(),
        revision: 1,
        kind: GuidedActionKind::ValidateTask,
        what: "Assess all approved task acceptance criteria".into(),
        target: ActionTarget::Validation {
            spec_ref: spec.document_ref(),
            plan_ref: plan.document_ref(),
            step_id: None,
            criterion_refs: criteria.clone(),
            paths: vec!["greeting.rs".into()],
        },
        why: "Confirm the completed steps meet the whole approved task".into(),
        expected_result: "Every approved acceptance criterion is satisfied".into(),
        required_evidence: criteria
            .into_iter()
            .map(|criterion_ref| RequiredActionEvidence::CriterionAssessment { criterion_ref })
            .collect(),
    };
    let walk = walkthrough_proposal(&session, vec![action]).unwrap();
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            ),
        )
        .unwrap();
    (dir, store, session, project)
}

#[test]
fn all_steps_queue_final_task_validation_and_only_its_reviewed_next_completes_implementation() {
    let (dir, mut store, session, project) = ui_final_validation_fixture();
    let (input, guidance) = ui_capture(&dir, &store, &session, &project);
    let (session, review, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Final task reviewed".into(),
        }),
    );
    let (_, outcome) = store.current_action_review(&session).unwrap().unwrap();
    for negative in [
        ActionReviewOutcome::NeedsChanges,
        ActionReviewOutcome::InsufficientEvidence,
        ActionReviewOutcome::Cancelled,
        ActionReviewOutcome::Failed,
        ActionReviewOutcome::Interrupted,
    ] {
        let mut negative_review = review.clone();
        negative_review.outcome = negative;
        assert!(!current_review_candidate(
            &session,
            &negative_review,
            outcome.as_ref(),
            "rust",
            &guidance
        ));
    }
    assert!(
        !session.implementation_complete(),
        "passing final review still awaits explicit Next"
    );
    let completed = advance_review(&mut store, &session, &review, &input).unwrap();
    assert_eq!(
        walkthrough_continuation(&completed),
        WalkthroughContinuation::Complete
    );
    assert!(completed.implementation_complete());
    assert_eq!(completed_action_summary(&completed).len(), 5);
    assert!(current_action_summary(&completed).is_empty());
}

#[test]
fn explicit_attachment_survives_restart_before_review_and_replacement_is_immutable_and_scoped() {
    let (dir, mut store, mut session, project) = ui_walk_fixture();
    let mut walk = support::fixture_walkthrough(&session);
    let action = &mut walk.actions[0];
    action.kind = GuidedActionKind::Command;
    action.target = ActionTarget::Command {
        argv: vec!["cargo".into(), "test".into()],
        cwd: ".".into(),
        expected_exit_codes: vec![0],
    };
    action.required_evidence = vec![RequiredActionEvidence::CommandReceipt];
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            ),
        )
        .unwrap();
    let (input, guidance) = ui_capture(&dir, &store, &session, &project);
    let captured = tandem_companion::capture_action_context(
        &session,
        &project,
        &[],
        &[],
        &dir.path().join("explicit-attachment"),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let draft = OutputAttachmentDraft {
        command: "[\"cargo\",\"test\"]".into(),
        cwd: ".".into(),
        exit: "0".into(),
        stdout: "complete first report".into(),
        stderr: "".into(),
    };
    let first = tandem_companion::attach_action_output(
        input.scope(),
        &captured,
        draft.attachment().unwrap(),
    )
    .unwrap();
    let saved = store
        .record_action_attachment(&session.id, &first, &input.binding)
        .unwrap();
    assert_eq!(saved.fingerprint(), first.fingerprint());
    let duplicate = store
        .record_action_attachment(&session.id, &first, &input.binding)
        .unwrap();
    assert_eq!(duplicate.fingerprint(), first.fingerprint());
    let mut changed = first.clone();
    changed.output = CommandOutput::Combined {
        text: "rewritten under the same ID".into(),
    };
    assert!(
        store
            .record_action_attachment(&session.id, &changed, &input.binding)
            .is_err()
    );
    let mut second_draft = draft;
    second_draft.stdout = "complete corrected report".into();
    let second = tandem_companion::attach_action_output(
        input.scope(),
        &captured,
        second_draft.attachment().unwrap(),
    )
    .unwrap();
    store
        .record_action_attachment(&session.id, &second, &input.binding)
        .unwrap();
    let retry_old = store
        .record_action_attachment(&session.id, &first, &input.binding)
        .unwrap();
    assert_eq!(
        retry_old.id, second.id,
        "retrying an old submit never rolls back a replacement"
    );
    let id = session.id.clone();
    let sequence = session.sequence;
    drop(store);
    let mut store = Store::open(&dir.path().join("state")).unwrap();
    let session = store.load_session(&id).unwrap().unwrap();
    let restored = store.current_action_attachment(&session).unwrap().unwrap();
    assert_eq!(restored.fingerprint(), second.fingerprint());
    let restored_form = OutputAttachmentDraft::from_evidence(&restored);
    assert_eq!(restored_form.stdout, "complete corrected report");
    assert_eq!(restored_form.command, "[\"cargo\",\"test\"]");
    assert_eq!(session.sequence, sequence);
    assert_eq!(session.current_action().unwrap().0.cursor, 0);
    assert!(store.current_action_review(&session).unwrap().is_none());
    assert!(store.model_runs(project.key()).unwrap().is_empty());
    assert!(store.active_session_operation(&id).unwrap().is_none());
    let mut wrong_scope = restored.clone();
    wrong_scope.id = new_id();
    wrong_scope.scope.action_id = "another-action".into();
    assert!(
        store
            .record_action_attachment(&id, &wrong_scope, &input.binding)
            .is_err()
    );
    std::fs::write(
        project.root.join("greeting.rs"),
        "// source edited after attachment\n",
    )
    .unwrap();
    let (fresh, _) = ui_capture(&dir, &store, &session, &project);
    assert!(
        store
            .record_action_attachment(&id, &restored, &fresh.binding)
            .is_err(),
        "old output cannot be rebound to a fresh source capture"
    );
    let old_snapshot = restored.snapshot.as_ref().unwrap().id.clone();
    let stale = ActionReviewInput::capture(
        &session,
        fresh.context,
        fresh.targets,
        vec![restored],
        "rust",
        &guidance,
    )
    .unwrap();
    assert_eq!(
        stale.evidence_gate(session.current_action().unwrap().1).0,
        ActionReviewOutcome::InsufficientEvidence
    );
    assert_eq!(
        store
            .current_action_attachment(&session)
            .unwrap()
            .unwrap()
            .snapshot
            .unwrap()
            .id,
        old_snapshot
    );
}

fn ui_pending_check(store: &mut Store, session: &TaskSession) -> CheckRecord {
    let mut record = CheckRecord::pending(
        session.project.clone(),
        "cargo-check".into(),
        "recipe".into(),
    );
    bind_check_record(
        &mut record,
        Some(session),
        session.plan.as_ref().map(|p| &p.task),
    );
    store.save_check(&record).unwrap();
    record
}
fn ui_finish_check(
    dir: &tempfile::TempDir,
    store: &mut Store,
    session: &TaskSession,
    project: &Project,
    record: &mut CheckRecord,
    status: CheckStatus,
) {
    let captured = tandem_companion::capture_action_context(
        session,
        project,
        &[],
        &[],
        &dir.path().join("check-inputs"),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    record.sequence += 1;
    record.command = vec!["/tools/cargo".into(), "check".into()];
    record.snapshot = Some(captured.snapshot);
    record.snapshot_unchanged = Some(true);
    record.status = status.clone();
    record.exit_code = Some(if status == CheckStatus::Passed { 0 } else { 1 });
    record.output = "complete deterministic local result".into();
    record.completed_at_ms = Some(now_ms());
    store.save_check(record).unwrap();
}
fn ui_check_capture(
    dir: &tempfile::TempDir,
    store: &Store,
    session: &TaskSession,
    project: &Project,
) -> ActionReviewInput {
    let (mut input, guidance) = ui_capture(dir, store, session, project);
    let recipes = vec![("cargo-check".into(), "recipe".into())];
    input
        .context
        .validation_environments
        .extend(recipes.clone());
    let checks = super::super::companion::action_checks(
        store,
        session,
        &store.checks(project.key()).unwrap(),
    )
    .unwrap();
    let commands = tandem_companion::collect_action_check_evidence(
        input.scope(),
        &input.context,
        &checks,
        &recipes,
    );
    let input = ActionReviewInput::capture(
        session,
        input.context,
        input.targets,
        commands,
        "rust",
        &guidance,
    )
    .unwrap();
    super::super::companion::authenticate_capture(store, session, input, "rust", &guidance).unwrap()
}

#[test]
fn fresh_check_during_final_validation_has_its_actual_origin_and_enables_reviewed_next() {
    let (dir, mut store, session, project) = ui_final_validation_fixture();
    assert!(session.plan.as_ref().unwrap().task.active_step.is_none());
    let mut walk = session.current_action().unwrap().0.clone();
    walk.id = new_id();
    walk.actions[0]
        .required_evidence
        .push(RequiredActionEvidence::CheckReceipt {
            recipe_id: "cargo-check".into(),
        });
    let session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
            ),
        )
        .unwrap();
    std::fs::write(
        project.root.join("greeting.rs"),
        "// corrected implementation before final acceptance\n",
    )
    .unwrap();
    let mut record = ui_pending_check(&mut store, &session);
    let (empty, _) = ui_capture(&dir, &store, &session, &project);
    assert_eq!(
        store.check_action_scope(&record.id).unwrap(),
        Some(empty.scope()),
        "fresh final check must originate at ValidateTask, whose walkthrough still has a step ID"
    );
    ui_finish_check(
        &dir,
        &mut store,
        &session,
        &project,
        &mut record,
        CheckStatus::Passed,
    );
    let input = ui_check_capture(&dir, &store, &session, &project);
    assert_eq!(input.commands.len(), 1);
    assert_eq!(input.commands[0].scope, input.scope());
    assert_eq!(
        input.evidence_gate(session.current_action().unwrap().1).0,
        ActionReviewOutcome::Passed
    );
    let (session, review, _) = ui_review(
        &mut store,
        &session,
        &input,
        Some(OperationOutcome::Completed {
            summary: "Final acceptance with fresh check".into(),
        }),
    );
    assert_eq!(review.outcome, ActionReviewOutcome::Passed);
    let fresh = ui_check_capture(&dir, &store, &session, &project);
    assert!(
        advance_review(&mut store, &session, &review, &fresh)
            .unwrap()
            .implementation_complete()
    );
}

#[test]
fn next_atomically_rejects_checks_journaled_after_capture_in_either_order() {
    for status in [
        CheckStatus::Preparing,
        CheckStatus::Running,
        CheckStatus::Failed,
    ] {
        for journal_before_capture in [false, true] {
            let (dir, mut store, session, project) = ui_walk_fixture();
            let mut walk = support::fixture_walkthrough(&session);
            walk.actions[0]
                .required_evidence
                .push(RequiredActionEvidence::CheckReceipt {
                    recipe_id: "cargo-check".into(),
                });
            let session = store
                .apply_session_transition(
                    &session.id,
                    session.sequence,
                    &SessionTransition::new(
                        new_id(),
                        vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
                    ),
                )
                .unwrap();
            let mut passed = ui_pending_check(&mut store, &session);
            ui_finish_check(
                &dir,
                &mut store,
                &session,
                &project,
                &mut passed,
                CheckStatus::Passed,
            );
            let input = ui_check_capture(&dir, &store, &session, &project);
            let (session, review, _) = ui_review(
                &mut store,
                &session,
                &input,
                Some(OperationOutcome::Completed {
                    summary: "Initial check passed".into(),
                }),
            );
            assert_eq!(review.outcome, ActionReviewOutcome::Passed);
            let mut queued = journal_before_capture.then(|| ui_pending_check(&mut store, &session));
            let captured = ui_check_capture(&dir, &store, &session, &project);
            let mut pending = queued
                .take()
                .unwrap_or_else(|| ui_pending_check(&mut store, &session));
            if status != CheckStatus::Preparing {
                ui_finish_check(
                    &dir,
                    &mut store,
                    &session,
                    &project,
                    &mut pending,
                    status.clone(),
                );
                if status == CheckStatus::Running {
                    pending.exit_code = None;
                    pending.completed_at_ms = None;
                    pending.sequence += 1;
                    store.save_check(&pending).unwrap();
                }
            }
            let result = advance_review(&mut store, &session, &review, &captured);
            assert!(
                result.is_err(),
                "Next accepted old pass while newer {status:?} receipt was not reviewed (journal_before_capture={journal_before_capture})"
            );
            assert!(
                store
                    .advance_action(
                        &session.id,
                        &review.binding.action_id,
                        &review.id,
                        &review.binding
                    )
                    .is_err(),
                "the atomic Store transition also refuses the stale binding, independent of controller recapture"
            );
            assert_eq!(
                store
                    .load_session(&session.id)
                    .unwrap()
                    .unwrap()
                    .current_action()
                    .unwrap()
                    .0
                    .cursor,
                0
            );
        }
    }
}

#[test]
fn check_dispatch_fences_local_transitions_and_preserves_independent_offline_checks() {
    assert!(check_dispatch_allowed(false, false, false, false));
    // Next/freshness capture blocks every dispatch route until its UI commit.
    assert!(!check_dispatch_allowed(false, false, false, true));
    // A check journaled first owns the check slot before its worker is spawned.
    assert!(!check_dispatch_allowed(false, true, false, false));
    assert!(!check_dispatch_allowed(true, false, false, false));
    assert!(!check_dispatch_allowed(false, false, true, false));
    let (_dir, _store, session) = approved();
    let task = &session.plan.as_ref().unwrap().task;
    let mut legacy = CheckRecord::pending(
        session.project.clone(),
        "recipe".into(),
        "fingerprint".into(),
    );
    bind_check_record(&mut legacy, None, Some(task));
    assert_eq!(legacy.task_id.as_ref(), Some(&task.id));
    assert_eq!(legacy.step_id, task.active_step);
    assert_eq!(legacy.plan_revision, Some(task.revision));
    let unapproved = TaskSession::new(session.project.clone()).unwrap();
    let mut independent = CheckRecord::pending(
        session.project.clone(),
        "recipe".into(),
        "fingerprint".into(),
    );
    bind_check_record(&mut independent, Some(&unapproved), Some(task));
    assert!(
        independent.task_id.is_none()
            && independent.step_id.is_none()
            && independent.plan_revision.is_none(),
        "managed sessions without approval/current action keep independent check authority, never legacy action authority"
    );
}
