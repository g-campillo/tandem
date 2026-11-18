//! Locations: where Go to Type Definition and Go to Implementation lead, as the language
//! server reports them.
use lsp_types::Range;
use serde_json::Value;
use std::path::PathBuf;

/// The places of a `textDocument/typeDefinition` or `textDocument/implementation` reply,
/// in the server's order: the file and range of a `Location`, of each of a `Location[]`,
/// or the target file and the range of the target's name (`targetSelectionRange`) of each
/// of a `LocationLink[]`. Places outside a local file (an `untitled:` document, a remote
/// address) and malformed entries are skipped, and `null` has none.
pub fn locations(value: &Value) -> Vec<(PathBuf, Range)> {
    match value {
        Value::Array(items) => items.iter().filter_map(location).collect(),
        Value::Object(_) => location(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The file and range of one `Location` or `LocationLink`, or None when it is malformed or
/// not in a local file.
fn location(value: &Value) -> Option<(PathBuf, Range)> {
    let (uri, range) = match value.get("targetUri") {
        Some(uri) => (uri, value.get("targetSelectionRange")?),
        None => (value.get("uri")?, value.get("range")?),
    };
    let url = url::Url::parse(uri.as_str()?).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let range = serde_json::from_value(range.clone()).ok()?;
    Some((url.to_file_path().ok()?, range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use serde_json::json;

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    fn lsp_range(line: u32, start: u32, end: u32) -> Range {
        Range::new(Position::new(line, start), Position::new(line, end))
    }

    #[test]
    fn null_and_an_empty_list_have_no_locations() {
        assert!(locations(&Value::Null).is_empty());
        assert!(locations(&json!([])).is_empty());
    }

    #[test]
    fn a_single_location_gives_its_file_and_range() {
        let reply = json!({"uri": "file:///project/src/lib.rs", "range": range(4, 11, 17)});
        assert_eq!(
            locations(&reply),
            [(PathBuf::from("/project/src/lib.rs"), lsp_range(4, 11, 17))]
        );
    }

    #[test]
    fn a_location_list_keeps_the_server_order() {
        let reply = json!([
            {"uri": "file:///project/src/square.rs", "range": range(8, 0, 22)},
            {"uri": "file:///project/src/circle.rs", "range": range(2, 0, 22)},
        ]);
        assert_eq!(
            locations(&reply),
            [
                (PathBuf::from("/project/src/square.rs"), lsp_range(8, 0, 22)),
                (PathBuf::from("/project/src/circle.rs"), lsp_range(2, 0, 22)),
            ]
        );
    }

    #[test]
    fn a_location_link_gives_its_target_file_and_the_range_of_the_target_name() {
        let reply = json!([{
            "originSelectionRange": range(38, 8, 13),
            "targetUri": "file:///project/src/lib.rs",
            "targetRange": {"start": {"line": 4, "character": 0}, "end": {"line": 6, "character": 1}},
            "targetSelectionRange": range(4, 11, 17),
        }]);
        assert_eq!(
            locations(&reply),
            [(PathBuf::from("/project/src/lib.rs"), lsp_range(4, 11, 17))]
        );
    }

    #[test]
    fn escaped_paths_are_decoded() {
        let reply = json!({"uri": "file:///project/my%20file.rs", "range": range(1, 0, 1)});
        assert_eq!(locations(&reply)[0].0, PathBuf::from("/project/my file.rs"));
    }

    #[test]
    fn locations_outside_local_files_and_malformed_entries_are_skipped() {
        let reply = json!([
            {"uri": "untitled:Untitled-1", "range": range(0, 0, 1)},
            {"targetUri": "untitled:Untitled-2", "targetRange": range(0, 0, 1),
             "targetSelectionRange": range(0, 0, 1)},
            {"uri": "https://example.com/lib.rs", "range": range(0, 0, 1)},
            {"uri": "not a uri", "range": range(0, 0, 1)},
            {"uri": "file:///project/no_range.rs"},
            {"uri": "file:///project/bad_range.rs", "range": {"start": 3}},
            {"targetUri": "file:///project/no_selection.rs", "targetRange": range(0, 0, 1)},
            {"range": range(0, 0, 1)},
            "file:///project/a.rs",
            {"uri": "file:///project/kept.rs", "range": range(3, 0, 1)},
        ]);
        assert_eq!(
            locations(&reply),
            [(PathBuf::from("/project/kept.rs"), lsp_range(3, 0, 1))]
        );
    }

    #[test]
    fn a_lone_untitled_location_is_skipped() {
        let reply = json!({"uri": "untitled:Untitled-1", "range": range(0, 0, 1)});
        assert!(locations(&reply).is_empty());
    }

    #[test]
    fn other_replies_have_no_locations() {
        assert!(locations(&json!("file:///project/a.rs")).is_empty());
        assert!(locations(&json!(3)).is_empty());
    }
}
