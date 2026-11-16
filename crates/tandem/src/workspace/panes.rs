//! The file tree and chat panes beside the editor: which side each is on, how wide it is,
//! and saving that layout.
use super::pane_logic::ShownPanes;
use super::*;
use gpui_kit::component::*;
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

impl Workspace {
    /// Which panes are on screen. Anything that shows or hides a pane changes this, and the
    /// widths and the row of panes follow.
    pub(super) fn shown_panes(&self) -> ShownPanes {
        ShownPanes {
            // The file tree has no command that hides it.
            files: true,
            chat: self.chat.visible,
        }
    }

    /// Moves the file tree and the chat to each other's side.
    pub(super) fn swap_panes(&mut self, cx: &mut Context<Self>) {
        self.panes.layout.swapped = !self.panes.layout.swapped;
        self.persist_layout(cx);
        cx.notify();
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

    /// Wraps a pane's content with a divider line on the edge that faces the editor.
    pub(super) fn pane_frame(
        &self,
        pane: Pane,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let divider = div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(1.))
            .bg(cx.theme().border);
        div()
            .relative()
            .h_flex()
            .items_stretch()
            .min_h_0()
            .flex_shrink_0()
            .child(content)
            .child(match self.panes.layout.side(pane) {
                PaneSide::Left => divider.right_0(),
                PaneSide::Right => divider.left_0(),
            })
            .into_any_element()
    }
}
