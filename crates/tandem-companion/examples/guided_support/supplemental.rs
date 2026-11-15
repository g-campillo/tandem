//! Independent harness test input, never a live-project edit or model tool.
use super::*;

pub const PYTHON_DIRECT_PATH: &str = "test_tandem_direct_evidence.py";
pub const PYTHON_DIRECT_TESTS: &str = r#"import unittest
from limits import bounded_count

class TandemIndependentDirectEvidence(unittest.TestCase):
    def test_supported_integer_boundaries(self):
        cases = [(0, 0, 0), (0, 1000000, 0), (1000000, 0, 0),
                 (7, 9, 7), (9, 7, 7), (9, 9, 9),
                 (1000000, 1000000, 1000000),
                 (999999, 1000000, 999999), (1000000, 999999, 999999)]
        for value, ceiling, expected in cases:
            with self.subTest(value=value, ceiling=ceiling):
                actual = bounded_count(value, ceiling)
                self.assertIs(type(actual), int)
                self.assertEqual(actual, expected)
"#;

pub fn check_input_overlays(
    task: &FixtureTask,
    mut overlays: Vec<Document>,
) -> Result<Vec<Document>> {
    task.verify_originals()?;
    if task.language != "python" {
        return Ok(overlays);
    }
    ensure!(
        matches!(fs::symlink_metadata(task.project.root.join(PYTHON_DIRECT_PATH)), Err(error) if error.kind()==std::io::ErrorKind::NotFound),
        "Supplemental oracle must never exist in the live fixture"
    );
    let existing = overlays
        .iter()
        .filter(|d| d.path == PYTHON_DIRECT_PATH)
        .collect::<Vec<_>>();
    ensure!(
        existing.len() <= 1 && existing.iter().all(|d| d.text == PYTHON_DIRECT_TESTS),
        "Supplemental check-input oracle changed"
    );
    if existing.is_empty() {
        let mut document = task.project.open_document(PYTHON_DIRECT_PATH, true)?;
        document.snapshot(PYTHON_DIRECT_TESTS.into());
        overlays.push(document);
    }
    Ok(overlays)
}

fn review_input(
    task: &FixtureTask,
    store: &Store,
    session: &TaskSession,
    environment: &tandem_checks::CheckEnvironment,
    overlays: &[Document],
    record: &CheckRecord,
    cache: &Path,
) -> Result<ActionReviewInput> {
    let captured = capture_action_context(
        session,
        &task.project,
        overlays,
        &[],
        cache,
        &AtomicBool::new(false),
    )?;
    let mut context = captured.context;
    let recipes = bind_validation_environment(
        &mut context,
        Some(environment),
        std::slice::from_ref(&task.recipe),
    );
    let (walk, action) = session.current_action()?;
    let scope = ActionEvidenceScope {
        workflow: session.authorize(WorkflowAction::ReviewAction)?,
        walkthrough_id: walk.id.clone(),
        action_id: action.id.clone(),
        action_hash: action.definition_hash(),
    };
    let commands =
        collect_action_check_evidence(scope, &context, std::slice::from_ref(record), &recipes);
    authenticate_review(
        store,
        session,
        ActionReviewInput::capture(
            session,
            context,
            captured.targets,
            commands,
            "python",
            &task.guidance(),
        )?,
        "python",
        &task.guidance(),
    )
}

