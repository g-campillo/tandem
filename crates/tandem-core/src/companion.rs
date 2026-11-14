use crate::*;
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompanionAction {
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
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
        if !self.action.permits_solution() {
            ensure!(
                !contains_solution_code(&value),
                "Response contained implementation code in a hidden channel; request a solution explicitly to view code"
            );
        }
        let response = match self.action {
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
            response: None,
            detail: "Preparing selected context".into(),
            started_at_ms: now_ms(),
            completed_at_ms: None,
            received_bytes: 0,
            cancel_requested: false,
            tool_items: 0,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(action: CompanionAction) -> ModelRequest {
        ModelRequest {
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
