//! The Claude chat: one task per feature, taken from discussion to plan to build.
use crate::{new_id, normalized_relative, now_ms};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const DEFAULT_MODEL: &str = "default";
pub const NEW_TASK_TITLE: &str = "New task";

const MAX_GOAL_CHARS: usize = 500;
const MAX_DONE_WHEN: usize = 10;
const MAX_DONE_WHEN_CHARS: usize = 300;
const MAX_STEPS: usize = 40;
const MAX_SUMMARY_CHARS: usize = 200;
const MAX_TARGET_CHARS: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelpLevel {
    #[default]
    Beginner,
    Intermediate,
    Expert,
}

impl HelpLevel {
    pub const ALL: [HelpLevel; 3] = [
        HelpLevel::Beginner,
        HelpLevel::Intermediate,
        HelpLevel::Expert,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HelpLevel::Beginner => "Beginner",
            HelpLevel::Intermediate => "Intermediate",
            HelpLevel::Expert => "Expert",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            HelpLevel::Beginner => "beginner",
            HelpLevel::Intermediate => "intermediate",
            HelpLevel::Expert => "expert",
        }
    }
}

/// Languages that always get a help-level row, as (id, label).
pub const HELP_LANGUAGES: [(&str, &str); 7] = [
    ("rust", "Rust"),
    ("go", "Go"),
    ("typescript", "TypeScript"),
    ("python", "Python"),
    ("java", "Java"),
    ("javascript", "JavaScript"),
    ("shell", "Shell"),
];

pub fn language_for_path(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "java" => "java",
        "sh" | "bash" | "zsh" => "shell",
        _ => return None,
    })
}

