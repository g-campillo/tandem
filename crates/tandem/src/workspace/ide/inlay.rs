//! Type and parameter-name hints at the end of the line: once the text has rested after
//! an edit, the language server's inlay hints for the whole file are asked for, and each
//! line's hints are drawn after it in a muted italic. Every kind is kept, and the Editor
//! settings choose which kinds are drawn, so a switch shows or hides them at once.
use super::*;
use crate::appearance::palette;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::RopeExt as _;
use std::time::{Duration, Instant};
use tandem_lsp::features::inlay::{LineHint, eol_hints, hint_text, inlay_params, shift_hint_lines};

/// How long the text rests after an edit before its hints are asked for.
const DELAY: Duration = Duration::from_millis(700);
/// How long to wait before asking again when the server had no answer (rust-analyzer
/// answers `null` while it loads the project) or failed.
const RETRY: Duration = Duration::from_secs(2);
/// The size of the hints against the editor font's.
const SCALE: f32 = 0.9;
/// The space between the end of a line and its hints, in characters of the editor font.
const GAP: f32 = 1.5;

/// The hints of the active tab's editor.
pub(super) struct InlayState {
    editor_id: EntityId,
    /// The tab instance and text hash last asked about; cleared by an edit, so the text is
    /// asked about again even when an undo brings back the text these hints are for.
    key: String,
    /// Every kind of hint, by line. Shared with the overlay drawn over the editor.
    hints: Rc<Vec<LineHint>>,
    /// Whether a request is in flight.
    busy: bool,
    /// When to ask again although the text has not changed: after a reply without hints
    /// or a failed request.
    retry_at: Option<Instant>,
}

