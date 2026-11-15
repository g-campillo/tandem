//! Pure command and presentation policy shared by buttons, menus and shortcuts.
use anyhow::{Result, ensure};
use tandem_core::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompanionTab {
    Conversation,
    Spec,
    Plan,
}

pub fn workflow_action(action: CompanionAction) -> WorkflowAction {
    match action {
        CompanionAction::DraftSpec => WorkflowAction::DraftSpec,
        CompanionAction::DraftPlan => WorkflowAction::DraftPlan,
        CompanionAction::Guide
        | CompanionAction::Hint
        | CompanionAction::Explain
        | CompanionAction::PrepareWalkthrough => WorkflowAction::PrepareWalkthrough,
        CompanionAction::Review | CompanionAction::ReviewAction => WorkflowAction::ReviewAction,
        CompanionAction::Reveal => WorkflowAction::Reveal,
        CompanionAction::Delegate => WorkflowAction::Takeover,
        CompanionAction::ImproveClarity
        | CompanionAction::ImproveSpeed
        | CompanionAction::ImproveBalanced => WorkflowAction::PrepareAction,
        _ => WorkflowAction::Discuss,
    }
}

pub fn command_binding(
    session: Option<&TaskSession>,
    action: CompanionAction,
    busy: bool,
) -> Result<WorkflowBinding> {
    ensure!(!busy, "Wait for the active operation or cancel it first");
    let session = session.ok_or_else(|| {
        anyhow::anyhow!("Continue this legacy task in a conversation before implementation")
    })?;
    if matches!(
        action,
        CompanionAction::Hint
            | CompanionAction::Explain
            | CompanionAction::Reveal
            | CompanionAction::Review
            | CompanionAction::ReviewAction
    ) {
        session.current_action()?;
    }
    Ok(session.authorize(workflow_action(action))?)
}

pub fn current_action_summary(session: &TaskSession) -> Vec<String> {
    let Ok((walk, action)) = session.current_action() else {
        return vec![];
    };
    vec![
        format!(
            "Current action {} of {} · {:?}",
            walk.cursor + 1,
            walk.actions.len(),
            action.kind
        ),
        format!("What: {}", action.what),
        format!("Why: {}", action.why),
        format!("Expected result: {}", action.expected_result),
    ]
}
pub fn completed_action_summary(session: &TaskSession) -> Vec<String> {
    session
        .walkthroughs
        .iter()
        .flat_map(|w| w.actions.iter().take(w.cursor))
        .map(|a| format!("Completed history: {} · {}", a.id, a.what))
        .collect()
}

pub fn model_label(preference: &ModelPreference, catalog: Option<&ModelCatalog>) -> String {
    match preference {
        ModelPreference::Automatic => "Automatic (account default)".into(),
        ModelPreference::Explicit { model } => catalog
            .and_then(|c| c.models.iter().find(|m| &m.model == model))
            .map(|m| m.display_name.clone())
            .unwrap_or_else(|| format!("{model} · unavailable; refresh or choose a model")),
    }
}

pub fn approval_matches(
    dirty: bool,
    displayed: Option<&DocumentRef>,
    saved: Option<&DocumentRef>,
) -> Result<()> {
    ensure!(!dirty, "Save your document edits before approving");
    ensure!(
        displayed.is_some() && displayed == saved,
        "The displayed saved version changed; inspect it again"
    );
    Ok(())
}

pub fn preview_key(
    session: &TaskSession,
    language: &str,
    guidance: &EffectiveGuidance,
) -> Option<String> {
    if guidance.solution != SolutionPolicy::AutomaticPreview {
        return None;
    }
    let binding = session.authorize(WorkflowAction::Reveal).ok()?;
    let (walkthrough, action) = session.current_action().ok()?;
    let task = &session.plan.as_ref()?.task;
    let step = task
        .steps
        .iter()
        .find(|s| Some(&s.id) == task.active_step.as_ref())?;
    if !step.languages.is_empty() && !step.languages.iter().any(|l| l == language) {
        return None;
    }
    Some(format!(
        "{}:{}:{}:{}:{}:{}",
        binding.session_id,
        binding.execution_generation,
        walkthrough.id,
        action.id,
        action.revision,
        language
    ))
}

#[cfg(test)]
#[path = "companion_state_tests.rs"]
mod tests;

