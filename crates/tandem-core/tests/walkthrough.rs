use tandem_core::*;

fn documents() -> (SpecDocument, PlanDocument) {
    let spec = SpecDocument {
        revision: 1,
        summary: "Build a greeting".into(),
        requirements: vec![Criterion {
            id: "greeting".into(),
            text: "Return a greeting".into(),
        }],
        acceptance: vec![Criterion {
            id: "works".into(),
            text: "The greeting works".into(),
        }],
        in_scope: vec![],
        out_of_scope: vec![],
        constraints: vec![],
    };
    let mut task = TaskPlan::new(
        "p".into(),
        "Greeting".into(),
        vec!["Works".into()],
        vec!["Implement".into(), "Exercise".into()],
    )
    .unwrap();
    task.steps[0].done_when = vec!["The function returns a greeting".into()];
    let plan = PlanDocument {
        revision: 1,
        spec_ref: spec.document_ref(),
        task,
    };
    (spec, plan)
}
fn edit() -> GuidedAction {
    GuidedAction {
        id: "create".into(),
        revision: 1,
        kind: GuidedActionKind::Edit,
        what: "Create the greeting module".into(),
        target: ActionTarget::File {
            path: "greeting.rs".into(),
            symbol: None,
            change_kind: FileChangeKind::Create,
        },
        why: "Give the function a home".into(),
        expected_result: "The module exists".into(),
        required_evidence: vec![RequiredActionEvidence::FileSnapshot {
            path: "greeting.rs".into(),
        }],
    }
}
#[test]
fn atomic_actions_require_their_own_evidence_and_cannot_carry_implementation_scripts() {
    let (spec, plan) = documents();
    let mut action = edit();
    assert!(action.validate(&spec, &plan).is_ok());
    action.required_evidence.clear();
    assert!(
        action.validate(&spec, &plan).is_err(),
        "an edit needs its target snapshot"
    );
    action = edit();
    action.kind = GuidedActionKind::Command;
    action.target = ActionTarget::Command {
        argv: vec![
            "python".into(),
            "-c".into(),
            "open('greeting.rs','w').write('solution')".into(),
        ],
        cwd: ".".into(),
        expected_exit_codes: vec![0],
    };
    action.required_evidence = vec![RequiredActionEvidence::CommandReceipt];
    assert!(
        action.validate(&spec, &plan).is_err(),
        "an inline implementation script is not an atomic human command"
    );
}

