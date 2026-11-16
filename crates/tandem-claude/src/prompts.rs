//! The text Tandem gives Claude: the system prompt addition and each turn's message.

use tandem_core::chat::{ChatItem, ChatTask, HelpLevel, ItemKind, Phase, Plan, PlanStep, StepKind};

/// Appended to Claude Code's system prompt for every session.
pub const SYSTEM_APPEND: &str = "\
You are the companion inside Tandem, a code editor for learning by building. The user is an experienced engineer who is learning Rust, Go and TypeScript. Never guess how much help they need: Tandem names the help level to use.

How a task works
1. Discuss. The user describes a feature. Research before you answer: read the project's code, search the web when facts may have changed, and ask clarifying questions in plain chat. Do not edit files.
2. Plan. When you and the user agree on what to build, call the propose_plan tool with the goal, the \"done when\" criteria and numbered steps. Each step is tiny and does one thing: create one file, make one focused edit to one file (add or remove something), or run one terminal command. Anything that must run in a terminal is its own step. If the user asks for changes, call propose_plan again with the whole revised plan. Do not start implementing until Tandem says the build has started.
3. Build. Tandem tells you which step is active and what to do with it: show guidance (show_step), check the user's work (report_check), or do the work yourself during a takeover (step_done). Do only that.

Rules that always apply
- Give the reason for everything. Write \"do X, because Y\", never only \"do X\".
- Explain every language on its own terms. Never compare code to Python or any other programming language, and never explain a construct by what it would be in another language, unless the user asks for that comparison.
- Follow the help level Tandem names for the step.
  beginner: give the complete code for the step and explain every keyword, construct and function call in it; explain terminal commands token by token. Copy each explained part exactly from the code or command, because the user reads an explanation by pointing at that part. For an edit, give the exact line where the change goes, because Tandem takes the user there.
  intermediate: describe the change, say in words where it goes, give pseudocode rather than the finished code, and explain the parts that are new or unusual in this language.
  expert: a short description, where the change goes in words, and the reason.
- Use Tandem's tools for plans, step guidance and verdicts. Use plain chat for conversation and questions.
- Change project files only while Tandem says a takeover is active. At every other time explain, because the user writes the code.
- Lines that begin with [Tandem] come from the editor, not from the user.";

/// Repeated in every turn that asks Claude to explain code.
const OWN_TERMS: &str = "Explain the code on its own terms: do not compare it to Python or any other programming language unless the user asked for that.";

const RECAP_MESSAGES: usize = 6;
const RECAP_MESSAGE_CHARS: usize = 500;

/// The message for a chat turn: an optional recap of a conversation Claude lost, the
/// phase's context line, then the user's text. Slash commands go through unchanged so
/// Claude Code commands and skills keep working.
pub fn chat(task: &ChatTask, items: &[ChatItem], text: &str, recap: bool) -> String {
    if text.starts_with('/') {
        return text.to_owned();
    }
    let mut message = String::new();
    if recap {
        message.push_str(&recap_block(task, items));
    }
    if let Some(line) = context_line(task) {
        message.push_str(&line);
        message.push_str("\n\n");
    }
    message.push_str(text);
    message
}

