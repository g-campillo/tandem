use tandem_core::*;

fn spec() -> SpecDocument {
    SpecDocument {
        revision: 1,
        summary: "Implement lookup".into(),
        requirements: vec![Criterion {
            id: "lookup".into(),
            text: "Return matching values".into(),
        }],
        in_scope: vec!["Lookup library".into()],
        out_of_scope: vec!["User interface".into()],
        constraints: vec![],
        acceptance: vec![Criterion {
            id: "empty".into(),
            text: "Empty input returns no match".into(),
        }],
    }
}

fn task() -> TaskPlan {
    let mut task = TaskPlan::new(
        "project".into(),
        "Implement lookup".into(),
        vec!["Empty input".into()],
        vec!["Implement".into(), "Test".into()],
    )
    .unwrap();
    for step in &mut task.steps {
        step.why = "Satisfy the contract".into();
        step.done_when = vec!["The contract is satisfied".into()];
    }
    task
}

fn saved_session() -> TaskSession {
    let mut session = TaskSession::new("project".into()).unwrap();
    session.save_spec(spec()).unwrap();
    let plan = PlanDocument {
        revision: 1,
        spec_ref: session.spec.as_ref().unwrap().document_ref(),
        task: task(),
    };
    session.save_plan(plan).unwrap();
    session
}

fn approve(session: &mut TaskSession) {
    let spec_ref = session.spec.as_ref().unwrap().document_ref();
    let plan_ref = session.plan.as_ref().unwrap().document_ref();
    session.approve_spec(&spec_ref, 10).unwrap();
    session.approve_plan(&plan_ref, 11).unwrap();
}

#[test]
fn all_implementation_operations_require_documents_approvals_and_explicit_start() {
    let operations = [
        WorkflowAction::PrepareWalkthrough,
        WorkflowAction::PrepareAction,
        WorkflowAction::ReviewAction,
        WorkflowAction::Reveal,
        WorkflowAction::Takeover,
        WorkflowAction::AdvanceAction,
        WorkflowAction::ValidateStep,
        WorkflowAction::ValidateTask,
    ];
    let mut session = TaskSession::new("project".into()).unwrap();
    for operation in operations {
        assert_eq!(
            session.authorize(operation).unwrap_err(),
            WorkflowDenial::MissingSpec
        );
    }
    for operation in [
        WorkflowAction::Discuss,
        WorkflowAction::Research,
        WorkflowAction::DraftSpec,
        WorkflowAction::DraftPlan,
    ] {
        assert!(session.authorize(operation).is_ok());
    }
    session.save_spec(spec()).unwrap();
    assert_eq!(
        session.authorize(WorkflowAction::Reveal).unwrap_err(),
        WorkflowDenial::MissingPlan
    );
    session = saved_session();
    let spec_ref = session.spec.as_ref().unwrap().document_ref();
    let plan_ref = session.plan.as_ref().unwrap().document_ref();
    assert_eq!(
        session.authorize(WorkflowAction::Reveal).unwrap_err(),
        WorkflowDenial::SpecNotApproved
    );
    session.approve_plan(&plan_ref, 1).unwrap();
    assert_eq!(
        session.authorize(WorkflowAction::Reveal).unwrap_err(),
        WorkflowDenial::SpecNotApproved
    );
    session.approve_spec(&spec_ref, 2).unwrap();
    for operation in operations {
        assert_eq!(
            session.authorize(operation).unwrap_err(),
            WorkflowDenial::NotStarted
        );
    }
    session.start_implementation().unwrap();
    for operation in operations {
        assert!(session.authorize(operation).is_ok());
    }
    let plan = session.plan.as_ref().unwrap();
    assert_eq!(plan.task.active_step.as_ref(), Some(&plan.task.steps[0].id));
}

#[test]
fn approvals_match_both_revision_and_hash_and_are_independent() {
    let mut session = saved_session();
    let exact = session.spec.as_ref().unwrap().document_ref();
    let mut stale = exact.clone();
    stale.revision += 1;
    assert!(session.approve_spec(&stale, 1).is_err());
    stale = exact.clone();
    stale.hash = "forged".into();
    assert!(session.approve_spec(&stale, 1).is_err());
    session.approve_spec(&exact, 1).unwrap();
    assert_eq!(
        session.authorize(WorkflowAction::Reveal).unwrap_err(),
        WorkflowDenial::PlanNotApproved
    );
    let mut plan_ref = session.plan.as_ref().unwrap().document_ref();
    plan_ref.hash = "forged".into();
    assert!(session.approve_plan(&plan_ref, 2).is_err());
}