#[test]
fn manual_command_metadata_and_existing_recipe_shapes_do_not_grant_execution_authority() {
    let (spec, plan) = documents();
    for argv in [
        vec!["mkdir", "src"],
        vec!["touch", "src/greeting.rs"],
        vec!["cargo", "init", "--lib"],
        vec!["npm", "test"],
        vec!["cc", "-c", "greeting.c"],
        vec!["rg", "-e", "greeting", "src"],
        vec![
            "/usr/bin/python3",
            "-I",
            "-B",
            "-m",
            "pytest",
            "-q",
            "-o",
            "cache_dir={artifacts}/pytest-cache",
        ],
        vec![
            "python3", "-I", "-B", "-m", "unittest", "discover", "-s", ".", "-t", ".", "-v",
        ],
        vec!["python3", "-I", "-B", "/checks/codex-home/benchmark.py"],
        vec![
            "/toolchain/bin/cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--locked",
            "--offline",
            "--message-format=json",
        ],
        vec![
            "/toolchain/bin/cargo",
            "test",
            "--workspace",
            "--locked",
            "--offline",
            "--message-format=json",
        ],
    ] {
        let mut action = command();
        action.target = ActionTarget::Command {
            argv: argv.iter().map(|s| (*s).into()).collect(),
            cwd: "project/subdir".into(),
            expected_exit_codes: vec![0],
        };
        action
            .validate(&spec, &plan)
            .unwrap_or_else(|error| panic!("{argv:?}: {error}"));
    }
    for argv in [
        vec!["sh", "solution.sh"],
        vec!["python3", "-c", "write_solution()"],
        vec!["python3", "-cwrite_solution()"],
        vec!["node", "--eval", "writeSolution()"],
        vec!["node", "-ewriteSolution()"],
        vec!["touch", "file; cargo test"],
        vec!["printf", "fn greet() { solution() }"],
    ] {
        let mut action = command();
        action.target = ActionTarget::Command {
            argv: argv.iter().map(|s| (*s).into()).collect(),
            cwd: ".".into(),
            expected_exit_codes: vec![0],
        };
        assert!(action.validate(&spec, &plan).is_err(), "{argv:?}");
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
fn started(store: &mut Store, two_steps: bool) -> TaskSession {
    let (spec, mut plan) = documents();
    if !two_steps {
        plan.task.steps.truncate(1);
    }
    let mut s = store.create_session("p").unwrap();
    s = apply(store, &s, SessionCommand::SaveSpec { spec });
    s = apply(store, &s, SessionCommand::SavePlan { plan });
    let document_ref = s.spec.as_ref().unwrap().document_ref();
    s = apply(
        store,
        &s,
        SessionCommand::ApproveSpec {
            document_ref,
            approved_at_ms: 1,
        },
    );
    let document_ref = s.plan.as_ref().unwrap().document_ref();
    s = apply(
        store,
        &s,
        SessionCommand::ApprovePlan {
            document_ref,
            approved_at_ms: 2,
        },
    );
    apply(store, &s, SessionCommand::StartImplementation)
}
fn validation(s: &TaskSession, task: bool) -> GuidedAction {
    let spec = s.spec.as_ref().unwrap();
    let plan = s.plan.as_ref().unwrap();
    let step_id = plan
        .task
        .active_step
        .clone()
        .unwrap_or_else(|| plan.task.steps.last().unwrap().id.clone());
    let criteria = if task {
        vec![CriterionRef::SpecAcceptance { id: "works".into() }]
    } else {
        plan.task
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .unwrap()
            .done_when
            .iter()
            .enumerate()
            .map(|(index, _)| CriterionRef::StepDoneWhen {
                step_id: step_id.clone(),
                index,
            })
            .chain(std::iter::once(CriterionRef::SpecAcceptance {
                id: "works".into(),
            }))
            .collect()
    };
    GuidedAction {
        id: if task { "spec" } else { "step" }.into(),
        revision: 1,
        kind: if task {
            GuidedActionKind::ValidateTask
        } else {
            GuidedActionKind::ValidateStep
        },
        what: "Assess cumulative behavior".into(),
        target: ActionTarget::Validation {
            spec_ref: spec.document_ref(),
            plan_ref: plan.document_ref(),
            step_id: if task { None } else { Some(step_id) },
            criterion_refs: criteria.clone(),
            paths: vec!["greeting.rs".into()],
        },
        why: "All conditions must hold together".into(),
        expected_result: "Every criterion is satisfied".into(),
        required_evidence: criteria
            .into_iter()
            .map(|criterion_ref| RequiredActionEvidence::CriterionAssessment { criterion_ref })
            .collect(),
    }
}
fn walkthrough(s: &TaskSession, mut actions: Vec<GuidedAction>) -> Walkthrough {
    actions.push(validation(s, false));
    Walkthrough {
        id: new_id(),
        generation: s.execution_generation,
        step_id: s.plan.as_ref().unwrap().task.active_step.clone().unwrap(),
        plan_ref: s.plan.as_ref().unwrap().document_ref(),
        actions,
        cursor: 0,
    }
}
fn context(version: &str) -> ContextSnapshot {
    ContextSnapshot {
        id: version.into(),
        project_fingerprint: version.into(),
        files: vec![ContextFile {
            path: "greeting.rs".into(),
            language: "rust".into(),
            hash: content_hash(version.as_bytes()),
            text: version.into(),
            unsaved: true,
        }],
        tree: vec!["greeting.rs".into()],
        omitted: vec![],
        captured_at_ms: now_ms(),
        validation_environments: Default::default(),
    }
}
fn input(s: &TaskSession, version: &str) -> ActionReviewInput {
    let context = context(version);
    let targets = std::collections::BTreeMap::from([(
        "greeting.rs".into(),
        ActionFileState::Present {
            hash: context.files[0].hash.clone(),
        },
    )]);
    ActionReviewInput::capture(
        s,
        context,
        targets,
        vec![],
        "rust",
        &GuidanceSettings::default().effective("p", "rust", None),
    )
    .unwrap()
}
fn review(s: &TaskSession, input: &ActionReviewInput) -> ActionReview {
    let (_, action) = s.current_action().unwrap();
    let assessments = action
        .required_evidence
        .iter()
        .filter_map(|e| match e {
            RequiredActionEvidence::CriterionAssessment { criterion_ref } => {
                Some(ActionCriterionAssessment {
                    criterion_ref: criterion_ref.clone(),
                    satisfied: true,
                    explanation: "The observed behavior satisfies this criterion".into(),
                    evidence_ids: input
                        .binding
                        .evidence
                        .iter()
                        .map(|e| e.id.clone())
                        .collect(),
                })
            }
            _ => None,
        })
        .collect();
    let mut r = ActionReview {
        id: new_id(),
        binding: input.binding.clone(),
        outcome: ActionReviewOutcome::Passed,
        summary: "The alternative implementation satisfies the action".into(),
        findings: vec![],
        evidence_used: input
            .binding
            .evidence
            .iter()
            .map(|e| e.id.clone())
            .collect(),
        limitations: vec![],
        assessments,
        reviewed_at_ms: now_ms(),
    };
    r.validate_and_gate(s, input).unwrap();
    r
}
fn consume(store: &mut Store, s: &TaskSession, input: &ActionReviewInput) -> TaskSession {
    let r = review(s, input);
    assert_eq!(r.outcome, ActionReviewOutcome::Passed);
    store.record_action_review(&s.id, &r, input, None).unwrap();
    store
        .advance_action(&s.id, &input.binding.action_id, &r.id, &input.binding)
        .unwrap()
}
#[test]
fn every_action_requires_current_review_and_next_is_atomic_idempotent_history() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    let mut symbol = edit();
    symbol.id = "symbol".into();
    symbol.target = ActionTarget::File {
        path: "greeting.rs".into(),
        symbol: Some("greet".into()),
        change_kind: FileChangeKind::Modify,
    };
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit(), symbol, command()]),
        },
    );
    let first = input(&s, "create-empty-module");
    assert!(
        store
            .advance_action(&s.id, "create", "unbound-legacy-review", &first.binding)
            .is_err()
    );
    assert!(store.save_task(&s.plan.as_ref().unwrap().task).is_err());
    let r = review(&s, &first);
    store.record_action_review(&s.id, &r, &first, None).unwrap();
    let edited = input(&s, "new-draft");
    assert!(
        store
            .advance_action(&s.id, "create", &r.id, &edited.binding)
            .is_err()
    );
    s = store
        .advance_action(&s.id, "create", &r.id, &first.binding)
        .unwrap();
    let sequence = s.sequence;
    s = store
        .advance_action(&s.id, "create", &r.id, &first.binding)
        .unwrap();
    assert_eq!(s.sequence, sequence);
    assert_eq!(s.walkthroughs[0].cursor, 1);
    // Later edits affect the active symbol review, not consumed create history.
    s = consume(&mut store, &s, &input(&s, "alternative-function-body"));
    assert_eq!(
        s.current_action().unwrap().1.kind,
        GuidedActionKind::Command
    );
    let command_input = input(&s, "alternative-function-body");
    let claimed = review(&s, &command_input);
    assert_eq!(claimed.outcome, ActionReviewOutcome::InsufficientEvidence);
    store
        .record_action_review(&s.id, &claimed, &command_input, None)
        .unwrap();
    assert!(
        store
            .advance_action(&s.id, "test", &claimed.id, &command_input.binding)
            .is_err()
    );
    let check = check_record(&s, &command_input);
    store.save_check(&check).unwrap();
    let receipt = ActionCommandEvidence::from_local_check(command_input.scope(), &check).unwrap();
    let command_input = with_commands(&s, &command_input, vec![receipt]);
    s = consume(&mut store, &s, &command_input);
    assert_eq!(
        s.current_action().unwrap().1.kind,
        GuidedActionKind::ValidateStep
    );
    assert!(!s.implementation_complete());
    s = consume(&mut store, &s, &input(&s, "alternative-function-body"));
    assert!(
        s.plan
            .as_ref()
            .unwrap()
            .task
            .steps
            .iter()
            .all(|step| step.status == StepStatus::Complete)
    );
    assert!(
        !s.implementation_complete(),
        "step completion cannot imply full-spec completion"
    );
    let final_walk = Walkthrough {
        id: new_id(),
        generation: s.execution_generation,
        step_id: s.plan.as_ref().unwrap().task.steps[0].id.clone(),
        plan_ref: s.plan.as_ref().unwrap().document_ref(),
        actions: vec![validation(&s, true)],
        cursor: 0,
    };
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: final_walk,
        },
    );
    let final_input = input(&s, "alternative-function-body");
    let mut incomplete = review(&s, &final_input);
    incomplete.assessments.clear();
    store
        .record_action_review(&s.id, &incomplete, &final_input, None)
        .unwrap();
    assert!(
        store
            .advance_action(&s.id, "spec", &incomplete.id, &final_input.binding)
            .is_err()
    );
    s = consume(&mut store, &s, &final_input);
    assert!(s.implementation_complete());
    let session_id = s.id.clone();
    drop(store);
    let store = Store::open(temp.path()).unwrap();
    assert!(
        store
            .load_session(&session_id)
            .unwrap()
            .unwrap()
            .implementation_complete()
    );
    assert_eq!(
        store
            .action_review(&session_id, &r.id)
            .unwrap()
            .unwrap()
            .binding,
        first.binding
    );
}
#[test]
fn bindings_detect_context_language_guidance_evidence_and_definition_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    let original = input(&s, "v1");
    let r = review(&s, &original);
    store
        .record_action_review(&s.id, &r, &original, None)
        .unwrap();
    for field in [
        "context",
        "project",
        "language",
        "guidance",
        "evidence",
        "revision",
        "spec",
        "plan",
        "action",
        "walkthrough",
        "generation",
    ] {
        let mut changed = original.binding.clone();
        match field {
            "context" => changed.context_id.push('x'),
            "project" => changed.project_fingerprint.push('x'),
            "language" => changed.language = "python".into(),
            "guidance" => changed.guidance_fingerprint.push('x'),
            "evidence" => changed.evidence.clear(),
            "revision" => changed.action_revision += 1,
            "spec" => changed.workflow.spec_ref.as_mut().unwrap().hash.push('x'),
            "plan" => changed.workflow.plan_ref.as_mut().unwrap().hash.push('x'),
            "action" => changed.action_id.push('x'),
            "walkthrough" => changed.walkthrough_id.push('x'),
            _ => changed.workflow.execution_generation += 1,
        }
        assert!(
            store
                .advance_action(&s.id, &changed.action_id, &r.id, &changed)
                .is_err(),
            "{field}"
        );
    }
    // Ordinary conversation sequence changes do not make a valid review stale.
    s = store
        .append_user_message(
            &s.id,
            store.load_session(&s.id).unwrap().unwrap().sequence,
            "chat",
            "Why this action?",
        )
        .unwrap();
    assert_eq!(s.current_action().unwrap().1.id, "create");
    s = store
        .advance_action(&s.id, "create", &r.id, &original.binding)
        .unwrap();
    let old_walk = s.walkthroughs[0].clone();
    let mut changed_plan = s.plan.clone().unwrap();
    changed_plan.task.goal.push_str(" carefully");
    s = apply(
        &mut store,
        &s,
        SessionCommand::SavePlan { plan: changed_plan },
    );
    assert!(s.authorize(WorkflowAction::AdvanceAction).is_err());
    assert!(s.walkthroughs.is_empty());
    assert!(s.history.iter().any(|h| {
        h.walkthroughs
            .iter()
            .any(|w| w.id == old_walk.id && w.cursor == 1)
    }));
    assert!(
        store
            .advance_action(&s.id, "create", &r.id, &original.binding)
            .is_ok(),
        "identical historical retry has no new effect"
    );
}
#[test]
fn inactive_replacement_and_cancelled_late_reviews_cannot_change_progress() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, true);
    let mut inactive = walkthrough(&s, vec![edit()]);
    inactive.step_id = s.plan.as_ref().unwrap().task.steps[1].id.clone();
    assert!(
        store
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    new_id(),
                    vec![SessionCommand::SaveWalkthrough {
                        walkthrough: inactive
                    }]
                )
            )
            .is_err()
    );
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    let old = input(&s, "v1");
    let old_review = review(&s, &old);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    assert!(
        store
            .record_action_review(&s.id, &old_review, &old, None)
            .is_err()
    );
    assert_eq!(s.history[0].walkthroughs.len(), 1);
    s = apply(
        &mut store,
        &s,
        SessionCommand::BeginOperation {
            operation: SessionOperation {
                id: "operation".into(),
                session_id: s.id.clone(),
                kind: SessionOperationKind::ReviewAction,
                started_at_ms: now_ms(),
            },
        },
    );
    let current = input(&s, "v1");
    let r = review(&s, &current);
    s = apply(
        &mut store,
        &s,
        SessionCommand::FinishOperation {
            operation_id: "operation".into(),
            outcome: OperationOutcome::Cancelled {
                detail: "Stopped".into(),
            },
        },
    );
    assert!(
        store
            .record_action_review(&s.id, &r, &current, Some("operation"))
            .is_err()
    );
    assert_eq!(
        store.load_session(&s.id).unwrap().unwrap().walkthroughs[0].cursor,
        0
    );
}
#[test]
fn negative_reviews_and_forged_citations_never_authorize_next() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    let i = input(&s, "v1");
    let pass = review(&s, &i);
    store.record_action_review(&s.id, &pass, &i, None).unwrap();
    let mut forged = review(&s, &i);
    forged.evidence_used.push("fictional-test-pass".into());
    assert!(
        store
            .record_action_review(&s.id, &forged, &i, None)
            .is_err()
    );
    let mut negative = review(&s, &i);
    negative.outcome = ActionReviewOutcome::InsufficientEvidence;
    store
        .record_action_review(&s.id, &negative, &i, None)
        .unwrap();
    assert!(
        store
            .advance_action(&s.id, "create", &pass.id, &i.binding)
            .is_err()
    );
    assert!(
        store
            .advance_action(&s.id, "create", &negative.id, &i.binding)
            .is_err()
    );
    assert!(
        !store
            .action_review(&s.id, &negative.id)
            .unwrap()
            .unwrap()
            .limitations
            .is_empty()
    );
}
#[test]
fn failed_next_transaction_rolls_back_and_retry_consumes_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    let i = input(&s, "v1");
    let r = review(&s, &i);
    store.record_action_review(&s.id, &r, &i, None).unwrap();
    let connection = rusqlite::Connection::open(temp.path().join("workspace.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_next BEFORE INSERT ON action_transition BEGIN SELECT RAISE(ABORT,'injected Next failure'); END;").unwrap();
    assert!(
        store
            .advance_action(&s.id, "create", &r.id, &i.binding)
            .unwrap_err()
            .to_string()
            .contains("injected Next failure")
    );
    assert_eq!(
        store.load_session(&s.id).unwrap().unwrap().walkthroughs[0].cursor,
        0
    );
    connection.execute_batch("DROP TRIGGER fail_next").unwrap();
    let next = store
        .advance_action(&s.id, "create", &r.id, &i.binding)
        .unwrap();
    assert_eq!(next.walkthroughs[0].cursor, 1);
    drop(store);
    let mut store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .advance_action(&s.id, "create", &r.id, &i.binding)
            .unwrap()
            .walkthroughs[0]
            .cursor,
        1
    );
}

