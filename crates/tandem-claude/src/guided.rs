//! The guided build: guidance for each step at the user's help level, "Check my work" and
//! the final check against the plan's "done when" criteria. These are methods of the
//! companion controller; `companion.rs` routes the commands, the `show_step` and
//! `report_check` tools and every finished turn here. A takeover of the remaining steps
//! also ends with `report_check` (see `takeover.rs`).

use crate::companion::Controller;
use crate::prompts;
use crate::protocol::{Policy, TurnStatus};
use crate::state::{ChatState, Connection, HostAnswer, HostQuestion, TurnKind};
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use tandem_core::Store;
use tandem_core::chat::{
    BreakdownItem, ChatPrefs, ChatTask, CheckResult, EditLocation, Finding, Guidance, HelpLevel,
    ItemKind, Mode, Phase, PlanStep, StepKind, TakeoverScope, normalize_language,
};

const BUILDING: &str = "You are building this. Claude guides each step.";
const GUIDANCE_LOCKED: &str = "Step guidance can only be shown during a guided build.";
const GUIDANCE_SHOWN: &str = "Guidance is on screen. Reply with one short sentence.";
const NO_CHECK: &str = "No check is running.";
const ALREADY_REPORTED: &str = "A verdict was already reported for this turn.";
const NO_EXPLANATION: &str = "Explain the verdict in explanation.";
const VERDICT_SHOWN: &str = "The verdict is on screen. Finish with one short sentence.";
const TASK_COMPLETE: &str = "Task complete.";
const NO_GUIDANCE: &str =
    "Claude did not show guidance for this step. Use \"Guide me through this step\" to try again.";
const NO_VERDICT: &str = "Claude did not give a verdict. Use \"Check my work\" again.";

/// What the running turn achieved, and the turn Tandem starts by itself when it completes.
#[derive(Default)]
pub(crate) struct TurnProgress {
    guidance_shown: bool,
    verdict_reported: bool,
    /// A takeover recorded a finished step with `step_done`.
    pub(crate) step_recorded: bool,
    pub(crate) follow_up: Option<FollowUp>,
}

pub(crate) enum FollowUp {
    /// A step passed and another one is open.
    NextGuidance,
    /// The last step passed.
    FinalCheck,
}

