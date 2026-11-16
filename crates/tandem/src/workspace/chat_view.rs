//! The chat panel on the right: header, connection notice, transcript, the card of the
//! step being built, and composer.
use super::chat_logic::{check_title, finding_label};
use super::panels::code_note;
use super::*;
use crate::appearance::{
    Palette, compact_button, palette, panel_error, path_button, solid_section, wrapping_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_claude::prompts::kind_label;
use tandem_claude::view::{
    self, BlockKind, ChangeRow, ChangesCard, ComposerMode, ConnectionNotice, StepBlock, StepCard,
    TrailState,
};
use tandem_claude::{ChatState, Connection, TakeoverRequest};
use tandem_core::chat::{
    ChangeKind, ChatItem, CheckResult, DEFAULT_MODEL, Decision, Finding, Guidance, HelpLevel,
    ItemKind, Phase, Plan, StepKind,
};

const NO_PROJECT: &str = "Open a folder to start a task with Claude.";
const IGNORED_NOT_TRACKED: &str = "Files in ignored folders (like target/ or node_modules/) aren't tracked, so Undo can't restore them.";

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Discuss => "Discuss",
        Phase::Plan => "Plan",
        Phase::Build => "Build",
        Phase::Done => "Done",
    }
}

/// A menu item handler that sends `command` to the companion.
fn on_command(
    weak: &WeakEntity<Workspace>,
    command: Command,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let weak = weak.clone();
    move |_, _, cx| {
        let _ = weak.update(cx, |view, _| view.chat_command(command.clone()));
    }
}

/// Keep (`keep`) or Undo of `path` in `change_set`, or of every undecided file there when
/// `path` is `None`.
fn decide(keep: bool, change_set: &str, path: Option<String>) -> Command {
    let change_set = change_set.to_owned();
    if keep {
        Command::Keep { change_set, path }
    } else {
        Command::Undo { change_set, path }
    }
}