fn command() -> GuidedAction {
    GuidedAction {
        id: "test".into(),
        revision: 1,
        kind: GuidedActionKind::Command,
        what: "Run the greeting test".into(),
        target: ActionTarget::Command {
            argv: vec!["cargo".into(), "test".into()],
            cwd: ".".into(),
            expected_exit_codes: vec![0],
        },
        why: "Observe the actual test outcome".into(),
        expected_result: "The test exits successfully".into(),
        required_evidence: vec![RequiredActionEvidence::CommandReceipt],
    }
}
fn check_record(s: &TaskSession, i: &ActionReviewInput) -> CheckRecord {
    let mut c = CheckRecord::pending("p".into(), "rust-test".into(), "recipe".into());
    c.task_id = s.plan.as_ref().map(|p| p.task.id.clone());
    c.step_id = Some(i.binding.step_id.clone());
    c.plan_revision = Some(1);
    c.command = vec!["cargo".into(), "test".into()];
    c.snapshot = Some(SnapshotManifest {
        id: i.binding.project_fingerprint.clone(),
        project: "p".into(),
        files: vec![],
        omitted: vec![],
        total_bytes: 0,
        captured_at_ms: now_ms(),
    });
    c.status = CheckStatus::Passed;
    c.exit_code = Some(0);
    c.output = "Actual complete test output".into();
    c.snapshot_unchanged = Some(true);
    c.completed_at_ms = Some(now_ms());
    c
}
fn with_commands(
    s: &TaskSession,
    i: &ActionReviewInput,
    commands: Vec<ActionCommandEvidence>,
) -> ActionReviewInput {
    let mut context = i.context.clone();
    context.captured_at_ms = now_ms();
    context
        .validation_environments
        .insert("rust-test".into(), "recipe".into());
    ActionReviewInput::capture(
        s,
        context,
        i.targets.clone(),
        commands,
        "rust",
        &GuidanceSettings::default().effective("p", "rust", None),
    )
    .unwrap()
}
#[test]
fn persisted_local_checks_cannot_be_forged_and_user_attachments_remain_immutable() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![command()]),
        },
    );
    let i = input(&s, "v1");
    let check = check_record(&s, &i);
    let local = ActionCommandEvidence::from_local_check(i.scope(), &check).unwrap();
    let local_input = with_commands(&s, &i, vec![local.clone()]);
    let r = review(&s, &local_input);
    assert!(
        store
            .record_action_review(&s.id, &r, &local_input, None)
            .is_err(),
        "invented local receipt must not be authenticated"
    );
    store.save_check(&check).unwrap();
    let mut forged = local.clone();
    forged.output = CommandOutput::Combined {
        text: "fake output".into(),
    };
    let forged_input = with_commands(&s, &i, vec![forged]);
    let fake_review = review(&s, &forged_input);
    assert!(
        store
            .record_action_review(&s.id, &fake_review, &forged_input, None)
            .is_err()
    );
    store
        .record_action_review(&s.id, &r, &local_input, None)
        .unwrap();
    let mut attached = local;
    attached.id = "deliberate-user-attachment".into();
    attached.provenance = CommandProvenance::UserAttachedOutput;
    attached.recipe_id = None;
    attached.recipe_fingerprint = None;
    let attached_input = with_commands(&s, &i, vec![attached.clone()]);
    let attached_review = review(&s, &attached_input);
    store
        .record_action_review(&s.id, &attached_review, &attached_input, None)
        .unwrap();
    attached.output = CommandOutput::Split {
        stdout: "corrected attachment".into(),
        stderr: String::new(),
    };
    let changed = with_commands(&s, &i, vec![attached]);
    let changed_review = review(&s, &changed);
    assert!(
        store
            .record_action_review(&s.id, &changed_review, &changed, None)
            .is_err(),
        "attachments replace with new IDs, never mutate old evidence"
    );
    assert!(
        store
            .advance_action(&s.id, "test", &r.id, &local_input.binding)
            .is_err(),
        "new evidence review supersedes old evidence"
    );
    assert_eq!(
        store
            .advance_action(&s.id, "test", &attached_review.id, &attached_input.binding)
            .unwrap()
            .walkthroughs[0]
            .cursor,
        1
    );
    drop(store);
    let store = Store::open(temp.path()).unwrap();
    let persisted = store
        .action_review_input(&s.id, &attached_review.id)
        .unwrap()
        .unwrap();
    assert!(matches!(
        persisted.commands[0].provenance,
        CommandProvenance::UserAttachedOutput
    ));
    assert_eq!(persisted.binding, attached_input.binding);
}

