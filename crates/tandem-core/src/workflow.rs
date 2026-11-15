//! Versioned definitions and pure workflow transitions; persistence revalidates them atomically.
use crate::conversation::{bounded_structure, bounded_text, stable_id};
use crate::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRef {
    pub revision: u64,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecDocument {
    pub revision: u64,
    pub summary: String,
    pub requirements: Vec<Criterion>,
    pub in_scope: Vec<String>,
    pub out_of_scope: Vec<String>,
    pub constraints: Vec<String>,
    pub acceptance: Vec<Criterion>,
}

impl SpecDocument {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.revision > 0, "Spec revision must be positive");
        bounded_text(&self.summary, 4096, "Spec summary")?;
        let mut ids = BTreeSet::new();
        for criteria in [&self.requirements, &self.acceptance] {
            ensure!(
                !criteria.is_empty() && criteria.len() <= 64,
                "A spec needs 1–64 requirements and acceptance criteria"
            );
            for criterion in criteria {
                stable_id(&criterion.id)?;
                ensure!(
                    ids.insert(&criterion.id),
                    "Criterion IDs must be unique across the spec"
                );
                bounded_text(&criterion.text, 4096, "Criterion")?;
            }
        }
        for entries in [&self.in_scope, &self.out_of_scope, &self.constraints] {
            ensure!(entries.len() <= 64, "Spec lists exceed 64 entries");
            for entry in entries {
                bounded_text(entry, 4096, "Spec entry")?;
            }
        }
        bounded_structure(self)
    }

    pub fn definition_hash(&self) -> String {
        // Fixed tuple order is canonical; document revision is identity, not content.
        content_hash(
            &serde_json::to_vec(&(
                &self.summary,
                &self.requirements,
                &self.in_scope,
                &self.out_of_scope,
                &self.constraints,
                &self.acceptance,
            ))
            .expect("String-only spec serializes"),
        )
    }

    pub fn document_ref(&self) -> DocumentRef {
        DocumentRef {
            revision: self.revision,
            hash: self.definition_hash(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDocument {
    pub revision: u64,
    pub spec_ref: DocumentRef,
    pub task: TaskPlan,
}

impl PlanDocument {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.revision > 0 && self.spec_ref.revision > 0 && !self.spec_ref.hash.is_empty(),
            "Plan needs positive revisions and a bound spec"
        );
        stable_id(&self.task.id)?;
        self.task.validate()?;
        for step in &self.task.steps {
            stable_id(&step.id)?;
            ensure!(
                step.done_when.len() <= 64,
                "Step completion conditions exceed 64 entries"
            );
            for criterion in &step.done_when {
                bounded_text(criterion, 4096, "Step completion condition")?;
            }
            for path in &step.files {
                normalized_relative(std::path::Path::new(path))?;
            }
        }
        // Bound the same immutable fields used by definition_hash, retaining the
        // full task definition here rather than its fixed-size hash. Runtime
        // progress can grow without changing the approved document identity.
        bounded_structure(&(
            &self.spec_ref,
            &self.task.id,
            &self.task.project,
            self.task.definition_content(),
        ))
    }

    pub fn definition_hash(&self) -> String {
        content_hash(
            &serde_json::to_vec(&(
                &self.spec_ref,
                &self.task.id,
                &self.task.project,
                self.task.definition_hash(),
            ))
            .expect("String-only plan identity serializes"),
        )
    }

    pub fn document_ref(&self) -> DocumentRef {
        DocumentRef {
            revision: self.revision,
            hash: self.definition_hash(),
        }
    }
}

/// Only a human document-approval command may create this record. Model proposals
/// use DocumentProposal, a deliberately separate contract without approval fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentApproval {
    pub document_ref: DocumentRef,
    pub approved_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "document", deny_unknown_fields)]
