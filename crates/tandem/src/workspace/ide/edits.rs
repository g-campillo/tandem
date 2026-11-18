//! Applying a language server's edits to the buffers: open tabs are edited in place, other
//! files are opened as tabs first. Either every file changes or none does, each buffer in
//! one undo step, and the changed buffers stay unsaved for the user to review and save.
use super::*;
use lsp_types::TextEdit;
use tandem_lsp::features::edits::{FileEdits, relative_edits};

/// The most tabs Tandem keeps open, as `open_file` does.
const MAX_TABS: usize = 32;

impl Workspace {
    /// Applies `files` to the buffers: open tabs are edited in place; other files are
    /// loaded with `load_document` in the background and opened as tabs, then edited; the
    /// active tab stays active. Each buffer gets one `ide_replace` (Edit::between of the
    /// old and new text, its cursor kept with `Edit::map_offset`). Nothing is applied when
    /// any file fails: a refusal, an edit that does not apply (`tandem_lsp::apply_text_edits`
    /// error), more tabs than 32, loading or a takeover. Calls `done(Ok(changed files))`
    /// or `done(Err(message))` on the main thread.
    pub(in crate::workspace) fn apply_file_edits(
        &mut self,
        files: Vec<FileEdits>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<usize, String>, &mut Context<Self>) + 'static,
    ) {
        if let Some(refusal) = self.edit_refusal() {
            return done(self, Err(refusal), cx);
        }
        let Some(project) = self.project.clone() else {
            return done(self, Err("Open a project folder first.".into()), cx);
        };
        let files = match relative_edits(&project.root, files) {
            Ok(files) => files,
            Err(refusal) => return done(self, Err(refusal.to_string()), cx),
        };
        let missing: Vec<String> = files
            .iter()
            .map(|(path, _)| path)
            .filter(|path| !self.tabs.iter().any(|tab| tab.document.path == **path))
            .cloned()
            .collect();
        if missing.is_empty() {
            let applied = self.apply_to_buffers(files, Vec::new(), window, cx);
            return done(self, applied, cx);
        }
        if self.tabs.len() + missing.len() > MAX_TABS {
            return done(self, Err(too_many_tabs(missing.len())), cx);
        }
        let (store, epoch) = (self.store.clone(), self.epoch);
        let loading = cx.background_spawn(async move {
            missing
                .iter()
                .map(|path| {
                    load_document(&store, &project, path, false)
                        .with_context(|| format!("Could not open {path} to change it"))
                })
                .collect::<Result<Vec<_>>>()
        });
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let loaded = loading.await;
                let _ = weak.update_in(cx, |view, window, cx| {
                    let applied = if view.epoch != epoch {
                        Err("Another project was opened, so the change was not applied.".into())
                    } else {
                        match loaded {
                            Ok(documents) => view.apply_to_buffers(files, documents, window, cx),
                            Err(error) => Err(format!("{error:#}. Nothing was changed.")),
                        }
                    };
                    done(view, applied, cx);
                });
            })
            .detach();
    }

    /// Why the buffers can't be edited now (a project is opening or Claude is editing the
    /// files), as a status message.
    fn edit_refusal(&self) -> Option<String> {
        if self.loading {
            Some("Wait for the project to finish opening.".into())
        } else if self.takeover_running() {
            Some("The editor is read-only while Claude edits files.".into())
        } else {
            None
        }
    }

    /// Applies each file's edits to its tab, opening the `loaded` documents of files
    /// without one as tabs first while the active tab stays active. Every buffer's edit is
    /// planned before any is applied, so nothing changes when one of them does not apply.
    /// The number of buffers that changed.
    fn apply_to_buffers(
        &mut self,
        files: Vec<(String, Vec<TextEdit>)>,
        loaded: Vec<Document>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        if let Some(refusal) = self.edit_refusal() {
            return Err(refusal);
        }
        let mut opening = Vec::new();
        let mut plans = Vec::new();
        for (path, edits) in &files {
            let tab = self.tabs.iter().find(|tab| tab.document.path == *path);
            let (text, cursor) = match tab {
                Some(tab) => {
                    let state = tab.editor.read(cx);
                    (state.text().to_string(), state.cursor())
                }
                None => {
                    // Its tab was closed while the other files were read.
                    let document = loaded
                        .iter()
                        .find(|document| document.path == *path)
                        .ok_or_else(|| {
                            format!("{path} was closed before the change could be applied. Nothing was changed.")
                        })?;
                    opening.push(document.clone());
                    (document.text.clone(), 0)
                }
            };
            let edit = plan_buffer_edit(&text, edits, cursor).map_err(|error| {
                format!(
                    "The change does not fit {path} as it is now ({error:#}). Nothing was changed."
                )
            })?;
            plans.push((path, edit));
        }
        if self.tabs.len() + opening.len() > MAX_TABS {
            return Err(too_many_tabs(opening.len()));
        }
        let active = self
            .active
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.editor.entity_id());
        for document in opening {
            self.add_tab(document, window, cx);
        }
        if let Some(active) = active {
            self.active = self
                .tabs
                .iter()
                .position(|tab| tab.editor.entity_id() == active);
        }
        let mut changed = 0;
        for (path, edit) in plans {
            let Some(editor) = self
                .tabs
                .iter()
                .find(|tab| tab.document.path == *path)
                .map(|tab| tab.editor.clone())
            else {
                continue;
            };
            // A tab not on screen keeps the read-only state of its last render, which may
            // have been during a takeover; nothing is read-only now.
            editor.update(cx, |editor, cx| editor.set_readonly(false, cx));
            if self.ide_replace(&editor, &edit, window, cx) {
                changed += 1;
            }
        }
        cx.notify();
        Ok(changed)
    }
}