/// The message that asks Claude for guidance on step `step` (an index into the plan's
/// steps) at `level`.
pub fn guidance(plan: &Plan, step: usize, level: HelpLevel) -> String {
    let active = &plan.steps[step];
    let rule = match (level, active.kind) {
        (HelpLevel::Beginner, StepKind::RunCommand) => "Put the exact command in command and add a breakdown entry for every token of it, each part copied exactly from the command.".to_owned(),
        (HelpLevel::Beginner, StepKind::EditFile) => "Put the complete code for this step in code and add a breakdown entry for every keyword, construct and function call in that code, each part copied exactly from the code. Set location to where the change goes in the file as it is now: action (insert, update or remove), line (the number of the existing line the new code goes next to, or of the first line to replace or remove), anchor (the exact text of that line), end_line (the last line to replace or remove, when there are several) and placement (before or after the anchor line, for an insert; after is the default). Read the file first, so that the line number and the anchor are exact.".to_owned(),
        (HelpLevel::Beginner, StepKind::CreateFile) => "Put the complete code for this step in code and add a breakdown entry for every keyword, construct and function call in that code, each part copied exactly from the code. Leave location out, because the file is new.".to_owned(),
        (HelpLevel::Intermediate, StepKind::RunCommand) => "Put the exact command in command and explain in breakdown only the parts that are new or unusual, each part copied exactly from the command.".to_owned(),
        (HelpLevel::Intermediate, StepKind::EditFile) => format!(
            "Describe the change, say in where, in words, where it goes (for example: in src/cli.rs, inside struct Args, after the verbose field), put pseudocode in pseudocode and leave code empty, and add breakdown entries only for what is new or unusual in {}.",
            active.language
        ),
        (HelpLevel::Intermediate, StepKind::CreateFile) => format!(
            "Describe the change, put pseudocode in pseudocode and leave code empty, and add breakdown entries only for what is new or unusual in {}.",
            active.language
        ),
        (HelpLevel::Expert, StepKind::RunCommand) => {
            "Put the exact command in command; summary and why only.".to_owned()
        }
        (HelpLevel::Expert, StepKind::EditFile) => "Keep it short: summary, why and where (in words, where the change goes); leave code, pseudocode and breakdown empty.".to_owned(),
        (HelpLevel::Expert, StepKind::CreateFile) => {
            "Keep it short: summary and why only; leave code, pseudocode and breakdown empty."
                .to_owned()
        }
    };
    [
        format!(
            "[Tandem] Step {} of {} is now active.",
            step + 1,
            plan.steps.len()
        ),
        format!("Step: {}", step_name(active)),
        format!("Summary: {}", active.summary),
        format!(
            "Language: {}. Help level: {}.",
            active.language,
            level.id()
        ),
        format!(
            "Call show_step for exactly this step. {rule} {OWN_TERMS} Do not edit files and do not move to another step. After the tool call, reply with one short sentence."
        ),
    ]
    .join("\n")
}

/// The message for "Check my work" on step `step` (an index into the plan's steps).
/// `unsaved` lists the files with unsaved changes in the editor.
pub fn check(plan: &Plan, step: usize, level: HelpLevel, unsaved: &[String]) -> String {
    let active = &plan.steps[step];
    let mut lines = vec![
        format!(
            "[Tandem] The user clicked \"Check my work\" for step {} of {}: {}. {}",
            step + 1,
            plan.steps.len(),
            step_name(active),
            active.summary
        ),
        format!("Help level: {}.", level.id()),
    ];
    if !unsaved.is_empty() {
        lines.push(format!(
            "These files have unsaved changes in the editor: {}. Read them with read_editor_buffer, because the Read tool shows the older saved version. Commands you run see only saved files; say so if it matters.",
            unsaved.join(", ")
        ));
    }
    lines.push(format!("Inspect the user's work for this step only. You may run build, test or lint commands. Then call report_check with scope \"step\": verdict \"pass\" when the step is done correctly, or \"needs_changes\" with what to fix and why, written for the help level. {OWN_TERMS} Do not edit files."));
    lines.join("\n")
}

/// The message that checks the whole task against its "done when" criteria.
pub fn final_check(plan: &Plan) -> String {
    let mut lines = vec![format!(
        "[Tandem] All {} steps are finished. Check each \"done when\" criterion:",
        plan.steps.len()
    )];
    lines.extend(numbered_criteria(plan));
    lines.push("You may run build, test or lint commands. Then call report_check with scope \"task\": \"pass\" only when every criterion is met, otherwise \"needs_changes\" naming each criterion that fails and why. Do not edit files.".to_owned());
    lines.join("\n")
}

