//! The window's chrome around the panes: the title bar with its command center.
use super::chrome_logic::tooltip;
use super::commands::shortcut;
use super::*;
use crate::appearance::palette;
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
}