/// The `show_step` tool input; `step` is 1-based.
#[derive(Deserialize)]
struct ShowStepInput {
    step: usize,
    summary: String,
    why: String,
    code: Option<String>,
    pseudocode: Option<String>,
    command: Option<String>,
    #[serde(default)]
    breakdown: Vec<BreakdownItem>,
    path: Option<String>,
    #[serde(rename = "where")]
    where_: Option<String>,
    location: Option<EditLocation>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CheckScope {
    Step,
    Task,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    Pass,
    NeedsChanges,
}

/// The `report_check` tool input; `step` is 1-based.
#[derive(Deserialize)]
struct ReportCheckInput {
    scope: CheckScope,
    step: Option<usize>,
    verdict: Verdict,
    explanation: String,
    #[serde(default)]
    findings: Vec<Finding>,
}

/// The active step of a guided build, while one is still open.
fn guided_step(task: &ChatTask) -> Option<&PlanStep> {
    if task.phase != Phase::Build || task.mode != Mode::Guided {
        return None;
    }
    task.plan.as_ref()?.steps.get(task.current_step)
}

/// The stored guidance for the active step of a guided build at the current help level.
pub(crate) fn stored_guidance(
    store: &Store,
    task: &ChatTask,
    prefs: &ChatPrefs,
) -> Result<Option<Guidance>> {
    match guided_step(task) {
        Some(step) => store.chat_guidance(
            &task.id,
            task.current_step,
            prefs.help_level(&step.language),
        ),
        None => Ok(None),
    }
}

pub(crate) fn guidance_load_error(error: &anyhow::Error) -> String {
    format!("Could not load the step guidance: {error:#}")
}

/// Every step of the plan is finished.
fn every_step_finished(task: &ChatTask) -> bool {
    task.plan
        .as_ref()
        .is_some_and(|plan| task.current_step >= plan.steps.len())
}

fn idle_and_ready(state: &ChatState) -> bool {
    state.turn.is_none() && matches!(state.connection, Connection::Ready { .. })
}

/// `None` for a missing or blank text.
fn non_blank(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
}

/// The guidance a `show_step` input describes for the step at `index`, at `level`.
fn guidance_from(
    input: ShowStepInput,
    index: usize,
    level: HelpLevel,
    step: &PlanStep,
) -> Guidance {
    let path = match non_blank(input.path) {
        Some(path) => Some(path.trim().to_owned()),
        None if step.kind != StepKind::RunCommand => Some(step.target.clone()),
        None => None,
    };
    Guidance {
        step: index,
        level,
        summary: input.summary.trim().to_owned(),
        why: input.why.trim().to_owned(),
        code: non_blank(input.code),
        pseudocode: non_blank(input.pseudocode),
        command: non_blank(input.command).map(|command| command.trim().to_owned()),
        breakdown: input.breakdown,
        path,
        anchor: None,
        where_: non_blank(input.where_).map(|text| text.trim().to_owned()),
        location: input.location,
    }
}

impl Controller {
    /// Publishes the stored guidance for the active step at the current help level.
    pub(crate) fn refresh_guidance(&self) {
        let state = self.snapshot();
        let guidance = match &state.task {
            Some(task) => self.with_store(|store| stored_guidance(store, task, &state.prefs)),
            None => Ok(None),
        };
        self.update(|state| match guidance {
            Ok(guidance) => state.guidance = guidance,
            Err(error) => {
                state.guidance = None;
                state.banner = Some(guidance_load_error(&error));
            }
        });
    }

    /// Shows the stored guidance for the active step at its help level, or asks Claude for
    /// it.
    pub(crate) fn start_guidance(&mut self) {
        let state = self.snapshot();
        let Some(task) = &state.task else {
            return;
        };
        match self.with_store(|store| stored_guidance(store, task, &state.prefs)) {
            Ok(Some(guidance)) => self.update(|state| state.guidance = Some(guidance)),
            // Guidance that cannot be read is asked for again.
            Ok(None) | Err(_) => self.start_guidance_turn(),
        }
    }

    /// Asks Claude for guidance on the active step at the current help level.
    fn start_guidance_turn(&mut self) {
        let state = self.snapshot();
        let Some(task) = &state.task else {
            return;
        };
        let (Some(plan), Some(step)) = (&task.plan, guided_step(task)) else {
            return;
        };
        let index = task.current_step;
        let level = state.prefs.help_level(&step.language);
        self.update(|state| state.guidance = None);
        self.start_turn(
            TurnKind::Guidance { step: index, level },
            prompts::guidance(plan, index, level),
            Policy::Guided,
        );
    }

    /// "I'll write it": starts the guided build at step 1.
    pub(crate) fn choose_guided(&mut self) {
        let state = self.snapshot();
        let Some(task) = state.task.as_ref().filter(|task| {
            task.phase == Phase::Plan && task.plan.is_some() && idle_and_ready(&state)
        }) else {
            return;
        };
        let mut task = task.clone();
        task.phase = Phase::Build;
        task.mode = Mode::Guided;
        task.current_step = 0;
        self.save_task(task);
        self.append_item(ItemKind::Notice {
            text: BUILDING.into(),
        });
        self.start_guidance();
    }

    /// "Guide me through this step": forgets the stored guidance and asks again.
    pub(crate) fn guide_step(&mut self) {
        let state = self.snapshot();
        let Some(task) = state.task.as_ref().filter(|_| idle_and_ready(&state)) else {
            return;
        };
        let Some(step) = guided_step(task) else {
            return;
        };
        let level = state.prefs.help_level(&step.language);
        if let Err(error) =
            self.with_store(|store| store.delete_chat_guidance(&task.id, task.current_step, level))
        {
            self.set_banner(format!("Could not forget the old guidance: {error:#}"));
        }
        self.start_guidance_turn();
    }

