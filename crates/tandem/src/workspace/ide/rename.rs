//! Rename Symbol (F2): a box under the symbol at the cursor with its name selected. Enter
//! asks the language server to rename the symbol everywhere and applies its edits to the
//! buffers, which stay unsaved; Escape, or leaving the box, cancels.
use super::*;
use crate::appearance::{RADIUS_CONTROL, palette};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use std::ops::Range;
use tandem_core::edit::word_range_at;
use tandem_lsp::features::edits::workspace_edit;

const BOX_WIDTH: f32 = 280.;
/// Space between the symbol and the box.
const GAP: f32 = 2.;
/// The box keeps this far from the window's edges.
const EDGE: f32 = 8.;
const CANT_RENAME: &str = "This symbol can't be renamed.";
const NO_SYMBOL: &str = "Put the cursor on a symbol to rename it.";
const STALE: &str = "The code changed before the language server answered. Try again.";

/// The rename box of one editor.
pub(in crate::workspace) struct RenameState {
    pub(super) editor_id: EntityId,
    /// The symbol's byte range in `text`, which the box hangs below.
    range: Range<usize>,
    /// The name the box opened with.
    old: String,
    /// The editor's text and cursor when the box opened; the rename is asked for there.
    text: String,
    cursor: usize,
    input: Entity<InputState>,
    _events: Subscription,
}

