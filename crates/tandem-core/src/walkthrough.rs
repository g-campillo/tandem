//! Atomic action and immutable review contracts. Only a consumed current review
//! can authorize advancement; a historical ReviewResponse cannot do so.
use crate::conversation::{bounded_structure, bounded_text, stable_id};
use crate::{
    DocumentRef, PlanDocument, SpecDocument, WorkflowBinding, content_hash, normalized_relative,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum CriterionRef {
    SpecAcceptance { id: String },
    StepDoneWhen { step_id: String, index: usize },
}

impl CriterionRef {
    pub fn validate(&self, spec: &SpecDocument, plan: &PlanDocument) -> Result<()> {
        ensure!(
            plan.spec_ref == spec.document_ref(),
            "Criterion documents do not match"
        );
        match self {
            Self::SpecAcceptance { id } => ensure!(
                spec.acceptance.iter().any(|c| &c.id == id),
                "Unknown spec acceptance criterion"
            ),
            Self::StepDoneWhen { step_id, index } => ensure!(
                plan.task
                    .steps
                    .iter()
                    .any(|s| &s.id == step_id && *index < s.done_when.len()),
                "Unknown step completion criterion"
            ),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuidedActionKind {
    Edit,
    Command,
    ValidateStep,
    ValidateTask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeKind {
    Create,
    Modify,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ActionTarget {
    File {
        path: String,
        symbol: Option<String>,
        change_kind: FileChangeKind,
    },
    Command {
        argv: Vec<String>,
        cwd: String,
        expected_exit_codes: Vec<i32>,
    },
    Validation {
        spec_ref: DocumentRef,
        plan_ref: DocumentRef,
        step_id: Option<String>,
        criterion_refs: Vec<CriterionRef>,
        paths: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum RequiredActionEvidence {
    FileSnapshot { path: String },
    CommandReceipt,
    CheckReceipt { recipe_id: String },
    CriterionAssessment { criterion_ref: CriterionRef },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuidedAction {
    pub id: String,
    pub revision: u64,
    pub kind: GuidedActionKind,
    pub what: String,
    pub target: ActionTarget,
    pub why: String,
    pub expected_result: String,
    pub required_evidence: Vec<RequiredActionEvidence>,
}

impl GuidedAction {
    pub fn definition_hash(&self) -> String {
        content_hash(&serde_json::to_vec(self).expect("String-only action serializes"))
    }

    pub fn validate(&self, spec: &SpecDocument, plan: &PlanDocument) -> Result<()> {
        spec.validate()?;
        plan.validate()?;
        ensure!(
            plan.spec_ref == spec.document_ref(),
            "Action documents do not match"
        );
        stable_id(&self.id)?;
        ensure!(self.revision > 0, "Action revision must be positive");
        for text in [&self.what, &self.why, &self.expected_result] {
            bounded_text(text, 4096, "Action description")?;
        }
        ensure!(
            self.required_evidence.len() <= 64,
            "Action evidence requirements exceed 64 entries"
        );
        match &self.target {
            ActionTarget::File { path, symbol, .. } => {
                ensure!(
                    self.kind == GuidedActionKind::Edit,
                    "File target requires one edit action"
                );
                normalized_relative(Path::new(path))?;
                if let Some(symbol) = symbol {
                    bounded_text(symbol, 1024, "Target symbol")?;
                }
            }
            ActionTarget::Command {
                argv,
                cwd,
                expected_exit_codes,
            } => {
                ensure!(
                    self.kind == GuidedActionKind::Command,
                    "Command target requires one command action"
                );
                ensure!(
                    !argv.is_empty() && argv.len() <= 64,
                    "Use one bounded argv command"
                );
                bounded_text(&argv[0], 4096, "Command executable")?;
                ensure!(
                    argv.iter().all(|a| a.len() <= 4096 && !a.contains('\0')),
                    "Command argument is oversized or contains NUL"
                );
                if cwd != "." {
                    normalized_relative(Path::new(cwd))?;
                }
                ensure!(
                    !expected_exit_codes.is_empty() && expected_exit_codes.len() <= 16,
                    "Declare 1–16 expected command exit codes"
                );
            }
            ActionTarget::Validation {
                spec_ref,
                plan_ref,
                step_id,
                criterion_refs,
                paths,
            } => {
                ensure!(
                    *spec_ref == spec.document_ref()
                        && *plan_ref == plan.document_ref()
                        && plan.spec_ref == *spec_ref,
                    "Validation target uses stale documents"
                );
                ensure!(
                    !criterion_refs.is_empty() && criterion_refs.len() <= 128,
                    "Validation needs 1–128 named criteria"
                );
                let mut unique = BTreeSet::new();
                for criterion in criterion_refs {
                    criterion.validate(spec, plan)?;
                    ensure!(
                        unique.insert(criterion),
                        "Validation criteria must be unique"
                    );
                }
                match (self.kind, step_id) {
                    (GuidedActionKind::ValidateStep, Some(id)) => {
                        let step = plan
                            .task
                            .steps
                            .iter()
                            .find(|s| &s.id == id)
                            .context("Unknown validation step")?;
                        ensure!(
                            criterion_refs.iter().all(|c| match c {
                                CriterionRef::SpecAcceptance { .. } => true,
                                CriterionRef::StepDoneWhen { step_id, .. } => step_id == id,
                            }),
                            "Step assessment cannot name another step's completion conditions"
                        );
                        ensure!(
                            (0..step.done_when.len()).all(|index| criterion_refs.contains(
                                &CriterionRef::StepDoneWhen {
                                    step_id: id.clone(),
                                    index
                                }
                            )),
                            "Final step assessment must cover every completion condition"
                        );
                    }
                    (GuidedActionKind::ValidateTask, None) => {
                        ensure!(
                            criterion_refs
                                .iter()
                                .all(|c| matches!(c, CriterionRef::SpecAcceptance { .. })),
                            "Task validation must name approved spec acceptance criteria"
                        );
                        ensure!(
                            spec.acceptance.iter().all(|c| criterion_refs
                                .contains(&CriterionRef::SpecAcceptance { id: c.id.clone() })),
                            "Task validation must cover all spec acceptance criteria"
                        );
                    }
                    _ => {
                        anyhow::bail!("Validation target requires a step or task assessment action")
                    }
                }
                ensure!(paths.len() <= 64, "Validation paths exceed 64 files");
                for path in paths {
                    normalized_relative(Path::new(path))?;
                }
            }
        }
        for evidence in &self.required_evidence {
            match evidence {
                RequiredActionEvidence::FileSnapshot { path } => {
                    normalized_relative(Path::new(path))?;
                }
                RequiredActionEvidence::CheckReceipt { recipe_id } => stable_id(recipe_id)?,
                RequiredActionEvidence::CriterionAssessment { criterion_ref } => {
                    criterion_ref.validate(spec, plan)?
                }
                RequiredActionEvidence::CommandReceipt => {}
            }
        }
        bounded_structure(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Walkthrough {
    pub id: String,
    pub generation: u64,
    pub step_id: String,
    pub plan_ref: DocumentRef,
    pub actions: Vec<GuidedAction>,
    /// Cursor equal to actions.len() means all actions have been consumed.
    pub cursor: usize,
}

impl Walkthrough {
    pub fn validate(&self, spec: &SpecDocument, plan: &PlanDocument) -> Result<()> {
        spec.validate()?;
        plan.validate()?;
        stable_id(&self.id)?;
        ensure!(
            self.generation > 0 && self.plan_ref == plan.document_ref(),
            "Walkthrough uses an older plan or generation"
        );
        ensure!(
            plan.task.steps.iter().any(|s| s.id == self.step_id),
            "Walkthrough step is unknown"
        );
        ensure!(
            (1..=32).contains(&self.actions.len()) && self.cursor <= self.actions.len(),
            "Walkthrough needs 1–32 actions and a valid cursor"
        );
        let mut ids = BTreeSet::new();
        for (index, action) in self.actions.iter().enumerate() {
            action.validate(spec, plan)?;
            ensure!(
                ids.insert(&action.id),
                "Walkthrough action IDs must be unique"
            );
            if index + 1 == self.actions.len() {
                ensure!(
                    action.kind == GuidedActionKind::ValidateStep,
                    "Finish a walkthrough with a cumulative step assessment"
                );
            } else {
                ensure!(
                    matches!(
                        action.kind,
                        GuidedActionKind::Edit | GuidedActionKind::Command
                    ),
                    "Step assessment must be the final walkthrough action"
                );
            }
            if let ActionTarget::Validation { step_id, .. } = &action.target {
                ensure!(
                    step_id.as_ref() == Some(&self.step_id),
                    "Walkthrough assessment names another step"
                );
            }
        }
        // Cursor progress must not change whether an accepted definition fits.
        bounded_structure(&(
            &self.id,
            self.generation,
            &self.step_id,
            &self.plan_ref,
            &self.actions,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionEvidenceBinding {
    pub id: String,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReviewBinding {
    pub workflow: WorkflowBinding,
    pub walkthrough_id: String,
    pub step_id: String,
    pub action_id: String,
    pub action_revision: u64,
    pub action_hash: String,
    pub context_id: String,
    pub project_fingerprint: String,
    pub language: String,
    pub guidance_fingerprint: String,
    pub evidence: Vec<ActionEvidenceBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionReviewOutcome {
    Passed,
    NeedsChanges,
    InsufficientEvidence,
    Cancelled,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReviewFinding {
    pub problem: String,
    pub why: String,
    pub next_action: String,
    pub blocking: bool,
    pub criterion_refs: Vec<CriterionRef>,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReview {
    pub id: String,
    pub binding: ActionReviewBinding,
    pub outcome: ActionReviewOutcome,
    pub summary: String,
    pub findings: Vec<ActionReviewFinding>,
    pub evidence_used: Vec<String>,
    pub limitations: Vec<String>,
    pub reviewed_at_ms: i64,
}

/// Persisted atomically by Next. A passing review alone creates no transition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumedActionTransition {
    pub id: String,
    pub review_id: String,
    pub binding: ActionReviewBinding,
    pub from_cursor: usize,
    pub to_cursor: usize,
    pub consumed_at_ms: i64,
}
