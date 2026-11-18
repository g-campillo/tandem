//! Code actions: the quick fixes, refactorings and source actions a language server offers
//! for a range, read from its `textDocument/codeAction` reply.

use lsp_types::{CodeAction, Command, Diagnostic};
use serde_json::Value;
use std::ops::Range;

/// The kind of the action that sorts and cleans up a file's imports.
pub const ORGANIZE_IMPORTS: &str = "source.organizeImports";

/// The command Pyright organizes a file's imports with. Pyright offers no
/// organize-imports code action and lists no commands, so Organize Imports runs this one
/// command by name, for Pyright only.
pub const PYRIGHT_ORGANIZE_IMPORTS: &str = "pyright.organizeimports";

/// Pyright's organize-imports command for the document at `uri`.
pub fn pyright_organize_imports(uri: &str) -> Command {
    Command {
        title: "Organize Imports".into(),
        command: PYRIGHT_ORGANIZE_IMPORTS.into(),
        arguments: Some(vec![Value::String(uri.into())]),
    }
}

/// Params for `textDocument/codeAction` (the server adds `textDocument`): the byte `range`
/// of `text` as a UTF-16 range, and a context with the `diagnostics` the range touches,
/// the user's request as the trigger and, when given, the only kinds wanted.
pub fn code_action_params(
    text: &str,
    range: Range<usize>,
    diagnostics: Vec<Diagnostic>,
    only: Option<&[&str]>,
) -> anyhow::Result<Value> {
    let range = lsp_types::Range::new(
        crate::utf16_position(text, range.start)?,
        crate::utf16_position(text, range.end)?,
    );
    let mut context = serde_json::json!({"diagnostics": diagnostics, "triggerKind": 1});
    if let Some(only) = only {
        context["only"] = serde_json::json!(only);
    }
    Ok(serde_json::json!({"range": range, "context": context}))
}

/// CodeAction | Command entries; Commands become CodeAction { title, command }. Drops
/// `disabled` ones. Order: isPreferred first, then quickfix, refactor*, source*, other;
/// stable within a group. An entry that is neither is skipped.
pub fn code_actions(value: &Value) -> Vec<CodeAction> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut actions: Vec<CodeAction> = entries
        .iter()
        .filter_map(code_action)
        .filter(|action| action.disabled.is_none())
        .collect();
    actions.sort_by_key(group);
    actions
}

/// The organize-imports action among `actions`: the first whose kind is
/// `source.organizeImports` or one of its sub-kinds (`source.organizeImports.ts`).
pub fn organize_imports(actions: &[CodeAction]) -> Option<CodeAction> {
    actions
        .iter()
        .find(|action| is_kind(action, ORGANIZE_IMPORTS))
        .cloned()
}

/// One entry of a code action reply: a `Command` (its `command` is the command's name)
/// becomes an action that runs it; anything else is read as a `CodeAction`.
fn code_action(entry: &Value) -> Option<CodeAction> {
    if entry.get("command").is_some_and(Value::is_string) {
        let command: Command = serde_json::from_value(entry.clone()).ok()?;
        return Some(CodeAction {
            title: command.title.clone(),
            command: Some(command),
            ..CodeAction::default()
        });
    }
    serde_json::from_value(entry.clone()).ok()
}

/// Where an action is listed: preferred ones first, then by kind.
fn group(action: &CodeAction) -> u8 {
    if action.is_preferred == Some(true) {
        0
    } else if is_kind(action, "quickfix") {
        1
    } else if is_kind(action, "refactor") {
        2
    } else if is_kind(action, "source") {
        3
    } else {
        4
    }
}