impl Workspace {
    /// Rename Symbol (F2): opens the rename box on the symbol at the cursor of the active
    /// file: the one the language server names in its `textDocument/prepareRename` reply
    /// when it answers that request, else the word at the cursor.
    pub(in crate::workspace) fn rename_symbol(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An earlier Rename Symbol still waiting for the server must not open its box later.
        self.ide.renaming = None;
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let Some(manager) = self.rename_manager(&target, cx) else {
            return;
        };
        let (text, cursor) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        if !manager.capabilities().rename_prepare() {
            match word_at(&text, cursor) {
                Some((range, old)) => self.open_rename(&target.editor, range, old, window, cx),
                None => self.message(NO_SYMBOL, false, cx),
            }
            return;
        }
        let Ok(params) = tandem_lsp::position_params(&text, cursor) else {
            return;
        };
        self.sync_language(cx);
        let reply = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, text.clone());
            async move {
                let reply = manager
                    .request(
                        "textDocument/prepareRename",
                        &file,
                        &instance,
                        &text,
                        params,
                    )
                    .await?;
                Ok(manager.is_current(&reply.stamp).then_some(reply.value))
            }
        });
        let editor = target.editor.entity_id();
        let weak = cx.entity().downgrade();
        self.ide.renaming = Some(window.spawn(cx, async move |cx| {
            let reply = reply.await;
            weak.update_in(cx, |view, window, cx| {
                view.rename_prepared(editor, &text, cursor, reply, window, cx)
            })
            .ok();
        }));
    }

    /// The language server that renames symbols in the target's file, or `None` with a
    /// status message saying why there is none now.
    fn rename_manager(
        &mut self,
        target: &IdeTarget,
        cx: &mut Context<Self>,
    ) -> Option<LanguageManager> {
        let refusal = if self.loading {
            "Wait for the project to finish opening."
        } else if self.takeover_running() {
            "The editor is read-only while Claude edits files."
        } else {
            match &target.manager {
                None => {
                    "Turn on language tools for this file's language in Settings → Language tools."
                }
                Some(manager) if manager.health().state != "ready" => {
                    "Rename Symbol needs a running language server for this file."
                }
                Some(manager) if !manager.capabilities().has("renameProvider") => {
                    "This file's language server does not offer Rename Symbol."
                }
                Some(manager) => return Some(manager.clone()),
            }
        };
        self.message(refusal, false, cx);
        None
    }

    /// Opens the rename box on what the server's `textDocument/prepareRename` reply offers,
    /// while `editor` is the active tab's on screen and still has the `text` and the
    /// `cursor` it was asked about.
    fn rename_prepared(
        &mut self,
        editor: EntityId,
        text: &str,
        cursor: usize,
        reply: Result<Option<serde_json::Value>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.renaming = None;
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)
        else {
            return;
        };
        {
            let state = target.editor.read(cx);
            if state.cursor() != cursor || *state.text() != text {
                return;
            }
        }
        match reply {
            Ok(None) => self.message(STALE, false, cx),
            Ok(Some(value)) => match prepared_rename(text, cursor, &value) {
                Ok(Some((range, old))) => self.open_rename(&target.editor, range, old, window, cx),
                Ok(None) | Err(_) => self.message(CANT_RENAME, false, cx),
            },
            Err(_) => self.message(CANT_RENAME, false, cx),
        }
    }

    /// Shows the rename box on `range` of `editor` with `old` in it, all selected and
    /// focused. The editor's text and cursor now are kept for the rename request.
    fn open_rename(
        &mut self,
        editor: &Entity<EditorState>,
        range: Range<usize>,
        old: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (text, cursor) = {
            let state = editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(old.clone()));
        let events = cx.subscribe_in(
            &input,
            window,
            |view, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => view.submit_rename(input, window, cx),
                InputEvent::Blur => view.rename_blurred(input, cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.ide.rename = Some(RenameState {
            editor_id: editor.entity_id(),
            range,
            old,
            text,
            cursor,
            input,
            _events: events,
        });
        self.ide.sync_keys();
        cx.notify();
    }

    /// Enter in the rename box: closes it and gives the focus back to the editor, then,
    /// unless the name is empty or unchanged, asks the language server to rename the
    /// symbol and applies its edits.
    fn submit_rename(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.ide.rename.take_if(|rename| rename.input == *input) else {
            return;
        };
        self.rename_closed(rename.editor_id, Some(window), cx);
        let Some(name) = new_name(&rename.input.read(cx).value(), &rename.old) else {
            return;
        };
        let Some(target) = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == rename.editor_id)
        else {
            return;
        };
        let Some(manager) = self.rename_manager(&target, cx) else {
            return;
        };
        if *target.editor.read(cx).text() != rename.text {
            self.message(STALE, false, cx);
            return;
        }
        let Ok(mut params) = tandem_lsp::position_params(&rename.text, rename.cursor) else {
            return;
        };
        params["newName"] = serde_json::Value::String(name.clone());
        self.sync_language(cx);
        let reply = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, rename.text);
            async move {
                let reply = manager
                    .request("textDocument/rename", &file, &instance, &text, params)
                    .await?;
                Ok(manager.is_current(&reply.stamp).then_some(reply.value))
            }
        });
        self.message("Renaming…", false, cx);
        let weak = cx.entity().downgrade();
        self.ide.renaming = Some(window.spawn(cx, async move |cx| {
            let reply = reply.await;
            weak.update_in(cx, |view, window, cx| {
                view.rename_replied(name, reply, window, cx)
            })
            .ok();
        }));
    }

    /// Applies the server's rename to `name` to the buffers, or says why it was not.
    fn rename_replied(
        &mut self,
        name: String,
        reply: Result<Option<serde_json::Value>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.renaming = None;
        let files = match reply {
            Ok(Some(value)) => workspace_edit(&value).map_err(|refusal| refusal.to_string()),
            Ok(None) => Err(STALE.to_string()),
            Err(error) => Err(format!("Could not rename: {error:#}")),
        };
        match files {
            Err(message) => self.message(message, true, cx),
            Ok(files) if files.is_empty() => {
                self.message("The language server found nothing to rename.", false, cx)
            }
            Ok(files) => {
                self.apply_file_edits(files, window, cx, move |view, applied, cx| match applied {
                    Ok(changed) => view.message(
                        format!(
                            "Renamed to {name} in {changed} file(s). Review and save the changes."
                        ),
                        false,
                        cx,
                    ),
                    Err(message) => view.message(message, true, cx),
                })
            }
        }
    }

    /// Handles a key while the rename box is open: Escape cancels the rename and gives the
    /// focus back to the editor; no other key is taken. `None` when no box is open.
    pub(super) fn rename_key(
        &mut self,
        key: IdeKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let rename = self.ide.rename.as_ref()?;
        if key != IdeKey::Escape {
            return Some(false);
        }
        let editor = rename.editor_id;
        self.ide.rename = None;
        self.rename_closed(editor, Some(window), cx);
        Some(true)
    }

    /// Cancels the rename when its box loses the focus; the focus stays where it went.
    fn rename_blurred(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        if let Some(rename) = self.ide.rename.take_if(|rename| rename.input == *input) {
            self.rename_closed(rename.editor_id, None, cx);
        }
    }

    /// Closes the rename box when its editor is no longer the active tab's on screen.
    pub(super) fn rename_tick(&mut self, cx: &mut Context<Self>) {
        let active = self.ide_target().map(|target| target.editor.entity_id());
        if let Some(rename) = self
            .ide
            .rename
            .take_if(|rename| Some(rename.editor_id) != active)
        {
            self.rename_closed(rename.editor_id, None, cx);
        }
    }

    /// After the rename box of `editor` closed: tells the key listeners, redraws, and with
    /// a `window` gives the focus back to the editor.
    fn rename_closed(
        &mut self,
        editor: EntityId,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.ide.sync_keys();
        cx.notify();
        let editor = self
            .ide
            .tabs
            .get(&editor)
            .and_then(|tab| tab.editor.upgrade());
        if let (Some(window), Some(editor)) = (window, editor) {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
    }
}

