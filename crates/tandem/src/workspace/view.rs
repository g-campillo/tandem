use super::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Input};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h_flex()
            .flex_shrink_0()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().font_semibold().px_2().child("Tandem"))
            .child(
                Button::new("open-project")
                    .label("Open Folder")
                    .on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx))),
            )
            .child(
                Button::new("recent-projects")
                    .label("Recent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_recent = !this.show_recent;
                        cx.notify();
                    })),
            )
            .child(Button::new("new-file").label("New File").on_click(
                cx.listener(|this, _, window, cx| this.set_sidebar(Sidebar::New, window, cx)),
            ))
            .child(
                Button::new("save")
                    .label("Save  ⌘S")
                    .on_click(cx.listener(|this, _, _, cx| this.save_active(cx))),
            )
            .child(
                Button::new("changes")
                    .label(if self.diff.is_some() {
                        "Back to Code"
                    } else {
                        "Changes"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.show_diff(window, cx))),
            )
            .child(
                Button::new("terminal")
                    .label(if self.show_terminal {
                        "Hide Terminal"
                    } else {
                        "Terminal"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_terminal(cx))),
            )
            .when(self.terminal.is_some(), |d| {
                d.child(
                    Button::new("close-terminal")
                        .label("Stop Terminal")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.terminal = None;
                            this.show_terminal = false;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("development-tools")
                    .label(if self.dev.visible {
                        "Hide Companion"
                    } else {
                        "Companion & Tools"
                    })
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
                    .text_sm()
                    .child(if self.agent.busy {
                        "Agent · isolated copy"
                    } else {
                        "AI edits off"
                    }),
            )
            .into_any_element()
    }

    fn sidebar_view(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    .pl(px((depth * 12) as f32))
                    .child(
                        Button::new(SharedString::from(format!("nav:{path}")))
                            .label(format!(
                                "{}{}",
                                if directory {
                                    if expanded { "▾ " } else { "▸ " }
                                } else {
                                    ""
                                },
                                label
                            ))
                            .w_full()
                            .ghost()
                            .justify_start()
                            .when(selected, |b| b.primary())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if directory {
                                    if !this.expanded.remove(&path) {
                                        this.expanded.insert(path.clone());
                                    }
                                    cx.notify();
                                } else {
                                    this.open_file(path.clone(), false, None, window, cx);
                                }
                            })),
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
                Button::new(("hit", i))
                    .label(format!("{}:{}  {}", hit.path, hit.line, hit.preview))
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(path.clone(), false, Some(line), window, cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .v_flex()
            .w(px(270.0))
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .p_2()
                    .child(
                        Button::new("files")
                            .label("Files")
                            .when(self.sidebar == Sidebar::Files, |b| b.primary())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_sidebar(Sidebar::Files, window, cx)
                            })),
                    )
                    .child(
                        Button::new("search")
                            .label("Search")
                            .when(self.sidebar == Sidebar::Search, |b| b.primary())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_sidebar(Sidebar::Search, window, cx)
                            })),
                    )
                    .child(
                        Button::new("refresh-files")
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.disk_events.store(true, Ordering::Relaxed);
                                this.message("Refreshing project files…", false, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .p_2()
                    .child(Input::new(&self.query).aria_label(match self.sidebar {
                        Sidebar::Files => "Quick open files",
                        Sidebar::Search => "Search project text",
                        Sidebar::New => "New file relative path",
                    })),
            )
            .when(self.sidebar != Sidebar::Files, |d| {
                d.child(
                    div().px_2().pb_2().child(
                        Button::new("submit-query")
                            .label(if self.sidebar == Sidebar::New {
                                "Create File"
                            } else {
                                "Search"
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_query(window, cx)),
                            ),
                    ),
                )
            })
            .child(
                div()
                    .id("file-list")
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .when(self.sidebar == Sidebar::Files, |d| d.children(files))
                    .when(self.sidebar == Sidebar::Search, |d| d.children(hits)),
            )
            .child(div().p_2().text_xs().child(if self.index.truncated {
                "Listing limited to 20,000 files".into()
            } else {
                format!("{} indexed files", self.index.files.len())
            }))
            .into_any_element()
    }

    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let tab_path = tab.document.path.clone();
                Button::new(SharedString::from(format!("tab:{tab_path}")))
                    .label(format!(
                        "{}{}{}",
                        tab.document.path,
                        if tab.document.dirty() { " •" } else { "" },
                        if tab.saving { " ⟳" } else { "" }
                    ))
                    .when(self.active == Some(index), |b| b.primary())
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
            .flex_shrink_0()
            .overflow_x_scroll()
            .p_1()
            .children(tabs)
            .when(self.active.is_some(), |d| {
                d.child(Button::new("close-tab").label("Close").on_click(
                    cx.listener(|this, _, window, cx| this.close_file(false, window, cx)),
                ))
            })
            .into_any_element()
    }

    fn content_view(&self, cx: &mut Context<Self>) -> AnyElement {
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
                Button::new(("recent", index))
                    .label(root.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_project(path.clone(), window, cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = div()
            .v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .gap_4()
            .p_8()
            .child(div().text_xl().child(if self.project.is_some() {
                "Choose a file to start coding"
            } else {
                "Open your project"
            }))
            .child("⌘P opens files · ⌘N creates a file · ⌘S saves")
            .children(recent);
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.tab_strip(cx))
            .when(conflict || recovered || read_only, |d| {
                d.child(
                    div()
                        .id("file-notice")
                        .role(Role::Label)
                        .aria_label(notice)
                        .p_2()
                        .text_sm()
                        .child(notice),
                )
            })
            .when(conflict, |d| {
                d.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .px_2()
                        .pb_2()
                        .child(
                            Button::new("use-disk").label("Use Disk Version").on_click(
                                cx.listener(|this, _, window, cx| this.use_disk(window, cx)),
                            ),
                        )
                        .child(
                            Button::new("save-conflict-copy")
                                .label("Save Draft Copy…")
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
                        .p_2()
                        .gap_2()
                        .child("Unsaved changes")
                        .child(
                            Button::new("keep-draft-close")
                                .label("Keep Draft & Close")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_file(true, window, cx)
                                })),
                        )
                        .child(Button::new("cancel-close").label("Keep Editing").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.close_pending = false;
                                cx.notify();
                            }),
                        )),
                )
            })
            .child(
                div()
                    .flex_1()
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
            let columns =
                ((f32::from(window.viewport_size().width) - 286.0) / 7.3).max(40.0) as u16;
            terminal.update(cx, |terminal, _| terminal.resize_columns(columns));
        }
        div()
            .id("workspace")
            .key_context("Workspace")
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.toolbar(cx))
            .when(self.show_recent, |d| {
                d.child(
                    div()
                        .v_flex()
                        .p_2()
                        .gap_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child("Recent projects")
                        .children(self.recent.iter().take(8).enumerate().map(|(index, root)| {
                            let path = PathBuf::from(root);
                            Button::new(("recent-list", index))
                                .label(root.clone())
                                .justify_start()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_project(path.clone(), window, cx)
                                }))
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar_view(cx))
                    .child(self.content_view(cx))
                    .when(self.dev.visible, |d| d.child(self.development_panel(cx))),
            )
            .child(
                div()
                    .id("status")
                    .role(Role::Label)
                    .aria_label(format!("{}. {backup}", self.status.trim_end_matches('.')))
                    .h_flex()
                    .flex_shrink_0()
                    .p_2()
                    .gap_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .when(self.error, |d| d.text_color(rgb(0xb42318)))
                            .child(self.status.clone()),
                    )
                    .child(backup),
            )
    }
}