/// The message that hands step `step` (an index into the plan's steps) to Claude.
pub fn takeover_step(plan: &Plan, step: usize) -> String {
    let active = &plan.steps[step];
    let number = step + 1;
    format!(
        "[Tandem] Takeover is active for step {number} of {} only: {}. {}\n\
         Do this step yourself now: edit files inside the project folder, or run the command. Do not do later steps. When the step is finished and verified, call step_done with step {number} and one sentence that says what you did and why.",
        plan.steps.len(),
        step_name(active),
        active.summary
    )
}

/// The message that hands every step from `from` (an index into the plan's steps) to
/// Claude, followed by the "done when" check. With no step left it asks only for that
/// check.
pub fn takeover_rest(plan: &Plan, from: usize) -> String {
    let total = plan.steps.len();
    let mut lines = Vec::new();
    if from < total {
        lines.push(format!(
            "[Tandem] Takeover is active for all remaining steps, starting at step {} of {total}.",
            from + 1
        ));
        lines.extend(
            plan.steps
                .iter()
                .enumerate()
                .skip(from)
                .map(|(index, step)| {
                    format!("{}. {}: {}", index + 1, step_name(step), step.summary)
                }),
        );
        lines.push("Do them in order yourself and stay inside the project folder. After each step call step_done with its number. When every step is finished, check each \"done when\" criterion, fix what fails, and call report_check with scope \"task\":".to_owned());
    } else {
        lines.push("[Tandem] Takeover is active. Every step is finished but the task does not meet its \"done when\" criteria yet. Fix what is missing inside the project folder, then call report_check with scope \"task\":".to_owned());
    }
    lines.extend(numbered_criteria(plan));
    lines.push("If you need a decision from the user, stop and ask in chat.".to_owned());
    lines.join("\n")
}

/// The user's answer while a takeover of the remaining steps is paused; `current` is the
/// index of the next step. Slash commands go through unchanged.
pub fn takeover_chat(plan: &Plan, current: usize, text: &str) -> String {
    if text.starts_with('/') {
        return text.to_owned();
    }
    let context = if current < plan.steps.len() {
        format!(
            "[Tandem] Takeover is still active for the remaining steps (next: step {} of {}).",
            current + 1,
            plan.steps.len()
        )
    } else {
        "[Tandem] Takeover is still active. Every step is finished.".to_owned()
    };
    format!("{context}\n\n{text}")
}

fn numbered_criteria(plan: &Plan) -> impl Iterator<Item = String> + '_ {
    plan.done_when
        .iter()
        .enumerate()
        .map(|(index, criterion)| format!("{}. {criterion}", index + 1))
}

/// The label of a step kind in prompts, as in "Edit src/cli.rs".
pub fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::CreateFile => "Create",
        StepKind::EditFile => "Edit",
        StepKind::RunCommand => "Run",
    }
}

fn step_name(step: &PlanStep) -> String {
    format!("{} {}", kind_label(step.kind), step.target)
}

fn context_line(task: &ChatTask) -> Option<String> {
    match task.phase {
        Phase::Discuss | Phase::Plan => None,
        Phase::Build => {
            let steps = task.plan.as_ref().map_or(&[][..], |plan| &plan.steps);
            Some(match steps.get(task.current_step) {
                Some(step) => format!(
                    "[Tandem] Guided build. Step {} of {} is active ({}). The user writes the code: answer their question and do not edit files.",
                    task.current_step + 1,
                    steps.len(),
                    step_name(step)
                ),
                None => all_finished(),
            })
        }
        Phase::Done => Some(all_finished()),
    }
}

fn all_finished() -> String {
    "[Tandem] All steps of the plan are finished. Do not edit files.".to_owned()
}

