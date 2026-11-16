//! Application-owned semantic colors, reapplied after the toolkit loads a mode.
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::{Icon, Selectable, Sizable, StyledExt, Theme, ThemeMode, ThemeTokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Div, Hsla, IntoElement, ParentElement, SharedString, Subscription, Window, div, rgb,
};
use gpui_kit::{FontWeight, Styled, px};
use std::sync::Arc;

/// Pane widths include their separators. Keep the editor and terminal on the
/// same available-width calculation as the visible navigation panes.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceGeometry {
    pub sidebar: f32,
    pub utility: f32,
    pub content: f32,
}

impl WorkspaceGeometry {
    pub fn for_width(width: f32, utility_visible: bool) -> Self {
        let width = width.max(0.);
        let progress = ((width - 900.) / (1280. - 900.)).clamp(0., 1.);
        let sidebar = 210. + 40. * progress;
        let utility = if utility_visible {
            320. + 70. * progress
        } else {
            0.
        };
        Self {
            sidebar,
            utility,
            content: (width - sidebar - utility).max(0.),
        }
    }

    pub fn terminal_available_width(self) -> f32 {
        // TerminalView retains p_2: 8px on either side with the 16px rem basis.
        (self.content - 16.).max(0.)
    }
}

/// Toolkit buttons retain their keyboard/accessibility behavior; a child label
/// lets chrome use 13px type independently of the toolkit's 16px spacing basis.
pub fn compact_button(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button
        .small()
        .h(px(28.))
        .rounded(px(6.))
        .font_family(".SystemUIFont")
        .accessibility_label(label.clone())
        .child(div().text_size(px(13.)).child(label))
}

/// A clickable item of the status bar: 20 px high with 12 px text.
pub fn status_item(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button
        .ghost()
        .xsmall()
        .h(px(20.))
        .px_1()
        .rounded(px(4.))
        .accessibility_label(label.clone())
        .child(div().text_size(px(12.)).child(label))
}

/// Long paths stay visible and retain their complete accessible name.
pub fn path_button(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button
        .small()
        .h_auto()
        .min_h(px(28.))
        .py_1()
        .w_full()
        .min_w_0()
        .justify_start()
        .rounded(px(6.))
        .accessibility_label(label.clone())
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_size(px(13.))
                .child(label),
        )
}

/// One row of the file tree: a single line that is cut off with an ellipsis, in the
/// explorer's font, behind its icons.
pub fn tree_button(
    button: Button,
    icons: Vec<Icon>,
    label: impl Into<SharedString>,
    family: SharedString,
    size: f32,
) -> Button {
    let label = label.into();
    button
        .small()
        .h(px((size * 1.7).round().max(22.)))
        .w_full()
        .min_w_0()
        .justify_start()
        .rounded(px(6.))
        .accessibility_label(label.clone())
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .w_full()
                .min_w_0()
                .children(icons.into_iter().map(|icon| icon.xsmall()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(family)
                        .text_size(px(size))
                        .child(label),
                ),
        )
}

/// Content-sized actions wrap their label within a narrow utility viewport.
pub fn wrapping_button(button: Button, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    button
        .small()
        .h_auto()
        .min_h(px(28.))
        .max_w_full()
        .min_w_0()
        .py_1()
        .rounded(px(6.))
        .accessibility_label(label.clone())
        .child(
            div()
                .min_w_0()
                .whitespace_normal()
                .text_size(px(13.))
                .child(label),
        )
}

/// Shared utility-panel typography and spacing. Content stays opaque beneath
/// the independently tinted navigation header.
pub fn panel_body(cx: &App) -> Div {
    let p = palette(Theme::global(cx).mode);
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .gap_3()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(p.foreground)
        .whitespace_normal()
}

pub fn panel_heading(content: impl IntoElement) -> Div {
    div()
        .min_w_0()
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(content)
}

