//! The chat panel on the right: header, connection notice, transcript and composer.
use super::panels::code_note;
use super::*;
use crate::appearance::{
    Palette, compact_button, palette, panel_error, solid_section, wrapping_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_claude::ChatState;
use tandem_claude::view::{self, ComposerMode, ConnectionNotice, TrailState};
use tandem_core::chat::{ChatItem, DEFAULT_MODEL, ItemKind, Phase, Plan};

const NO_PROJECT: &str = "Open a folder to start a task with Claude.";

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
        let working = state.turn.is_some() && state.streaming.is_none();
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
                    .when(working, |d| {
                        d.child(
                            div()
                                .text_color(p.secondary_foreground)
                                .child("Claude is working…"),
                        )
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
            ItemKind::Check { .. } => muted(row, "Check result".into()),
            ItemKind::Changes { .. } => muted(row, "Changed files".into()),
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
                    div().h_flex().flex_wrap().gap_2().child(
                        compact_button(
                            Button::new(("chat-write-it", item_id as usize)),
                            "I'll write it",
                        )
                        .primary()
                        .on_click(
                            cx.listener(|this, _, _, _| this.chat_command(Command::ChooseGuided)),
                        ),
                    ),
                )
            })
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
