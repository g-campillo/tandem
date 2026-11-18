//! The file tree and chat panes beside the editor: which side each is on, how wide it is,
//! the dividers that resize them, and saving that layout.
use super::pane_logic::{ShownPanes, drag_pane};
use super::*;
use crate::appearance::{PANEL_INSET, glass_panel};
use gpui_kit::component::resizable::HandleEdge;
use gpui_kit::component::*;
use std::rc::Rc;
use tandem_core::layout::{Pane, PaneLayout, PaneSide};

actions!(tandem, [SwapPanes]);

pub(super) struct PaneUi {
    /// The layout on screen.
    pub layout: PaneLayout,
    /// The layout the store holds, so only a change is written.
    saved: PaneLayout,
}

impl PaneUi {
    /// The saved layout, or the default one when none was saved or it cannot be read.
    pub fn load(store: &Store) -> Self {
        let layout = store.pane_layout().unwrap_or_default();
        Self {
            layout,
            saved: layout,
        }
    }
}

/// What a divider drag carries: the pane it resizes. It draws nothing, because the divider
/// itself shows the drag.
#[derive(Clone)]
pub(super) struct PaneResize(Pane);

impl Render for PaneResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl Workspace {
    /// Which panes are on screen. Anything that shows or hides a pane changes this, and the
    /// widths and the row of panes follow.
    pub(super) fn shown_panes(&self) -> ShownPanes {
        ShownPanes {
            // ⌘B hides and shows the file tree.
            files: self.commands.explorer_visible,
            chat: self.chat.visible,
        }
    }

    /// Moves the file tree and the chat to each other's side.
    pub(super) fn swap_panes(&mut self, cx: &mut Context<Self>) {
        self.panes.layout.swapped = !self.panes.layout.swapped;
        self.persist_layout(cx);
        cx.notify();
    }

    /// Follows a divider drag: the pane's edge moves with the pointer.
    pub(super) fn resize_pane(
        &mut self,
        event: &DragMoveEvent<PaneResize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = event.drag(cx).0;
        let next = drag_pane(
            &self.panes.layout,
            pane,
            f32::from(event.event.position.x),
            f32::from(event.bounds.left()),
            f32::from(event.bounds.right()),
            self.shown_panes(),
        );
        if next != self.panes.layout {
            self.panes.layout = next;
            cx.notify();
        }
    }

    /// Returns `pane` to the window's default width.
    fn reset_pane_width(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.panes.layout.set_width(pane, None);
        self.persist_layout(cx);
        cx.notify();
    }

    /// Runs from the workspace's 500 ms loop: saves a resized layout once the drag is over.
    /// A drag changes the layout on every pointer move, so it does not save by itself.
    pub(super) fn pane_tick(&mut self, cx: &mut Context<Self>) {
        if !cx.has_active_drag() {
            self.persist_layout(cx);
        }
    }

    /// Writes the layout to the store when it differs from what the store holds. A failure
    /// is reported once and not retried until the layout changes again.
    pub(super) fn persist_layout(&mut self, cx: &mut Context<Self>) {
        let layout = self.panes.layout;
        if layout == self.panes.saved {
            return;
        }
        self.panes.saved = layout;
        let result = self
            .store
            .lock()
            .map_err(|_| anyhow!("Recovery store unavailable"))
            .and_then(|mut store| store.save_pane_layout(&layout));
        if let Err(error) = result {
            self.message(
                format!("Could not save the pane layout: {error:#}"),
                true,
                cx,
            );
        }
    }

    /// Sets a pane's content in a glass panel `width` wide, inset from the window's edges and
    /// the editor, with the divider on the edge that faces the editor. Dragging the divider
    /// resizes the pane; a double click returns it to its default width. The divider draws
    /// no line: the gap between the panels already shows the seam, and a full-height line
    /// would run past their rounded corners.
    pub(super) fn pane_frame(
        &self,
        pane: Pane,
        width: f32,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match pane {
            Pane::Files => "files-divider",
            Pane::Chat => "chat-divider",
        };
        let edge = match self.panes.layout.side(pane) {
            PaneSide::Left => HandleEdge::Trailing,
            PaneSide::Right => HandleEdge::Leading,
        };
        div()
            .relative()
            .h_flex()
            .items_stretch()
            .min_h_0()
            .flex_shrink_0()
            .w(px(width))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .px(px(PANEL_INSET))
                    .pb(px(PANEL_INSET))
                    .child(glass_panel(cx).flex_1().child(content)),
            )
            .child(
                resize_handle(id, Axis::Horizontal)
                    .inside(edge)
                    .with_appearance(Rc::new(|_, _, _| Some(Empty.into_any_element())))
                    .on_drag(PaneResize(pane), |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_double_click(
                        cx.listener(move |this, _, _, cx| this.reset_pane_width(pane, cx)),
                    ),
            )
            .into_any_element()
    }
}
