//! Pure decisions the chat panel makes from a `ChatState`, kept here so they are tested
//! without a UI.

use crate::protocol::ModelChoice;
use crate::state::{ChatState, Connection, TurnKind};
use tandem_core::chat::{
    ActivityKind, ChangeKind, Decision, Guidance, HelpLevel, ItemKind, PermissionMode, Phase,
    StepKind, normalize_language,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMode {
    Send,
    Stop,
    Stopping,
    /// The reason the composer cannot send.
    Disabled(&'static str),
}

pub fn composer_mode(state: &ChatState, draft_is_empty: bool) -> ComposerMode {
    if state.turn.is_some() {
        if state.stopping {
            ComposerMode::Stopping
        } else {
            ComposerMode::Stop
        }
    } else if !matches!(state.connection, Connection::Ready { .. }) {
        ComposerMode::Disabled("Claude is not connected.")
    } else if draft_is_empty {
        ComposerMode::Disabled("Type a message.")
    } else {
        ComposerMode::Send
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailState {
    Done,
    Current,
    Upcoming,
}

/// The "Discuss › Plan › Build" trail in the panel header.
pub fn phase_trail(phase: Phase) -> [(&'static str, TrailState); 3] {
    let current = match phase {
        Phase::Discuss => 0,
        Phase::Plan => 1,
        Phase::Build => 2,
        Phase::Done => 3,
    };
    let mut trail = [
        ("Discuss", TrailState::Upcoming),
        ("Plan", TrailState::Upcoming),
        ("Build", TrailState::Upcoming),
    ];
    for (index, (_, state)) in trail.iter_mut().enumerate() {
        *state = match index.cmp(&current) {
            std::cmp::Ordering::Less => TrailState::Done,
            std::cmp::Ordering::Equal => TrailState::Current,
            std::cmp::Ordering::Greater => TrailState::Upcoming,
        };
    }
    trail
}

/// One line of Claude's activity, such as "read src/cli.rs".
pub fn activity_text(kind: ActivityKind, label: &str) -> String {
    match kind {
        ActivityKind::Read => format!("read {label}"),
        ActivityKind::Search => format!("searched {label}"),
        ActivityKind::WebSearch => format!("searched the web: {label}"),
        ActivityKind::WebFetch => format!("fetched {label}"),
        ActivityKind::Command => format!("ran `{label}`"),
        ActivityKind::Edit => format!("edited {label}"),
        ActivityKind::Tool => format!("used {label}"),
    }
}

fn selected_model(state: &ChatState) -> Option<&ModelChoice> {
    state
        .models
        .iter()
        .find(|model| model.value == state.prefs.model)
}

/// The selected model's effort levels; empty hides the effort picker.
pub fn effort_choices(state: &ChatState) -> Vec<String> {
    selected_model(state)
        .map(|model| model.efforts.clone())
        .unwrap_or_default()
}

pub fn model_label(state: &ChatState) -> String {
    selected_model(state).map_or_else(|| "Default".to_owned(), |model| model.name.clone())
}

/// One entry of the Permissions menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionChoice {
    pub mode: PermissionMode,
    pub label: &'static str,
    pub checked: bool,
    /// False while Claude works for a choice that would reopen the session.
    pub enabled: bool,
}

/// The Permissions button and its menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPicker {
    pub label: String,
    /// Shown in the warning colour: Bypass is on.
    pub warn: bool,
    pub tooltip: &'static str,
    pub choices: [PermissionChoice; 3],
}

/// The Permissions picker for the saved mode. Switching to or from Bypass reopens the
/// session, so while Claude works only the choices on the same side of Bypass are enabled.
pub fn permission_picker(state: &ChatState) -> PermissionPicker {
    let saved = state.prefs.permission_mode;
    let tooltip = match saved {
        PermissionMode::Ask => {
            "Claude asks before it runs a command that Tandem does not allow by itself."
        }
        PermissionMode::Auto => {
            "Claude Code's safety check lets routine commands run and asks only about risky ones. A command that runs outside a takeover has no snapshot, so Undo cannot reverse what it changes."
        }
        PermissionMode::Bypass => {
            "Nothing asks: every command runs, and Claude may edit project files in any turn. Outside a takeover there is no snapshot, so there is no Keep or Undo for what Claude changes."
        }
    };
    let choices = PermissionMode::ALL.map(|mode| PermissionChoice {
        mode,
        label: match mode {
            PermissionMode::Ask => "Ask · you approve commands",
            PermissionMode::Auto => "Auto · asks only about risky actions",
            PermissionMode::Bypass => "Bypass · never asks, may edit files",
        },
        checked: mode == saved,
        enabled: state.turn.is_none()
            || (mode == PermissionMode::Bypass) == (saved == PermissionMode::Bypass),
    });
    PermissionPicker {
        label: format!("Permissions: {}", saved.label()),
        warn: saved == PermissionMode::Bypass,
        tooltip,
        choices,
    }
}

/// What the panel shows instead of the composer while Claude is not ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionNotice {
    pub title: String,
    pub body: String,
    /// A command for the user to run in a terminal.
    pub command: Option<String>,
    /// Whether a Reconnect button is offered.
    pub can_retry: bool,
}

pub fn connection_notice(connection: &Connection) -> Option<ConnectionNotice> {
    let notice = |title: &str, body: String, command: Option<String>, can_retry: bool| {
        Some(ConnectionNotice {
            title: title.to_owned(),
            body,
            command,
            can_retry,
        })
    };
    match connection {
        Connection::Ready { .. } => None,
        Connection::Starting => notice("Starting Claude…", String::new(), None, false),
        Connection::Unavailable { reason, hint } => {
            let body = if hint.is_empty() {
                reason.clone()
            } else {
                format!("{reason} {hint}")
            };
            notice("Claude is unavailable", body, None, true)
        }
        Connection::SignedOut { hint } => notice(
            "Sign in to Claude",
            "Tandem uses your Claude subscription through Claude Code. Run this command in a terminal, then press Reconnect.".to_owned(),
            Some(hint.clone()),
            true,
        ),
        Connection::Refused { reason } => notice(
            "Claude subscription required",
            format!(
                "{reason} Tandem only works with a Claude subscription and never uses an API key."
            ),
            None,
            true,
        ),
        Connection::Failed { message } => notice("Claude stopped", message.clone(), None, true),
    }
}

/// Whether the plan item `item_id` offers "I'll write it" and "You do it": only the latest
/// plan, while the task is in Plan, Claude is idle and connected.
pub fn plan_actions_visible(state: &ChatState, item_id: i64) -> bool {
    let latest_plan = state
        .items
        .iter()
        .rev()
        .find(|item| matches!(item.kind, ItemKind::Plan { .. }))
        .map(|item| item.id);
    latest_plan == Some(item_id)
        && state
            .task
            .as_ref()
            .is_some_and(|task| task.phase == Phase::Plan)
        && state.turn.is_none()
        && matches!(state.connection, Connection::Ready { .. })
}

/// The card pinned above the composer while a step of the build is open.
#[derive(Clone, Debug, PartialEq)]
pub struct StepCard {
    /// The step's index in the plan; the card shows `index + 1`.
    pub index: usize,
    pub total: usize,
    pub kind: StepKind,
    pub target: String,
    pub summary: String,
    pub language: String,
    pub level: HelpLevel,
    pub guidance: Option<Guidance>,
    /// A Guidance turn for this step is running.
    pub preparing: bool,
    /// Claude is connected and no turn runs.
    pub can_act: bool,
}

/// The active step's card: `Some` in Build while a step is still open.
pub fn step_card(state: &ChatState) -> Option<StepCard> {
    let task = state
        .task
        .as_ref()
        .filter(|task| task.phase == Phase::Build)?;
    let steps = &task.plan.as_ref()?.steps;
    let index = task.current_step;
    let step = steps.get(index)?;
    Some(StepCard {
        index,
        total: steps.len(),
        kind: step.kind,
        target: step.target.clone(),
        summary: step.summary.clone(),
        language: step.language.clone(),
        level: level_for(state, &step.language),
        guidance: state
            .guidance
            .clone()
            .filter(|guidance| guidance.step == index),
        preparing: matches!(state.turn, Some(TurnKind::Guidance { step, .. }) if step == index),
        can_act: matches!(state.connection, Connection::Ready { .. }) && state.turn.is_none(),
    })
}

/// Whether the "All steps are finished" card shows: Build with every step finished.
pub fn final_card(state: &ChatState) -> bool {
    state.task.as_ref().is_some_and(|task| {
        task.phase == Phase::Build
            && task
                .plan
                .as_ref()
                .is_some_and(|plan| task.current_step >= plan.steps.len())
    })
}

/// Whether Claude is editing files: a takeover turn runs or its snapshot is being taken.
pub fn takeover_running(state: &ChatState) -> bool {
    state.preparing_takeover || matches!(state.turn, Some(TurnKind::Takeover { .. }))
}

/// One file of a takeover's changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRow {
    pub path: String,
    pub kind: ChangeKind,
    pub decision: Decision,
    /// Undo can restore the earlier version.
    pub undoable: bool,
}

/// The card for a `Changes` item: what one takeover changed, with Keep and Undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesCard {
    pub id: String,
    pub rows: Vec<ChangeRow>,
    /// The files still waiting for Keep or Undo.
    pub undecided: usize,
    /// No turn runs and no takeover is being prepared; the connection does not matter.
    pub can_act: bool,
}

