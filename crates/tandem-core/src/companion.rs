use crate::*;
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompanionAction {
    Discuss,
    DraftSpec,
    DraftPlan,
    PrepareWalkthrough,
    ReviewAction,
    Analyze,
    Guide,
    Hint,
    Explain,
    Ask,
    Reveal,
    Review,
    Delegate,
    ImproveClarity,
    ImproveSpeed,
    ImproveBalanced,
}
impl CompanionAction {
    pub fn permits_solution(self) -> bool {
        matches!(self, Self::Reveal | Self::Delegate)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelStatus {
    Preparing,
    Running,
    Completed,
    Cancelled,
    Interrupted,
    Failed,
    Quarantined,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextFile {
    pub path: String,
    pub language: String,
    pub hash: String,
    pub text: String,
    pub unsaved: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub id: String,
    pub project_fingerprint: String,
    pub files: Vec<ContextFile>,
    pub tree: Vec<String>,
    pub omitted: Vec<OmittedInput>,
    pub captured_at_ms: i64,
    #[serde(default)]
    pub validation_environments: BTreeMap<String, String>,
}
impl ContextSnapshot {
    pub fn manifest(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .map(|f| (f.path.clone(), f.hash.clone()))
            .collect()
    }
    pub fn delta(&self, previous: &BTreeMap<String, String>) -> Value {
        serde_json::json!({"manifest":self.manifest(),"changed_files":self.files.iter().filter(|f|previous.get(&f.path)!=Some(&f.hash)).collect::<Vec<_>>(),
            "removed_files":previous.keys().filter(|p|!self.files.iter().any(|f|&f.path==*p)).collect::<Vec<_>>(),"project_fingerprint":self.project_fingerprint,"tree":self.tree,"omitted":self.omitted})
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewBinding {
    pub context_id: String,
    pub project_fingerprint: String,
    pub task_id: Option<String>,
    pub plan_revision: u64,
    pub step_id: Option<String>,
    pub language: String,
    pub guidance_fingerprint: String,
}
impl ReviewBinding {
    pub fn current(
        &self,
        context: &ContextSnapshot,
        task: Option<&TaskPlan>,
        guidance: &EffectiveGuidance,
        language: &str,
    ) -> bool {
        self.context_id == context.id
            && self.project_fingerprint == context.project_fingerprint
            && self.task_id.as_deref() == task.map(|t| t.id.as_str())
            && self.plan_revision == task.map_or(0, |t| t.revision)
            && self.step_id == task.and_then(|t| t.active_step.clone())
            && self.language == language
            && self.guidance_fingerprint == guidance.fingerprint()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposedStep {
    pub title: String,
    pub why: String,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub languages: Vec<String>,
    pub depends_on: Vec<usize>,
    pub done_when: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskProposal {
    pub goal: String,
    pub architecture: String,
    pub constraints: Vec<String>,
    pub unknowns: Vec<String>,
    pub acceptance: Vec<String>,
    pub steps: Vec<ProposedStep>,
    pub requested_context: Vec<String>,
}
impl TaskProposal {
    pub fn into_plan(&self, project: &str, previous: Option<&TaskPlan>) -> Result<TaskPlan> {
        ensure!(
            !self.acceptance.is_empty() && self.acceptance.len() <= 32,
            "A plan needs bounded acceptance criteria"
        );
        ensure!(
            !self.steps.is_empty() && self.steps.len() <= 32,
            "A plan needs 1–32 meaningful steps"
        );
        let mut task = TaskPlan::new(
            project.into(),
            self.goal.clone(),
            self.acceptance.clone(),
            self.steps.iter().map(|s| s.title.clone()).collect(),
        )?;
        task.analysis = self.architecture.clone();
        task.constraints = self.constraints.clone();
        task.unknowns = self.unknowns.clone();
        let ids = task.steps.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        for (step, proposal) in task.steps.iter_mut().zip(&self.steps) {
            step.why = proposal.why.clone();
            step.files = proposal.files.clone();
            step.symbols = proposal.symbols.clone();
            step.languages = proposal.languages.clone();
            step.done_when = proposal.done_when.clone();
            ensure!(
                !step.why.trim().is_empty() && !step.done_when.is_empty(),
                "Every step needs a reason and completion conditions"
            );
            for path in &step.files {
                normalized_relative(std::path::Path::new(path))?;
            }
            step.depends_on = proposal
                .depends_on
                .iter()
                .map(|i| ids.get(*i).cloned().context("Unknown step dependency"))
                .collect::<Result<_>>()?;
        }
        if let Some(previous) = previous {
            ensure!(
                previous.project == project,
                "Plan belongs to another project"
            );
            task.id = previous.id.clone();
            task.revision = previous.revision + 1;
            task.state_sequence = previous.state_sequence + 1;
        }
        task.validate()?;
        Ok(task)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplanationSection {
    pub title: String,
    pub explanation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuidanceResponse {
    pub summary: String,
    pub sections: Vec<ExplanationSection>,
    pub next_action: String,
    pub requested_context: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestedSolution {
    pub summary: String,
    pub language: String,
    pub path: String,
    pub code: String,
    pub sections: Vec<ExplanationSection>,
    pub tradeoffs: Vec<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum FindingCategory {
    DemonstratedProblem,
    NeedsVerification,
    DesignTradeoff,
    StylePreference,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub category: FindingCategory,
    pub blocking: bool,
    pub problem: String,
    pub why: String,
    pub next_action: String,
    pub path: String,
    pub line: u32,
    pub criterion_indices: Vec<usize>,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum ReviewConclusion {
    RequirementsSatisfied,
    NeedsChanges,
    InsufficientEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewResponse {
    pub conclusion: ReviewConclusion,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
    pub evidence_used: Vec<String>,
    pub validation_needed: Vec<String>,
    pub requested_context: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CompanionResponse {
    Discussion(ConversationReply),
    SpecDraft(SpecDraft),
    PlanDraft(PlanDraft),
    WalkthroughDraft(WalkthroughDraft),
    ActionAssessment(ActionAssessmentDraft),
    Plan(TaskProposal),
    Guidance(GuidanceResponse),
    Solution(SuggestedSolution),
    Review(ReviewResponse),
    Agent(AgentReply),
    Improvement(ImprovementProposal),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewEvidence {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub summary: String,
    pub verified_failure: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationReply {
    pub text: String,
    pub question: Option<ClarificationQuestion>,
    pub findings: Vec<ResearchFinding>,
    /// Requests have no model-chosen receipt identity or execution authority.
    pub research_requests: Vec<ResearchRequestKind>,
}
impl ConversationReply {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.text.trim().is_empty() && self.text.len() <= 16 * 1024,
            "Reply needs bounded prose"
        );
        ensure!(
            !contains_solution_code(&serde_json::json!([
                self.text,
                self.question,
                self.findings
            ])),
            "Implementation code is hidden in discussion"
        );
        ensure!(
            self.findings.len() <= 32 && self.research_requests.len() <= 4,
            "Reply exceeds research bounds"
        );
        if let Some(question) = &self.question {
            question.validate()?;
        }
        for kind in &self.research_requests {
            ResearchRequest::new(kind.clone())?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecDraft {
    pub reply: ConversationReply,
    pub spec: Option<SpecDocument>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDraft {
    pub reply: ConversationReply,
    pub plan: Option<TaskProposal>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WalkthroughDraft {
    pub reply: ConversationReply,
    pub actions: Option<Vec<GuidedAction>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionAssessment {
    pub outcome: ActionReviewOutcome,
    pub summary: String,
    pub findings: Vec<ActionReviewFinding>,
    pub evidence_used: Vec<String>,
    pub limitations: Vec<String>,
    pub assessments: Vec<ActionCriterionAssessment>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionAssessmentDraft {
    pub reply: ConversationReply,
    pub assessment: Option<ActionAssessment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuidedContext {
    pub operation_id: String,
    pub session: TaskSession,
    pub workflow: Option<WorkflowBinding>,
    pub transcript: TranscriptContext,
    pub receipts: Vec<ResearchReceipt>,
    pub review: Option<ActionReviewInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    #[serde(default)]
    pub model_preference: ModelPreference,
    /// A logical operation resolves once; follow-up subturns copy this snapshot.
    #[serde(default)]
    pub resolved_model: Option<ResolvedModel>,
    /// TaskSession id for managed conversations. Legacy callers may supply their
    /// retained conversation id until explicitly adopted into a TaskSession.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Host-created operation snapshot, never model authority.
    #[serde(default)]
    pub guided: Option<GuidedContext>,
    pub action: CompanionAction,
    pub question: String,
    pub language: String,
    pub guidance: EffectiveGuidance,
    pub task: Option<TaskPlan>,
    pub context: ContextSnapshot,
    pub evidence: Vec<ReviewEvidence>,
    pub prior_decisions: Vec<String>,
    #[serde(default)]
    pub delegation: Option<DelegationInput>,
}
impl ModelRequest {
    pub fn validate_guided(&self) -> Result<()> {
        let managed = matches!(
            self.action,
            CompanionAction::Discuss
                | CompanionAction::DraftSpec
                | CompanionAction::DraftPlan
                | CompanionAction::PrepareWalkthrough
                | CompanionAction::ReviewAction
        );
        if !managed && self.guided.is_none() {
            return Ok(());
        }
        let guided = self
            .guided
            .as_ref()
            .context("Managed requests require a durable session snapshot")?;
        ensure!(
            self.conversation_id.as_ref() == Some(&guided.session.id),
            "Conversation identity differs from the durable session"
        );
        ensure!(
            self.delegation.is_none(),
            "Managed conversations carry no mutation grant"
        );
        match self.action {
            CompanionAction::PrepareWalkthrough
            | CompanionAction::ReviewAction
            | CompanionAction::Reveal => {
                let action = match self.action {
                    CompanionAction::PrepareWalkthrough => WorkflowAction::PrepareWalkthrough,
                    CompanionAction::ReviewAction => WorkflowAction::ReviewAction,
                    _ => WorkflowAction::Reveal,
                };
                let binding = guided
                    .workflow
                    .as_ref()
                    .context("Implementation request requires its workflow binding")?;
                ensure!(
                    guided.session.binding_is_current(action, binding),
                    "Implementation request is unauthorized"
                );
                if self.action != CompanionAction::PrepareWalkthrough {
                    let review = guided
                        .review
                        .as_ref()
                        .context("Action evidence/scope is missing")?;
                    review.validate(&guided.session)?;
                    ensure!(
                        review.context.id == self.context.id
                            && review.binding.project_fingerprint
                                == self.context.project_fingerprint
                            && review.binding.language == self.language
                            && review.binding.guidance_fingerprint == self.guidance.fingerprint(),
                        "Action evidence differs from the request snapshot"
                    );
                }
            }
            CompanionAction::DraftPlan => {
                ensure!(
                    guided.session.spec.is_some(),
                    "Save a spec before drafting a plan"
                );
            }
            _ => {}
        }
        Ok(())
    }
    pub fn binding(&self) -> ReviewBinding {
        ReviewBinding {
            context_id: self.context.id.clone(),
            project_fingerprint: self.context.project_fingerprint.clone(),
            task_id: self.task.as_ref().map(|t| t.id.clone()),
            plan_revision: self.task.as_ref().map_or(0, |t| t.revision),
            step_id: self.task.as_ref().and_then(|t| t.active_step.clone()),
            language: self.language.clone(),
            guidance_fingerprint: self.guidance.fingerprint(),
        }
    }
    pub fn schema(&self) -> Value {
        let schema = match self.action {
            CompanionAction::Discuss => schemars::schema_for!(ConversationReply),
            CompanionAction::DraftSpec => schemars::schema_for!(SpecDraft),
            CompanionAction::DraftPlan => schemars::schema_for!(PlanDraft),
            CompanionAction::PrepareWalkthrough => schemars::schema_for!(WalkthroughDraft),
            CompanionAction::ReviewAction => schemars::schema_for!(ActionAssessmentDraft),
            CompanionAction::Analyze => schemars::schema_for!(TaskProposal),
            CompanionAction::Reveal => schemars::schema_for!(SuggestedSolution),
            CompanionAction::Review => schemars::schema_for!(ReviewResponse),
            CompanionAction::Delegate => schemars::schema_for!(AgentReply),
            CompanionAction::ImproveClarity
            | CompanionAction::ImproveSpeed
            | CompanionAction::ImproveBalanced => schemars::schema_for!(ImprovementProposal),
            _ => schemars::schema_for!(GuidanceResponse),
        };
        let mut value = serde_json::to_value(schema).unwrap();
        value.as_object_mut().unwrap().remove("$schema");
        value
    }
    pub fn parse_response(&self, value: Value) -> Result<CompanionResponse> {
        ensure!(
            serde_json::to_vec(&value)?.len() <= 128 * 1024,
            "Companion response exceeded its size limit"
        );
        if !self.action.permits_solution()
            && !matches!(
                self.action,
                CompanionAction::PrepareWalkthrough
                    | CompanionAction::Discuss
                    | CompanionAction::DraftSpec
                    | CompanionAction::DraftPlan
            )
        {
            ensure!(
                !contains_solution_code(&value),
                "Response contained implementation code in a hidden channel; request a solution explicitly to view code"
            );
        }
        let response = match self.action {
            CompanionAction::Discuss => {
                let reply: ConversationReply = serde_json::from_value(value)?;
                self.validate_conversation_reply(&reply)?;
                CompanionResponse::Discussion(reply)
            }
            CompanionAction::DraftSpec => {
                let draft: SpecDraft = serde_json::from_value(value)?;
                self.validate_conversation_reply(&draft.reply)?;
                if let Some(spec) = &draft.spec {
                    spec.validate()?;
                    ensure!(
                        !contains_solution_code(&serde_json::to_value(spec)?),
                        "Implementation code is hidden in specs"
                    );
                }
                CompanionResponse::SpecDraft(draft)
            }
            CompanionAction::DraftPlan => {
                let draft: PlanDraft = serde_json::from_value(value)?;
                self.validate_conversation_reply(&draft.reply)?;
                if let Some(plan) = &draft.plan {
                    plan.into_plan(
                        self.guided
                            .as_ref()
                            .context("Missing session snapshot")?
                            .session
                            .project
                            .as_str(),
                        self.task.as_ref(),
                    )?;
                    ensure!(
                        !contains_solution_code(&serde_json::to_value(plan)?),
                        "Implementation code is hidden in plans"
                    );
                }
                CompanionResponse::PlanDraft(draft)
            }
            CompanionAction::PrepareWalkthrough => {
                let draft: WalkthroughDraft = serde_json::from_value(value)?;
                self.validate_conversation_reply(&draft.reply)?;
                if let Some(actions) = &draft.actions {
                    ensure!(
                        (1..=32).contains(&actions.len()),
                        "A walkthrough needs 1–32 actions"
                    );
                    let session = &self
                        .guided
                        .as_ref()
                        .context("Missing workflow snapshot")?
                        .session;
                    session.authorize(WorkflowAction::PrepareWalkthrough)?;
                    for action in actions {
                        action.validate(
                            session.spec.as_ref().context("Missing spec")?,
                            session.plan.as_ref().context("Missing plan")?,
                        )?;
                        // argv and file paths are data in their typed slots.
                        ensure!(
                            !contains_solution_code(&serde_json::json!([
                                action.what,
                                action.why,
                                action.expected_result
                            ])),
                            "Implementation code is hidden in action prose"
                        );
                    }
                    walkthrough_proposal(session, actions.clone())?;
                }
                CompanionResponse::WalkthroughDraft(draft)
            }
            CompanionAction::ReviewAction => {
                let draft: ActionAssessmentDraft = serde_json::from_value(value)?;
                self.validate_conversation_reply(&draft.reply)?;
                if let Some(assessment) = &draft.assessment {
                    let guided = self.guided.as_ref().context("Missing review snapshot")?;
                    parse_action_review_response(
                        &guided.session,
                        guided.review.as_ref().context("Missing review evidence")?,
                        serde_json::to_value(assessment)?,
                    )?;
                }
                CompanionResponse::ActionAssessment(draft)
            }
            CompanionAction::Delegate => {
                let input = self
                    .delegation
                    .as_ref()
                    .context("Delegation requires an explicit recorded grant")?;
                input.grant.validate()?;
                ensure!(
                    self.task
                        .as_ref()
                        .is_some_and(|t| input.grant.matches_task(t)),
                    "Delegation does not match the accepted plan"
                );
                let reply: AgentReply = serde_json::from_value(value)?;
                ensure!(
                    reply
                        .edits
                        .iter()
                        .all(|e| input.grant.paths.iter().any(|p| p == e.path())),
                    "Agent requested a path outside its grant"
                );
                ensure!(
                    reply
                        .check_requests
                        .iter()
                        .all(|id| input.grant.checks.contains_key(id)),
                    "Agent requested an unapproved recipe"
                );
                CompanionResponse::Agent(reply)
            }
            CompanionAction::ImproveClarity
            | CompanionAction::ImproveSpeed
            | CompanionAction::ImproveBalanced => {
                let proposal: ImprovementProposal = serde_json::from_value(value)?;
                let objective = match self.action {
                    CompanionAction::ImproveClarity => ImprovementObjective::Clarity,
                    CompanionAction::ImproveSpeed => ImprovementObjective::Speed,
                    _ => ImprovementObjective::Balanced,
                };
                ensure!(
                    proposal.objective == objective,
                    "Improvement objective differs from the request"
                );
                ensure!(
                    proposal
                        .evidence_ids
                        .iter()
                        .all(|id| self.evidence.iter().any(|e| &e.id == id)),
                    "Proposal cites unavailable evidence"
                );
                ensure!(
                    !proposal.invariants.is_empty()
                        && proposal.preconditions.len() <= 32
                        && proposal.suggested_steps.len() <= 16,
                    "Proposal needs bounded invariants and steps"
                );
                CompanionResponse::Improvement(proposal)
            }
            CompanionAction::Analyze => {
                let plan: TaskProposal = serde_json::from_value(value)?;
                let _ = plan.into_plan(
                    self.task
                        .as_ref()
                        .map_or("proposal", |t| t.project.as_str()),
                    self.task.as_ref(),
                )?;
                CompanionResponse::Plan(plan)
            }
            CompanionAction::Reveal => {
                let solution: SuggestedSolution = serde_json::from_value(value)?;
                ensure!(
                    solution.language == self.language,
                    "Solution language differs from its authorization"
                );
                ensure!(!solution.code.trim().is_empty(), "No solution was returned");
                if !solution.path.is_empty() {
                    normalized_relative(std::path::Path::new(&solution.path))?;
                }
                CompanionResponse::Solution(solution)
            }
            CompanionAction::Review => {
                let mut review: ReviewResponse = serde_json::from_value(value)?;
                ensure!(review.findings.len() <= 32, "Too many findings");
                let valid_ids = self
                    .evidence
                    .iter()
                    .map(|e| e.id.as_str())
                    .collect::<BTreeSet<_>>();
                for finding in &mut review.findings {
                    if !finding.path.is_empty() {
                        normalized_relative(std::path::Path::new(&finding.path))?;
                    }
                    ensure!(
                        finding
                            .criterion_indices
                            .iter()
                            .all(|i| self.task.as_ref().is_some_and(|t| *i < t.acceptance.len())),
                        "Finding cites an unknown acceptance criterion"
                    );
                    ensure!(
                        finding
                            .evidence_ids
                            .iter()
                            .all(|id| valid_ids.contains(id.as_str())),
                        "Finding cites unavailable evidence"
                    );
                    ensure!(
                        !finding.problem.trim().is_empty()
                            && !finding.why.trim().is_empty()
                            && !finding.next_action.trim().is_empty(),
                        "Finding needs a problem, reason and next action"
                    );
                    if finding.category == FindingCategory::DemonstratedProblem
                        && !finding.evidence_ids.iter().any(|id| {
                            self.evidence
                                .iter()
                                .any(|e| &e.id == id && e.verified_failure)
                        })
                    {
                        finding.category = FindingCategory::NeedsVerification;
                    }
                    if matches!(
                        finding.category,
                        FindingCategory::DesignTradeoff | FindingCategory::StylePreference
                    ) {
                        finding.blocking = false;
                    }
                }
                ensure!(
                    review
                        .evidence_used
                        .iter()
                        .all(|id| valid_ids.contains(id.as_str())),
                    "Review cites unavailable evidence"
                );
                if review.findings.iter().any(|f| f.blocking) {
                    review.conclusion = ReviewConclusion::NeedsChanges;
                }
                CompanionResponse::Review(review)
            }
            _ => CompanionResponse::Guidance(serde_json::from_value(value)?),
        };
        Ok(response)
    }
    fn validate_conversation_reply(&self, reply: &ConversationReply) -> Result<()> {
        reply.validate()?;
        if let Some(guided) = &self.guided {
            for finding in &reply.findings {
                finding.validate(&guided.receipts)?;
            }
            if let Some(question) = &reply.question {
                let mut session = guided.session.clone();
                session.ask_question(question.clone(), None)?;
            }
        } else {
            ensure!(
                reply.findings.is_empty(),
                "Citations require host-supplied receipts"
            );
        }
        Ok(())
    }
}

/// The host stamps walkthrough identity and scope. A separate final task
/// assessment becomes available only after all high-level steps are complete.
pub fn walkthrough_proposal(
    session: &TaskSession,
    actions: Vec<GuidedAction>,
) -> Result<Walkthrough> {
    session.authorize(WorkflowAction::PrepareWalkthrough)?;
    let plan = session.plan.as_ref().context("Missing plan")?;
    let step_id = plan
        .task
        .active_step
        .clone()
        .or_else(|| {
            plan.task
                .steps
                .iter()
                .all(|s| s.status == StepStatus::Complete)
                .then(|| plan.task.steps.last().map(|s| s.id.clone()))
                .flatten()
        })
        .context("No dependency-ready step or final task assessment")?;
    let walkthrough = Walkthrough {
        id: new_id(),
        generation: session.execution_generation,
        step_id,
        plan_ref: plan.document_ref(),
        actions,
        cursor: 0,
    };
    walkthrough.validate(session.spec.as_ref().context("Missing spec")?, plan)?;
    Ok(walkthrough)
}

/// Defense in depth for prose channels, not a proof that every semantic answer
/// can be detected. Solution bodies are structurally absent from these schemas.
pub fn contains_solution_code(value: &Value) -> bool {
    fn suspicious(text: &str) -> bool {
        if text.contains("```") || text.contains("~~~") {
            return true;
        }
        let rust_body = text.contains("fn ") && text.contains('{') && text.contains('}');
        let python_body = text
            .lines()
            .any(|line| line.trim_start().starts_with("def ") && line.ends_with(':'));
        let js_body = (text.contains("function ") || text.contains("=>"))
            && text.contains('{')
            && text.contains('}');
        rust_body || python_body || js_body
    }
    match value {
        Value::String(s) => suspicious(s),
        Value::Array(a) => a.iter().any(contains_solution_code),
        Value::Object(o) => o.values().any(contains_solution_code),
        _ => false,
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRun {
    pub id: String,
    pub project: String,
    pub sequence: u64,
    pub status: ModelStatus,
    pub request: ModelRequest,
    pub binding: ReviewBinding,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    #[serde(default)]
    pub thread_binding: Option<ThreadBinding>,
    pub response: Option<CompanionResponse>,
    pub detail: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub received_bytes: usize,
    pub cancel_requested: bool,
    pub tool_items: usize,
}
impl ModelRun {
    pub fn pending(project: String, request: ModelRequest) -> Self {
        Self {
            id: new_id(),
            project,
            sequence: 0,
            status: ModelStatus::Preparing,
            binding: request.binding(),
            request,
            thread_id: None,
            turn_id: None,
            thread_binding: None,
            response: None,
            detail: "Preparing selected context".into(),
            started_at_ms: now_ms(),
            completed_at_ms: None,
            received_bytes: 0,
            cancel_requested: false,
            tool_items: 0,
        }
    }
    /// Compatibility for the old Analyze-to-task name change. Managed
    /// TaskSessions keep their durable id and do not need this transfer.
    pub fn legacy_task_thread(
        &self,
        task_id: &str,
        preference: &ModelPreference,
    ) -> Option<ThreadBinding> {
        if self.status != ModelStatus::Completed || self.request.action != CompanionAction::Analyze
        {
            return None;
        }
        let model = self.request.resolved_model.as_ref()?;
        if let ModelPreference::Explicit { model: selected } = preference
            && selected != &model.model
        {
            return None;
        }
        let mut binding = self.thread_binding.clone()?;
        let hash = binding.instructions_hash.as_deref()?;
        if self.thread_id.as_ref() != Some(&binding.thread_id)
            || !binding.reusable_for(&self.request.thread_identity(&self.id), model, hash)
        {
            return None;
        }
        binding.identity.as_mut()?.session_id = task_id.to_string();
        Some(binding)
    }
    pub fn finish(&mut self, status: ModelStatus, detail: impl Into<String>) {
        self.status = status;
        self.detail = detail.into();
        self.completed_at_ms = Some(now_ms());
        self.sequence += 1;
    }
    pub fn displayable(
        &self,
        current_settings: &EffectiveGuidance,
        current_language: &str,
    ) -> bool {
        self.status == ModelStatus::Completed
            && self.request.language == current_language
            && self.binding.guidance_fingerprint == current_settings.fingerprint()
            && (!matches!(self.response, Some(CompanionResponse::Solution(_)))
                || self.request.action.permits_solution())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThreadBinding {
    pub thread_id: String,
    pub manifest: BTreeMap<String, String>,
    /// None means Unknown for legacy rows; it never inherits a current choice.
    #[serde(default)]
    pub resolved_model: Option<ResolvedModel>,
    #[serde(default)]
    pub identity: Option<ThreadIdentity>,
    #[serde(default)]
    pub instructions_hash: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadMode {
    PrivateCompanion,
    Delegation,
    PublicResearch,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadIdentity {
    pub session_id: String,
    pub channel: String,
    pub language: String,
    pub mode: ThreadMode,
}
impl ModelRequest {
    pub fn thread_identity(&self, fallback: &str) -> ThreadIdentity {
        ThreadIdentity {
            session_id: self
                .conversation_id
                .clone()
                .or_else(|| self.task.as_ref().map(|t| t.id.clone()))
                .unwrap_or_else(|| fallback.to_string()),
            channel: match self.action {
                CompanionAction::Reveal => "solution",
                CompanionAction::Delegate => "delegation",
                _ => "main",
            }
            .into(),
            language: self.language.clone(),
            mode: if self.action == CompanionAction::Delegate {
                ThreadMode::Delegation
            } else {
                ThreadMode::PrivateCompanion
            },
        }
    }
}
impl ThreadBinding {
    pub fn reusable_for(
        &self,
        identity: &ThreadIdentity,
        model: &ResolvedModel,
        instructions_hash: &str,
    ) -> bool {
        self.identity.as_ref() == Some(identity)
            && self
                .resolved_model
                .as_ref()
                .is_some_and(|m| m.same_configuration(model))
            && self.instructions_hash.as_deref() == Some(instructions_hash)
    }
}

/// Parse an atomic action assessment. Identity and evidence are host supplied;
/// legacy ReviewResponse has no route to an action transition.
pub fn parse_action_review_response(
    session: &crate::TaskSession,
    input: &crate::ActionReviewInput,
    value: Value,
) -> Result<crate::ActionReview> {
    use crate::{
        ActionCriterionAssessment, ActionReview, ActionReviewFinding, ActionReviewOutcome,
    };
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Assessment {
        outcome: ActionReviewOutcome,
        summary: String,
        findings: Vec<ActionReviewFinding>,
        evidence_used: Vec<String>,
        limitations: Vec<String>,
        assessments: Vec<ActionCriterionAssessment>,
    }
    ensure!(
        serde_json::to_vec(&value)?.len() <= 128 * 1024,
        "Action review exceeds response limit"
    );
    ensure!(
        !contains_solution_code(&value),
        "Implementation code is hidden in action reviews"
    );
    let response: Assessment = serde_json::from_value(value)?;
    let mut review = ActionReview {
        id: new_id(),
        binding: input.binding.clone(),
        outcome: response.outcome,
        summary: response.summary,
        findings: response.findings,
        evidence_used: response.evidence_used,
        limitations: response.limitations,
        assessments: response.assessments,
        reviewed_at_ms: now_ms(),
    };
    review.validate_and_gate(session, input)?;
    Ok(review)
}

/// Action metadata contains no free-form executable body. Solution code cannot
/// be smuggled into prose, a symbol, or an inline human command.
pub fn parse_walkthrough_response(
    session: &crate::TaskSession,
    value: Value,
) -> Result<crate::Walkthrough> {
    session.authorize(crate::WorkflowAction::PrepareWalkthrough)?;
    ensure!(
        serde_json::to_vec(&value)?.len() <= 128 * 1024,
        "Walkthrough exceeds response limit"
    );
    ensure!(
        !contains_solution_code(&value),
        "Implementation code is hidden in walkthrough metadata"
    );
    let walk: crate::Walkthrough = serde_json::from_value(value)?;
    walk.validate(
        session.spec.as_ref().context("Missing spec")?,
        session.plan.as_ref().context("Missing plan")?,
    )?;
    ensure!(
        walk.cursor == 0 && walk.generation == session.execution_generation,
        "Walkthrough starts at the current generation"
    );
    Ok(walk)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(action: CompanionAction) -> ModelRequest {
        ModelRequest {
            model_preference: ModelPreference::Automatic,
            resolved_model: None,
            conversation_id: None,
            guided: None,
            delegation: None,
            action,
            question: "Explain".into(),
            language: "rust".into(),
            guidance: GuidanceSettings::default().effective("p", "rust", None),
            task: None,
            context: ContextSnapshot {
                validation_environments: BTreeMap::new(),
                id: "a".into(),
                project_fingerprint: "p1".into(),
                files: vec![],
                tree: vec![],
                omitted: vec![],
                captured_at_ms: 0,
            },
            evidence: vec![],
            prior_decisions: vec![],
        }
    }
    #[test]
    fn hidden_channels_reject_code_and_reveal_never_implies_edit_authority() {
        let input = request(CompanionAction::Explain);
        let value = serde_json::json!({"summary":"fn answer() { 42 }","sections":[],"next_action":"Write it","requested_context":[]});
        assert!(input.parse_response(value).is_err());
        assert!(!input.schema().to_string().contains("SuggestedSolution"));
        let reveal = request(CompanionAction::Reveal);
        let result=reveal.parse_response(serde_json::json!({"summary":"One option","language":"rust","path":"lib.rs","code":"fn answer() -> u32 { 42 }","sections":[],"tradeoffs":[]})).unwrap();
        assert!(matches!(result, CompanionResponse::Solution(_)));
        // No execution grant or editor mutation exists in either result contract.
        let mut run = ModelRun::pending("p".into(), reveal);
        run.response = Some(result);
        run.finish(ModelStatus::Completed, "Complete");
        let mut changed = run.request.guidance.clone();
        changed.explanation = ExplanationDepth::Detailed;
        assert!(!run.displayable(&changed, "rust"));
        assert!(!run.displayable(&run.request.guidance, "python"));
    }
    #[test]
    fn stale_reviews_bind_to_code_plan_and_language_not_text_similarity() {
        let request = request(CompanionAction::Review);
        let binding = request.binding();
        assert!(binding.current(&request.context, None, &request.guidance, "rust"));
        let mut changed = request.context.clone();
        changed.project_fingerprint = "p2".into();
        assert!(!binding.current(&changed, None, &request.guidance, "rust"));
        let response=request.parse_response(serde_json::json!({"conclusion":"RequirementsSatisfied","summary":"Different implementation satisfies the behavior","findings":[],"evidence_used":[],"validation_needed":["Run tests"],"requested_context":[]})).unwrap();
        assert!(matches!(response, CompanionResponse::Review(_)));
    }
    #[test]
    fn context_delta_sends_changes_and_removals() {
        let mut context = request(CompanionAction::Guide).context;
        context.files.push(ContextFile {
            path: "a.rs".into(),
            language: "rust".into(),
            hash: "v1".into(),
            text: "source".into(),
            unsaved: true,
        });
        let manifest = context.manifest();
        assert_eq!(
            context.delta(&manifest)["changed_files"],
            serde_json::json!([])
        );
        context.files.clear();
        assert_eq!(
            context.delta(&manifest)["removed_files"],
            serde_json::json!(["a.rs"])
        );
    }
}