    /// Saves the level for `language`; when that is the active step's language, shows the
    /// guidance for the new level (asking Claude only when none is stored).
    pub(crate) fn set_help_level(&mut self, language: &str, level: HelpLevel) {
        let language = normalize_language(language);
        let state = self.snapshot();
        let mut prefs = state.prefs.clone();
        prefs.help_levels.insert(language.clone(), level);
        self.save_prefs(&prefs);
        self.update(|state| state.prefs = prefs);
        let active_language = state
            .task
            .as_ref()
            .and_then(guided_step)
            .is_some_and(|step| step.language == language);
        if !active_language {
            return;
        }
        if idle_and_ready(&state) {
            self.start_guidance();
        } else {
            self.refresh_guidance();
        }
    }

    /// "Check my work": checks the active step, or the whole task once every step is done.
    pub(crate) fn check_my_work(&mut self) {
        let state = self.snapshot();
        let Some(task) = state.task.as_ref().filter(|task| {
            task.phase == Phase::Build && task.mode == Mode::Guided && idle_and_ready(&state)
        }) else {
            return;
        };
        let Some(plan) = &task.plan else {
            return;
        };
        let index = task.current_step;
        let Some(step) = plan.steps.get(index) else {
            return self.start_final_check();
        };
        let level = state.prefs.help_level(&step.language);
        let unsaved = self.dirty_paths();
        self.start_turn(
            TurnKind::Check { step: index },
            prompts::check(plan, index, level, &unsaved),
            Policy::Check,
        );
    }

    fn start_final_check(&mut self) {
        let Some(plan) = self
            .snapshot()
            .task
            .as_ref()
            .and_then(|task| task.plan.clone())
        else {
            return;
        };
        self.start_turn(
            TurnKind::FinalCheck,
            prompts::final_check(&plan),
            Policy::Check,
        );
    }

    /// The `show_step` tool: guidance for the active step of a guided build, during a
    /// Guidance turn (at its level) or a chat turn (at the current level).
    pub(crate) fn show_step(&mut self, input: Value) -> (bool, String) {
        let state = self.snapshot();
        let turn_level = match state.turn {
            Some(TurnKind::Guidance { level, .. }) => Some(level),
            Some(TurnKind::Chat) => None,
            _ => return (false, GUIDANCE_LOCKED.into()),
        };
        let Some(task) = &state.task else {
            return (false, GUIDANCE_LOCKED.into());
        };
        let Some(step) = guided_step(task) else {
            return (false, GUIDANCE_LOCKED.into());
        };
        let input = match serde_json::from_value::<ShowStepInput>(input) {
            Ok(input) => input,
            Err(error) => {
                return (
                    false,
                    format!("The guidance does not have the expected fields: {error}."),
                );
            }
        };
        let active = task.current_step + 1;
        if input.step != active {
            return (
                false,
                format!("Step {active} is active. Show guidance for that step only."),
            );
        }
        let level = turn_level.unwrap_or_else(|| state.prefs.help_level(&step.language));
        let mut guidance = guidance_from(input, task.current_step, level, step);
        if let Err(error) = guidance.check(step) {
            return (false, format!("{error:#}"));
        }
        let file = (level == HelpLevel::Beginner && step.kind == StepKind::EditFile)
            .then(|| {
                guidance
                    .path
                    .as_deref()
                    .and_then(|path| self.current_text(path))
            })
            .flatten();
        match guidance.located(step, file.as_deref()) {
            Ok(location) => guidance.location = location,
            Err(error) => return (false, format!("{error:#}")),
        }
        if let Err(error) = self.with_store(|store| store.save_chat_guidance(&task.id, &guidance)) {
            self.set_banner(format!("Could not save the step guidance: {error:#}"));
        }
        self.progress.guidance_shown = true;
        // The level may have changed while Claude worked; the card shows the current one.
        if self.snapshot().prefs.help_level(&step.language) == level {
            self.update(|state| state.guidance = Some(guidance));
        }
        (true, GUIDANCE_SHOWN.into())
    }