/// The card for the active task's change set `change_set`, if it has one by that id.
pub fn changes_card(state: &ChatState, change_set: &str) -> Option<ChangesCard> {
    let set = state.change_sets.iter().find(|set| set.id == change_set)?;
    let rows: Vec<ChangeRow> = set
        .files
        .iter()
        .map(|file| ChangeRow {
            path: file.path.clone(),
            kind: file.kind,
            decision: file.decision,
            undoable: file.undoable,
        })
        .collect();
    Some(ChangesCard {
        id: set.id.clone(),
        undecided: rows
            .iter()
            .filter(|row| row.decision == Decision::Undecided)
            .count(),
        rows,
        can_act: state.turn.is_none() && !state.preparing_takeover,
    })
}

/// The saved help level for `language`, Beginner when none was chosen.
pub fn level_for(state: &ChatState, language: &str) -> HelpLevel {
    state.prefs.help_level(&normalize_language(language))
}

/// Model text ready for the Markdown renderer, with every image turned into text so that
/// nothing remote is ever loaded. `![alt](url)` becomes `alt (url)`, any other `![` becomes
/// `!\[`, and the `<` of an `<img` or `<image` tag becomes `&lt;`. This applies everywhere,
/// code included: no `![` and no HTML image tag is left for the renderer to find.
pub fn display_markdown(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    rewrite_inline_images(text, &mut shown);
    // The rewrite can join a "!" to a "[" that follows it, so escaping comes after it.
    disarm_image_tags(&shown.replace("![", "!\\["))
}

