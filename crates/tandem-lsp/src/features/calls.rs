//! The call hierarchy: the functions that call a function (incoming calls) and the ones it
//! calls (outgoing calls), as the language server reports them.
use serde_json::Value;
use std::path::PathBuf;

/// One function of the call hierarchy, from a `CallHierarchyItem`.
#[derive(Clone, Debug, PartialEq)]
pub struct CallItem {
    pub name: String,
    /// What the server says about it besides its name (a signature, a container), or
    /// empty.
    pub detail: String,
    /// The protocol's SymbolKind number, 0 when the server gave none.
    pub kind: u32,
    /// The file it is in.
    pub path: PathBuf,
    /// The line of its name (the start of its `selectionRange`), 0-based.
    pub line: u32,
    /// The item as the server sent it, which the incoming and outgoing call requests take
    /// back as `{"item": raw}`.
    pub raw: Value,
}

/// The items of a `textDocument/prepareCallHierarchy` reply, in the server's order.
/// Items outside a local file (an `untitled:` document, a remote address) and malformed
/// ones are skipped, and `null` has none.
pub fn prepared(value: &Value) -> Vec<CallItem> {
    list(value).iter().filter_map(call_item).collect()
}

/// The callers of a `callHierarchy/incomingCalls` reply, in the server's order, each with
/// the line of its first call (the earliest of `fromRanges`, which are in the caller's
/// file), or None when the server gave no range. Calls whose caller is skipped by
/// [`prepared`]'s rules are skipped.
pub fn incoming(value: &Value) -> Vec<(CallItem, Option<u32>)> {
    calls(value, "from")
}

/// The callees of a `callHierarchy/outgoingCalls` reply, in the server's order, each with
/// the line of its first call (the earliest of `fromRanges`, which are in the caller's
/// file, not the callee's), or None when the server gave no range. Calls whose callee is
/// skipped by [`prepared`]'s rules are skipped.
pub fn outgoing(value: &Value) -> Vec<(CallItem, Option<u32>)> {
    calls(value, "to")
}