pub fn normalize_language(raw: &str) -> String {
    let language = raw.trim().to_lowercase();
    match language.as_str() {
        "ts" => "typescript".into(),
        "js" => "javascript".into(),
        "py" => "python".into(),
        "golang" => "go".into(),
        "" | "bash" | "sh" | "zsh" | "terminal" => "shell".into(),
        _ => language,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatPrefs {
    pub model: String,
    pub effort: Option<String>,
    pub use_my_setup: bool,
    pub help_levels: BTreeMap<String, HelpLevel>,
}

impl Default for ChatPrefs {
    fn default() -> Self {
        ChatPrefs {
            model: DEFAULT_MODEL.into(),
            effort: None,
            use_my_setup: true,
            help_levels: BTreeMap::new(),
        }
    }
}

impl ChatPrefs {
    /// The saved level for `language`, or Beginner when none was chosen.
    pub fn help_level(&self, language: &str) -> HelpLevel {
        self.help_levels.get(language).copied().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Discuss,
    Plan,
    Build,
    Done,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Guided,
    Takeover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    CreateFile,
    EditFile,
    RunCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub kind: StepKind,
    pub target: String,
    pub summary: String,
    pub language: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    pub done_when: Vec<String>,
    pub steps: Vec<PlanStep>,
}

impl Plan {
    /// Trims and validates a proposed plan; errors are worded for Claude to correct.
    pub fn normalized(self) -> Result<Plan> {
        let goal = self.goal.trim().to_string();
        ensure!(
            within(&goal, MAX_GOAL_CHARS),
            "The plan needs a goal of 1 to {MAX_GOAL_CHARS} characters."
        );
        ensure!(
            (1..=MAX_DONE_WHEN).contains(&self.done_when.len()),
            "The plan needs 1 to {MAX_DONE_WHEN} \"done when\" criteria, not {}.",
            self.done_when.len()
        );
        let mut done_when = Vec::with_capacity(self.done_when.len());
        for (index, criterion) in self.done_when.iter().enumerate() {
            let criterion = criterion.trim();
            ensure!(
                within(criterion, MAX_DONE_WHEN_CHARS),
                "The \"done when\" criterion {} must be 1 to {MAX_DONE_WHEN_CHARS} characters.",
                index + 1
            );
            done_when.push(criterion.to_string());
        }
        ensure!(
            (1..=MAX_STEPS).contains(&self.steps.len()),
            "The plan needs 1 to {MAX_STEPS} steps, not {}.",
            self.steps.len()
        );
        let steps = self
            .steps
            .into_iter()
            .enumerate()
            .map(|(index, step)| step.normalized(index + 1))
            .collect::<Result<Vec<_>>>()?;
        Ok(Plan {
            goal,
            done_when,
            steps,
        })
    }
}

impl PlanStep {
    /// `number` is the 1-based step number used in error messages.
    fn normalized(self, number: usize) -> Result<PlanStep> {
        let summary = self.summary.trim().to_string();
        ensure!(
            within(&summary, MAX_SUMMARY_CHARS),
            "Step {number}: the summary must be 1 to {MAX_SUMMARY_CHARS} characters."
        );
        let target = self.target.trim().to_string();
        ensure!(
            within(&target, MAX_TARGET_CHARS),
            "Step {number}: the target must be 1 to {MAX_TARGET_CHARS} characters."
        );
        let language = match self.kind {
            StepKind::CreateFile | StepKind::EditFile => {
                if normalized_relative(Path::new(&target)).is_err() {
                    bail!(
                        "Step {number}: the target must be a path inside the project, not \"{target}\"."
                    );
                }
                language_for_path(&target)
                    .map(String::from)
                    .unwrap_or_else(|| normalize_language(&self.language))
            }
            StepKind::RunCommand => {
                ensure!(
                    single_line(&target),
                    "Step {number}: the command must be a single line."
                );
                normalize_language(&self.language)
            }
        };
        Ok(PlanStep {
            kind: self.kind,
            target,
            summary,
            language,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreakdownItem {
    pub part: String,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guidance {
    pub step: usize,
    pub level: HelpLevel,
    pub summary: String,
    pub why: String,
    pub code: Option<String>,
    pub pseudocode: Option<String>,
    pub command: Option<String>,
    pub breakdown: Vec<BreakdownItem>,
    pub path: Option<String>,
    pub anchor: Option<String>,
}

impl Guidance {
    /// Checks that the guidance has the shape its help level promises for this step.
    pub fn check(&self, step: &PlanStep) -> Result<()> {
        ensure!(
            !self.summary.trim().is_empty(),
            "Give a short summary of what to do in this step."
        );
        ensure!(
            !self.why.trim().is_empty(),
            "Explain why in `why`: every level says why the step is done this way."
        );
        if let Some(path) = &self.path {
            ensure!(
                normalized_relative(Path::new(path)).is_ok(),
                "The path must be inside the project, not \"{path}\"."
            );
        }
        let has_code = present(&self.code);
        let has_pseudocode = present(&self.pseudocode);
        let has_breakdown = !self.breakdown.is_empty();
        match step.kind {
            StepKind::RunCommand => {
                let command = self.command.as_deref().unwrap_or_default();
                ensure!(
                    !command.trim().is_empty(),
                    "This step runs a command: put the exact command in `command`."
                );
                ensure!(single_line(command), "The command must be a single line.");
                if self.level == HelpLevel::Beginner {
                    ensure!(
                        has_breakdown,
                        "At the beginner level, explain every token of the command in breakdown."
                    );
                }
            }
            StepKind::CreateFile | StepKind::EditFile => match self.level {
                HelpLevel::Beginner => ensure!(
                    has_code && has_breakdown,
                    "At the beginner level, give the complete code for this step and a breakdown entry for every keyword, construct and function call."
                ),
                HelpLevel::Intermediate => ensure!(
                    has_pseudocode && !has_code,
                    "At the intermediate level, give pseudocode, not the finished code, because the user wants to write it."
                ),
                HelpLevel::Expert => ensure!(
                    !has_code && !has_pseudocode && !has_breakdown,
                    "At the expert level, give only a short description and the reason."
                ),
            },
        }
        Ok(())
    }
}

/// True when the trimmed text has 1 to `max` characters.
fn within(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.chars().count())
}

fn single_line(text: &str) -> bool {
    !text.contains(['\n', '\r'])
}

fn present(text: &Option<String>) -> bool {
    text.as_deref().is_some_and(|text| !text.trim().is_empty())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub problem: String,
    pub why: String,
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// `None` means the whole task against its "done when" criteria.
    pub step: Option<usize>,
    pub passed: bool,
    pub explanation: String,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Read,
    Search,
    WebSearch,
    WebFetch,
    Command,
    Edit,
    Tool,
}

impl ActivityKind {
    /// Maps the helper's `activity` event kind; unknown kinds show as a tool call.
    pub fn from_wire(kind: &str) -> ActivityKind {
        match kind {
            "read" => ActivityKind::Read,
            "search" => ActivityKind::Search,
            "web_search" => ActivityKind::WebSearch,
            "web_fetch" => ActivityKind::WebFetch,
            "command" => ActivityKind::Command,
            "edit" => ActivityKind::Edit,
            _ => ActivityKind::Tool,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemKind {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Activity {
        kind: ActivityKind,
        label: String,
    },
    Plan {
        plan: Plan,
    },
    Check {
        result: CheckResult,
    },
    Changes {
        change_set: String,
    },
    Notice {
        text: String,
    },
    Permission {
        title: String,
        detail: String,
        command: Option<String>,
        allowed: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatItem {
    pub id: i64,
    pub created_at_ms: i64,
    pub kind: ItemKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatTask {
    pub id: String,
    pub project: String,
    pub title: String,
    pub phase: Phase,
    pub mode: Mode,
    pub plan: Option<Plan>,
    pub current_step: usize,
    pub session_id: String,
    pub session_started: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ChatTask {
    pub fn new(project: &str) -> ChatTask {
        let now = now_ms();
        ChatTask {
            id: new_id(),
            project: project.into(),
            title: NEW_TASK_TITLE.into(),
            phase: Phase::Discuss,
            mode: Mode::Guided,
            plan: None,
            current_step: 0,
            // The Agent SDK requires session ids to be UUIDs.
            session_id: new_id(),
            session_started: false,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub phase: Phase,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverScope {
    Step(usize),
    /// All steps from this index on.
    Rest(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Undecided,
    Kept,
    Undone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub decision: Decision,
    pub after_hash: Option<String>,
    pub undoable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetStatus {
    Open,
    Review,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub id: String,
    pub task_id: String,
    pub scope: TakeoverScope,
    pub status: ChangeSetStatus,
    pub files: Vec<FileChange>,
    pub created_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_for_path_maps_known_extensions() {
        let cases = [
            ("src/main.rs", Some("rust")),
            ("cmd/server.go", Some("go")),
            ("web/app.ts", Some("typescript")),
            ("web/App.tsx", Some("typescript")),
            ("web/x.mts", Some("typescript")),
            ("web/x.cts", Some("typescript")),
            ("web/app.js", Some("javascript")),
            ("web/App.jsx", Some("javascript")),
            ("web/x.mjs", Some("javascript")),
            ("web/x.cjs", Some("javascript")),
            ("tool.py", Some("python")),
            ("stubs.pyi", Some("python")),
            ("src/Main.java", Some("java")),
            ("run.sh", Some("shell")),
            ("run.bash", Some("shell")),
            ("run.zsh", Some("shell")),
            ("README.md", None),
            ("Makefile", None),
            ("", None),
        ];
        for (path, expected) in cases {
            assert_eq!(language_for_path(path), expected, "{path}");
        }
    }

    #[test]
    fn normalize_language_trims_lowercases_and_maps_aliases() {
        let cases = [
            ("Rust", "rust"),
            ("  Go ", "go"),
            ("golang", "go"),
            ("ts", "typescript"),
            ("TypeScript", "typescript"),
            ("js", "javascript"),
            ("py", "python"),
            ("Python", "python"),
            ("bash", "shell"),
            ("sh", "shell"),
            ("zsh", "shell"),
            ("Terminal", "shell"),
            ("", "shell"),
            ("   ", "shell"),
            ("Kotlin", "kotlin"),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize_language(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn help_level_defaults_to_beginner_and_returns_the_saved_level() {
        let mut prefs = ChatPrefs::default();
        assert_eq!(prefs.model, DEFAULT_MODEL);
        assert_eq!(prefs.effort, None);
        assert!(prefs.use_my_setup);
        assert_eq!(prefs.help_level("rust"), HelpLevel::Beginner);
        prefs.help_levels.insert("rust".into(), HelpLevel::Expert);
        assert_eq!(prefs.help_level("rust"), HelpLevel::Expert);
        assert_eq!(prefs.help_level("go"), HelpLevel::Beginner);
    }

    #[test]
    fn help_level_labels_and_ids() {
        assert_eq!(
            HelpLevel::ALL.map(HelpLevel::label),
            ["Beginner", "Intermediate", "Expert"]
        );
        assert_eq!(
            HelpLevel::ALL.map(HelpLevel::id),
            ["beginner", "intermediate", "expert"]
        );
    }

    #[test]
    fn activity_kind_from_wire_maps_every_kind_and_unknown_to_tool() {
        let cases = [
            ("read", ActivityKind::Read),
            ("search", ActivityKind::Search),
            ("web_search", ActivityKind::WebSearch),
            ("web_fetch", ActivityKind::WebFetch),
            ("command", ActivityKind::Command),
            ("edit", ActivityKind::Edit),
            ("tool", ActivityKind::Tool),
            ("something_new", ActivityKind::Tool),
        ];
        for (wire, expected) in cases {
            assert_eq!(ActivityKind::from_wire(wire), expected, "{wire}");
        }
    }

    #[test]
    fn new_task_starts_in_discuss_with_uuid_ids() {
        let task = ChatTask::new("/p");
        assert_eq!(task.project, "/p");
        assert_eq!(task.title, NEW_TASK_TITLE);
        assert_eq!(task.phase, Phase::Discuss);
        assert_eq!(task.mode, Mode::Guided);
        assert_eq!(task.plan, None);
        assert_eq!(task.current_step, 0);
        assert!(!task.session_started);
        assert!(uuid::Uuid::parse_str(&task.id).is_ok());
        assert!(uuid::Uuid::parse_str(&task.session_id).is_ok());
        assert_ne!(task.id, task.session_id);
    }

    fn step(kind: StepKind, target: &str, language: &str) -> PlanStep {
        PlanStep {
            kind,
            target: target.into(),
            summary: "Do the thing".into(),
            language: language.into(),
        }
    }

    fn plan(steps: Vec<PlanStep>) -> Plan {
        Plan {
            goal: "Print results as JSON when --json is set".into(),
            done_when: vec!["Output parses as JSON".into()],
            steps,
        }
    }

    fn three_steps() -> Vec<PlanStep> {
        vec![
            step(StepKind::EditFile, " src/cli.rs ", "python"),
            step(StepKind::RunCommand, "cargo add serde", "Rust"),
            step(StepKind::CreateFile, "tests/cli.rs", ""),
        ]
    }

    fn message(result: Result<impl std::fmt::Debug>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn normalized_plan_trims_and_sets_languages() {
        let mut input = plan(three_steps());
        input.goal = "  Print JSON  ".into();
        input.done_when = vec![" parses ".into(), "old output unchanged".into()];
        let plan = input.normalized().unwrap();
        assert_eq!(plan.goal, "Print JSON");
        assert_eq!(plan.done_when, ["parses", "old output unchanged"]);
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].target, "src/cli.rs");
        assert_eq!(plan.steps[0].language, "rust");
        assert_eq!(plan.steps[1].target, "cargo add serde");
        assert_eq!(plan.steps[1].language, "rust");
        assert_eq!(plan.steps[2].language, "rust");
    }

    #[test]
    fn normalized_plan_uses_the_given_language_for_unknown_extensions() {
        let plan = plan(vec![step(StepKind::EditFile, "README.md", " Markdown ")])
            .normalized()
            .unwrap();
        assert_eq!(plan.steps[0].language, "markdown");
    }

    #[test]
    fn normalized_plan_rejects_a_missing_goal_or_criteria() {
        let mut input = plan(three_steps());
        input.goal = "   ".into();
        assert!(message(input.normalized()).contains("goal"));

        let mut input = plan(three_steps());
        input.goal = "x".repeat(501);
        assert!(message(input.normalized()).contains("goal"));

        let mut input = plan(three_steps());
        input.done_when = vec![];
        assert!(message(input.normalized()).contains("done when"));

        let mut input = plan(three_steps());
        input.done_when = vec!["ok".into(), " ".into()];
        assert!(message(input.normalized()).contains("done when"));

        let mut input = plan(three_steps());
        input.done_when = vec!["ok".into(); 11];
        assert!(message(input.normalized()).contains("done when"));
    }

    #[test]
    fn normalized_plan_rejects_no_steps_and_too_many_steps() {
        assert!(message(plan(vec![]).normalized()).contains("1 to 40 steps"));
        let many = vec![step(StepKind::EditFile, "src/a.rs", "rust"); 41];
        assert!(message(plan(many).normalized()).contains("1 to 40 steps"));
        let most = vec![step(StepKind::EditFile, "src/a.rs", "rust"); 40];
        assert!(plan(most).normalized().is_ok());
    }

    #[test]
    fn normalized_plan_rejects_targets_outside_the_project() {
        let error = message(
            plan(vec![
                step(StepKind::EditFile, "src/a.rs", "rust"),
                step(StepKind::EditFile, "/etc/hosts", "shell"),
            ])
            .normalized(),
        );
        assert_eq!(
            error,
            "Step 2: the target must be a path inside the project, not \"/etc/hosts\"."
        );
        let error = message(plan(vec![step(StepKind::CreateFile, "../x.rs", "rust")]).normalized());
        assert!(error.starts_with("Step 1: the target must be a path inside the project"));
    }

    #[test]
    fn normalized_plan_rejects_a_multi_line_command_and_bad_summaries() {
        let error = message(
            plan(vec![step(StepKind::RunCommand, "cargo test\nrm -rf /", "")]).normalized(),
        );
        assert!(error.starts_with("Step 1:"), "{error}");
        assert!(error.contains("single line"), "{error}");

        let mut empty_summary = step(StepKind::EditFile, "src/a.rs", "rust");
        empty_summary.summary = " ".into();
        assert!(message(plan(vec![empty_summary]).normalized()).contains("summary"));

        let mut long_summary = step(StepKind::EditFile, "src/a.rs", "rust");
        long_summary.summary = "x".repeat(201);
        assert!(message(plan(vec![long_summary]).normalized()).contains("summary"));

        let empty_target = step(StepKind::RunCommand, "  ", "shell");
        assert!(message(plan(vec![empty_target]).normalized()).contains("target"));
    }

    fn guidance(level: HelpLevel) -> Guidance {
        Guidance {
            step: 0,
            level,
            summary: "Add a json flag field to Args.".into(),
            why: "clap turns each field into a flag.".into(),
            code: None,
            pseudocode: None,
            command: None,
            breakdown: Vec::new(),
            path: Some("src/cli.rs".into()),
            anchor: None,
        }
    }

    fn breakdown() -> Vec<BreakdownItem> {
        vec![BreakdownItem {
            part: "#[arg(long)]".into(),
            explanation: "makes the field a --flag".into(),
        }]
    }

    fn file_step() -> PlanStep {
        step(StepKind::EditFile, "src/cli.rs", "rust")
    }

    fn command_step() -> PlanStep {
        step(StepKind::RunCommand, "cargo add serde", "rust")
    }

    #[test]
    fn every_level_must_say_why_and_summarize() {
        for level in HelpLevel::ALL {
            for step in [file_step(), command_step()] {
                let mut g = guidance(level);
                g.why = "  ".into();
                assert!(g.check(&step).is_err(), "{level:?} {step:?}");
                let mut g = guidance(level);
                g.summary = String::new();
                assert!(g.check(&step).is_err(), "{level:?} {step:?}");
            }
        }
    }

    #[test]
    fn beginner_file_guidance_needs_code_and_a_breakdown() {
        let mut g = guidance(HelpLevel::Beginner);
        g.code = Some("#[arg(long)]\njson: bool,".into());
        g.breakdown = breakdown();
        g.check(&file_step()).unwrap();

        let mut missing_code = g.clone();
        missing_code.code = None;
        assert_eq!(
            message(missing_code.check(&file_step())),
            "At the beginner level, give the complete code for this step and a breakdown entry for every keyword, construct and function call."
        );
        let mut missing_breakdown = g.clone();
        missing_breakdown.breakdown.clear();
        assert!(missing_breakdown.check(&file_step()).is_err());
    }

    #[test]
    fn intermediate_file_guidance_needs_pseudocode_and_no_code() {
        let mut g = guidance(HelpLevel::Intermediate);
        g.pseudocode = Some("add a bool field named json marked as a long flag".into());
        g.check(&file_step()).unwrap();
        let mut with_breakdown = g.clone();
        with_breakdown.breakdown = breakdown();
        with_breakdown.check(&file_step()).unwrap();

        let mut missing = g.clone();
        missing.pseudocode = None;
        assert_eq!(
            message(missing.check(&file_step())),
            "At the intermediate level, give pseudocode, not the finished code, because the user wants to write it."
        );
        let mut with_code = g.clone();
        with_code.code = Some("json: bool,".into());
        assert!(with_code.check(&file_step()).is_err());
    }

    #[test]
    fn expert_file_guidance_has_only_a_description_and_reason() {
        let g = guidance(HelpLevel::Expert);
        g.check(&file_step()).unwrap();
        let expected = "At the expert level, give only a short description and the reason.";

        let mut with_code = g.clone();
        with_code.code = Some("json: bool,".into());
        assert_eq!(message(with_code.check(&file_step())), expected);
        let mut with_pseudocode = g.clone();
        with_pseudocode.pseudocode = Some("add a field".into());
        assert_eq!(message(with_pseudocode.check(&file_step())), expected);
        let mut with_breakdown = g.clone();
        with_breakdown.breakdown = breakdown();
        assert_eq!(message(with_breakdown.check(&file_step())), expected);
    }

    #[test]
    fn command_guidance_needs_a_single_line_command_at_every_level() {
        for level in HelpLevel::ALL {
            let mut g = guidance(level);
            g.path = None;
            g.command = Some("cargo add serde".into());
            if level == HelpLevel::Beginner {
                g.breakdown = breakdown();
            }
            g.check(&command_step()).unwrap();

            let mut missing = g.clone();
            missing.command = None;
            assert!(missing.check(&command_step()).is_err(), "{level:?}");
            let mut blank = g.clone();
            blank.command = Some("  ".into());
            assert!(blank.check(&command_step()).is_err(), "{level:?}");
            let mut multi = g.clone();
            multi.command = Some("cargo add serde\ncargo build".into());
            assert!(multi.check(&command_step()).is_err(), "{level:?}");
        }
    }

    #[test]
    fn beginner_command_guidance_explains_every_token() {
        let mut g = guidance(HelpLevel::Beginner);
        g.command = Some("cargo add serde".into());
        assert_eq!(
            message(g.check(&command_step())),
            "At the beginner level, explain every token of the command in breakdown."
        );
    }

    #[test]
    fn guidance_path_must_stay_inside_the_project() {
        let mut g = guidance(HelpLevel::Expert);
        g.path = Some("../outside.rs".into());
        assert!(g.check(&file_step()).is_err());
        g.path = Some("/etc/hosts".into());
        assert!(g.check(&file_step()).is_err());
        g.path = None;
        g.check(&file_step()).unwrap();
    }

    #[test]
    fn item_kinds_serialize_with_a_snake_case_type_tag() {
        let item = ItemKind::Activity {
            kind: ActivityKind::WebSearch,
            label: "clap".into(),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type":"activity","kind":"web_search","label":"clap"})
        );
        assert_eq!(serde_json::from_value::<ItemKind>(json).unwrap(), item);
    }
}
