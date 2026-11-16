//! The window's chrome around the panes: the title bar with its command center, the
//! activity bar on the window's edge beside the file tree, the breadcrumbs above the
//! editor and the status bar.
use super::breadcrumbs::{path_segments, symbol_path};
use super::chat_view::permission_menu_with;
use super::chrome_logic::{
    Activity, FileIcon, Shown, activity_selected, cursor_label, file_icon, language_label,
    problems_label, tooltip,
};
use super::commands::shortcut;
use super::pane_logic::ACTIVITY_BAR;
use super::*;
use crate::appearance::{palette, status_item};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_core::layout::{Pane, PaneSide};

impl Workspace {
    /// The title bar: the traffic lights' space, the command center in the middle and, on
    /// the right, the takeover notice and the switches for the explorer, the panel below
    /// the editor and the chat.
    pub(super) fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx.theme().mode);
        let name = self
            .project
            .as_ref()
            .and_then(|project| project.root.file_name())
            .map_or_else(
                || "Tandem".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        let hint = format!(
            "{}  ·  {}",
            tooltip("Search Files", shortcut("Quick Open")),
            tooltip("Commands", shortcut("Command Palette…"))
        );
        let center = div()
            .id("command-center")
            .role(Role::Button)
            .aria_label(format!(
                "Search files in {name}. Type a greater-than sign for commands."
            ))
            .h_flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h(px(24.))
            .w(px(420.))
            .min_w_0()
            .px_2()
            .rounded(px(8.))
            .border_1()
            .border_color(p.border)
            .bg(p.surface.alpha(0.6))
            .text_size(px(12.))
            .text_color(p.secondary_foreground)
            .cursor_pointer()
            .hover(|style| style.bg(p.surface))
            // A press here must not start a window drag.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.open_center("", window, cx)))
            .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
            .child(Icon::new(IconName::Search).xsmall())
            .child(div().min_w_0().truncate().child(name));
        let (files_icon, chat_icon) = match self.panes.layout.side(Pane::Files) {
            PaneSide::Left => (IconName::PanelLeft, IconName::PanelRight),
            PaneSide::Right => (IconName::PanelRight, IconName::PanelLeft),
        };
        let switches = div()
            .h_flex()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("title-toggle-explorer")
                    .icon(files_icon)
                    .ghost()
                    .xsmall()
                    .selected(self.commands.explorer_visible)
                    .tooltip(tooltip(
                        "Toggle File Explorer",
                        shortcut("Toggle File Explorer"),
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_explorer(window, cx))),
            )
            .child(
                Button::new("title-toggle-panel")
                    .icon(IconName::PanelBottom)
                    .ghost()
                    .xsmall()
                    .selected(self.bottom.is_some())
                    .tooltip(tooltip("Toggle Panel", shortcut("Toggle Panel")))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bottom_panel(cx))),
            )
            .child(
                Button::new("title-toggle-chat")
                    .icon(chat_icon)
                    .ghost()
                    .xsmall()
                    .selected(self.chat.visible)
                    .tooltip(tooltip("Toggle Chat", shortcut("Toggle Chat")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.chat.visible = !this.chat.visible;
                        cx.notify();
                    })),
            );
        TitleBar::new()
            .bg(Hsla::transparent_black())
            .border_color(Hsla::transparent_black())
            .child(div().flex_1().min_w_0())
            .child(center)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pr_2()
                    .when(self.takeover_running(), |d| {
                        d.child(
                            div()
                                .id("mode")
                                .role(Role::Label)
                                .aria_label(
                                    "Claude is editing files. The editor is read-only; Command-period stops Claude.",
                                )
                                .text_size(px(12.))
                                .text_color(p.warning)
                                .child("Claude is editing files · ⌘. stops"),
                        )
                    })
                    .child(switches),
            )
    }

    /// What the window shows, as far as the activity bar cares.
    fn shown(&self) -> Shown {
        Shown {
            explorer: self.commands.explorer_visible,
            searching: self.sidebar == Sidebar::Search,
            chat: self.chat.visible,
            bottom: self.bottom,
            settings: self.settings.open,
        }
    }

    /// Runs the activity bar's button for `activity`. Explorer and Search hide the pane when
    /// it already shows their view; Chat, Problems, Terminal and Settings toggle.
    fn activity_click(&mut self, activity: Activity, window: &mut Window, cx: &mut Context<Self>) {
        match activity {
            Activity::Explorer => {
                if activity_selected(Activity::Explorer, self.shown()) {
                    self.toggle_explorer(window, cx);
                } else {
                    self.set_sidebar(Sidebar::Files, window, cx);
                }
            }
            Activity::Search => {
                if activity_selected(Activity::Search, self.shown()) {
                    self.toggle_explorer(window, cx);
                } else {
                    self.set_sidebar(Sidebar::Search, window, cx);
                }
            }
            Activity::Chat => {
                self.chat.visible = !self.chat.visible;
                cx.notify();
            }
            Activity::Problems => {
                self.bottom = next_bottom(self.bottom, BottomTab::Problems);
                cx.notify();
            }
            Activity::Terminal => self.toggle_terminal(cx),
            Activity::Settings => self.toggle_settings(cx),
        }
    }

    /// The activity bar: Explorer, Search, Chat, Problems and Terminal at the top and
    /// Settings at the bottom, each drawn as active while what it opens is on screen.
    pub(super) fn activity_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let shown = self.shown();
        let mut button = |activity: Activity| {
            let (id, icon) = match activity {
                Activity::Explorer => ("activity-explorer", Icon::new(Lucide::Files)),
                Activity::Search => ("activity-search", Icon::new(IconName::Search)),
                Activity::Chat => ("activity-chat", Icon::new(Lucide::MessageSquare)),
                Activity::Problems => ("activity-problems", Icon::new(IconName::TriangleAlert)),
                Activity::Terminal => ("activity-terminal", Icon::new(IconName::SquareTerminal)),
                Activity::Settings => ("activity-settings", Icon::new(IconName::Settings)),
            };
            Button::new(id)
                .icon(icon.size_5())
                .ghost()
                .selected(activity_selected(activity, shown))
                .accessibility_label(activity.label())
                .tooltip(tooltip(activity.label(), shortcut(activity.command())))
                .on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.activity_click(activity, window, cx)
                    }),
                )
        };
        div()
            .id("activity-bar")
            .v_flex()
            .items_center()
            .w(px(ACTIVITY_BAR))
            .flex_shrink_0()
            .min_h_0()
            .py_1()
            .gap_1()
            .children(Activity::TOP.map(&mut button))
            .child(div().flex_1())
            .child(button(Activity::Settings))
            .into_any_element()
    }

    /// The breadcrumbs above the editor: the open file's folders and name and, once the
    /// language tools have answered for this file, the symbols the cursor is in. They are
    /// not clickable.
    pub(super) fn breadcrumbs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.active.and_then(|i| self.tabs.get(i))?;
        let p = palette(cx.theme().mode);
        let path = &tab.document.path;
        let segments = path_segments(path);
        let symbols = if self.dev.outline_instance == tab.instance {
            symbol_path(
                &self.dev.outline,
                tab.editor.read(cx).cursor_position().line,
            )
        } else {
            Vec::new()
        };
        let label = segments
            .iter()
            .chain(&symbols)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" › ");
        let file = segments.len().saturating_sub(1);
        let parts = segments
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                if index == file {
                    div()
                        .flex_none()
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .text_color(p.foreground)
                        .child(Icon::new(file_icon_image(file_icon(path))).xsmall())
                        .child(name)
                } else {
                    div().flex_none().child(name)
                }
            })
            .chain(
                symbols
                    .into_iter()
                    .map(|name| div().flex_none().child(name)),
            );
        let mut row = Vec::new();
        for (index, part) in parts.enumerate() {
            if index > 0 {
                row.push(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .into_any_element(),
                );
            }
            row.push(part.into_any_element());
        }
        Some(
            div()
                .id("breadcrumbs")
                .role(Role::Label)
                .aria_label(label)
                .h_flex()
                .items_center()
                .gap_1()
                .h(px(24.))
                .px_3()
                .flex_shrink_0()
                .min_w_0()
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(p.secondary_foreground)
                .children(row)
                .into_any_element(),
        )
    }

    /// The status bar: the git branch, the open file's errors and warnings, the status
    /// message, the cursor, the line count, the language, the permission mode and whether
    /// the drafts are backed up.
    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let weak = cx.entity().downgrade();
        let backup = if self.project.is_none() {
            "Ready"
        } else if self.checkpointed >= self.generation {
            "Drafts backed up"
        } else {
            "Backing up drafts…"
        };
        let tab = self.active.and_then(|i| self.tabs.get(i));
        // GitHub-style line count of the open file, or None when no file is open.
        let lines = tab.map(|tab| line_count_label(&tab.document.text));
        let branch = self.branch.as_ref().map(|name| {
            div()
                .id("status-branch")
                .role(Role::Label)
                .aria_label(format!("Git branch {name}"))
                .h_flex()
                .items_center()
                .gap_1()
                .flex_none()
                .child(Icon::new(Lucide::GitBranch).xsmall())
                .child(name.clone())
        });
        let counts = self.dev.problem_counts;
        let problems = self.project.is_some().then(|| {
            Button::new("status-problems")
                .ghost()
                .xsmall()
                .h(px(20.))
                .px_1()
                .rounded(px(4.))
                .accessibility_label(problems_label(counts))
                .tooltip(tooltip("Problems", shortcut("Show Problems")))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(12.))
                        .child(Icon::new(IconName::CircleX).xsmall())
                        .child(counts.errors.to_string())
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(counts.warnings.to_string()),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.bottom = Some(BottomTab::Problems);
                    cx.notify();
                }))
        });
        let status = SharedString::from(self.status.clone());
        let message = div()
            .id("status")
            .role(Role::Label)
            .aria_label(match &lines {
                Some(count) => format!("{}. {count}. {backup}", self.status.trim_end_matches('.')),
                None => format!("{}. {backup}", self.status.trim_end_matches('.')),
            })
            .flex_1()
            .min_w_0()
            .truncate()
            .when(self.error, |d| d.text_color(p.error))
            .when(!status.is_empty(), |d| {
                let text = status.clone();
                d.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            })
            .child(status);
        let editor_shown = self.diff.is_none() && !self.settings.open;
        let cursor = tab.filter(|_| editor_shown).map(|tab| {
            let position = tab.editor.read(cx).cursor_position();
            status_item(
                Button::new("status-cursor"),
                cursor_label(position.line, position.character),
            )
            .tooltip(tooltip("Go to Line", shortcut("Go to Line…")))
            .on_click(cx.listener(|this, _, window, cx| this.open_go_to_line(window, cx)))
        });
        let language = tab.map(|tab| {
            div()
                .flex_none()
                .child(language_label(tab.document.language()))
        });
        let permissions = self.chat_state().map(|state| {
            permission_menu_with(
                |label| {
                    status_item(
                        Button::new("status-permissions").icon(Lucide::Shield),
                        label,
                    )
                },
                &state,
                &weak,
            )
        });
        div()
            .id("status-bar")
            .h_flex()
            .items_center()
            .h(px(24.))
            .flex_shrink_0()
            .min_w_0()
            .px_2()
            .gap_2()
            .border_t_1()
            .border_color(p.border)
            .bg(p.content)
            .text_size(px(12.))
            .text_color(p.secondary_foreground)
            .children(branch)
            .children(problems)
            .child(message)
            .children(cursor)
            .children(lines.map(|lines| div().flex_none().child(lines)))
            .children(language)
            .children(permissions)
            .child(div().flex_none().child(backup))
            .into_any_element()
    }
}

/// The Lucide icon drawn for a file of kind `icon` in the file tree.
pub(super) fn file_icon_image(icon: FileIcon) -> Lucide {
    match icon {
        FileIcon::Code => Lucide::FileCode,
        FileIcon::Data => Lucide::FileBraces,
        FileIcon::Config => Lucide::FileCog,
        FileIcon::Text => Lucide::FileText,
        FileIcon::Script => Lucide::FileTerminal,
        FileIcon::Lock => Lucide::FileLock,
        FileIcon::Image => Lucide::FileImage,
        FileIcon::Plain => Lucide::File,
    }
}