pub enum DocumentProposal {
    Spec(SpecDocument),
    Plan(PlanDocument),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowAction {
    Discuss,
    Research,
    DraftSpec,
    DraftPlan,
    PrepareWalkthrough,
    PrepareAction,
    ReviewAction,
    Reveal,
    Takeover,
    AdvanceAction,
    ValidateStep,
    ValidateTask,
}

impl WorkflowAction {
    pub fn requires_implementation(self) -> bool {
        !matches!(
            self,
            Self::Discuss | Self::Research | Self::DraftSpec | Self::DraftPlan
        )
    }
}

/// For discussion/research the optional definition fields are absent. Such a
/// binding never compares equal to an implementation authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBinding {
    pub session_id: String,
    pub task_id: Option<String>,
    pub spec_ref: Option<DocumentRef>,
    pub plan_ref: Option<DocumentRef>,
    pub execution_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowDenial {
    MissingSpec,
    MissingPlan,
    OutdatedPlan,
    SpecNotApproved,
    PlanNotApproved,
    NotStarted,
    InvalidDocuments(String),
}

impl std::fmt::Display for WorkflowDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::MissingSpec => "Save a spec before implementation",
            Self::MissingPlan => "Save a plan before implementation",
            Self::OutdatedPlan => "The plan is based on an older spec; revise it first",
            Self::SpecNotApproved => "Approve the current spec before implementation",
            Self::PlanNotApproved => "Approve the current plan before implementation",
            Self::NotStarted => "Choose Start Implementation first",
            Self::InvalidDocuments(detail) => {
                return write!(f, "Invalid workflow documents: {detail}");
            }
        };
        f.write_str(message)
    }
}
impl std::error::Error for WorkflowDenial {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionArchive {
    pub generation: u64,
    pub spec: Option<SpecDocument>,
    pub plan: Option<PlanDocument>,
    pub walkthroughs: Vec<Walkthrough>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSession {
    pub id: String,
    pub project: String,
    pub sequence: u64,
    pub spec: Option<SpecDocument>,
    pub plan: Option<PlanDocument>,
    pub spec_approval: Option<DocumentApproval>,
    pub plan_approval: Option<DocumentApproval>,
    pub execution_generation: u64,
    pub started_generation: Option<u64>,
    pub pending_question: Option<ClarificationQuestion>,
    pub walkthroughs: Vec<Walkthrough>,
    pub answered_questions: Vec<ClarificationAnswer>,
    pub history: Vec<ExecutionArchive>,
}

impl TaskSession {
    pub fn new(project: String) -> Result<Self> {
        bounded_text(&project, 4096, "Project")?;
        Ok(Self {
            id: new_id(),
            project,
            sequence: 0,
            spec: None,
            plan: None,
            spec_approval: None,
            plan_approval: None,
            execution_generation: 1,
            started_generation: None,
            pending_question: None,
            walkthroughs: vec![],
            answered_questions: vec![],
            history: vec![],
        })
    }

    fn event(
        &mut self,
        payload: ConversationPayload,
        run_id: Option<String>,
    ) -> Result<ConversationEvent> {
        let sequence = self
            .sequence
            .checked_add(1)
            .context("Session sequence exhausted")?;
        let event = ConversationEvent {
            id: new_id(),
            session_id: self.id.clone(),
            sequence,
            created_at_ms: now_ms(),
            run_id,
            payload,
        };
        self.sequence = sequence;
        Ok(event)
    }

    pub fn append_human_message(&mut self, text: String) -> Result<ConversationEvent> {
        bounded_text(&text, MAX_HUMAN_MESSAGE_BYTES, "Human message")?;
        self.event(ConversationPayload::HumanMessage { text }, None)
    }

    pub fn append_assistant_message(
        &mut self,
        text: String,
        run_id: String,
    ) -> Result<ConversationEvent> {
        bounded_text(&text, MAX_STRUCTURED_RESPONSE_BYTES, "Assistant message")?;
        self.event(ConversationPayload::AssistantMessage { text }, Some(run_id))
    }

    pub fn ask_question(
        &mut self,
        question: ClarificationQuestion,
        run_id: Option<String>,
    ) -> Result<ConversationEvent> {
        question.validate()?;
        ensure!(
            self.pending_question.is_none(),
            "Answer the pending question first"
        );
        ensure!(
            !self
                .answered_questions
                .iter()
                .any(|a| a.question_id == question.id),
            "Question ID was already answered"
        );
        let event = self.event(
            ConversationPayload::QuestionAsked {
                question: question.clone(),
            },
            run_id,
        )?;
        self.pending_question = Some(question);
        Ok(event)
    }

    pub fn answer_question(&mut self, answer: ClarificationAnswer) -> Result<ConversationEvent> {
        ensure!(
            !self
                .answered_questions
                .iter()
                .any(|a| a.question_id == answer.question_id),
            "Question was already answered"
        );
        answer.validate(
            self.pending_question
                .as_ref()
                .context("There is no pending question")?,
        )?;
        let event = self.event(
            ConversationPayload::QuestionAnswered {
                answer: answer.clone(),
            },
            None,
        )?;
        self.answered_questions.push(answer);
        self.pending_question = None;
        Ok(event)
    }

    pub fn propose_spec(
        &mut self,
        mut spec: SpecDocument,
        run_id: Option<String>,
    ) -> Result<ConversationEvent> {
        spec.revision = self.next_spec_revision()?;
        spec.validate()?;
        self.event(
            ConversationPayload::DocumentProposed {
                proposal: DocumentProposal::Spec(spec),
            },
            run_id,
        )
    }