pub fn panel_metadata(content: impl IntoElement, cx: &App) -> Div {
    div()
        .min_w_0()
        .text_size(px(12.))
        .text_color(palette(Theme::global(cx).mode).secondary_foreground)
        .child(content)
}

pub fn panel_warning(content: impl IntoElement, cx: &App) -> Div {
    let p = palette(Theme::global(cx).mode);
    solid_section(div(), cx)
        .min_w_0()
        .p_3()
        .bg(p.warning_surface)
        .text_color(p.warning)
        .child(content)
}

pub fn panel_error(content: impl IntoElement, cx: &App) -> Div {
    let p = palette(Theme::global(cx).mode);
    solid_section(div(), cx)
        .min_w_0()
        .p_3()
        .bg(p.error_surface)
        .text_color(p.error)
        .child(content)
}

/// Code and paths retain their exact text and can be scrolled horizontally.
pub fn panel_code<T: Styled>(element: T, cx: &App) -> T {
    solid_section(element, cx)
        .bg(palette(Theme::global(cx).mode).content)
        .min_w_0()
        .p_3()
        .font_family(Theme::global(cx).mono_font_family.clone())
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(palette(Theme::global(cx).mode).foreground)
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub content: Hsla,
    pub surface: Hsla,
    pub chrome: Hsla,
    pub foreground: Hsla,
    pub secondary_foreground: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub focus: Hsla,
    pub accent_foreground: Hsla,
    pub selected: Hsla,
    pub selected_foreground: Hsla,
    pub error: Hsla,
    pub error_surface: Hsla,
    pub warning: Hsla,
    pub warning_surface: Hsla,
    pub success: Hsla,
    pub success_surface: Hsla,
}

pub fn palette(mode: ThemeMode) -> Palette {
    if mode.is_dark() {
        Palette {
            content: rgb(0x1C1D21).into(),
            surface: rgb(0x252830).into(),
            chrome: Hsla::from(rgb(0x22252D)).alpha(0.88),
            foreground: rgb(0xECEDEF).into(),
            secondary_foreground: rgb(0xADB1BC).into(),
            border: rgb(0x434852).into(),
            accent: rgb(0x78ABFF).into(),
            focus: rgb(0xD7E6FF).into(),
            accent_foreground: rgb(0x1C1D21).into(),
            selected: rgb(0x303F5B).into(),
            selected_foreground: rgb(0xD7E6FF).into(),
            error: rgb(0xFF9AA7).into(),
            error_surface: rgb(0x3B262C).into(),
            warning: rgb(0xF1C36B).into(),
            warning_surface: rgb(0x3C3323).into(),
            success: rgb(0x8CD6AF).into(),
            success_surface: rgb(0x253A30).into(),
        }
    } else {
        Palette {
            content: rgb(0xFBFBFD).into(),
            surface: rgb(0xFFFFFF).into(),
            chrome: Hsla::from(rgb(0xF1F3F7)).alpha(0.86),
            foreground: rgb(0x202126).into(),
            // 4.6:1 even over the darkest composited light chrome.
            secondary_foreground: rgb(0x545862).into(),
            border: rgb(0xD5D8E0).into(),
            accent: rgb(0x1D5FCC).into(),
            // The toolkit paints its outside focus band at 50% opacity.
            focus: rgb(0x001538).into(),
            accent_foreground: rgb(0xFFFFFF).into(),
            selected: rgb(0xE2ECFF).into(),
            selected_foreground: rgb(0x153F83).into(),
            error: rgb(0xB42332).into(),
            error_surface: rgb(0xFBECEF).into(),
            warning: rgb(0x805400).into(),
            warning_surface: rgb(0xFFF4D6).into(),
            success: rgb(0x246542).into(),
            success_surface: rgb(0xE8F5ED).into(),
        }
    }
}

pub fn apply(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    apply_palette(cx);
}

pub fn install(window: &mut Window, cx: &mut App) -> Subscription {
    apply(window, cx);
    window.observe_window_appearance(apply)
}

/// The family of the editor's font. The toolkit draws its own code in it too.
struct CodeFont(SharedString);

