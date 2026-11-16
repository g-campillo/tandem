use super::chrome::file_icon_image;
use super::chrome_logic::{file_icon, tab_labels, tab_name, tooltip};
use super::commands::shortcut;
use super::pane_logic::{pane_geometry, row_width, terminal_columns};
use super::*;
use crate::appearance::{
    PANEL_INSET, compact_button, glass_panel, navigation_button, palette, path_button, tree_button,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Editor, Input};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_core::layout::{Pane, PaneSide};

impl Workspace {
    fn sidebar_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let face = self.look.fonts.explorer.clone();
        let rows = self.navigation(cx);
        let files = rows
            .into_iter()
            .map(|(path, label, depth, directory)| {
                let expanded = self.expanded.contains(&path);
                let selected = self
                    .active
                    .and_then(|i| self.tabs.get(i))
                    .is_some_and(|t| t.document.path == path);
                // A file's icon sits under the folder icon of the directory rows.
                let (icons, indent) = if directory {
                    let (chevron, folder) = if expanded {
                        (IconName::ChevronDown, IconName::FolderOpen)
                    } else {
                        (IconName::ChevronRight, IconName::Folder)
                    };
                    (vec![Icon::new(chevron), Icon::new(folder)], depth * 12)
                } else {
                    (
                        vec![Icon::new(file_icon_image(file_icon(&path)))],
                        depth * 12 + 16,
                    )
                };
                div()
                    .min_w_0()
                    .pl(px(indent as f32))
                    .child(
                        navigation_button(
                            tree_button(
                                Button::new(SharedString::from(format!("nav:{path}"))),
                                icons,
                                label,
                                face.family.clone(),
                                face.size,
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
            .size_full()
            .min_w_0()
            .min_h_0()
            .font_family(face.family.clone())
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .h(px(32.))
                    .px_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .font_semibold()
                            .text_color(p.secondary_foreground)
                            .child(match self.sidebar {
                                Sidebar::Files => "EXPLORER",
                                Sidebar::Search => "SEARCH",
                                Sidebar::New => "NEW FILE",
                            }),
                    )
                    .child(
                        Button::new("new-file")
                            .icon(Lucide::FilePlus)
                            .ghost()
                            .xsmall()
                            .accessibility_label("New File")
                            .tooltip(tooltip("New File", shortcut("New File")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_sidebar(Sidebar::New, window, cx)
                            })),
                    )
                    .child(
                        Button::new("refresh-files")
                            .icon(Lucide::RefreshCw)
                            .ghost()
                            .xsmall()
                            .accessibility_label("Refresh Files")
                            .tooltip("Refresh Files")
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

    /// The tab bar: one tab per open file, with the file's icon, its name and, in a slot
    /// at its end, a dot for unsaved changes that turns into a close button while the pointer
    /// is on the tab. Save and Compare with Disk sit at the bar's right end.
    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let paths: Vec<&str> = self
            .tabs
            .iter()
            .map(|tab| tab.document.path.as_str())
            .collect();
        let labels = tab_labels(&paths);
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let active = self.active == Some(index);
                let dirty = tab.document.dirty();
                let path = tab.document.path.clone();
                let label = labels[index].clone();
                let selected_path = path.clone();
                let button = Button::new(SharedString::from(format!("tab:{path}")))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(Hsla::transparent_black())
                            .foreground(if active {
                                p.selected_foreground
                            } else {
                                p.foreground
                            })
                            .hover(Hsla::transparent_black())
                            .active(Hsla::transparent_black()),
                    )
                    .small()
                    .h(px(24.))
                    .icon(file_icon_image(file_icon(&path)))
                    .accessibility_label(tab_name(&path, dirty, tab.saving))
                    .tooltip(path.clone())
                    .child(div().text_size(px(13.)).child(label.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(index) = this
                            .tabs
                            .iter()
                            .position(|tab| tab.document.path == selected_path)
                        {
                            this.select_tab(index, window, cx);
                        }
                    }));
                let slot = div().relative().size(px(16.)).flex_none();
                let slot = if tab.saving {
                    slot.child(Icon::new(IconName::LoaderCircle).xsmall())
                } else {
                    let layer = || {
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                    };
                    let closed_path = path.clone();
                    let close = Button::new(("tab-close", index))
                        .icon(Lucide::X)
                        .ghost()
                        .xsmall()
                        .accessibility_label(format!("Close {label}"))
                        .tooltip(tooltip("Close", shortcut("Close File")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(index) = this
                                .tabs
                                .iter()
                                .position(|tab| tab.document.path == closed_path)
                            else {
                                return;
                            };
                            if this.active != Some(index) {
                                this.select_tab(index, window, cx);
                            }
                            this.close_file(false, window, cx);
                        }));
                    slot.when(dirty, |slot| {
                        slot.child(
                            layer()
                                .group_hover("tab", |style| style.invisible())
                                .child(div().size(px(7.)).rounded_full().bg(p.foreground)),
                        )
                    })
                    .child(
                        layer()
                            .when(!active || dirty, |layer| {
                                layer
                                    .invisible()
                                    .group_hover("tab", |style| style.visible())
                            })
                            .child(close),
                    )
                };
                div()
                    .id(("tab-item", index))
                    .group("tab")
                    .h_flex()
                    .items_center()
                    .flex_none()
                    .h(px(26.))
                    .px_1()
                    .gap_1()
                    .rounded(px(6.))
                    .when(active, |tab| tab.bg(p.selected))
                    .when(!active, |tab| tab.hover(|style| style.bg(p.hover())))
                    .child(button)
                    .child(slot)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let open = self.active.is_some();
        let comparing = self.diff.is_some();
        let compare_label = if comparing {
            "Back to Code"
        } else {
            "Compare with Disk"
        };
        let actions = div()
            .h_flex()
            .items_center()
            .gap_1()
            .flex_none()
            .px_1()
            .child(
                Button::new("tab-save")
                    .icon(Lucide::Save)
                    .ghost()
                    .xsmall()
                    .disabled(!open)
                    .accessibility_label("Save")
                    .tooltip(tooltip("Save", shortcut("Save")))
                    .on_click(cx.listener(|this, _, _, cx| this.save_active(cx))),
            )
            .child(
                Button::new("tab-compare")
                    .icon(Lucide::FileDiff)
                    .ghost()
                    .xsmall()
                    .disabled(!open)
                    .selected(comparing)
                    .accessibility_label(compare_label)
                    .tooltip(tooltip(compare_label, shortcut("Compare with Disk")))
                    .on_click(cx.listener(|this, _, window, cx| this.show_diff(window, cx))),
            );
        div()
            .id("tabs")
            .h_flex()
            .items_center()
            .h(px(34.))
            .flex_shrink_0()
            .min_w_0()
            .bg(p.content)
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .id("tab-list")
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .px_1()
                    .children(tabs),
            )
            .child(actions)
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
        // Claude edits the files on disk during a takeover, so no buffer may change meanwhile.
        let takeover_running = self.takeover_running();
        let notice = if conflict {
            "Disk changed · Your draft is preserved. Use Changes to compare or Save Copy (⇧⌘S)."
        } else if read_only {
            "Read-only file · Save Copy (⇧⌘S) keeps your changes."
        } else {
            "Recovered unsaved draft · Review before saving."
        };
        let face = self.look.fonts.editor.clone();
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
            .id("welcome-content")
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .v_flex()
                    .w_full()
                    .min_h(relative(1.0))
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
                    .when(self.project.is_none(), |d| {
                        d.child(
                            compact_button(Button::new("welcome-open-folder"), "Open Folder…")
                                .icon(IconName::FolderOpen)
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_project(window, cx)
                                })),
                        )
                    })
                    .child("⌘P opens files · ⌘N creates a file · ⌘S saves")
                    .children(recent),
            );
        let editor_block = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(p.content)
            .when(!self.tabs.is_empty(), |d| d.child(self.tab_strip(cx)))
            .children(
                self.breadcrumbs(cx)
                    .filter(|_| self.diff.is_none() && !self.settings.open),
            )
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
                    .when(self.settings.open, |d| d.child(self.settings_view(cx)))
                    .when_some(
                        self.diff.clone().filter(|_| !self.settings.open),
                        |d, diff| {
                            d.child(
                                Editor::new(&diff)
                                    .readonly(true)
                                    .aria_label("Read-only changes preview")
                                    .h(relative(1.0))
                                    .size_full()
                                    .font_family(face.family.clone())
                                    .font_features(face.features.clone())
                                    .text_size(px(face.size)),
                            )
                        },
                    )
                    .when(self.diff.is_none() && !self.settings.open, |d| {
                        d.when_some(editor, |d, editor| {
                            let (hovered, tip) = (editor.clone(), self.hover.clone());
                            d.on_mouse_move(move |event, _, cx| {
                                super::hover::claim_diagnostic(&hovered, &tip, event, cx)
                            })
                            .child(
                                Editor::new(&editor)
                                    .readonly(self.loading || takeover_running)
                                    .aria_label(format!("Editor: {path}"))
                                    .h(relative(1.0))
                                    .size_full()
                                    .font_family(face.family.clone())
                                    .font_features(face.features.clone())
                                    .text_size(px(face.size)),
                            )
                            .child(self.hover.clone())
                        })
                        .when(self.active.is_none(), |d| d.child(empty))
                    }),
            );
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(editor_block)
            .children(self.bottom_panel(cx).map(|panel| {
                div()
                    .flex_shrink_0()
                    .min_w_0()
                    .p(px(PANEL_INSET))
                    .child(glass_panel(cx).child(panel))
            }))
            .into_any_element()
    }

    /// The Terminal and Problems tabs below the editor, while one of them is shown.
    fn bottom_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bottom = self.bottom?;
        let p = palette(cx.theme().mode);
        let header = div()
            .h_flex()
            .flex_wrap()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(p.border)
            .child(
                navigation_button(
                    compact_button(Button::new("bottom-terminal"), "Terminal")
                        .icon(IconName::SquareTerminal),
                    bottom == BottomTab::Terminal,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_terminal(cx))),
            )
            .child(
                navigation_button(
                    compact_button(
                        Button::new("bottom-problems"),
                        format!("Problems ({})", self.dev.problems.len()),
                    )
                    .icon(IconName::TriangleAlert),
                    bottom == BottomTab::Problems,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.bottom = next_bottom(this.bottom, BottomTab::Problems);
                    cx.notify();
                })),
            )
            .child(div().flex_1().min_w_0())
            .when(self.terminal.is_some(), |d| {
                d.child(
                    Button::new("close-terminal")
                        .icon(Lucide::Trash)
                        .ghost()
                        .xsmall()
                        .tooltip("Stop Terminal")
                        .accessibility_label("Stop Terminal")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.terminal = None;
                            if this.bottom == Some(BottomTab::Terminal) {
                                this.bottom = None;
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("hide-panel")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .accessibility_label("Hide Panel")
                    .tooltip(tooltip("Hide Panel", shortcut("Toggle Panel")))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bottom_panel(cx))),
            );
        let content = match bottom {
            BottomTab::Terminal => self
                .terminal
                .clone()
                .map(|terminal| terminal.into_any_element()),
            BottomTab::Problems => Some(
                div()
                    .id("bottom-problems-content")
                    .min_w_0()
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .p_3()
                    .child(self.problems_panel(cx))
                    .into_any_element(),
            ),
        };
        Some(
            div()
                .v_flex()
                .flex_shrink_0()
                .min_w_0()
                .child(header)
                .children(content)
                .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx.theme().mode);
        let shown = self.shown_panes();
        let geometry = pane_geometry(
            row_width(f32::from(window.viewport_size().width)),
            &self.panes.layout,
            shown,
        );
        if self.bottom == Some(BottomTab::Terminal)
            && let Some(terminal) = &self.terminal
        {
            // A hidden file tree has width 0 in `geometry`, so the editor column already
            // includes its space.
            let cell = terminal.read(cx).cell_width(window);
            let columns = terminal_columns(geometry.terminal_available_width(), cell);
            terminal.update(cx, |terminal, _| terminal.resize_columns(columns));
        }
        let files = shown.files.then(|| {
            let files = self.sidebar_view(cx);
            self.pane_frame(Pane::Files, geometry.sidebar, files, cx)
        });
        let chat = shown.chat.then(|| {
            let chat = self.chat_panel(cx);
            self.pane_frame(Pane::Chat, geometry.utility, chat, cx)
        });
        // The activity bar sits on the window's edge beside the file tree.
        let activity_bar = Some(self.activity_bar(cx));
        let (bar_left, left, right, bar_right) = match self.panes.layout.side(Pane::Files) {
            PaneSide::Left => (activity_bar, files, chat, None),
            PaneSide::Right => (None, chat, files, activity_bar),
        };
        div()
            .id("workspace")
            .key_context("Workspace")
            .v_flex()
            .size_full()
            .bg(p.chrome())
            .text_color(p.foreground)
            .text_size(px(13.))
            .child(self.title_bar(cx))
            .when(self.show_recent, |d| {
                d.child(
                    div()
                        .id("recent-projects-list")
                        .v_flex()
                        .min_w_0()
                        .min_h_0()
                        .max_h(relative(0.25))
                        .flex_shrink_0()
                        .overflow_y_scroll()
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
                    .children(bar_left)
                    .child(
                        div()
                            .h_flex()
                            .items_stretch()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .on_drag_move(cx.listener(Self::resize_pane))
                            .children(left)
                            .child(self.content_view(cx))
                            .children(right),
                    )
                    .children(bar_right),
            )
            .child(self.status_bar(cx))
    }
}
