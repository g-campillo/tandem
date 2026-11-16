//! Tandem's own hover tooltip: a card with the symbol's highlighted signature above its
//! documentation, anchored to the symbol and drawn over the editor.

use super::content::HoverContent;
use super::hit;
use crate::appearance::palette;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::*;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

/// Space between the symbol and the card; the pointer may cross it.
const GAP: f32 = 4.;
/// The card keeps this far from the window's edges.
const EDGE: f32 = 8.;
const MAX_WIDTH: f32 = 560.;
const MAX_HEIGHT: f32 = 360.;

pub struct HoverTip {
    shown: Option<Shown>,
    /// Counts tooltips, so each one starts with fresh text and scroll state.
    generation: usize,
    /// Where the card was last painted.
    card: Rc<Cell<Option<Bounds<Pixels>>>>,
    _keys: Subscription,
}

#[derive(Clone)]
struct Shown {
    editor: WeakEntity<EditorState>,
    /// The symbol the tooltip describes.
    range: Range<usize>,
    /// The character the pointer was on when it opened.
    hit: Range<usize>,
    content: HoverContent,
}

impl HoverTip {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let keys = cx.observe_keystrokes(|tip, event, _, cx| {
            if hit::closes_on_key(event.keystroke.modifiers.platform, &event.keystroke.key) {
                tip.hide(cx);
            }
        });
        Self {
            shown: None,
            generation: 0,
            card: Rc::new(Cell::new(None)),
            _keys: keys,
        }
    }

    pub fn show(
        &mut self,
        editor: WeakEntity<EditorState>,
        range: Range<usize>,
        hit: Range<usize>,
        content: HoverContent,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        self.card.set(None);
        self.shown = Some(Shown {
            editor,
            range,
            hit,
            content,
        });
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.shown.take().is_some() {
            cx.notify();
        }
    }

    /// Whether the tooltip for `editor` already describes the character at `hit`.
    pub fn covers(&self, editor: EntityId, hit: &Range<usize>) -> bool {
        self.shown.as_ref().is_some_and(|shown| {
            shown.editor.entity_id() == editor
                && shown.range.start <= hit.start
                && hit.end <= shown.range.end
        })
    }
}

impl Render for HoverTip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = self.shown.clone() else {
            return Empty.into_any_element();
        };
        let placed = shown.editor.upgrade().and_then(|editor| {
            let state = editor.read(cx);
            let symbol = hit::anchor_bounds(&shown.range, &shown.hit, |range| {
                state.range_to_bounds(range)
            })?;
            Some((symbol, state.diagnostic_popover().is_some()))
        });
        // The symbol scrolled away, or the pointer is no longer on it or on the card.
        let Some((symbol, below)) = placed.filter(|(symbol, _)| {
            hit::keeps_open(window.mouse_position(), *symbol, self.card.get(), px(GAP))
        }) else {
            self.shown = None;
            return Empty.into_any_element();
        };

        let p = palette(cx.theme().mode);
        let viewport = window.viewport_size();
        let max_width = px(MAX_WIDTH)
            .min(viewport.width - px(EDGE * 2.))
            .max(px(160.));
        let max_height = px(MAX_HEIGHT)
            .min(viewport.height - px(EDGE * 2.))
            .max(px(80.));
        let style = |code_block: StyleRefinement| {
            let mut style = TextViewStyle::default()
                .paragraph_gap(rems(0.5))
                .heading_font_size(|_, base| base)
                .code_block(code_block.text_size(px(12.)));
            // The editor's highlight theme, so code has the editor's colours.
            style.highlight_theme = cx.theme().highlight_theme.clone();
            style
        };
        // Code sits in wells of the editor's background, which the highlight colours are
        // chosen for; prose sits on the popover surface.
        let signature = (!shown.content.signature.is_empty()).then(|| {
            div()
                .flex_shrink_0()
                .px(px(10.))
                .py_2()
                .rounded(px(6.))
                .bg(p.content)
                .font_family(cx.theme().mono_font_family.clone())
                .child(
                    TextView::markdown(
                        ("hover-tip-signature", self.generation),
                        shown.content.signature_markdown(),
                    )
                    .style(style(
                        StyleRefinement::default()
                            .bg(Hsla::transparent_black())
                            .p_0(),
                    ))
                    .selectable(true),
                )
        });
        let documentation = (!shown.content.documentation.is_empty()).then(|| {
            div().flex_shrink_0().px(px(2.)).child(
                TextView::markdown(
                    ("hover-tip-documentation", self.generation),
                    shown.content.documentation_markdown(),
                )
                .style(style(
                    StyleRefinement::default()
                        .bg(p.content)
                        .p_2()
                        .rounded(px(6.)),
                ))
                .selectable(true),
            )
        });
        let tip = cx.entity().downgrade();
        let last_card = self.card.clone();
        let card = div()
            .relative()
            .occlude()
            .max_w(max_width)
            .rounded(px(10.))
            .border_1()
            .border_color(p.border)
            .bg(p.surface)
            .shadow_lg()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.))
            .line_height(px(19.))
            .text_color(p.foreground)
            .child(
                div()
                    .id(("hover-tip-scroll", self.generation))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p(px(10.))
                    .max_h(max_height)
                    .overflow_y_scroll()
                    .children(signature)
                    .children(documentation),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |card, _, window, _| {
                        last_card.set(Some(card));
                        watch(card, symbol, tip, window);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        // Above the symbol, or below it while the toolkit's diagnostic popover is above.
        let anchored = if below {
            anchored()
                .anchor(Anchor::TopLeft)
                .attach(symbol, Anchor::BottomLeft)
                .offset(point(px(0.), px(GAP)))
        } else {
            anchored()
                .anchor(Anchor::BottomLeft)
                .attach(symbol, Anchor::TopLeft)
                .offset(point(px(0.), px(-GAP)))
        };
        deferred(anchored.snap_to_window_with_margin(px(EDGE)).child(card))
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element()
    }
}

/// Closes the tooltip when the pointer leaves the symbol and the card, on a press or a
/// scroll outside the card, and when the pointer leaves the window. Registered while the
/// card is painted, so it lasts exactly as long as the card is on screen.
fn watch(
    card: Bounds<Pixels>,
    symbol: Bounds<Pixels>,
    tip: WeakEntity<HoverTip>,
    window: &mut Window,
) {
    fn close(tip: &WeakEntity<HoverTip>, cx: &mut App) {
        tip.update(cx, |tip, cx| tip.hide(cx)).ok();
    }
    window.on_mouse_event({
        let tip = tip.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            // A drag that started in the card is a text selection.
            if phase.bubble()
                && event.pressed_button.is_none()
                && !hit::keeps_open(event.position, symbol, Some(card), px(GAP))
            {
                close(&tip, cx);
            }
        }
    });
    window.on_mouse_event({
        let tip = tip.clone();
        move |event: &MouseDownEvent, phase, _, cx| {
            if phase.bubble() && !card.contains(&event.position) {
                close(&tip, cx);
            }
        }
    });
    window.on_mouse_event({
        let tip = tip.clone();
        move |event: &ScrollEvent, phase, _, cx| {
            if phase.bubble() && !card.contains(&event.position) {
                close(&tip, cx);
            }
        }
    });
    window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
        if phase.bubble() {
            close(&tip, cx);
        }
    });
}
