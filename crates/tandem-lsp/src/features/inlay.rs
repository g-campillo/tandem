//! Inlay hints drawn at the end of the line: the inferred types and the parameter names a
//! language server reports, grouped per line of the text they are about.
use crate::utf16_offset;
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel};
use serde_json::{Value, json};

/// The most characters drawn after a line.
const MAX_CHARS: usize = 80;

/// The hints of one line of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHint {
    pub line: u32,
    /// Type hints and hints without a kind (chaining, closing-brace), in position order.
    pub labels: Vec<String>,
    /// Parameter-name hints (kind 2): the names, with surrounding whitespace and any
    /// trailing ':' removed, in position order.
    pub parameters: Vec<String>,
}

/// The parameters of a textDocument/inlayHint request for the whole of `text`.
pub fn inlay_params(text: &str) -> Value {
    let last_line = &text[text.rfind('\n').map_or(0, |newline| newline + 1)..];
    json!({"range": {
        "start": {"line": 0, "character": 0},
        "end": {
            "line": text.matches('\n').count(),
            "character": last_line.encode_utf16().count(),
        },
    }})
}

/// Groups an `InlayHint[]` reply per line. Label parts are concatenated. A type hint
/// whose label starts with ':' gets the identifier right before its position in front
/// ("total: f64"). Hints past the end of the text are skipped.
pub fn eol_hints(text: &str, value: &Value) -> Vec<LineHint> {
    let Some(hints) = value.as_array() else {
        return Vec::new();
    };
    let mut placed: Vec<_> = hints
        .iter()
        .filter_map(|hint| {
            let hint: InlayHint = serde_json::from_value(hint.clone()).ok()?;
            let offset = utf16_offset(text, hint.position).ok()?;
            let label = match hint.label {
                InlayHintLabel::String(label) => label,
                InlayHintLabel::LabelParts(parts) => {
                    parts.into_iter().map(|part| part.value).collect()
                }
            };
            let label = label.trim();
            let parameter = hint.kind == Some(InlayHintKind::PARAMETER);
            let label = if parameter {
                label.trim_end_matches(':').trim_end().to_string()
            } else if hint.kind == Some(InlayHintKind::TYPE) && label.starts_with(':') {
                format!("{}{label}", identifier_before(text, offset))
            } else {
                label.to_string()
            };
            (!label.is_empty()).then_some((hint.position, parameter, label))
        })
        .collect();
    placed.sort_by_key(|(position, ..)| (position.line, position.character));
    let mut lines: Vec<LineHint> = Vec::new();
    for (position, parameter, label) in placed {
        if lines.last().is_none_or(|last| last.line != position.line) {
            lines.push(LineHint {
                line: position.line,
                labels: Vec::new(),
                parameters: Vec::new(),
            });
        }
        if let Some(hint) = lines.last_mut() {
            if parameter {
                hint.parameters.push(label);
            } else {
                hint.labels.push(label);
            }
        }
    }
    lines
}

/// The identifier that ends at `offset` in `text`, empty when there is none.
fn identifier_before(text: &str, offset: usize) -> &str {
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '$'))
        .last()
        .map_or(offset, |(start, _)| start);
    &text[start..offset]
}

/// What is drawn after the line: the labels joined with two spaces (when `types`), then,
/// after two more spaces, the parameter names as one group in parentheses, each with a
/// colon: "(side:, label:)" (when `parameters`). Cut to 80 chars with "…". None when
/// nothing is left.
pub fn hint_text(hint: &LineHint, types: bool, parameters: bool) -> Option<String> {
    let mut parts = Vec::new();
    if types && !hint.labels.is_empty() {
        parts.push(hint.labels.join("  "));
    }
    if parameters && !hint.parameters.is_empty() {
        let names: Vec<_> = hint
            .parameters
            .iter()
            .map(|name| format!("{name}:"))
            .collect();
        parts.push(format!("({})", names.join(", ")));
    }
    if parts.is_empty() {
        return None;
    }
    let text = parts.join("  ");
    if text.chars().count() <= MAX_CHARS {
        return Some(text);
    }
    let mut cut: String = text.chars().take(MAX_CHARS - 1).collect();
    cut.push('…');
    Some(cut)
}