#[test]
fn proposals_and_discussion_do_not_save_approve_answer_or_invalidate() {
    let mut session = saved_session();
    approve(&mut session);
    session.start_implementation().unwrap();
    let binding = session.authorize(WorkflowAction::Reveal).unwrap();
    let initial = serde_json::to_value(&session.spec).unwrap();
    let mut proposed = spec();
    proposed.requirements[0].text = "Proposed change".into();
    session.propose_spec(proposed, Some("run".into())).unwrap();
    session.propose_plan(task(), Some("run".into())).unwrap();
    session
        .ask_question(question(), Some("run".into()))
        .unwrap();
    session
        .append_human_message("Approve, start, reveal and choose a".into())
        .unwrap();
    session
        .append_assistant_message("Approval is a separate command".into(), "run".into())
        .unwrap();
    assert_eq!(serde_json::to_value(&session.spec).unwrap(), initial);
    assert!(session.pending_question.is_some());
    assert_eq!(session.authorize(WorkflowAction::Reveal).unwrap(), binding);
}

#[test]
fn no_op_saves_preserve_approvals_generation_progress_and_document_revisions() {
    let mut session = saved_session();
    approve(&mut session);
    session.start_implementation().unwrap();
    let before = serde_json::to_value(&session).unwrap();
    let mut same_spec = session.spec.clone().unwrap();
    same_spec.revision = 500;
    assert!(session.save_spec(same_spec).unwrap().is_none());
    let mut same_plan = session.plan.clone().unwrap();
    same_plan.revision = 700;
    same_plan.task.steps[0].status = StepStatus::Complete;
    same_plan.task.state_sequence += 100;
    assert!(session.save_plan(same_plan).unwrap().is_none());
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
}

#[test]
fn spec_revision_invalidates_both_approvals_and_archives_walkthroughs_and_completed_history() {
    let mut session = saved_session();
    approve(&mut session);
    session.start_implementation().unwrap();
    let previous_binding = session.authorize(WorkflowAction::Reveal).unwrap();
    let walkthrough = walkthrough(&session);
    let walkthrough_id = walkthrough.id.clone();
    session.walkthroughs.push(walkthrough);
    session
        .plan
        .as_mut()
        .unwrap()
        .task
        .complete_active_manually(vec!["old evidence".into()])
        .unwrap();
    let mut changed = spec();
    changed.requirements[0].text = "Revised lookup".into();
    session.save_spec(changed).unwrap();
    assert_eq!(session.spec.as_ref().unwrap().revision, 2);
    assert!(session.spec_approval.is_none() && session.plan_approval.is_none());
    assert!(session.started_generation.is_none());
    assert!(session.walkthroughs.is_empty());
    assert_eq!(
        session.history.last().unwrap().walkthroughs[0].id,
        walkthrough_id
    );
    assert_eq!(
        session
            .history
            .last()
            .unwrap()
            .plan
            .as_ref()
            .unwrap()
            .task
            .steps[0]
            .status,
        StepStatus::Complete
    );
    assert!(session.execution_generation > previous_binding.execution_generation);
    assert_eq!(
        session.authorize(WorkflowAction::Reveal).unwrap_err(),
        WorkflowDenial::OutdatedPlan
    );
    assert!(
        session
            .approve_plan(&session.plan.as_ref().unwrap().document_ref(), 20)
            .is_err()
    );
    assert!(session.save_plan(session.plan.clone().unwrap()).is_err());
}

#[test]
fn changed_plan_keeps_spec_approval_but_requires_plan_approval_and_restart() {
    let mut session = saved_session();
    approve(&mut session);
    session.start_implementation().unwrap();
    let old = session.authorize(WorkflowAction::ReviewAction).unwrap();
    let mut plan = session.plan.clone().unwrap();
    plan.task.steps[0].title = "Revised implementation".into();
    plan.task.steps[0].status = StepStatus::Complete;
    session.save_plan(plan).unwrap();
    assert!(session.spec_approval.is_some() && session.plan_approval.is_none());
    assert!(session.started_generation.is_none());
    assert_eq!(session.plan.as_ref().unwrap().revision, 2);
    assert_eq!(
        session.plan.as_ref().unwrap().task.steps[0].status,
        StepStatus::Pending
    );
    assert!(session.execution_generation > old.execution_generation);
    assert!(session.start_implementation().is_err());
}