/// A button that puts `text` on the clipboard.
fn copy_button(id: impl Into<ElementId>, text: String) -> Button {
    compact_button(Button::new(id), "Copy")
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

/// The text of an optional guidance field, when it has any.
fn filled(text: &Option<String>) -> Option<String> {
    text.clone().filter(|text| !text.trim().is_empty())
}

/// A button labelled `label` whose menu sets the help level for `language`. Disabled, it
/// shows the label without a menu.
pub(super) fn help_level_menu(
    id: impl Into<ElementId>,
    label: String,
    language: String,
    level: HelpLevel,
    enabled: bool,
    weak: &WeakEntity<Workspace>,
) -> AnyElement {
    let button = compact_button(Button::new(id), label)
        .ghost()
        .accessibility_label(format!("Help level for {language}: {}", level.label()));
    if !enabled {
        return button.disabled(true).into_any_element();
    }
    let weak = weak.clone();
    button
        .dropdown_menu(move |menu, _, _| {
            HelpLevel::ALL.iter().fold(menu, |menu, choice| {
                menu.item(
                    PopupMenuItem::new(choice.label())
                        .checked(*choice == level)
                        .on_click(on_command(
                            &weak,
                            Command::SetHelpLevel {
                                language: language.clone(),
                                level: *choice,
                            },
                        )),
                )
            })
        })
        .into_any_element()
}

/// "You" or "Claude" above a message.
fn speaker(name: &'static str, p: Palette) -> Div {
    div()
        .text_size(px(12.))
        .font_semibold()
        .text_color(p.secondary_foreground)
        .child(name)
}

/// "Discuss › Plan › Build" with the current phase emphasised.
fn phase_trail(phase: Phase, p: Palette) -> AnyElement {
    let steps =
        view::phase_trail(phase)
            .into_iter()
            .enumerate()
            .flat_map(move |(index, (label, step))| {
                let separator = (index > 0).then(|| {
                    div()
                        .text_color(p.secondary_foreground)
                        .child("›")
                        .into_any_element()
                });
                let current = step == TrailState::Current;
                let label = div()
                    .text_color(if current {
                        p.foreground
                    } else {
                        p.secondary_foreground
                    })
                    .when(current, |d| d.font_semibold())
                    .child(label)
                    .into_any_element();
                separator.into_iter().chain(std::iter::once(label))
            });
    div()
        .id("chat-trail")
        .role(Role::Label)
        .aria_label(format!("Phase: {}", phase_name(phase)))
        .h_flex()
        .items_center()
        .gap_1()
        .text_size(px(12.))
        .children(steps)
        .into_any_element()
}

impl Workspace {
    pub(super) fn chat_panel(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let state = self.chat_state();
        let panel = div()
            .v_flex()
            .w(px(width))
            .min_w_0()
            .flex_shrink_0()
            .min_h_0()
            .border_l_1()
            .border_color(p.border)
            .child(self.chat_header(state.as_deref(), cx));
        match state {
            Some(state) => panel
                .child(self.chat_body(&state, cx))
                .children(self.build_card(&state, cx))
                .child(self.chat_composer(&state, cx)),
            None => panel.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .bg(p.surface)
                    .text_color(p.secondary_foreground)
                    .child(NO_PROJECT),
            ),
        }
        .into_any_element()
    }

    fn chat_header(&self, state: Option<&ChatState>, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let task_menu = state.map(|state| self.task_menu(state, cx));
        let phase = state.and_then(|state| state.task.as_ref().map(|task| task.phase));
        div()
            .h_flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .p_2()
            .bg(p.chrome())
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .text_size(px(14.))
                    .font_semibold()
                    .child("Chat · Claude"),
            )
            .children(task_menu)
            .children(phase.map(|phase| phase_trail(phase, p)))
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("chat-settings")
                    .icon(IconName::Settings2)
                    .ghost()
                    .small()
                    .accessibility_label("Settings")
                    .tooltip("Settings  ⌘,")
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            )
            .into_any_element()
    }

    /// The active task's title, with a menu to start a task or switch to another.
    fn task_menu(&self, state: &ChatState, cx: &mut Context<Self>) -> AnyElement {
        let busy = state.turn.is_some();
        let active = state.task.as_ref().map(|task| task.id.clone());
        let title = state
            .task
            .as_ref()
            .map_or_else(|| "Tasks".to_owned(), |task| task.title.clone());
        let tasks: Vec<(String, String, bool)> = state
            .tasks
            .iter()
            .map(|task| {
                (
                    task.id.clone(),
                    format!("{} · {}", task.title, phase_name(task.phase)),
                    active.as_deref() == Some(task.id.as_str()),
                )
            })
            .collect();
        let weak = cx.entity().downgrade();
        wrapping_button(Button::new("chat-task"), title)
            .ghost()
            .dropdown_menu(move |menu, _, _| {
                let menu = menu
                    .item(
                        PopupMenuItem::new("New task")
                            .disabled(busy)
                            .on_click(on_command(&weak, Command::NewTask)),
                    )
                    .separator();
                tasks.iter().fold(menu, |menu, (id, label, active)| {
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(*active)
                            .disabled(busy)
                            .on_click(on_command(&weak, Command::SelectTask(id.clone()))),
                    )
                })
            })
            .into_any_element()
    }

    fn chat_body(&self, state: &ChatState, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let notice = view::connection_notice(&state.connection)
            .map(|notice| self.connection_card(notice, state.turn.is_some(), cx));
        let items: Vec<AnyElement> = state
            .items
            .iter()
            .map(|item| self.chat_item(state, item, cx))
            .collect();
        let working = if state.preparing_takeover {
            Some("Preparing takeover…")
        } else if state.turn.is_some() && state.streaming.is_none() {
            Some("Claude is working…")
        } else {
            None
        };
        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(p.surface)
            .children(notice)
            .child(
                div()
                    .id("chat-transcript")
                    .track_scroll(&self.chat.scroll)
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .children(items)
                    .when_some(state.streaming.clone(), |d, text| {
                        d.child(
                            div()
                                .id("chat-streaming")
                                .v_flex()
                                .min_w_0()
                                .gap_1()
                                .child(speaker("Claude", p))
                                .child(
                                    TextView::markdown(
                                        "chat-streaming-text",
                                        view::display_markdown(&text),
                                    )
                                    .selectable(true),
                                ),
                        )
                    })
                    .when_some(working, |d, working| {
                        d.child(div().text_color(p.secondary_foreground).child(working))
                    }),
            )
            .into_any_element()
    }

    fn connection_card(
        &self,
        notice: ConnectionNotice,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let command = notice.command.clone();
        let can_retry = notice.can_retry;
        solid_section(div(), cx)
            .v_flex()
            .flex_shrink_0()
            .min_w_0()
            .gap_2()
            .m_2()
            .p_3()
            .border_1()
            .child(div().font_semibold().child(notice.title))
            .when(!notice.body.is_empty(), |d| {
                d.child(div().text_size(px(13.)).child(notice.body))
            })
            .when_some(command, |d, command| {
                let copied = command.clone();
                d.child(code_note("chat-sign-in", command, cx)).child(
                    compact_button(Button::new("chat-copy-sign-in"), "Copy").on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                        },
                    ),
                )
            })
            .when(can_retry, |d| {
                d.child(
                    compact_button(Button::new("chat-reconnect"), "Reconnect")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, _| this.chat_command(Command::Reconnect)),
                        ),
                )
            })
            .into_any_element()
    }

    fn chat_item(&self, state: &ChatState, item: &ChatItem, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let id = item.id as usize;
        let row = div().id(("chat-item", id)).v_flex().min_w_0().gap_1();
        let muted = |row: Stateful<Div>, text: String| {
            row.text_size(px(12.))
                .text_color(p.secondary_foreground)
                .child(text)
        };
        match &item.kind {
            ItemKind::User { text } => row
                .child(speaker("You", p))
                .child(div().min_w_0().child(text.clone())),
            ItemKind::Assistant { text } => row.child(speaker("Claude", p)).child(
                TextView::markdown(("chat-text", id), view::display_markdown(text))
                    .selectable(true),
            ),
            ItemKind::Activity { kind, label } => {
                muted(row, format!("↳ {}", view::activity_text(*kind, label)))
            }
            ItemKind::Notice { text } => muted(row, text.clone()),
            ItemKind::Plan { plan } => row.child(self.plan_card(state, item.id, plan, cx)),
            ItemKind::Permission {
                title,
                detail,
                command,
                allowed,
            } => {
                let pending = allowed.is_none() && state.pending_permission == Some(item.id);
                let card = solid_section(div(), cx)
                    .v_flex()
                    .min_w_0()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .when(pending, |d| d.border_color(p.warning))
                    .child(div().font_semibold().child(title.clone()))
                    .when(!detail.is_empty(), |d| d.child(div().child(detail.clone())))
                    .when_some(command.clone(), |d, command| {
                        d.child(code_note(("chat-permission-command", id), command, cx))
                    })
                    .when(pending, |d| {
                        d.child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    compact_button(Button::new(("chat-allow", id)), "Allow")
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, _| {
                                            this.chat_command(Command::Permission { allow: true })
                                        })),
                                )
                                .child(
                                    compact_button(Button::new(("chat-deny", id)), "Deny")
                                        .on_click(cx.listener(|this, _, _, _| {
                                            this.chat_command(Command::Permission { allow: false })
                                        })),
                                ),
                        )
                    })
                    .when_some(*allowed, |d, allowed| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(p.secondary_foreground)
                                .child(if allowed { "Allowed" } else { "Denied" }),
                        )
                    });
                row.child(card)
            }
            ItemKind::Check { result } => row.child(self.check_card(id, result, cx)),
            ItemKind::Changes { change_set } => match view::changes_card(state, change_set) {
                Some(card) => row.child(self.changes_card(id, card, cx)),
                None => muted(row, "Changed files".into()),
            },
        }
        .into_any_element()
    }

    fn plan_card(
        &self,
        state: &ChatState,
        item_id: i64,
        plan: &Plan,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = view::plan_actions_visible(state, item_id);
        solid_section(div(), cx)
            .v_flex()
            .min_w_0()
            .gap_2()
            .p_3()
            .border_1()
            .child(div().font_semibold().child("Plan"))
            .child(div().min_w_0().child(format!("Goal: {}", plan.goal)))
            .child(div().child("Done when:"))
            .children(
                plan.done_when
                    .iter()
                    .map(|criterion| div().min_w_0().pl_2().child(format!("• {criterion}"))),
            )
            .children(plan.steps.iter().enumerate().map(|(index, step)| {
                div().min_w_0().child(format!(
                    "{}. {}  ({})",
                    index + 1,
                    step.summary,
                    step.target
                ))
            }))
            .when(actions, |d| {
                d.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            compact_button(
                                Button::new(("chat-write-it", item_id as usize)),
                                "I'll write it",
                            )
                            .primary()
                            .on_click(
                                cx.listener(|this, _, _, _| {
                                    this.chat_command(Command::ChooseGuided)
                                }),
                            ),
                        )
                        .child(
                            compact_button(
                                Button::new(("chat-you-do-it", item_id as usize)),
                                "You do it",
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.begin_takeover(TakeoverRequest::Rest, window, cx)
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    /// What one takeover changed: a row per file with Keep and Undo, until each is decided.
    fn changes_card(
        &self,
        item_id: usize,
        card: ChangesCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx.theme().mode);
        let can_act = card.can_act;
        let rows: Vec<AnyElement> = card
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.change_row(item_id, index, &card.id, row, can_act, cx))
            .collect();
        let decide_all = |label: &'static str, keep: bool| {
            let command = decide(keep, &card.id, None);
            compact_button(
                Button::new(SharedString::from(format!(
                    "chat-changes-{item_id}-{}",
                    if keep { "keep-all" } else { "undo-all" }
                ))),
                label,
            )
            .disabled(!can_act)
            .on_click(cx.listener(move |this, _, _, _| this.chat_command(command.clone())))
        };
        let footer = (card.undecided > 0).then(|| {
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(decide_all("Keep all", true))
                .child(decide_all("Undo all", false))
        });
        solid_section(div(), cx)
            .v_flex()
            .min_w_0()
            .gap_2()
            .p_3()
            .border_1()
            .when(card.undecided > 0, |d| d.border_color(p.warning))
            .child(
                div()
                    .font_semibold()
                    .child(format!("Claude changed {} file(s)", card.rows.len())),
            )
            .children(rows)
            .children(footer)
            .child(
                div()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .child(IGNORED_NOT_TRACKED),
            )
            .into_any_element()
    }

    /// One changed file: its kind, its path (a button that opens it while the file exists)
    /// and Keep and Undo, or what the user decided.
    fn change_row(
        &self,
        item_id: usize,
        index: usize,
        change_set: &str,
        row: &ChangeRow,
        can_act: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx.theme().mode);
        let id = |part: &str| SharedString::from(format!("chat-change-{item_id}-{index}-{part}"));
        let (badge, colour, surface) = match row.kind {
            ChangeKind::Created => ("Created", p.success, p.success_surface),
            ChangeKind::Modified => ("Modified", p.warning, p.warning_surface),
            ChangeKind::Deleted => ("Deleted", p.error, p.error_surface),
        };
        // A deleted file, or a created one that Undo removed, has nothing to open.
        let gone = row.kind == ChangeKind::Deleted
            || (row.kind == ChangeKind::Created && row.decision == Decision::Undone);
        let path = if gone {
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .child(row.path.clone())
                .into_any_element()
        } else {
            let open = row.path.clone();
            div()
                .flex_1()
                .min_w_0()
                .child(
                    path_button(Button::new(id("open")), row.path.clone())
                        .ghost()
                        .disabled(!can_act)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.settings.open = false;
                            this.open_file(open.clone(), false, None, window, cx)
                        })),
                )
                .into_any_element()
        };
        let decided = |word: &'static str| {
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(p.secondary_foreground)
                .child(word)
                .into_any_element()
        };
        let decision = match row.decision {
            Decision::Undecided => {
                let keep = decide(true, change_set, Some(row.path.clone()));
                let undo = decide(false, change_set, Some(row.path.clone()));
                div()
                    .h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        compact_button(Button::new(id("keep")), "Keep")
                            .disabled(!can_act)
                            .on_click(
                                cx.listener(move |this, _, _, _| this.chat_command(keep.clone())),
                            ),
                    )
                    .child(
                        compact_button(Button::new(id("undo")), "Undo")
                            .disabled(!can_act || !row.undoable)
                            .when(!row.undoable, |button| {
                                button.tooltip("Too large to restore")
                            })
                            .on_click(
                                cx.listener(move |this, _, _, _| this.chat_command(undo.clone())),
                            ),
                    )
                    .into_any_element()
            }
            Decision::Kept => decided("Kept"),
            Decision::Undone => decided("Undone"),
        };
        div()
            .id(id("row"))
            .h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(colour)
                    .bg(surface)
                    .text_color(colour)
                    .text_size(px(11.))
                    .font_semibold()
                    .child(badge),
            )
            .child(path)
            .child(decision)
            .into_any_element()
    }

    /// Claude's verdict from "Check my work" or the final check, with each finding.
    fn check_card(
        &self,
        item_id: usize,
        result: &CheckResult,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx.theme().mode);
        let (colour, surface) = if result.passed {
            (p.success, p.success_surface)
        } else {
            (p.warning, p.warning_surface)
        };
        let findings: Vec<AnyElement> = result
            .findings
            .iter()
            .enumerate()
            .map(|(index, finding)| self.finding_row(item_id, index, finding, cx))
            .collect();
        solid_section(div(), cx)
            .v_flex()
            .min_w_0()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(colour)
            .bg(surface)
            .child(
                div()
                    .min_w_0()
                    .font_semibold()
                    .text_color(colour)
                    .child(check_title(result)),
            )
            .when(!result.explanation.trim().is_empty(), |d| {
                d.child(
                    TextView::markdown(
                        ("chat-check-text", item_id),
                        view::display_markdown(&result.explanation),
                    )
                    .selectable(true),
                )
            })
            .children(findings)
            .into_any_element()
    }

    /// One finding of a check; a finding with a path opens that file at its line.
    fn finding_row(
        &self,
        item_id: usize,
        index: usize,
        finding: &Finding,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx.theme().mode);
        let label = finding_label(finding);
        let head = match finding.path.clone() {
            Some(path) => {
                let line = finding.line.map(|line| line as usize);
                path_button(
                    Button::new(SharedString::from(format!(
                        "chat-finding-{item_id}-{index}"
                    ))),
                    label,
                )
                .ghost()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.settings.open = false;
                    this.open_file(path.clone(), false, line, window, cx)
                }))
                .into_any_element()
            }
            None => div().min_w_0().child(label).into_any_element(),
        };
        div()
            .v_flex()
            .min_w_0()
            .gap_1()
            .child(head)
            .child(
                div()
                    .min_w_0()
                    .pl_2()
                    .text_size(px(12.))
                    .child(format!("Why: {}", finding.why)),
            )
            .when_some(filled(&finding.hint), |d, hint| {
                d.child(
                    div()
                        .min_w_0()
                        .pl_2()
                        .text_size(px(12.))
                        .text_color(p.secondary_foreground)
                        .child(format!("Hint: {hint}")),
                )
            })
            .into_any_element()
    }

    /// The card pinned above the composer during Build: the open step, or the final check
    /// once every step is finished.
    fn build_card(&self, state: &ChatState, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (card, hover) = match view::step_card(state) {
            Some(card) => {
                let hover = self.code_hover_card(&card, cx);
                (self.step_card(card, cx), hover)
            }
            None if view::final_card(state) => {
                let can_act =
                    matches!(state.connection, Connection::Ready { .. }) && state.turn.is_none();
                (self.final_card(can_act, cx), None)
            }
            None => return None,
        };
        let p = palette(cx.theme().mode);
        Some(
            div()
                .v_flex()
                .flex_shrink_0()
                .min_w_0()
                .min_h_0()
                .max_h(relative(0.45))
                .p_2()
                .bg(p.surface)
                .border_t_1()
                .border_color(p.border)
                .child(card)
                .children(hover)
                .into_any_element(),
        )
    }

    fn step_card(&self, card: StepCard, cx: &mut Context<Self>) -> AnyElement {
        let index = card.index;
        let can_act = card.can_act;
        let weak = cx.entity().downgrade();
        let title = format!(
            "Step {} of {} · {} {}",
            index + 1,
            card.total,
            kind_label(card.kind),
            card.target
        );
        let body = match &card.guidance {
            Some(guidance) => self.guidance_view(&card, guidance, cx),
            None => self.unguided_view(&card, cx),
        };
        solid_section(div().id(("chat-step-card", index)), cx)
            .v_flex()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .gap_2()
            .p_3()
            .border_1()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().font_semibold().child(title))
                    .child(help_level_menu(
                        ("chat-step-help", index),
                        format!("Help: {} ▾", card.level.label()),
                        card.language.clone(),
                        card.level,
                        can_act,
                        &weak,
                    )),
            )
            .child(body)
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .when(card.kind != StepKind::RunCommand, |d| {
                        d.child(
                            compact_button(Button::new(("chat-step-open", index)), "Open file")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_step_file(window, cx)
                                })),
                        )
                    })
                    .child(
                        compact_button(Button::new(("chat-step-check", index)), "Check my work")
                            .primary()
                            .disabled(!can_act)
                            .on_click(
                                cx.listener(|this, _, _, _| {
                                    this.chat_command(Command::CheckMyWork)
                                }),
                            ),
                    )
                    .child(
                        compact_button(Button::new(("chat-step-do-it", index)), "Do it for me")
                            .disabled(!can_act)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_takeover(TakeoverRequest::Step, window, cx)
                            })),
                    )
                    .child(
                        compact_button(
                            Button::new(("chat-step-build-rest", index)),
                            "Build the rest for me",
                        )
                        .disabled(!can_act)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_takeover(TakeoverRequest::Rest, window, cx)
                        })),
                    ),
            )
            .into_any_element()
    }

    /// The step's guidance: summary, why, then code, pseudocode or the command, whose parts
    /// are explained when the user points at them, and the explanations of the parts that
    /// are in none of them.
    fn guidance_view(&self, card: &StepCard, guidance: &Guidance, cx: &mut Context<Self>) -> Div {
        let index = card.index;
        let blocks = view::step_blocks(guidance, &card.language);
        let p = palette(cx.theme().mode);
        let unmatched = (!blocks.unmatched.is_empty()).then(|| {
            let rows = blocks.unmatched.iter().map(|item| {
                div()
                    .h_flex()
                    .items_start()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.4))
                            .px_1()
                            .rounded(px(4.))
                            .bg(p.content)
                            .font_family("Menlo")
                            .text_size(px(12.))
                            .child(item.part.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(item.explanation.clone()))
            });
            div()
                .v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_semibold()
                        .text_color(p.secondary_foreground)
                        .child("More explanations"),
                )
                .children(rows)
        });
        let blocks: Vec<AnyElement> = blocks
            .blocks
            .iter()
            .map(|block| self.step_block(index, block, cx))
            .collect();
        div()
            .v_flex()
            .min_w_0()
            .gap_2()
            .child(
                TextView::markdown(
                    ("chat-step-summary", index),
                    view::display_markdown(&guidance.summary),
                )
                .selectable(true),
            )
            .child(
                TextView::markdown(
                    ("chat-step-why", index),
                    view::display_markdown(&format!("Why: {}", guidance.why)),
                )
                .selectable(true),
            )
            .when_some(view::where_line(guidance), |d, place| {
                d.child(
                    TextView::markdown(
                        ("chat-step-where", index),
                        view::display_markdown(&format!("Where: {place}")),
                    )
                    .selectable(true),
                )
            })
            .children(blocks)
            .children(unmatched)
    }

    /// One code block of the step's guidance with what goes with it: the Pseudocode label,
    /// how to read the explanations of its parts, or the row of buttons under code and
    /// commands.
    fn step_block(&self, index: usize, block: &StepBlock, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let code = self.code_view(index, block, cx);
        let hint = (!block.parts.is_empty()).then(|| {
            div()
                .text_size(px(12.))
                .text_color(p.secondary_foreground)
                .child("Point at a part to see what it does.")
        });
        let column = div().v_flex().min_w_0().gap_2();
        match block.kind {
            BlockKind::Code => column
                .child(code)
                .children(hint)
                .child(div().h_flex().child(copy_button(
                    ("chat-step-copy-code", index),
                    block.copy.clone(),
                ))),
            BlockKind::Pseudocode => column
                .child(
                    div()
                        .text_size(px(12.))
                        .font_semibold()
                        .text_color(p.secondary_foreground)
                        .child("Pseudocode"),
                )
                .child(code)
                .children(hint),
            BlockKind::Command => column.child(code).children(hint).child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(copy_button(
                        ("chat-step-copy-command", index),
                        block.copy.clone(),
                    ))
                    .child(
                        compact_button(Button::new(("chat-step-terminal", index)), "Open Terminal")
                            .on_click(cx.listener(|this, _, _, cx| this.show_terminal(cx))),
                    ),
            ),
        }
        .into_any_element()
    }

    /// The step before its guidance arrives: the plan's summary, and either a note that
    /// Claude is preparing it or a button to ask for it.
    fn unguided_view(&self, card: &StepCard, cx: &mut Context<Self>) -> Div {
        let p = palette(cx.theme().mode);
        div()
            .v_flex()
            .min_w_0()
            .gap_2()
            .child(
                TextView::markdown(
                    ("chat-step-plan-summary", card.index),
                    view::display_markdown(&card.summary),
                )
                .selectable(true),
            )
            .child(if card.preparing {
                div()
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .child("Preparing this step…")
                    .into_any_element()
            } else {
                compact_button(
                    Button::new(("chat-step-guide", card.index)),
                    "Guide me through this step",
                )
                .disabled(!card.can_act)
                .on_click(cx.listener(|this, _, _, _| this.chat_command(Command::GuideStep)))
                .into_any_element()
            })
    }

    /// Every step is finished: Claude checks the "done when" criteria.
    fn final_card(&self, can_act: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        solid_section(div().id("chat-final-card"), cx)
            .v_flex()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .gap_2()
            .p_3()
            .border_1()
            .child(div().font_semibold().child("All steps are finished"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .child("Claude checks the \"done when\" criteria."),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        compact_button(Button::new("chat-check-again"), "Check again")
                            .primary()
                            .disabled(!can_act)
                            .on_click(
                                cx.listener(|this, _, _, _| {
                                    this.chat_command(Command::CheckMyWork)
                                }),
                            ),
                    )
                    .child(
                        compact_button(Button::new("chat-fix-it"), "Fix it for me")
                            .disabled(!can_act)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_takeover(TakeoverRequest::Rest, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn chat_composer(&self, state: &ChatState, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let draft_is_empty = self.chat.composer.read(cx).value().trim().is_empty();
        let mode = view::composer_mode(state, draft_is_empty);
        let weak = cx.entity().downgrade();
        let efforts = view::effort_choices(state);
        div()
            .v_flex()
            .flex_shrink_0()
            .min_w_0()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(p.border)
            .bg(p.chrome())
            .when_some(state.banner.clone(), |d, banner| {
                d.child(panel_error(div().text_size(px(13.)).child(banner), cx))
            })
            .child(Textarea::new(&self.chat.composer).aria_label("Message Claude"))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(model_menu(state, &weak))
                    .when(!efforts.is_empty(), |d| {
                        d.child(effort_menu(state, efforts, &weak))
                    })
                    .child(div().flex_1())
                    .child(self.composer_action(mode, cx)),
            )
            .into_any_element()
    }

    fn composer_action(&self, mode: ComposerMode, cx: &mut Context<Self>) -> AnyElement {
        match mode {
            ComposerMode::Send => compact_button(Button::new("chat-send"), "Send")
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.chat_send(window, cx))),
            ComposerMode::Stop => compact_button(Button::new("chat-stop"), "Stop")
                .danger()
                .on_click(cx.listener(|this, _, _, _| this.chat_command(Command::Stop))),
            ComposerMode::Stopping => {
                compact_button(Button::new("chat-stopping"), "Stopping…").disabled(true)
            }
            ComposerMode::Disabled(reason) => compact_button(Button::new("chat-send"), "Send")
                .primary()
                .disabled(true)
                .tooltip(reason),
        }
        .into_any_element()
    }
}