/// The entries of a reply that is a list; none for `null` and anything else.
fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The calls of an incoming or outgoing reply, whose item is under `side` (`from` or
/// `to`), with the earliest line of their `fromRanges`.
fn calls(value: &Value, side: &str) -> Vec<(CallItem, Option<u32>)> {
    list(value)
        .iter()
        .filter_map(|call| {
            let item = call_item(call.get(side)?)?;
            let first_line = list(call.get("fromRanges").unwrap_or(&Value::Null))
                .iter()
                .filter_map(|range| number(range.pointer("/start/line")))
                .min();
            Some((item, first_line))
        })
        .collect()
}

/// One `CallHierarchyItem`, or None when it has no name, no `selectionRange` start line or
/// is not in a local file.
fn call_item(raw: &Value) -> Option<CallItem> {
    let name = raw.get("name")?.as_str()?;
    let url = url::Url::parse(raw.get("uri")?.as_str()?).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    Some(CallItem {
        name: name.to_owned(),
        detail: raw
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        kind: number(raw.get("kind")).unwrap_or(0),
        path: url.to_file_path().ok()?,
        line: number(raw.pointer("/selectionRange/start/line"))?,
        raw: raw.clone(),
    })
}

/// A JSON number that fits a `u32`.
fn number(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
    }

    /// A `CallHierarchyItem` for `name` whose body starts on `line` - 1 and whose name is
    /// on `line`.
    fn item(name: &str, uri: &str, line: u32) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "detail": format!("pub fn {name}()"),
            "uri": uri,
            "range": {"start": {"line": line - 1, "character": 0}, "end": {"line": line + 6, "character": 1}},
            "selectionRange": range(line, 7, 7 + name.len() as u32),
            "data": {"opaque": name},
        })
    }

    fn call_item(name: &str, path: &str, line: u32, raw: Value) -> CallItem {
        CallItem {
            name: name.into(),
            detail: format!("pub fn {name}()"),
            kind: 12,
            path: PathBuf::from(path),
            line,
            raw,
        }
    }

    #[test]
    fn prepared_items_give_name_detail_kind_path_the_name_line_and_the_raw_item() {
        let total = item("total_area", "file:///project/src/lib.rs", 15);
        let describe = item("describe", "file:///project/src/lib.rs", 23);
        let reply = json!([total, describe]);
        assert_eq!(
            prepared(&reply),
            [
                call_item("total_area", "/project/src/lib.rs", 15, total),
                call_item("describe", "/project/src/lib.rs", 23, describe),
            ]
        );
    }

    #[test]
    fn null_and_other_replies_prepare_nothing() {
        assert!(prepared(&Value::Null).is_empty());
        assert!(prepared(&json!([])).is_empty());
        assert!(prepared(&json!({"name": "total_area"})).is_empty());
        assert!(incoming(&Value::Null).is_empty());
        assert!(outgoing(&Value::Null).is_empty());
    }

    #[test]
    fn an_item_without_detail_or_kind_has_empty_detail_and_kind_0() {
        let raw = json!({
            "name": "main",
            "uri": "file:///project/main.py",
            "range": range(0, 0, 4),
            "selectionRange": range(0, 0, 4),
        });
        let [found] = prepared(&json!([raw])).try_into().unwrap();
        assert_eq!(found.detail, "");
        assert_eq!(found.kind, 0);
        assert_eq!(found.line, 0);
    }

    #[test]
    fn escaped_paths_are_decoded() {
        let reply = json!([item("total_area", "file:///project/my%20lib.rs", 3)]);
        assert_eq!(
            prepared(&reply)[0].path,
            PathBuf::from("/project/my lib.rs")
        );
    }

    #[test]
    fn items_outside_local_files_and_malformed_items_are_skipped() {
        let mut no_name = item("x", "file:///project/a.rs", 1);
        no_name.as_object_mut().unwrap().remove("name");
        let mut no_selection = item("x", "file:///project/a.rs", 1);
        no_selection
            .as_object_mut()
            .unwrap()
            .remove("selectionRange");
        let reply = json!([
            item("untitled", "untitled:Untitled-1", 1),
            item("remote", "https://example.com/lib.rs", 1),
            item("bad", "not a uri", 1),
            no_name,
            no_selection,
            "file:///project/a.rs",
            item("kept", "file:///project/kept.rs", 4),
        ]);
        let names: Vec<_> = prepared(&reply).into_iter().map(|item| item.name).collect();
        assert_eq!(names, ["kept"]);
    }

    #[test]
    fn incoming_calls_give_each_caller_with_the_line_of_its_first_call() {
        let describe = item("describe", "file:///project/src/lib.rs", 23);
        let main = item("main", "file:///project/src/main.rs", 2);
        let reply = json!([
            {"from": describe, "fromRanges": [range(24, 27, 37)]},
            // Ranges out of order: the earliest line counts.
            {"from": main, "fromRanges": [range(9, 4, 14), range(5, 4, 14), range(7, 4, 14)]},
        ]);
        assert_eq!(
            incoming(&reply),
            [
                (
                    call_item("describe", "/project/src/lib.rs", 23, describe),
                    Some(24)
                ),
                (call_item("main", "/project/src/main.rs", 2, main), Some(5)),
            ]
        );
    }

    #[test]
    fn outgoing_calls_give_each_callee_with_the_line_of_its_first_call_in_the_caller() {
        let area = item("area", "file:///project/src/shapes.rs", 32);
        let reply = json!([{"to": area, "fromRanges": [range(18, 23, 27)]}]);
        assert_eq!(
            outgoing(&reply),
            [(
                call_item("area", "/project/src/shapes.rs", 32, area),
                Some(18)
            )]
        );
    }

    #[test]
    fn calls_without_ranges_have_no_call_line() {
        let area = item("area", "file:///project/src/lib.rs", 32);
        let reply = json!([
            {"to": area, "fromRanges": []},
            {"to": area},
            {"to": area, "fromRanges": [{"start": 3}]},
        ]);
        let lines: Vec<_> = outgoing(&reply).into_iter().map(|(_, line)| line).collect();
        assert_eq!(lines, [None, None, None]);
    }

    #[test]
    fn calls_with_a_skipped_item_or_the_wrong_side_are_skipped() {
        let lib = item("describe", "file:///project/src/lib.rs", 23);
        let reply = json!([
            {"from": item("x", "untitled:Untitled-1", 1), "fromRanges": [range(1, 0, 1)]},
            {"to": lib, "fromRanges": [range(1, 0, 1)]},
            {"fromRanges": [range(1, 0, 1)]},
            {"from": lib, "fromRanges": [range(24, 0, 1)]},
        ]);
        assert_eq!(
            incoming(&reply),
            [(
                call_item("describe", "/project/src/lib.rs", 23, lib.clone()),
                Some(24)
            )]
        );
        let reply = json!([
            {"from": lib, "fromRanges": [range(1, 0, 1)]},
            {"to": lib, "fromRanges": [range(2, 0, 1)]},
        ]);
        assert_eq!(
            outgoing(&reply),
            [(
                call_item("describe", "/project/src/lib.rs", 23, lib),
                Some(2)
            )]
        );
    }
}
