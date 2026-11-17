use serde_json::{Value, json};

/// Client capabilities for the features beyond the original subset, merged into the
/// base object `client_capabilities()` builds.
pub fn feature_client_capabilities() -> Value {
    json!({"workspace":{"applyEdit":true,
        "workspaceEdit":{"documentChanges":true,"resourceOperations":[],"failureHandling":"abort"},
        "symbol":{"symbolKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]}},
        "executeCommand":{}},
     "textDocument":{
        "completion":{"contextSupport":true,
            "completionItem":{"snippetSupport":true,"documentationFormat":["markdown","plaintext"],
                "deprecatedSupport":true,"preselectSupport":true,"tagSupport":{"valueSet":[1]},
                "labelDetailsSupport":true,
                "resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}},
            "completionItemKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]}},
        "signatureHelp":{"contextSupport":true,"signatureInformation":{"documentationFormat":["markdown","plaintext"],
            "parameterInformation":{"labelOffsetSupport":true},"activeParameterSupport":true}},
        "rename":{"prepareSupport":true},
        "codeAction":{"isPreferredSupport":true,"disabledSupport":true,"dataSupport":true,
            "resolveSupport":{"properties":["edit"]},
            "codeActionLiteralSupport":{"codeActionKind":{"valueSet":["","quickfix","refactor","refactor.extract","refactor.inline","refactor.rewrite","source","source.organizeImports","source.fixAll"]}}},
        "documentHighlight":{},"inlayHint":{},
        "typeDefinition":{"linkSupport":false},"implementation":{"linkSupport":false},
        "callHierarchy":{},"formatting":{}}})
}

/// Deep-merges `feature_client_capabilities()` into `base`: objects merge key by key,
/// any other value replaces the base value.
pub fn extend_client_capabilities(base: &mut Value) {
    merge(base, feature_client_capabilities());
}

fn merge(base: &mut Value, extra: Value) {
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => {
            for (key, value) in extra {
                match base.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, extra) => *base = extra,
    }
}

/// The server's `capabilities` object from its `initialize` reply, `Null` when unknown
/// (before the server is ready, or after it stopped).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerFeatures(pub Value);

impl ServerFeatures {
    /// Whether the server's capabilities are known, that is, a server is running.
    pub fn is_known(&self) -> bool {
        !self.0.is_null()
    }
    /// Whether the server offers `provider` (a key such as `renameProvider`): the key is
    /// present and neither `null` nor `false`.
    pub fn has(&self, provider: &str) -> bool {
        self.0
            .get(provider)
            .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
    }
    /// The characters that open the completion list (`completionProvider.triggerCharacters`).
    pub fn completion_triggers(&self) -> Vec<String> {
        self.strings("/completionProvider/triggerCharacters")
    }
    /// Whether completion items can be resolved for their details.
    pub fn completion_resolve(&self) -> bool {
        self.flag("/completionProvider/resolveProvider")
    }
    /// The characters that open parameter hints (`signatureHelpProvider.triggerCharacters`).
    pub fn signature_triggers(&self) -> Vec<String> {
        self.strings("/signatureHelpProvider/triggerCharacters")
    }
    /// The characters that refresh open parameter hints
    /// (`signatureHelpProvider.retriggerCharacters`).
    pub fn signature_retriggers(&self) -> Vec<String> {
        self.strings("/signatureHelpProvider/retriggerCharacters")
    }
    /// Whether the server checks a rename position before the user types a new name.
    pub fn rename_prepare(&self) -> bool {
        self.flag("/renameProvider/prepareProvider")
    }
    /// Whether code actions can be resolved for their edit.
    pub fn code_action_resolve(&self) -> bool {
        self.flag("/codeActionProvider/resolveProvider")
    }
    /// The commands the server executes (`executeCommandProvider.commands`).
    pub fn commands(&self) -> Vec<String> {
        self.strings("/executeCommandProvider/commands")
    }

    fn flag(&self, pointer: &str) -> bool {
        self.0.pointer(pointer) == Some(&Value::Bool(true))
    }
    fn strings(&self, pointer: &str) -> Vec<String> {
        self.0
            .pointer(pointer)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        json!({"general":{"positionEncodings":["utf-16"]},"workspace":{"configuration":true},
            "textDocument":{"completion":{"completionItem":{"snippetSupport":false}},"definition":{"linkSupport":true},"hover":{"contentFormat":["markdown","plaintext"]}}})
    }

    #[test]
    fn merging_keeps_the_base_keys_and_turns_snippets_on() {
        let mut capabilities = base();
        extend_client_capabilities(&mut capabilities);
        assert_eq!(
            capabilities.pointer("/textDocument/hover/contentFormat"),
            Some(&json!(["markdown", "plaintext"]))
        );
        assert_eq!(
            capabilities.pointer("/textDocument/definition/linkSupport"),
            Some(&json!(true))
        );
        assert_eq!(
            capabilities.pointer("/general/positionEncodings"),
            Some(&json!(["utf-16"]))
        );
        assert_eq!(
            capabilities.pointer("/workspace/configuration"),
            Some(&json!(true))
        );
        assert_eq!(
            capabilities.pointer("/textDocument/completion/completionItem/snippetSupport"),
            Some(&json!(true))
        );
    }

    #[test]
    fn merged_object_declares_rename_preparation_and_no_file_operations() {
        let mut capabilities = base();
        extend_client_capabilities(&mut capabilities);
        assert_eq!(
            capabilities.pointer("/textDocument/rename/prepareSupport"),
            Some(&json!(true))
        );
        assert_eq!(
            capabilities.pointer("/workspace/workspaceEdit/resourceOperations"),
            Some(&json!([]))
        );
        assert_eq!(
            capabilities.pointer("/workspace/applyEdit"),
            Some(&json!(true))
        );
    }

    #[test]
    fn merging_into_an_empty_object_gives_the_feature_capabilities() {
        let mut capabilities = json!({});
        extend_client_capabilities(&mut capabilities);
        assert_eq!(capabilities, feature_client_capabilities());
    }

    #[test]
    fn feature_capabilities_are_exactly_the_planned_object() {
        assert_eq!(
            feature_client_capabilities(),
            json!({"workspace":{"applyEdit":true,
                "workspaceEdit":{"documentChanges":true,"resourceOperations":[],"failureHandling":"abort"},
                "symbol":{"symbolKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]}},
                "executeCommand":{}},
             "textDocument":{
                "completion":{"contextSupport":true,
                    "completionItem":{"snippetSupport":true,"documentationFormat":["markdown","plaintext"],
                        "deprecatedSupport":true,"preselectSupport":true,"tagSupport":{"valueSet":[1]},
                        "labelDetailsSupport":true,
                        "resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}},
                    "completionItemKind":{"valueSet":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]}},
                "signatureHelp":{"contextSupport":true,"signatureInformation":{"documentationFormat":["markdown","plaintext"],
                    "parameterInformation":{"labelOffsetSupport":true},"activeParameterSupport":true}},
                "rename":{"prepareSupport":true},
                "codeAction":{"isPreferredSupport":true,"disabledSupport":true,"dataSupport":true,
                    "resolveSupport":{"properties":["edit"]},
                    "codeActionLiteralSupport":{"codeActionKind":{"valueSet":["","quickfix","refactor","refactor.extract","refactor.inline","refactor.rewrite","source","source.organizeImports","source.fixAll"]}}},
                "documentHighlight":{},"inlayHint":{},
                "typeDefinition":{"linkSupport":false},"implementation":{"linkSupport":false},
                "callHierarchy":{},"formatting":{}}})
        );
    }

    #[test]
    fn a_non_object_base_value_is_replaced() {
        let mut capabilities = json!({"textDocument":{"completion":true}});
        extend_client_capabilities(&mut capabilities);
        assert_eq!(
            capabilities.pointer("/textDocument/completion/contextSupport"),
            Some(&json!(true))
        );
    }

    #[test]
    fn a_provider_is_present_unless_missing_null_or_false() {
        let features = ServerFeatures(json!({
            "renameProvider": true,
            "hoverProvider": {},
            "inlayHintProvider": false,
            "callHierarchyProvider": null,
        }));
        assert!(features.has("renameProvider"));
        assert!(features.has("hoverProvider"));
        assert!(!features.has("inlayHintProvider"));
        assert!(!features.has("callHierarchyProvider"));
        assert!(!features.has("documentHighlightProvider"));
    }

    #[test]
    fn trigger_retrigger_and_command_lists_parse() {
        let features = ServerFeatures(json!({
            "completionProvider": {"triggerCharacters": [".", ":"], "resolveProvider": true},
            "signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [")"]},
            "renameProvider": {"prepareProvider": true},
            "codeActionProvider": {"resolveProvider": true},
            "executeCommandProvider": {"commands": ["one", "two"]},
        }));
        assert_eq!(features.completion_triggers(), [".", ":"]);
        assert!(features.completion_resolve());
        assert_eq!(features.signature_triggers(), ["(", ","]);
        assert_eq!(features.signature_retriggers(), [")"]);
        assert!(features.rename_prepare());
        assert!(features.code_action_resolve());
        assert_eq!(features.commands(), ["one", "two"]);
    }

    #[test]
    fn lists_are_empty_and_flags_false_when_absent() {
        let features = ServerFeatures(json!({
            "completionProvider": {},
            "renameProvider": true,
            "codeActionProvider": true,
        }));
        assert!(features.completion_triggers().is_empty());
        assert!(!features.completion_resolve());
        assert!(features.signature_triggers().is_empty());
        assert!(features.signature_retriggers().is_empty());
        assert!(!features.rename_prepare());
        assert!(!features.code_action_resolve());
        assert!(features.commands().is_empty());
    }

    #[test]
    fn unknown_capabilities_are_not_known() {
        assert!(!ServerFeatures::default().is_known());
        assert!(!ServerFeatures::default().has("completionProvider"));
        assert!(ServerFeatures(json!({})).is_known());
    }
}