fn question() -> ClarificationQuestion {
    ClarificationQuestion {
        id: "storage-choice".into(),
        prompt: "Where should results go?".into(),
        choices: vec![
            QuestionChoice {
                id: "a".into(),
                label: "Memory".into(),
                description: "Temporary results".into(),
            },
            QuestionChoice {
                id: "b".into(),
                label: "Disk".into(),
                description: "Persistent results".into(),
            },
        ],
    }
}

#[test]
fn questions_have_one_pending_identity_and_accept_known_choices_or_custom_text_once() {
    let mut session = TaskSession::new("project".into()).unwrap();
    session.ask_question(question(), None).unwrap();
    assert!(session.ask_question(question(), None).is_err());
    let mut answer = ClarificationAnswer {
        question_id: "wrong".into(),
        choice_id: Some("a".into()),
        custom_text: None,
    };
    assert!(session.answer_question(answer.clone()).is_err());
    answer.question_id = "storage-choice".into();
    answer.choice_id = Some("unknown".into());
    assert!(session.answer_question(answer.clone()).is_err());
    answer.choice_id = None;
    answer.custom_text = Some("   ".into());
    assert!(session.answer_question(answer.clone()).is_err());
    answer.custom_text = Some("Use the operating system cache".into());
    let event = session.answer_question(answer.clone()).unwrap();
    assert!(matches!(
        event.payload,
        ConversationPayload::QuestionAnswered { .. }
    ));
    assert!(session.pending_question.is_none());
    assert!(session.answer_question(answer).is_err());
    assert!(session.ask_question(question(), None).is_err());
    let mut next = question();
    next.id = "next".into();
    session.ask_question(next, None).unwrap();
    session
        .answer_question(ClarificationAnswer {
            question_id: "next".into(),
            choice_id: Some("b".into()),
            custom_text: None,
        })
        .unwrap();
}

