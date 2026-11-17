//! A language server paired with a TypeScript companion (Vue): the tsserver requests the main
//! server relays through the client, and how the two servers' answers are merged.
use serde_json::{Value, json};

/// (id, command, args) from tsserver/request params `[[id, command, args]]`; also accepts the flat
/// `[id, command, args]`.
pub(crate) fn relay_request(params: &Value) -> Option<(Value, String, Value)> {
    let request = match params.as_array()?.as_slice() {
        [Value::Array(nested)] => nested.as_slice(),
        flat => flat,
    };
    match request {
        [id, Value::String(command), args] => Some((id.clone(), command.clone(), args.clone())),
        _ => None,
    }
}

/// tsserver/response params: `[[id, body]]`.
pub(crate) fn relay_response(id: Value, body: Value) -> Value {
    json!([[id, body]])
}

/// Methods answered by both servers and merged.
pub(crate) const MERGED: [&str; 6] = [
    "textDocument/hover",
    "textDocument/completion",
    "textDocument/definition",
    "textDocument/references",
    "textDocument/documentSymbol",
    "textDocument/diagnostic",
];

/// One answer from the main server's and the companion's replies to `method`.
pub(crate) fn merge_replies(method: &str, main: Value, companion: Value) -> Value {
    match method {
        "textDocument/hover" if main.is_null() => companion,
        "textDocument/completion" => {
            if main.is_null() && companion.is_null() {
                return Value::Null;
            }
            let incomplete = [&main, &companion]
                .iter()
                .any(|reply| reply["isIncomplete"] == true);
            let mut items = completion_items(main);
            items.extend(completion_items(companion));
            json!({"isIncomplete": incomplete, "items": items})
        }
        "textDocument/definition" | "textDocument/references" => {
            let mut locations: Vec<Value> = vec![];
            for location in into_list(main).into_iter().chain(into_list(companion)) {
                if !locations.contains(&location) {
                    locations.push(location);
                }
            }
            Value::Array(locations)
        }
        "textDocument/documentSymbol"
            if !main.as_array().is_some_and(|symbols| !symbols.is_empty()) =>
        {
            companion
        }
        "textDocument/diagnostic" => {
            let mut items = diagnostic_items(main);
            items.extend(diagnostic_items(companion));
            json!({"kind": "full", "items": items})
        }
        _ => main,
    }
}

/// A completion reply's items: a `CompletionList`'s, an array as is, none for null.
fn completion_items(reply: Value) -> Vec<Value> {
    match reply {
        Value::Object(mut list) => into_list(list.remove("items").unwrap_or_default()),
        other => into_list(other),
    }
}

/// A full diagnostic report's items; none for null or any other reply.
fn diagnostic_items(mut report: Value) -> Vec<Value> {
    into_list(report.get_mut("items").map(Value::take).unwrap_or_default())
}