#[derive(Clone)]
pub struct SolutionScope {
    pub workflow: WorkflowBinding,
    pub action_scope: DelegationActionScope,
    pub language: String,
    pub guidance: String,
}
pub fn solution_visible(
    scope: &SolutionScope,
    session: Option<&TaskSession>,
    language: &str,
    guidance: &EffectiveGuidance,
) -> bool {
    session.is_some_and(|s| {
        s.binding_is_current(WorkflowAction::Reveal, &scope.workflow)
            && DelegationActionScope::current(s).as_ref() == Some(&scope.action_scope)
    }) && scope.language == language
        && scope.guidance == guidance.fingerprint()
}

/// Unrelated discussion and edits made during a request never consume a draft.
pub fn clear_submitted_input(completed: bool, submitted: Option<&str>, current: &str) -> bool {
    completed && submitted.is_some_and(|submitted| current.trim() == submitted)
}

#[path = "document_drafts.rs"]
mod document_drafts;
pub use document_drafts::{
    DocumentEditorDraft, DraftField, SessionDocumentDrafts, entry_index, field_kind,
};

#[path = "companion_history.rs"]
mod companion_history;
pub use companion_history::{
    HistoricalRow, historical_run, historical_task, linked_legacy_history,
};

/// UI ownership starts before capture/account setup and ends only for this ID.
#[derive(Clone, Debug)]
pub struct ForegroundOperation {
    pub id: String,
    pub session_id: String,
    pub action: CompanionAction,
    pub phase: OperationPhase,
    pub stopping: bool,
    pub control: Option<String>,
}
pub struct OperationButton {
    pub label: String,
    pub loading: bool,
    pub disabled: bool,
}
fn canonical_action(action: CompanionAction) -> CompanionAction {
    match action {
        CompanionAction::Guide => CompanionAction::PrepareWalkthrough,
        CompanionAction::Review => CompanionAction::ReviewAction,
        other => other,
    }
}
impl ForegroundOperation {
    pub fn new(id: String, session_id: String, action: CompanionAction) -> Self {
        Self {
            id,
            session_id,
            action: canonical_action(action),
            phase: OperationPhase::PreservingInput,
            stopping: false,
            control: None,
        }
    }
    pub fn owns(&self, id: &str, session: &str) -> bool {
        self.id == id && self.session_id == session
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.phase = OperationPhase::Stopping;
    }
    pub fn button(&self, action: CompanionAction, label: &str) -> OperationButton {
        let loading =
            self.control.is_none() && canonical_action(action) == canonical_action(self.action);
        self.presentation(loading, label)
    }
    pub fn control_button(&self, control: &str, label: &str) -> OperationButton {
        self.presentation(self.control.as_deref() == Some(control), label)
    }
    fn presentation(&self, loading: bool, label: &str) -> OperationButton {
        OperationButton {
            label: if loading {
                format!("{label} · {}", phase_label(self.phase))
            } else {
                format!("{label} · Another operation is active")
            },
            loading,
            disabled: true,
        }
    }
}
pub fn phase_label(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::PreservingInput => "Preserving input…",
        OperationPhase::CapturingContext => "Capturing current buffers…",
        OperationPhase::CheckingAccount => "Checking account…",
        OperationPhase::CheckingModels => "Checking models…",
        OperationPhase::ReadingSource => "Reading source…",
        OperationPhase::SearchingSource => "Searching source…",
        OperationPhase::InspectingSource => "Inspecting source…",
        OperationPhase::PublicSearch => "Searching public sources…",
        OperationPhase::PublicRetrieval => "Reading public sources…",
        OperationPhase::Discussing => "Discussing…",
        OperationPhase::DraftingSpec => "Drafting Spec…",
        OperationPhase::DraftingPlan => "Drafting Plan…",
        OperationPhase::PreparingAction => "Preparing action…",
        OperationPhase::Reviewing => "Reviewing current action…",
        OperationPhase::Revealing => "Preparing requested suggestion…",
        OperationPhase::Stopping => "Stopping…",
    }
}

/// Local capture/commit and check dispatch cannot overlap. AI discussion alone
/// does not prevent independent, offline recipe execution.
pub fn check_dispatch_allowed(
    loading: bool,
    check_running: bool,
    agent_busy: bool,
    local_transition: bool,
) -> bool {
    !loading && !check_running && !agent_busy && !local_transition
}

