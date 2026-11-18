use super::*;
use crate::appearance::{RADIUS_CONTROL, panel_body, panel_code, panel_heading, path_button};
use crate::scrolling::{SideScroll, side_scroll};
use gpui_kit::component::button::Button;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn problems_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .dev
            .diagnostic_stamp
            .as_ref()
            .is_some_and(|s| self.language_manager().is_some_and(|m| m.is_current(s)));
        let navigation_current = self
            .dev
            .navigation_stamp
            .as_ref()
            .is_some_and(|s| self.language_manager().is_some_and(|m| m.is_current(s)));
        panel_body(cx)
            .child(panel_heading(note("Current language buffer")))
            .child(if self.dev.diagnostic_busy {
                "Checking the current language snapshot…"
            } else if current && self.dev.problems.is_empty() {
                "No language diagnostics reported for this buffer. Cargo checks are separate."
            } else if current {
                "Language diagnostics for the inspected open-buffer context"
            } else {
                "No current diagnostics. Enable tools and open a Rust or Python file."
            })
            .child(
                path_button(
                    Button::new("refresh-diagnostics"),
                    "Check Language Snapshot",
                )
                .icon(IconName::RefreshCw)
                .disabled(self.dev.diagnostic_busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.language_action("textDocument/diagnostic", window, cx)
                })),
            )
            .when(current, |d| {
                d.children(self.dev.problems.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    path_button(Button::new(("problem", i)), item.label).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        },
                    ))
                }))
            })
            .when(navigation_current, |d| {
                d.child(
                    panel_heading(note("Navigation results"))
                        .mt_3()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border),
                )
                .children(self.dev.navigation.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    path_button(Button::new(("location", i)), item.label).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        },
                    ))
                }))
            })
            .children(self.call_hierarchy_section(cx))
            .into_any_element()
    }
}

#[track_caller]
pub(super) fn note(text: impl Into<SharedString>) -> impl IntoElement {
    let text = text.into();
    let call = std::panic::Location::caller();
    div()
        .id(SharedString::from(format!(
            "note:{}:{}:{text}",
            call.line(),
            call.column()
        )))
        .role(Role::Label)
        .aria_label(text.clone())
        .child(text)
}

/// Full path/argv text remains available even when it contains no wrap points.
pub(super) fn code_note(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    cx: &App,
) -> SideScroll {
    panel_code(side_scroll(id), cx)
        .p_2()
        .rounded(px(RADIUS_CONTROL))
        .child(
            div()
                .flex_none()
                .w_auto()
                .min_w_full()
                .whitespace_nowrap()
                .child(note(text)),
        )
}
