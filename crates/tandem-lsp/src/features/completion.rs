//! The completion list's logic: the candidates offered at the cursor (a server's items,
//! snippets, the options of a snippet choice), how they rank against what was typed, and
//! what accepting one changes in the text.
use crate::{position_params, utf16_offset, utf16_position};
use anyhow::{Context, Result, ensure};
use lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemTag, CompletionResponse,
    CompletionTextEdit, CompletionTriggerKind, InsertTextFormat, Position, TextEdit,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ops::Range;
use tandem_core::edit::fuzzy::{FuzzyMatch, fuzzy_match};
use tandem_core::edit::indent::IndentUnit;
use tandem_core::edit::snippet::library::SnippetDef;
use tandem_core::edit::snippet::session::SnippetSession;
use tandem_core::edit::snippet::{self, Expansion, SnippetIndent};
use tandem_core::edit::{Edit, indent_len, line_start};

/// Where a candidate comes from: an index into the server's items, into the snippets, or
/// into the active snippet stop's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Server(usize),
    Snippet(usize),
    Choice(usize),
}

/// One entry the completion list can show. `start` is the byte offset where the text it
/// is matched against begins; the query is the text from there to the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub label: String,
    pub label_detail: Option<String>,
    pub description: Option<String>,
    pub kind: Option<CompletionItemKind>,
    pub filter: String,
    pub sort: String,
    pub preselect: bool,
    pub deprecated: bool,
    pub start: usize,
    pub source: Source,
}

/// A candidate that matches what was typed: its index in the candidates, its score, and
/// the byte ranges of its label to highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub candidate: usize,
    pub score: i32,
    pub matches: Vec<Range<usize>>,
}

/// Word characters: alphanumeric and `_`; also `$` for javascript, javascriptreact,
/// typescript, typescriptreact, vue, svelte and scss; `-` for css, scss, less and html;
/// `@` for less. Prefixes the word rules miss (`@click`, `v-if` and `:prop` in Vue and
/// Svelte templates) come from the server's textEdit range in `server_candidates`.
pub fn is_word_char(c: char, language: &str) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || (c == '$'
            && matches!(
                language,
                "javascript"
                    | "javascriptreact"
                    | "typescript"
                    | "typescriptreact"
                    | "vue"
                    | "svelte"
                    | "scss"
            ))
        || (c == '-' && matches!(language, "css" | "scss" | "less" | "html"))
        || (c == '@' && language == "less")
}

/// The start of the run of word characters that ends at `cursor`.
pub fn word_start(text: &str, cursor: usize, language: &str) -> usize {
    let cursor = text.floor_char_boundary(cursor);
    let word: usize = text[..cursor]
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c, language))
        .map(char::len_utf8)
        .sum();
    cursor - word
}

/// Items and `isIncomplete` of a textDocument/completion reply (null → empty, complete).
pub fn completion_items(value: Value) -> Result<(Vec<CompletionItem>, bool)> {
    if value.is_null() {
        return Ok((Vec::new(), false));
    }
    let response: CompletionResponse = serde_json::from_value(value)
        .context("Could not read the language server's completions")?;
    Ok(match response {
        CompletionResponse::Array(items) => (items, false),
        CompletionResponse::List(list) => (list.items, list.is_incomplete),
    })
}

/// Params for textDocument/completion: position plus `context`: triggerKind 3
/// (TriggerForIncompleteCompletions) when `incomplete_retrigger`; else 2 with the
/// character when `typed` is one of `triggers`; else 1 (Invoked).
pub fn completion_params(
    text: &str,
    cursor: usize,
    typed: Option<char>,
    triggers: &[String],
    incomplete_retrigger: bool,
) -> Result<Value> {
    let mut params = position_params(text, cursor)?;
    let trigger = typed
        .map(String::from)
        .filter(|typed| triggers.contains(typed));
    params["context"] = match (incomplete_retrigger, trigger) {
        (true, _) => {
            json!({"triggerKind": CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS})
        }
        (false, Some(character)) => json!({
            "triggerKind": CompletionTriggerKind::TRIGGER_CHARACTER,
            "triggerCharacter": character,
        }),
        (false, None) => json!({"triggerKind": CompletionTriggerKind::INVOKED}),
    };
    Ok(params)
}

