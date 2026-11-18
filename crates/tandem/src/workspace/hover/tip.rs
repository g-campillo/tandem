//! Tandem's own tooltip over the editor: one card, anchored to the text under the pointer.
//! For a symbol it shows the highlighted signature above the documentation; for a
//! diagnostic it shows the message in a block of the severity's colours. When the pointer
//! is on both, the card shows both.

use super::content::{HoverContent, ProblemContent, Tone};
use super::hit;
use crate::appearance::{RADIUS_CARD, RADIUS_CONTROL, palette};
use crate::scrolling::VerticalOnly as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::*;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

/// Space between the text and the card; the pointer may cross it.
const GAP: f32 = 4.;
/// The card keeps this far from the window's edges.
const EDGE: f32 = 8.;
const MAX_WIDTH: f32 = 560.;
/// The tallest the scrolling part of the card gets.
const MAX_HEIGHT: f32 = 360.;
/// Height kept free for a diagnostic's block beside the scrolling part.
const PROBLEM_ROOM: f32 = 120.;

pub struct HoverTip {
    shown: Option<Shown>,
    /// Counts parts and cards, so each one starts with fresh text and scroll state.
    generation: usize,
    _keys: Subscription,
}

/// The open card.
struct Shown {
    editor: WeakEntity<EditorState>,
    /// The text the card is attached to, and the character under the pointer when it
    /// opened. They are fixed then, so the card does not move while a part is added or
    /// removed.
    anchor: Range<usize>,
    anchor_hit: Range<usize>,
    /// Whether a diagnostic opened the card. Such a card is placed where there is more
    /// room, with the diagnostic next to the text; a card a symbol opened goes above it.
    by_problem: bool,
    hover: Option<Part<HoverContent>>,
    problem: Option<Part<ProblemContent>>,
    /// Where the card was last painted.
    card: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// One part of the card and the text it describes.
struct Part<T> {
    /// The symbol, or the piece of a diagnostic on one line.
    range: Range<usize>,
    /// The character the pointer was on when the part was added.
    hit: Range<usize>,
    content: T,
    generation: usize,
}

/// Where the card goes and where the text of its parts is, in window coordinates.
#[derive(Clone, Copy)]
struct Place {
    anchor: Bounds<Pixels>,
    hover: Option<Bounds<Pixels>>,
    problem: Option<Bounds<Pixels>>,
}

impl Shown {
    /// Drops every part whose text the pointer has left, unless the pointer is on the
    /// card, and says where the rest goes. `None` when nothing is left to show or the
    /// text is no longer laid out.
    fn place(&mut self, pointer: Point<Pixels>, cx: &App) -> Option<Place> {
        let editor = self.editor.upgrade()?;
        let state = editor.read(cx);
        let bounds_of = |range: &Range<usize>| state.range_to_bounds(range);
        let anchor = hit::anchor_bounds(&self.anchor, &self.anchor_hit, bounds_of)?;
        let card = self.card.get();
        let kept = |range: &Range<usize>, hit: &Range<usize>| {
            hit::anchor_bounds(range, hit, bounds_of)
                .filter(|text| hit::keeps_open(pointer, *text, card, px(GAP)))
        };
        let hover = self
            .hover
            .as_ref()
            .and_then(|part| kept(&part.range, &part.hit));
        let problem = self
            .problem
            .as_ref()
            .and_then(|part| kept(&part.range, &part.hit));
        if hover.is_none() {
            self.hover = None;
        }
        if problem.is_none() {
            self.problem = None;
        }
        (hover.is_some() || problem.is_some()).then_some(Place {
            anchor,
            hover,
            problem,
        })
    }
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
            _keys: keys,
        }
    }

    /// Shows a symbol's hover. It joins the card of a diagnostic that is open in the same
    /// editor; otherwise it opens a card of its own.
    pub fn show(
        &mut self,
        editor: WeakEntity<EditorState>,
        range: Range<usize>,
        hit: Range<usize>,
        content: HoverContent,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        let generation = self.generation;
        let part = Part {
            range: range.clone(),
            hit: hit.clone(),
            content,
            generation,
        };
        match self.shown.as_mut().filter(|shown| {
            shown.editor.entity_id() == editor.entity_id() && shown.problem.is_some()
        }) {
            Some(shown) => shown.hover = Some(part),
            None => {
                self.shown = Some(Shown {
                    editor,
                    anchor: range,
                    anchor_hit: hit,
                    by_problem: false,
                    hover: Some(part),
                    problem: None,
                    card: Rc::default(),
                })
            }
        }
        cx.notify();
    }

    /// Shows a diagnostic for the piece of its text the pointer is on. It joins the card
    /// that is open in the same editor; otherwise it opens a card of its own.
    pub fn show_problem(
        &mut self,
        editor: WeakEntity<EditorState>,
        piece: Range<usize>,
        content: ProblemContent,
        cx: &mut Context<Self>,
    ) {
        let same_editor = |shown: &&mut Shown| shown.editor.entity_id() == editor.entity_id();
        let shown = self.shown.as_mut().filter(same_editor);
        // The toolkit reports the diagnostic on every pointer move; nothing changes then.
        if shown.as_ref().is_some_and(|shown| {
            shown
                .problem
                .as_ref()
                .is_some_and(|part| part.range == piece && part.content == content)
        }) {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let part = Part {
            range: piece.clone(),
            hit: piece.clone(),
            content,
            generation,
        };
        match shown {
            Some(shown) => shown.problem = Some(part),
            None => {
                self.shown = Some(Shown {
                    editor,
                    anchor: piece.clone(),
                    anchor_hit: piece,
                    by_problem: true,
                    hover: None,
                    problem: Some(part),
                    card: Rc::default(),
                })
            }
        }
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.shown.take().is_some() {
            cx.notify();
        }
    }

    /// Removes the parts whose text the pointer has left; the card closes with its last
    /// part.
    fn leave(&mut self, hover: bool, problem: bool, cx: &mut Context<Self>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        if hover {
            shown.hover = None;
        }
        if problem {
            shown.problem = None;
        }
        if shown.hover.is_none() && shown.problem.is_none() {
            self.shown = None;
        }
        cx.notify();
    }

    /// Whether the card for `editor` already describes the symbol at `hit`.
    pub fn covers(&self, editor: EntityId, hit: &Range<usize>) -> bool {
        self.shown.as_ref().is_some_and(|shown| {
            shown.editor.entity_id() == editor
                && shown
                    .hover
                    .as_ref()
                    .is_some_and(|part| part.range.start <= hit.start && hit.end <= part.range.end)
        })
    }
}

