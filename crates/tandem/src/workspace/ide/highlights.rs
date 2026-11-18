//! Highlights of the symbol under the cursor: shortly after the cursor comes to rest on a
//! word, its other uses are tinted in the text and marked on the scrollbar, the writes
//! more strongly than the reads. The language server finds the uses when it offers to; in
//! a file without one, the word's whole-word matches are highlighted instead.
use super::*;
use crate::appearance::palette;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
    OverviewRulerDecoration, OverviewRulerLane, RangeDecoration, RangeDecorationStyle,
};
use std::ops::Range;
use std::time::Duration;
use tandem_core::edit::select::occurrences;
use tandem_core::edit::word_range_at;
use tandem_lsp::features::highlights::document_highlights;

/// How long the cursor rests on a word before its uses are highlighted.
const DELAY: Duration = Duration::from_millis(250);
/// The most whole-word matches highlighted in a file without a language server.
const MAX_TEXT_MATCHES: usize = 500;
/// How strongly reads and plain uses, writes, and the scrollbar marks are tinted with the
/// accent colour.
const READ_FILL: f32 = 0.16;
const WRITE_FILL: f32 = 0.28;
const RULER_MARK: f32 = 0.7;

/// A use of the symbol under the cursor: its byte range, and whether it writes the symbol.
type Highlight = (Range<usize>, bool);

/// A request for the uses of the symbol at the cursor, with the text it is about.
struct HighlightRequest {
    reply: Task<Result<Vec<Highlight>>>,
    text: String,
}

impl Workspace {
    /// After `editor` changed, follows a move of its cursor: highlights the cursor has left
    /// go at once, and the uses of the word under the cursor are looked for once it has
    /// rested there. A selection, or a move in an editor that is not the active tab's on
    /// screen, clears them. A redraw leaves the selection where it was and changes nothing.
    pub(super) fn highlight_moved(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let id = editor.entity_id();
        let selection = editor.read(cx).selected_range();
        let Some(tab) = self.ide.tabs.get_mut(&id) else {
            return;
        };
        if tab.highlight_selection == selection {
            return;
        }
        tab.highlight_selection = selection.clone();
        let drawn = tab
            .highlights
            .as_ref()
            .map(|(ranges, _)| ranges.get_ranges(cx))
            .unwrap_or_default();
        let active = self
            .ide_target()
            .is_some_and(|target| target.editor.entity_id() == id);
        if !active || !selection.is_empty() {
            self.clear_highlights(id, cx);
            return;
        }
        if !touches(&drawn, selection.start) {
            self.clear_highlights(id, cx);
        }
        self.schedule_highlights(id, selection.start, cx);
    }

    /// Clears the highlights of `editor` after an edit there, and takes note of where the
    /// edit left the cursor, so that the move is not taken for one of the user's: typing
    /// does not bring the highlights back.
    pub(super) fn highlights_edited(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let id = editor.entity_id();
        let selection = editor.read(cx).selected_range();
        if let Some(tab) = self.ide.tabs.get_mut(&id) {
            tab.highlight_selection = selection;
        }
        self.clear_highlights(id, cx);
    }

    /// Clears the highlights of `editor` when the editor loses the focus.
    pub(super) fn highlights_blurred(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        self.clear_highlights(editor.entity_id(), cx);
    }

    /// Removes the highlights of `editor` and stops looking for new ones there.
    fn clear_highlights(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        if self
            .ide
            .highlight_task
            .as_ref()
            .is_some_and(|(pending, _)| *pending == editor)
        {
            self.ide.highlight_task = None;
        }
        if let Some((ranges, marks)) = self
            .ide
            .tabs
            .get(&editor)
            .and_then(|tab| tab.highlights.as_ref())
        {
            ranges.clear(cx);
            marks.clear(cx);
        }
    }