#[test]
fn local_execution_is_bound_at_journaling_and_cannot_be_reused_for_the_next_command() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    let mut second = command();
    second.id = "test-again".into();
    let mut walk = walkthrough(&s, vec![command(), second]);
    walk.actions
        .last_mut()
        .unwrap()
        .required_evidence
        .push(RequiredActionEvidence::CheckReceipt {
            recipe_id: "rust-test".into(),
        });
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough { walkthrough: walk },
    );
    let first = input(&s, "v1");
    let check = check_record(&s, &first);
    store.save_check(&check).unwrap();
    let receipt = ActionCommandEvidence::from_local_check(first.scope(), &check).unwrap();
    let first = with_commands(&s, &first, vec![receipt.clone()]);
    let r = review(&s, &first);
    store.record_action_review(&s.id, &r, &first, None).unwrap();
    // Retrying the same immutable review is harmless.
    store.record_action_review(&s.id, &r, &first, None).unwrap();
    s = store
        .advance_action(&s.id, "test", &r.id, &first.binding)
        .unwrap();
    let sequence = s.sequence;
    s = store
        .advance_action(&s.id, "test", &r.id, &first.binding)
        .unwrap();
    assert_eq!(s.sequence, sequence);
    drop(store);
    let mut store = Store::open(temp.path()).unwrap();
    s = store.load_session(&s.id).unwrap().unwrap();
    let next = input(&s, "v1");
    let rebound = ActionCommandEvidence::from_local_check(next.scope(), &check).unwrap();
    let forged = with_commands(&s, &next, vec![rebound]);
    let claimed = review(&s, &forged);
    assert!(
        store
            .record_action_review(&s.id, &claimed, &forged, None)
            .is_err(),
        "one local execution cannot be relabelled as a second command execution"
    );

    let rerun = check_record(&s, &next);
    store.save_check(&rerun).unwrap();
    let rerun_receipt = ActionCommandEvidence::from_local_check(next.scope(), &rerun).unwrap();
    s = consume(
        &mut store,
        &s,
        &with_commands(&s, &next, vec![rerun_receipt]),
    );
    // Cumulative assessment can cite the first execution under its original scope.
    let cumulative = input(&s, "v1");
    let cumulative = with_commands(&s, &cumulative, vec![receipt]);
    let r = review(&s, &cumulative);
    assert_eq!(r.outcome, ActionReviewOutcome::Passed);
    store
        .record_action_review(&s.id, &r, &cumulative, None)
        .unwrap();
    assert_eq!(
        store
            .action_review_input(&s.id, &r.id)
            .unwrap()
            .unwrap()
            .commands[0]
            .scope,
        first.scope()
    );
    s = store
        .advance_action(
            &s.id,
            &cumulative.binding.action_id,
            &r.id,
            &cumulative.binding,
        )
        .unwrap();
    let mut final_action = validation(&s, true);
    final_action
        .required_evidence
        .push(RequiredActionEvidence::CheckReceipt {
            recipe_id: "rust-test".into(),
        });
    let final_walk = Walkthrough {
        id: new_id(),
        generation: s.execution_generation,
        step_id: s.plan.as_ref().unwrap().task.steps[0].id.clone(),
        plan_ref: s.plan.as_ref().unwrap().document_ref(),
        actions: vec![final_action],
        cursor: 0,
    };
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: final_walk,
        },
    );
    let final_input = input(&s, "v1");
    let final_input = with_commands(&s, &final_input, cumulative.commands);
    s = consume(&mut store, &s, &final_input);
    assert!(
        s.implementation_complete(),
        "full-spec assessment may cite earlier execution without relabelling it"
    );
}