/// Server items: start = the item's textEdit range start when it is at or before the
/// cursor (even when it lies before `word_start`, so `background-c`, `aria-l`, `@cl`,
/// `v-i` and `$va` are matched whole), else `word_start`; label_detail = labelDetails.detail;
/// description = labelDetails.description or detail; deprecated from `deprecated` or tag 1;
/// filter = filterText or label; sort = sortText or label.
pub fn server_candidates(
    text: &str,
    cursor: usize,
    word_start: usize,
    items: &[CompletionItem],
) -> Vec<Candidate> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let start = item
                .text_edit
                .as_ref()
                .and_then(|edit| utf16_offset(text, main_range(edit).start).ok())
                .filter(|start| *start <= cursor)
                .unwrap_or(word_start);
            let details = item.label_details.as_ref();
            Candidate {
                label: item.label.clone(),
                label_detail: details.and_then(|details| details.detail.clone()),
                description: details
                    .and_then(|details| details.description.clone())
                    .or_else(|| item.detail.clone()),
                kind: item.kind,
                filter: item
                    .filter_text
                    .clone()
                    .unwrap_or_else(|| item.label.clone()),
                sort: item.sort_text.clone().unwrap_or_else(|| item.label.clone()),
                preselect: item.preselect == Some(true),
                deprecated: item.deprecated == Some(true)
                    || item
                        .tags
                        .as_ref()
                        .is_some_and(|tags| tags.contains(&CompletionItemTag::DEPRECATED)),
                start,
                source: Source::Server(index),
            }
        })
        .collect()
}

/// One candidate per snippet prefix: label = prefix, description = description or name,
/// kind Snippet, sort = "~" + prefix (after server items on ties).
pub fn snippet_candidates(word_start: usize, snippets: &[SnippetDef]) -> Vec<Candidate> {
    snippets
        .iter()
        .enumerate()
        .flat_map(|(index, snippet)| {
            let description = [&snippet.description, &snippet.name]
                .into_iter()
                .find(|text| !text.is_empty())
                .cloned();
            snippet.prefixes.iter().map(move |prefix| Candidate {
                label: prefix.clone(),
                label_detail: None,
                description: description.clone(),
                kind: Some(CompletionItemKind::SNIPPET),
                filter: prefix.clone(),
                sort: format!("~{prefix}"),
                preselect: false,
                deprecated: false,
                start: word_start,
                source: Source::Snippet(index),
            })
        })
        .collect()
}

/// One candidate per option of a snippet choice, kind Value, sorted in the snippet's order.
pub fn choice_candidates(start: usize, choices: &[String]) -> Vec<Candidate> {
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| Candidate {
            label: choice.clone(),
            label_detail: None,
            description: None,
            kind: Some(CompletionItemKind::VALUE),
            filter: choice.clone(),
            sort: format!("{index:010}"),
            preselect: false,
            deprecated: false,
            start,
            source: Source::Choice(index),
        })
        .collect()
}

/// Ranks by `fuzzy_match(text[start..cursor], filter)`; drops non-matches; `matches`
/// come from `fuzzy_match(query, label)` (empty when the label does not match). Order:
/// score desc, preselect first, sort asc, label length asc, label asc. At most `limit`.
pub fn rank(text: &str, cursor: usize, candidates: &[Candidate], limit: usize) -> Vec<Ranked> {
    let query = |candidate: &Candidate| text.get(candidate.start..cursor);
    let mut found: Vec<(usize, FuzzyMatch)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            Some((index, fuzzy_match(query(candidate)?, &candidate.filter)?))
        })
        .collect();
    found.sort_by(|(a, a_match), (b, b_match)| {
        let (a, b) = (&candidates[*a], &candidates[*b]);
        b_match
            .score
            .cmp(&a_match.score)
            .then(b.preselect.cmp(&a.preselect))
            .then_with(|| a.sort.cmp(&b.sort))
            .then(a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(&b.label))
    });
    found.truncate(limit);
    found
        .into_iter()
        .map(|(index, filter_match)| {
            let candidate = &candidates[index];
            let matches = if candidate.filter == candidate.label {
                filter_match.ranges
            } else {
                query(candidate)
                    .and_then(|query| fuzzy_match(query, &candidate.label))
                    .map(|label_match| label_match.ranges)
                    .unwrap_or_default()
            };
            Ranked {
                candidate: index,
                score: filter_match.score,
                matches,
            }
        })
        .collect()
}

/// What accepting a candidate does: one edit of the text, the selection after it, the
/// snippet session to continue with, and the server command to run afterwards.
#[derive(Clone, Debug)]
pub struct AcceptPlan {
    pub edit: Edit,
    pub selection: Range<usize>,
    pub session: Option<SnippetSession>,
    pub command: Option<Command>,
}

