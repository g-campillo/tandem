use crate::*;
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

pub const MAX_DELEGATED_FILES: usize = 16;
pub const MAX_DELEGATED_BYTES: usize = 256 * 1024;
pub fn path_identity(path: &str) -> String {
    path.nfc().flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DelegationScope {
    Step,
    Feature,
}
/// Exact current action, independent of the inspection step selected in the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationActionScope {
    pub walkthrough_id: String,
    pub action_id: String,
    pub action_revision: u64,
    pub action_hash: String,
}
impl DelegationActionScope {
    pub fn current(session: &TaskSession) -> Option<Self> {
        session
            .current_action()
            .ok()
            .map(|(walkthrough, action)| Self {
                walkthrough_id: walkthrough.id.clone(),
                action_id: action.id.clone(),
                action_revision: action.revision,
                action_hash: action.definition_hash(),
            })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationGrant {
    pub id: String,
    #[serde(default)]
    pub workflow: Option<WorkflowBinding>,
    #[serde(default)]
    pub action_scope: Option<DelegationActionScope>,
    pub project: String,
    pub task_id: String,
    pub plan_revision: u64,
    pub step_id: Option<String>,
    pub scope: DelegationScope,
    pub objective: String,
    pub paths: Vec<String>,
    pub checks: BTreeMap<String, String>,
    pub max_rounds: u8,
    pub confirmed_at_ms: i64,
}
impl DelegationGrant {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && !self.project.is_empty()
                && !self.task_id.is_empty()
                && self.plan_revision > 0,
            "Delegation needs a project and accepted plan"
        );
        ensure!(
            !self.objective.trim().is_empty() && self.objective.len() <= 16000,
            "Provide a bounded delegation objective"
        );
        ensure!(
            !self.paths.is_empty() && self.paths.len() <= MAX_DELEGATED_FILES,
            "Delegate between 1 and 16 exact source paths"
        );
        ensure!(
            (1..=4).contains(&self.max_rounds),
            "Delegation allows at most four implement/check rounds"
        );
        ensure!(
            self.scope != DelegationScope::Step || self.step_id.is_some(),
            "Select an active step before delegating it"
        );
        let mut identities = BTreeSet::new();
        for path in &self.paths {
            normalized_relative(std::path::Path::new(path))?;
            validate_new_name(path)?;
            ensure!(
                identities.insert(path_identity(path)),
                "Delegated paths collide after case/Unicode normalization"
            );
        }
        for a in &self.paths {
            for b in &self.paths {
                ensure!(
                    a == b || !std::path::Path::new(a).starts_with(b),
                    "A delegated file cannot also be another file's parent"
                );
            }
        }
        ensure!(
            self.checks.len() <= 4,
            "Choose at most four fixed check recipes"
        );
        Ok(())
    }
    /// Historical grants remain readable, but authority requires the current durable session.
    pub fn validate_workflow(&self, session: &TaskSession) -> Result<()> {
        let binding = self
            .workflow
            .as_ref()
            .context("Legacy Takeover grant has no workflow approval; prepare a new scope")?;
        ensure!(
            session.project == self.project
                && session.binding_is_current(WorkflowAction::Takeover, binding),
            "Takeover approvals or execution generation changed; staged work is retained"
        );
        let plan = session
            .plan
            .as_ref()
            .context("Save a plan before Takeover")?;
        ensure!(self.matches_task(&plan.task), "Takeover plan changed");
        ensure!(
            self.scope != DelegationScope::Step || self.step_id == plan.task.active_step,
            "Takeover scope is not the current implementation step"
        );
        ensure!(
            self.action_scope == DelegationActionScope::current(session),
            "Takeover current action changed; prepare a new scope"
        );
        Ok(())
    }
    pub fn matches_task(&self, task: &TaskPlan) -> bool {
        self.project == task.project
            && self.task_id == task.id
            && self.plan_revision == task.revision
            && (self.scope != DelegationScope::Step
                || self
                    .step_id
                    .as_ref()
                    .is_some_and(|id| task.steps.iter().any(|step| &step.id == id)))
    }
    pub fn scope_fingerprint(&self) -> String {
        let mut scope = self.clone();
        scope.confirmed_at_ms = 0;
        content_hash(serde_json::to_string(&scope).unwrap().as_bytes())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextReplacement {
    pub before: String,
    pub after: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", deny_unknown_fields)]
pub enum AgentEdit {
    Modify {
        path: String,
        before_hash: String,
        replacements: Vec<TextReplacement>,
    },
    Create {
        path: String,
        content: String,
    },
}
impl AgentEdit {
    pub fn path(&self) -> &str {
        match self {
            Self::Modify { path, .. } | Self::Create { path, .. } => path,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentReply {
    pub summary: String,
    pub edits: Vec<AgentEdit>,
    pub check_requests: Vec<String>,
    pub done: bool,
    pub limitations: Vec<String>,
    pub next_action: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationInput {
    pub grant: DelegationGrant,
    pub round: u8,
    pub staged_hashes: BTreeMap<String, Option<String>>,
    pub broker_feedback: Vec<String>,
}

/// Validate an entire batch in memory before dispatching any write or command.
pub fn apply_agent_batch(
    grant: &DelegationGrant,
    current: &BTreeMap<String, Option<Document>>,
    reply: &AgentReply,
) -> Result<BTreeMap<String, Document>> {
    grant.validate()?;
    ensure!(
        reply.edits.len() <= MAX_DELEGATED_FILES,
        "Agent edit batch is oversized"
    );
    ensure!(
        reply.check_requests.len() <= 4
            && reply
                .check_requests
                .iter()
                .all(|id| grant.checks.contains_key(id)),
        "Agent requested a check outside the delegated recipes"
    );
    let mut result = BTreeMap::new();
    let mut bytes = 0;
    for edit in &reply.edits {
        let path = edit.path();
        ensure!(
            grant.paths.iter().any(|p| p == path),
            "Agent requested a source path outside the grant"
        );
        ensure!(
            !result.contains_key(path),
            "One edit entry per source file is required"
        );
        let baseline = current
            .get(path)
            .context("Delegated file state is unavailable")?;
        let document = match edit {
            AgentEdit::Modify {
                before_hash,
                replacements,
                ..
            } => {
                let mut doc = baseline
                    .clone()
                    .context("Cannot modify a missing source file; use Create")?;
                ensure!(
                    content_hash(&doc.format.encode(&doc.text)) == *before_hash,
                    "The agent's edit refers to an older staged version"
                );
                ensure!(
                    !replacements.is_empty() && replacements.len() <= 32,
                    "Use 1–32 exact replacements per modified file"
                );
                let mut text = doc.text.clone();
                for replacement in replacements {
                    ensure!(
                        !replacement.before.is_empty()
                            && text.match_indices(&replacement.before).count() == 1,
                        "Replacement context is missing or ambiguous; no batch changes were applied"
                    );
                    text = text.replacen(&replacement.before, &replacement.after, 1);
                }
                ensure!(!text.contains('\0'), "Binary edits are not supported");
                doc.snapshot(text);
                doc
            }
            AgentEdit::Create { content, .. } => {
                ensure!(
                    baseline.is_none(),
                    "A new file would overwrite existing staged work"
                );
                ensure!(!content.contains('\0'), "Binary creation is not supported");
                Document {
                    path: path.into(),
                    text: content.clone(),
                    base_text: String::new(),
                    base_hash: None,
                    format: TextFormat::default(),
                    revision: 1,
                    recovered: false,
                    conflict: false,
                    read_only: false,
                }
            }
        };
        bytes += document.text.len();
        ensure!(
            bytes <= MAX_DELEGATED_BYTES,
            "The edit batch exceeds 256 KiB"
        );
        result.insert(path.into(), document);
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    Preparing,
    Prepared,
    Running,
    Checking,
    ReadyForReview,
    Cancelled,
    Interrupted,
    Failed,
}
impl AgentStatus {
    pub fn active(self) -> bool {
        matches!(self, Self::Preparing | Self::Running | Self::Checking)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: String,
    pub project: String,
    pub sequence: u64,
    pub grant: DelegationGrant,
    pub status: AgentStatus,
    pub directory: String,
    pub baseline_manifest: Option<SnapshotManifest>,
    pub baseline: BTreeMap<String, Option<Document>>,
    pub staged: BTreeMap<String, Option<Document>>,
    pub round: u8,
    pub model_runs: Vec<String>,
    pub checks: Vec<String>,
    pub activity: Vec<String>,
    pub summary: String,
    pub limitations: Vec<String>,
    pub next_action: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub cancel_requested: bool,
    pub stage_verified: bool,
}
impl AgentRun {
    pub fn pending(grant: DelegationGrant, directory: String) -> Self {
        Self {
            id: grant.id.clone(),
            project: grant.project.clone(),
            sequence: 0,
            grant,
            status: AgentStatus::Preparing,
            directory,
            baseline_manifest: None,
            baseline: BTreeMap::new(),
            staged: BTreeMap::new(),
            round: 0,
            model_runs: vec![],
            checks: vec![],
            activity: vec![],
            summary: "Preparing the delegated working copy".into(),
            limitations: vec![],
            next_action: String::new(),
            started_at_ms: now_ms(),
            completed_at_ms: None,
            cancel_requested: false,
            stage_verified: false,
        }
    }
    pub fn activity(&mut self, text: impl Into<String>) {
        self.activity.push(text.into());
        if self.activity.len() > 100 {
            self.activity.remove(0);
        }
        self.sequence += 1;
    }
    pub fn record_check_intent(&mut self, id: String) {
        self.checks.push(id);
        self.sequence += 1;
    }
    pub fn finish(&mut self, status: AgentStatus, summary: impl Into<String>) {
        self.status = status;
        self.summary = summary.into();
        self.completed_at_ms = Some(now_ms());
        self.sequence += 1;
    }
    pub fn changed_paths(&self) -> Vec<String> {
        self.grant
            .paths
            .iter()
            .filter(|path| {
                self.baseline
                    .get(*path)
                    .and_then(Option::as_ref)
                    .map(|d| &d.text)
                    != self
                        .staged
                        .get(*path)
                        .and_then(Option::as_ref)
                        .map(|d| &d.text)
            })
            .cloned()
            .collect()
    }
}