impl gpui_kit::Global for CodeFont {}

/// Makes `family` the code font of everything the toolkit draws.
pub fn set_code_font(cx: &mut App, family: SharedString) {
    cx.set_global(CodeFont(family));
    apply_palette(cx);
}

fn apply_palette(cx: &mut App) {
    let code_font = cx.try_global::<CodeFont>().map(|font| font.0.clone());
    Theme::update(cx, |theme| {
        project(theme);
        project_code_font(theme, code_font);
    });
}

/// Pure: the theme's code font becomes `family`, when there is one.
fn project_code_font(theme: &mut Theme, family: Option<SharedString>) {
    if let Some(family) = family {
        theme.mono_font_family = family;
    }
}

impl Palette {
    pub fn hover(self) -> Hsla {
        self.surface.blend(self.selected.alpha(0.45))
    }

    pub fn pressed(self) -> Hsla {
        self.surface.blend(self.selected.alpha(0.75))
    }

    pub fn accent_hover(self) -> Hsla {
        self.accent.blend(self.foreground.alpha(0.10))
    }

    pub fn accent_pressed(self) -> Hsla {
        self.accent.blend(self.foreground.alpha(0.18))
    }

    /// Keep a solid fallback on backends without the native macOS material.
    pub fn chrome(self) -> Hsla {
        if cfg!(target_os = "macos") {
            self.chrome
        } else {
            self.surface
        }
    }
}