    /// Looks for the uses of the word at `cursor` in `editor` after [`DELAY`], replacing a
    /// search still waiting or in flight.
    fn schedule_highlights(&mut self, editor: EntityId, cursor: usize, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |view, cx| {
            smol::Timer::after(DELAY).await;
            let request = view.update(cx, |view, cx| view.highlight_request(editor, cursor, cx));
            let Ok(Some(request)) = request else {
                return;
            };
            let reply = request.reply.await;
            view.update(cx, |view, cx| {
                view.highlights_replied(editor, &request.text, cursor, reply, cx)
            })
            .ok();
        });
        self.ide.highlight_task = Some((editor, task));
    }

    /// The request for the uses of the word at `cursor` in `editor`, when its language
    /// server is ready and finds them. A file without one gets the word's whole-word
    /// matches at once, and a cursor that is not on a word clears the highlights; both
    /// ask nothing. Nothing is done once the cursor has moved or `editor` is no longer the
    /// active tab's on screen.
    fn highlight_request(
        &mut self,
        editor: EntityId,
        cursor: usize,
        cx: &mut Context<Self>,
    ) -> Option<HighlightRequest> {
        let target = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)?;
        let (text, selection) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.selected_range())
        };
        if selection != (cursor..cursor) {
            return None;
        }
        let Some(word) = word_range_at(&text, cursor) else {
            self.clear_highlights(editor, cx);
            return None;
        };
        let manager = target.manager.filter(|manager| {
            manager.health().state == "ready"
                && manager.capabilities().has("documentHighlightProvider")
        });
        let Some(manager) = manager else {
            let found = text_highlights(&text, &text[word]);
            self.draw_highlights(editor, &found, cx);
            return None;
        };
        let Ok(params) = tandem_lsp::position_params(&text, cursor) else {
            self.clear_highlights(editor, cx);
            return None;
        };
        self.sync_language(cx);
        let reply = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, text.clone());
            async move {
                let reply = manager
                    .request(
                        "textDocument/documentHighlight",
                        &file,
                        &instance,
                        &text,
                        params,
                    )
                    .await?;
                anyhow::ensure!(manager.is_current(&reply.stamp), "Stale highlights");
                Ok(document_highlights(&text, &reply.value))
            }
        });
        Some(HighlightRequest { reply, text })
    }

    /// Draws the server's uses of the symbol at `cursor` in `text` while `editor` still
    /// has that text with the cursor there; a stale or failed reply clears the highlights.
    fn highlights_replied(
        &mut self,
        editor: EntityId,
        text: &str,
        cursor: usize,
        reply: Result<Vec<Highlight>>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .ide
            .tabs
            .get(&editor)
            .and_then(|tab| tab.editor.upgrade())
            .is_some_and(|state| {
                let state = state.read(cx);
                state.selected_range() == (cursor..cursor) && *state.text() == text
            });
        match reply {
            Ok(found) if current => self.draw_highlights(editor, &found, cx),
            _ => self.clear_highlights(editor, cx),
        }
    }

    /// Draws `found` in `editor`: each use filled, writes more strongly, and a mark on the
    /// scrollbar for each.
    fn draw_highlights(&mut self, editor: EntityId, found: &[Highlight], cx: &mut Context<Self>) {
        let p = palette(cx.theme().mode);
        let Some(tab) = self.ide.tabs.get_mut(&editor) else {
            return;
        };
        let Some(state) = tab.editor.upgrade() else {
            return;
        };
        let (ranges, marks) = tab.highlights.get_or_insert_with(|| {
            state.update(cx, |state, cx| {
                (
                    state.create_range_decorations_collection(Vec::new(), cx),
                    state.create_overview_ruler_decorations_collection(Vec::new(), cx),
                )
            })
        });
        let fills =
            highlight_decorations(found, p.accent.alpha(READ_FILL), p.accent.alpha(WRITE_FILL));
        ranges.set(fills, cx);
        marks.set(ruler_marks(found, p.accent.alpha(RULER_MARK)), cx);
    }
}

