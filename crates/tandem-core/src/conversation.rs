//! Append-only conversation contracts. Conversation data never grants workflow authority.
use crate::{
    DocumentApproval, DocumentProposal, DocumentRef, ResearchActivity, ResearchFinding,
    WorkflowBinding,
};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_HUMAN_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_STRUCTURED_RESPONSE_BYTES: usize = 128 * 1024;

pub(crate) fn bounded_text(text: &str, limit: usize, name: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= limit && !text.contains('\0'),
        "{name} is empty or exceeds its text limit"
    );
    Ok(())
}

pub(crate) fn stable_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')),
        "Use a stable ID of at most 128 ASCII letters, digits, dots, underscores or hyphens"
    );
    Ok(())
}

pub(crate) fn bounded_structure(value: &impl Serialize) -> Result<()> {
    ensure!(
        serde_json::to_vec(value)?.len() <= MAX_STRUCTURED_RESPONSE_BYTES,
        "Structured content exceeds 128 KiB"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionChoice {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClarificationQuestion {
    pub id: String,
    pub prompt: String,
    pub choices: Vec<QuestionChoice>,
}

impl ClarificationQuestion {
    pub fn validate(&self) -> Result<()> {
        stable_id(&self.id)?;
        bounded_text(&self.prompt, 4096, "Question")?;
        ensure!(
            (2..=5).contains(&self.choices.len()),
            "A question needs 2–5 choices and also allows a custom answer"
        );
        let mut ids = BTreeSet::new();
        for choice in &self.choices {
            stable_id(&choice.id)?;
            ensure!(ids.insert(&choice.id), "Question choice IDs must be unique");
            bounded_text(&choice.label, 256, "Choice label")?;
            ensure!(
                choice.description.len() <= 2048,
                "Choice description exceeds 2 KiB"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClarificationAnswer {
    pub question_id: String,
    pub choice_id: Option<String>,
    pub custom_text: Option<String>,
}

impl ClarificationAnswer {
    pub fn validate(&self, question: &ClarificationQuestion) -> Result<()> {
        ensure!(
            self.question_id == question.id,
            "Answer refers to a stale or unknown question"
        );
        if let Some(id) = &self.choice_id {
            ensure!(
                question.choices.iter().any(|c| &c.id == id),
                "Answer refers to an unknown choice"
            );
        }
        if let Some(text) = &self.custom_text {
            bounded_text(text, MAX_HUMAN_MESSAGE_BYTES, "Custom answer")?;
        }
        ensure!(
            self.choice_id.is_some() || self.custom_text.is_some(),
            "Choose an answer or enter custom text"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub id: String,
    pub session_id: String,
    pub sequence: u64,
    pub created_at_ms: i64,
    pub run_id: Option<String>,
    pub payload: ConversationPayload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDocumentKind {
    Spec,
    Plan,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ConversationPayload {
    OperationInputRecorded {
        operation_id: String,
        input_hash: String,
    },
    OperationResultHistorical {
        operation_id: String,
        detail: String,
    },
    OperationPhaseChanged {
        operation_id: String,
        phase: OperationPhase,
    },
    OperationSnapshot {
        snapshot: ConversationSnapshot,
    },
    SummaryCheckpoint {
        checkpoint: SummaryCheckpoint,
    },
    HumanMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    QuestionAsked {
        question: ClarificationQuestion,
    },
    QuestionAnswered {
        answer: ClarificationAnswer,
    },
    /// Host-created continuation metadata, never accepted from model output.
    QuestionContinuationRecorded {
        question_id: String,
        continuation: QuestionContinuation,
    },
    ResearchActivity {
        activity: ResearchActivity,
    },
    ResearchFindings {
        findings: Vec<ResearchFinding>,
    },
    DocumentProposed {
        proposal: DocumentProposal,
    },
    DocumentSaved {
        kind: WorkflowDocumentKind,
        document_ref: DocumentRef,
    },
    DocumentApproved {
        kind: WorkflowDocumentKind,
        approval: DocumentApproval,
    },
    WorkflowTransition {
        transition: WorkflowTransition,
    },
    ResearchReceiptRecorded {
        receipt_id: String,
    },
    WalkthroughPrepared {
        walkthrough_id: String,
    },
    ActionTransitionRecorded {
        transition_id: String,
    },
    OperationStarted {
        operation: SessionOperation,
    },
    OperationOutcome {
        operation_id: String,
        outcome: OperationOutcome,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum WorkflowTransition {
    ImplementationStarted {
        binding: WorkflowBinding,
        step_id: String,
    },
    DefinitionInvalidated {
        generation: u64,
    },
    ActionAdvanced {
        transition_id: String,
    },
    ImplementationCompleted {
        binding: WorkflowBinding,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", deny_unknown_fields)]
pub enum OperationOutcome {
    Completed { summary: String },
    Cancelled { detail: String },
    Interrupted { detail: String },
    Failed { detail: String },
}

/// Durable operation identity. Restart records an outcome before clearing its active slot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOperation {
    pub id: String,
    pub session_id: String,
    pub kind: SessionOperationKind,
    pub started_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionOperationKind {
    Discuss,
    Research,
    DraftSpec,
    DraftPlan,
    PrepareAction,
    ReviewAction,
    Reveal,
}

/// The unfinished operation behind a clarification. Definitions and action
/// identity are retained across restart, but this never grants approval or Start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionContinuation {
    pub kind: SessionOperationKind,
    pub spec_ref: Option<DocumentRef>,
    pub plan_ref: Option<DocumentRef>,
    pub workflow: Option<WorkflowBinding>,
    pub step_id: Option<String>,
    pub action_scope: Option<crate::ActionEvidenceScope>,
}
impl QuestionContinuation {
    pub fn capture(session: &crate::TaskSession, kind: SessionOperationKind) -> Result<Self> {
        let implementation = match kind {
            SessionOperationKind::DraftSpec | SessionOperationKind::DraftPlan => None,
            SessionOperationKind::PrepareAction => Some(crate::WorkflowAction::PrepareWalkthrough),
            SessionOperationKind::ReviewAction => Some(crate::WorkflowAction::ReviewAction),
            _ => anyhow::bail!("This operation has no typed clarification continuation"),
        };
        let workflow = implementation
            .map(|action| session.authorize(action))
            .transpose()?;
        let action_scope = workflow.as_ref().and_then(|workflow| {
            session
                .current_action()
                .ok()
                .map(|(walk, action)| crate::ActionEvidenceScope {
                    workflow: workflow.clone(),
                    walkthrough_id: walk.id.clone(),
                    action_id: action.id.clone(),
                    action_hash: action.definition_hash(),
                })
        });
        ensure!(
            kind != SessionOperationKind::ReviewAction || action_scope.is_some(),
            "Prepare an action before continuing its review"
        );
        Ok(Self {
            kind,
            spec_ref: session.spec.as_ref().map(crate::SpecDocument::document_ref),
            plan_ref: session.plan.as_ref().map(crate::PlanDocument::document_ref),
            step_id: workflow.as_ref().and_then(|_| {
                session
                    .plan
                    .as_ref()
                    .and_then(|p| p.task.active_step.clone())
            }),
            workflow,
            action_scope,
        })
    }
    pub fn validate(&self, session: &crate::TaskSession) -> Result<()> {
        ensure!(
            self == &Self::capture(session, self.kind)?,
            "Clarification belongs to an earlier document, workflow or action scope; request the operation again"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationPhase {
    PreservingInput,
    CapturingContext,
    CheckingAccount,
    CheckingModels,
    ReadingSource,
    SearchingSource,
    InspectingSource,
    PublicSearch,
    PublicRetrieval,
    Discussing,
    DraftingSpec,
    DraftingPlan,
    PreparingAction,
    Reviewing,
    Revealing,
    Stopping,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub operation_id: String,
    pub context_id: String,
    pub project_fingerprint: String,
    pub research_snapshot_id: String,
    pub model: crate::ResolvedModel,
    pub guidance_fingerprint: String,
    pub workflow: Option<WorkflowBinding>,
    pub spec_ref: Option<DocumentRef>,
    pub plan_ref: Option<DocumentRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummaryCheckpoint {
    pub first_sequence: u64,
    pub covered_sequence: u64,
    pub covered_hash: String,
    pub text: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranscriptContext {
    pub checkpoint: Option<SummaryCheckpoint>,
    pub recent: Vec<ConversationEvent>,
    pub omitted_history_available: bool,
}