#[test]
fn invalid_documents_questions_and_oversized_messages_do_not_mutate_session() {
    let mut session = saved_session();
    let before = serde_json::to_value(&session).unwrap();
    let mut duplicate = spec();
    duplicate.acceptance[0].id = duplicate.requirements[0].id.clone();
    assert!(session.save_spec(duplicate).is_err());
    let mut empty = spec();
    empty.requirements.clear();
    assert!(session.save_spec(empty).is_err());
    let mut plan = session.plan.clone().unwrap();
    let second_id = plan.task.steps[1].id.clone();
    plan.task.steps[0].depends_on.push(second_id);
    assert!(session.save_plan(plan).is_err());
    let mut invalid = question();
    invalid.choices[1].id = "a".into();
    assert!(session.ask_question(invalid, None).is_err());
    assert!(
        session
            .append_human_message("x".repeat(16 * 1024 + 1))
            .is_err()
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
}

#[test]
fn plan_hash_ignores_progress_but_binds_spec_and_task_definition() {
    let session = saved_session();
    let mut plan = session.plan.clone().unwrap();
    let original = plan.document_ref();
    plan.task
        .start_step(&plan.task.steps[0].id.clone())
        .unwrap();
    plan.task
        .complete_active_manually(vec!["receipt".into()])
        .unwrap();
    plan.task.updated_at_ms += 100;
    plan.task.revision += 1;
    assert_eq!(plan.document_ref(), original);
    plan.spec_ref.revision += 1;
    assert_ne!(plan.document_ref().hash, original.hash);
    plan.spec_ref.revision -= 1;
    plan.task.steps[0].done_when.push("Extra criterion".into());
    assert_ne!(plan.document_ref().hash, original.hash);
}

#[test]
fn near_limit_plan_stays_authorized_after_start_and_progress() {
    let mut session = saved_session();
    let mut plan = session.plan.take().unwrap();
    let initial_size = serde_json::to_vec(&plan).unwrap().len();
    plan.task.analysis = "x".repeat(MAX_STRUCTURED_RESPONSE_BYTES - initial_size);
    assert_eq!(
        serde_json::to_vec(&plan).unwrap().len(),
        MAX_STRUCTURED_RESPONSE_BYTES
    );
    session.save_plan(plan).unwrap();
    approve(&mut session);
    let approved_ref = session.plan.as_ref().unwrap().document_ref();

    session.start_implementation().unwrap();
    assert!(
        serde_json::to_vec(session.plan.as_ref().unwrap())
            .unwrap()
            .len()
            > MAX_STRUCTURED_RESPONSE_BYTES
    );
    let binding = session
        .authorize(WorkflowAction::PrepareWalkthrough)
        .unwrap();
    assert_eq!(binding.plan_ref.as_ref(), Some(&approved_ref));

    let plan = session.plan.as_mut().unwrap();
    plan.task
        .complete_active_manually(vec!["e".repeat(MAX_STRUCTURED_RESPONSE_BYTES)])
        .unwrap();
    plan.task
        .start_step(&plan.task.steps[1].id.clone())
        .unwrap();
    assert_eq!(plan.document_ref(), approved_ref);
    assert_eq!(
        session
            .authorize(WorkflowAction::PrepareWalkthrough)
            .unwrap(),
        binding
    );
    assert_eq!(
        session.authorize(WorkflowAction::ReviewAction).unwrap(),
        binding
    );
}

#[test]
fn oversized_plan_definition_is_rejected_without_changing_approved_state() {
    let mut session = saved_session();
    approve(&mut session);
    session.start_implementation().unwrap();
    let before = serde_json::to_value(&session).unwrap();
    let mut plan = session.plan.clone().unwrap();
    plan.task.analysis = "x".repeat(MAX_STRUCTURED_RESPONSE_BYTES);
    assert_eq!(
        session.save_plan(plan).unwrap_err().to_string(),
        "Structured content exceeds 128 KiB"
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
}

#[test]
fn near_limit_spec_is_validated_after_host_revision_normalization() {
    let mut session = saved_session();
    session.spec.as_mut().unwrap().revision = 9;
    session.plan.as_mut().unwrap().spec_ref = session.spec.as_ref().unwrap().document_ref();
    approve(&mut session);
    session.start_implementation().unwrap();
    session.walkthroughs.push(walkthrough(&session));
    session.ask_question(question(), None).unwrap();
    let before = serde_json::to_value(&session).unwrap();

    let mut candidate = spec();
    candidate.revision = 9;
    candidate.constraints = vec!["x".repeat(4096); 31];
    let remaining =
        MAX_STRUCTURED_RESPONSE_BYTES - serde_json::to_vec(&candidate).unwrap().len() - 3;
    candidate.constraints.push("x".repeat(remaining));
    assert_eq!(
        serde_json::to_vec(&candidate).unwrap().len(),
        MAX_STRUCTURED_RESPONSE_BYTES
    );
    candidate.validate().unwrap();

    assert_eq!(
        session
            .propose_spec(candidate.clone(), None)
            .unwrap_err()
            .to_string(),
        "Structured content exceeds 128 KiB"
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);
    assert_eq!(
        session
            .save_spec(candidate.clone())
            .unwrap_err()
            .to_string(),
        "Structured content exceeds 128 KiB"
    );
    assert_eq!(serde_json::to_value(&session).unwrap(), before);

    candidate.constraints.last_mut().unwrap().pop();
    session.save_spec(candidate).unwrap();
    let saved = session.spec.as_ref().unwrap();
    assert_eq!(saved.revision, 10);
    assert_eq!(
        serde_json::to_vec(saved).unwrap().len(),
        MAX_STRUCTURED_RESPONSE_BYTES
    );
    session.approve_spec(&saved.document_ref(), 20).unwrap();
}

#[test]
fn near_limit_plan_remains_valid_after_host_revision_normalization() {
    let mut session = saved_session();
    session.plan.as_mut().unwrap().revision = 9;
    session.plan.as_mut().unwrap().task.revision = 9;
    approve(&mut session);
    session.start_implementation().unwrap();
    let mut candidate = session.plan.clone().unwrap();
    let initial_size = serde_json::to_vec(&candidate).unwrap().len();
    candidate.task.analysis = "x".repeat(MAX_STRUCTURED_RESPONSE_BYTES - initial_size);
    assert_eq!(
        serde_json::to_vec(&candidate).unwrap().len(),
        MAX_STRUCTURED_RESPONSE_BYTES
    );

    let event = session.propose_plan(candidate.task.clone(), None).unwrap();
    let ConversationPayload::DocumentProposed {
        proposal: DocumentProposal::Plan(proposed),
    } = event.payload
    else {
        panic!("Expected a proposed plan");
    };
    assert_eq!(proposed.revision, 10);
    proposed.validate().unwrap();
    session.save_plan(candidate).unwrap();
    let saved = session.plan.as_ref().unwrap();
    assert_eq!(saved.revision, 10);
    assert_eq!(saved.task.revision, 10);
    session.approve_plan(&saved.document_ref(), 20).unwrap();
    session.start_implementation().unwrap();
    session.authorize(WorkflowAction::ReviewAction).unwrap();
}

#[test]
fn exhausted_document_transition_counters_preserve_the_entire_session() {
    for counter in ["session", "document", "generation", "task"] {
        let mut session = saved_session();
        approve(&mut session);
        session.start_implementation().unwrap();
        session.walkthroughs.push(walkthrough(&session));
        match counter {
            "session" => session.sequence = u64::MAX,
            "document" => {
                session.spec.as_mut().unwrap().revision = u64::MAX;
                session.plan.as_mut().unwrap().revision = u64::MAX;
                session.plan.as_mut().unwrap().spec_ref =
                    session.spec.as_ref().unwrap().document_ref();
            }
            "generation" => session.execution_generation = u64::MAX,
            "task" => session.plan.as_mut().unwrap().task.state_sequence = u64::MAX,
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&session).unwrap();
        let mut changed_spec = session.spec.clone().unwrap();
        changed_spec.summary.push_str(" revised");
        let mut changed_plan = session.plan.clone().unwrap();
        changed_plan.task.goal.push_str(" revised");

        if counter != "task" {
            let expected = match counter {
                "session" => "Session sequence exhausted",
                "document" => "Spec revision exhausted",
                "generation" => "Execution generation exhausted",
                _ => unreachable!(),
            };
            assert_eq!(
                session
                    .save_spec(changed_spec.clone())
                    .unwrap_err()
                    .to_string(),
                expected,
                "{counter}"
            );
            assert_eq!(serde_json::to_value(&session).unwrap(), before, "{counter}");
        }
        let expected = match counter {
            "session" => "Session sequence exhausted",
            "document" => "Plan revision exhausted",
            "generation" => "Execution generation exhausted",
            "task" => "Task sequence exhausted",
            _ => unreachable!(),
        };
        assert_eq!(
            session
                .save_plan(changed_plan.clone())
                .unwrap_err()
                .to_string(),
            expected,
            "{counter}"
        );
        assert_eq!(serde_json::to_value(&session).unwrap(), before, "{counter}");
        if matches!(counter, "session" | "document") {
            assert_eq!(
                session
                    .propose_spec(changed_spec, None)
                    .unwrap_err()
                    .to_string(),
                if counter == "session" {
                    "Session sequence exhausted"
                } else {
                    "Spec revision exhausted"
                },
                "{counter}"
            );
            assert_eq!(serde_json::to_value(&session).unwrap(), before, "{counter}");
            assert_eq!(
                session
                    .propose_plan(changed_plan.task, None)
                    .unwrap_err()
                    .to_string(),
                expected,
                "{counter}"
            );
            assert_eq!(serde_json::to_value(&session).unwrap(), before, "{counter}");
        }
    }
}

#[test]
fn legacy_task_json_still_deserializes_without_new_workflow_fields() {
    let task: TaskPlan = serde_json::from_str(r#"{"id":"legacy","project":"project","goal":"Old task","acceptance":[],"constraints":[],"unknowns":[],"steps":[{"id":"step","title":"Implement","why":"","files":[],"depends_on":[],"done_when":[],"status":"Complete","evidence":["manually recorded"],"completion_note":"Historical completion"}],"revision":1,"active_step":null,"updated_at_ms":0}"#).unwrap();
    task.validate().unwrap();
    assert_eq!(task.steps[0].status, StepStatus::Complete);
    assert_eq!(task.state_sequence, 0);
}

fn walkthrough(session: &TaskSession) -> Walkthrough {
    let plan = session.plan.as_ref().unwrap();
    let step_id = plan.task.steps[0].id.clone();
    Walkthrough {
        id: "walkthrough".into(),
        generation: session.execution_generation,
        step_id: step_id.clone(),
        plan_ref: plan.document_ref(),
        actions: vec![GuidedAction {
            id: "validate".into(),
            revision: 1,
            kind: GuidedActionKind::ValidateStep,
            what: "Assess the step".into(),
            target: ActionTarget::Validation {
                spec_ref: plan.spec_ref.clone(),
                plan_ref: plan.document_ref(),
                step_id: Some(step_id.clone()),
                criterion_refs: vec![CriterionRef::StepDoneWhen {
                    step_id: step_id.clone(),
                    index: 0,
                }],
                paths: vec![],
            },
            why: "Confirm the completion conditions".into(),
            expected_result: "A cumulative assessment".into(),
            required_evidence: vec![RequiredActionEvidence::CriterionAssessment {
                criterion_ref: CriterionRef::StepDoneWhen { step_id, index: 0 },
            }],
        }],
        cursor: 0,
    }
}

fn walkthrough_with_serialized_size(session: &TaskSession, size: usize) -> Walkthrough {
    let mut walkthrough = walkthrough(session);
    let actions = (0..11).map(|index| GuidedAction {
        id: format!("edit-{index}"),
        revision: 1,
        kind: GuidedActionKind::Edit,
        what: "Implement one symbol".into(),
        target: ActionTarget::File {
            path: "src/lib.rs".into(),
            symbol: Some(format!("symbol_{index}")),
            change_kind: FileChangeKind::Modify,
        },
        why: "Satisfy one requirement".into(),
        expected_result: "One observable edit".into(),
        required_evidence: vec![RequiredActionEvidence::FileSnapshot {
            path: "src/lib.rs".into(),
        }],
    });
    walkthrough.actions.splice(0..0, actions);
    let mut remaining = size - serde_json::to_vec(&walkthrough).unwrap().len();
    for action in &mut walkthrough.actions {
        for text in [
            &mut action.what,
            &mut action.why,
            &mut action.expected_result,
        ] {
            let added = remaining.min(4096 - text.len());
            text.push_str(&"x".repeat(added));
            remaining -= added;
        }
    }
    assert_eq!(
        remaining, 0,
        "Fixture needs more action description capacity"
    );
    assert_eq!(serde_json::to_vec(&walkthrough).unwrap().len(), size);
    walkthrough
}

#[test]
fn near_limit_walkthrough_remains_valid_when_cursor_gains_a_digit() {
    let session = saved_session();
    let spec = session.spec.as_ref().unwrap();
    let plan = session.plan.as_ref().unwrap();
    let mut walkthrough = walkthrough_with_serialized_size(&session, MAX_STRUCTURED_RESPONSE_BYTES);
    assert_eq!(walkthrough.actions.len(), 12);
    walkthrough.validate(spec, plan).unwrap();

    for cursor in [10, walkthrough.actions.len()] {
        walkthrough.cursor = cursor;
        assert_eq!(
            serde_json::to_vec(&walkthrough).unwrap().len(),
            MAX_STRUCTURED_RESPONSE_BYTES + 1
        );
        walkthrough.validate(spec, plan).unwrap();
    }
}

#[test]
fn oversized_walkthrough_definition_is_rejected_regardless_of_cursor() {
    let session = saved_session();
    let mut walkthrough =
        walkthrough_with_serialized_size(&session, MAX_STRUCTURED_RESPONSE_BYTES + 1024);
    for cursor in [0, 10, walkthrough.actions.len()] {
        walkthrough.cursor = cursor;
        assert_eq!(
            walkthrough
                .validate(
                    session.spec.as_ref().unwrap(),
                    session.plan.as_ref().unwrap()
                )
                .unwrap_err()
                .to_string(),
            "Structured content exceeds 128 KiB"
        );
    }
}

#[test]
fn walkthrough_validation_binds_exact_criteria_and_requires_final_step_assessment() {
    let session = saved_session();
    let mut walkthrough = walkthrough(&session);
    walkthrough
        .validate(
            session.spec.as_ref().unwrap(),
            session.plan.as_ref().unwrap(),
        )
        .unwrap();
    if let ActionTarget::Validation { criterion_refs, .. } = &mut walkthrough.actions[0].target {
        criterion_refs.push(CriterionRef::SpecAcceptance { id: "empty".into() });
    }
    walkthrough.actions[0]
        .required_evidence
        .push(RequiredActionEvidence::CriterionAssessment {
            criterion_ref: CriterionRef::SpecAcceptance { id: "empty".into() },
        });
    walkthrough
        .validate(
            session.spec.as_ref().unwrap(),
            session.plan.as_ref().unwrap(),
        )
        .unwrap();
    if let ActionTarget::Validation { criterion_refs, .. } = &mut walkthrough.actions[0].target {
        *criterion_refs = vec![CriterionRef::StepDoneWhen {
            step_id: walkthrough.step_id.clone(),
            index: 99,
        }];
    }
    assert!(
        walkthrough
            .validate(
                session.spec.as_ref().unwrap(),
                session.plan.as_ref().unwrap()
            )
            .is_err()
    );
    walkthrough = self::walkthrough(&session);
    walkthrough.actions[0].kind = GuidedActionKind::Edit;
    assert!(
        walkthrough
            .validate(
                session.spec.as_ref().unwrap(),
                session.plan.as_ref().unwrap()
            )
            .is_err()
    );
    walkthrough = self::walkthrough(&session);
    walkthrough.plan_ref.hash = "old".into();
    assert!(
        walkthrough
            .validate(
                session.spec.as_ref().unwrap(),
                session.plan.as_ref().unwrap()
            )
            .is_err()
    );
}

fn receipt(content: String) -> ResearchReceipt {
    ResearchReceipt::new(
        ResearchRequest::new(ResearchRequestKind::ReadFileRange {
            path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 5,
        })
        .unwrap(),
        vec![ResearchSource {
            id: "source".into(),
            locator: SourceLocator::FileRange {
                path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 5,
            },
            source_hash: content_hash(b"source bytes"),
            captured_at_ms: 12,
            provenance: ResearchProvenance::ProjectFile,
        }],
        ResearchReceiptStatus::Complete,
        content,
        vec![],
    )
    .unwrap()
}

#[test]
fn research_citations_need_real_receipt_source_and_exact_locator() {
    let receipt = receipt("Lookup is defined here".into());
    let mut finding = ResearchFinding {
        summary: "The lookup implementation is local".into(),
        citations: vec![ResearchCitation {
            receipt_id: receipt.id.clone(),
            source_id: "source".into(),
            locator: receipt.sources[0].locator.clone(),
        }],
    };
    finding.validate(std::slice::from_ref(&receipt)).unwrap();
    finding.citations[0].receipt_id = "model-invented".into();
    assert!(finding.validate(std::slice::from_ref(&receipt)).is_err());
    finding.citations[0].receipt_id = receipt.id.clone();
    finding.citations[0].source_id = "invented-source".into();
    assert!(finding.validate(std::slice::from_ref(&receipt)).is_err());
    finding.citations[0].source_id = "source".into();
    finding.citations[0].locator = SourceLocator::FileRange {
        path: "other.rs".into(),
        start_line: 1,
        end_line: 5,
    };
    assert!(finding.validate(&[receipt]).is_err());
}

#[test]
fn research_receipts_preserve_content_integrity_and_disclose_omissions() {
    let mut valid = receipt("source content".into());
    let serialized = serde_json::to_string(&valid).unwrap();
    let reopened: ResearchReceipt = serde_json::from_str(&serialized).unwrap();
    assert_eq!(reopened, valid);
    valid.content.push('!');
    assert!(valid.validate().is_err());
    valid = reopened.clone();
    valid.omissions.push(ResearchOmission {
        locator: None,
        reason: "Another file exceeded the byte limit".into(),
    });
    assert!(valid.validate().is_err());
    valid.status = ResearchReceiptStatus::Partial;
    valid.validate().unwrap();
    let request = reopened.request;
    assert!(
        ResearchReceipt::new(
            request,
            vec![],
            ResearchReceiptStatus::Complete,
            "x".repeat(MAX_RESEARCH_RECEIPT_BYTES + 1),
            vec![]
        )
        .is_err()
    );
}

#[test]
fn research_budgets_enforce_round_request_receipt_and_total_text_limits() {
    let mut budget = ResearchBudget::default();
    assert!(budget.begin_round(5).is_err());
    for _ in 0..8 {
        budget.begin_round(4).unwrap();
    }
    let exhausted = budget.clone();
    assert!(budget.begin_round(1).is_err());
    assert_eq!(budget, exhausted);
    let large = receipt("x".repeat(MAX_RESEARCH_RECEIPT_BYTES));
    for _ in 0..4 {
        budget.record_receipt(&large).unwrap();
    }
    let exhausted = budget.clone();
    assert!(
        budget
            .record_receipt(&receipt("one more byte".into()))
            .is_err()
    );
    assert_eq!(budget, exhausted);
    let mut budget = ResearchBudget::default();
    for _ in 0..8 {
        budget.begin_round(4).unwrap();
        for _ in 0..4 {
            budget.record_receipt(&receipt(String::new())).unwrap();
        }
    }
    assert!(budget.record_receipt(&receipt(String::new())).is_err());
}

#[test]
fn final_task_assessment_covers_all_spec_acceptance_without_edit_or_command_target() {
    let session = saved_session();
    let spec = session.spec.as_ref().unwrap();
    let plan = session.plan.as_ref().unwrap();
    let mut action = walkthrough(&session).actions.remove(0);
    action.kind = GuidedActionKind::ValidateTask;
    action.target = ActionTarget::Validation {
        spec_ref: spec.document_ref(),
        plan_ref: plan.document_ref(),
        step_id: None,
        criterion_refs: vec![CriterionRef::SpecAcceptance { id: "empty".into() }],
        paths: vec!["src/lib.rs".into()],
    };
    action.required_evidence = vec![RequiredActionEvidence::CriterionAssessment {
        criterion_ref: CriterionRef::SpecAcceptance { id: "empty".into() },
    }];
    action.validate(spec, plan).unwrap();
    let mut expanded = spec.clone();
    expanded.acceptance.push(Criterion {
        id: "duplicates".into(),
        text: "Preserve duplicate values".into(),
    });
    let mut expanded_plan = plan.clone();
    expanded_plan.spec_ref = expanded.document_ref();
    if let ActionTarget::Validation {
        spec_ref, plan_ref, ..
    } = &mut action.target
    {
        *spec_ref = expanded.document_ref();
        *plan_ref = expanded_plan.document_ref();
    }
    assert!(action.validate(&expanded, &expanded_plan).is_err());
    let json = serde_json::to_value(&action).unwrap();
    let mut with_script = json;
    with_script["target"]["script"] = "echo hidden command".into();
    assert!(serde_json::from_value::<GuidedAction>(with_script).is_err());
}

#[test]
fn initial_plan_save_does_not_replace_an_approved_definition_or_accept_model_progress_sequence() {
    let mut session = TaskSession::new("project".into()).unwrap();
    session.save_spec(spec()).unwrap();
    session
        .approve_spec(&session.spec.as_ref().unwrap().document_ref(), 1)
        .unwrap();
    let generation = session.execution_generation;
    let mut task = task();
    task.state_sequence = u64::MAX;
    session
        .save_plan(PlanDocument {
            revision: 1,
            spec_ref: session.spec.as_ref().unwrap().document_ref(),
            task,
        })
        .unwrap();
    assert_eq!(session.execution_generation, generation);
    assert_eq!(session.plan.as_ref().unwrap().task.state_sequence, 0);
    approve(&mut session);
    session.start_implementation().unwrap();
}

#[test]
fn generated_plans_are_bounded_to_32_steps_and_saved_domain_plans_to_64() {
    let mut session = TaskSession::new("project".into()).unwrap();
    session.save_spec(spec()).unwrap();
    let task = TaskPlan::new(
        "project".into(),
        "Many steps".into(),
        vec![],
        (0..33).map(|n| format!("Step {n}")).collect(),
    )
    .unwrap();
    assert!(session.propose_plan(task.clone(), None).is_err());
    session
        .save_plan(PlanDocument {
            revision: 1,
            spec_ref: session.spec.as_ref().unwrap().document_ref(),
            task,
        })
        .unwrap();
    assert_eq!(session.plan.as_ref().unwrap().task.steps.len(), 33);
}
