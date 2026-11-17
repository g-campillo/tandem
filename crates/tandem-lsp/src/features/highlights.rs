//! Document highlights: the other uses of the symbol under the cursor, as the language
//! server reports them, read as byte ranges of the text they are about.
use crate::utf16_offset;
use lsp_types::{DocumentHighlight, DocumentHighlightKind};
use serde_json::Value;
use std::ops::Range;

/// The highlights of a textDocument/documentHighlight reply about `text`: byte ranges,
/// each with whether it writes the symbol (kind 3, Write); reads, plain text and a missing
/// kind are not writes. A highlight whose range is malformed, empty or not in `text` is
/// skipped, and `null` (or a reply that is not a list) has none.
pub fn document_highlights(text: &str, value: &Value) -> Vec<(Range<usize>, bool)> {
    let Some(highlights) = value.as_array() else {
        return Vec::new();
    };
    highlights
        .iter()
        .filter_map(|highlight| {
            let highlight: DocumentHighlight = serde_json::from_value(highlight.clone()).ok()?;
            let start = utf16_offset(text, highlight.range.start).ok()?;
            let end = utf16_offset(text, highlight.range.end).ok()?;
            let write = highlight.kind == Some(DocumentHighlightKind::WRITE);
            (start < end).then_some((start..end, write))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A highlight of `line`, UTF-16 columns `start..end`, with `kind` when there is one.
    fn highlight(line: u32, start: u32, end: u32, kind: Option<u8>) -> Value {
        let mut highlight = json!({"range": {
            "start": {"line": line, "character": start},
            "end": {"line": line, "character": end},
        }});
        if let Some(kind) = kind {
            highlight["kind"] = json!(kind);
        }
        highlight
    }

    const TEXT: &str = "let mut total = 0;\ntotal += 1;\nprint(total);\n";

    #[test]
    fn text_read_and_write_highlights_become_byte_ranges() {
        let reply = json!([
            highlight(0, 8, 13, Some(3)),
            highlight(1, 0, 5, Some(2)),
            highlight(2, 6, 11, Some(1)),
        ]);
        assert_eq!(
            document_highlights(TEXT, &reply),
            [(8..13, true), (19..24, false), (37..42, false)]
        );
    }

    #[test]
    fn a_highlight_without_a_kind_is_text() {
        let reply = json!([highlight(1, 0, 5, None)]);
        assert_eq!(document_highlights(TEXT, &reply), [(19..24, false)]);
    }

    #[test]
    fn ranges_outside_the_text_are_skipped() {
        let reply = json!([
            highlight(0, 8, 13, Some(3)),
            // Past the end of the second line.
            highlight(1, 6, 40, Some(2)),
            // A line the text does not have.
            highlight(9, 0, 5, Some(2)),
            // Reversed.
            highlight(2, 11, 6, Some(2)),
            // Empty.
            highlight(2, 6, 6, Some(2)),
            // Not a highlight.
            json!({"kind": 2}),
        ]);
        assert_eq!(document_highlights(TEXT, &reply), [(8..13, true)]);
    }

    #[test]
    fn utf16_columns_after_an_emoji_are_converted_to_bytes() {
        // The emoji is two UTF-16 units and four bytes; the accented letter one and two.
        let text = "let s = \"🦀é\"; let total = s;\ntotal\n";
        let reply = json!([highlight(0, 19, 24, Some(3)), highlight(1, 0, 5, None)]);
        let total = text.find("total").unwrap();
        assert_eq!(
            document_highlights(text, &reply),
            [(total..total + 5, true), (33..38, false)]
        );
        // Splitting the emoji's surrogate pair is not a range of the text.
        let split = json!([highlight(0, 9, 10, Some(1))]);
        assert!(document_highlights(text, &split).is_empty());
    }

    #[test]
    fn null_and_other_replies_have_no_highlights() {
        assert!(document_highlights(TEXT, &Value::Null).is_empty());
        assert!(document_highlights(TEXT, &json!([])).is_empty());
        assert!(document_highlights(TEXT, &json!({"range": {}})).is_empty());
    }
}
