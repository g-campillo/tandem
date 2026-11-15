//! Deterministic, explicitly saved/approved/started fixture; never production authority.
use anyhow::Result;
use tandem_core::*;
pub fn approved_session(store: &mut Store, task: TaskPlan) -> Result<TaskSession> {
    let mut session = store.create_session(&task.project)?;
    let spec = SpecDocument {
        revision: 1,
        summary: task.goal.clone(),
        requirements: vec![Criterion {
            id: "requirement".into(),
            text: task.goal.clone(),
        }],
        in_scope: vec![],
        out_of_scope: vec![],
        constraints: vec![],
        acceptance: task
            .acceptance
            .iter()
            .enumerate()
            .map(|(i, t)| Criterion {
                id: format!("acceptance-{i}"),
                text: t.clone(),
            })
            .collect(),
    };
    session = store.apply_session_transition(
        &session.id,
        session.sequence,
        &SessionTransition::new(
            new_id(),
            vec![SessionCommand::SaveSpec { spec: spec.clone() }],
        ),
    )?;
    let plan = PlanDocument {
        revision: 1,
        spec_ref: session.spec.as_ref().unwrap().document_ref(),
        task,
    };
    session = store.apply_session_transition(
        &session.id,
        session.sequence,
        &SessionTransition::new(new_id(), vec![SessionCommand::SavePlan { plan }]),
    )?;
    store.apply_session_transition(
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
    )
}

// Shared fixtures are compiled by examples that exercise only document approval.
#[allow(dead_code)]
pub fn fixture_walkthrough(session: &TaskSession) -> Walkthrough {
    let plan = session.plan.as_ref().unwrap();
    let step = &plan.task.steps[0];
    let path = step.files.first().cloned().unwrap_or("greeting.rs".into());
    let criteria = if step.done_when.is_empty() {
        session
            .spec
            .as_ref()
            .unwrap()
            .acceptance
            .iter()
            .map(|c| CriterionRef::SpecAcceptance { id: c.id.clone() })
            .collect::<Vec<_>>()
    } else {
        step.done_when
            .iter()
            .enumerate()
            .map(|(i, _)| CriterionRef::StepDoneWhen {
                step_id: step.id.clone(),
                index: i,
            })
            .collect()
    };
    Walkthrough {
        id: new_id(),
        generation: session.execution_generation,
        step_id: step.id.clone(),
        plan_ref: plan.document_ref(),
        cursor: 0,
        actions: vec![
            GuidedAction {
                id: "edit".into(),
                revision: 1,
                kind: GuidedActionKind::Edit,
                what: "Implement the named behavior".into(),
                target: ActionTarget::File {
                    path: path.clone(),
                    symbol: None,
                    change_kind: FileChangeKind::Modify,
                },
                why: "Meet the saved requirement".into(),
                expected_result: "The behavior is implemented".into(),
                required_evidence: vec![RequiredActionEvidence::FileSnapshot { path }],
            },
            GuidedAction {
                id: "validate".into(),
                revision: 1,
                kind: GuidedActionKind::ValidateStep,
                what: "Assess the implementation".into(),
                target: ActionTarget::Validation {
                    spec_ref: plan.spec_ref.clone(),
                    plan_ref: plan.document_ref(),
                    step_id: Some(step.id.clone()),
                    criterion_refs: criteria.clone(),
                    paths: vec![],
                },
                why: "Confirm the acceptance condition".into(),
                expected_result: "The saved acceptance condition is met".into(),
                required_evidence: criteria
                    .into_iter()
                    .map(
                        |criterion_ref| RequiredActionEvidence::CriterionAssessment {
                            criterion_ref,
                        },
                    )
                    .collect(),
            },
        ],
    }
}
