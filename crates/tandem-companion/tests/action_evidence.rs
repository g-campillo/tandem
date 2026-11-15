use std::sync::atomic::AtomicBool;
use tandem_companion::*;
use tandem_core::*;

fn session(project: &Project, kind: GuidedActionKind, path: &str) -> TaskSession {
    let mut s = TaskSession::new(project.key().into()).unwrap();
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
    s.save_spec(spec).unwrap();
    let mut task = TaskPlan::new(
        project.key().into(),
        "Greeting".into(),
        vec!["Works".into()],
        vec!["Implement greeting".into()],
    )
    .unwrap();
    task.steps[0].files = vec![path.into(), "interface.rs".into()];
    task.steps[0].done_when = vec!["Greeting works".into()];
    let plan = PlanDocument {
        revision: 1,
        spec_ref: s.spec.as_ref().unwrap().document_ref(),
        task,
    };
    s.save_plan(plan).unwrap();
    s.approve_spec(&s.spec.as_ref().unwrap().document_ref(), 0)
        .unwrap();
    s.approve_plan(&s.plan.as_ref().unwrap().document_ref(), 0)
        .unwrap();
    s.start_implementation().unwrap();
    let plan = s.plan.as_ref().unwrap();
    let spec = s.spec.as_ref().unwrap();
    let step_id = plan.task.steps[0].id.clone();
    let criterion_ref = CriterionRef::StepDoneWhen {
        step_id: step_id.clone(),
        index: 0,
    };
    let action = GuidedAction {
        id: "action".into(),
        revision: 1,
        kind,
        what: "Perform one observable action".into(),
        target: if kind == GuidedActionKind::Command {
            ActionTarget::Command {
                argv: vec!["cargo".into(), "test".into()],
                cwd: ".".into(),
                expected_exit_codes: vec![0],
            }
        } else {
            ActionTarget::File {
                path: path.into(),
                symbol: None,
                change_kind: FileChangeKind::Create,
            }
        },
        why: "Satisfy the approved task".into(),
        expected_result: "The action result is present".into(),
        required_evidence: if kind == GuidedActionKind::Command {
            vec![RequiredActionEvidence::CommandReceipt]
        } else {
            vec![RequiredActionEvidence::FileSnapshot { path: path.into() }]
        },
    };
    let validation = GuidedAction {
        id: "validate".into(),
        revision: 1,
        kind: GuidedActionKind::ValidateStep,
        what: "Assess the greeting".into(),
        target: ActionTarget::Validation {
            spec_ref: spec.document_ref(),
            plan_ref: plan.document_ref(),
            step_id: Some(step_id.clone()),
            criterion_refs: vec![criterion_ref.clone()],
            paths: vec![path.into()],
        },
        why: "Check all conditions".into(),
        expected_result: "The greeting satisfies the criteria".into(),
        required_evidence: vec![RequiredActionEvidence::CriterionAssessment { criterion_ref }],
    };
    s.walkthroughs.push(Walkthrough {
        id: "walk".into(),
        generation: s.execution_generation,
        step_id,
        plan_ref: plan.document_ref(),
        actions: vec![action, validation],
        cursor: 0,
    });
    s
}
fn fixture() -> (tempfile::TempDir, Project) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("interface.rs"), "pub trait Greeter {}\n").unwrap();
    let project = Project::open(&root).unwrap();
    (temp, project)
}
fn capture(
    temp: &tempfile::TempDir,
    s: &TaskSession,
    p: &Project,
    overlays: &[Document],
) -> ActionContext {
    capture_action_context(
        s,
        p,
        overlays,
        &[],
        &temp.path().join("captures"),
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn input(
    s: &TaskSession,
    c: &ActionContext,
    commands: Vec<ActionCommandEvidence>,
) -> ActionReviewInput {
    ActionReviewInput::capture(
        s,
        c.context.clone(),
        c.targets.clone(),
        commands,
        "rust",
        &GuidanceSettings::default().effective(&s.project, "rust", None),
    )
    .unwrap()
}
fn claimed_pass(s: &TaskSession, i: &ActionReviewInput) -> ActionReview {
    parse_action_review_response(s,i,serde_json::json!({"outcome":"Passed","summary":"Behavior is satisfied by this alternative implementation","findings":[],"evidence_used":i.binding.evidence.iter().map(|e|&e.id).collect::<Vec<_>>(),"limitations":[],"assessments":[]})).unwrap()
}
fn attachment(scope: ActionEvidenceScope, c: &ActionContext) -> ActionCommandEvidence {
    attach_action_output(
        scope,
        c,
        UserCommandAttachment {
            argv: vec!["cargo".into(), "test".into()],
            cwd: ".".into(),
            exit_code: 0,
            stdout: "one test passed".into(),
            stderr: "".into(),
            started_at_ms: 0,
            completed_at_ms: 1,
        },
    )
    .unwrap()
}
#[test]
fn capture_distinguishes_absent_empty_draft_ignored_unreadable_and_saved_provenance() {
    let (temp, p) = fixture();
    let s = session(&p, GuidedActionKind::Edit, "greeting.rs");
    let missing = capture(&temp, &s, &p, &[]);
    assert_eq!(missing.targets["greeting.rs"], ActionFileState::Absent);
    assert_eq!(
        claimed_pass(&s, &input(&s, &missing, vec![])).outcome,
        ActionReviewOutcome::NeedsChanges
    );
    let doc = p.open_document("greeting.rs", true).unwrap();
    let draft = capture(&temp, &s, &p, std::slice::from_ref(&doc));
    assert_eq!(
        draft.targets["greeting.rs"],
        ActionFileState::Present {
            hash: content_hash(b"")
        }
    );
    assert!(draft.context.files.iter().any(|f| f.path == "interface.rs"));
    assert_eq!(
        claimed_pass(&s, &input(&s, &draft, vec![])).outcome,
        ActionReviewOutcome::Passed
    );
    p.save(&doc).unwrap();
    let saved = capture(&temp, &s, &p, &[]);
    assert_eq!(
        input(&s, &draft, vec![]).binding,
        input(&s, &saved, vec![]).binding
    );
    std::fs::write(p.root.join("interface.rs"), "pub trait RevisedGreeter {}\n").unwrap();
    let changed_dependency = capture(&temp, &s, &p, &[]);
    assert_ne!(
        input(&s, &saved, vec![]).binding,
        input(&s, &changed_dependency, vec![]).binding
    );
    std::fs::write(p.root.join(".ignore"), "greeting.rs\n").unwrap();
    let ignored = capture(&temp, &s, &p, &[]);
    assert!(matches!(
        ignored.targets["greeting.rs"],
        ActionFileState::Unavailable { .. }
    ));
    assert_eq!(
        claimed_pass(&s, &input(&s, &ignored, vec![])).outcome,
        ActionReviewOutcome::InsufficientEvidence
    );
    std::fs::write(p.root.join(".ignore"), "").unwrap();
    std::fs::write(p.root.join("greeting.rs"), [0_u8, 1, 2]).unwrap();
    let binary = capture(&temp, &s, &p, &[]);
    assert!(matches!(
        binary.targets["greeting.rs"],
        ActionFileState::Unavailable { .. }
    ));
}
#[test]
fn required_command_receipt_must_match_argv_cwd_inputs_completion_and_exit() {
    let (temp, p) = fixture();
    let s = session(&p, GuidedActionKind::Command, "greeting.rs");
    let c = capture(&temp, &s, &p, &[]);
    let empty = input(&s, &c, vec![]);
    assert_eq!(
        claimed_pass(&s, &empty).outcome,
        ActionReviewOutcome::InsufficientEvidence
    );
    let good = attachment(empty.scope(), &c);
    let supplied = input(&s, &c, vec![good.clone()]);
    assert_eq!(
        claimed_pass(&s, &supplied).outcome,
        ActionReviewOutcome::Passed
    );
    assert!(matches!(
        supplied.commands[0].provenance,
        CommandProvenance::UserAttachedOutput
    ));
    assert!(
        serde_json::to_string(&supplied.commands[0])
            .unwrap()
            .contains("UserAttachedOutput")
    );
    for problem in [
        "argv",
        "cwd",
        "snapshot",
        "running",
        "interrupted",
        "truncated",
        "unfinished",
        "future",
        "mutated",
        "exit",
        "failed",
    ] {
        let mut receipt = good.clone();
        match problem {
            "argv" => receipt.argv.push("different-test".into()),
            "cwd" => receipt.cwd = "nested".into(),
            "snapshot" => receipt.snapshot.id.push('x'),
            "running" => receipt.status = CheckStatus::Running,
            "interrupted" => receipt.status = CheckStatus::Interrupted,
            "truncated" => receipt.truncated = true,
            "unfinished" => receipt.completed_at_ms = None,
            "future" => receipt.completed_at_ms = Some(c.context.captured_at_ms + 1),
            "mutated" => receipt.snapshot_unchanged = false,
            "exit" => receipt.exit_code = Some(1),
            _ => receipt.status = CheckStatus::Failed,
        }
        let i = input(&s, &c, vec![receipt]);
        assert_eq!(
            claimed_pass(&s, &i).outcome,
            if problem == "exit" || problem == "failed" {
                ActionReviewOutcome::NeedsChanges
            } else {
                ActionReviewOutcome::InsufficientEvidence
            },
            "{problem}"
        );
    }
    let mut receipt = good;
    receipt.scope.workflow.execution_generation += 1;
    assert!(
        ActionReviewInput::capture(
            &s,
            c.context,
            c.targets,
            vec![receipt],
            "rust",
            &GuidanceSettings::default().effective(&s.project, "rust", None)
        )
        .is_err()
    );
}
#[test]
fn check_collection_keeps_authentication_recipe_and_status_distinct_from_attachments() {
    let (temp, p) = fixture();
    let s = session(&p, GuidedActionKind::Command, "greeting.rs");
    let mut c = capture(&temp, &s, &p, &[]);
    c.context
        .validation_environments
        .insert("rust-test".into(), "recipe-v1".into());
    let empty = input(&s, &c, vec![]);
    let mut check = CheckRecord::pending(p.key().into(), "rust-test".into(), "recipe-v1".into());
    check.task_id = s.plan.as_ref().map(|p| p.task.id.clone());
    check.step_id = Some(empty.binding.step_id.clone());
    check.plan_revision = Some(1);
    check.command = vec!["cargo".into(), "test".into()];
    check.snapshot = Some(c.snapshot.clone());
    check.snapshot_unchanged = Some(true);
    check.output = "complete stdout and stderr".into();
    check.exit_code = Some(0);
    check.status = CheckStatus::Passed;
    check.completed_at_ms = Some(c.context.captured_at_ms);
    check.started_at_ms = 0;
    let recipes = vec![("rust-test".into(), "recipe-v1".into())];
    let evidence =
        collect_action_check_evidence(empty.scope(), &c.context, &[check.clone()], &recipes);
    assert_eq!(evidence.len(), 1);
    assert!(matches!(
        evidence[0].provenance,
        CommandProvenance::LocalCheck { .. }
    ));
    assert!(matches!(evidence[0].output, CommandOutput::Combined { .. }));
    assert_eq!(
        claimed_pass(&s, &input(&s, &c, evidence)).outcome,
        ActionReviewOutcome::Passed
    );
    for problem in ["recipe", "snapshot", "unchanged", "timestamp"] {
        let mut wrong = check.clone();
        match problem {
            "recipe" => wrong.recipe_fingerprint = "changed".into(),
            "snapshot" => wrong.snapshot.as_mut().unwrap().id = "changed".into(),
            "unchanged" => wrong.snapshot_unchanged = Some(false),
            _ => wrong.completed_at_ms = None,
        }
        let unavailable =
            collect_action_check_evidence(empty.scope(), &c.context, &[wrong], &recipes);
        assert_eq!(
            claimed_pass(&s, &input(&s, &c, unavailable)).outcome,
            ActionReviewOutcome::InsufficientEvidence,
            "{problem}"
        );
    }
    check.status = CheckStatus::Failed;
    check.exit_code = Some(1);
    let evidence = collect_action_check_evidence(empty.scope(), &c.context, &[check], &recipes);
    assert_eq!(
        claimed_pass(&s, &input(&s, &c, evidence)).outcome,
        ActionReviewOutcome::NeedsChanges
    );
}
#[test]
fn hidden_walkthrough_metadata_rejects_unknown_code_fields_inline_scripts_and_solution_prose() {
    let (temp, p) = fixture();
    let s = session(&p, GuidedActionKind::Edit, "greeting.rs");
    let c = capture(&temp, &s, &p, &[]);
    let i = input(&s, &c, vec![]);
    let mut metadata = serde_json::to_value(&s.walkthroughs[0]).unwrap();
    metadata["actions"][0]["implementation_script"] = serde_json::json!("write full solution");
    assert!(parse_walkthrough_response(&s, metadata).is_err());
    let mut metadata = serde_json::to_value(&s.walkthroughs[0]).unwrap();
    metadata["actions"][0]["what"] = serde_json::json!("fn greet() { solution() }");
    assert!(parse_walkthrough_response(&s, metadata).is_err());
    assert!(parse_action_review_response(&s,&i,serde_json::json!({"conclusion":"RequirementsSatisfied","summary":"Legacy pass","findings":[],"evidence_used":[],"validation_needed":[],"requested_context":[]})).is_err());
    assert!(parse_action_review_response(&s,&i,serde_json::json!({"outcome":"Passed","summary":"Looks good","findings":[],"evidence_used":["invented receipt"],"limitations":[],"assessments":[]})).is_err());
}

#[test]
fn a_known_required_failure_cannot_disappear_behind_the_legacy_six_receipt_summary_limit() {
    let (temp, p) = fixture();
    let s = session(&p, GuidedActionKind::Command, "greeting.rs");
    let mut c = capture(&temp, &s, &p, &[]);
    c.context
        .validation_environments
        .insert("rust-test".into(), "recipe".into());
    let i = input(&s, &c, vec![]);
    let checks = (0..8)
        .map(|n| {
            let mut check =
                CheckRecord::pending(p.key().into(), "rust-test".into(), "recipe".into());
            check.snapshot = Some(c.snapshot.clone());
            check.command = vec!["cargo".into(), "test".into()];
            check.snapshot_unchanged = Some(true);
            check.started_at_ms = 0;
            check.completed_at_ms = Some(1);
            check.exit_code = Some(if n == 7 { 1 } else { 0 });
            check.status = if n == 7 {
                CheckStatus::Failed
            } else {
                CheckStatus::Passed
            };
            check.output = if n == 7 {
                "x".repeat(70 * 1024)
            } else {
                "passed".into()
            };
            check
        })
        .collect::<Vec<_>>();
    let receipts = collect_action_check_evidence(
        i.scope(),
        &c.context,
        &checks,
        &[("rust-test".into(), "recipe".into())],
    );
    assert_ne!(
        claimed_pass(&s, &input(&s, &c, receipts)).outcome,
        ActionReviewOutcome::Passed,
        "a known failure, including withheld required output, must block a model pass"
    );
    let mut running = checks;
    running[7].status = CheckStatus::Running;
    running[7].exit_code = None;
    running[7].completed_at_ms = None;
    running[7].snapshot_unchanged = None;
    running[7].output.clear();
    let receipts = collect_action_check_evidence(
        i.scope(),
        &c.context,
        &running,
        &[("rust-test".into(), "recipe".into())],
    );
    assert_eq!(
        claimed_pass(&s, &input(&s, &c, receipts)).outcome,
        ActionReviewOutcome::InsufficientEvidence
    );
}