/// The whole-word matches of `word` in `text`, at most [`MAX_TEXT_MATCHES`], all reads.
fn text_highlights(text: &str, word: &str) -> Vec<Highlight> {
    occurrences(text, word, true)
        .into_iter()
        .take(MAX_TEXT_MATCHES)
        .map(|range| (range, false))
        .collect()
}

/// The fills of `found`: writes in `write`, other uses in `read`.
fn highlight_decorations(found: &[Highlight], read: Hsla, write: Hsla) -> Vec<RangeDecoration> {
    found
        .iter()
        .map(|(range, writes)| {
            RangeDecoration::new(range.clone())
                .with_style(RangeDecorationStyle::Fill)
                .with_color(if *writes { write } else { read })
        })
        .collect()
}

/// The scrollbar marks of `found`, in the centre lane in `color`.
fn ruler_marks(found: &[Highlight], color: Hsla) -> Vec<OverviewRulerDecoration> {
    found
        .iter()
        .map(|(range, _)| {
            OverviewRulerDecoration::new(range.clone())
                .with_lane(OverviewRulerLane::Center)
                .with_color(color)
        })
        .collect()
}

/// Whether `cursor` is inside one of `ranges` or at either end of one.
fn touches(ranges: &[Range<usize>], cursor: usize) -> bool {
    ranges
        .iter()
        .any(|range| range.start <= cursor && cursor <= range.end)
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{MAX_TEXT_MATCHES, highlight_decorations, ruler_marks, text_highlights, touches};
    use gpui_kit::Hsla;
    use gpui_kit::component::input::{OverviewRulerLane, RangeDecorationStyle};

    #[test]
    fn text_matches_are_whole_words_and_reads() {
        let text = "total = 1\nsubtotal = total + total_2\nprint(total)\n";
        let found: Vec<_> = text_highlights(text, "total")
            .into_iter()
            .map(|(range, write)| (&text[range.clone()], range.start, write))
            .collect();
        assert_eq!(
            found,
            [
                ("total", 0, false),
                ("total", 21, false),
                ("total", 43, false)
            ]
        );
    }

    #[test]
    fn text_matches_stop_at_the_limit() {
        let text = "x ".repeat(MAX_TEXT_MATCHES + 20);
        let found = text_highlights(&text, "x");
        assert_eq!(found.len(), MAX_TEXT_MATCHES);
        assert_eq!(found.last().map(|(range, _)| range.start), Some(998));
    }

    #[test]
    fn writes_are_filled_more_strongly_than_reads() {
        let (read, write) = (Hsla::white(), Hsla::black());
        let drawn: Vec<_> = highlight_decorations(&[(0..5, true), (10..15, false)], read, write)
            .iter()
            .map(|decoration| {
                (
                    decoration.range().clone(),
                    decoration.style(),
                    decoration.color(),
                )
            })
            .collect();
        assert_eq!(
            drawn,
            [
                (0..5, RangeDecorationStyle::Fill, Some(write)),
                (10..15, RangeDecorationStyle::Fill, Some(read)),
            ]
        );
    }

    #[test]
    fn every_use_is_marked_in_the_centre_of_the_scrollbar() {
        let color = Hsla::white();
        let marks: Vec<_> = ruler_marks(&[(0..5, true), (10..15, false)], color)
            .iter()
            .map(|mark| (mark.range().clone(), mark.lane(), mark.color()))
            .collect();
        assert_eq!(
            marks,
            [
                (0..5, OverviewRulerLane::Center, Some(color)),
                (10..15, OverviewRulerLane::Center, Some(color)),
            ]
        );
    }

    #[test]
    fn the_cursor_touches_a_range_at_its_ends_and_inside() {
        let ranges = [4..9, 20..25];
        for cursor in [4, 6, 9, 20, 25] {
            assert!(touches(&ranges, cursor), "{cursor}");
        }
        for cursor in [0, 3, 10, 19, 26] {
            assert!(!touches(&ranges, cursor), "{cursor}");
        }
        assert!(!touches(&[], 0));
    }
}