/// The rename box of `rename` under its symbol in `editor`: the name in the monospace font
/// and a hint below. While the symbol is scrolled out of view the box still shows, at the
/// container's edge, so the focus it holds is never out of sight.
pub(super) fn rename_box(
    rename: &RenameState,
    editor: &Entity<EditorState>,
    face: &Face,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let symbol = editor.read(cx).range_to_bounds(&rename.range);
    let p = palette(cx.theme().mode);
    let input = Input::new(&rename.input)
        .small()
        .font_family(cx.theme().mono_font_family.clone())
        .font_features(face.features.clone())
        .aria_label(format!("New name for {}", rename.old));
    let card = div()
        .occlude()
        .w(px(BOX_WIDTH))
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(4.))
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .shadow_lg()
        .font_family(cx.theme().font_family.clone())
        .text_size(px(13.))
        .text_color(p.foreground)
        .child(input)
        .child(
            div()
                .px(px(4.))
                .pb(px(2.))
                .text_size(px(11.))
                .text_color(p.secondary_foreground)
                .child("Enter to rename · Escape to cancel"),
        );
    let placed = anchored()
        .anchor(Anchor::TopLeft)
        .offset(point(px(0.), px(GAP)))
        .snap_to_window_with_margin(px(EDGE));
    let placed = match symbol {
        Some(symbol) => placed.attach(symbol, Anchor::BottomLeft),
        None => placed,
    };
    deferred(placed.child(card))
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element()
}

/// The symbol a `textDocument/prepareRename` reply offers to rename in `text`, as its byte
/// range and the name to show: a `Range` and its text, `{range, placeholder}`, or for
/// `{defaultBehavior: true}` the word at `cursor`. `None` when the reply is null (the
/// symbol can't be renamed) or there is no word at the cursor; an error when the reply is
/// none of these or its range does not fit `text`.
fn prepared_rename(
    text: &str,
    cursor: usize,
    reply: &serde_json::Value,
) -> Result<Option<(Range<usize>, String)>> {
    if reply.is_null() {
        return Ok(None);
    }
    if let Some(default) = reply.get("defaultBehavior") {
        anyhow::ensure!(
            default.as_bool() == Some(true),
            "The language server declined the default rename"
        );
        return Ok(word_at(text, cursor));
    }
    let (range, placeholder) = match reply.get("range") {
        Some(range) => (
            range,
            reply.get("placeholder").and_then(serde_json::Value::as_str),
        ),
        None => (reply, None),
    };
    let range: lsp_types::Range = serde_json::from_value(range.clone())?;
    let start = tandem_lsp::utf16_offset(text, range.start)?;
    let end = tandem_lsp::utf16_offset(text, range.end)?;
    anyhow::ensure!(start <= end, "The symbol's range ends before it starts");
    let name = placeholder.map_or_else(|| text[start..end].to_string(), String::from);
    Ok(Some((start..end, name)))
}