/// Managed checks use the approved current action, including final validation
/// after the high-level task no longer has an active step. Unapproved sessions
/// can still run independent checks without claiming action evidence authority.
pub fn bind_check_record(
    record: &mut CheckRecord,
    session: Option<&TaskSession>,
    legacy_task: Option<&TaskPlan>,
) {
    if let Some(session) = session {
        if session.project == record.project
            && let Ok(workflow) = session.authorize(WorkflowAction::ReviewAction)
            && let Ok((walk, _)) = session.current_action()
        {
            record.task_id = workflow.task_id;
            record.step_id = Some(walk.step_id.clone());
            record.plan_revision = workflow.plan_ref.map(|p| p.revision);
        }
    } else if let Some(task) = legacy_task {
        record.task_id = Some(task.id.clone());
        record.step_id = task.active_step.clone();
        record.plan_revision = Some(task.revision);
    }
}

/// Automated execution is offered only for the identical registered command.
/// Recipes with per-run substitutions cannot match an immutable action argv.
pub fn matching_action_recipe<'a>(
    action: &GuidedAction,
    recipes: &'a [tandem_checks::Recipe],
) -> Option<&'a str> {
    let ActionTarget::Command { argv, cwd, .. } = &action.target else {
        return None;
    };
    recipes
        .iter()
        .find(|r| {
            cwd == "."
                && r.argv == *argv
                && !r
                    .argv
                    .iter()
                    .any(|a| a.contains("{artifacts}") || a.contains("{harness}"))
        })
        .map(|r| r.id.as_str())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputAttachmentDraft {
    pub command: String,
    pub cwd: String,
    pub exit: String,
    pub stdout: String,
    pub stderr: String,
}
impl OutputAttachmentDraft {
    pub fn from_evidence(evidence: &ActionCommandEvidence) -> Self {
        let (stdout, stderr) = match &evidence.output {
            CommandOutput::Split { stdout, stderr } => (stdout.clone(), stderr.clone()),
            _ => (evidence.output.text(), String::new()),
        };
        Self {
            command: serde_json::to_string(&evidence.argv).expect("String argv serializes"),
            cwd: evidence.cwd.clone(),
            exit: evidence
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_default(),
            stdout,
            stderr,
        }
    }
    pub fn attachment(&self) -> Result<tandem_companion::UserCommandAttachment> {
        let argv: Vec<String> = serde_json::from_str(&self.command).map_err(|_| {
            anyhow::anyhow!("Enter the exact command as a JSON argv array; no shell is run")
        })?;
        validate_action_command(&argv)?;
        if self.cwd != "." {
            normalized_relative(std::path::Path::new(&self.cwd))?;
        }
        let exit_code = self
            .exit
            .trim()
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("Enter the command's numeric exit status"))?;
        let time = now_ms();
        Ok(tandem_companion::UserCommandAttachment {
            argv,
            cwd: self.cwd.clone(),
            exit_code,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            started_at_ms: time,
            completed_at_ms: time,
        })
    }
}

pub fn current_review_candidate(
    session: &TaskSession,
    review: &ActionReview,
    outcome: Option<&OperationOutcome>,
    language: &str,
    guidance: &EffectiveGuidance,
) -> bool {
    review.outcome == ActionReviewOutcome::Passed
        && matches!(outcome, Some(OperationOutcome::Completed { .. }))
        && session.validate_action_binding(&review.binding).is_ok()
        && review.binding.language == language
        && review.binding.guidance_fingerprint == guidance.fingerprint()
}
/// Both bindings are captured by the real evidence adapter. No initial/frozen
/// review snapshot can be used to authorize Next after source/evidence changes.
pub fn advance_review(
    store: &mut Store,
    session: &TaskSession,
    review: &ActionReview,
    current: &ActionReviewInput,
) -> Result<TaskSession> {
    review_matches_capture(session, review, current)?;
    store.advance_action(
        &session.id,
        &review.binding.action_id,
        &review.id,
        &current.binding,
    )
}
pub fn review_matches_capture(
    session: &TaskSession,
    review: &ActionReview,
    current: &ActionReviewInput,
) -> Result<()> {
    current.validate(session)?;
    ensure!(
        review.binding == current.binding,
        "Inputs changed. Review the current action again before Next"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkthroughContinuation {
    DisplayAction,
    PrepareAction,
    Complete,
    AwaitApproval,
}
pub fn walkthrough_continuation(session: &TaskSession) -> WalkthroughContinuation {
    if session.implementation_complete() {
        WalkthroughContinuation::Complete
    } else if session.current_action().is_ok() {
        WalkthroughContinuation::DisplayAction
    } else if session
        .authorize(WorkflowAction::PrepareWalkthrough)
        .is_ok()
    {
        WalkthroughContinuation::PrepareAction
    } else {
        WalkthroughContinuation::AwaitApproval
    }
}