#[test]
fn a_check_journaled_on_an_earlier_action_cannot_first_bind_after_next() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit(), command()]),
        },
    );
    let first = input(&s, "v1");
    let check = check_record(&s, &first);
    store.save_check(&check).unwrap();
    let mut changed_identity = check.clone();
    changed_identity.started_at_ms += 1;
    changed_identity.sequence += 1;
    assert!(
        store.save_check(&changed_identity).is_err(),
        "updating a receipt cannot change the execution identity"
    );
    s = consume(&mut store, &s, &first);
    // Even an update received on the next action must retain the execution origin.
    let mut finished = check;
    finished.sequence += 1;
    store.save_check(&finished).unwrap();
    let next = input(&s, "v1");
    let receipt = ActionCommandEvidence::from_local_check(next.scope(), &finished).unwrap();
    let next = with_commands(&s, &next, vec![receipt]);
    let r = review(&s, &next);
    assert!(
        store.record_action_review(&s.id, &r, &next, None).is_err(),
        "the current model-supplied scope is not the execution origin"
    );
}

#[test]
fn preparing_evidence_keeps_immutable_history_and_recovers_after_a_local_rerun() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![command()]),
        },
    );
    let empty = input(&s, "v1");
    let mut check = check_record(&s, &empty);
    check.snapshot = None;
    check.command.clear();
    check.snapshot_unchanged = None;
    check.status = CheckStatus::Preparing;
    check.exit_code = None;
    check.completed_at_ms = None;
    store.save_check(&check).unwrap();
    let pending = ActionCommandEvidence::from_local_check(empty.scope(), &check).unwrap();
    let pending_input = with_commands(&s, &empty, vec![pending.clone()]);
    let pending_review = review(&s, &pending_input);
    assert_eq!(
        pending_review.outcome,
        ActionReviewOutcome::InsufficientEvidence
    );
    store
        .record_action_review(&s.id, &pending_review, &pending_input, None)
        .unwrap();
    check.finish(CheckStatus::Interrupted, "Interrupted before input capture");
    store.save_check(&check).unwrap();
    let interrupted = ActionCommandEvidence::from_local_check(empty.scope(), &check).unwrap();
    assert_ne!(
        interrupted.id, pending.id,
        "each immutable local receipt version needs its own evidence identity"
    );
    let interrupted_input = with_commands(&s, &empty, vec![interrupted.clone()]);
    let interrupted_review = review(&s, &interrupted_input);
    store
        .record_action_review(&s.id, &interrupted_review, &interrupted_input, None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let rerun = check_record(&s, &empty);
    store.save_check(&rerun).unwrap();
    let rerun = ActionCommandEvidence::from_local_check(empty.scope(), &rerun).unwrap();
    let recovered = with_commands(&s, &empty, vec![interrupted, rerun]);
    s = consume(&mut store, &s, &recovered);
    assert_eq!(s.walkthroughs[0].cursor, 1);
    let historical = store
        .action_review_input(&s.id, &pending_review.id)
        .unwrap()
        .unwrap();
    assert_eq!(historical.commands[0].id, pending.id);
    assert!(historical.commands[0].snapshot.is_none());
    assert_eq!(historical.commands[0].status, CheckStatus::Preparing);
}
#[test]
fn completing_step_selects_only_the_next_dependency_ready_step() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, true);
    let next = s.plan.as_ref().unwrap().task.steps[1].id.clone();
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    s = consume(&mut store, &s, &input(&s, "v1"));
    assert_ne!(
        s.plan.as_ref().unwrap().task.active_step.as_ref(),
        Some(&next)
    );
    s = consume(&mut store, &s, &input(&s, "v1"));
    assert_eq!(
        s.plan.as_ref().unwrap().task.active_step.as_ref(),
        Some(&next)
    );
    assert!(
        s.current_action().is_err(),
        "the next step needs its own walkthrough"
    );
}