/// The word at `cursor` in `text` (touching it from either side): its byte range and text.
fn word_at(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let range = word_range_at(text, cursor)?;
    Some((range.clone(), text[range].to_string()))
}

/// The name typed into the box, without surrounding whitespace, when it names something
/// other than `old`; an empty or unchanged name renames nothing.
fn new_name(typed: &str, old: &str) -> Option<String> {
    let typed = typed.trim();
    (!typed.is_empty() && typed != old).then(|| typed.to_string())
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{new_name, prepared_rename, word_at};
    use serde_json::json;

    const TEXT: &str = "fn total_area() {}\nlet é = total_area();\n";

    fn range(line: u32, start: u32, end: u32) -> serde_json::Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    #[test]
    fn a_range_reply_renames_the_text_in_it() {
        assert_eq!(
            prepared_rename(TEXT, 5, &range(0, 3, 13)).unwrap(),
            Some((3..13, "total_area".to_string()))
        );
        // After `é`, UTF-16 columns and bytes differ.
        let second = TEXT.rfind("total_area").unwrap();
        assert_eq!(
            prepared_rename(TEXT, second, &range(1, 8, 18)).unwrap(),
            Some((second..second + 10, "total_area".to_string()))
        );
    }

    #[test]
    fn a_placeholder_reply_shows_the_placeholder() {
        let reply = json!({"range": range(0, 3, 13), "placeholder": "area"});
        assert_eq!(
            prepared_rename(TEXT, 5, &reply).unwrap(),
            Some((3..13, "area".to_string()))
        );
    }

    #[test]
    fn a_default_behavior_reply_renames_the_word_at_the_cursor() {
        let reply = json!({"defaultBehavior": true});
        assert_eq!(
            prepared_rename(TEXT, 13, &reply).unwrap(),
            Some((3..13, "total_area".to_string()))
        );
        assert_eq!(prepared_rename(TEXT, 15, &reply).unwrap(), None);
    }

    #[test]
    fn a_null_reply_renames_nothing() {
        assert_eq!(prepared_rename(TEXT, 5, &json!(null)).unwrap(), None);
    }

    #[test]
    fn a_malformed_reply_is_an_error() {
        for reply in [
            json!("total_area"),
            json!({"defaultBehavior": false}),
            json!({"placeholder": "x"}),
            json!({"start": {"line": 0, "character": 3}}),
            range(0, 13, 3),
            range(5, 0, 1),
        ] {
            assert!(prepared_rename(TEXT, 5, &reply).is_err(), "{reply}");
        }
    }

    #[test]
    fn the_word_at_the_cursor_is_found_from_inside_and_at_its_end() {
        assert_eq!(word_at(TEXT, 5), Some((3..13, "total_area".to_string())));
        assert_eq!(word_at(TEXT, 13), Some((3..13, "total_area".to_string())));
        assert_eq!(word_at(TEXT, 3), Some((3..13, "total_area".to_string())));
        assert_eq!(word_at(TEXT, 15), None);
        let accent = TEXT.find('é').unwrap();
        assert_eq!(
            word_at(TEXT, accent),
            Some((accent..accent + 2, "é".into()))
        );
    }

    #[test]
    fn an_empty_or_unchanged_name_renames_nothing() {
        assert_eq!(
            new_name("sum_area", "total_area").as_deref(),
            Some("sum_area")
        );
        assert_eq!(
            new_name("  sum_area\t", "total_area").as_deref(),
            Some("sum_area")
        );
        assert_eq!(new_name("", "total_area"), None);
        assert_eq!(new_name("   ", "total_area"), None);
        assert_eq!(new_name("total_area", "total_area"), None);
        assert_eq!(new_name(" total_area ", "total_area"), None);
    }
}