    /// The file's text as the user sees it: the editor's buffer when the file is open
    /// (unsaved changes included), otherwise the saved file. `None` when neither can be read.
    fn current_text(&self, path: &str) -> Option<String> {
        match self.ask_host(HostQuestion::ReadBuffer {
            path: path.to_owned(),
        }) {
            Some(HostAnswer::Buffer(Some(buffer))) => Some(buffer.text),
            _ => self
                .project
                .open_document(path, false)
                .ok()
                .map(|document| document.text),
        }
    }

    /// The `report_check` tool: one verdict per Check or FinalCheck turn, or per takeover
    /// of the remaining steps once every step is finished.
    pub(crate) fn report_check(&mut self, input: Value) -> (bool, String) {
        let state = self.snapshot();
        // The step being checked, `None` for the whole task.
        let checking = match &state.turn {
            Some(TurnKind::Check { step }) => Some(*step),
            Some(TurnKind::FinalCheck) => None,
            Some(TurnKind::Takeover {
                scope: TakeoverScope::Rest(_),
                ..
            }) if state.task.as_ref().is_some_and(every_step_finished) => None,
            _ => return (false, NO_CHECK.into()),
        };
        if self.progress.verdict_reported {
            return (false, ALREADY_REPORTED.into());
        }
        let input = match serde_json::from_value::<ReportCheckInput>(input) {
            Ok(input) => input,
            Err(error) => {
                return (
                    false,
                    format!("The verdict does not have the expected fields: {error}."),
                );
            }
        };
        let step = match (input.scope, checking) {
            (CheckScope::Step, Some(step)) => {
                if input.step.is_some_and(|number| number != step + 1) {
                    return (
                        false,
                        format!(
                            "This check is for step {}. Report the verdict for that step only.",
                            step + 1
                        ),
                    );
                }
                Some(step)
            }
            (CheckScope::Task, None) => None,
            _ => return (false, NO_CHECK.into()),
        };
        let explanation = input.explanation.trim().to_owned();
        if explanation.is_empty() {
            return (false, NO_EXPLANATION.into());
        }
        let Some(mut task) = state.task.clone() else {
            return (false, NO_CHECK.into());
        };
        let passed = input.verdict == Verdict::Pass;
        self.progress.verdict_reported = true;
        self.append_item(ItemKind::Check {
            result: CheckResult {
                step,
                passed,
                explanation,
                findings: input.findings,
            },
        });
        match (passed, step) {
            (true, Some(step)) => {
                let steps = task.plan.as_ref().map_or(0, |plan| plan.steps.len());
                task.current_step = step + 1;
                self.update(|state| state.guidance = None);
                self.save_task(task);
                self.progress.follow_up = Some(if step + 1 < steps {
                    FollowUp::NextGuidance
                } else {
                    FollowUp::FinalCheck
                });
            }
            (true, None) => {
                task.phase = Phase::Done;
                self.save_task(task);
                self.append_item(ItemKind::Notice {
                    text: TASK_COMPLETE.into(),
                });
            }
            (false, _) => {}
        }
        (true, VERDICT_SHOWN.into())
    }