/// Pure presentation projection; production always invokes it via Theme::update
/// after sync_system_appearance has loaded that mode's toolkit/syntax theme.
fn project(theme: &mut Theme) {
    let p = palette(theme.mode);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(16.);
    theme.radius = px(6.);
    theme.radius_lg = px(10.);
    theme.focus_ring = true;

    theme.background = p.content;
    theme.foreground = p.foreground;
    theme.border = p.border;
    theme.input = p.border;
    theme.ring = p.focus;
    theme.drag_border = p.accent;
    theme.caret = p.accent;
    theme.popover = p.surface;
    theme.popover_foreground = p.foreground;
    theme.accordion = p.surface;
    theme.group_box = p.surface;
    theme.group_box_foreground = p.foreground;
    theme.muted = p.hover();
    theme.muted_foreground = p.secondary_foreground;
    theme.accent = p.selected;
    theme.accent_foreground = p.selected_foreground;
    theme.selection = p.selected;
    theme.primary = p.accent;
    theme.primary_hover = p.accent_hover();
    theme.primary_active = p.accent_pressed();
    theme.primary_foreground = p.accent_foreground;
    theme.secondary = p.surface;
    theme.secondary_hover = p.hover();
    theme.secondary_active = p.pressed();
    theme.secondary_foreground = p.foreground;
    theme.link = p.accent;
    theme.link_hover = p.accent_hover();
    theme.link_active = p.accent_pressed();

    theme.button = p.surface;
    theme.button_hover = p.hover();
    theme.button_active = p.pressed();
    theme.button_foreground = p.foreground;
    theme.button_primary = p.accent;
    theme.button_primary_hover = p.accent_hover();
    theme.button_primary_active = p.accent_pressed();
    theme.button_primary_foreground = p.accent_foreground;
    theme.button_secondary = p.surface;
    theme.button_secondary_hover = p.hover();
    theme.button_secondary_active = p.pressed();
    theme.button_secondary_foreground = p.foreground;

    theme.danger = p.error_surface;
    theme.danger_foreground = p.error;
    theme.danger_hover = p.error_surface.blend(p.error.alpha(0.08));
    theme.danger_active = p.error_surface.blend(p.error.alpha(0.14));
    theme.button_danger = theme.danger;
    theme.button_danger_hover = theme.danger_hover;
    theme.button_danger_active = theme.danger_active;
    theme.button_danger_foreground = p.error;
    theme.warning = p.warning_surface;
    theme.warning_foreground = p.warning;
    theme.warning_hover = p.warning_surface.blend(p.warning.alpha(0.08));
    theme.warning_active = p.warning_surface.blend(p.warning.alpha(0.14));
    theme.button_warning = theme.warning;
    theme.button_warning_hover = theme.warning_hover;
    theme.button_warning_active = theme.warning_active;
    theme.button_warning_foreground = p.warning;
    theme.success = p.success_surface;
    theme.success_foreground = p.success;
    theme.success_hover = p.success_surface.blend(p.success.alpha(0.08));
    theme.success_active = p.success_surface.blend(p.success.alpha(0.14));
    theme.button_success = theme.success;
    theme.button_success_hover = theme.success_hover;
    theme.button_success_active = theme.success_active;
    theme.button_success_foreground = p.success;
    theme.info = p.selected;
    theme.info_foreground = p.selected_foreground;
    theme.info_hover = p.hover();
    theme.info_active = p.pressed();
    theme.button_info = theme.info;
    theme.button_info_hover = theme.info_hover;
    theme.button_info_active = theme.info_active;
    theme.button_info_foreground = p.selected_foreground;
    theme.red = p.error;
    theme.yellow = p.warning;
    theme.green = p.success;
    theme.blue = p.accent;

    theme.sidebar = p.surface;
    theme.sidebar_foreground = p.foreground;
    theme.sidebar_border = p.border;
    theme.sidebar_accent = p.selected;
    theme.sidebar_accent_foreground = p.selected_foreground;
    theme.sidebar_primary = p.selected;
    theme.sidebar_primary_foreground = p.selected_foreground;
    theme.colors.list = p.surface;
    theme.list_head = p.surface;
    theme.list_even = p.content;
    theme.list_hover = p.hover();
    theme.list_active = p.selected;
    theme.list_active_border = p.accent;
    theme.tab = p.surface;
    theme.tab_foreground = p.secondary_foreground;
    theme.tab_active = p.selected;
    theme.tab_active_foreground = p.selected_foreground;
    theme.tab_bar = p.surface;
    theme.tab_bar_segmented = p.content;
    theme.title_bar = p.chrome();
    theme.title_bar_border = p.border;
    theme.status_bar = p.content;
    theme.status_bar_border = p.border;

    let highlight = Arc::make_mut(&mut theme.highlight_theme);
    highlight.style.editor_background = Some(p.content);
    highlight.style.editor_gutter_background = Some(p.content);
    highlight.style.editor_foreground = Some(p.foreground);
    highlight.style.editor_line_number = Some(p.secondary_foreground);
    highlight.style.editor_active_line_number = Some(p.foreground);
    highlight.style.editor_active_line = Some(p.hover());
    // Application colors are solid. Set the tokens as well so a same-valued
    // color cannot accidentally retain a gradient from a toolkit reload.
    theme.tokens = ThemeTokens::from(theme.colors);
}

/// Style an existing navigation action without changing its label or handler.
// Consumed by the following workspace/panel presentation tasks.
#[allow(dead_code)]
pub fn navigation_button(button: Button, selected: bool, cx: &App) -> Button {
    let p = palette(Theme::global(cx).mode);
    button
        .custom(
            ButtonCustomVariant::new(cx)
                .color(if selected {
                    p.selected
                } else {
                    Hsla::transparent_black()
                })
                .foreground(if selected {
                    p.selected_foreground
                } else {
                    p.foreground
                })
                .hover(if selected { p.selected } else { p.hover() })
                .active(if selected { p.selected } else { p.pressed() }),
        )
        .selected(selected)
        .rounded(px(6.))
        .when(selected, |button| button.font_weight(FontWeight::SEMIBOLD))
}

/// Give an existing section a solid, readable surface.
#[allow(dead_code)]
pub fn solid_section<T: Styled>(element: T, cx: &App) -> T {
    let p = palette(Theme::global(cx).mode);
    element
        .bg(p.surface)
        .border_color(p.border)
        .rounded(px(10.))
}

#[cfg(test)]
mod tests;
