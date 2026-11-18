//! Format Document (⇧⌥F) and Format on Save (⌘S): the language server's formatting of the
//! whole file, applied to the buffer as one undo step with the cursor kept on its text.
//! A save formats first only when Settings → Editor → Format on save is on and the file's
//! language server can format; a formatter that fails or is slow never stops the save.
use super::edits::plan_buffer_edit;
use super::*;
use lsp_types::TextEdit;
use serde_json::{Value, json};

/// How long a save waits for the language server's formatting before it saves the file
/// as it is.
const SAVE_FORMAT_WAIT: Duration = Duration::from_secs(3);
const STALE: &str = "The code changed before the language server answered. Try again.";
const FORMATTING: &str = "textDocument/formatting";
const FORMATTING_PROVIDER: &str = "documentFormattingProvider";

/// What a save does with the file first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SavePlan {
    /// Ask the language server to format the file, apply its edits, then save.
    FormatThenSave,
    /// Save the file as it is.
    SaveOnly,
}

/// A save formats the file first only when Format on save is on, the file's language
/// server can format it and the buffers may be edited (no project opening, no takeover).
fn save_plan(format_on_save: bool, server_can_format: bool, busy: bool) -> SavePlan {
    if format_on_save && server_can_format && !busy {
        SavePlan::FormatThenSave
    } else {
        SavePlan::SaveOnly
    }
}

/// The parameters of a `textDocument/formatting` request (the server adds the document):
/// the file's indentation unit as the formatting options.
fn formatting_params(unit: IndentUnit) -> Value {
    json!({"options": {"tabSize": unit.width, "insertSpaces": !unit.hard_tabs}})
}

/// The one edit that applies a `textDocument/formatting` reply (`TextEdit[]` or null) to
/// `text`, with the cursor at `cursor` kept on its text; `None` when the reply changes
/// nothing. An error when the reply is not a list of edits or does not fit `text`.
fn formatting_edit(text: &str, reply: &Value, cursor: usize) -> Result<Option<Edit>> {
    let edits: Option<Vec<TextEdit>> = serde_json::from_value(reply.clone())
        .context("The language server's formatting is not a list of edits")?;
    let edit = plan_buffer_edit(text, &edits.unwrap_or_default(), cursor)?;
    Ok(edit.changes_text().then_some(edit))
}

/// Whether `manager` is running and formats whole files.
fn can_format(manager: &LanguageManager) -> bool {
    manager.health().state == "ready" && manager.capabilities().has(FORMATTING_PROVIDER)
}

