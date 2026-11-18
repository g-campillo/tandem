//! The chat panel beside the editor: header, connection notice, transcript, the card of the
//! step being built, and composer.
use super::chat_logic::{check_title, finding_label, phase_name, task_menu_name};
use super::chrome_logic::tooltip;
use super::commands::shortcut;
use super::panels::code_note;
use super::*;
use crate::appearance::{
    Palette, compact_button, palette, panel_error, path_button, solid_section, wrapping_button,
};
use crate::scrolling::VerticalOnly as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_claude::prompts::kind_label;
use tandem_claude::view::{
    self, BlockKind, ChangeRow, ChangesCard, ComposerMode, ConnectionNotice, StepBlock, StepCard,
};
use tandem_claude::{ChatState, Connection, TakeoverRequest};
use tandem_core::chat::{
    ChangeKind, ChatItem, CheckResult, DEFAULT_MODEL, Decision, Finding, Guidance, HelpLevel,
    ItemKind, Plan, StepKind,
};

const NO_PROJECT: &str = "Open a folder to start a task with Claude.";
const IGNORED_NOT_TRACKED: &str = "Files in ignored folders (like target/ or node_modules/) aren't tracked, so Undo can't restore them.";

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

/// An icon button that puts `text` on the clipboard.
pub(super) fn copy_button(id: impl Into<ElementId>, text: String) -> Button {
    Button::new(id)
        .icon(IconName::Copy)
        .ghost()
        .small()
        .accessibility_label("Copy")
        .tooltip("Copy")
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

/// "You" or "Claude" above a message, in the chat's small print.
fn speaker(name: &'static str, p: Palette, small: Pixels) -> Div {
    div()
        .text_size(small)
        .font_semibold()
        .text_color(p.secondary_foreground)
        .child(name)
}

impl Workspace {
    /// Markdown in the chat, in the chat's text and code fonts.
    fn chat_markdown(
        &self,
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> TextView {
        let (chat, code) = (&self.look.fonts.chat, &self.look.fonts.chat_code);
        let code_font = || {
            StyleRefinement::default()
                .font_family(code.family.clone())
                .font_features(code.features.clone())
        };
        let mut style = TextViewStyle::default().code_block(code_font().text_size(px(code.size)));
        // The editor's highlight theme, so code has the editor's colours.
        style.highlight_theme = cx.theme().highlight_theme.clone();
        TextView::markdown(id, text)
            .style(style)
            .inline_code_style(code_font())
            .font_family(chat.family.clone())
            .text_size(px(chat.size))
            .selectable(true)
    }

    /// The chat's text size, its small print (speaker names, notes, hints: one pixel
    /// less) and its badges (two less); none below 10.
    fn chat_sizes(&self) -> (Pixels, Pixels, Pixels) {
        let size = self.look.fonts.chat.size;
        (px(size), px((size - 1.).max(10.)), px((size - 2.).max(10.)))
    }

    /// The chat panel; `card_max` is the tallest the card above the composer may be, in
    /// pixels.
    pub(super) fn chat_panel(&self, card_max: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let state = self.chat_state();
        let panel = div()
            .v_flex()
            .font_family(self.look.fonts.chat.family.clone())
            .text_size(px(self.look.fonts.chat.size))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.chat_header(state.as_deref(), cx));
        match state {
            Some(state) => panel
                .child(self.chat_body(&state, cx))
                .children(self.build_card(&state, card_max, cx))
                .child(self.chat_composer(&state, cx)),
            None => panel.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .text_color(p.secondary_foreground)
                    .child(NO_PROJECT),
            ),
        }
        .into_any_element()
    }

    fn chat_header(&self, state: Option<&ChatState>, cx: &mut Context<Self>) -> AnyElement {
        let task_menu = state.map(|state| self.task_menu(state, cx));
        div()
            // A control, so not in the chat's text font that the panel sets.
            .font_family(cx.theme().font_family.clone())
            .h_flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .p_2()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(14.))
                    .font_semibold()
                    .child(Icon::new(Lucide::MessageSquare).small())
                    .child("Chat · Claude"),
            )
            .children(task_menu)
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("chat-settings")
                    .icon(IconName::Settings2)
                    .ghost()
                    .small()
                    .accessibility_label("Settings")
                    .tooltip(tooltip("Settings", shortcut("Settings…")))
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
        let name = task_menu_name(
            state
                .task
                .as_ref()
                .map(|task| (task.title.as_str(), task.phase)),
        );
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
        // The default variant's border and fill and the caret make it read as a button
        // that opens a menu, not as text.
        wrapping_button(Button::new("chat-task"), title)
            .accessibility_label(name)
            .dropdown_caret(true)
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
        let (_, small, _) = self.chat_sizes();
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
            .children(notice)
            .child(
                div()
                    .id("chat-transcript")
                    .track_scroll(&self.chat.scroll)
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .vertical_only()
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
                                .child(speaker("Claude", p, small))
                                .child(self.chat_markdown(
                                    "chat-streaming-text",
                                    view::display_markdown(&text),
                                    cx,
                                )),
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
        let (size, _, _) = self.chat_sizes();
        let code = &self.look.fonts.chat_code;
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
                d.child(div().text_size(size).child(notice.body))
            })
            .when_some(command, |d, command| {
                let copied = command.clone();
                d.child(
                    code_note("chat-sign-in", command, cx)
                        .font_family(code.family.clone())
                        .font_features(code.features.clone())
                        .text_size(px(code.size)),
                )
                .child(copy_button("chat-copy-sign-in", copied))
            })
            .when(can_retry, |d| {
                d.child(
                    compact_button(Button::new("chat-reconnect"), "Reconnect")
                        .icon(IconName::RefreshCw)
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
        let (_, small, _) = self.chat_sizes();
        let code = &self.look.fonts.chat_code;
        let id = item.id as usize;
        let row = div().id(("chat-item", id)).v_flex().min_w_0().gap_1();
        let muted = |row: Stateful<Div>, text: String| {
            row.text_size(small)
                .text_color(p.secondary_foreground)
                .child(text)
        };
        match &item.kind {
            ItemKind::User { text } => row
                .child(speaker("You", p, small))
                .child(div().min_w_0().child(text.clone())),
            ItemKind::Assistant { text } => row
                .child(speaker("Claude", p, small))
                .child(self.chat_markdown(("chat-text", id), view::display_markdown(text), cx)),
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
                        d.child(
                            code_note(("chat-permission-command", id), command, cx)
                                .font_family(code.family.clone())
                                .font_features(code.features.clone())
                                .text_size(px(code.size)),
                        )
                    })
                    .when(pending, |d| {
                        d.child(
                            div()
                                .h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    compact_button(Button::new(("chat-allow", id)), "Allow")
                                        .icon(IconName::Check)
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, _| {
                                            this.chat_command(Command::Permission { allow: true })
                                        })),
                                )
                                .child(
                                    compact_button(Button::new(("chat-deny", id)), "Deny")
                                        .icon(IconName::Ban)
                                        .on_click(cx.listener(|this, _, _, _| {
                                            this.chat_command(Command::Permission { allow: false })
                                        })),
                                ),
                        )
                    })
                    .when_some(*allowed, |d, allowed| {
                        d.child(
                            div()
                                .text_size(small)
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
                            .icon(Lucide::Pencil)
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
                            .icon(IconName::Bot)
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
        let (_, small, _) = self.chat_sizes();
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
            .icon(if keep {
                IconName::Check
            } else {
                IconName::Undo2
            })
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
                    .text_size(small)
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
        let (_, small, badge_size) = self.chat_sizes();
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
                            this.open_file(open.clone(), false, None, window, cx)
                        })),
                )
                .into_any_element()
        };
        let decided = |word: &'static str| {
            div()
                .flex_none()
                .text_size(small)
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
                            .icon(IconName::Check)
                            .disabled(!can_act)
                            .on_click(
                                cx.listener(move |this, _, _, _| this.chat_command(keep.clone())),
                            ),
                    )
                    .child(
                        compact_button(Button::new(id("undo")), "Undo")
                            .icon(IconName::Undo2)
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
                    .text_size(badge_size)
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
                d.child(self.chat_markdown(
                    ("chat-check-text", item_id),
                    view::display_markdown(&result.explanation),
                    cx,
                ))
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
        let (_, small, _) = self.chat_sizes();
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
                    .text_size(small)
                    .child(format!("Why: {}", finding.why)),
            )
            .when_some(filled(&finding.hint), |d, hint| {
                d.child(
                    div()
                        .min_w_0()
                        .pl_2()
                        .text_size(small)
                        .text_color(p.secondary_foreground)
                        .child(format!("Hint: {hint}")),
                )
            })
            .into_any_element()
    }

    /// The card pinned above the composer during Build: the open step, or the final check
    /// once every step is finished. It is at most `max_height` pixels tall.
    fn build_card(
        &self,
        state: &ChatState,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
                .max_h(px(max_height))
                .p_2()
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
        // The title and the buttons stay put; the guidance between them scrolls.
        solid_section(div().id(("chat-step-card", index)), cx)
            .v_flex()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_3()
            .border_1()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .flex_shrink_0()
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
            .child(
                div()
                    .id(("chat-step-body", index))
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .vertical_only()
                    .child(body.flex_shrink_0()),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap_2()
                    .when(card.kind != StepKind::RunCommand, |d| {
                        d.child(
                            compact_button(Button::new(("chat-step-open", index)), "Open file")
                                .icon(Lucide::FileCode)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_step_file(window, cx)
                                })),
                        )
                    })
                    .child(
                        compact_button(Button::new(("chat-step-check", index)), "Check my work")
                            .icon(IconName::Check)
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
                            .icon(Lucide::Play)
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
                        .icon(Lucide::ListChecks)
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
        let (_, small, _) = self.chat_sizes();
        let code = &self.look.fonts.chat_code;
        let unmatched = (!blocks.unmatched.is_empty()).then(|| {
            // The part's name on its own line, its explanation under it.
            let rows = blocks.unmatched.iter().map(|item| {
                div()
                    .v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div().h_flex().min_w_0().child(
                            div()
                                .max_w_full()
                                .truncate()
                                .px_1()
                                .rounded(px(4.))
                                .bg(p.content)
                                .font_family(code.family.clone())
                                .font_features(code.features.clone())
                                .text_size(px(code.size))
                                .child(item.part.clone()),
                        ),
                    )
                    .child(div().min_w_0().child(item.explanation.clone()))
            });
            div()
                .v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_size(small)
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
            .child(self.chat_markdown(
                ("chat-step-summary", index),
                view::display_markdown(&guidance.summary),
                cx,
            ))
            .child(self.chat_markdown(
                ("chat-step-why", index),
                view::display_markdown(&format!("Why: {}", guidance.why)),
                cx,
            ))
            .when_some(view::where_line(guidance), |d, place| {
                d.child(self.chat_markdown(
                    ("chat-step-where", index),
                    view::display_markdown(&format!("Where: {place}")),
                    cx,
                ))
            })
            .children(blocks)
            .children(unmatched)
    }

    /// One code block of the step's guidance with what goes with it: the Pseudocode label,
    /// how to read the explanations of its parts, or the row of buttons under code and
    /// commands.
    fn step_block(&self, index: usize, block: &StepBlock, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let (_, small, _) = self.chat_sizes();
        let code = self.code_view(index, block, cx);
        let hint = (!block.parts.is_empty()).then(|| {
            div()
                .text_size(small)
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
                        .text_size(small)
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
                            .icon(IconName::SquareTerminal)
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
        let (_, small, _) = self.chat_sizes();
        div()
            .v_flex()
            .min_w_0()
            .gap_2()
            .child(self.chat_markdown(
                ("chat-step-plan-summary", card.index),
                view::display_markdown(&card.summary),
                cx,
            ))
            .child(if card.preparing {
                div()
                    .text_size(small)
                    .text_color(p.secondary_foreground)
                    .child("Preparing this step…")
                    .into_any_element()
            } else {
                compact_button(
                    Button::new(("chat-step-guide", card.index)),
                    "Guide me through this step",
                )
                .icon(Lucide::Sparkles)
                .disabled(!card.can_act)
                .on_click(cx.listener(|this, _, _, _| this.chat_command(Command::GuideStep)))
                .into_any_element()
            })
    }

    /// Every step is finished: Claude checks the "done when" criteria.
    fn final_card(&self, can_act: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let (_, small, _) = self.chat_sizes();
        solid_section(div().id("chat-final-card"), cx)
            .v_flex()
            .min_w_0()
            .min_h_0()
            .vertical_only()
            .gap_2()
            .p_3()
            .border_1()
            .child(
                div()
                    .flex_shrink_0()
                    .font_semibold()
                    .child("All steps are finished"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(small)
                    .text_color(p.secondary_foreground)
                    .child("Claude checks the \"done when\" criteria."),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap_2()
                    .child(
                        compact_button(Button::new("chat-check-again"), "Check again")
                            .icon(IconName::Check)
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
                            .icon(Lucide::Wrench)
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
                    .child(permission_menu_with(
                        |label| {
                            compact_button(
                                Button::new("chat-permissions").icon(Lucide::Shield),
                                label,
                            )
                        },
                        state,
                        &weak,
                    ))
                    .child(div().flex_1())
                    .child(self.composer_action(mode, cx)),
            )
            .into_any_element()
    }

    fn composer_action(&self, mode: ComposerMode, cx: &mut Context<Self>) -> AnyElement {
        match mode {
            ComposerMode::Send => Button::new("chat-send")
                .icon(Lucide::ArrowUp)
                .primary()
                .small()
                .accessibility_label("Send")
                .tooltip(tooltip("Send", "↵"))
                .on_click(cx.listener(|this, _, window, cx| this.chat_send(window, cx))),
            ComposerMode::Stop => Button::new("chat-stop")
                .icon(IconName::Square)
                .danger()
                .small()
                .accessibility_label("Stop")
                .tooltip(tooltip("Stop Claude", shortcut("Stop Claude")))
                .on_click(cx.listener(|this, _, _, _| this.chat_command(Command::Stop))),
            ComposerMode::Stopping => Button::new("chat-stopping")
                .icon(IconName::LoaderCircle)
                .small()
                .disabled(true)
                .accessibility_label("Stopping…")
                .tooltip("Stopping…"),
            ComposerMode::Disabled(reason) => Button::new("chat-send")
                .icon(Lucide::ArrowUp)
                .primary()
                .small()
                .disabled(true)
                .accessibility_label("Send")
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
        .icon(Lucide::Cpu)
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
        .icon(Lucide::Gauge)
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

/// The permissions picker: Ask, Auto or Bypass. Bypass is shown in the warning colour.
pub(super) fn permission_menu(
    id: impl Into<ElementId>,
    state: &ChatState,
    weak: &WeakEntity<Workspace>,
) -> AnyElement {
    permission_menu_with(|label| compact_button(Button::new(id), label), state, weak)
}

/// The permissions picker on a button the caller styles: `button` gets the picker's
/// label and returns the button; the tooltip, the warning colour for Bypass and the
/// menu are added here.
pub(super) fn permission_menu_with(
    button: impl FnOnce(String) -> Button,
    state: &ChatState,
    weak: &WeakEntity<Workspace>,
) -> AnyElement {
    let picker = view::permission_picker(state);
    let button = button(picker.label.clone()).tooltip(picker.tooltip);
    let button = if picker.warn {
        button.warning()
    } else {
        button.ghost()
    };
    let weak = weak.clone();
    button
        .dropdown_menu(move |menu, _, _| {
            picker.choices.iter().fold(menu, |menu, choice| {
                menu.item(
                    PopupMenuItem::new(choice.label)
                        .checked(choice.checked)
                        .disabled(!choice.enabled)
                        .on_click(on_command(&weak, Command::SetPermissionMode(choice.mode))),
                )
            })
        })
        .into_any_element()
}
