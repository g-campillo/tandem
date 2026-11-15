//! Atomic action and immutable review contracts. Only a consumed current review
//! can authorize advancement; a historical ReviewResponse cannot do so.
use crate::conversation::{bounded_structure, bounded_text, stable_id};
use crate::{
    CheckRecord, CheckStatus, ContextSnapshot, DocumentRef, EffectiveGuidance, PlanDocument,
    SnapshotManifest, SpecDocument, TaskSession, WorkflowAction, WorkflowBinding, content_hash,
    normalized_relative,
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
            !crate::contains_solution_code(&serde_json::to_value(self)?),
            "Implementation code is hidden in action metadata"
        );
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
            !self.required_evidence.is_empty() && self.required_evidence.len() <= 64,
            "Action evidence requirements exceed 64 entries"
        );
        match &self.target {
            ActionTarget::File { path, symbol, .. } => {
                ensure!(
                    self.kind == GuidedActionKind::Edit,
                    "File target requires one edit action"
                );
                normalized_relative(Path::new(path))?;
                ensure!(
                    self.required_evidence
                        .contains(&RequiredActionEvidence::FileSnapshot { path: path.clone() }),
                    "An edit requires its target snapshot"
                );
                if let Some(symbol) = symbol {
                    ensure!(
                        !symbol.contains(['\n', ',', ';', '{', '}']),
                        "Target one symbol per action"
                    );
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
                validate_action_command(argv)?;
                ensure!(
                    self.required_evidence
                        .contains(&RequiredActionEvidence::CommandReceipt),
                    "A command requires its exact receipt"
                );
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
                ensure!(
                    criterion_refs
                        .iter()
                        .all(|criterion_ref| self.required_evidence.contains(
                            &RequiredActionEvidence::CriterionAssessment {
                                criterion_ref: criterion_ref.clone()
                            }
                        )),
                    "Validation requires explicit assessments for all criteria"
                );
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
#[serde(deny_unknown_fields)]
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
                    action.kind == GuidedActionKind::ValidateStep
                        || (self.actions.len() == 1
                            && action.kind == GuidedActionKind::ValidateTask),
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
                    step_id.as_ref() == Some(&self.step_id)
                        || (action.kind == GuidedActionKind::ValidateTask && step_id.is_none()),
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
    #[serde(default)]
    pub assessments: Vec<ActionCriterionAssessment>,
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

/// Commands are instructions for a human or a separately authorized fixed
/// recipe, never an implementation-script or shell-execution channel.
pub fn validate_action_command(argv: &[String]) -> Result<()> {
    ensure!(
        !argv.is_empty() && argv.len() <= 64,
        "Use one bounded argv command"
    );
    let executable = Path::new(&argv[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    ensure!(
        matches!(
            executable,
            "cargo" | "rustc" | "rustfmt" | "python" | "python3" | "pytest" | "git"
        ),
        "Unsupported guided command; use a fixed validation tool"
    );
    ensure!(
        argv.iter().all(|arg| arg.len() <= 4096
            && !arg.contains(['\0', '\n', '\r', ';', '|', '&', '`', '<', '>'])
            && !arg.contains("$(")),
        "Commands cannot contain shell scripts or redirection"
    );
    ensure!(
        !argv.iter().any(|arg| matches!(
            arg.as_str(),
            "-c" | "--command" | "--config" | "--config-file"
        ) || arg.starts_with("--config=")
            || arg.starts_with("-c=")),
        "Inline code and configuration overrides are not guided commands"
    );
    match executable {
        "python" | "python3" => ensure!(
            argv.len() >= 3
                && argv[1] == "-m"
                && matches!(argv[2].as_str(), "pytest" | "unittest" | "compileall"),
            "Use a named Python validation module, not a script"
        ),
        "cargo" => ensure!(
            argv.get(1).is_some_and(|s| matches!(
                s.as_str(),
                "test" | "check" | "clippy" | "fmt" | "build"
            )),
            "Use a Cargo validation command"
        ),
        "git" => ensure!(
            argv.get(1)
                .is_some_and(|s| matches!(s.as_str(), "status" | "diff")),
            "Guided Git commands inspect status or diff only"
        ),
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", deny_unknown_fields)]
pub enum ActionFileState {
    Present { hash: String },
    Absent,
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionEvidenceScope {
    pub workflow: WorkflowBinding,
    pub walkthrough_id: String,
    pub action_id: String,
    pub action_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum CommandOutput {
    /// Existing local check records interleave streams. Do not invent a split.
    Combined {
        text: String,
    },
    Split {
        stdout: String,
        stderr: String,
    },
    Withheld {
        hash: String,
        reason: String,
    },
}
impl CommandOutput {
    pub fn text(&self) -> String {
        match self {
            Self::Combined { text } => text.clone(),
            Self::Split { stdout, stderr } => format!("{stdout}\n{stderr}"),
            Self::Withheld { reason, .. } => format!("Required output withheld: {reason}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum CommandProvenance {
    LocalCheck {
        check_id: String,
        check_hash: String,
        executable_version: String,
    },
    /// Deliberately attached by the user; never authenticated as a local run.
    UserAttachedOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionCommandEvidence {
    pub id: String,
    pub scope: ActionEvidenceScope,
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: Option<i64>,
    pub output: CommandOutput,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub snapshot: SnapshotManifest,
    pub snapshot_unchanged: bool,
    pub status: CheckStatus,
    pub truncated: bool,
    pub recipe_id: Option<String>,
    pub recipe_fingerprint: Option<String>,
    pub provenance: CommandProvenance,
}
impl ActionCommandEvidence {
    pub fn fingerprint(&self) -> String {
        content_hash(&serde_json::to_vec(self).expect("Evidence serializes"))
    }
    pub fn from_local_check(scope: ActionEvidenceScope, check: &CheckRecord) -> Result<Self> {
        Ok(Self {
            id: check.id.clone(),
            scope,
            argv: check.command.clone(),
            cwd: ".".into(),
            exit_code: check.exit_code,
            output: if check.output.len() > 64 * 1024 {
                CommandOutput::Withheld {
                    hash: content_hash(check.output.as_bytes()),
                    reason: "Output exceeds the complete 64 KiB action evidence limit".into(),
                }
            } else {
                CommandOutput::Combined {
                    text: check.output.clone(),
                }
            },
            started_at_ms: check.started_at_ms,
            completed_at_ms: check.completed_at_ms,
            snapshot: check
                .snapshot
                .clone()
                .context("Check has no captured input")?,
            snapshot_unchanged: check.snapshot_unchanged == Some(true),
            status: check.status.clone(),
            truncated: check.output_truncated || check.output.len() > 64 * 1024,
            recipe_id: Some(check.recipe_id.clone()),
            recipe_fingerprint: Some(check.recipe_fingerprint.clone()),
            provenance: CommandProvenance::LocalCheck {
                check_id: check.id.clone(),
                check_hash: content_hash(&serde_json::to_vec(check)?),
                executable_version: check.executable_version.clone(),
            },
        })
    }
    pub fn validate(&self) -> Result<()> {
        stable_id(&self.id)?;
        ensure!(
            !self.argv.is_empty()
                && self.argv.len() <= 64
                && self
                    .argv
                    .iter()
                    .all(|s| s.len() <= 4096 && !s.contains('\0')),
            "Invalid evidence command"
        );
        if self.cwd != "." {
            normalized_relative(Path::new(&self.cwd))?;
        }
        ensure!(
            self.output.text().len() <= 64 * 1024,
            "Required command output exceeds the 64 KiB evidence limit; attach a complete bounded receipt"
        );
        if matches!(self.provenance, CommandProvenance::UserAttachedOutput) {
            ensure!(
                self.recipe_id.is_none() && self.recipe_fingerprint.is_none(),
                "User output cannot claim a verified recipe"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionCriterionAssessment {
    pub criterion_ref: CriterionRef,
    pub satisfied: bool,
    pub explanation: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReviewInput {
    pub binding: ActionReviewBinding,
    pub context: ContextSnapshot,
    pub targets: std::collections::BTreeMap<String, ActionFileState>,
    pub commands: Vec<ActionCommandEvidence>,
}
impl ActionReviewInput {
    pub fn capture(
        session: &TaskSession,
        context: ContextSnapshot,
        targets: std::collections::BTreeMap<String, ActionFileState>,
        commands: Vec<ActionCommandEvidence>,
        language: &str,
        guidance: &EffectiveGuidance,
    ) -> Result<Self> {
        let (walk, action) = session.current_action()?;
        let mut evidence = targets
            .iter()
            .map(|(path, state)| ActionEvidenceBinding {
                id: content_hash(format!("file:{path}").as_bytes()),
                hash: content_hash(
                    &serde_json::to_vec(&(path, state)).expect("File state serializes"),
                ),
            })
            .collect::<Vec<_>>();
        evidence.extend(commands.iter().map(|c| ActionEvidenceBinding {
            id: c.id.clone(),
            hash: c.fingerprint(),
        }));
        evidence.sort_by(|a, b| a.id.cmp(&b.id));
        let input = Self {
            binding: ActionReviewBinding {
                workflow: session.authorize(WorkflowAction::ReviewAction)?,
                walkthrough_id: walk.id.clone(),
                step_id: walk.step_id.clone(),
                action_id: action.id.clone(),
                action_revision: action.revision,
                action_hash: action.definition_hash(),
                context_id: context.id.clone(),
                project_fingerprint: context.project_fingerprint.clone(),
                language: language.into(),
                guidance_fingerprint: guidance.fingerprint(),
                evidence,
            },
            context,
            targets,
            commands,
        };
        input.validate(session)?;
        Ok(input)
    }
    pub fn scope(&self) -> ActionEvidenceScope {
        self.binding.scope()
    }
    pub fn validate(&self, session: &TaskSession) -> Result<()> {
        session.validate_action_binding(&self.binding)?;
        ensure!(
            self.context.id == self.binding.context_id
                && self.context.project_fingerprint == self.binding.project_fingerprint,
            "Review context binding differs"
        );
        ensure!(
            !self.binding.language.trim().is_empty()
                && !self.binding.guidance_fingerprint.is_empty(),
            "Review needs language and guidance"
        );
        ensure!(
            self.targets.len() <= 128 && self.commands.len() <= 32,
            "Too much action evidence"
        );
        let mut expected = vec![];
        for (path, state) in &self.targets {
            normalized_relative(Path::new(path))?;
            if let ActionFileState::Present { hash } = state {
                ensure!(
                    self.context.files.iter().any(|f| &f.path == path
                        && &f.hash == hash
                        && content_hash(f.text.as_bytes()) == f.hash),
                    "Target content is missing from review context"
                );
            }
            expected.push(ActionEvidenceBinding {
                id: content_hash(format!("file:{path}").as_bytes()),
                hash: content_hash(&serde_json::to_vec(&(path, state))?),
            });
        }
        for command in &self.commands {
            command.validate()?;
            ensure!(
                command.scope == self.scope(),
                "Command evidence belongs to another action or generation"
            );
            expected.push(ActionEvidenceBinding {
                id: command.id.clone(),
                hash: command.fingerprint(),
            });
        }
        expected.sort_by(|a, b| a.id.cmp(&b.id));
        ensure!(
            expected.windows(2).all(|w| w[0].id != w[1].id) && expected == self.binding.evidence,
            "Evidence set differs from immutable review binding"
        );
        Ok(())
    }
    /// Fail closed using actual receipts before considering the model verdict.
    pub fn evidence_gate(&self, action: &GuidedAction) -> (ActionReviewOutcome, Vec<String>) {
        let mut missing = self
            .targets
            .iter()
            .filter(|(_, state)| matches!(state, ActionFileState::Unavailable { .. }))
            .map(|(path, _)| {
                format!("Capture the required surrounding interface {path} before review")
            })
            .collect::<Vec<_>>();
        let mut failed = vec![];
        let mut paths = action
            .required_evidence
            .iter()
            .filter_map(|r| match r {
                RequiredActionEvidence::FileSnapshot { path } => Some(path),
                _ => None,
            })
            .collect::<Vec<_>>();
        match &action.target {
            ActionTarget::File {
                path, change_kind, ..
            } => {
                paths.push(path);
                match (change_kind, self.targets.get(path)) {
                    (FileChangeKind::Delete, Some(ActionFileState::Absent)) => {}
                    (_, Some(ActionFileState::Unavailable { .. }) | None) => missing.push(format!(
                        "Capture the target {path} with surrounding interfaces"
                    )),
                    (FileChangeKind::Delete, _) => {
                        failed.push(format!("Remove the target {path}, then request review"))
                    }
                    (_, Some(ActionFileState::Absent)) => {
                        failed.push(format!("Create the target {path}, then request review"))
                    }
                    _ => {}
                }
            }
            ActionTarget::Validation {
                paths: validation_paths,
                ..
            } => paths.extend(validation_paths),
            _ => {}
        }
        for path in paths {
            if !matches!(
                self.targets.get(path),
                Some(ActionFileState::Present { .. } | ActionFileState::Absent)
            ) {
                missing.push(format!("Capture readable target evidence for {path}"));
            }
        }
        for requirement in &action.required_evidence {
            let candidates = self.commands.iter().filter(|c| match requirement {
                RequiredActionEvidence::CommandReceipt => matches!(&action.target, ActionTarget::Command { argv, cwd, .. } if argv == &c.argv && cwd == &c.cwd),
                RequiredActionEvidence::CheckReceipt { recipe_id } => c.recipe_id.as_ref() == Some(recipe_id) && matches!(c.provenance, CommandProvenance::LocalCheck { .. }),
                _ => false,
            }).collect::<Vec<_>>();
            if !matches!(
                requirement,
                RequiredActionEvidence::CommandReceipt
                    | RequiredActionEvidence::CheckReceipt { .. }
            ) {
                continue;
            }
            for c in &candidates {
                if c.snapshot.id == self.binding.project_fingerprint
                    && c.status == CheckStatus::Failed
                {
                    failed.push("Fix the failing required command and run it again".into());
                }
            }
            let candidate_count = candidates.len();
            let valid = candidates
                .into_iter()
                .filter(|c| {
                    c.snapshot.id == self.binding.project_fingerprint
                        && c.snapshot_unchanged
                        && !c.truncated
                        && !matches!(c.output, CommandOutput::Withheld { .. })
                        && c.completed_at_ms.is_some_and(|t| {
                            t >= c.started_at_ms && t <= self.context.captured_at_ms
                        })
                        && c.started_at_ms >= 0
                        && (matches!(c.status, CheckStatus::Passed | CheckStatus::Failed)
                            || (matches!(c.provenance, CommandProvenance::UserAttachedOutput)
                                && c.status == CheckStatus::Unvalidated))
                        && c.exit_code.is_some()
                        && (matches!(c.provenance, CommandProvenance::UserAttachedOutput)
                            || c.recipe_id
                                .as_ref()
                                .zip(c.recipe_fingerprint.as_ref())
                                .is_some_and(|(id, fp)| {
                                    self.context.validation_environments.get(id) == Some(fp)
                                        && fp != "unavailable"
                                }))
                })
                .collect::<Vec<_>>();
            if valid.is_empty() || valid.len() != candidate_count {
                missing.push("Run the exact required command on the current inputs and attach its complete result".into());
            }
            for c in valid {
                let expected = match &action.target {
                    ActionTarget::Command {
                        expected_exit_codes,
                        ..
                    } => expected_exit_codes
                        .iter()
                        .any(|code| Some(i64::from(*code)) == c.exit_code),
                    _ => c.exit_code == Some(0),
                };
                if c.status == CheckStatus::Failed || !expected {
                    failed.push("Fix the failing required command and run it again".into());
                }
            }
        }
        if !failed.is_empty() {
            (ActionReviewOutcome::NeedsChanges, failed)
        } else if !missing.is_empty() {
            (ActionReviewOutcome::InsufficientEvidence, missing)
        } else {
            (ActionReviewOutcome::Passed, vec![])
        }
    }
}
impl ActionReviewBinding {
    pub fn scope(&self) -> ActionEvidenceScope {
        ActionEvidenceScope {
            workflow: self.workflow.clone(),
            walkthrough_id: self.walkthrough_id.clone(),
            action_id: self.action_id.clone(),
            action_hash: self.action_hash.clone(),
        }
    }
}

impl ActionReview {
    pub fn validate_and_gate(
        &mut self,
        session: &TaskSession,
        input: &ActionReviewInput,
    ) -> Result<()> {
        input.validate(session)?;
        stable_id(&self.id)?;
        ensure!(
            self.binding == input.binding,
            "Review uses different immutable inputs"
        );
        ensure!(
            !crate::contains_solution_code(&serde_json::to_value(&self)?),
            "Implementation code is hidden in action reviews"
        );
        bounded_text(&self.summary, 4096, "Review summary")?;
        ensure!(
            self.findings.len() <= 32
                && self.assessments.len() <= 128
                && self.limitations.len() <= 64,
            "Review exceeds its bounds"
        );
        let ids = self
            .binding
            .evidence
            .iter()
            .map(|e| &e.id)
            .collect::<BTreeSet<_>>();
        let spec = session.spec.as_ref().context("Missing spec")?;
        let plan = session.plan.as_ref().context("Missing plan")?;
        ensure!(
            self.evidence_used.iter().all(|id| ids.contains(id)),
            "Review cites unavailable evidence"
        );
        for f in &self.findings {
            for text in [&f.problem, &f.why, &f.next_action] {
                bounded_text(text, 4096, "Review finding")?;
            }
            ensure!(
                f.evidence_ids.iter().all(|id| ids.contains(id)),
                "Finding cites unavailable evidence"
            );
            for criterion in &f.criterion_refs {
                criterion.validate(spec, plan)?;
            }
        }
        let mut assessed = BTreeSet::new();
        for assessment in &self.assessments {
            assessment.criterion_ref.validate(spec, plan)?;
            ensure!(
                assessed.insert(&assessment.criterion_ref),
                "Duplicate criterion assessment"
            );
            bounded_text(&assessment.explanation, 4096, "Criterion assessment")?;
            ensure!(
                assessment.evidence_ids.iter().all(|id| ids.contains(id)),
                "Assessment cites unavailable evidence"
            );
        }
        let (_, action) = session.current_action()?;
        let (gate, instructions) = input.evidence_gate(action);
        if matches!(
            self.outcome,
            ActionReviewOutcome::Passed
                | ActionReviewOutcome::NeedsChanges
                | ActionReviewOutcome::InsufficientEvidence
        ) {
            if gate != ActionReviewOutcome::Passed {
                self.outcome = gate;
            }
            if self.findings.iter().any(|f| f.blocking)
                || self.assessments.iter().any(|a| !a.satisfied)
            {
                self.outcome = ActionReviewOutcome::NeedsChanges;
            }
            let absent_assessment = action.required_evidence.iter().any(|e| matches!(e, RequiredActionEvidence::CriterionAssessment { criterion_ref } if !assessed.contains(criterion_ref)));
            if self.outcome == ActionReviewOutcome::Passed
                && (absent_assessment || !self.limitations.is_empty())
            {
                self.outcome = ActionReviewOutcome::InsufficientEvidence;
            }
            for instruction in instructions {
                if !self.limitations.contains(&instruction) {
                    self.limitations.push(instruction);
                }
            }
            if self.outcome != ActionReviewOutcome::Passed
                && !self
                    .findings
                    .iter()
                    .any(|f| !f.next_action.trim().is_empty())
                && self.limitations.is_empty()
            {
                self.limitations.push(
                    "Resolve the review uncertainty for this action, then request review again"
                        .into(),
                );
            }
        }
        bounded_structure(self)
    }
}
