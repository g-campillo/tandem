//! The edits a language server asks Tandem to make (a rename, a code action, a server
//! command), read into text edits per file. Edits that create, rename or delete files, or
//! that name anything but a local file, are refused as a whole.

use lsp_types::TextEdit;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The text edits for one file, in the order the server sent them.
#[derive(Clone, Debug, PartialEq)]
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
}

/// Why Tandem does not apply an edit. A refusal always covers the whole edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditRefusal {
    /// The edit creates, renames or deletes files.
    FileOperation,
    /// The edit names a document that is not a local file (its URI).
    NotAFile(String),
    /// The edit touches a file outside the project (its path).
    OutsideProject(String),
    /// The edit is not a well-formed workspace edit (what is wrong with it).
    Malformed(String),
}

impl fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileOperation => write!(
                f,
                "This change creates, renames or deletes files, which Tandem does not apply."
            ),
            Self::NotAFile(uri) => write!(
                f,
                "This change edits {uri}, which is not a file on this computer."
            ),
            Self::OutsideProject(path) => {
                write!(f, "This change edits {path}, which is outside the project.")
            }
            Self::Malformed(reason) => {
                write!(f, "Could not read the language server's edit: {reason}.")
            }
        }
    }
}

impl std::error::Error for EditRefusal {}

/// `changes` and/or `documentChanges` (TextDocumentEdit only; an entry with `kind`
/// create/rename/delete refuses the whole edit). When both are present `documentChanges`
/// is used, as the protocol asks. Edits for the same file are merged in order; files are
/// sorted by path, and a file with no edits is left out. A non-`file:` URI refuses the
/// whole edit.
pub fn workspace_edit(value: &Value) -> Result<Vec<FileEdits>, EditRefusal> {
    let mut files = Files::default();
    files.add_workspace_edit(value)?;
    Ok(files.finish())
}

/// What a `workspace/executeCommand` reply asks Tandem to apply: every collected
/// `edits` entry (each a WorkspaceEdit), plus `result` when it is a WorkspaceEdit
/// (has `changes` or `documentChanges`), a `TextEdit[]` (applies to `file`), or an
/// object with an `edits` WorkspaceEdit (Pyright's CommandResult). Merged per file, the
/// collected edits first since the server sent them before its result.
pub fn command_edits(reply: &Value, file: &Path) -> Result<Vec<FileEdits>, EditRefusal> {
    let mut files = Files::default();
    match reply.get("edits") {
        None | Some(Value::Null) => {}
        Some(Value::Array(edits)) => {
            for edit in edits {
                files.add_workspace_edit(edit)?;
            }
        }
        Some(_) => return Err(malformed("the collected edits are not a list")),
    }
    let result = reply.get("result").unwrap_or(&Value::Null);
    if result.get("changes").is_some() || result.get("documentChanges").is_some() {
        files.add_workspace_edit(result)?;
    } else if is_text_edit_list(result) {
        files.add(file.to_path_buf(), text_edits(result)?);
    } else if let Some(edit) = result.get("edits").filter(|edit| edit.is_object()) {
        files.add_workspace_edit(edit)?;
    }
    Ok(files.finish())
}

/// Text edits gathered per file, in the order they were added.
#[derive(Default)]
struct Files(BTreeMap<PathBuf, Vec<TextEdit>>);

impl Files {
    fn add(&mut self, path: PathBuf, edits: Vec<TextEdit>) {
        self.0.entry(path).or_default().extend(edits);
    }