/// The model picker: Default plus every model Claude Code offers.
fn model_menu(state: &ChatState, weak: &WeakEntity<Workspace>) -> AnyElement {
    let selected = state.prefs.model.clone();
    // "Default" already stands for the listed default model.
    let choices: Vec<(String, String)> =
        std::iter::once((DEFAULT_MODEL.to_owned(), "Default".to_owned()))
            .chain(
                state
                    .models
                    .iter()
                    .filter(|model| model.value != DEFAULT_MODEL)
                    .map(|model| (model.value.clone(), model.name.clone())),
            )
            .collect();
    let weak = weak.clone();
    compact_button(Button::new("chat-model"), view::model_label(state))
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            choices.iter().fold(menu, |menu, (value, name)| {
                menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(*value == selected)
                        .on_click(on_command(&weak, Command::SetModel(value.clone()))),
                )
            })
        })
        .into_any_element()
}

/// The effort picker for the selected model: Default plus each effort level it offers.
fn effort_menu(
    state: &ChatState,
    efforts: Vec<String>,
    weak: &WeakEntity<Workspace>,
) -> AnyElement {
    let selected = state.prefs.effort.clone();
    let label = format!("Effort: {}", selected.as_deref().unwrap_or("Default"));
    let weak = weak.clone();
    compact_button(Button::new("chat-effort"), label)
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            let menu = menu.item(
                PopupMenuItem::new("Default")
                    .checked(selected.is_none())
                    .on_click(on_command(&weak, Command::SetEffort(None))),
            );
            efforts.iter().fold(menu, |menu, effort| {
                menu.item(
                    PopupMenuItem::new(effort.clone())
                        .checked(selected.as_deref() == Some(effort.as_str()))
                        .on_click(on_command(&weak, Command::SetEffort(Some(effort.clone())))),
                )
            })
        })
        .into_any_element()
}