fn recap_block(task: &ChatTask, items: &[ChatItem]) -> String {
    let mut lines = vec![
        "[Tandem] The earlier conversation with you could not be restored, so here is where things stand."
            .to_owned(),
    ];
    if let Some(plan) = &task.plan {
        lines.push(format!("Goal: {}", plan.goal));
        for (index, step) in plan.steps.iter().enumerate() {
            let marker = match task.phase {
                Phase::Done => " (done)",
                Phase::Build if index < task.current_step => " (done)",
                Phase::Build if index == task.current_step => " (active)",
                _ => "",
            };
            lines.push(format!(
                "{}. {}: {}{marker}",
                index + 1,
                step_name(step),
                step.summary
            ));
        }
    }
    let messages: Vec<String> = items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::User { text } => Some(format!("You: {}", cut(text, RECAP_MESSAGE_CHARS))),
            ItemKind::Assistant { text } => {
                Some(format!("Claude: {}", cut(text, RECAP_MESSAGE_CHARS)))
            }
            _ => None,
        })
        .collect();
    if !messages.is_empty() {
        lines.push("Recent messages:".to_owned());
        lines.extend(
            messages[messages.len().saturating_sub(RECAP_MESSAGES)..]
                .iter()
                .cloned(),
        );
    }
    let mut block = lines.join("\n");
    block.push_str("\n\n");
    block
}