impl Workspace {
    /// Asks the language server of the active tab for the hints of its text, when either
    /// kind of hint is shown, the server is ready and offers hints, the text has rested
    /// [`DELAY`] after the last edit, no request is in flight and the text has changed
    /// since the last request (or a retry is due). Forgets the hints once their editor is
    /// gone or the active tab has no language server.
    pub(super) fn inlay_tick(&mut self, cx: &mut Context<Self>) {
        let target = self.active_ide_target();
        let gone = self.ide.inlay.as_ref().is_some_and(|inlay| {
            !self.ide.tabs.contains_key(&inlay.editor_id)
                || target
                    .as_ref()
                    .is_some_and(|target| target.manager.is_none())
        });
        if gone {
            self.ide.inlay = None;
            cx.notify();
        }
        let prefs = self.editor_prefs();
        let busy = self.ide.inlay.as_ref().is_some_and(|inlay| inlay.busy);
        if !(prefs.type_hints || prefs.parameter_hints)
            || busy
            || self.loading
            || self.dev.last_edit.elapsed() < DELAY
        {
            return;
        }
        let Some(target) = target else {
            return;
        };
        let manager = target.manager.filter(|manager| {
            manager.health().state == "ready" && manager.capabilities().has("inlayHintProvider")
        });
        let Some(manager) = manager else {
            return;
        };
        let editor_id = target.editor.entity_id();
        let text = target.editor.read(cx).text().to_string();
        let key = format!("{}:{}", target.instance, content_hash(text.as_bytes()));
        let asked = self.ide.inlay.as_ref().is_some_and(|inlay| {
            inlay.key == key && inlay.retry_at.is_none_or(|at| Instant::now() < at)
        });
        if asked {
            return;
        }
        self.sync_language(cx);
        // The hints on screen stay until the reply replaces them.
        let hints = match self.ide.inlay.take() {
            Some(inlay) if inlay.editor_id == editor_id => inlay.hints,
            _ => Rc::default(),
        };
        self.ide.inlay = Some(InlayState {
            editor_id,
            key: key.clone(),
            hints,
            busy: true,
            retry_at: None,
        });
        let reply = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, text.clone());
            async move {
                let reply = manager
                    .request(
                        "textDocument/inlayHint",
                        &file,
                        &instance,
                        &text,
                        inlay_params(&text),
                    )
                    .await?;
                anyhow::ensure!(!reply.value.is_null(), "No hints yet");
                Ok(manager
                    .is_current(&reply.stamp)
                    .then(|| eol_hints(&text, &reply.value)))
            }
        });
        cx.spawn(async move |view, cx| {
            let reply = reply.await;
            view.update(cx, |view, cx| {
                view.inlay_replied(editor_id, &key, &text, reply, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the hints of the text `key` was asked about, while `editor` still has that
    /// `text`. A stale reply is asked for again on a later tick, and a reply without
    /// hints or a failed request after [`RETRY`]. A reply for text edited meanwhile is
    /// dropped: the edit cleared the key, so the new text is asked about.
    fn inlay_replied(
        &mut self,
        editor_id: EntityId,
        key: &str,
        text: &str,
        reply: Result<Option<Vec<LineHint>>>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .ide
            .tabs
            .get(&editor_id)
            .and_then(|tab| tab.editor.upgrade())
            .is_some_and(|editor| *editor.read(cx).text() == text);
        let Some(inlay) = self
            .ide
            .inlay
            .as_mut()
            .filter(|inlay| inlay.editor_id == editor_id)
        else {
            return;
        };
        inlay.busy = false;
        if inlay.key != key {
            return;
        }
        match reply {
            Ok(Some(hints)) if current => {
                inlay.hints = Rc::new(hints);
                cx.notify();
            }
            Ok(_) => inlay.key.clear(),
            Err(_) => inlay.retry_at = Some(Instant::now() + RETRY),
        }
    }

    /// Follows an edit of `editor`: the hints on the edited rows go, the hints after them
    /// move with their lines, and the text is asked about again.
    pub(super) fn inlay_edited(
        &mut self,
        editor: &Entity<EditorState>,
        event: &InputEditsEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(inlay) = self
            .ide
            .inlay
            .as_mut()
            .filter(|inlay| inlay.editor_id == editor.entity_id())
        else {
            return;
        };
        inlay.key.clear();
        shift_for_edits(Rc::make_mut(&mut inlay.hints), event.edits());
        cx.notify();
    }

    /// The hints of `editor` drawn after its lines, in the editor font at 0.9 times its
    /// size, italic and muted, or None when there are none to draw. The overlay covers
    /// the editor and takes no mouse events. It places the hints while it paints, from
    /// the layout the editor made for this frame, so they keep to their lines while the
    /// editor scrolls.
    pub(super) fn inlay_overlay(
        &self,
        editor: &Entity<EditorState>,
        face: &Face,
        cx: &App,
    ) -> Option<AnyElement> {
        let prefs = self.editor_prefs();
        let (types, parameters) = (prefs.type_hints, prefs.parameter_hints);
        let inlay = self.ide.inlay.as_ref().filter(|inlay| {
            inlay.editor_id == editor.entity_id()
                && (types || parameters)
                && !inlay.hints.is_empty()
        })?;
        let hints = inlay.hints.clone();
        let editor = editor.clone();
        let color = palette(cx.theme().mode).secondary_foreground;
        let editor_font = Font {
            family: face.family.clone(),
            features: face.features.clone(),
            fallbacks: None,
            weight: FontWeight::default(),
            style: FontStyle::Normal,
        };
        let hint_font = Font {
            style: FontStyle::Italic,
            ..editor_font.clone()
        };
        let size = px(face.size);
        let overlay = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let placed = place_hints(editor.read(cx), &hints, types, parameters);
                if placed.is_empty() {
                    return;
                }
                let text_system = window.text_system().clone();
                let character = text_system
                    .ch_advance(text_system.resolve_font(&editor_font), size)
                    .unwrap_or(size * 0.6);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (end, label) in placed {
                        let run = TextRun {
                            len: label.len(),
                            font: hint_font.clone(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = text_system.shape_line(label.into(), size * SCALE, &[run], None);
                        let origin = point(end.origin.x + character * GAP, end.origin.y);
                        line.paint(origin, end.size.height, TextAlign::Left, None, window, cx)
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        Some(overlay.into_any_element())
    }
}

/// Where the hints of the lines `state` shows go, with their text: the bounds of each
/// hinted line's end. Lines outside the rows laid out for this frame and folded lines
/// are left out, as the editor places an offset in them at the start of the next line it
/// shows; so is the first line of a fold, whose end shows the fold's placeholder.
fn place_hints(
    state: &EditorState,
    hints: &[LineHint],
    types: bool,
    parameters: bool,
) -> Vec<(Bounds<Pixels>, String)> {
    let Some(rows) = state.visible_row_range() else {
        return Vec::new();
    };
    let text = state.text();
    let folds = state.display_map();
    visible_hints(hints, rows, text.lines_len())
        .iter()
        .filter(|hint| {
            let line = hint.line as usize;
            !folds.is_buffer_line_hidden(line) && !folds.is_folded_at(line)
        })
        .filter_map(|hint| {
            let label = hint_text(hint, types, parameters)?;
            let end = text.line_end_offset(hint.line as usize);
            let bounds = state.range_to_bounds(&(end..end))?;
            Some((bounds, label))
        })
        .collect()
}

/// The hints of `hints` (sorted by line) on `rows` of a text of `lines` lines.
fn visible_hints(hints: &[LineHint], rows: Range<usize>, lines: usize) -> &[LineHint] {
    let end = rows.end.min(lines);
    let first = hints.partition_point(|hint| (hint.line as usize) < rows.start);
    let last = hints.partition_point(|hint| (hint.line as usize) < end);
    &hints[first..last.max(first)]
}

/// Moves `hints` with each of `edits` in turn.
fn shift_for_edits(hints: &mut Vec<LineHint>, edits: &[(InputEdit, Rope)]) {
    let row = |row: usize| u32::try_from(row).unwrap_or(u32::MAX);
    for (edit, _) in edits {
        shift_hint_lines(
            hints,
            row(edit.start_position.row),
            row(edit.old_end_position.row),
            row(edit.new_end_position.row),
        );
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{shift_for_edits, visible_hints};
    use gpui_kit::component::input::{InputEdit, Point as TextPoint, Rope};
    use tandem_lsp::features::inlay::LineHint;

    fn hint(line: u32) -> LineHint {
        LineHint {
            line,
            labels: vec![format!("line {line}")],
            parameters: Vec::new(),
        }
    }

    fn lines(hints: &[LineHint]) -> Vec<u32> {
        hints.iter().map(|hint| hint.line).collect()
    }

    #[test]
    fn only_the_hints_of_the_laid_out_rows_are_drawn() {
        let hints: Vec<_> = [0, 3, 5, 9, 12].into_iter().map(hint).collect();
        assert_eq!(lines(visible_hints(&hints, 3..10, 20)), [3, 5, 9]);
        assert_eq!(lines(visible_hints(&hints, 4..9, 20)), [5]);
        assert!(visible_hints(&hints, 6..9, 20).is_empty());
        // Rows past the end of the text have no hints.
        assert_eq!(lines(visible_hints(&hints, 8..30, 10)), [9]);
        assert!(visible_hints(&hints, 15..30, 10).is_empty());
        assert!(visible_hints(&[], 0..10, 10).is_empty());
    }

    /// The edit that turned `before` into `after` by replacing `start..old_end`.
    fn edit(before: &str, after: &str, start: usize, old_end: usize) -> (InputEdit, Rope) {
        let new_end = old_end + after.len() - before.len();
        let point = |offset: usize, text: &str| {
            let row = text[..offset].matches('\n').count();
            let column = offset - text[..offset].rfind('\n').map_or(0, |line| line + 1);
            TextPoint::new(row, column)
        };
        let input = InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(start, before),
            old_end_position: point(old_end, before),
            new_end_position: point(new_end, after),
        };
        (input, Rope::from_str(after))
    }

    #[test]
    fn a_new_line_moves_the_hints_below_it() {
        let mut hints: Vec<_> = [0, 1, 2].into_iter().map(hint).collect();
        // Enter at the end of the first line.
        shift_for_edits(&mut hints, &[edit("a\nb\nc", "a\n\nb\nc", 1, 1)]);
        assert_eq!(lines(&hints), [2, 3]);
    }

    #[test]
    fn several_edits_apply_in_turn() {
        let mut hints: Vec<_> = [0, 2, 4].into_iter().map(hint).collect();
        let first = edit("a\nb\nc\nd\ne", "a\nb\nc\nd\nex", 9, 9);
        // Then the second line is joined to the first.
        let second = edit("a\nb\nc\nd\nex", "ab\nc\nd\nex", 1, 2);
        shift_for_edits(&mut hints, &[first, second]);
        assert_eq!(lines(&hints), [1]);
        assert_eq!(hints[0].labels, ["line 2"]);
    }
}