impl Render for HoverTip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pointer = window.mouse_position();
        let place = self
            .shown
            .as_mut()
            .and_then(|shown| shown.place(pointer, cx));
        // The text scrolled away, or the pointer is no longer on it or on the card.
        let (Some(shown), Some(place)) = (self.shown.as_ref(), place) else {
            self.shown = None;
            return Empty.into_any_element();
        };

        let p = palette(cx.theme().mode);
        let viewport = window.viewport_size();
        let max_width = px(MAX_WIDTH)
            .min(viewport.width - px(EDGE * 2.))
            .max(px(160.));
        // A card that a diagnostic opened goes on the side of the text with more room, and
        // its scrolling part is limited to that room. It then never has to change sides,
        // and the diagnostic stays where it is when the symbol's part arrives.
        let room_above = place.anchor.top() - px(EDGE + GAP);
        let room_below = viewport.height - place.anchor.bottom() - px(EDGE + GAP);
        let below = shown.by_problem && room_below >= room_above;
        let room = if shown.by_problem {
            room_above.max(room_below) - px(PROBLEM_ROOM)
        } else {
            viewport.height - px(EDGE * 2.)
        };
        let max_height = px(MAX_HEIGHT).min(room).max(px(80.));
        // How Markdown is set in every part of the card.
        let style = |code_block: StyleRefinement| {
            let mut style = TextViewStyle::default()
                .paragraph_gap(rems(0.5))
                .heading_font_size(|_, base| base)
                .code_block(code_block.text_size(px(12.)));
            // The editor's highlight theme, so code has the editor's colours.
            style.highlight_theme = cx.theme().highlight_theme.clone();
            style
        };
        // A code block inside prose: a well of the editor's background.
        let well = || {
            StyleRefinement::default()
                .bg(p.content)
                .p_2()
                .rounded(px(RADIUS_CONTROL))
        };
        // A diagnostic sits in a block of its severity's colours, as Tandem's notices do.
        let problem = shown.problem.as_ref().map(|part| {
            let (surface, text) = match part.content.tone {
                Tone::Error => (p.error_surface, p.error),
                Tone::Warning => (p.warning_surface, p.warning),
                Tone::Note => (p.selected, p.selected_foreground),
            };
            div()
                .flex_shrink_0()
                .px(px(10.))
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(surface)
                .text_color(text)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(part.content.title.clone()),
                )
                .children((!part.content.message.is_empty()).then(|| {
                    TextView::markdown(
                        ("hover-tip-problem", part.generation),
                        part.content.message.clone(),
                    )
                    .style(style(well()))
                    .selectable(true)
                }))
        });
        // A symbol's signature sits in a well of the editor's background, which the
        // highlight colours are chosen for; its documentation sits on the popover surface.
        let hover = shown.hover.as_ref();
        let signature = hover
            .filter(|part| !part.content.signature.is_empty())
            .map(|part| {
                div()
                    .flex_shrink_0()
                    .px(px(10.))
                    .py_2()
                    .rounded(px(RADIUS_CONTROL))
                    .bg(p.content)
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(
                        TextView::markdown(
                            ("hover-tip-signature", part.generation),
                            part.content.signature_markdown(),
                        )
                        .style(style(
                            StyleRefinement::default()
                                .bg(Hsla::transparent_black())
                                .p_0(),
                        ))
                        .selectable(true),
                    )
            });
        let documentation = hover
            .filter(|part| !part.content.documentation.is_empty())
            .map(|part| {
                div().flex_shrink_0().px(px(2.)).child(
                    TextView::markdown(
                        ("hover-tip-documentation", part.generation),
                        part.content.documentation_markdown(),
                    )
                    .style(style(well()))
                    .selectable(true),
                )
            });
        // The symbol's part scrolls; the diagnostic does not, and is next to the text.
        let scroll = hover
            .filter(|_| signature.is_some() || documentation.is_some())
            .map(|part| {
                div()
                    .id(("hover-tip-scroll", part.generation))
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap_2()
                    .max_h(max_height)
                    .vertical_only()
                    .children(signature)
                    .children(documentation)
                    .into_any_element()
            });
        let problem = problem.map(IntoElement::into_any_element);
        let parts = if below {
            [problem, scroll]
        } else {
            [scroll, problem]
        };
        let tip = cx.entity().downgrade();
        let last_card = shown.card.clone();
        let card = div()
            .relative()
            .occlude()
            .max_w(max_width)
            .rounded(px(RADIUS_CARD))
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
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p(px(10.))
                    .children(parts.into_iter().flatten()),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |card, _, window, _| {
                        last_card.set(Some(card));
                        watch(card, place, tip, window);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        let anchored = if below {
            anchored()
                .anchor(Anchor::TopLeft)
                .attach(place.anchor, Anchor::BottomLeft)
                .offset(point(px(0.), px(GAP)))
        } else {
            anchored()
                .anchor(Anchor::BottomLeft)
                .attach(place.anchor, Anchor::TopLeft)
                .offset(point(px(0.), px(-GAP)))
        };
        deferred(anchored.snap_to_window_with_margin(px(EDGE)).child(card))
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element()
    }
}

/// Closes the card, or the part the pointer has left, when the pointer moves off the text
/// and the card; closes it on a press or a scroll outside the card and when the pointer
/// leaves the window. Registered while the card is painted, so it lasts exactly as long as
/// the card is on screen, wherever in the window the pointer goes.
fn watch(card: Bounds<Pixels>, place: Place, tip: WeakEntity<HoverTip>, window: &mut Window) {
    fn close(tip: &WeakEntity<HoverTip>, cx: &mut App) {
        tip.update(cx, |tip, cx| tip.hide(cx)).ok();
    }
    window.on_mouse_event({
        let tip = tip.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            // A drag that started in the card is a text selection.
            if !phase.bubble() || event.pressed_button.is_some() {
                return;
            }
            let left = |text: Option<Bounds<Pixels>>| {
                text.is_some_and(|text| !hit::keeps_open(event.position, text, Some(card), px(GAP)))
            };
            let (hover, problem) = (left(place.hover), left(place.problem));
            if hover || problem {
                tip.update(cx, |tip, cx| tip.leave(hover, problem, cx)).ok();
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
