//! The Outline section at the bottom of the Explorer: the symbols of the open file, from
//! the same document-symbol answer as the breadcrumbs, indented by nesting, with the
//! symbol the cursor is in selected. Clicking a symbol moves the cursor to its line.
use super::icons::symbol_icon;
use super::*;
use crate::appearance::{navigation_button, palette, tree_button};
use crate::scrolling::VerticalOnly as _;
use crate::workspace::breadcrumbs::OutlineSymbol;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _};

/// The indentation of one level of nesting, in pixels, as in the file tree.
const INDENT: f32 = 12.;

/// The innermost symbol of `outline` that contains `line` (0-based): the one with the
/// fewest lines and, of two with the same lines, the later one, since the server lists a
/// parent before its children. None when no symbol contains the line.
fn current_symbol(outline: &[OutlineSymbol], line: u32) -> Option<usize> {
    outline
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.start_line <= line && line <= symbol.end_line)
        .min_by_key(|(index, symbol)| {
            (
                symbol.end_line - symbol.start_line,
                std::cmp::Reverse(*index),
            )
        })
        .map(|(index, _)| index)
}

impl Workspace {
    /// The Outline section of the Explorer, below the file tree: a header that expands and
    /// collapses it and, while it is expanded, the symbols of the active tab's file. None
    /// while the sidebar shows Search or New File.
    pub(in crate::workspace) fn outline_section(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.sidebar != Sidebar::Files {
            return None;
        }
        let p = palette(cx.theme().mode);
        let open = !self.ide.outline_collapsed;
        let header = div().flex_shrink_0().px_1().py(px(2.)).child(
            Button::new("outline-toggle")
                .ghost()
                .xsmall()
                .w_full()
                .icon(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .accessibility_label(if open {
                    "Collapse Outline"
                } else {
                    "Expand Outline"
                })
                // The button centres its content; a label that fills the rest of the row
                // keeps the chevron at the left edge.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .font_semibold()
                        .text_color(p.secondary_foreground)
                        .child("OUTLINE"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ide.outline_collapsed = !this.ide.outline_collapsed;
                    cx.notify();
                })),
        );
        let body = if open { self.outline_body(cx) } else { None };
        Some(
            div()
                .v_flex()
                .flex_shrink_0()
                .min_w_0()
                .max_h(relative(0.45))
                .border_t_1()
                .border_color(cx.theme().border)
                .child(header)
                .children(body)
                .into_any_element(),
        )
    }

    /// What the expanded Outline section lists: the active file's symbols once the
    /// language server has answered for it, or why there are none. Nothing without an
    /// open file or while the server's answer is still to come.
    fn outline_body(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.active.and_then(|index| self.tabs.get(index))?;
        let p = palette(cx.theme().mode);
        let message = |text: &'static str| {
            div()
                .px_3()
                .pb_2()
                .text_size(px(12.))
                .text_color(p.secondary_foreground)
                .child(text)
                .into_any_element()
        };
        if self.language_manager().is_none() {
            return Some(message("Turn on language tools to see the outline."));
        }
        if self.dev.outline_instance != tab.instance {
            return None;
        }
        if self.dev.outline.is_empty() {
            return Some(message("No symbols in this file."));
        }
        let face = &self.look.fonts.explorer;
        let cursor_line = tab.editor.read(cx).cursor_position().line;
        let selected = current_symbol(&self.dev.outline, cursor_line);
        let rows = self
            .dev
            .outline
            .iter()
            .enumerate()
            .map(|(index, symbol)| {
                let line = symbol.line;
                div()
                    .min_w_0()
                    .pl(px(symbol.depth as f32 * INDENT))
                    .child(
                        navigation_button(
                            tree_button(
                                Button::new(("outline", index)),
                                vec![Icon::new(symbol_icon(symbol.kind))],
                                symbol.name.clone(),
                                face.family.clone(),
                                face.size,
                            ),
                            selected == Some(index),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.go_to_symbol(line, window, cx),
                        )),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .id("outline-list")
                .v_flex()
                .min_w_0()
                .min_h_0()
                .vertical_only()
                .px_1()
                .pb_1()
                .children(rows)
                .into_any_element(),
        )
    }

    /// Moves the cursor to the start of `line` (0-based) of the active file, remembering
    /// where it was for Go Back, and gives the editor the keyboard focus.
    fn go_to_symbol(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.record_jump(cx);
        self.go_to_line(Some(line as usize + 1), window, cx);
        if let Some(tab) = self.active.and_then(|index| self.tabs.get(index)) {
            tab.editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
    }

    /// Shows the Explorer with its Outline section expanded (Show Outline).
    pub(in crate::workspace) fn show_outline(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ide.outline_collapsed = false;
        self.set_sidebar(Sidebar::Files, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::current_symbol;
    use crate::workspace::breadcrumbs::{OutlineSymbol, outline};
    use serde_json::{Value, json};

    fn range(start: u32, end: u32) -> Value {
        json!({"start": {"line": start, "character": 0}, "end": {"line": end, "character": 1}})
    }

    /// `trait Shape` (lines 0-2) with `area` (1), `struct Square` (4-6), `impl Shape for
    /// Square` (8-12) with `area` (9-11), and `total_area` (14-20).
    fn symbols() -> Vec<OutlineSymbol> {
        outline(&json!([
            {"name": "Shape", "kind": 11, "range": range(0, 2),
             "children": [{"name": "area", "kind": 6, "range": range(1, 1)}]},
            {"name": "Square", "kind": 23, "range": range(4, 6)},
            {"name": "impl Shape for Square", "kind": 19, "range": range(8, 12),
             "children": [{"name": "area", "kind": 6, "range": range(9, 11)}]},
            {"name": "total_area", "kind": 12, "range": range(14, 20)}
        ]))
    }

    #[test]
    fn current_symbol_is_the_innermost_symbol_around_the_line() {
        let symbols = symbols();
        assert_eq!(current_symbol(&symbols, 1), Some(1));
        assert_eq!(current_symbol(&symbols, 0), Some(0));
        assert_eq!(current_symbol(&symbols, 5), Some(2));
        assert_eq!(current_symbol(&symbols, 8), Some(3));
        assert_eq!(current_symbol(&symbols, 10), Some(4));
        assert_eq!(current_symbol(&symbols, 12), Some(3));
        assert_eq!(current_symbol(&symbols, 20), Some(5));
    }

    #[test]
    fn current_symbol_is_none_outside_every_symbol() {
        let symbols = symbols();
        assert_eq!(current_symbol(&symbols, 3), None);
        assert_eq!(current_symbol(&symbols, 13), None);
        assert_eq!(current_symbol(&symbols, 21), None);
        assert_eq!(current_symbol(&[], 0), None);
    }

    #[test]
    fn current_symbol_takes_the_later_of_two_symbols_on_the_same_lines() {
        // The server lists a parent before its children, so the later one is inside.
        let symbols = outline(&json!([
            {"name": "outer", "range": range(3, 3),
             "children": [{"name": "inner", "range": range(3, 3)}]}
        ]));
        assert_eq!(current_symbol(&symbols, 3), Some(1));
    }
}