    pub fn propose_plan(
        &mut self,
        task: TaskPlan,
        run_id: Option<String>,
    ) -> Result<ConversationEvent> {
        let spec_ref = self
            .spec
            .as_ref()
            .context("Save a spec before drafting a plan")?
            .document_ref();
        let mut plan = PlanDocument {
            revision: self.next_plan_revision()?,
            spec_ref,
            task,
        };
        ensure!(
            plan.task.steps.len() <= 32,
            "Generated plans need 1–32 high-level steps"
        );
        // Host owns task identity even when a fresh proposal contains a new ID.
        if let Some(current) = &self.plan {
            plan.task.id = current.task.id.clone();
        }
        self.validate_plan(&plan)?;
        reset_progress(&mut plan.task, plan.revision);
        self.event(
            ConversationPayload::DocumentProposed {
                proposal: DocumentProposal::Plan(plan),
            },
            run_id,
        )
    }

    /// Human Save command. A model proposal alone must never call this transition.
    pub fn save_spec(&mut self, mut spec: SpecDocument) -> Result<Option<ConversationEvent>> {
        spec.validate()?;
        if self
            .spec
            .as_ref()
            .is_some_and(|old| old.definition_hash() == spec.definition_hash())
        {
            return Ok(None);
        }
        spec.revision = self.next_spec_revision()?;
        let generation = self.replacement_generation(
            self.spec_approval.is_some()
                || self.plan_approval.is_some()
                || self.started_generation.is_some(),
        )?;
        let event = self.event(
            ConversationPayload::DocumentSaved {
                kind: WorkflowDocumentKind::Spec,
                document_ref: spec.document_ref(),
            },
            None,
        )?;
        self.archive_execution();
        self.spec = Some(spec);
        self.spec_approval = None;
        self.plan_approval = None;
        self.started_generation = None;
        self.execution_generation = generation;
        Ok(Some(event))
    }

    /// Human Save command; a stale proposed spec binding cannot be rebound silently.
    pub fn save_plan(&mut self, mut plan: PlanDocument) -> Result<Option<ConversationEvent>> {
        self.validate_plan(&plan)?;
        if let Some(old) = &self.plan {
            ensure!(
                old.task.id == plan.task.id,
                "Plan replacement must retain the session's task ID"
            );
            if old.definition_hash() == plan.definition_hash() {
                return Ok(None);
            }
        }
        plan.revision = self.next_plan_revision()?;
        let state_sequence = self
            .plan
            .as_ref()
            .map_or(Some(0), |current| {
                current.task.state_sequence.checked_add(1)
            })
            .context("Task sequence exhausted")?;
        reset_progress(&mut plan.task, plan.revision);
        plan.task.state_sequence = state_sequence;
        let generation = self.replacement_generation(
            self.plan.is_some()
                && (self.plan_approval.is_some() || self.started_generation.is_some()),
        )?;
        let event = self.event(
            ConversationPayload::DocumentSaved {
                kind: WorkflowDocumentKind::Plan,
                document_ref: plan.document_ref(),
            },
            None,
        )?;
        self.archive_execution();
        self.plan = Some(plan);
        self.plan_approval = None;
        self.started_generation = None;
        self.execution_generation = generation;
        Ok(Some(event))
    }

    /// Human UI command, bound to the exact reference displayed to that person.
    pub fn approve_spec(
        &mut self,
        displayed_ref: &DocumentRef,
        approved_at_ms: i64,
    ) -> Result<ConversationEvent> {
        let spec = self.spec.as_ref().context("Save a spec first")?;
        spec.validate()?;
        ensure!(
            &spec.document_ref() == displayed_ref,
            "Displayed spec is stale; inspect the current revision"
        );
        let approval = DocumentApproval {
            document_ref: displayed_ref.clone(),
            approved_at_ms,
        };
        let event = self.event(
            ConversationPayload::DocumentApproved {
                kind: WorkflowDocumentKind::Spec,
                approval: approval.clone(),
            },
            None,
        )?;
        self.spec_approval = Some(approval);
        Ok(event)
    }

    /// Human UI command. This does not imply approval of the referenced spec.
    pub fn approve_plan(
        &mut self,
        displayed_ref: &DocumentRef,
        approved_at_ms: i64,
    ) -> Result<ConversationEvent> {
        let plan = self.plan.as_ref().context("Save a plan first")?;
        self.validate_plan(plan)?;
        ensure!(
            &plan.document_ref() == displayed_ref,
            "Displayed plan is stale; inspect the current revision"
        );
        let approval = DocumentApproval {
            document_ref: displayed_ref.clone(),
            approved_at_ms,
        };
        let event = self.event(
            ConversationPayload::DocumentApproved {
                kind: WorkflowDocumentKind::Plan,
                approval: approval.clone(),
            },
            None,
        )?;
        self.plan_approval = Some(approval);
        Ok(event)
    }