/// The first `max` characters of `text`.
pub(crate) fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tandem_core::chat::{HelpLevel, ItemKind, Phase, Plan, PlanStep, StepKind};

    fn step(kind: StepKind, target: &str, summary: &str) -> PlanStep {
        PlanStep {
            kind,
            target: target.into(),
            summary: summary.into(),
            language: "rust".into(),
        }
    }

    fn task_in(phase: Phase, current_step: usize) -> ChatTask {
        let mut task = ChatTask::new("/project");
        task.phase = phase;
        task.current_step = current_step;
        task.plan = Some(Plan {
            goal: "Print results as JSON when --json is set".into(),
            done_when: vec!["Output parses as JSON".into()],
            steps: vec![
                step(StepKind::EditFile, "src/cli.rs", "Add `json` to Args"),
                step(StepKind::EditFile, "src/main.rs", "Print JSON in run()"),
                step(StepKind::RunCommand, "cargo test", "Run the tests"),
            ],
        });
        task
    }

    fn item(id: i64, kind: ItemKind) -> ChatItem {
        ChatItem {
            id,
            created_at_ms: id,
            kind,
        }
    }

    #[test]
    fn the_system_append_names_the_workflow_and_the_rules() {
        assert!(SYSTEM_APPEND.starts_with("You are the companion inside Tandem,"));
        assert!(SYSTEM_APPEND.contains("\n\nHow a task works\n1. Discuss."));
        assert!(SYSTEM_APPEND.ends_with("come from the editor, not from the user."));
        assert!(
            SYSTEM_APPEND.contains("Copy each explained part exactly from the code or command")
        );
        assert!(SYSTEM_APPEND.contains("For an edit, give the exact line where the change goes"));
        assert!(SYSTEM_APPEND.contains("say in words where it goes"));
    }

    #[test]
    fn the_system_append_forbids_comparisons_with_other_languages() {
        assert!(
            SYSTEM_APPEND
                .contains("Never compare code to Python or any other programming language")
        );
        assert!(SYSTEM_APPEND.contains("unless the user asks for that comparison"));
        assert!(!SYSTEM_APPEND.contains("strongest in"));
    }

    #[test]
    fn guidance_and_check_prompts_repeat_the_own_terms_rule() {
        let rule = "do not compare it to Python or any other programming language unless the user asked for that";
        let plan = guided_plan();
        for level in HelpLevel::ALL {
            for index in 0..3 {
                let prompt = guidance(&plan, index, level);
                assert!(prompt.contains(rule), "{level:?} {index}: {prompt}");
            }
        }
        assert!(check(&plan, 0, HelpLevel::Beginner, &[]).contains(rule));
        assert!(!final_check(&plan).contains("Python"));
        assert!(!takeover_step(&plan, 0).contains("Python"));
        assert!(!takeover_rest(&plan, 0).contains("Python"));
    }

    #[test]
    fn a_slash_command_is_sent_unchanged() {
        let task = task_in(Phase::Build, 1);
        assert_eq!(chat(&task, &[], "/review src", true), "/review src");
    }

    #[test]
    fn discuss_and_plan_add_no_prefix() {
        for phase in [Phase::Discuss, Phase::Plan] {
            let task = task_in(phase, 0);
            assert_eq!(
                chat(&task, &[], "Add a --json flag", false),
                "Add a --json flag"
            );
        }
    }

    #[test]
    fn build_names_the_active_step_and_forbids_edits() {
        let task = task_in(Phase::Build, 1);
        assert_eq!(
            chat(&task, &[], "Why a Vec?", false),
            "[Tandem] Guided build. Step 2 of 3 is active (Edit src/main.rs). The user writes the code: answer their question and do not edit files.\n\nWhy a Vec?"
        );
        let run = task_in(Phase::Build, 2);
        assert!(chat(&run, &[], "ok?", false).contains("Step 3 of 3 is active (Run cargo test)."));
    }

    #[test]
    fn a_finished_plan_says_so_and_forbids_edits() {
        let finished = "[Tandem] All steps of the plan are finished. Do not edit files.\n\nThanks";
        assert_eq!(
            chat(&task_in(Phase::Build, 3), &[], "Thanks", false),
            finished
        );
        assert_eq!(
            chat(&task_in(Phase::Done, 3), &[], "Thanks", false),
            finished
        );
    }

    #[test]
    fn the_recap_lists_goal_steps_and_recent_messages() {
        let task = task_in(Phase::Build, 1);
        let mut items = vec![item(1, ItemKind::User { text: "old".into() })];
        for id in 2..=7 {
            let text = format!("message {id}");
            items.push(item(
                id,
                if id % 2 == 0 {
                    ItemKind::Assistant { text }
                } else {
                    ItemKind::User { text }
                },
            ));
        }
        items.push(item(
            8,
            ItemKind::Notice {
                text: "Stopped.".into(),
            },
        ));
        items.push(item(
            9,
            ItemKind::User {
                text: "x".repeat(600),
            },
        ));
        let prompt = chat(&task, &items, "Go on", true);
        let expected = format!(
            "[Tandem] The earlier conversation with you could not be restored, so here is where things stand.\n\
             Goal: Print results as JSON when --json is set\n\
             1. Edit src/cli.rs: Add `json` to Args (done)\n\
             2. Edit src/main.rs: Print JSON in run() (active)\n\
             3. Run cargo test: Run the tests\n\
             Recent messages:\n\
             You: message 3\n\
             Claude: message 4\n\
             You: message 5\n\
             Claude: message 6\n\
             You: message 7\n\
             You: {}\n\
             \n\
             [Tandem] Guided build. Step 2 of 3 is active (Edit src/main.rs). The user writes the code: answer their question and do not edit files.\n\
             \n\
             Go on",
            "x".repeat(500)
        );
        assert_eq!(prompt, expected);
    }

    fn guided_plan() -> Plan {
        Plan {
            goal: "Print results as JSON when --json is set".into(),
            done_when: vec![
                "`--json` prints valid JSON".into(),
                "The old output is unchanged".into(),
            ],
            steps: vec![
                step(StepKind::EditFile, "src/cli.rs", "Add `json` to Args"),
                step(StepKind::RunCommand, "cargo add serde", "Add serde"),
                step(StepKind::CreateFile, "tests/cli.rs", "Add a test"),
            ],
        }
    }

    #[test]
    fn guidance_names_the_step_and_the_level() {
        let plan = guided_plan();
        assert_eq!(
            guidance(&plan, 0, HelpLevel::Beginner),
            "[Tandem] Step 1 of 3 is now active.\n\
             Step: Edit src/cli.rs\n\
             Summary: Add `json` to Args\n\
             Language: rust. Help level: beginner.\n\
             Call show_step for exactly this step. Put the complete code for this step in code and add a breakdown entry for every keyword, construct and function call in that code, each part copied exactly from the code. Set location to where the change goes in the file as it is now: action (insert, update or remove), line (the number of the existing line the new code goes next to, or of the first line to replace or remove), anchor (the exact text of that line), end_line (the last line to replace or remove, when there are several) and placement (before or after the anchor line, for an insert; after is the default). Read the file first, so that the line number and the anchor are exact. Explain the code on its own terms: do not compare it to Python or any other programming language unless the user asked for that. Do not edit files and do not move to another step. After the tool call, reply with one short sentence."
        );
    }

    #[test]
    fn guidance_gives_each_level_its_rule_for_files_and_commands() {
        let plan = guided_plan();
        let cases = [
            (
                0,
                HelpLevel::Intermediate,
                "Describe the change, say in where, in words, where it goes (for example: in src/cli.rs, inside struct Args, after the verbose field), put pseudocode in pseudocode and leave code empty, and add breakdown entries only for what is new or unusual in rust.",
            ),
            (
                0,
                HelpLevel::Expert,
                "Keep it short: summary, why and where (in words, where the change goes); leave code, pseudocode and breakdown empty.",
            ),
            (
                1,
                HelpLevel::Beginner,
                "Put the exact command in command and add a breakdown entry for every token of it, each part copied exactly from the command.",
            ),
            (
                1,
                HelpLevel::Intermediate,
                "Put the exact command in command and explain in breakdown only the parts that are new or unusual, each part copied exactly from the command.",
            ),
            (
                1,
                HelpLevel::Expert,
                "Put the exact command in command; summary and why only.",
            ),
            (
                2,
                HelpLevel::Beginner,
                "Put the complete code for this step in code and add a breakdown entry for every keyword, construct and function call in that code, each part copied exactly from the code. Leave location out, because the file is new.",
            ),
            (
                2,
                HelpLevel::Intermediate,
                "Describe the change, put pseudocode in pseudocode and leave code empty, and add breakdown entries only for what is new or unusual in rust.",
            ),
            (
                2,
                HelpLevel::Expert,
                "Keep it short: summary and why only; leave code, pseudocode and breakdown empty.",
            ),
        ];
        for (index, level, rule) in cases {
            let prompt = guidance(&plan, index, level);
            let step = &plan.steps[index];
            assert!(
                prompt.contains(&format!("Step {} of 3 is now active.", index + 1)),
                "{prompt}"
            );
            assert!(prompt.contains(&step.target), "{prompt}");
            assert!(
                prompt.contains(&format!("Help level: {}.", level.id())),
                "{prompt}"
            );
            assert!(prompt.contains(rule), "{level:?} {index}: {prompt}");
            assert!(
                prompt.ends_with("Do not edit files and do not move to another step. After the tool call, reply with one short sentence."),
                "{prompt}"
            );
        }
        assert!(guidance(&plan, 1, HelpLevel::Beginner).contains("Step: Run cargo add serde\n"));
        assert!(guidance(&plan, 2, HelpLevel::Expert).contains("Step: Create tests/cli.rs\n"));
    }

    #[test]
    fn check_names_the_step_and_any_unsaved_files() {
        let plan = guided_plan();
        assert_eq!(
            check(&plan, 0, HelpLevel::Intermediate, &[]),
            "[Tandem] The user clicked \"Check my work\" for step 1 of 3: Edit src/cli.rs. Add `json` to Args\n\
             Help level: intermediate.\n\
             Inspect the user's work for this step only. You may run build, test or lint commands. Then call report_check with scope \"step\": verdict \"pass\" when the step is done correctly, or \"needs_changes\" with what to fix and why, written for the help level. Explain the code on its own terms: do not compare it to Python or any other programming language unless the user asked for that. Do not edit files."
        );
        let unsaved = ["src/cli.rs".to_owned(), "src/main.rs".to_owned()];
        assert_eq!(
            check(&plan, 1, HelpLevel::Beginner, &unsaved),
            "[Tandem] The user clicked \"Check my work\" for step 2 of 3: Run cargo add serde. Add serde\n\
             Help level: beginner.\n\
             These files have unsaved changes in the editor: src/cli.rs, src/main.rs. Read them with read_editor_buffer, because the Read tool shows the older saved version. Commands you run see only saved files; say so if it matters.\n\
             Inspect the user's work for this step only. You may run build, test or lint commands. Then call report_check with scope \"step\": verdict \"pass\" when the step is done correctly, or \"needs_changes\" with what to fix and why, written for the help level. Explain the code on its own terms: do not compare it to Python or any other programming language unless the user asked for that. Do not edit files."
        );
    }

    #[test]
    fn final_check_numbers_every_criterion() {
        assert_eq!(
            final_check(&guided_plan()),
            "[Tandem] All 3 steps are finished. Check each \"done when\" criterion:\n\
             1. `--json` prints valid JSON\n\
             2. The old output is unchanged\n\
             You may run build, test or lint commands. Then call report_check with scope \"task\": \"pass\" only when every criterion is met, otherwise \"needs_changes\" naming each criterion that fails and why. Do not edit files."
        );
    }

    #[test]
    fn takeover_step_limits_claude_to_the_one_step() {
        assert_eq!(
            takeover_step(&guided_plan(), 0),
            "[Tandem] Takeover is active for step 1 of 3 only: Edit src/cli.rs. Add `json` to Args\n\
             Do this step yourself now: edit files inside the project folder, or run the command. Do not do later steps. When the step is finished and verified, call step_done with step 1 and one sentence that says what you did and why."
        );
        assert!(takeover_step(&guided_plan(), 1).starts_with(
            "[Tandem] Takeover is active for step 2 of 3 only: Run cargo add serde. Add serde\n"
        ));
        assert!(takeover_step(&guided_plan(), 1).contains("call step_done with step 2 and"));
    }

    #[test]
    fn takeover_rest_lists_the_remaining_steps_and_the_criteria() {
        assert_eq!(
            takeover_rest(&guided_plan(), 1),
            "[Tandem] Takeover is active for all remaining steps, starting at step 2 of 3.\n\
             2. Run cargo add serde: Add serde\n\
             3. Create tests/cli.rs: Add a test\n\
             Do them in order yourself and stay inside the project folder. After each step call step_done with its number. When every step is finished, check each \"done when\" criterion, fix what fails, and call report_check with scope \"task\":\n\
             1. `--json` prints valid JSON\n\
             2. The old output is unchanged\n\
             If you need a decision from the user, stop and ask in chat."
        );
        let all = takeover_rest(&guided_plan(), 0);
        assert!(all.starts_with(
            "[Tandem] Takeover is active for all remaining steps, starting at step 1 of 3.\n1. Edit src/cli.rs: Add `json` to Args\n2. Run"
        ));
    }

    #[test]
    fn takeover_rest_with_no_step_left_asks_only_for_the_criteria() {
        assert_eq!(
            takeover_rest(&guided_plan(), 3),
            "[Tandem] Takeover is active. Every step is finished but the task does not meet its \"done when\" criteria yet. Fix what is missing inside the project folder, then call report_check with scope \"task\":\n\
             1. `--json` prints valid JSON\n\
             2. The old output is unchanged\n\
             If you need a decision from the user, stop and ask in chat."
        );
    }

    #[test]
    fn takeover_chat_names_the_next_step_and_keeps_slash_commands() {
        let plan = guided_plan();
        assert_eq!(
            takeover_chat(&plan, 1, "yes, go on"),
            "[Tandem] Takeover is still active for the remaining steps (next: step 2 of 3).\n\nyes, go on"
        );
        assert_eq!(
            takeover_chat(&plan, 3, "Use serde_json."),
            "[Tandem] Takeover is still active. Every step is finished.\n\nUse serde_json."
        );
        assert_eq!(takeover_chat(&plan, 0, "/compact"), "/compact");
    }

    #[test]
    fn the_recap_without_a_plan_or_messages_is_one_line() {
        let task = ChatTask::new("/project");
        assert_eq!(
            chat(&task, &[], "Hi", true),
            "[Tandem] The earlier conversation with you could not be restored, so here is where things stand.\n\nHi"
        );
    }
}
