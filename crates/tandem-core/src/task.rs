use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Active,
    Blocked,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub why: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub depends_on: Vec<String>,
    pub done_when: Vec<String>,
    pub status: StepStatus,
    pub evidence: Vec<String>,
    pub completion_note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: String,
    pub project: String,
    pub goal: String,
    #[serde(default)]
    pub analysis: String,
    pub acceptance: Vec<String>,
    pub constraints: Vec<String>,
    pub unknowns: Vec<String>,
    pub steps: Vec<Step>,
    pub revision: u64,
    pub active_step: Option<String>,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub state_sequence: u64,
}

impl TaskPlan {
    pub fn new(
        project: String,
        goal: String,
        acceptance: Vec<String>,
        titles: Vec<String>,
    ) -> Result<Self> {
        let mut previous = None;
        let steps = titles
            .into_iter()
            .map(|title| {
                let id = new_id();
                let step = Step {
                    id: id.clone(),
                    title,
                    why: String::new(),
                    files: Vec::new(),
                    symbols: Vec::new(),
                    languages: Vec::new(),
                    depends_on: previous.iter().cloned().collect(),
                    done_when: Vec::new(),
                    status: StepStatus::Pending,
                    evidence: Vec::new(),
                    completion_note: None,
                };
                previous = Some(id);
                step
            })
            .collect();
        let task = Self {
            id: new_id(),
            project,
            goal,
            analysis: String::new(),
            acceptance,
            constraints: Vec::new(),
            unknowns: Vec::new(),
            steps,
            revision: 1,
            active_step: None,
            updated_at_ms: now_ms(),
            state_sequence: 0,
        };
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.goal.trim().is_empty() && self.goal.len() <= 4096,
            "Enter a goal of at most 4 KiB"
        );
        ensure!(
            !self.steps.is_empty() && self.steps.len() <= 64,
            "Use between 1 and 64 meaningful steps"
        );
        ensure!(
            self.acceptance.len() <= 64 && self.acceptance.iter().all(|s| s.len() <= 4096),
            "Acceptance criteria exceed limits"
        );
        let ids: HashSet<_> = self.steps.iter().map(|s| &s.id).collect();
        ensure!(ids.len() == self.steps.len(), "Step IDs must be unique");
        for step in &self.steps {
            ensure!(
                !step.title.trim().is_empty() && step.title.len() <= 2048,
                "Step titles cannot be empty or oversized"
            );
            ensure!(
                step.depends_on
                    .iter()
                    .all(|id| ids.contains(id) && id != &step.id),
                "Unknown or self-dependent step"
            );
        }
        let graph: HashMap<_, _> = self
            .steps
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.depends_on.iter().map(String::as_str).collect::<Vec<_>>(),
                )
            })
            .collect();
        fn visit<'a>(
            id: &'a str,
            graph: &HashMap<&'a str, Vec<&'a str>>,
            visiting: &mut HashSet<&'a str>,
            done: &mut HashSet<&'a str>,
        ) -> bool {
            if done.contains(id) {
                return true;
            }
            if !visiting.insert(id) {
                return false;
            }
            for dependency in &graph[id] {
                if !visit(dependency, graph, visiting, done) {
                    return false;
                }
            }
            visiting.remove(id);
            done.insert(id);
            true
        }
        let mut done = HashSet::new();
        for id in graph.keys() {
            ensure!(
                visit(id, &graph, &mut HashSet::new(), &mut done),
                "Step dependencies contain a cycle"
            );
        }
        ensure!(
            self.active_step.as_ref().is_none_or(|id| ids.contains(id)),
            "Unknown active step"
        );
        Ok(())
    }

    pub fn start_step(&mut self, id: &str) -> Result<()> {
        let step = self
            .steps
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown step"))?;
        ensure!(
            step.depends_on.iter().all(|dependency| self
                .steps
                .iter()
                .any(|s| &s.id == dependency && s.status == StepStatus::Complete)),
            "Complete this step's dependencies first"
        );
        self.state_sequence = self
            .state_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Task sequence exhausted"))?;
        for step in &mut self.steps {
            if step.status == StepStatus::Active {
                step.status = StepStatus::Pending;
            }
            if step.id == id {
                step.status = StepStatus::Active;
            }
        }
        self.active_step = Some(id.into());
        self.updated_at_ms = now_ms();
        Ok(())
    }

    pub fn complete_active_manually(&mut self, evidence: Vec<String>) -> Result<()> {
        let id = self
            .active_step
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Start a step first"))?;
        let step = self
            .steps
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown step"))?;
        self.state_sequence = self
            .state_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Task sequence exhausted"))?;
        step.status = StepStatus::Complete;
        step.evidence = evidence;
        step.completion_note =
            Some("Marked complete by the user; not an automatic correctness verdict.".into());
        self.active_step = None;
        self.updated_at_ms = now_ms();
        Ok(())
    }

    pub fn revise(
        &mut self,
        goal: String,
        acceptance: Vec<String>,
        titles: Vec<String>,
    ) -> Result<()> {
        let mut revised = Self::new(self.project.clone(), goal, acceptance, titles)?;
        revised.id = self.id.clone();
        revised.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Plan revision exhausted"))?;
        revised.state_sequence = self
            .state_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Task sequence exhausted"))?;
        *self = revised;
        Ok(())
    }

    pub(crate) fn definition_content(&self) -> serde_json::Value {
        serde_json::json!({"goal":self.goal,"analysis":self.analysis,"acceptance":self.acceptance,"constraints":self.constraints,"unknowns":self.unknowns,
            "steps":self.steps.iter().map(|s|serde_json::json!({"id":s.id,"title":s.title,"why":s.why,"files":s.files,"symbols":s.symbols,"languages":s.languages,"depends_on":s.depends_on,"done_when":s.done_when})).collect::<Vec<_>>()})
    }

    pub fn definition_hash(&self) -> String {
        crate::content_hash(self.definition_content().to_string().as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Preparing,
    Running,
    Passed,
    Failed,
    Unvalidated,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotFile {
    pub path: String,
    pub hash: String,
    pub bytes: u64,
    pub executable: bool,
    pub buffer_revision: Option<u64>,
    pub original_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OmittedInput {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: String,
    pub project: String,
    pub files: Vec<SnapshotFile>,
    pub omitted: Vec<OmittedInput>,
    pub total_bytes: u64,
    pub captured_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckDiagnostic {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub level: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckRecord {
    pub id: String,
    pub project: String,
    pub task_id: Option<String>,
    pub step_id: Option<String>,
    pub plan_revision: Option<u64>,
    pub recipe_id: String,
    pub recipe_fingerprint: String,
    pub status: CheckStatus,
    pub sequence: u64,
    pub snapshot: Option<SnapshotManifest>,
    pub command: Vec<String>,
    pub executable_version: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub exit_code: Option<i64>,
    pub output: String,
    pub output_truncated: bool,
    pub diagnostics: Vec<CheckDiagnostic>,
    pub detail: String,
    pub cancel_requested: bool,
    pub snapshot_unchanged: Option<bool>,
    pub log_path: Option<String>,
    #[serde(default)]
    pub benchmark: Option<crate::BenchmarkMeasurement>,
}

impl CheckRecord {
    pub fn pending(project: String, recipe_id: String, fingerprint: String) -> Self {
        Self {
            id: new_id(),
            project,
            task_id: None,
            step_id: None,
            plan_revision: None,
            recipe_id,
            recipe_fingerprint: fingerprint,
            status: CheckStatus::Preparing,
            sequence: 0,
            snapshot: None,
            command: Vec::new(),
            executable_version: String::new(),
            started_at_ms: now_ms(),
            completed_at_ms: None,
            exit_code: None,
            output: String::new(),
            output_truncated: false,
            diagnostics: Vec::new(),
            detail: "Preparing captured inputs".into(),
            cancel_requested: false,
            snapshot_unchanged: None,
            log_path: None,
            benchmark: None,
        }
    }
    pub fn finish(&mut self, status: CheckStatus, detail: impl Into<String>) {
        self.status = status;
        self.detail = detail.into();
        self.completed_at_ms = Some(now_ms());
        self.sequence += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_dependencies_and_revision_preserve_honest_completion() {
        let mut task = TaskPlan::new(
            "project".into(),
            "Implement lookup".into(),
            vec!["Handles empty input".into()],
            vec!["Define contract".into(), "Implement".into()],
        )
        .unwrap();
        let second = task.steps[1].id.clone();
        assert!(task.start_step(&second).is_err());
        task.start_step(&task.steps[0].id.clone()).unwrap();
        task.complete_active_manually(Vec::new()).unwrap();
        assert!(
            task.steps[0]
                .completion_note
                .as_ref()
                .unwrap()
                .contains("not an automatic")
        );
        task.start_step(&second).unwrap();
        task.revise("New contract".into(), vec![], vec!["Reassess".into()])
            .unwrap();
        assert_eq!(task.revision, 2);
        assert_eq!(task.steps[0].status, StepStatus::Pending);
        assert!(task.active_step.is_none());
    }
    #[test]
    fn cyclic_or_unknown_dependencies_are_rejected() {
        let mut task = TaskPlan::new(
            "p".into(),
            "Goal".into(),
            vec![],
            vec!["A".into(), "B".into()],
        )
        .unwrap();
        let second = task.steps[1].id.clone();
        task.steps[0].depends_on.push(second);
        assert!(task.validate().is_err());
    }
}