    pub fn start_implementation(&mut self) -> Result<ConversationEvent> {
        let binding = self.approved_binding()?;
        ensure!(
            self.started_generation != Some(self.execution_generation),
            "Implementation is already started"
        );
        let mut plan = self.plan.clone().context("Save a plan first")?;
        let step_id = plan
            .task
            .steps
            .iter()
            .find(|step| {
                step.status != StepStatus::Complete
                    && step.depends_on.iter().all(|id| {
                        plan.task.steps.iter().any(|dependency| {
                            &dependency.id == id && dependency.status == StepStatus::Complete
                        })
                    })
            })
            .context("No unfinished dependency-ready step is available")?
            .id
            .clone();
        plan.task.start_step(&step_id)?;
        let event = self.event(
            ConversationPayload::WorkflowTransition {
                transition: WorkflowTransition::ImplementationStarted { binding, step_id },
            },
            None,
        )?;
        self.plan = Some(plan);
        self.started_generation = Some(self.execution_generation);
        Ok(event)
    }

    pub fn authorize(
        &self,
        action: WorkflowAction,
    ) -> std::result::Result<WorkflowBinding, WorkflowDenial> {
        if !action.requires_implementation() {
            return Ok(WorkflowBinding {
                session_id: self.id.clone(),
                task_id: None,
                spec_ref: None,
                plan_ref: None,
                execution_generation: self.execution_generation,
            });
        }
        let binding = self.approved_binding()?;
        if self.started_generation != Some(self.execution_generation) {
            return Err(WorkflowDenial::NotStarted);
        }
        Ok(binding)
    }

    pub fn binding_is_current(&self, action: WorkflowAction, binding: &WorkflowBinding) -> bool {
        self.authorize(action)
            .is_ok_and(|current| &current == binding)
    }

    fn approved_binding(&self) -> std::result::Result<WorkflowBinding, WorkflowDenial> {
        let spec = self.spec.as_ref().ok_or(WorkflowDenial::MissingSpec)?;
        let plan = self.plan.as_ref().ok_or(WorkflowDenial::MissingPlan)?;
        if plan.spec_ref != spec.document_ref() {
            return Err(WorkflowDenial::OutdatedPlan);
        }
        spec.validate()
            .and_then(|_| self.validate_plan(plan))
            .map_err(|error| WorkflowDenial::InvalidDocuments(error.to_string()))?;
        let spec_ref = spec.document_ref();
        let plan_ref = plan.document_ref();
        if self
            .spec_approval
            .as_ref()
            .is_none_or(|a| a.document_ref != spec_ref)
        {
            return Err(WorkflowDenial::SpecNotApproved);
        }
        if self
            .plan_approval
            .as_ref()
            .is_none_or(|a| a.document_ref != plan_ref)
        {
            return Err(WorkflowDenial::PlanNotApproved);
        }
        Ok(WorkflowBinding {
            session_id: self.id.clone(),
            task_id: Some(plan.task.id.clone()),
            spec_ref: Some(spec_ref),
            plan_ref: Some(plan_ref),
            execution_generation: self.execution_generation,
        })
    }

    fn validate_plan(&self, plan: &PlanDocument) -> Result<()> {
        plan.validate()?;
        let spec = self
            .spec
            .as_ref()
            .context("Save a spec before saving a plan")?;
        spec.validate()?;
        ensure!(
            plan.spec_ref == spec.document_ref(),
            "The plan is based on an older spec"
        );
        ensure!(
            plan.task.project == self.project,
            "Plan belongs to another project"
        );
        Ok(())
    }

    fn next_spec_revision(&self) -> Result<u64> {
        self.spec
            .as_ref()
            .map_or(Some(1), |d| d.revision.checked_add(1))
            .context("Spec revision exhausted")
    }

    fn next_plan_revision(&self) -> Result<u64> {
        self.plan
            .as_ref()
            .map_or(Some(1), |d| d.revision.checked_add(1))
            .context("Plan revision exhausted")
    }

    fn replacement_generation(&self, replaces_approved_definition: bool) -> Result<u64> {
        if replaces_approved_definition {
            self.execution_generation
                .checked_add(1)
                .context("Execution generation exhausted")
        } else {
            Ok(self.execution_generation)
        }
    }

    fn archive_execution(&mut self) {
        if self.plan.is_some() || !self.walkthroughs.is_empty() {
            self.history.push(ExecutionArchive {
                generation: self.execution_generation,
                spec: self.spec.clone(),
                plan: self.plan.clone(),
                walkthroughs: std::mem::take(&mut self.walkthroughs),
            });
        }
    }
}

fn reset_progress(task: &mut TaskPlan, revision: u64) {
    task.revision = revision;
    task.state_sequence = 0;
    task.updated_at_ms = now_ms();
    task.active_step = None;
    for step in &mut task.steps {
        step.status = StepStatus::Pending;
        step.evidence.clear();
        step.completion_note = None;
    }
}