    fn add_workspace_edit(&mut self, value: &Value) -> Result<(), EditRefusal> {
        let edit = match value {
            Value::Null => return Ok(()),
            Value::Object(edit) => edit,
            _ => return Err(malformed("the workspace edit is not an object")),
        };
        match edit.get("documentChanges") {
            None | Some(Value::Null) => {}
            Some(Value::Array(changes)) => {
                for change in changes {
                    self.add_document_change(change)?;
                }
                return Ok(());
            }
            Some(_) => return Err(malformed("its document changes are not a list")),
        }
        match edit.get("changes") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::Object(changes)) => {
                for (uri, edits) in changes {
                    self.add(file_path(uri)?, text_edits(edits)?);
                }
                Ok(())
            }
            Some(_) => Err(malformed("its changes are not a map of documents")),
        }
    }

    fn add_document_change(&mut self, change: &Value) -> Result<(), EditRefusal> {
        if let Some(kind) = change.get("kind") {
            return Err(match kind.as_str() {
                Some("create" | "rename" | "delete") => EditRefusal::FileOperation,
                _ => malformed(format!("unknown document change kind {kind}")),
            });
        }
        let uri = change
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a document change names no document"))?;
        let edits = change
            .get("edits")
            .ok_or_else(|| malformed("a document change has no edits"))?;
        self.add(file_path(uri)?, text_edits(edits)?);
        Ok(())
    }

    fn finish(self) -> Vec<FileEdits> {
        self.0
            .into_iter()
            .filter(|(_, edits)| !edits.is_empty())
            .map(|(path, edits)| FileEdits { path, edits })
            .collect()
    }
}

fn malformed(reason: impl Into<String>) -> EditRefusal {
    EditRefusal::Malformed(reason.into())
}

/// The local path a `file:` URI names.
fn file_path(uri: &str) -> Result<PathBuf, EditRefusal> {
    let url = url::Url::parse(uri)
        .map_err(|error| malformed(format!("{uri} is not a valid address ({error})")))?;
    if url.scheme() != "file" {
        return Err(EditRefusal::NotAFile(uri.into()));
    }
    url.to_file_path()
        .map_err(|_| EditRefusal::NotAFile(uri.into()))
}

fn text_edits(value: &Value) -> Result<Vec<TextEdit>, EditRefusal> {
    let edits: Vec<TextEdit> =
        serde_json::from_value(value.clone()).map_err(|error| malformed(error.to_string()))?;
    if edits.iter().any(|edit| edit.range.start > edit.range.end) {
        return Err(malformed("an edit's range ends before it starts"));
    }
    Ok(edits)
}