/// Why a change that would open `opening` more tabs is not applied.
fn too_many_tabs(opening: usize) -> String {
    format!(
        "This change also edits {opening} file(s) that are not open, which would be more than {MAX_TABS} tabs. Close some tabs and try again."
    )
}

/// The one replacement that applies the server's `edits` to `old` as a single undo step
/// (`Edit::between` the old and the new text), with the cursor at `cursor` kept on the
/// text it was on: it is moved through each edit with `Edit::map_offset`, the last edit
/// first, so text the edits leave alone keeps the cursor. Fails when an edit does not
/// apply to `old` (a position past its end, or edits that overlap).
pub(super) fn plan_buffer_edit(
    old: &str,
    edits: &[TextEdit],
    cursor: usize,
) -> anyhow::Result<Edit> {
    let new = tandem_lsp::apply_text_edits(old, edits)?;
    let mut replacements = edits
        .iter()
        .map(|edit| {
            Ok(Edit {
                range: tandem_lsp::utf16_offset(old, edit.range.start)?
                    ..tandem_lsp::utf16_offset(old, edit.range.end)?,
                text: edit.new_text.clone(),
                selection: 0..0,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    replacements.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let cursor = replacements
        .iter()
        .rev()
        .fold(cursor, |cursor, edit| edit.map_offset(cursor));
    Ok(Edit::between(old, &new, cursor..cursor))
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::plan_buffer_edit;
    use lsp_types::{Position, Range, TextEdit};

    const TEXT: &str = "fn total_area() {}\n\nfn f() -> f64 {\n    total_area()\n}\n";

    fn edit(line: u32, start: u32, end: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range::new(Position::new(line, start), Position::new(line, end)),
            new_text: text.into(),
        }
    }

    /// The server's rename of both `total_area`s in `TEXT` to `sum_area`.
    fn rename() -> Vec<TextEdit> {
        vec![edit(3, 4, 14, "sum_area"), edit(0, 3, 13, "sum_area")]
    }

    #[test]
    fn the_edits_apply_as_one_replacement() {
        let planned = plan_buffer_edit(TEXT, &rename(), 0).unwrap();
        assert_eq!(
            planned.apply(TEXT),
            "fn sum_area() {}\n\nfn f() -> f64 {\n    sum_area()\n}\n"
        );
        // From the first changed character to the last: `total` and `sum` differ, `_area`
        // is common.
        let second_end = TEXT.find("()\n}").unwrap();
        assert_eq!(planned.range, 3..second_end - "_area".len());
    }

    #[test]
    fn the_cursor_stays_on_its_text() {
        let after = "fn sum_area() {}\n\nfn f() -> f64 {\n    sum_area()\n}\n";
        let cursor_at = |cursor: usize| {
            let planned = plan_buffer_edit(TEXT, &rename(), cursor).unwrap();
            assert!(planned.selection.is_empty(), "{:?}", planned.selection);
            planned.selection.start
        };
        // Before every edit.
        assert_eq!(cursor_at(1), 1);
        // Between the two edits, on `f` of `fn f`: one name shorter before it.
        let between = TEXT.find("fn f").unwrap();
        assert_eq!(cursor_at(between), after.find("fn f").unwrap());
        // After both edits, at the end.
        assert_eq!(cursor_at(TEXT.len()), after.len());
        // Inside the first name, three characters in.
        assert_eq!(cursor_at(6), 6);
        // At the end of the second name: past the new name.
        let end = TEXT.rfind("total_area").unwrap() + "total_area".len();
        assert_eq!(
            cursor_at(end),
            after.rfind("sum_area").unwrap() + "sum_area".len()
        );
    }

    #[test]
    fn utf16_columns_after_a_wide_character_are_read_as_utf16() {
        let text = "let é = 1; // ü\nlet x = é;\n";
        // `é` is one UTF-16 unit and two UTF-8 bytes.
        let edits = vec![edit(0, 4, 5, "e"), edit(1, 8, 9, "e")];
        let planned = plan_buffer_edit(text, &edits, text.len()).unwrap();
        assert_eq!(planned.apply(text), "let e = 1; // ü\nlet x = e;\n");
        assert_eq!(planned.selection, text.len() - 2..text.len() - 2);
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let edits = vec![edit(0, 3, 13, "sum_area"), edit(0, 8, 15, "x")];
        assert!(plan_buffer_edit(TEXT, &edits, 0).is_err());
        let past_the_end = vec![edit(9, 0, 1, "x")];
        assert!(plan_buffer_edit(TEXT, &past_the_end, 0).is_err());
    }

    #[test]
    fn edits_that_change_nothing_give_an_empty_edit() {
        let planned = plan_buffer_edit(TEXT, &[edit(0, 3, 13, "total_area")], 5).unwrap();
        assert!(!planned.changes_text());
        assert_eq!(planned.selection, 5..5);
        assert!(!plan_buffer_edit(TEXT, &[], 5).unwrap().changes_text());
    }
}
