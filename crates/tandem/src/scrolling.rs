//! Scroll areas that keep the two scroll directions apart. A horizontal wheel or swipe
//! scrolls only what scrolls sideways, and a vertical one only what scrolls up and down;
//! neither is turned into the other. GPUI and the toolkit both do turn them by default:
//! an area that scrolls one way takes the other axis' delta as its own, so a vertical
//! wheel over a wide code block scrolled it sideways while the chat around it scrolled
//! down.
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    ScrollHandle, StatefulInteractiveElement, StyleRefinement, Styled, Window, canvas, div, px,
};

/// Content that scrolls sideways only, with a scroll bar along its bottom edge whenever
/// the content is wider than the area. Style it like the `div` it replaces: the styles
/// apply to the area's outer box, and the children are laid out in a row that grows with
/// them.
#[derive(IntoElement)]
pub struct SideScroll {
    id: ElementId,
    style: StyleRefinement,
    children: Vec<AnyElement>,
    /// Keep a strip under wide content for the bar, instead of drawing it over the content.
    strip: bool,
}

/// A [`SideScroll`] with `id`, which keeps its scroll position between frames.
pub fn side_scroll(id: impl Into<ElementId>) -> SideScroll {
    SideScroll {
        id: id.into(),
        style: StyleRefinement::default(),
        children: Vec::new(),
        strip: true,
    }
}

impl SideScroll {
    /// Draws the bar over the bottom of the content, for a row that has no room to grow,
    /// like the tab bar.
    pub fn bar_over_content(mut self) -> Self {
        self.strip = false;
        self
    }
}

impl Styled for SideScroll {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for SideScroll {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// The strip kept under wide content for the scroll bar's thumb, so the thumb does not
/// cover the last line: the toolkit draws it 4 px from the edge, up to 8 px thick.
const BAR_SPACE: f32 = 12.;

impl RenderOnce for SideScroll {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = window
            .use_keyed_state(self.id.clone(), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        // Whether the content was wider than the area when last laid out. The bar and its
        // strip follow it (the toolkit's always-shown bar shows even when nothing
        // overflows); the canvas below asks for another frame when it changes.
        let wide = handle.max_offset().x > px(0.);
        let area = div()
            .id(self.id.clone())
            .track_scroll(&handle)
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .when(wide && self.strip, |area| area.pb(px(BAR_SPACE)))
            .children(self.children);
        let watched = handle.clone();
        let watch = canvas(
            move |_, window, _| {
                if (watched.max_offset().x > px(0.)) != wide {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0();
        div().flex().min_w_0().refine_style(&self.style).child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(area)
                .child(watch)
                .when(wide, |wrapper| {
                    wrapper.child(Scrollbar::horizontal(&handle).mode(ScrollbarMode::Always))
                }),
        )
    }
}

/// Keeps a vertical scroll area from taking a horizontal wheel or swipe as its own.
pub trait VerticalOnly: StatefulInteractiveElement + Sized {
    fn vertical_only(self) -> Self {
        self.overflow_y_scroll().restrict_scroll_to_axis()
    }
}

impl<E: StatefulInteractiveElement + Sized> VerticalOnly for E {}