fn rewrite_inline_images(text: &str, shown: &mut String) {
    let mut rest = text;
    while let Some(position) = rest.find("![") {
        shown.push_str(&rest[..position]);
        rest = &rest[position..];
        match inline_image(&rest[2..]) {
            Some((alt, url, used)) => {
                if alt.is_empty() {
                    shown.push_str(url);
                } else {
                    // The alt text may hold another image.
                    rewrite_inline_images(alt, shown);
                    shown.push_str(" (");
                    shown.push_str(url);
                    shown.push(')');
                }
                rest = &rest[2 + used..];
            }
            None => {
                shown.push('!');
                rest = &rest[1..];
            }
        }
    }
    shown.push_str(rest);
}

/// Turns the `<` of every `<img` and `<image` (any case) into `&lt;`, so the renderer shows
/// the tag as text. HTML parsers treat `<image` as `<img`.
fn disarm_image_tags(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('<') {
        shown.push_str(&rest[..position]);
        rest = &rest[position + 1..];
        let names_an_image = ["img", "image"].into_iter().any(|name| {
            rest.get(..name.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(name))
        });
        shown.push_str(if names_an_image { "&lt;" } else { "<" });
    }
    shown.push_str(rest);
    shown
}

/// Parses `alt](url)` (the part after `![`), allowing balanced brackets in the alt text.
/// Returns the alt text, the destination and the number of bytes used.
fn inline_image(text: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0usize;
    let mut alt_end = None;
    for (index, character) in text.char_indices() {
        match character {
            '[' => depth += 1,
            ']' if depth == 0 => {
                alt_end = Some(index);
                break;
            }
            ']' => depth -= 1,
            _ => {}
        }
    }
    let alt_end = alt_end?;
    let after = text[alt_end + 1..].strip_prefix('(')?;
    let url_length = after.find(')')?;
    Some((
        &text[..alt_end],
        &after[..url_length],
        alt_end + 2 + url_length + 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TurnKind;
    use tandem_core::chat::{
        ChangeSet, ChangeSetStatus, ChatItem, ChatTask, FileChange, ItemKind, Plan, PlanStep,
        TakeoverScope,
    };

    fn ready() -> Connection {
        Connection::Ready {
            subscription: "Claude Ultra".into(),
            claude_code: "2.1.325".into(),
        }
    }

    fn model(value: &str, name: &str, efforts: &[&str]) -> ModelChoice {
        ModelChoice {
            value: value.into(),
            name: name.into(),
            description: String::new(),
            efforts: efforts.iter().map(|effort| effort.to_string()).collect(),
        }
    }

    fn state() -> ChatState {
        let mut state = ChatState::starting();
        state.connection = ready();
        state.models = vec![
            model("default", "Default (recommended)", &["low", "high"]),
            model("opus", "Opus", &["low", "high", "max"]),
            model("haiku", "Haiku", &[]),
        ];
        state
    }

    fn plan() -> Plan {
        Plan {
            goal: "g".into(),
            done_when: vec!["d".into()],
            steps: vec![PlanStep {
                kind: StepKind::EditFile,
                target: "src/a.rs".into(),
                summary: "s".into(),
                language: "rust".into(),
            }],
        }
    }

    fn item(id: i64, kind: ItemKind) -> ChatItem {
        ChatItem {
            id,
            created_at_ms: id,
            kind,
        }
    }

    #[test]
    fn composer_mode_follows_the_turn_the_connection_and_the_draft() {
        let mut state = state();
        assert_eq!(composer_mode(&state, false), ComposerMode::Send);
        assert_eq!(
            composer_mode(&state, true),
            ComposerMode::Disabled("Type a message.")
        );
        state.turn = Some(TurnKind::Chat);
        assert_eq!(composer_mode(&state, true), ComposerMode::Stop);
        state.stopping = true;
        assert_eq!(composer_mode(&state, false), ComposerMode::Stopping);
        state.turn = None;
        state.stopping = false;
        state.connection = Connection::Starting;
        assert_eq!(
            composer_mode(&state, false),
            ComposerMode::Disabled("Claude is not connected.")
        );
    }

    #[test]
    fn phase_trail_marks_each_phase() {
        use TrailState::{Current, Done, Upcoming};
        assert_eq!(
            phase_trail(Phase::Discuss),
            [
                ("Discuss", Current),
                ("Plan", Upcoming),
                ("Build", Upcoming)
            ]
        );
        assert_eq!(
            phase_trail(Phase::Plan),
            [("Discuss", Done), ("Plan", Current), ("Build", Upcoming)]
        );
        assert_eq!(
            phase_trail(Phase::Build),
            [("Discuss", Done), ("Plan", Done), ("Build", Current)]
        );
        assert_eq!(
            phase_trail(Phase::Done),
            [("Discuss", Done), ("Plan", Done), ("Build", Done)]
        );
    }

    #[test]
    fn activity_text_describes_each_kind() {
        let cases = [
            (ActivityKind::Read, "read src/cli.rs"),
            (ActivityKind::Search, "searched src/cli.rs"),
            (ActivityKind::WebSearch, "searched the web: src/cli.rs"),
            (ActivityKind::WebFetch, "fetched src/cli.rs"),
            (ActivityKind::Command, "ran `src/cli.rs`"),
            (ActivityKind::Edit, "edited src/cli.rs"),
            (ActivityKind::Tool, "used src/cli.rs"),
        ];
        for (kind, text) in cases {
            assert_eq!(activity_text(kind, "src/cli.rs"), text);
        }
    }

    #[test]
    fn effort_choices_are_the_selected_models_efforts() {
        let mut state = state();
        assert_eq!(effort_choices(&state), ["low", "high"]);
        state.prefs.model = "opus".into();
        assert_eq!(effort_choices(&state), ["low", "high", "max"]);
        state.prefs.model = "haiku".into();
        assert!(effort_choices(&state).is_empty());
        state.prefs.model = "gone".into();
        assert!(effort_choices(&state).is_empty());
    }

    #[test]
    fn model_label_names_the_selected_model_or_default() {
        let mut state = state();
        assert_eq!(model_label(&state), "Default (recommended)");
        state.prefs.model = "opus".into();
        assert_eq!(model_label(&state), "Opus");
        state.models.clear();
        assert_eq!(model_label(&state), "Default");
    }

    fn picker_for(mode: PermissionMode) -> PermissionPicker {
        let mut state = state();
        state.prefs.permission_mode = mode;
        permission_picker(&state)
    }

    #[test]
    fn the_permission_picker_names_the_saved_mode() {
        let labels = [
            (PermissionMode::Ask, "Permissions: Ask"),
            (PermissionMode::Auto, "Permissions: Auto"),
            (PermissionMode::Bypass, "Permissions: Bypass"),
        ];
        for (mode, label) in labels {
            let picker = picker_for(mode);
            assert_eq!(picker.label, label);
            let checked: Vec<PermissionMode> = picker
                .choices
                .iter()
                .filter(|choice| choice.checked)
                .map(|choice| choice.mode)
                .collect();
            assert_eq!(checked, [mode]);
            assert_eq!(
                picker.choices.map(|choice| (choice.mode, choice.label)),
                [
                    (PermissionMode::Ask, "Ask · you approve commands"),
                    (PermissionMode::Auto, "Auto · asks only about risky actions"),
                    (
                        PermissionMode::Bypass,
                        "Bypass · never asks, may edit files"
                    ),
                ]
            );
        }
    }

    #[test]
    fn only_bypass_is_shown_as_a_warning() {
        assert!(!picker_for(PermissionMode::Ask).warn);
        assert!(!picker_for(PermissionMode::Auto).warn);
        assert!(picker_for(PermissionMode::Bypass).warn);
    }

    #[test]
    fn the_auto_and_bypass_tooltips_say_what_undo_cannot_reverse() {
        assert_eq!(
            picker_for(PermissionMode::Ask).tooltip,
            "Claude asks before it runs a command that Tandem does not allow by itself."
        );
        let auto = picker_for(PermissionMode::Auto).tooltip;
        assert!(auto.contains("has no snapshot, so Undo cannot reverse"));
        let bypass = picker_for(PermissionMode::Bypass).tooltip;
        assert!(bypass.contains("may edit project files in any turn"));
        assert!(bypass.contains("no Keep or Undo"));
    }

    #[test]
    fn bypass_cannot_be_switched_while_claude_works() {
        let enabled = |state: &ChatState| {
            permission_picker(state)
                .choices
                .map(|choice| choice.enabled)
        };
        let mut state = state();
        for mode in PermissionMode::ALL {
            state.prefs.permission_mode = mode;
            assert_eq!(enabled(&state), [true, true, true], "{mode:?} while idle");
        }

        state.turn = Some(TurnKind::Chat);
        state.prefs.permission_mode = PermissionMode::Ask;
        assert_eq!(enabled(&state), [true, true, false]);
        state.prefs.permission_mode = PermissionMode::Auto;
        assert_eq!(enabled(&state), [true, true, false]);
        state.prefs.permission_mode = PermissionMode::Bypass;
        assert_eq!(enabled(&state), [false, false, true]);
    }

    #[test]
    fn connection_notice_explains_every_state_but_ready() {
        assert_eq!(connection_notice(&ready()), None);

        let starting = connection_notice(&Connection::Starting).unwrap();
        assert_eq!(starting.title, "Starting Claude…");
        assert!(!starting.can_retry);

        let unavailable = connection_notice(&Connection::Unavailable {
            reason: "Claude is switched off for this run.".into(),
            hint: "Unset TANDEM_CLAUDE_OFFLINE and restart Tandem.".into(),
        })
        .unwrap();
        assert_eq!(unavailable.title, "Claude is unavailable");
        assert_eq!(
            unavailable.body,
            "Claude is switched off for this run. Unset TANDEM_CLAUDE_OFFLINE and restart Tandem."
        );
        assert_eq!(unavailable.command, None);
        assert!(unavailable.can_retry);
        let no_hint = connection_notice(&Connection::Unavailable {
            reason: "Node.js 20 or newer was not found.".into(),
            hint: String::new(),
        })
        .unwrap();
        assert_eq!(no_hint.body, "Node.js 20 or newer was not found.");

        let signed_out = connection_notice(&Connection::SignedOut {
            hint: "claude auth login".into(),
        })
        .unwrap();
        assert_eq!(signed_out.title, "Sign in to Claude");
        assert_eq!(
            signed_out.body,
            "Tandem uses your Claude subscription through Claude Code. Run this command in a terminal, then press Reconnect."
        );
        assert_eq!(signed_out.command.as_deref(), Some("claude auth login"));
        assert!(signed_out.can_retry);

        let refused = connection_notice(&Connection::Refused {
            reason: "Claude Code would bill an API key (ANTHROPIC_API_KEY) instead of your subscription.".into(),
        })
        .unwrap();
        assert_eq!(refused.title, "Claude subscription required");
        assert_eq!(
            refused.body,
            "Claude Code would bill an API key (ANTHROPIC_API_KEY) instead of your subscription. Tandem only works with a Claude subscription and never uses an API key."
        );
        assert!(refused.can_retry);

        let failed = connection_notice(&Connection::Failed {
            message: "Claude stopped unexpectedly.".into(),
        })
        .unwrap();
        assert_eq!(failed.title, "Claude stopped");
        assert_eq!(failed.body, "Claude stopped unexpectedly.");
        assert!(failed.can_retry);
    }

    #[test]
    fn plan_actions_show_only_on_the_last_plan_while_idle_and_ready() {
        let mut state = state();
        let mut task = ChatTask::new("/project");
        task.phase = Phase::Plan;
        state.task = Some(task);
        state.items = vec![
            item(1, ItemKind::Plan { plan: plan() }),
            item(2, ItemKind::Assistant { text: "a".into() }),
            item(3, ItemKind::Plan { plan: plan() }),
            item(4, ItemKind::Assistant { text: "b".into() }),
        ];
        assert!(plan_actions_visible(&state, 3));
        assert!(!plan_actions_visible(&state, 1));
        assert!(!plan_actions_visible(&state, 4));

        state.turn = Some(TurnKind::Chat);
        assert!(!plan_actions_visible(&state, 3));
        state.turn = None;
        state.connection = Connection::Starting;
        assert!(!plan_actions_visible(&state, 3));
        state.connection = ready();
        state.task.as_mut().unwrap().phase = Phase::Build;
        assert!(!plan_actions_visible(&state, 3));
        state.task = None;
        assert!(!plan_actions_visible(&state, 3));
    }

    fn guided_state(current_step: usize) -> ChatState {
        let mut state = state();
        let mut task = ChatTask::new("/project");
        task.phase = Phase::Build;
        task.current_step = current_step;
        task.plan = Some(Plan {
            goal: "g".into(),
            done_when: vec!["d".into()],
            steps: vec![
                PlanStep {
                    kind: StepKind::EditFile,
                    target: "src/a.rs".into(),
                    summary: "Edit a".into(),
                    language: "rust".into(),
                },
                PlanStep {
                    kind: StepKind::RunCommand,
                    target: "go test ./...".into(),
                    summary: "Run the tests".into(),
                    language: "go".into(),
                },
            ],
        });
        state.task = Some(task);
        state
    }

    fn guidance(step: usize) -> Guidance {
        Guidance {
            step,
            level: HelpLevel::Expert,
            summary: "Add a field.".into(),
            why: "Because.".into(),
            code: None,
            pseudocode: None,
            command: None,
            breakdown: Vec::new(),
            path: Some("src/a.rs".into()),
            anchor: None,
        }
    }

    #[test]
    fn step_card_shows_the_open_step_with_its_level_and_guidance() {
        let mut state = guided_state(0);
        state
            .prefs
            .help_levels
            .insert("rust".into(), HelpLevel::Expert);
        state.guidance = Some(guidance(0));
        assert_eq!(
            step_card(&state),
            Some(StepCard {
                index: 0,
                total: 2,
                kind: StepKind::EditFile,
                target: "src/a.rs".into(),
                summary: "Edit a".into(),
                language: "rust".into(),
                level: HelpLevel::Expert,
                guidance: Some(guidance(0)),
                preparing: false,
                can_act: true,
            })
        );

        let mut state = guided_state(1);
        state.guidance = Some(guidance(0));
        state.turn = Some(TurnKind::Guidance {
            step: 1,
            level: HelpLevel::Beginner,
        });
        let card = step_card(&state).expect("a card");
        assert_eq!((card.index, card.kind), (1, StepKind::RunCommand));
        assert_eq!(card.level, HelpLevel::Beginner);
        assert_eq!(
            card.guidance, None,
            "guidance for another step is not shown"
        );
        assert!(card.preparing);
        assert!(!card.can_act);

        state.turn = Some(TurnKind::Check { step: 1 });
        let card = step_card(&state).unwrap();
        assert!(!card.preparing);
        assert!(!card.can_act);
        state.turn = None;
        state.connection = Connection::Starting;
        assert!(!step_card(&state).unwrap().can_act);
    }

    #[test]
    fn step_card_and_final_card_follow_the_phase_and_the_step() {
        let mut state = guided_state(2);
        assert_eq!(step_card(&state), None);
        assert!(final_card(&state));

        state.task.as_mut().unwrap().current_step = 1;
        assert!(step_card(&state).is_some());
        assert!(!final_card(&state));

        for phase in [Phase::Discuss, Phase::Plan, Phase::Done] {
            state.task.as_mut().unwrap().phase = phase;
            assert_eq!(step_card(&state), None, "{phase:?}");
            state.task.as_mut().unwrap().current_step = 2;
            assert!(!final_card(&state), "{phase:?}");
            state.task.as_mut().unwrap().current_step = 1;
        }
        state.task = None;
        assert_eq!(step_card(&state), None);
        assert!(!final_card(&state));
    }

    fn takeover_turn() -> TurnKind {
        TurnKind::Takeover {
            scope: TakeoverScope::Step(0),
            change_set: "set-1".into(),
        }
    }

    #[test]
    fn takeover_running_covers_the_turn_and_the_snapshot() {
        let mut state = guided_state(0);
        assert!(!takeover_running(&state));
        state.turn = Some(TurnKind::Chat);
        assert!(!takeover_running(&state));
        state.turn = Some(takeover_turn());
        assert!(takeover_running(&state));
        state.turn = None;
        state.preparing_takeover = true;
        assert!(takeover_running(&state));
    }

    fn change(path: &str, kind: ChangeKind, decision: Decision, undoable: bool) -> FileChange {
        FileChange {
            path: path.into(),
            kind,
            decision,
            after_hash: None,
            undoable,
        }
    }

    #[test]
    fn changes_card_lists_the_files_and_counts_the_undecided_ones() {
        let mut state = guided_state(1);
        state.change_sets = vec![
            ChangeSet {
                id: "set-0".into(),
                task_id: "t".into(),
                scope: TakeoverScope::Step(0),
                status: ChangeSetStatus::Resolved,
                files: vec![change("a.rs", ChangeKind::Created, Decision::Kept, true)],
                created_at_ms: 1,
            },
            ChangeSet {
                id: "set-1".into(),
                task_id: "t".into(),
                scope: TakeoverScope::Step(0),
                status: ChangeSetStatus::Review,
                files: vec![
                    change("src/a.rs", ChangeKind::Modified, Decision::Undecided, true),
                    change(
                        "src/big.bin",
                        ChangeKind::Modified,
                        Decision::Undecided,
                        false,
                    ),
                    change("src/main.rs", ChangeKind::Deleted, Decision::Undone, true),
                ],
                created_at_ms: 2,
            },
        ];
        let card = changes_card(&state, "set-1").expect("a card");
        assert_eq!(
            card,
            ChangesCard {
                id: "set-1".into(),
                rows: vec![
                    ChangeRow {
                        path: "src/a.rs".into(),
                        kind: ChangeKind::Modified,
                        decision: Decision::Undecided,
                        undoable: true,
                    },
                    ChangeRow {
                        path: "src/big.bin".into(),
                        kind: ChangeKind::Modified,
                        decision: Decision::Undecided,
                        undoable: false,
                    },
                    ChangeRow {
                        path: "src/main.rs".into(),
                        kind: ChangeKind::Deleted,
                        decision: Decision::Undone,
                        undoable: true,
                    },
                ],
                undecided: 2,
                can_act: true,
            }
        );
        assert_eq!(changes_card(&state, "set-0").unwrap().undecided, 0);
        assert_eq!(changes_card(&state, "missing"), None);

        // The buttons wait while any turn runs, also without a connection.
        state.turn = Some(TurnKind::Chat);
        assert!(!changes_card(&state, "set-1").unwrap().can_act);
        state.turn = None;
        state.connection = Connection::Starting;
        assert!(changes_card(&state, "set-1").unwrap().can_act);
        state.preparing_takeover = true;
        assert!(!changes_card(&state, "set-1").unwrap().can_act);
    }

    #[test]
    fn level_for_reads_the_saved_level_by_normalized_language() {
        let mut state = state();
        assert_eq!(level_for(&state, "rust"), HelpLevel::Beginner);
        state
            .prefs
            .help_levels
            .insert("typescript".into(), HelpLevel::Intermediate);
        assert_eq!(level_for(&state, "typescript"), HelpLevel::Intermediate);
        assert_eq!(level_for(&state, " TS "), HelpLevel::Intermediate);
    }

    #[test]
    fn display_markdown_never_shows_remote_images() {
        assert_eq!(
            display_markdown("![x](http://a/b.png)"),
            "x (http://a/b.png)"
        );
        assert_eq!(
            display_markdown("See ![a [b] c](http://a/b.png \"t\") and ![](http://c/d.png)."),
            "See a [b] c (http://a/b.png \"t\") and http://c/d.png."
        );
        assert_eq!(
            display_markdown("[![![a](b)](c)](http://d)"),
            "[a (b) (c)](http://d)"
        );
        // A reference image loses its image meaning: "!" and an escaped "[".
        assert_eq!(
            display_markdown("![logo][1]\n\n[1]: http://a/b.png"),
            "!\\[logo][1]\n\n[1]: http://a/b.png"
        );
        // Images inside code are rewritten too.
        assert_eq!(
            display_markdown("Use `![x](y)` here.\n```md\n![x](http://a/b.png)\n```\n"),
            "Use `x (y)` here.\n```md\nx (http://a/b.png)\n```\n"
        );
        // An HTML image tag is shown as text.
        assert_eq!(
            display_markdown("<img src=\"http://a/b.png\"> and <IMAGE src=x>"),
            "&lt;img src=\"http://a/b.png\"> and &lt;IMAGE src=x>"
        );
        // Links, other HTML and ordinary text are untouched.
        let plain = "Wow! [docs](http://a) and `a!` [x] <b>Vec<String></b> <http://a>";
        assert_eq!(display_markdown(plain), plain);
    }

    #[test]
    fn display_markdown_leaves_no_image_syntax_anywhere() {
        let hostile = [
            // An escaped backtick opens no code span.
            "\\`![x](http://evil.test/p.png)\\`",
            // A line with an info string does not close a fence.
            "```js\ncode\n```js\n```\n![x](http://evil.test/p.png)\n",
            // A backtick fence info string cannot hold a backtick, so this is no fence.
            "```a`b\n![x](http://evil.test/p.png)\n",
            "```a`b\n![x][1]\n\n[1]: http://evil.test/p.png\n",
            // A backslash before the rewrite must not escape the rewrite's own escape.
            "\\![x][1]\n\n[1]: http://evil.test/p.png",
            // Rewriting must not join a "!" or "<" to what follows into a new image.
            "!![]([x](http://evil.test/p.png))",
            "!![[x]](y)\n\n[x]: http://evil.test/p.png",
            "<![](img) src=\"http://evil.test/p.png\">",
            "<div>\n<img src=\"http://evil.test/p.png\">\n</div>\n",
        ];
        for text in hostile {
            let shown = display_markdown(text);
            let lowercase = shown.to_ascii_lowercase();
            assert!(
                !shown.contains("![")
                    && !lowercase.contains("<img")
                    && !lowercase.contains("<image"),
                "{text:?} became {shown:?}"
            );
        }
    }
}