impl Workspace {
    /// Format Document (⇧⌥F): asks the language server of the active file to format it and
    /// applies the result to the buffer as one undo step. The view stays where it is and
    /// the cursor stays on its text.
    pub(in crate::workspace) fn format_document(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An earlier Format Document still waiting for the server must not apply later.
        self.ide.formatting = None;
        // Settings → Language tools has a Format Buffer button, so Settings may cover the
        // editor.
        let Some(target) = self.active_ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let Some(manager) = self.format_manager(&target, cx) else {
            return;
        };
        let text = target.editor.read(cx).text().to_string();
        let reply = self.request_formatting(&target, manager, text.clone(), cx);
        let (editor, name) = (target.editor.downgrade(), target.relative);
        self.message("Formatting…", false, cx);
        self.ide.formatting = Some(cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                view.format_replied(editor, &name, &text, reply, window, cx)
            })
            .ok();
        }));
    }

    /// The language server that formats the target's file, or `None` with a status
    /// message saying why there is none now.
    fn format_manager(
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
                    "Format Document needs a running language server for this file."
                }
                Some(manager) if !manager.capabilities().has(FORMATTING_PROVIDER) => {
                    "This file's language server does not format files."
                }
                Some(manager) => return Some(manager.clone()),
            }
        };
        self.message(refusal, false, cx);
        None
    }

    /// Asks `manager` to format the target's file, whose text is `text`, once the server
    /// has that text: the reply, or `None` when the code changed before the server
    /// answered.
    fn request_formatting(
        &mut self,
        target: &IdeTarget,
        manager: LanguageManager,
        text: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<Value>>> {
        self.sync_language(cx);
        let params = formatting_params(target.unit);
        let (file, instance) = (target.file.clone(), target.instance.clone());
        cx.background_spawn(async move {
            let reply = manager
                .request(FORMATTING, &file, &instance, &text, params)
                .await?;
            Ok(manager.is_current(&reply.stamp).then_some(reply.value))
        })
    }

    /// Applies the server's formatting of `name` to `editor`, while it still has the
    /// `text` the server formatted, or says why it was not applied.
    fn format_replied(
        &mut self,
        editor: WeakEntity<EditorState>,
        name: &str,
        text: &str,
        reply: Result<Option<Value>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.formatting = None;
        let Some(editor) = editor.upgrade() else {
            return;
        };
        let value = match reply {
            Ok(Some(value)) => value,
            Ok(None) => return self.message(STALE, false, cx),
            Err(error) => {
                return self.message(format!("Could not format {name}: {error:#}"), true, cx);
            }
        };
        let cursor = {
            let state = editor.read(cx);
            if *state.text() != text {
                return self.message(STALE, false, cx);
            }
            state.cursor()
        };
        match formatting_edit(text, &value, cursor) {
            Ok(None) => self.message("Already formatted.", false, cx),
            Ok(Some(edit)) => {
                if self.ide_replace(&editor, &edit, window, cx) {
                    self.message(format!("Formatted {name}."), false, cx);
                } else {
                    let message = format!("{name} can't be changed now, so it was not formatted.");
                    self.message(message, false, cx);
                }
            }
            Err(error) => {
                let message = format!("Could not format {name}: {error:#}");
                self.message(message, true, cx);
            }
        }
    }

    /// Save (⌘S, File → Save and the tab's Save button): with Format on save on and a
    /// language server that can format the file, formats it first, then saves it. The
    /// save waits at most 3 s for the formatting and saves the file as it is when the
    /// server fails, is slow, or the text changed meanwhile.
    pub(in crate::workspace) fn save_formatted(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.active_ide_target();
        let manager = target
            .as_ref()
            .and_then(|target| target.manager.clone())
            .filter(can_format);
        let plan = save_plan(
            self.editor_prefs().format_on_save,
            manager.is_some(),
            self.ide_refused(),
        );
        let (SavePlan::FormatThenSave, Some(target), Some(manager)) = (plan, target, manager)
        else {
            self.save_active(cx);
            return;
        };
        let text = target.editor.read(cx).text().to_string();
        let reply = self.request_formatting(&target, manager, text.clone(), cx);
        let editor = target.editor.downgrade();
        cx.spawn_in(window, async move |view, cx| {
            let timeout = async {
                smol::Timer::after(SAVE_FORMAT_WAIT).await;
                None
            };
            // On a timeout the request is dropped, which cancels it.
            let reply = smol::future::or(async { Some(reply.await) }, timeout).await;
            view.update_in(cx, |view, window, cx| {
                view.save_after_formatting(editor, &text, reply, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Applies the server's formatting to `editor` when it came in time, is current and
    /// the editor still has the `text` it formatted, then saves the editor's tab, which
    /// stays the one saved even when another tab became active meanwhile.
    fn save_after_formatting(
        &mut self,
        editor: WeakEntity<EditorState>,
        text: &str,
        reply: Option<Result<Option<Value>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = editor.upgrade() else {
            return;
        };
        let cursor = {
            let state = editor.read(cx);
            (*state.text() == text).then(|| state.cursor())
        };
        if let (Some(Ok(Some(value))), Some(cursor)) = (reply, cursor)
            && let Ok(Some(edit)) = formatting_edit(text, &value, cursor)
        {
            self.ide_replace(&editor, &edit, window, cx);
        }
        if let Some(index) = self.tabs.iter().position(|tab| tab.editor == editor) {
            self.save_index(index, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{SavePlan, formatting_edit, formatting_params, save_plan};
    use serde_json::json;
    use tandem_core::edit::indent::IndentUnit;

    #[test]
    fn formatting_params_follow_the_files_indentation() {
        let spaces = IndentUnit {
            width: 4,
            hard_tabs: false,
        };
        assert_eq!(
            formatting_params(spaces),
            json!({"options": {"tabSize": 4, "insertSpaces": true}})
        );
        let tabs = IndentUnit {
            width: 8,
            hard_tabs: true,
        };
        assert_eq!(
            formatting_params(tabs),
            json!({"options": {"tabSize": 8, "insertSpaces": false}})
        );
    }

    #[test]
    fn a_save_formats_first_only_when_asked_able_and_not_busy() {
        use SavePlan::{FormatThenSave, SaveOnly};
        let table = [
            // (format on save, server can format, busy) → plan
            ((true, true, false), FormatThenSave),
            ((false, true, false), SaveOnly),
            ((true, false, false), SaveOnly),
            ((true, true, true), SaveOnly),
            ((false, false, false), SaveOnly),
            ((false, false, true), SaveOnly),
            ((true, false, true), SaveOnly),
            ((false, true, true), SaveOnly),
        ];
        for ((format_on_save, server_can_format, busy), plan) in table {
            assert_eq!(
                save_plan(format_on_save, server_can_format, busy),
                plan,
                "format on save {format_on_save}, server can format {server_can_format}, busy {busy}"
            );
        }
    }

    const MESSY: &str = "fn describe() -> String {\n  let x = 1;\n        x.to_string()\n}\n";
    const TIDY: &str = "fn describe() -> String {\n    let x = 1;\n    x.to_string()\n}\n";

    fn indent(line: u32, from: u32, to: &str) -> serde_json::Value {
        json!({
            "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": from}},
            "newText": to,
        })
    }

    #[test]
    fn a_formatting_reply_becomes_one_edit_that_keeps_the_cursor_on_its_text() {
        let reply = json!([indent(1, 2, "    "), indent(2, 8, "    ")]);
        // On `x` of `x.to_string()`.
        let cursor = MESSY.find("x.to").unwrap();
        let edit = formatting_edit(MESSY, &reply, cursor).unwrap().unwrap();
        assert_eq!(edit.apply(MESSY), TIDY);
        let moved = TIDY.find("x.to").unwrap();
        assert_eq!(edit.selection, moved..moved);
    }

    #[test]
    fn a_reply_without_changes_means_already_formatted() {
        assert_eq!(formatting_edit(TIDY, &json!(null), 0).unwrap(), None);
        assert_eq!(formatting_edit(TIDY, &json!([]), 0).unwrap(), None);
        let same = json!([indent(1, 4, "    ")]);
        assert_eq!(formatting_edit(TIDY, &same, 0).unwrap(), None);
    }

    #[test]
    fn a_reply_that_does_not_fit_the_text_is_an_error() {
        assert!(formatting_edit(TIDY, &json!({"edits": []}), 0).is_err());
        assert!(formatting_edit(TIDY, &json!([indent(9, 1, "")]), 0).is_err());
    }
}
