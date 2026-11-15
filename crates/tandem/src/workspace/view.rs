use super::*;
use crate::appearance::{
    WorkspaceGeometry, compact_button, navigation_button, palette, path_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Input};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        div()
            .h_flex()
            .flex_wrap()
            .min_w_0()
            .flex_shrink_0()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(p.border)
            .bg(p.chrome())
            .child(
                div()
                    .text_size(px(16.))
                    .font_semibold()
                    .px_2()
                    .child("Tandem"),
            )
            .child(
                compact_button(Button::new("open-project"), "Open Folder")
                    .icon(IconName::FolderOpen)
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx))),
            )
            .child(
                navigation_button(
                    compact_button(Button::new("recent-projects"), "Recent")
                        .icon(IconName::RotateCw),
                    self.show_recent,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_recent = !this.show_recent;
                    cx.notify();
                })),
            )
            .child(
                compact_button(Button::new("new-file"), "New File")
                    .icon(IconName::Plus)
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_sidebar(Sidebar::New, window, cx)
                    })),
            )
            .child(
                compact_button(Button::new("save"), "Save  ⌘S")
                    .icon(IconName::HardDrive)
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.save_active(cx))),
            )
            .child(
                navigation_button(
                    compact_button(
                        Button::new("changes"),
                        if self.diff.is_some() {
                            "Back to Code"
                        } else {
                            "Changes"
                        },
                    )
                    .icon(IconName::Replace),
                    self.diff.is_some(),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.show_diff(window, cx))),
            )
            .child(
                navigation_button(
                    compact_button(
                        Button::new("terminal"),
                        if self.show_terminal {
                            "Hide Terminal"
                        } else {
                            "Terminal"
                        },
                    )
                    .icon(IconName::SquareTerminal),
                    self.show_terminal,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_terminal(cx))),
            )
            .when(self.terminal.is_some(), |d| {
                d.child(
                    compact_button(Button::new("close-terminal"), "Stop Terminal")
                        .icon(IconName::Square)
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.terminal = None;
                            this.show_terminal = false;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                navigation_button(
                    compact_button(
                        Button::new("development-tools"),
                        if self.dev.visible {
                            "Hide Companion"
                        } else {
                            "Companion & Tools"
                        },
                    )
                    .icon(IconName::PanelRight),
                    self.dev.visible,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dev.visible = !this.dev.visible;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id("mode")
                    .role(Role::Label)
                    .aria_label(if self.agent.busy {
                        "Agent takeover uses an isolated copy. Live buffers remain yours."
                    } else {
                        "Manual editing. AI source changes are off."
                    })
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .px_2()
                    .child(if self.agent.busy {
                        "Agent · isolated copy"
                    } else {
                        "AI edits off"
                    }),
            )
            .into_any_element()
    }

    fn sidebar_view(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let rows = self.navigation(cx);
        let files = rows
            .into_iter()
            .map(|(path, label, depth, directory)| {
                let expanded = self.expanded.contains(&path);
                let selected = self
                    .active
                    .and_then(|i| self.tabs.get(i))
                    .is_some_and(|t| t.document.path == path);
                div()
                    .min_w_0()
                    .pl(px((depth * 12) as f32))
                    .child(
                        navigation_button(
                            path_button(
                                Button::new(SharedString::from(format!("nav:{path}"))),
                                format!(
                                    "{}{}",
                                    if directory {
                                        if expanded { "▾ " } else { "▸ " }
                                    } else {
                                        ""
                                    },
                                    label
                                ),
                            )
                            .tooltip(path.clone()),
                            selected,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if directory {
                                    if !this.expanded.remove(&path) {
                                        this.expanded.insert(path.clone());
                                    }
                                    cx.notify();
                                } else {
                                    this.open_file(path.clone(), false, None, window, cx);
                                }
                            },
                        )),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let hits = self
            .search_hits
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                let path = hit.path.clone();
                let line = hit.line;
                navigation_button(
                    path_button(
                        Button::new(("hit", i)),
                        format!("{}:{}  {}", hit.path, hit.line, hit.preview),
                    ),
                    false,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_file(path.clone(), false, Some(line), window, cx)
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .v_flex()
            .w(px(width))
            .min_w_0()
            .min_h_0()
            .flex_shrink_0()
            .bg(p.chrome())
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_1()
                    .p_2()
                    .child(
                        navigation_button(
                            compact_button(Button::new("files"), "Files"),
                            self.sidebar == Sidebar::Files,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_sidebar(Sidebar::Files, window, cx)
                        })),
                    )
                    .child(
                        navigation_button(
                            compact_button(Button::new("search"), "Search"),
                            self.sidebar == Sidebar::Search,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_sidebar(Sidebar::Search, window, cx)
                        })),
                    )
                    .child(
                        compact_button(Button::new("refresh-files"), "Refresh")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.disk_events.store(true, Ordering::Relaxed);
                                this.message("Refreshing project files…", false, cx);
                            })),
                    ),
            )
            .child(
                div().p_2().child(
                    Input::new(&self.query)
                        .small()
                        .aria_label(match self.sidebar {
                            Sidebar::Files => "Quick open files",
                            Sidebar::Search => "Search project text",
                            Sidebar::New => "New file relative path",
                        }),
                ),
            )
            .when(self.sidebar != Sidebar::Files, |d| {
                d.child(
                    div().px_2().pb_2().child(
                        compact_button(
                            Button::new("submit-query"),
                            if self.sidebar == Sidebar::New {
                                "Create File"
                            } else {
                                "Search"
                            },
                        )
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| this.submit_query(window, cx))),
                    ),
                )
            })
            .child(
                div()
                    .id("file-list")
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .when(self.sidebar == Sidebar::Files, |d| d.children(files))
                    .when(self.sidebar == Sidebar::Search, |d| d.children(hits)),
            )
            .child(
                div()
                    .p_2()
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .child(if self.index.truncated {
                        "Listing limited to 20,000 files".into()
                    } else {
                        format!("{} indexed files", self.index.files.len())
                    }),
            )
            .into_any_element()
    }

    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let tab_path = tab.document.path.clone();
                navigation_button(
                    compact_button(
                        Button::new(SharedString::from(format!("tab:{tab_path}"))),
                        format!(
                            "{}{}{}",
                            tab.document.path,
                            if tab.document.dirty() { " •" } else { "" },
                            if tab.saving { " ⟳" } else { "" }
                        ),
                    )
                    .tooltip(tab_path.clone()),
                    self.active == Some(index),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(index) = this
                        .tabs
                        .iter()
                        .position(|tab| tab.document.path == tab_path)
                    {
                        this.select_tab(index, window, cx);
                    }
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id("tabs")
            .h_flex()
            .gap_1()
            .min_w_0()
            .flex_shrink_0()
            .bg(p.chrome())
            .border_b_1()
            .border_color(p.border)
            .overflow_x_scroll()
            .p_1()
            .children(tabs)
            .when(self.active.is_some(), |d| {
                d.child(
                    compact_button(Button::new("close-tab"), "Close")
                        .icon(IconName::Close)
                        .ghost()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_file(false, window, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    fn content_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let active = self.active.and_then(|i| self.tabs.get(i));
        let editor = active.map(|t| t.editor.clone());
        let path = active.map(|t| t.document.path.clone()).unwrap_or_default();
        let conflict = active.is_some_and(|t| t.document.conflict);
        let recovered = active.is_some_and(|t| t.document.recovered);
        let read_only = active.is_some_and(|t| t.document.read_only);
        let notice = if conflict {
            "Disk changed · Your draft is preserved. Use Changes to compare or Save Copy (⇧⌘S)."
        } else if read_only {
            "Read-only file · Save Copy (⇧⌘S) keeps your changes."
        } else {
            "Recovered unsaved draft · Review before saving."
        };
        let recent = self
            .recent
            .iter()
            .take(if self.project.is_none() { 5 } else { 0 })
            .enumerate()
            .map(|(index, root)| {
                let path = PathBuf::from(root);
                navigation_button(
                    path_button(Button::new(("recent", index)), root.clone()),
                    false,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_project(path.clone(), window, cx)
                }))
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = div()
            .v_flex()
            .size_full()
            .min_w_0()
            .justify_center()
            .items_center()
            .gap_4()
            .p_6()
            .text_size(px(13.))
            .text_color(p.secondary_foreground)
            .child(
                div()
                    .text_size(px(16.))
                    .font_semibold()
                    .text_color(p.foreground)
                    .child(if self.project.is_some() {
                        "Choose a file to start coding"
                    } else {
                        "Open your project"
                    }),
            )
            .child("⌘P opens files · ⌘N creates a file · ⌘S saves")
            .children(recent);
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(p.content)
            .child(self.tab_strip(cx))
            .when(conflict || recovered || read_only, |d| {
                d.child(
                    div()
                        .id("file-notice")
                        .role(Role::Label)
                        .aria_label(notice)
                        .p_2()
                        .bg(p.warning_surface)
                        .text_color(p.warning)
                        .text_size(px(13.))
                        .child(notice),
                )
            })
            .when(conflict, |d| {
                d.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .bg(p.warning_surface)
                        .gap_2()
                        .px_2()
                        .pb_2()
                        .child(
                            compact_button(Button::new("use-disk"), "Use Disk Version").on_click(
                                cx.listener(|this, _, window, cx| this.use_disk(window, cx)),
                            ),
                        )
                        .child(
                            compact_button(Button::new("save-conflict-copy"), "Save Draft Copy…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_copy(window, cx)),
                                ),
                        ),
                )
            })
            .when(self.close_pending, |d| {
                d.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .p_2()
                        .gap_2()
                        .bg(p.warning_surface)
                        .text_color(p.warning)
                        .text_size(px(13.))
                        .child("Unsaved changes")
                        .child(
                            compact_button(Button::new("keep-draft-close"), "Keep Draft & Close")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_file(true, window, cx)
                                })),
                        )
                        .child(
                            compact_button(Button::new("cancel-close"), "Keep Editing").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.close_pending = false;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .when_some(self.diff.clone(), |d, diff| {
                        d.child(
                            Editor::new(&diff)
                                .readonly(true)
                                .aria_label("Read-only changes preview")
                                .h(relative(1.0))
                                .size_full()
                                .text_size(px(self.font_size)),
                        )
                    })
                    .when(self.diff.is_none(), |d| {
                        d.when_some(editor, |d, editor| {
                            d.child(
                                Editor::new(&editor)
                                    .readonly(self.loading)
                                    .aria_label(format!("Editor: {path}"))
                                    .h(relative(1.0))
                                    .size_full()
                                    .text_size(px(self.font_size)),
                            )
                        })
                        .when(self.active.is_none(), |d| d.child(empty))
                    }),
            )
            .when(self.show_terminal, |d| {
                d.when_some(self.terminal.clone(), |d, terminal| d.child(terminal))
            })
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx.theme().mode);
        let geometry =
            WorkspaceGeometry::for_width(f32::from(window.viewport_size().width), self.dev.visible);
        let backup = if self.project.is_none() {
            "Ready"
        } else if self.checkpointed >= self.generation {
            "Drafts backed up"
        } else {
            "Backing up drafts…"
        };
        if self.show_terminal
            && let Some(terminal) = &self.terminal
        {
            let columns = (geometry.terminal_available_width() / 7.3).max(40.0) as u16;
            terminal.update(cx, |terminal, _| terminal.resize_columns(columns));
        }
        div()
            .id("workspace")
            .key_context("Workspace")
            .v_flex()
            .size_full()
            .text_color(p.foreground)
            .text_size(px(13.))
            .child(self.toolbar(cx))
            .when(self.show_recent, |d| {
                d.child(
                    div()
                        .v_flex()
                        .min_w_0()
                        .p_2()
                        .gap_1()
                        .border_b_1()
                        .border_color(p.border)
                        .bg(p.surface)
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_semibold()
                                .child("Recent projects"),
                        )
                        .children(self.recent.iter().take(8).enumerate().map(|(index, root)| {
                            let path = PathBuf::from(root);
                            navigation_button(
                                path_button(Button::new(("recent-list", index)), root.clone()),
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_project(path.clone(), window, cx)
                                },
                            ))
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(self.sidebar_view(geometry.sidebar, cx))
                    .child(self.content_view(cx))
                    .when(self.dev.visible, |d| {
                        d.child(
                            div()
                                .h_flex()
                                .items_stretch()
                                .min_h_0()
                                .flex_shrink_0()
                                .bg(p.surface)
                                .child(self.development_panel(geometry.utility, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .id("status")
                    .role(Role::Label)
                    .aria_label(format!("{}. {backup}", self.status.trim_end_matches('.')))
                    .h_flex()
                    .flex_wrap()
                    .min_w_0()
                    .flex_shrink_0()
                    .p_2()
                    .gap_3()
                    .border_t_1()
                    .border_color(p.border)
                    .bg(p.content)
                    .text_size(px(12.))
                    .text_color(p.secondary_foreground)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(self.error, |d| d.text_color(p.error))
                            .child(self.status.clone()),
                    )
                    .child(backup),
            )
    }
}