    /// Runs after every turn: notes a Guidance or Check turn that completed without its
    /// result, and starts the remembered follow-up when the turn completed.
    pub(crate) fn after_turn(&mut self, kind: TurnKind, status: TurnStatus) {
        let progress = std::mem::take(&mut self.progress);
        if status != TurnStatus::Completed || self.closing {
            return;
        }
        let missing = match kind {
            TurnKind::Guidance { .. } if !progress.guidance_shown => Some(NO_GUIDANCE),
            TurnKind::Check { .. } | TurnKind::FinalCheck if !progress.verdict_reported => {
                Some(NO_VERDICT)
            }
            _ => None,
        };
        if let Some(text) = missing {
            self.append_item(ItemKind::Notice { text: text.into() });
        }
        if !idle_and_ready(&self.snapshot()) {
            return;
        }
        match progress.follow_up {
            Some(FollowUp::NextGuidance) => self.start_guidance(),
            Some(FollowUp::FinalCheck) => self.start_final_check(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tandem_core::chat::{EditAction, Placement};

    /// `show_step` inputs that the helper's tests read too.
    const SAMPLES: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/helper/test/fixtures/show_step.json"
    ));

    /// `src/cli.rs` of the samples, 7 lines.
    const CLI: &str = "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    /// The name to greet.\n    pub name: String,\n}\n";

    fn sample(name: &str) -> Value {
        let samples: Value = serde_json::from_str(SAMPLES).expect("the fixture is JSON");
        samples[name].clone()
    }

    fn edit_step() -> PlanStep {
        PlanStep {
            kind: StepKind::EditFile,
            target: "src/cli.rs".into(),
            summary: "Add a `json` flag to Args".into(),
            language: "rust".into(),
        }
    }

    fn command_step() -> PlanStep {
        PlanStep {
            kind: StepKind::RunCommand,
            target: "cargo add serde".into(),
            summary: "Add serde".into(),
            language: "rust".into(),
        }
    }

    /// The guidance the sample `name` describes for `step` at `level`.
    fn sample_guidance(name: &str, step: &PlanStep, level: HelpLevel) -> Guidance {
        let input = serde_json::from_value::<ShowStepInput>(sample(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let index = input.step - 1;
        guidance_from(input, index, level, step)
    }

    #[test]
    fn the_fixture_inputs_make_valid_guidance() {
        let mut cases = vec![
            ("beginner_edit", edit_step(), HelpLevel::Beginner),
            ("beginner_update", edit_step(), HelpLevel::Beginner),
            ("intermediate_edit", edit_step(), HelpLevel::Intermediate),
            ("expert_edit", edit_step(), HelpLevel::Expert),
        ];
        cases.extend(HelpLevel::ALL.map(|level| ("command", command_step(), level)));
        for (name, step, level) in cases {
            let guidance = sample_guidance(name, &step, level);
            assert_eq!(guidance.level, level, "{name}");
            if let Err(error) = guidance.check(&step) {
                panic!("{name} at {level:?}: {error:#}");
            }
        }
    }

    #[test]
    fn the_fixture_locations_match_the_file() {
        let step = edit_step();
        let edit = sample_guidance("beginner_edit", &step, HelpLevel::Beginner);
        let location = edit.located(&step, Some(CLI)).unwrap().expect("a location");
        assert_eq!(location.action, EditAction::Insert);
        assert_eq!(location.line, 6);
        assert_eq!(location.anchor, "pub name: String,");
        assert_eq!(location.placement, Placement::After);

        let update = sample_guidance("beginner_update", &step, HelpLevel::Beginner);
        let location = update
            .located(&step, Some(CLI))
            .unwrap()
            .expect("a location");
        assert_eq!(location.action, EditAction::Update);
        assert_eq!(location.line, 5);
        assert_eq!(location.end_line, None);

        let intermediate = sample_guidance("intermediate_edit", &step, HelpLevel::Intermediate);
        assert_eq!(intermediate.located(&step, Some(CLI)).unwrap(), None);
        assert_eq!(
            intermediate.where_.as_deref(),
            Some("in src/cli.rs, inside struct Args, after the name field")
        );
    }

    #[test]
    fn an_old_anchor_field_is_ignored() {
        let mut input = sample("beginner_edit");
        input["anchor"] = Value::from("x");
        let input = serde_json::from_value::<ShowStepInput>(input).unwrap();
        let guidance = guidance_from(input, 0, HelpLevel::Beginner, &edit_step());
        assert_eq!(guidance.anchor, None);
    }
}