/// Whether a command result is a `TextEdit[]`: a non-empty list of objects that each
/// have a `range` and `newText`.
fn is_text_edit_list(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items
                .iter()
                .all(|item| item.get("range").is_some() && item.get("newText").is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};
    use serde_json::json;

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    fn edit(line: u32, start: u32, end: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range::new(Position::new(line, start), Position::new(line, end)),
            new_text: text.into(),
        }
    }

    fn file(path: &str, edits: Vec<TextEdit>) -> FileEdits {
        FileEdits {
            path: path.into(),
            edits,
        }
    }

    #[test]
    fn changes_for_two_files_are_read_and_sorted_by_path() {
        let value = json!({"changes": {
            "file:///project/src/b.rs": [{"range": range(0, 0, 1), "newText": "b"}],
            "file:///project/src/a.rs": [
                {"range": range(2, 0, 3), "newText": "one"},
                {"range": range(0, 4, 4), "newText": "two"},
            ],
        }});
        assert_eq!(
            workspace_edit(&value),
            Ok(vec![
                file(
                    "/project/src/a.rs",
                    vec![edit(2, 0, 3, "one"), edit(0, 4, 4, "two")]
                ),
                file("/project/src/b.rs", vec![edit(0, 0, 1, "b")]),
            ])
        );
    }

    #[test]
    fn document_changes_for_the_same_file_merge_in_order() {
        let value = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///project/lib.rs", "version": 3},
             "edits": [{"range": range(4, 0, 2), "newText": "first"}]},
            {"textDocument": {"uri": "file:///project/a%20b.rs", "version": null},
             "edits": [{"range": range(0, 0, 0), "newText": "other"}]},
            {"textDocument": {"uri": "file:///project/lib.rs", "version": 3},
             "edits": [
                 {"range": range(1, 0, 2), "newText": "second"},
                 {"range": range(0, 0, 1), "newText": "third", "annotationId": "rename"},
             ]},
        ]});
        assert_eq!(
            workspace_edit(&value),
            Ok(vec![
                file("/project/a b.rs", vec![edit(0, 0, 0, "other")]),
                file(
                    "/project/lib.rs",
                    vec![
                        edit(4, 0, 2, "first"),
                        edit(1, 0, 2, "second"),
                        edit(0, 0, 1, "third"),
                    ]
                ),
            ])
        );
    }

    #[test]
    fn document_changes_are_preferred_over_changes() {
        let value = json!({
            "changes": {"file:///project/lib.rs": [{"range": range(0, 0, 1), "newText": "old"}]},
            "documentChanges": [
                {"textDocument": {"uri": "file:///project/lib.rs", "version": 1},
                 "edits": [{"range": range(0, 0, 1), "newText": "new"}]},
            ],
        });
        assert_eq!(
            workspace_edit(&value),
            Ok(vec![file("/project/lib.rs", vec![edit(0, 0, 1, "new")])])
        );
    }

    #[test]
    fn a_file_operation_refuses_the_whole_edit() {
        for kind in ["create", "rename", "delete"] {
            let value = json!({"documentChanges": [
                {"textDocument": {"uri": "file:///project/lib.rs", "version": 1},
                 "edits": [{"range": range(0, 0, 1), "newText": "x"}]},
                {"kind": kind, "uri": "file:///project/new.rs",
                 "oldUri": "file:///project/old.rs", "newUri": "file:///project/new.rs"},
            ]});
            assert_eq!(
                workspace_edit(&value),
                Err(EditRefusal::FileOperation),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_document_that_is_not_a_file_refuses_the_whole_edit() {
        let value = json!({"changes": {
            "file:///project/lib.rs": [{"range": range(0, 0, 1), "newText": "x"}],
            "untitled:Untitled-1": [{"range": range(0, 0, 1), "newText": "y"}],
        }});
        assert_eq!(
            workspace_edit(&value),
            Err(EditRefusal::NotAFile("untitled:Untitled-1".into()))
        );
        let value = json!({"documentChanges": [
            {"textDocument": {"uri": "https://example.com/lib.rs", "version": 1},
             "edits": [{"range": range(0, 0, 1), "newText": "x"}]},
        ]});
        assert_eq!(
            workspace_edit(&value),
            Err(EditRefusal::NotAFile("https://example.com/lib.rs".into()))
        );
    }

    #[test]
    fn null_and_empty_edits_apply_nothing() {
        for value in [
            Value::Null,
            json!({}),
            json!({"changes": {}}),
            json!({"changes": null, "documentChanges": null}),
            json!({"documentChanges": []}),
            json!({"changes": {"file:///project/lib.rs": []}}),
        ] {
            assert_eq!(workspace_edit(&value), Ok(vec![]), "{value}");
        }
    }

    #[test]
    fn a_malformed_edit_is_refused() {
        for value in [
            json!({"changes": {"file:///project/lib.rs": [
                {"range": {"start": {"line": 0, "character": 0}}, "newText": "x"},
            ]}}),
            json!({"changes": {"file:///project/lib.rs": [
                {"range": {"start": {"line": -1, "character": 0}, "end": {"line": 0, "character": 0}},
                 "newText": "x"},
            ]}}),
            json!({"changes": {"file:///project/lib.rs": [{"range": range(0, 5, 2), "newText": "x"}]}}),
            json!({"changes": {"file:///project/lib.rs": [{"range": range(0, 0, 1)}]}}),
            json!({"changes": {"file:///project/lib.rs": {"range": range(0, 0, 1), "newText": "x"}}}),
            json!({"changes": []}),
            json!({"documentChanges": {}}),
            json!({"documentChanges": [{"edits": []}]}),
            json!({"documentChanges": [{"textDocument": {"uri": "file:///project/lib.rs"}}]}),
            json!({"documentChanges": [{"kind": "move", "uri": "file:///project/lib.rs"}]}),
            json!({"changes": {"not a uri": []}}),
            json!("edit"),
        ] {
            assert!(
                matches!(workspace_edit(&value), Err(EditRefusal::Malformed(_))),
                "{value}"
            );
        }
    }

    #[test]
    fn refusals_read_as_sentences() {
        assert_eq!(
            EditRefusal::FileOperation.to_string(),
            "This change creates, renames or deletes files, which Tandem does not apply."
        );
        assert_eq!(
            EditRefusal::NotAFile("untitled:Untitled-1".into()).to_string(),
            "This change edits untitled:Untitled-1, which is not a file on this computer."
        );
        assert_eq!(
            EditRefusal::OutsideProject("/etc/hosts".into()).to_string(),
            "This change edits /etc/hosts, which is outside the project."
        );
        assert_eq!(
            EditRefusal::Malformed("missing field `end`".into()).to_string(),
            "Could not read the language server's edit: missing field `end`."
        );
    }

    #[test]
    fn command_edits_include_every_collected_edit() {
        let reply = json!({"result": null, "edits": [
            {"changes": {"file:///project/lib.rs": [{"range": range(0, 0, 1), "newText": "a"}]}},
            {"documentChanges": [
                {"textDocument": {"uri": "file:///project/lib.rs", "version": 2},
                 "edits": [{"range": range(3, 0, 1), "newText": "b"}]},
                {"textDocument": {"uri": "file:///project/main.rs", "version": 2},
                 "edits": [{"range": range(1, 0, 0), "newText": "c"}]},
            ]},
        ]});
        assert_eq!(
            command_edits(&reply, Path::new("/project/other.rs")),
            Ok(vec![
                file(
                    "/project/lib.rs",
                    vec![edit(0, 0, 1, "a"), edit(3, 0, 1, "b")]
                ),
                file("/project/main.rs", vec![edit(1, 0, 0, "c")]),
            ])
        );
    }

    #[test]
    fn a_text_edit_list_result_applies_to_the_file() {
        let reply = json!({"result": [
            {"range": range(0, 0, 10), "newText": "import os"},
            {"range": range(1, 0, 11), "newText": "import sys"},
        ], "edits": []});
        assert_eq!(
            command_edits(&reply, Path::new("/project/main.py")),
            Ok(vec![file(
                "/project/main.py",
                vec![edit(0, 0, 10, "import os"), edit(1, 0, 11, "import sys")]
            )])
        );
    }

    #[test]
    fn a_command_result_with_a_workspace_edit_applies_it() {
        let reply = json!({"result": {"label": "Add import", "edits": {"changes": {
            "file:///project/main.py": [{"range": range(0, 0, 0), "newText": "import os\n"}],
        }}}, "edits": []});
        assert_eq!(
            command_edits(&reply, Path::new("/project/other.py")),
            Ok(vec![file(
                "/project/main.py",
                vec![edit(0, 0, 0, "import os\n")]
            )])
        );
    }

    #[test]
    fn a_workspace_edit_result_merges_with_collected_edits_for_the_same_file() {
        let reply = json!({
            "result": {"changes": {"file:///project/lib.rs": [{"range": range(5, 0, 1), "newText": "result"}]}},
            "edits": [
                {"changes": {"file:///project/lib.rs": [{"range": range(0, 0, 1), "newText": "collected"}]}},
            ],
        });
        assert_eq!(
            command_edits(&reply, Path::new("/project/lib.rs")),
            Ok(vec![file(
                "/project/lib.rs",
                vec![edit(0, 0, 1, "collected"), edit(5, 0, 1, "result")]
            )])
        );
    }

    #[test]
    fn a_result_that_is_not_an_edit_applies_nothing() {
        for result in [
            Value::Null,
            json!({"done": true}),
            json!("ok"),
            json!(3),
            json!([]),
            json!(["one", "two"]),
            json!([{"title": "not an edit"}]),
        ] {
            let reply = json!({"result": result, "edits": []});
            assert_eq!(
                command_edits(&reply, Path::new("/project/lib.rs")),
                Ok(vec![]),
                "{result}"
            );
        }
    }

    #[test]
    fn a_refused_collected_edit_refuses_the_command_edits() {
        let reply = json!({
            "result": [{"range": range(0, 0, 1), "newText": "x"}],
            "edits": [{"documentChanges": [{"kind": "delete", "uri": "file:///project/lib.rs"}]}],
        });
        assert_eq!(
            command_edits(&reply, Path::new("/project/lib.rs")),
            Err(EditRefusal::FileOperation)
        );
        let reply =
            json!({"result": [{"range": {"start": {"line": 0}}, "newText": "x"}], "edits": []});
        assert!(matches!(
            command_edits(&reply, Path::new("/project/lib.rs")),
            Err(EditRefusal::Malformed(_))
        ));
    }
}
