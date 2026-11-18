//! Pure decisions behind the breadcrumbs above the editor: the parts of the file's path and
//! the symbols the cursor is in. The Outline section of the Explorer lists the same
//! symbols. Kept free of GPUI so they are tested directly.
use serde_json::Value;

/// The most symbols kept of one file's outline.
const MAX_SYMBOLS: usize = 2000;

/// One symbol of a file, from the language server's document symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineSymbol {
    pub name: String,
    /// The protocol's SymbolKind number, 0 when the server gave none.
    pub kind: u32,
    /// How deeply the symbol is nested: 0 at the top level, one more per level of
    /// `children`. Flat `SymbolInformation`s are all at the top level.
    pub depth: usize,
    /// The line of the symbol's name (the start of its selection range, else of its
    /// range), 0-based.
    pub line: u32,
    /// The first and last line of the whole symbol, 0-based.
    pub start_line: u32,
    pub end_line: u32,
}

/// The folders and the file name of a project-relative path, in order.
pub fn path_segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The symbols in a `textDocument/documentSymbol` reply: nested `DocumentSymbol`s (with
/// `range`, `selectionRange` and `children`) or flat `SymbolInformation`s (with
/// `location.range`), in the server's order, each parent before its children. Entries
/// without a name or a range are skipped.
pub fn outline(reply: &Value) -> Vec<OutlineSymbol> {
    fn line_of(range: Option<&Value>, end: &str) -> Option<u32> {
        range?
            .get(end)?
            .get("line")?
            .as_u64()
            .and_then(|line| u32::try_from(line).ok())
    }
    fn visit(value: &Value, depth: usize, symbols: &mut Vec<OutlineSymbol>) {
        if symbols.len() >= MAX_SYMBOLS {
            return;
        }
        if let Some(items) = value.as_array() {
            for item in items {
                visit(item, depth, symbols);
            }
            return;
        }
        let range = value.get("range").or_else(|| {
            value
                .get("location")
                .and_then(|location| location.get("range"))
        });
        if let (Some(name), Some(start_line), Some(end_line)) = (
            value.get("name").and_then(Value::as_str),
            line_of(range, "start"),
            line_of(range, "end"),
        ) && !name.is_empty()
        {
            let kind = value
                .get("kind")
                .and_then(Value::as_u64)
                .and_then(|kind| u32::try_from(kind).ok())
                .unwrap_or(0);
            symbols.push(OutlineSymbol {
                name: name.to_owned(),
                kind,
                depth,
                line: line_of(value.get("selectionRange"), "start").unwrap_or(start_line),
                start_line,
                end_line: end_line.max(start_line),
            });
        }
        if let Some(children) = value.get("children") {
            visit(children, depth + 1, symbols);
        }
    }
    let mut symbols = Vec::new();
    visit(reply, 0, &mut symbols);
    symbols
}

/// The names of the symbols that contain `line` (0-based), the outermost first: for a
/// cursor inside a method, the type or impl block and then the method.
pub fn symbol_path(outline: &[OutlineSymbol], line: u32) -> Vec<String> {
    let mut containing: Vec<&OutlineSymbol> = outline
        .iter()
        .filter(|symbol| symbol.start_line <= line && line <= symbol.end_line)
        .collect();
    // The widest symbol is the outermost; of two with the same lines, the one the server
    // listed first is the parent.
    containing.sort_by_key(|symbol| std::cmp::Reverse(symbol.end_line - symbol.start_line));
    containing
        .into_iter()
        .map(|symbol| symbol.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range(start: u32, end: u32) -> Value {
        json!({"start": {"line": start, "character": 0}, "end": {"line": end, "character": 1}})
    }

    fn nested() -> Value {
        json!([
            {"name": "Args", "kind": 23, "range": range(2, 6), "selectionRange": range(3, 3),
             "children": [{"name": "name", "kind": 8, "range": range(5, 5), "selectionRange": range(5, 5)}]},
            {"name": "impl Args", "kind": 19, "range": range(8, 20), "selectionRange": range(8, 8),
             "children": [
                {"name": "parse", "kind": 6, "range": range(9, 14), "selectionRange": range(9, 9)},
                {"name": "help", "kind": 6, "range": range(16, 19), "selectionRange": range(16, 16)}
             ]}
        ])
    }

    #[test]
    fn path_segments_splits_a_relative_path() {
        assert_eq!(
            path_segments("crates/tandem/src/main.rs"),
            ["crates", "tandem", "src", "main.rs"]
        );
        assert_eq!(path_segments("Cargo.toml"), ["Cargo.toml"]);
        assert!(path_segments("").is_empty());
        assert_eq!(path_segments("a//b/"), ["a", "b"]);
    }

    #[test]
    fn outline_flattens_nested_document_symbols() {
        let symbols = outline(&nested());
        let names: Vec<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(names, ["Args", "name", "impl Args", "parse", "help"]);
        assert_eq!((symbols[3].start_line, symbols[3].end_line), (9, 14));
    }

    /// The name, kind, depth and line of each symbol of `reply`'s outline.
    fn placed(reply: &Value) -> Vec<(String, u32, usize, u32)> {
        outline(reply)
            .into_iter()
            .map(|symbol| (symbol.name, symbol.kind, symbol.depth, symbol.line))
            .collect()
    }

    #[test]
    fn outline_gives_nested_symbols_their_kind_depth_and_name_line() {
        let expected = [
            ("Args", 23, 0, 3),
            ("name", 8, 1, 5),
            ("impl Args", 19, 0, 8),
            ("parse", 6, 1, 9),
            ("help", 6, 1, 16),
        ]
        .map(|(name, kind, depth, line)| (name.to_string(), kind, depth, line));
        assert_eq!(placed(&nested()), expected);
    }

    #[test]
    fn outline_counts_every_level_of_children_and_falls_back_to_the_range_start() {
        let reply = json!([{
            "name": "shapes", "kind": 2, "range": range(0, 30),
            "children": [{
                "name": "Square", "kind": 23, "range": range(2, 10), "selectionRange": range(4, 4),
                "children": [{"name": "side", "range": range(6, 6)}]
            }]
        }, {"name": "main", "kind": 12, "range": range(32, 40)}]);
        let expected = [
            ("shapes", 2, 0, 0),
            ("Square", 23, 1, 4),
            // Without a kind the symbol gets kind 0, without a selection range the start
            // of its range.
            ("side", 0, 2, 6),
            ("main", 12, 0, 32),
        ]
        .map(|(name, kind, depth, line)| (name.to_string(), kind, depth, line));
        assert_eq!(placed(&reply), expected);
    }

    #[test]
    fn outline_reads_flat_symbol_information() {
        let reply = json!([
            {"name": "main", "kind": 12,
             "location": {"uri": "file:///p/src/main.rs", "range": range(4, 8)}},
            {"name": "helper", "kind": 12, "containerName": "main",
             "location": {"uri": "file:///p/src/main.rs", "range": range(5, 6)}}
        ]);
        assert_eq!(
            outline(&reply),
            [
                OutlineSymbol {
                    name: "main".into(),
                    kind: 12,
                    depth: 0,
                    line: 4,
                    start_line: 4,
                    end_line: 8
                },
                OutlineSymbol {
                    name: "helper".into(),
                    kind: 12,
                    depth: 0,
                    line: 5,
                    start_line: 5,
                    end_line: 6
                }
            ]
        );
    }

    #[test]
    fn outline_skips_entries_it_cannot_place() {
        let reply = json!([
            {"name": "", "range": range(1, 2)},
            {"name": "no range"},
            {"range": range(1, 2)},
            {"name": "backwards", "range": range(9, 3)},
            "text",
            null
        ]);
        assert_eq!(
            outline(&reply),
            [OutlineSymbol {
                name: "backwards".into(),
                kind: 0,
                depth: 0,
                line: 9,
                start_line: 9,
                end_line: 9
            }]
        );
        assert!(outline(&json!(null)).is_empty());
        assert!(outline(&json!({"items": []})).is_empty());
    }

    #[test]
    fn outline_keeps_a_bounded_number_of_symbols() {
        let many: Vec<Value> = (0..MAX_SYMBOLS as u32 + 50)
            .map(|line| json!({"name": format!("s{line}"), "range": range(line, line)}))
            .collect();
        assert_eq!(outline(&Value::Array(many)).len(), MAX_SYMBOLS);
    }

    #[test]
    fn symbol_path_lists_the_enclosing_symbols_outermost_first() {
        let symbols = outline(&nested());
        assert_eq!(symbol_path(&symbols, 10), ["impl Args", "parse"]);
        assert_eq!(symbol_path(&symbols, 5), ["Args", "name"]);
        assert_eq!(symbol_path(&symbols, 8), ["impl Args"]);
        assert_eq!(symbol_path(&symbols, 20), ["impl Args"]);
    }

    #[test]
    fn symbol_path_is_empty_between_symbols_and_without_an_outline() {
        let symbols = outline(&nested());
        assert!(symbol_path(&symbols, 0).is_empty());
        assert!(symbol_path(&symbols, 7).is_empty());
        assert!(symbol_path(&symbols, 99).is_empty());
        assert!(symbol_path(&[], 3).is_empty());
    }

    #[test]
    fn symbol_path_keeps_the_servers_order_for_symbols_on_the_same_lines() {
        let symbols = [
            OutlineSymbol {
                name: "outer".into(),
                kind: 2,
                depth: 0,
                line: 1,
                start_line: 1,
                end_line: 1,
            },
            OutlineSymbol {
                name: "inner".into(),
                kind: 12,
                depth: 1,
                line: 1,
                start_line: 1,
                end_line: 1,
            },
        ];
        assert_eq!(symbol_path(&symbols, 1), ["outer", "inner"]);
    }
}
