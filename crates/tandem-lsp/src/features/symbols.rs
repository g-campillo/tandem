//! Workspace symbols: the symbols of the whole project whose names match a query, as the
//! language server reports them for Go to Symbol in Workspace.
use serde_json::Value;
use std::path::PathBuf;

/// The most symbols kept of one reply.
pub const MAX_SYMBOLS: usize = 200;

/// One symbol of a `workspace/symbol` reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolHit {
    pub name: String,
    /// The name of the symbol that contains it (a type, an impl block, a module), when the
    /// server gave one.
    pub container: Option<String>,
    /// The protocol's SymbolKind number, 0 when the server gave none.
    pub kind: u32,
    /// The file the symbol is in.
    pub path: PathBuf,
    /// The line the symbol starts on, 0-based; 0 when the server gave no range.
    pub line: u32,
}

/// The symbols of a `workspace/symbol` reply: `SymbolInformation`s or `WorkspaceSymbol`s,
/// in the server's order, at most [`MAX_SYMBOLS`]. A symbol whose location has no range
/// (a `WorkspaceSymbol` the client could resolve) is on line 0. Symbols without a name or
/// outside a local file (an `untitled:` document, a remote address) are skipped, and
/// `null` (or a reply that is not a list) has none.
pub fn workspace_symbols(value: &Value) -> Vec<SymbolHit> {
    let Some(symbols) = value.as_array() else {
        return Vec::new();
    };
    symbols
        .iter()
        .filter_map(symbol_hit)
        .take(MAX_SYMBOLS)
        .collect()
}

/// One symbol of the reply, or None when it is malformed or not in a local file.
fn symbol_hit(symbol: &Value) -> Option<SymbolHit> {
    let number = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    };
    let name = symbol
        .get("name")?
        .as_str()
        .filter(|name| !name.is_empty())?;
    let location = symbol.get("location")?;
    let url = url::Url::parse(location.get("uri")?.as_str()?).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    Some(SymbolHit {
        name: name.to_owned(),
        container: symbol
            .get("containerName")
            .and_then(Value::as_str)
            .filter(|container| !container.is_empty())
            .map(str::to_owned),
        kind: number(symbol.get("kind")).unwrap_or(0),
        path: url.to_file_path().ok()?,
        line: number(location.pointer("/range/start/line")).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn range(line: u32) -> Value {
        json!({"start": {"line": line, "character": 4}, "end": {"line": line, "character": 10}})
    }

    #[test]
    fn symbol_informations_give_name_container_kind_path_and_line() {
        let reply = json!([
            {"name": "Square", "kind": 23, "location": {"uri": "file:///project/src/lib.rs", "range": range(4)}},
            {"name": "area", "kind": 6, "containerName": "impl Shape for Square",
             "location": {"uri": "file:///project/src/lib.rs", "range": range(9)}, "deprecated": false},
        ]);
        assert_eq!(
            workspace_symbols(&reply),
            [
                SymbolHit {
                    name: "Square".into(),
                    container: None,
                    kind: 23,
                    path: PathBuf::from("/project/src/lib.rs"),
                    line: 4,
                },
                SymbolHit {
                    name: "area".into(),
                    container: Some("impl Shape for Square".into()),
                    kind: 6,
                    path: PathBuf::from("/project/src/lib.rs"),
                    line: 9,
                },
            ]
        );
    }

    #[test]
    fn workspace_symbols_with_and_without_a_range() {
        let reply = json!([
            {"name": "Square", "kind": 5, "containerName": "shapes",
             "location": {"uri": "file:///project/shapes.py", "range": range(0)}},
            // A location without a range (the client may resolve it later) is line 0.
            {"name": "describe", "kind": 12, "location": {"uri": "file:///project/shapes.py"},
             "data": {"id": 7}},
        ]);
        let hits = workspace_symbols(&reply);
        assert_eq!(
            hits.iter()
                .map(|hit| (
                    hit.name.as_str(),
                    hit.container.as_deref(),
                    hit.kind,
                    hit.line
                ))
                .collect::<Vec<_>>(),
            [("Square", Some("shapes"), 5, 0), ("describe", None, 12, 0)]
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path == Path::new("/project/shapes.py"))
        );
    }

    #[test]
    fn a_symbol_without_a_kind_gets_kind_0_and_an_empty_container_is_none() {
        let reply = json!([{"name": "main", "containerName": "",
            "location": {"uri": "file:///project/main.rs", "range": range(2)}}]);
        let [hit] = workspace_symbols(&reply).try_into().unwrap();
        assert_eq!((hit.kind, hit.container, hit.line), (0, None, 2));
    }

    #[test]
    fn escaped_paths_are_decoded() {
        let reply = json!([{"name": "x", "kind": 13,
            "location": {"uri": "file:///project/my%20file.rs", "range": range(1)}}]);
        assert_eq!(
            workspace_symbols(&reply)[0].path,
            PathBuf::from("/project/my file.rs")
        );
    }

    #[test]
    fn symbols_outside_local_files_and_malformed_entries_are_skipped() {
        let reply = json!([
            {"name": "draft", "kind": 12, "location": {"uri": "untitled:Untitled-1", "range": range(0)}},
            {"name": "remote", "kind": 12, "location": {"uri": "https://example.com/a.rs", "range": range(0)}},
            {"name": "bad", "kind": 12, "location": {"uri": "not a uri", "range": range(0)}},
            {"name": "nowhere", "kind": 12},
            {"kind": 12, "location": {"uri": "file:///project/a.rs", "range": range(0)}},
            {"name": "", "kind": 12, "location": {"uri": "file:///project/a.rs", "range": range(0)}},
            "Square",
            {"name": "kept", "kind": 12, "location": {"uri": "file:///project/a.rs", "range": range(3)}},
        ]);
        let names: Vec<String> = workspace_symbols(&reply)
            .into_iter()
            .map(|hit| hit.name)
            .collect();
        assert_eq!(names, ["kept"]);
    }

    #[test]
    fn at_most_two_hundred_symbols_are_kept() {
        let reply = Value::Array(
            (0..300)
                .map(|line| {
                    json!({"name": format!("item{line}"), "kind": 13,
                        "location": {"uri": "file:///project/a.rs", "range": range(line)}})
                })
                .collect(),
        );
        let hits = workspace_symbols(&reply);
        assert_eq!(hits.len(), MAX_SYMBOLS);
        assert_eq!(hits[0].name, "item0");
        assert_eq!(hits[MAX_SYMBOLS - 1].line, MAX_SYMBOLS as u32 - 1);
    }

    #[test]
    fn null_and_other_replies_have_no_symbols() {
        assert!(workspace_symbols(&Value::Null).is_empty());
        assert!(workspace_symbols(&json!([])).is_empty());
        assert!(workspace_symbols(&json!({"name": "Square"})).is_empty());
    }
}