#[test]
fn a_review_persisted_before_operation_cancellation_cannot_authorize_next() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let mut s = started(&mut store, false);
    s = apply(
        &mut store,
        &s,
        SessionCommand::SaveWalkthrough {
            walkthrough: walkthrough(&s, vec![edit()]),
        },
    );
    s = apply(
        &mut store,
        &s,
        SessionCommand::BeginOperation {
            operation: SessionOperation {
                id: "review-in-flight".into(),
                session_id: s.id.clone(),
                kind: SessionOperationKind::ReviewAction,
                started_at_ms: now_ms(),
            },
        },
    );
    let i = input(&s, "v1");
    let r = review(&s, &i);
    s = store
        .record_action_review(&s.id, &r, &i, Some("review-in-flight"))
        .unwrap();
    assert!(
        store
            .advance_action(&s.id, "create", &r.id, &i.binding)
            .is_err(),
        "pending operation cannot enable Next"
    );
    s = apply(
        &mut store,
        &s,
        SessionCommand::FinishOperation {
            operation_id: "review-in-flight".into(),
            outcome: OperationOutcome::Cancelled {
                detail: "Cancelled before logical review completion".into(),
            },
        },
    );
    assert!(
        store
            .advance_action(&s.id, "create", &r.id, &i.binding)
            .is_err(),
        "cancelled logical review cannot enable Next"
    );
}