/// After an edit of rows `start_row..=old_end_row` that now ends at `new_end_row`:
/// hints on the edited rows are dropped, later rows shift.
pub fn shift_hint_lines(
    hints: &mut Vec<LineHint>,
    start_row: u32,
    old_end_row: u32,
    new_end_row: u32,
) {
    hints.retain(|hint| hint.line < start_row || hint.line > old_end_row);
    for hint in hints.iter_mut().filter(|hint| hint.line > old_end_row) {
        hint.line = hint.line - old_end_row + new_end_row;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A hint at `line`, UTF-16 column `character`, with `label` (a string or label parts)
    /// and `kind` when there is one.
    fn hint(line: u32, character: u32, label: Value, kind: Option<u8>) -> Value {
        let mut hint = json!({
            "position": {"line": line, "character": character},
            "label": label,
        });
        if let Some(kind) = kind {
            hint["kind"] = json!(kind);
        }
        hint
    }

    /// The column right after the first `needle` on `line` of `text`.
    fn after(text: &str, line: usize, needle: &str) -> u32 {
        let row = text.lines().nth(line).unwrap();
        (row.find(needle).unwrap() + needle.len()) as u32
    }

    fn line_hint(line: u32, labels: &[&str], parameters: &[&str]) -> LineHint {
        LineHint {
            line,
            labels: labels.iter().map(|label| label.to_string()).collect(),
            parameters: parameters.iter().map(|name| name.to_string()).collect(),
        }
    }

    const TEXT: &str = "fn main() {\n    let mut total = 0.0;\n    let s = describe(1.0, \"x\");\n    let v = items.iter().map(f);\n}\n";

    #[test]
    fn a_type_hint_gets_the_identifier_before_it() {
        let reply = json!([hint(1, after(TEXT, 1, "total"), json!(": f64"), Some(1))]);
        assert_eq!(
            eol_hints(TEXT, &reply),
            [line_hint(1, &["total: f64"], &[])]
        );
    }

    #[test]
    fn a_chaining_hint_and_a_hint_without_a_kind_keep_their_labels() {
        let reply = json!([
            hint(
                3,
                after(TEXT, 3, "iter()"),
                json!("impl Iterator<Item = &u8>"),
                Some(1)
            ),
            hint(4, 1, json!("fn main"), None),
        ]);
        assert_eq!(
            eol_hints(TEXT, &reply),
            [
                line_hint(3, &["impl Iterator<Item = &u8>"], &[]),
                line_hint(4, &["fn main"], &[]),
            ]
        );
    }

    #[test]
    fn parameter_hints_become_plain_names_in_position_order() {
        let label = after(TEXT, 2, "1.0, ");
        let side = after(TEXT, 2, "describe(");
        // Out of order, as a server may send them.
        let reply = json!([
            hint(2, label, json!(" label "), Some(2)),
            hint(2, side, json!("side:"), Some(2)),
        ]);
        assert_eq!(
            eol_hints(TEXT, &reply),
            [line_hint(2, &[], &["side", "label"])]
        );
    }

    #[test]
    fn label_parts_are_concatenated() {
        let parts = json!([{"value": ": "}, {"value": "String", "location": {
            "uri": "file:///std/string.rs",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 6}},
        }}]);
        let reply = json!([
            hint(2, after(TEXT, 2, "let s"), parts, Some(1)),
            hint(
                2,
                after(TEXT, 2, "describe("),
                json!([{"value": "side"}, {"value": ":"}]),
                Some(2)
            ),
        ]);
        assert_eq!(
            eol_hints(TEXT, &reply),
            [line_hint(2, &["s: String"], &["side"])]
        );
    }

    #[test]
    fn two_type_hints_on_one_line_keep_their_order() {
        let text = "let (a, b) = pair();\n";
        let reply = json!([
            hint(0, after(text, 0, "a, b"), json!(": u8"), Some(1)),
            hint(0, after(text, 0, "(a"), json!(": i32"), Some(1)),
        ]);
        assert_eq!(
            eol_hints(text, &reply),
            [line_hint(0, &["a: i32", "b: u8"], &[])]
        );
    }

    #[test]
    fn hints_past_the_end_of_the_text_are_skipped() {
        let reply = json!([
            hint(1, after(TEXT, 1, "total"), json!(": f64"), Some(1)),
            // Past the end of the second line.
            hint(1, 80, json!(": f64"), Some(1)),
            // A line the text does not have.
            hint(9, 0, json!(": u8"), Some(1)),
            // Not a hint.
            json!({"label": "x"}),
        ]);
        assert_eq!(
            eol_hints(TEXT, &reply),
            [line_hint(1, &["total: f64"], &[])]
        );
    }

    #[test]
    fn null_and_other_replies_have_no_hints() {
        assert!(eol_hints(TEXT, &Value::Null).is_empty());
        assert!(eol_hints(TEXT, &json!([])).is_empty());
        assert!(eol_hints(TEXT, &json!({"position": {}})).is_empty());
    }

    #[test]
    fn a_type_hint_without_an_identifier_before_it_keeps_its_label() {
        let text = "let x = [1, 2].map(|_| 3);\n";
        let reply = json!([hint(0, after(text, 0, "]"), json!(": [i32; 2]"), Some(1))]);
        assert_eq!(
            eol_hints(text, &reply),
            [line_hint(0, &[": [i32; 2]"], &[])]
        );
    }

    #[test]
    fn the_identifier_is_found_after_wide_characters() {
        // The emoji is two UTF-16 units and four bytes.
        let text = "let 🦀 = 1; let crab = 2;\n";
        let column = "let 🦀 = 1; let crab".encode_utf16().count() as u32;
        let reply = json!([hint(0, column, json!(": i32"), Some(1))]);
        assert_eq!(eol_hints(text, &reply), [line_hint(0, &["crab: i32"], &[])]);
    }

    #[test]
    fn hint_text_shows_types_then_parameter_names() {
        let hint = line_hint(2, &["s: String"], &["side", "label"]);
        assert_eq!(
            hint_text(&hint, true, true).as_deref(),
            Some("s: String  (side:, label:)")
        );
        assert_eq!(hint_text(&hint, true, false).as_deref(), Some("s: String"));
        assert_eq!(
            hint_text(&hint, false, true).as_deref(),
            Some("(side:, label:)")
        );
        assert_eq!(hint_text(&hint, false, false), None);
    }

    #[test]
    fn hint_text_joins_the_labels_with_two_spaces() {
        let hint = line_hint(0, &["a: i32", "b: u8"], &[]);
        assert_eq!(
            hint_text(&hint, true, true).as_deref(),
            Some("a: i32  b: u8")
        );
    }

    #[test]
    fn hint_text_is_none_when_the_shown_kinds_are_empty() {
        let types = line_hint(0, &["total: f64"], &[]);
        assert_eq!(hint_text(&types, false, true), None);
        let names = line_hint(0, &[], &["side"]);
        assert_eq!(hint_text(&names, true, false), None);
        assert_eq!(hint_text(&line_hint(0, &[], &[]), true, true), None);
    }

    #[test]
    fn hint_text_is_cut_to_eighty_characters() {
        let long = "é".repeat(100);
        let hint = line_hint(0, &[&long], &[]);
        let text = hint_text(&hint, true, true).unwrap();
        assert_eq!(text.chars().count(), 80);
        assert!(text.ends_with("é…"), "{text}");
        let exact = "x".repeat(80);
        let fits = line_hint(0, &[&exact], &[]);
        assert_eq!(hint_text(&fits, true, true), Some(exact));
    }

    fn lines(hints: &[LineHint]) -> Vec<(u32, &str)> {
        hints
            .iter()
            .map(|hint| (hint.line, hint.labels[0].as_str()))
            .collect()
    }

    fn hints() -> Vec<LineHint> {
        vec![
            line_hint(1, &["one"], &[]),
            line_hint(4, &["four"], &[]),
            line_hint(6, &["six"], &[]),
        ]
    }

    #[test]
    fn inserted_lines_move_the_later_hints_down() {
        let mut hints = hints();
        // Enter at the end of row 2: rows 2..=2 now end at row 3.
        shift_hint_lines(&mut hints, 2, 2, 3);
        assert_eq!(lines(&hints), [(1, "one"), (5, "four"), (7, "six")]);
    }

    #[test]
    fn deleted_lines_drop_their_hints_and_move_the_later_ones_up() {
        let mut hints = hints();
        // Rows 3..=4 joined into row 3.
        shift_hint_lines(&mut hints, 3, 4, 3);
        assert_eq!(lines(&hints), [(1, "one"), (5, "six")]);
    }

    #[test]
    fn an_edit_inside_a_hinted_row_drops_its_hints() {
        let mut hints = hints();
        shift_hint_lines(&mut hints, 4, 4, 4);
        assert_eq!(lines(&hints), [(1, "one"), (6, "six")]);
    }

    #[test]
    fn the_params_cover_the_whole_text() {
        assert_eq!(
            inlay_params("ab\n🦀x"),
            json!({"range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 1, "character": 3},
            }})
        );
        assert_eq!(
            inlay_params("ab\n"),
            json!({"range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 1, "character": 0},
            }})
        );
    }
}