/// Whether the action's kind is `base` or one of its sub-kinds (`base.something`).
fn is_kind(action: &CodeAction, base: &str) -> bool {
    action.kind.as_ref().is_some_and(|kind| {
        let kind = kind.as_str();
        kind.strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Diagnostic, Position};
    use serde_json::json;

    fn titles(actions: &[CodeAction]) -> Vec<&str> {
        actions.iter().map(|action| action.title.as_str()).collect()
    }

    fn action(title: &str, kind: Option<&str>) -> Value {
        match kind {
            Some(kind) => json!({"title": title, "kind": kind}),
            None => json!({"title": title}),
        }
    }

    #[test]
    fn preferred_actions_come_first_then_quick_fixes_refactorings_source_actions_and_the_rest() {
        let reply = json!([
            action("other", None),
            action("source", Some("source.fixAll")),
            action("extract", Some("refactor.extract")),
            action("fix", Some("quickfix")),
            {"title": "preferred refactor", "kind": "refactor.rewrite", "isPreferred": true},
            action("empty kind", Some("")),
            action("refactor", Some("refactor")),
            action("organize", Some("source.organizeImports")),
            action("nested fix", Some("quickfix.import")),
            {"title": "preferred fix", "kind": "quickfix", "isPreferred": true},
            action("lookalike", Some("refactoring")),
        ]);
        assert_eq!(
            titles(&code_actions(&reply)),
            [
                "preferred refactor",
                "preferred fix",
                "fix",
                "nested fix",
                "extract",
                "refactor",
                "source",
                "organize",
                "other",
                "empty kind",
                "lookalike",
            ]
        );
    }

    #[test]
    fn a_preference_set_to_false_keeps_the_kind_order() {
        let reply = json!([
            {"title": "refactor", "kind": "refactor", "isPreferred": false},
            {"title": "fix", "kind": "quickfix", "isPreferred": false},
        ]);
        assert_eq!(titles(&code_actions(&reply)), ["fix", "refactor"]);
    }

    #[test]
    fn a_command_becomes_an_action_that_runs_it() {
        let reply = json!([
            {"title": "Organize Imports", "command": "pyright.organizeimports",
             "arguments": ["file:///project/main.py"]},
            {"title": "Fix", "kind": "quickfix",
             "command": {"title": "Fix", "command": "server.fix", "arguments": [1]}},
        ]);
        let actions = code_actions(&reply);
        assert_eq!(titles(&actions), ["Fix", "Organize Imports"]);
        let converted = &actions[1];
        assert_eq!(converted.kind, None);
        assert_eq!(converted.edit, None);
        let command = converted.command.as_ref().expect("the command to run");
        assert_eq!(command.title, "Organize Imports");
        assert_eq!(command.command, "pyright.organizeimports");
        assert_eq!(
            command.arguments,
            Some(vec![json!("file:///project/main.py")])
        );
        let kept = actions[0].command.as_ref().expect("the action's command");
        assert_eq!(kept.command, "server.fix");
    }

    #[test]
    fn an_action_keeps_its_edit_and_its_data() {
        let reply = json!([{
            "title": "Add import",
            "kind": "quickfix",
            "edit": {"changes": {"file:///project/main.py": [
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                 "newText": "import os\n"}
            ]}},
            "data": {"id": "add_import:0"},
        }]);
        let actions = code_actions(&reply);
        assert_eq!(actions.len(), 1);
        assert!(actions[0].edit.is_some());
        assert_eq!(actions[0].data, Some(json!({"id": "add_import:0"})));
    }

    #[test]
    fn disabled_actions_are_dropped() {
        let reply = json!([
            {"title": "Extract", "kind": "refactor.extract", "disabled": {"reason": "Select an expression"}},
            action("Inline", Some("refactor.inline")),
        ]);
        assert_eq!(titles(&code_actions(&reply)), ["Inline"]);
    }

    #[test]
    fn a_reply_without_actions_offers_none() {
        for reply in [
            Value::Null,
            json!([]),
            json!({"title": "not a list"}),
            json!([42, "text", {"kind": "quickfix"}]),
        ] {
            assert!(code_actions(&reply).is_empty(), "{reply}");
        }
    }

    #[test]
    fn a_malformed_entry_is_skipped_and_the_rest_kept() {
        let reply = json!([
            {"title": 3},
            action("Fix", Some("quickfix")),
        ]);
        assert_eq!(titles(&code_actions(&reply)), ["Fix"]);
    }

    #[test]
    fn organize_imports_takes_the_first_organize_imports_kind() {
        let actions = code_actions(&json!([
            action("Fix all", Some("source.fixAll")),
            action("Sort imports", Some("source.organizeImportsLookalike")),
            action("Organize Imports", Some("source.organizeImports.ts")),
            action("Organize again", Some("source.organizeImports")),
        ]));
        assert_eq!(
            organize_imports(&actions).map(|action| action.title),
            Some("Organize Imports".to_string())
        );
        let plain = code_actions(&json!([action("Organize", Some("source.organizeImports"))]));
        assert_eq!(
            organize_imports(&plain).map(|action| action.title),
            Some("Organize".to_string())
        );
    }

    #[test]
    fn params_name_the_range_in_utf16_with_the_diagnostics_and_an_invoked_trigger() {
        let text = "let 🦀 = 1;\nlet x = 🦀;\n";
        let start = text.find("x =").unwrap();
        let diagnostic = Diagnostic {
            range: lsp_types::Range::new(Position::new(1, 4), Position::new(1, 5)),
            message: "unused".into(),
            ..Diagnostic::default()
        };
        let params =
            code_action_params(text, start..text.len() - 2, vec![diagnostic], None).unwrap();
        assert_eq!(
            params,
            json!({
                "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 10}},
                "context": {
                    "diagnostics": [{"range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 5}},
                                     "message": "unused"}],
                    "triggerKind": 1,
                },
            })
        );
    }

    #[test]
    fn params_ask_only_for_the_kinds_given() {
        let text = "import sys\nimport os\n";
        let params =
            code_action_params(text, 0..text.len(), vec![], Some(&[ORGANIZE_IMPORTS])).unwrap();
        assert_eq!(
            params,
            json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}},
                "context": {"diagnostics": [], "only": ["source.organizeImports"], "triggerKind": 1},
            })
        );
    }

    #[test]
    fn params_for_a_range_outside_the_text_are_an_error() {
        assert!(code_action_params("abc", 1..9, vec![], None).is_err());
        assert!(code_action_params("é", 1..2, vec![], None).is_err());
    }

    #[test]
    fn organize_imports_is_none_without_such_an_action() {
        let actions = code_actions(&json!([
            action("Fix", Some("quickfix")),
            action("Source", Some("source")),
            action("Untyped", None),
        ]));
        assert_eq!(organize_imports(&actions), None);
        assert_eq!(organize_imports(&[]), None);
    }

    #[test]
    fn pyrights_organize_imports_command_names_the_document() {
        let command = pyright_organize_imports("file:///project/main.py");
        assert_eq!(command.command, "pyright.organizeimports");
        assert_eq!(command.command, PYRIGHT_ORGANIZE_IMPORTS);
        assert_eq!(command.title, "Organize Imports");
        assert_eq!(
            command.arguments,
            Some(vec![json!("file:///project/main.py")])
        );
    }
}