/// An array as is, null as empty, any other value as a list of one.
fn into_list(value: Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values,
        Value::Null => vec![],
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_request_reads_nested_and_flat_params() {
        let args = json!({"file": "/p/App.vue", "line": 1});
        assert_eq!(
            relay_request(&json!([[7, "_vue:quickinfo", args]])),
            Some((json!(7), "_vue:quickinfo".to_string(), args.clone()))
        );
        assert_eq!(
            relay_request(&json!([8, "_vue:projectInfo", args])),
            Some((json!(8), "_vue:projectInfo".to_string(), args.clone()))
        );
        // Arguments vscode-jsonrpc serialised from `undefined`.
        assert_eq!(
            relay_request(&json!([[9, "_vue:x", null]])),
            Some((json!(9), "_vue:x".to_string(), Value::Null))
        );
    }

    #[test]
    fn relay_request_rejects_anything_else() {
        for params in [
            json!(null),
            json!({"id": 7, "command": "_vue:quickinfo"}),
            json!("_vue:quickinfo"),
            json!([]),
            json!([[]]),
            json!([[7]]),
            json!([[7, 8, {}]]),
            json!([7, "_vue:quickinfo"]),
            json!([7, "_vue:quickinfo", {}, "extra"]),
            json!([[7, "_vue:quickinfo", {}], [8, "_vue:quickinfo", {}]]),
        ] {
            assert_eq!(relay_request(&params), None, "{params}");
        }
    }

    #[test]
    fn relay_response_wraps_the_id_and_body() {
        assert_eq!(
            relay_response(json!(7), json!({"displayString": "x"})),
            json!([[7, {"displayString": "x"}]])
        );
        assert_eq!(relay_response(json!(8), Value::Null), json!([[8, null]]));
    }

    fn location(line: u32) -> Value {
        json!({"uri": "file:///p/App.vue", "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 1}}})
    }

    #[test]
    fn hover_prefers_the_main_server() {
        let vue = json!({"contents": "vue"});
        let ts = json!({"contents": "ts"});
        let hover = "textDocument/hover";
        assert_eq!(merge_replies(hover, vue.clone(), ts.clone()), vue);
        assert_eq!(merge_replies(hover, Value::Null, ts.clone()), ts);
        assert_eq!(merge_replies(hover, Value::Null, Value::Null), Value::Null);
    }

    #[test]
    fn completion_joins_lists_and_arrays() {
        let completion = "textDocument/completion";
        assert_eq!(
            merge_replies(
                completion,
                json!({"isIncomplete": false, "items": [{"label": "fromVue"}]}),
                json!([{"label": "fromTs"}]),
            ),
            json!({"isIncomplete": false, "items": [{"label": "fromVue"}, {"label": "fromTs"}]})
        );
        assert_eq!(
            merge_replies(
                completion,
                json!([{"label": "fromVue"}]),
                json!({"isIncomplete": true, "items": [{"label": "fromTs"}]}),
            ),
            json!({"isIncomplete": true, "items": [{"label": "fromVue"}, {"label": "fromTs"}]})
        );
        assert_eq!(
            merge_replies(completion, Value::Null, json!([{"label": "fromTs"}])),
            json!({"isIncomplete": false, "items": [{"label": "fromTs"}]})
        );
        assert_eq!(
            merge_replies(completion, Value::Null, Value::Null),
            Value::Null
        );
    }

    #[test]
    fn locations_are_joined_without_duplicates() {
        for method in ["textDocument/definition", "textDocument/references"] {
            assert_eq!(
                merge_replies(
                    method,
                    json!([location(1), location(2)]),
                    json!([location(2), location(3)]),
                ),
                json!([location(1), location(2), location(3)]),
                "{method}"
            );
            // A single location counts as a list of one.
            assert_eq!(
                merge_replies(method, location(1), json!([location(1)])),
                json!([location(1)]),
                "{method}"
            );
            assert_eq!(
                merge_replies(method, Value::Null, location(4)),
                json!([location(4)]),
                "{method}"
            );
            assert_eq!(
                merge_replies(method, Value::Null, Value::Null),
                json!([]),
                "{method}"
            );
        }
    }

    #[test]
    fn document_symbols_come_from_the_main_server_when_it_has_any() {
        let symbols = "textDocument/documentSymbol";
        let vue = json!([{"name": "template"}]);
        let ts = json!([{"name": "count"}]);
        assert_eq!(merge_replies(symbols, vue.clone(), ts.clone()), vue);
        assert_eq!(merge_replies(symbols, json!([]), ts.clone()), ts);
        assert_eq!(merge_replies(symbols, Value::Null, ts.clone()), ts);
        assert_eq!(
            merge_replies(symbols, Value::Null, Value::Null),
            Value::Null
        );
    }

    #[test]
    fn diagnostics_are_concatenated() {
        let diagnostic = "textDocument/diagnostic";
        assert_eq!(
            merge_replies(
                diagnostic,
                json!({"kind": "full", "items": [{"message": "vue"}]}),
                json!({"kind": "full", "items": [{"message": "ts"}]}),
            ),
            json!({"kind": "full", "items": [{"message": "vue"}, {"message": "ts"}]})
        );
        assert_eq!(
            merge_replies(
                diagnostic,
                Value::Null,
                json!({"kind": "full", "items": [{"message": "ts"}]}),
            ),
            json!({"kind": "full", "items": [{"message": "ts"}]})
        );
    }

    #[test]
    fn other_methods_answer_from_the_main_server() {
        assert!(!MERGED.contains(&"textDocument/formatting"));
        assert_eq!(
            merge_replies(
                "textDocument/formatting",
                json!([]),
                json!([{"newText": "x"}])
            ),
            json!([])
        );
        assert_eq!(
            merge_replies("textDocument/formatting", Value::Null, json!([])),
            Value::Null
        );
    }
}