/// Explicitly fake workflow setup with real isolated local checks. This is a
/// validation-kernel proof, never progress in either real subscription journey.
pub fn verify(root: &Path) -> Result<()> {
    use tandem_checks::*;
    let ledger_before = fs::read(root.join("turn-budget.json"))?;
    let directory = root.join(format!("supplemental-kernel-proof-{}", new_id()));
    fs::create_dir(&directory)?;
    let task = FixtureTask::create(&directory.join("project"), "python")?;
    let environment = CheckEnvironment::discover(&task.project.root, directory.join("checks"))?;
    let recipe = environment.recipe(&task.recipe)?;
    let mut store = Store::open(&directory.join("simulated-workflow-state"))?;
    let mut plan = TaskPlan::new(
        task.project.key().into(),
        "Verify independent direct evidence".into(),
        vec!["Both argument positions include the upper boundary and integer results".into()],
        vec!["Run fixed isolated checks".into()],
    )?;
    plan.steps[0].files = vec![task.path.clone()];
    plan.steps[0].languages = vec!["python".into()];
    plan.steps[0].done_when = plan.acceptance.clone();
    let spec = SpecDocument {
        revision: 1,
        summary: plan.goal.clone(),
        requirements: vec![Criterion {
            id: "domain".into(),
            text: plan.goal.clone(),
        }],
        in_scope: vec![task.path.clone()],
        out_of_scope: vec![],
        constraints: vec![],
        acceptance: vec![Criterion {
            id: "boundary".into(),
            text: plan.acceptance[0].clone(),
        }],
    };
    let mut session = store.create_session(task.project.key())?;
    session = store.apply_session_transition(
        &session.id,
        session.sequence,
        &SessionTransition::new(
            new_id(),
            vec![
                SessionCommand::SaveSpec { spec: spec.clone() },
                SessionCommand::SavePlan {
                    plan: PlanDocument {
                        revision: 1,
                        spec_ref: spec.document_ref(),
                        task: plan,
                    },
                },
            ],
        ),
    )?;
    session = store.apply_session_transition(
        &session.id,
        session.sequence,
        &SessionTransition::new(
            new_id(),
            vec![
                SessionCommand::ApproveSpec {
                    document_ref: session.spec.as_ref().unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                },
                SessionCommand::ApprovePlan {
                    document_ref: session.plan.as_ref().unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                },
                SessionCommand::StartImplementation,
            ],
        ),
    )?;
    let command = GuidedAction {
        id: "isolated-command".into(),
        revision: 1,
        kind: GuidedActionKind::Command,
        what: "Run fixed isolated unittest with an independent check-input overlay".into(),
        target: ActionTarget::Command {
            argv: recipe.argv.clone(),
            cwd: ".".into(),
            expected_exit_codes: vec![0],
        },
        why: "Exercise independent boundary coverage".into(),
        expected_result: "All direct checks pass on unchanged captured inputs".into(),
        required_evidence: vec![
            RequiredActionEvidence::CommandReceipt,
            RequiredActionEvidence::CheckReceipt {
                recipe_id: task.recipe.clone(),
            },
        ],
    };
    let p = session.plan.as_ref().unwrap();
    let criterion = CriterionRef::StepDoneWhen {
        step_id: p.task.steps[0].id.clone(),
        index: 0,
    };
    let validation = GuidedAction {
        id: "validation".into(),
        revision: 1,
        kind: GuidedActionKind::ValidateStep,
        what: "Assess cumulative direct evidence".into(),
        target: ActionTarget::Validation {
            spec_ref: p.spec_ref.clone(),
            plan_ref: p.document_ref(),
            step_id: Some(p.task.steps[0].id.clone()),
            criterion_refs: vec![criterion.clone()],
            paths: vec![task.path.clone()],
        },
        why: "Keep cumulative criterion explicit".into(),
        expected_result: "Matching evidence demonstrates the boundary criterion".into(),
        required_evidence: vec![RequiredActionEvidence::CriterionAssessment {
            criterion_ref: criterion,
        }],
    };
    let walk = walkthrough_proposal(&session, vec![command.clone(), validation])?;
    session = store.apply_session_transition(
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
    let wrong_second_argument = "def bounded_count(value: int, ceiling: int) -> int:\n    if ceiling == 1000000:\n        return ceiling\n    return value if value < ceiling else ceiling\n";
    let mut results = Vec::new();
    let mut good = None;
    for (label, code, supplemental, expected) in [
        (
            "correct-alternative",
            task.alternative.as_str(),
            true,
            CheckStatus::Passed,
        ),
        (
            "wrong-original-seven-cases",
            wrong_second_argument,
            false,
            CheckStatus::Passed,
        ),
        (
            "wrong-supplemental-boundary",
            wrong_second_argument,
            true,
            CheckStatus::Failed,
        ),
    ] {
        let mut module = task.project.open_document(&task.path, true)?;
        module.snapshot(code.into());
        let overlays = if supplemental {
            check_input_overlays(&task, vec![module])?
        } else {
            vec![module]
        };
        let mut pending = CheckRecord::pending(
            task.project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        );
        pending.task_id = session.plan.as_ref().map(|p| p.task.id.clone());
        pending.step_id = session
            .plan
            .as_ref()
            .and_then(|p| p.task.active_step.clone());
        pending.plan_revision = session.plan.as_ref().map(|p| p.revision);
        store.save_check(&pending)?;
        ensure!(
            store.check_action_scope(&pending.id)?.is_some(),
            "Check origin must exist before execution"
        );
        let record = run_check(
            &environment,
            &task.project,
            &overlays,
            &recipe,
            pending,
            &AtomicBool::new(false),
            |r| store.save_check(r),
        )?;
        store.save_check(&record)?;
        write_json(&directory.join(format!("{label}.json")), &record)?;
        ensure!(
            record.status == expected,
            "{label}: {:?}; {}\n{}",
            record.status,
            record.detail,
            record.output
        );
        let input = review_input(
            &task,
            &store,
            &session,
            &environment,
            &overlays,
            &record,
            &directory.join("captures"),
        )?;
        if label == "correct-alternative" {
            ensure!(
                input.evidence_gate(&command).0 == ActionReviewOutcome::Passed,
                "Correct isolated evidence did not match action inputs"
            );
            good = Some((record.clone(), overlays, input.binding));
        } else if supplemental {
            ensure!(
                input.evidence_gate(&command).0 != ActionReviewOutcome::Passed,
                "Known failing boundary evidence passed"
            );
        }
        results.push(json!({"label":label,"check_id":record.id,"status":record.status,"snapshot_unchanged":record.snapshot_unchanged,"supplemental":supplemental,"origin_bound_before_execution":true}));
        task.verify_originals()?;
    }
    let (record, overlays, binding) = good.context("Missing good proof")?;
    let missing = overlays
        .iter()
        .filter(|d| d.path != PYTHON_DIRECT_PATH)
        .cloned()
        .collect::<Vec<_>>();
    let missing_input = review_input(
        &task,
        &store,
        &session,
        &environment,
        &missing,
        &record,
        &directory.join("missing-capture"),
    )?;
    ensure!(
        missing_input.binding != binding
            && missing_input.evidence_gate(&command).0 != ActionReviewOutcome::Passed,
        "Missing overlay reused earlier check evidence"
    );
    let mut changed = overlays;
    changed
        .iter_mut()
        .find(|d| d.path == PYTHON_DIRECT_PATH)
        .context("Missing oracle overlay")?
        .snapshot(format!(
            "{PYTHON_DIRECT_TESTS}\n# Changed input invalidates the former receipt\n"
        ));
    ensure!(
        check_input_overlays(&task, changed.clone()).is_err(),
        "Mutated supplemental oracle accepted"
    );
    let changed_input = review_input(
        &task,
        &store,
        &session,
        &environment,
        &changed,
        &record,
        &directory.join("changed-capture"),
    )?;
    ensure!(
        changed_input.binding != binding
            && changed_input.evidence_gate(&command).0 != ActionReviewOutcome::Passed,
        "Changed overlay reused stale bound evidence"
    );
    ensure!(
        !task.project.root.join(PYTHON_DIRECT_PATH).exists()
            && !task.project.root.join(&task.path).exists(),
        "Kernel proof wrote a live fixture input"
    );
    task.verify_originals()?;
    ensure!(
        fs::read(root.join("turn-budget.json"))? == ledger_before,
        "Zero-model proof changed subscription ledger"
    );
    let receipt = json!({"class":"R actual isolated validation kernel; workflow setup is deterministic simulated data, no AI reviews","identity":identity()?,"model_turns":0,"real_journey_progress":false,"supplemental_path":PYTHON_DIRECT_PATH,"supplemental_source_hash":content_hash(PYTHON_DIRECT_TESTS.as_bytes()),"supplemental_cases":9,"original_case_count":7,"results":results,"missing_overlay_evidence_rejected":true,"changed_overlay_stale_binding_rejected":true,"immutable_originals_unchanged":true,"module_and_supplemental_live_files_absent":true,"ledger_bytes_unchanged":true});
    write_json(&directory.join("receipt.json"), &receipt)?;
    println!(
        "Supplemental kernel proof passed; zero model turns; receipt={}",
        directory.join("receipt.json").display()
    );
    Ok(())
}
