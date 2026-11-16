//! Pure decisions the chat panel makes from a `ChatState`, kept here so they are tested
//! without a UI.

use crate::protocol::ModelChoice;
use crate::state::{ChatState, Connection};
use tandem_core::chat::{ActivityKind, ItemKind, Phase};

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

/// Model text ready for the Markdown renderer, with every image turned into text so that
/// nothing remote is ever loaded: `![alt](url)` becomes `alt (url)`, and any other `![`
/// (reference images) loses its image meaning. Code blocks and code spans are unchanged.
pub fn display_markdown(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut fence: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| indent <= 3 && rest.starts_with(marker));
        match (fence, marker) {
            (None, Some(marker)) => {
                fence = Some(marker);
                shown.push_str(line);
            }
            (Some(open), Some(marker)) if open == marker => {
                fence = None;
                shown.push_str(line);
            }
            (Some(_), _) => shown.push_str(line),
            (None, None) => rewrite_images(line, &mut shown),
        }
    }
    shown
}

fn rewrite_images(line: &str, shown: &mut String) {
    let mut rest = line;
    while let Some(position) = rest.find(['`', '!']) {
        shown.push_str(&rest[..position]);
        rest = &rest[position..];
        if rest.starts_with('`') {
            // A code span runs to the next run of the same number of backticks.
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            let end = rest[ticks..]
                .match_indices(fence)
                .find(|(index, _)| !rest[ticks + index + ticks..].starts_with('`'))
                .map(|(index, _)| ticks + index + ticks);
            let taken = end.unwrap_or(ticks);
            shown.push_str(&rest[..taken]);
            rest = &rest[taken..];
        } else if rest.starts_with("![") {
            match inline_image(&rest[2..]) {
                Some((alt, url, used)) => {
                    if alt.is_empty() {
                        shown.push_str(url);
                    } else {
                        // The alt text may hold another image.
                        rewrite_images(alt, shown);
                        shown.push_str(" (");
                        shown.push_str(url);
                        shown.push(')');
                    }
                    rest = &rest[2 + used..];
                }
                None => {
                    shown.push_str("\\![");
                    rest = &rest[2..];
                }
            }
        } else {
            shown.push('!');
            rest = &rest[1..];
        }
    }
    shown.push_str(rest);
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
    use tandem_core::chat::{ChatItem, ChatTask, ItemKind, Plan, PlanStep, StepKind};

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
        // A reference image becomes the literal "!" and a link.
        assert_eq!(
            display_markdown("![logo][1]\n\n[1]: http://a/b.png"),
            "\\![logo][1]\n\n[1]: http://a/b.png"
        );
        // Code is shown as written.
        let code = "Use `![x](y)` here.\n```md\n![x](http://a/b.png)\n```\n";
        assert_eq!(display_markdown(code), code);
        // Links and ordinary text are untouched.
        let plain = "Wow! [docs](http://a) and `a!` [x]";
        assert_eq!(display_markdown(plain), plain);
    }
}
