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
        CompanionAction::Guide | CompanionAction::Hint | CompanionAction::PrepareWalkthrough => {
            WorkflowAction::PrepareWalkthrough
        }
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
    Ok(session.authorize(workflow_action(action))?)
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