/// The edit that accepting a server item makes in `text` (the text now, with the cursor
/// at `cursor`; the completion was requested with the cursor at `request_cursor`).
///
/// The item replaces its textEdit range (the `replace` range of an insert-and-replace
/// edit), extended to the cursor when it ends where the request was made and the user
/// typed on since; without a textEdit, the word from `word_start` to the cursor. Its text
/// (newText, else insertText, else label) is expanded as a snippet when the item says so,
/// indented like the line it goes on. Its additionalTextEdits are applied with it, except
/// those that overlap that range or start at or after the request cursor (the text there
/// may have changed since). The selection is the snippet's first stop (with the session
/// that follows it), else where `$0` or the inserted text ends.
///
/// When text was deleted since the request (`cursor < request_cursor`), the server's
/// positions from the cursor on no longer hold: a range edge there becomes the cursor, and
/// additional edits that start there are dropped.
pub fn accept_item(
    text: &str,
    cursor: usize,
    request_cursor: usize,
    word_start: usize,
    item: &CompletionItem,
    variables: &HashMap<String, String>,
    unit: IndentUnit,
) -> Result<AcceptPlan> {
    let deleted = cursor < request_cursor;
    let cursor_position = utf16_position(text, cursor)?;
    let offset = |position: Position| {
        if deleted && position >= cursor_position {
            Ok(cursor)
        } else {
            utf16_offset(text, position)
        }
    };
    let (range, new_text) = match &item.text_edit {
        Some(edit) => {
            let lsp_range = main_range(edit);
            let start = offset(lsp_range.start)?;
            let mut end = offset(lsp_range.end)?;
            if end == request_cursor && cursor > request_cursor {
                end = cursor;
            }
            ensure!(start <= end, "The completion's range is reversed");
            let new_text = match edit {
                CompletionTextEdit::Edit(edit) => &edit.new_text,
                CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
            };
            (start..end, new_text.as_str())
        }
        None => {
            ensure!(
                word_start <= cursor && text.is_char_boundary(word_start),
                "The completion's word is not before the cursor"
            );
            let new_text = item.insert_text.as_deref().unwrap_or(&item.label);
            (word_start..cursor, new_text)
        }
    };
    let additional = additional_edits(
        text,
        item.additional_text_edits.as_deref().unwrap_or_default(),
        &range,
        cursor.min(request_cursor),
    )?;
    let insert = if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
        Insert::Snippet(expand_at(text, range.start, new_text, variables, unit))
    } else {
        Insert::Plain(new_text)
    };
    Ok(plan(text, range, insert, additional, item.command.clone()))
}

/// The edit that accepting a snippet makes: `body` replaces the word from `word_start` to
/// the cursor, expanded and indented like the line it goes on.
pub fn accept_snippet(
    text: &str,
    cursor: usize,
    word_start: usize,
    body: &str,
    variables: &HashMap<String, String>,
    unit: IndentUnit,
) -> AcceptPlan {
    let cursor = text.floor_char_boundary(cursor);
    let word_start = text.floor_char_boundary(word_start.min(cursor));
    let expansion = expand_at(text, word_start, body, variables, unit);
    plan(
        text,
        word_start..cursor,
        Insert::Snippet(expansion),
        Vec::new(),
        None,
    )
}

/// The range an item's textEdit replaces: the `replace` range of an insert-and-replace edit.
fn main_range(edit: &CompletionTextEdit) -> lsp_types::Range {
    match edit {
        CompletionTextEdit::Edit(edit) => edit.range,
        CompletionTextEdit::InsertAndReplace(edit) => edit.replace,
    }
}

/// `edits` as byte ranges in `text`, in the server's order, without those that start at
/// or after `stale` (where the text may differ from what the server saw) or overlap
/// `main`. Edits that overlap each other are an error.
fn additional_edits<'a>(
    text: &str,
    edits: &'a [TextEdit],
    main: &Range<usize>,
    stale: usize,
) -> Result<Vec<(Range<usize>, &'a str)>> {
    let stale = utf16_position(text, stale)?;
    let mut kept: Vec<(Range<usize>, &str)> = Vec::new();
    for edit in edits {
        if edit.range.start >= stale {
            continue;
        }
        let start = utf16_offset(text, edit.range.start)?;
        let end = utf16_offset(text, edit.range.end)?;
        ensure!(start <= end, "Overlapping or reversed language edits");
        let range = start..end;
        if overlaps(&range, main) {
            continue;
        }
        ensure!(
            !kept.iter().any(|(other, _)| overlaps(other, &range)),
            "Overlapping or reversed language edits"
        );
        kept.push((range, edit.new_text.as_str()));
    }
    Ok(kept)
}

/// Whether two ranges share text. An insertion overlaps only a range it lies strictly inside.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// What an accepted candidate inserts: text as it is, or an expanded snippet.
enum Insert<'a> {
    Plain(&'a str),
    Snippet(Expansion),
}

/// `body` expanded for insertion at `at`: its lines indented like the line `at` is on.
fn expand_at(
    text: &str,
    at: usize,
    body: &str,
    variables: &HashMap<String, String>,
    unit: IndentUnit,
) -> Expansion {
    let line = line_start(text, at);
    let indent = SnippetIndent {
        line: text[line..line + indent_len(&text[line..])].to_string(),
        unit: unit.text(),
    };
    snippet::expand(body, variables, &indent)
}

