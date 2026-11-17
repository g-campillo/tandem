//! The completion list on screen: a card of rows hanging below the word being completed,
//! with the selected entry's details beside it.
use super::Workspace;
use super::complete::{CompletionState, ROW_HEIGHT, VISIBLE_ROWS};
use super::icons::completion_icon;
use crate::appearance::{RADIUS_CONTROL, palette};
use crate::workspace::fonts::Face;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types::{CompletionItem, Documentation, MarkupKind};
use tandem_lsp::features::completion::Source;

const LIST_WIDTH: f32 = 440.;
const DETAILS_WIDTH: f32 = 360.;
const DETAILS_MAX_HEIGHT: f32 = 300.;
/// Space between the word and the list, and between the list and its details.
const GAP: f32 = 2.;
/// The popup keeps this far from the window's edges.
const EDGE: f32 = 8.;

/// The list over `editor` below its word, or `None` while that word is not laid out. The
/// window is at least 900 px wide, so the list and its details always fit beside each
/// other.
pub(super) fn completion_list(
    list: &CompletionState,
    editor: &Entity<EditorState>,
    face: &Face,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let word = editor
        .read(cx)
        .range_to_bounds(&(list.word_start..list.word_start))?;
    let p = palette(cx.theme().mode);
    let mono = cx.theme().mono_font_family.clone();
    let rows = list
        .ranked
        .iter()
        .enumerate()
        .skip(list.top)
        .take(VISIBLE_ROWS)
        .map(|(index, row)| {
            let candidate = &list.candidates[row.candidate];
            let selected = index == list.selected;
            let (text, muted) = if selected {
                (p.selected_foreground, p.selected_foreground.opacity(0.75))
            } else {
                (p.foreground, p.secondary_foreground)
            };
            let matched = HighlightStyle {
                color: Some(p.accent),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..HighlightStyle::default()
            };
            let label = StyledText::new(candidate.label.clone())
                .with_highlights(row.matches.iter().map(|range| (range.clone(), matched)));
            div()
                .id(("completion-row", index))
                .h(px(ROW_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .rounded(px(4.))
                .text_color(text)
                .when(selected, |row| row.bg(p.selected))
                .child(
                    Icon::new(completion_icon(candidate.kind))
                        .with_size(px(14.))
                        .text_color(if selected {
                            p.selected_foreground
                        } else {
                            p.secondary_foreground
                        }),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(LIST_WIDTH * 0.6))
                        .truncate()
                        .font_family(mono.clone())
                        .font_features(face.features.clone())
                        .when(candidate.deprecated, |label| label.line_through())
                        .child(label),
                )
                .children(candidate.label_detail.clone().map(|detail| {
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(mono.clone())
                        .text_color(muted)
                        .child(detail)
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_right()
                        .text_color(muted)
                        .children(candidate.description.clone()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        view.accept_completion(index, window, cx);
                    }),
                )
        });
    let card = div()
        .id(("completion-list", list.serial))
        .occlude()
        .w(px(LIST_WIDTH))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .p(px(4.))
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .shadow_lg()
        .text_size(px(13.))
        .font_family(cx.theme().font_family.clone())
        .children(rows)
        .on_scroll(cx.listener(|view, event: &ScrollEvent, _, cx| {
            cx.stop_propagation();
            view.scroll_completion(event.delta.pixel_delta(px(ROW_HEIGHT)).y, cx);
        }));
    let selected = list
        .ranked
        .get(list.selected)
        .map(|row| (row.candidate, &list.candidates[row.candidate]));
    let details = selected.and_then(|(candidate, entry)| match entry.source {
        Source::Server(item) => list
            .item(item)
            .and_then(|item| details(item, list.serial, candidate, cx)),
        _ => None,
    });
    let popup = div()
        .flex()
        .items_start()
        .gap(px(GAP * 2.))
        .child(card)
        .children(details);
    Some(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .attach(word, Anchor::BottomLeft)
                .offset(point(px(0.), px(GAP)))
                .snap_to_window_with_margin(px(EDGE))
                .child(popup),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element(),
    )
}

/// The details of a server entry beside the list, when it has any: its `detail` in a well
/// of the editor's background and its documentation below, styled like the hover card.
fn details(item: &CompletionItem, serial: u64, candidate: usize, cx: &App) -> Option<AnyElement> {
    if item.detail.is_none() && item.documentation.is_none() {
        return None;
    }
    let p = palette(cx.theme().mode);
    let mut style = TextViewStyle::default()
        .paragraph_gap(rems(0.5))
        .heading_font_size(|_, base| base)
        .code_block(
            StyleRefinement::default()
                .bg(p.content)
                .p_2()
                .rounded(px(RADIUS_CONTROL))
                .text_size(px(12.)),
        );
    // The editor's highlight theme, so code has the editor's colours.
    style.highlight_theme = cx.theme().highlight_theme.clone();
    let id = |part: &str| SharedString::from(format!("completion-{part}-{serial}-{candidate}"));
    let detail = item.detail.clone().map(|detail| {
        div()
            .flex_shrink_0()
            .px(px(10.))
            .py_2()
            .rounded(px(RADIUS_CONTROL))
            .bg(p.content)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(detail)
    });
    let documentation = item.documentation.as_ref().map(|documentation| {
        TextView::markdown(id("documentation"), documentation_markdown(documentation))
            .style(style)
            .selectable(true)
    });
    Some(
        div()
            .id(id("details"))
            .occlude()
            .w(px(DETAILS_WIDTH))
            .max_h(px(DETAILS_MAX_HEIGHT))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p(px(10.))
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(p.border)
            .bg(p.surface)
            .shadow_lg()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.))
            .line_height(px(19.))
            .text_color(p.foreground)
            .children(detail)
            .children(documentation)
            .into_any_element(),
    )
}

/// An entry's documentation as Markdown: Markdown as it is, plain text in a code block so
/// none of it is read as Markdown.
fn documentation_markdown(documentation: &Documentation) -> String {
    let plain = match documentation {
        Documentation::MarkupContent(markup) if markup.kind == MarkupKind::Markdown => {
            return markup.value.clone();
        }
        Documentation::MarkupContent(markup) => &markup.value,
        Documentation::String(text) => text,
    };
    // A fence longer than any run of backticks in the text, so none of them closes it.
    let longest = plain
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{}\n{fence}", plain.trim_end_matches('\n'))
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::documentation_markdown;
    use lsp_types::{Documentation, MarkupContent, MarkupKind};

    #[test]
    fn markdown_documentation_is_shown_as_it_is() {
        let documentation = Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "Adds **two** numbers.\n\n```rust\nadd(1, 2)\n```".into(),
        });
        assert_eq!(
            documentation_markdown(&documentation),
            "Adds **two** numbers.\n\n```rust\nadd(1, 2)\n```"
        );
    }

    #[test]
    fn plain_documentation_is_never_read_as_markdown() {
        let plain = Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::PlainText,
            value: "# not a heading\n*not bold*".into(),
        });
        assert_eq!(
            documentation_markdown(&plain),
            "```\n# not a heading\n*not bold*\n```"
        );
        let string = Documentation::String("Returns ``x``.".into());
        assert_eq!(documentation_markdown(&string), "```\nReturns ``x``.\n```");
        let fenced = Documentation::String("a ```` b".into());
        assert_eq!(documentation_markdown(&fenced), "`````\na ```` b\n`````");
    }
}
