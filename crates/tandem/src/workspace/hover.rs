//! The editor's tooltip. The toolkit asks [`HoverSource`] for a hover at the caret offset
//! nearest the pointer, on any part of a line; the source answers only when the pointer
//! is on a character of a symbol, and shows the reply in Tandem's own [`HoverTip`]
//! instead of the toolkit's popover. [`claim_diagnostic`] does the same for diagnostics.
mod content;
mod hit;
mod tip;

pub use tip::HoverTip;

use anyhow::Result;
use gpui_kit::component::input::{EditorState, HoverProvider, Rope, RopeExt as _};
use gpui_kit::{App, Entity, MouseMoveEvent, Pixels, Point, Task, WeakEntity, Window};
use lsp_types::Hover;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use tandem_lsp::{LanguageManager, position_params};

/// Answers the toolkit's hover requests for one editor tab.
#[derive(Clone)]
pub struct HoverSource {
    pub manager: LanguageManager,
    pub file: PathBuf,
    pub instance: String,
    /// `Document::language()` of the file, for code blocks that name no language.
    pub language: &'static str,
    pub editor: WeakEntity<EditorState>,
    pub tip: WeakEntity<HoverTip>,
}

impl HoverSource {
    /// The character to ask the server about: the one under the pointer, unless it is
    /// whitespace or the tooltip already describes it.
    fn target(
        &self,
        text: &str,
        offset: usize,
        pointer: Point<Pixels>,
        cx: &App,
    ) -> Option<Range<usize>> {
        let editor = self.editor.upgrade()?;
        let state = editor.read(cx);
        let hit =
            hit::char_under_pointer(text, offset, pointer, |range| state.range_to_bounds(range))?;
        let shown = self
            .tip
            .upgrade()?
            .read(cx)
            .covers(editor.entity_id(), &hit);
        (hit::is_hoverable(text, &hit) && !shown).then_some(hit)
    }

    /// Opens the tooltip for a reply, if it is about the character under the pointer and
    /// the pointer is still there.
    fn present(
        &self,
        text: &str,
        hit: Range<usize>,
        hover: Hover,
        pointer: Point<Pixels>,
        cx: &mut App,
    ) {
        let (Some(editor), Some(tip)) = (self.editor.upgrade(), self.tip.upgrade()) else {
            return;
        };
        let server = hover
            .range
            .and_then(|range| hit::server_range(text, &range));
        let Some(range) = hit::accepted_range(text, &hit, server) else {
            return;
        };
        let content = content::parse(&hover.contents, self.language);
        let on_hit = editor
            .read(cx)
            .range_to_bounds(&hit)
            .is_some_and(|bounds| bounds.contains(&pointer));
        if content.is_empty() || !on_hit {
            return;
        }
        tip.update(cx, |tip, cx| {
            tip.show(self.editor.clone(), range, hit, content, cx)
        });
    }
}

impl HoverProvider for HoverSource {
    /// Always resolves to `None`, so the toolkit's own popover never opens.
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let source = self.clone();
        let text: Arc<str> = Arc::from(text.to_string());
        window.spawn(cx, async move |cx| {
            // The toolkit calls `hover` while it is updating the editor, so the editor's
            // layout can only be read from here, after that update has ended.
            let hit =
                cx.update(|window, cx| source.target(&text, offset, window.mouse_position(), cx))?;
            let Some(hit) = hit else {
                return Ok(None);
            };
            let request = cx.background_executor().spawn({
                let (manager, file, instance) = (
                    source.manager.clone(),
                    source.file.clone(),
                    source.instance.clone(),
                );
                let text = text.clone();
                let at = hit::request_offset(&text, &hit);
                async move {
                    let reply = manager
                        .request(
                            "textDocument/hover",
                            &file,
                            &instance,
                            &text,
                            position_params(&text, at)?,
                        )
                        .await?;
                    anyhow::ensure!(manager.is_current(&reply.stamp), "Stale hover");
                    Ok::<Option<Hover>, anyhow::Error>(serde_json::from_value(reply.value)?)
                }
            });
            // A failed or outdated request shows nothing.
            if let Ok(Some(hover)) = request.await {
                cx.update(|window, cx| {
                    source.present(&text, hit, hover, window.mouse_position(), cx)
                })?;
            }
            Ok(None)
        })
    }
}

/// Takes a diagnostic over from the toolkit. The toolkit opens its own popover for the
/// diagnostic at the caret offset nearest the pointer, which is also reached from the
/// empty space beside a line, and it has no setting to turn that popover off. So this
/// closes the popover the moment the toolkit has opened it, before anything is painted,
/// and shows the diagnostic in Tandem's card instead, if the pointer is on the
/// diagnostic's own text. Called for every pointer move over the editor, after the
/// toolkit's own handler.
pub fn claim_diagnostic(
    editor: &Entity<EditorState>,
    tip: &Entity<HoverTip>,
    event: &MouseMoveEvent,
    cx: &mut App,
) {
    let state = editor.read(cx);
    let Some(entry) = state.diagnostic_popover() else {
        return;
    };
    // Only the lines the diagnostic is on are copied out of the buffer.
    let text = state.text();
    let start = entry.range.start.min(text.len());
    let end = entry.range.end.clamp(start, text.len());
    let from = text.line_start_offset(text.offset_to_point(start).row);
    let to = text.line_end_offset(text.offset_to_point(end).row);
    let lines = text.slice(from..to).to_string();
    let piece = hit::piece_under_pointer(
        &lines,
        &(start - from..end - from),
        event.position,
        |range| state.range_to_bounds(&(range.start + from..range.end + from)),
    )
    .map(|piece| piece.start + from..piece.end + from)
    // With a button down the pointer is dragging a selection; nothing opens then.
    .filter(|_| event.pressed_button.is_none());
    editor.update(cx, |state, cx| state.clear_diagnostic_popover(cx));
    if let Some(piece) = piece {
        let content = content::problem(
            entry.severity,
            entry.source.as_ref().map(|source| source.as_ref()),
            entry.code.as_ref().map(|code| code.as_ref()),
            &entry.message,
        );
        tip.update(cx, |tip, cx| {
            tip.show_problem(editor.downgrade(), piece, content, cx)
        });
    }
}