/// The plan for replacing `range` by `insert` together with the `additional` edits, which
/// overlap neither it nor each other.
fn plan(
    text: &str,
    range: Range<usize>,
    insert: Insert,
    additional: Vec<(Range<usize>, &str)>,
    command: Option<Command>,
) -> AcceptPlan {
    let inserted = match &insert {
        Insert::Plain(inserted) => inserted,
        Insert::Snippet(expansion) => expansion.text.as_str(),
    };
    // At equal starts an insertion goes before a replacement, and an additional edit
    // before the item's text; additional edits keep the server's order among themselves.
    let mut replacements: Vec<(Range<usize>, &str, bool)> = additional
        .into_iter()
        .map(|(range, text)| (range, text, false))
        .chain([(range, inserted, true)])
        .collect();
    replacements.sort_by_key(|(range, _, main)| (range.start, range.end, *main));
    let mut new = String::with_capacity(text.len() + inserted.len());
    let mut copied = 0;
    let mut insert_at = 0;
    for (range, replacement, main) in &replacements {
        new.push_str(&text[copied..range.start]);
        if *main {
            insert_at = new.len();
        }
        new.push_str(replacement);
        copied = range.end;
    }
    new.push_str(&text[copied..]);

    let (selection, session) = match &insert {
        Insert::Plain(inserted) => {
            let end = insert_at + inserted.len();
            (end..end, None)
        }
        Insert::Snippet(expansion) => match SnippetSession::start(expansion, insert_at) {
            Some((session, selection)) => (selection, Some(session)),
            None => {
                let end = insert_at + expansion.final_offset;
                (end..end, None)
            }
        },
    };
    AcceptPlan {
        edit: Edit::between(text, &new, selection.clone()),
        selection,
        session,
        command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SPACES: IndentUnit = IndentUnit {
        width: 4,
        hard_tabs: false,
    };

    fn item(value: Value) -> CompletionItem {
        serde_json::from_value(value).unwrap()
    }

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    fn candidate(label: &str, start: usize) -> Candidate {
        Candidate {
            label: label.into(),
            label_detail: None,
            description: None,
            kind: None,
            filter: label.into(),
            sort: label.into(),
            preselect: false,
            deprecated: false,
            start,
            source: Source::Server(0),
        }
    }

    fn labels(candidates: &[Candidate], ranked: &[Ranked]) -> Vec<String> {
        ranked
            .iter()
            .map(|ranked| candidates[ranked.candidate].label.clone())
            .collect()
    }

    /// The plan's edit applied to `text`, and the text its selection covers.
    fn applied(text: &str, plan: &AcceptPlan) -> (String, String) {
        let new = plan.edit.apply(text);
        assert_eq!(plan.edit.selection, plan.selection);
        let selected = new[plan.selection.clone()].to_string();
        (new, selected)
    }

    #[test]
    fn choice_candidates_keep_the_snippet_order_and_are_values() {
        let choices = ["post".to_string(), "get".into(), "a".into()];
        let candidates = choice_candidates(7, &choices);
        assert_eq!(candidates.len(), 3);
        for (index, candidate) in candidates.iter().enumerate() {
            assert_eq!(candidate.label, choices[index]);
            assert_eq!(candidate.source, Source::Choice(index));
            assert_eq!(candidate.kind, Some(CompletionItemKind::VALUE));
            assert_eq!(candidate.start, 7);
        }
        let ranked = rank("method=", 7, &candidates, 10);
        assert_eq!(labels(&candidates, &ranked), ["post", "get", "a"]);
    }

    #[test]
    fn word_start_follows_each_languages_word_characters() {
        assert_eq!(word_start("foo.ba", 6, "rust"), 4);
        assert_eq!(word_start("this.$el", 8, "typescript"), 5);
        assert_eq!(word_start("a { --ma", 8, "css"), 4);
        assert_eq!(word_start("  background-c", 14, "scss"), 2);
        assert_eq!(word_start("  background-c", 14, "less"), 2);
        assert_eq!(word_start("<a aria-l", 9, "html"), 3);
        assert_eq!(word_start("a: $va", 6, "scss"), 3);
        assert_eq!(word_start("@co", 3, "less"), 0);
        assert_eq!(word_start("let $va", 7, "rust"), 5);
        assert_eq!(word_start("a-bc", 4, "rust"), 2);
        assert_eq!(word_start("@cl", 3, "vue"), 1);
        assert_eq!(word_start("", 0, "rust"), 0);
        assert_eq!(word_start("é_x", 4, "rust"), 0);
        assert!(!is_word_char('-', "typescript"));
        assert!(is_word_char('$', "svelte"));
    }

    #[test]
    fn css_property_prefixes_are_matched_whole_from_the_servers_range() {
        let text = "a {\n  background-c";
        let cursor = text.len();
        let items = [
            item(
                json!({"label": "color", "textEdit": {"range": range(1, 2, 14), "newText": "color"}}),
            ),
            item(
                json!({"label": "background-attachment", "textEdit": {"range": range(1, 2, 14), "newText": "background-attachment"}}),
            ),
            item(
                json!({"label": "background-color", "textEdit": {"range": range(1, 2, 14), "newText": "background-color: $0;"}, "insertTextFormat": 2}),
            ),
            item(
                json!({"label": "border-color", "textEdit": {"range": range(1, 2, 14), "newText": "border-color"}}),
            ),
        ];
        let start = word_start(text, cursor, "css");
        let candidates = server_candidates(text, cursor, start, &items);
        assert!(candidates.iter().all(|candidate| candidate.start == 6));
        let ranked = rank(text, cursor, &candidates, 10);
        assert_eq!(
            labels(&candidates, &ranked),
            ["background-color", "background-attachment"]
        );
        assert_eq!(ranked[0].matches, vec![0..12]);
    }

    #[test]
    fn vue_template_prefixes_come_from_the_servers_range() {
        let text = "<button @cl";
        let cursor = text.len();
        let items = [
            item(
                json!({"label": "@click", "textEdit": {"range": range(0, 8, 11), "newText": "@click"}}),
            ),
            item(
                json!({"label": "class", "textEdit": {"range": range(0, 8, 11), "newText": "class"}}),
            ),
        ];
        let start = word_start(text, cursor, "vue");
        assert_eq!(start, 9);
        let candidates = server_candidates(text, cursor, start, &items);
        assert_eq!(candidates[0].start, 8);
        let ranked = rank(text, cursor, &candidates, 10);
        assert_eq!(labels(&candidates, &ranked), ["@click"]);
        assert_eq!(ranked[0].matches, vec![0..3]);

        let text = "<div v-i";
        let cursor = text.len();
        let items = [
            item(
                json!({"label": "v-for", "textEdit": {"range": range(0, 5, 8), "newText": "v-for"}}),
            ),
            item(
                json!({"label": "v-if", "textEdit": {"range": range(0, 5, 8), "newText": "v-if"}}),
            ),
        ];
        let start = word_start(text, cursor, "vue");
        assert_eq!(start, 7);
        let candidates = server_candidates(text, cursor, start, &items);
        let ranked = rank(text, cursor, &candidates, 10);
        assert_eq!(labels(&candidates, &ranked), ["v-if"]);
        assert_eq!(ranked[0].matches, vec![0..3]);
    }

    #[test]
    fn completion_items_read_arrays_lists_and_null() {
        let (items, incomplete) =
            completion_items(json!([{"label": "a"}, {"label": "b"}])).unwrap();
        assert_eq!(items.len(), 2);
        assert!(!incomplete);
        let (items, incomplete) =
            completion_items(json!({"isIncomplete": true, "items": [{"label": "a"}]})).unwrap();
        assert_eq!(items[0].label, "a");
        assert!(incomplete);
        let (items, incomplete) = completion_items(Value::Null).unwrap();
        assert!(items.is_empty());
        assert!(!incomplete);
        assert!(completion_items(json!({"items": 3})).is_err());
    }

    #[test]
    fn completion_params_carry_the_trigger_kind() {
        let triggers = [".".to_string(), ":".into()];
        let text = "self.";
        let invoked = completion_params(text, 5, None, &triggers, false).unwrap();
        assert_eq!(invoked["position"], json!({"line": 0, "character": 5}));
        assert_eq!(invoked["context"], json!({"triggerKind": 1}));
        let typed_word = completion_params(text, 4, Some('f'), &triggers, false).unwrap();
        assert_eq!(typed_word["context"], json!({"triggerKind": 1}));
        let triggered = completion_params(text, 5, Some('.'), &triggers, false).unwrap();
        assert_eq!(
            triggered["context"],
            json!({"triggerKind": 2, "triggerCharacter": "."})
        );
        let retrigger = completion_params(text, 5, Some('.'), &triggers, true).unwrap();
        assert_eq!(retrigger["context"], json!({"triggerKind": 3}));
        assert!(completion_params(text, 9, None, &triggers, false).is_err());
    }

    #[test]
    fn server_candidates_read_starts_details_and_deprecation() {
        let text = "x.fo\nyy";
        let items = [
            item(json!({
                "label": "foo", "detail": "fn foo()", "kind": 3,
                "labelDetails": {"detail": "(…)", "description": "fn()"},
                "textEdit": {"range": range(0, 0, 4), "newText": "x.foo"},
            })),
            item(json!({
                "label": "fob", "detail": "i32", "tags": [1], "sortText": "0001",
                "filterText": "fobx",
            })),
            item(json!({"label": "old", "deprecated": true, "preselect": true})),
            item(
                json!({"label": "later", "textEdit": {"range": range(1, 0, 1), "newText": "later"}}),
            ),
            item(json!({
                "label": "replace",
                "textEdit": {"newText": "replace", "insert": range(0, 2, 4), "replace": range(0, 1, 4)},
            })),
        ];
        let candidates = server_candidates(text, 4, 2, &items);
        let foo = &candidates[0];
        assert_eq!(foo.start, 0);
        assert_eq!(foo.label_detail.as_deref(), Some("(…)"));
        assert_eq!(foo.description.as_deref(), Some("fn()"));
        assert_eq!(foo.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!((foo.filter.as_str(), foo.sort.as_str()), ("foo", "foo"));
        assert!(!foo.deprecated && !foo.preselect);
        assert_eq!(foo.source, Source::Server(0));
        let fob = &candidates[1];
        assert_eq!(fob.start, 2);
        assert_eq!(fob.label_detail, None);
        assert_eq!(fob.description.as_deref(), Some("i32"));
        assert!(fob.deprecated);
        assert_eq!((fob.filter.as_str(), fob.sort.as_str()), ("fobx", "0001"));
        let old = &candidates[2];
        assert!(old.deprecated && old.preselect);
        assert_eq!(old.description, None);
        assert_eq!(candidates[3].start, 2);
        assert_eq!(candidates[4].start, 1);
        assert_eq!(candidates[4].source, Source::Server(4));
    }

    #[test]
    fn snippet_candidates_offer_every_prefix_after_server_items() {
        let snippets = [
            SnippetDef {
                name: "Function".into(),
                prefixes: vec!["fn".into(), "func".into()],
                body: "fn $1() {}".into(),
                description: String::new(),
                user: false,
            },
            SnippetDef {
                name: "Test".into(),
                prefixes: vec!["test".into()],
                body: "#[test]".into(),
                description: "A test function".into(),
                user: true,
            },
        ];
        let candidates = snippet_candidates(3, &snippets);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[1].label, "func");
        assert_eq!(candidates[1].filter, "func");
        assert_eq!(candidates[1].sort, "~func");
        assert_eq!(candidates[1].description.as_deref(), Some("Function"));
        assert_eq!(candidates[1].kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(candidates[1].source, Source::Snippet(0));
        assert_eq!(candidates[1].start, 3);
        assert_eq!(
            candidates[2].description.as_deref(),
            Some("A test function")
        );
        assert_eq!(candidates[2].source, Source::Snippet(1));

        let text = "fn";
        let mut all = vec![candidate("fn", 0)];
        all.extend(snippet_candidates(0, &snippets));
        let ranked = rank(text, 2, &all, 10);
        assert_eq!(all[ranked[0].candidate].source, Source::Server(0));
        assert_eq!(all[ranked[1].candidate].source, Source::Snippet(0));
    }

    #[test]
    fn rank_orders_by_score_then_preselect_sort_and_label() {
        let text = "len";
        let candidates = [candidate("get_len", 0), candidate("length", 0)];
        assert_eq!(
            labels(&candidates, &rank(text, 3, &candidates, 10)),
            ["length", "get_len"]
        );

        let text = "foo";
        let mut preselected = candidate("foo_b", 0);
        preselected.preselect = true;
        let candidates = [candidate("foo_a", 0), preselected];
        assert_eq!(
            labels(&candidates, &rank(text, 3, &candidates, 10)),
            ["foo_b", "foo_a"]
        );

        let mut first = candidate("foo_d", 0);
        first.sort = "1".into();
        let mut second = candidate("foo_c", 0);
        second.sort = "2".into();
        let candidates = [second, first];
        assert_eq!(
            labels(&candidates, &rank(text, 3, &candidates, 10)),
            ["foo_d", "foo_c"]
        );

        let (mut longer, mut shorter) = (candidate("foo_bb", 0), candidate("foo_c", 0));
        longer.sort = "x".into();
        shorter.sort = "x".into();
        let candidates = [longer, shorter];
        assert_eq!(
            labels(&candidates, &rank(text, 3, &candidates, 10)),
            ["foo_c", "foo_bb"]
        );

        let candidates = [
            candidate("foo_a", 0),
            candidate("foo_b", 0),
            candidate("foo_c", 0),
        ];
        assert_eq!(rank(text, 3, &candidates, 2).len(), 2);
        assert!(rank(text, 3, &candidates, 0).is_empty());
    }

    #[test]
    fn rank_matches_each_candidate_from_its_own_start() {
        let text = "a.b";
        let mut aliased = candidate("Bar (alias)", 2);
        aliased.filter = "bar".into();
        let candidates = [
            candidate("bar", 2),
            candidate("a.bc", 0),
            candidate("bar_too", 0),
            candidate("later", 4),
            aliased,
        ];
        let ranked = rank(text, 3, &candidates, 10);
        let found = labels(&candidates, &ranked);
        assert_eq!(found.len(), 3);
        assert!(found.contains(&"bar".to_string()));
        assert!(found.contains(&"a.bc".to_string()));
        let a_bc = ranked.iter().find(|r| r.candidate == 1).unwrap();
        assert_eq!(a_bc.matches, vec![0..3]);
        let alias = ranked.iter().find(|r| r.candidate == 4).unwrap();
        assert_eq!(alias.matches, vec![0..1]);

        let mut unmatched_label = candidate("Zed", 2);
        unmatched_label.filter = "bar".into();
        let ranked = rank(text, 3, &[unmatched_label], 10);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].matches.is_empty());
    }

    #[test]
    fn accepting_an_item_without_a_text_edit_replaces_the_word() {
        let text = "let x = fo;";
        let plan = accept_item(
            text,
            10,
            10,
            8,
            &item(json!({"label": "foo"})),
            &HashMap::new(),
            SPACES,
        )
        .unwrap();
        let (new, _) = applied(text, &plan);
        assert_eq!(new, "let x = foo;");
        assert_eq!(plan.selection, 11..11);
        assert!(plan.session.is_none() && plan.command.is_none());

        let with_insert_text = item(json!({"label": "foo", "insertText": "foo()"}));
        let plan =
            accept_item(text, 10, 10, 8, &with_insert_text, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied(text, &plan).0, "let x = foo();");
        assert_eq!(plan.selection, 13..13);
    }

    #[test]
    fn accepting_an_item_replaces_its_text_edit_range() {
        let text = "a {\n  background-c\n}";
        let cursor = 18;
        let color = item(json!({
            "label": "background-color", "insertTextFormat": 2,
            "textEdit": {"range": range(1, 2, 14), "newText": "background-color: $0;"},
        }));
        let start = word_start(text, cursor, "css");
        let plan =
            accept_item(text, cursor, cursor, start, &color, &HashMap::new(), SPACES).unwrap();
        let (new, _) = applied(text, &plan);
        assert_eq!(new, "a {\n  background-color: ;\n}");
        assert_eq!(plan.selection, 24..24);
        assert!(plan.session.is_none());

        let replace = item(json!({
            "label": "foobar",
            "textEdit": {"newText": "foobar", "insert": range(0, 0, 2), "replace": range(0, 0, 4)},
        }));
        let plan = accept_item("fozz;", 2, 2, 0, &replace, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied("fozz;", &plan).0, "foobar;");
        assert_eq!(plan.selection, 6..6);
    }

    #[test]
    fn a_range_ending_at_the_request_cursor_covers_what_was_typed_since() {
        // Requested after "fo"; "o" was typed before the reply was accepted.
        let text = "x = foo;";
        let typed = item(json!({
            "label": "foobar",
            "textEdit": {"range": range(0, 4, 6), "newText": "foobar"},
        }));
        let plan = accept_item(text, 7, 6, 4, &typed, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied(text, &plan).0, "x = foobar;");
        assert_eq!(plan.selection, 10..10);

        // Without a text edit the word up to the cursor is replaced.
        let plain = item(json!({"label": "foobar"}));
        let plan = accept_item(text, 7, 6, 4, &plain, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied(text, &plan).0, "x = foobar;");
    }

    #[test]
    fn a_range_past_the_cursor_ends_there_after_a_deletion() {
        // Requested after "foo"; "o" was deleted before the reply was accepted, so the
        // server's positions from the cursor on no longer hold.
        let text = "x = fo;\n";
        let deleted = item(json!({
            "label": "foobar",
            "textEdit": {"range": range(0, 4, 7), "newText": "foobar"},
            "additionalTextEdits": [{"range": range(0, 6, 8), "newText": "stale"}],
        }));
        let plan = accept_item(text, 6, 7, 4, &deleted, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied(text, &plan).0, "x = foobar;\n");
        assert_eq!(plan.selection, 10..10);
    }

    #[test]
    fn a_snippet_item_starts_a_session_on_its_first_stop() {
        let text = "fn main() {\n    total_ar\n}\n";
        let cursor = 24;
        let call = item(json!({
            "label": "total_area(…)", "insertTextFormat": 2,
            "textEdit": {"range": range(1, 4, 12), "newText": "total_area(${1:shapes})$0"},
            "command": {"title": "Trigger parameter hints", "command": "editor.action.triggerParameterHints"},
        }));
        let start = word_start(text, cursor, "rust");
        let plan =
            accept_item(text, cursor, cursor, start, &call, &HashMap::new(), SPACES).unwrap();
        let (new, selected) = applied(text, &plan);
        assert_eq!(new, "fn main() {\n    total_area(shapes)\n}\n");
        assert_eq!(selected, "shapes");
        let session = plan.session.expect("a session for the snippet");
        assert_eq!(session.active_stop().index, 1);
        assert_eq!(session.active_stop().ranges, vec![plan.selection.clone()]);
        assert_eq!(
            plan.command.map(|command| command.command).as_deref(),
            Some("editor.action.triggerParameterHints")
        );
    }

    #[test]
    fn an_additional_edit_above_shifts_the_selection_and_the_session() {
        let text = "fn main() {\n    let map = HashMa\n}\n";
        let cursor = 32;
        let import =
            json!([{"range": range(0, 0, 0), "newText": "use std::collections::HashMap;\n\n"}]);
        let plain = item(json!({
            "label": "HashMap",
            "textEdit": {"range": range(1, 14, 20), "newText": "HashMap"},
            "additionalTextEdits": import,
        }));
        let plan = accept_item(text, cursor, cursor, 26, &plain, &HashMap::new(), SPACES).unwrap();
        let (new, _) = applied(text, &plan);
        assert_eq!(
            new,
            "use std::collections::HashMap;\n\nfn main() {\n    let map = HashMap\n}\n"
        );
        let end = new.find("HashMap\n}").unwrap() + "HashMap".len();
        assert_eq!(plan.selection, end..end);

        let snippet = item(json!({
            "label": "HashMap::new", "insertTextFormat": 2,
            "textEdit": {"range": range(1, 14, 20), "newText": "HashMap::${1:new}()$0"},
            "additionalTextEdits": import,
        }));
        let plan =
            accept_item(text, cursor, cursor, 26, &snippet, &HashMap::new(), SPACES).unwrap();
        let (new, selected) = applied(text, &plan);
        assert!(new.ends_with("let map = HashMap::new()\n}\n"));
        assert_eq!(selected, "new");
        let session = plan.session.unwrap();
        assert_eq!(session.active_stop().ranges, vec![plan.selection.clone()]);
    }

    #[test]
    fn additional_edits_over_the_main_range_or_after_the_request_are_dropped() {
        let text = "fn main() {\n    let map = HashMa\n}\n";
        let cursor = 32;
        let item = item(json!({
            "label": "HashMap",
            "textEdit": {"range": range(1, 14, 20), "newText": "HashMap"},
            "additionalTextEdits": [
                {"range": range(1, 10, 16), "newText": "overlap"},
                {"range": range(2, 0, 1), "newText": "after"},
                {"range": range(0, 3, 7), "newText": "start"},
            ],
        }));
        let plan = accept_item(text, cursor, cursor, 26, &item, &HashMap::new(), SPACES).unwrap();
        let (new, _) = applied(text, &plan);
        assert_eq!(new, "fn start() {\n    let map = HashMap\n}\n");
        let end = new.find("HashMap").unwrap() + "HashMap".len();
        assert_eq!(plan.selection, end..end);
    }

    #[test]
    fn additional_edits_that_overlap_each_other_are_refused() {
        let text = "fn main() {\n    HashMa\n}\n";
        let item = item(json!({
            "label": "HashMap",
            "textEdit": {"range": range(1, 4, 10), "newText": "HashMap"},
            "additionalTextEdits": [
                {"range": range(0, 0, 4), "newText": "a"},
                {"range": range(0, 2, 6), "newText": "b"},
            ],
        }));
        assert!(accept_item(text, 22, 22, 16, &item, &HashMap::new(), SPACES).is_err());
    }

    #[test]
    fn positions_are_utf16_units_before_the_cursor() {
        let text = "let s = \"🦀\"; fo";
        let cursor = text.len();
        assert_eq!(cursor, 18);
        let foo = item(
            json!({"label": "foo", "textEdit": {"range": range(0, 14, 16), "newText": "foo"}}),
        );
        let start = word_start(text, cursor, "rust");
        let plan = accept_item(text, cursor, cursor, start, &foo, &HashMap::new(), SPACES).unwrap();
        assert_eq!(applied(text, &plan).0, "let s = \"🦀\"; foo");
        assert_eq!(plan.selection, 19..19);

        let split = item(
            json!({"label": "foo", "textEdit": {"range": range(0, 10, 16), "newText": "foo"}}),
        );
        assert!(accept_item(text, cursor, cursor, start, &split, &HashMap::new(), SPACES).is_err());
    }

    #[test]
    fn accepting_a_snippet_indents_its_lines_like_the_current_line() {
        let text = "impl Shape {\n    fn\n}\n";
        let cursor = 19;
        let start = word_start(text, cursor, "rust");
        let plan = accept_snippet(
            text,
            cursor,
            start,
            "fn ${1:name}($2) {\n\t$0\n}",
            &HashMap::new(),
            SPACES,
        );
        let (new, selected) = applied(text, &plan);
        assert_eq!(new, "impl Shape {\n    fn name() {\n        \n    }\n}\n");
        assert_eq!(selected, "name");
        let session = plan.session.unwrap();
        assert_eq!(session.active_stop().index, 1);
        assert!(plan.command.is_none());

        let tabs = IndentUnit {
            width: 4,
            hard_tabs: true,
        };
        let text = "\tif";
        let variables = HashMap::from([("TM_FILENAME_BASE".to_string(), "shapes".to_string())]);
        let plan = accept_snippet(text, 3, 1, "if x:\n\t${TM_FILENAME_BASE}", &variables, tabs);
        assert_eq!(applied(text, &plan).0, "\tif x:\n\t\tshapes");
        assert!(plan.session.is_none());
        assert_eq!(plan.selection, 15..15);
    }
}
