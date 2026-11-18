//! Parameter hints: the language server's signatures for the call around the cursor, read
//! for the signature card, and the context Tandem sends with each request for them.
use crate::position_params;
use anyhow::{Context, Result};
use lsp_types::{
    Documentation, ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpTriggerKind,
};
use serde_json::{Value, json};
use std::ops::Range;

/// The signature the card shows.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureView {
    /// The signature as the server writes it, such as `describe(side: f64) -> String`.
    pub label: String,
    /// The byte range of the active parameter in `label`, when one is active and found.
    pub active: Option<Range<usize>>,
    /// The signature's documentation, as Markdown or plain text.
    pub documentation: Option<String>,
    /// The active parameter's documentation, as Markdown or plain text.
    pub parameter_documentation: Option<String>,
    /// Which signature this is, counted from zero, and how many the server offers.
    pub index: usize,
    pub count: usize,
}

/// The signatures of a textDocument/signatureHelp reply: `None` for `null` or a reply
/// without signatures, which both mean there is no call to describe.
pub fn signature_help(value: Value) -> Result<Option<SignatureHelp>> {
    if value.is_null() {
        return Ok(None);
    }
    let help: SignatureHelp = serde_json::from_value(value)
        .context("Could not read the language server's parameter hints")?;
    Ok((!help.signatures.is_empty()).then_some(help))
}

/// Signature `chosen` or `activeSignature` (clamped); active parameter from the
/// signature's `activeParameter` or the help's; `ParameterLabel::Simple` is found in the
/// label (first occurrence after the first `(` when there is one), `LabelOffsets` are
/// UTF-16 offsets into the label. Markup or plain documentation as text.
///
/// A missing active parameter is the first one, as the protocol says; one past the
/// signature's parameters highlights nothing. A simple label is looked for after the
/// parameters before it, so repeated labels (`pair(T, T)`) are told apart.
pub fn signature_view(help: &SignatureHelp, chosen: Option<usize>) -> Option<SignatureView> {
    let last = help.signatures.len().checked_sub(1)?;
    let index = chosen
        .or(help.active_signature.map(|index| index as usize))
        .unwrap_or(0)
        .min(last);
    let signature = &help.signatures[index];
    let parameters = signature.parameters.as_deref().unwrap_or_default();
    let active_index = signature
        .active_parameter
        .or(help.active_parameter)
        .unwrap_or(0) as usize;
    let parameter = parameters.get(active_index);
    Some(SignatureView {
        label: signature.label.clone(),
        active: parameter.and_then(|_| parameter_range(&signature.label, parameters, active_index)),
        documentation: documentation_text(signature.documentation.as_ref()),
        parameter_documentation: parameter
            .and_then(|parameter| documentation_text(parameter.documentation.as_ref())),
        index,
        count: help.signatures.len(),
    })
}

/// `context` for textDocument/signatureHelp: triggerKind 1 invoked, 2 with the trigger
/// character, 3 content change; isRetrigger and activeSignatureHelp when one is shown.
pub fn signature_params(
    text: &str,
    cursor: usize,
    trigger: Option<char>,
    content_change: bool,
    active: Option<&SignatureHelp>,
) -> Result<Value> {
    let mut params = position_params(text, cursor)?;
    let mut context = match (trigger, content_change) {
        (Some(character), _) => json!({
            "triggerKind": SignatureHelpTriggerKind::TRIGGER_CHARACTER,
            "triggerCharacter": character.to_string(),
        }),
        (None, true) => json!({"triggerKind": SignatureHelpTriggerKind::CONTENT_CHANGE}),
        (None, false) => json!({"triggerKind": SignatureHelpTriggerKind::INVOKED}),
    };
    context["isRetrigger"] = json!(active.is_some());
    if let Some(active) = active {
        context["activeSignatureHelp"] = serde_json::to_value(active)?;
    }
    params["context"] = context;
    Ok(params)
}

/// The byte range in `label` of parameter `index` of `parameters`. Simple labels are
/// looked for from the first `(` on, each after the ones found before it.
fn parameter_range(
    label: &str,
    parameters: &[ParameterInformation],
    index: usize,
) -> Option<Range<usize>> {
    let mut from = label.find('(').map_or(0, |open| open + 1);
    for (at, parameter) in parameters.iter().enumerate().take(index + 1) {
        let range = match &parameter.label {
            ParameterLabel::Simple(name) if name.is_empty() => None,
            ParameterLabel::Simple(name) => label[from..]
                .find(name.as_str())
                .map(|found| from + found..from + found + name.len()),
            ParameterLabel::LabelOffsets([start, end]) => utf16_byte(label, *start)
                .zip(utf16_byte(label, *end))
                .filter(|(start, end)| start <= end)
                .map(|(start, end)| start..end),
        };
        if at == index {
            return range;
        }
        if let Some(range) = range {
            from = from.max(range.end);
        }
    }
    None
}

/// The byte offset in `text` of `units` UTF-16 code units, or `None` when that is past
/// the end of `text` or inside a character.
fn utf16_byte(text: &str, units: u32) -> Option<usize> {
    let mut counted = 0;
    for (byte, character) in text.char_indices() {
        if counted == units {
            return Some(byte);
        }
        counted += character.len_utf16() as u32;
        if counted > units {
            return None;
        }
    }
    (counted == units).then_some(text.len())
}

/// The text of a signature's or a parameter's documentation, Markdown or plain; `None`
/// when there is none or it is blank.
fn documentation_text(documentation: Option<&Documentation>) -> Option<String> {
    let text = match documentation? {
        Documentation::String(text) => text,
        Documentation::MarkupContent(markup) => &markup.value,
    };
    (!text.trim().is_empty()).then(|| text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn help(value: Value) -> SignatureHelp {
        serde_json::from_value(value).unwrap()
    }

    /// The text of `view.label` that `view.active` covers.
    fn active_text(view: &SignatureView) -> Option<&str> {
        view.active.clone().map(|range| &view.label[range])
    }

    /// The length of `text` in UTF-16 code units, as label offsets count it.
    fn utf16_len(text: &str) -> u32 {
        text.encode_utf16().count() as u32
    }

    const DESCRIBE: &str = "describe(side: f64, label: &str) -> String";

    fn describe(active_parameter: Value) -> SignatureHelp {
        help(json!({
            "signatures": [{
                "label": DESCRIBE,
                "parameters": [{"label": "side: f64"}, {"label": "label: &str"}],
            }],
            "activeSignature": 0,
            "activeParameter": active_parameter,
        }))
    }

    #[test]
    fn a_simple_parameter_label_is_found_in_the_signature() {
        let view = signature_view(&describe(json!(1)), None).unwrap();
        assert_eq!(view.label, DESCRIBE);
        assert_eq!(active_text(&view), Some("label: &str"));
        assert_eq!(view.active, Some(20..31));
        assert_eq!((view.index, view.count), (0, 1));
        let first = signature_view(&describe(json!(0)), None).unwrap();
        assert_eq!(first.active, Some(9..18));
    }

    #[test]
    fn a_simple_label_is_looked_for_after_the_opening_parenthesis() {
        // `label` is also the function's name, before the parenthesis.
        let named = help(json!({
            "signatures": [{"label": "label(label: &str)", "parameters": [{"label": "label"}]}],
            "activeParameter": 0,
        }));
        assert_eq!(signature_view(&named, None).unwrap().active, Some(6..11));
        // Without a parenthesis the whole label is searched.
        let bare = help(json!({
            "signatures": [{"label": "side: f64", "parameters": [{"label": "side"}]}],
            "activeParameter": 0,
        }));
        assert_eq!(signature_view(&bare, None).unwrap().active, Some(0..4));
    }

    #[test]
    fn repeated_simple_labels_are_found_in_order() {
        let pair = help(json!({
            "signatures": [{"label": "pair(T, T)", "parameters": [{"label": "T"}, {"label": "T"}]}],
            "activeParameter": 1,
        }));
        assert_eq!(signature_view(&pair, None).unwrap().active, Some(8..9));
    }

    #[test]
    fn a_simple_label_missing_from_the_signature_highlights_nothing() {
        let missing = help(json!({
            "signatures": [{"label": "f(a: i32)", "parameters": [{"label": "b"}, {"label": ""}]}],
            "activeParameter": 0,
        }));
        assert_eq!(signature_view(&missing, None).unwrap().active, None);
        let empty = SignatureHelp {
            active_parameter: Some(1),
            ..missing
        };
        assert_eq!(signature_view(&empty, None).unwrap().active, None);
    }

    #[test]
    fn label_offsets_count_utf16_units_in_a_non_ascii_label() {
        let label = "décrire(côté: f64, 🙂étiquette: &str) -> String";
        let first = "côté: f64";
        let second = "🙂étiquette: &str";
        let offsets = |part: &str| {
            let start = utf16_len(&label[..label.find(part).unwrap()]);
            json!([start, start + utf16_len(part)])
        };
        let help = help(json!({
            "signatures": [{
                "label": label,
                "parameters": [{"label": offsets(first)}, {"label": offsets(second)}],
            }],
            "activeParameter": 1,
        }));
        let view = signature_view(&help, None).unwrap();
        assert_eq!(active_text(&view), Some(second));
        let start = label.find(second).unwrap();
        assert_eq!(view.active, Some(start..start + second.len()));
    }

    #[test]
    fn label_offsets_outside_the_label_or_inside_a_character_highlight_nothing() {
        let label = "f(🙂: u8)";
        let with_offsets = |offsets: Value| {
            help(json!({
                "signatures": [{"label": label, "parameters": [{"label": offsets}]}],
                "activeParameter": 0,
            }))
        };
        // Past the end of the label.
        let past = with_offsets(json!([2, 99]));
        assert_eq!(signature_view(&past, None).unwrap().active, None);
        // Between the two halves of the emoji's surrogate pair.
        let split = with_offsets(json!([3, 8]));
        assert_eq!(signature_view(&split, None).unwrap().active, None);
        // Backwards.
        let backwards = with_offsets(json!([6, 2]));
        assert_eq!(signature_view(&backwards, None).unwrap().active, None);
        let whole = with_offsets(json!([2, 8]));
        assert_eq!(
            active_text(&signature_view(&whole, None).unwrap()),
            Some("🙂: u8")
        );
    }

    #[test]
    fn a_bad_earlier_parameter_does_not_hide_the_active_one() {
        let help = help(json!({
            "signatures": [{
                "label": "f(a: u8, b: u8)",
                "parameters": [{"label": [40, 50]}, {"label": [9, 14]}],
            }],
            "activeParameter": 1,
        }));
        let view = signature_view(&help, None).unwrap();
        assert_eq!(active_text(&view), Some("b: u8"));
    }

    fn overloads(active_signature: Value) -> SignatureHelp {
        help(json!({
            "signatures": [
                {"label": "open(path: str)", "parameters": [{"label": "path: str"}]},
                {"label": "open(path: str, mode: str)",
                 "parameters": [{"label": "path: str"}, {"label": "mode: str"}],
                 "activeParameter": 1},
            ],
            "activeSignature": active_signature,
            "activeParameter": 0,
        }))
    }

    #[test]
    fn the_chosen_signature_comes_before_the_active_one() {
        let first = signature_view(&overloads(json!(0)), None).unwrap();
        assert_eq!((first.index, first.count), (0, 2));
        assert_eq!(first.label, "open(path: str)");
        let chosen = signature_view(&overloads(json!(0)), Some(1)).unwrap();
        assert_eq!((chosen.index, chosen.count), (1, 2));
        assert_eq!(chosen.label, "open(path: str, mode: str)");
    }

    #[test]
    fn an_out_of_range_signature_is_clamped_to_the_last() {
        let active = signature_view(&overloads(json!(7)), None).unwrap();
        assert_eq!(active.index, 1);
        let chosen = signature_view(&overloads(json!(0)), Some(9)).unwrap();
        assert_eq!(chosen.index, 1);
    }

    #[test]
    fn the_signatures_active_parameter_comes_before_the_helps() {
        let view = signature_view(&overloads(json!(1)), None).unwrap();
        assert_eq!(active_text(&view), Some("mode: str"));
        let help_level = signature_view(&overloads(json!(0)), None).unwrap();
        assert_eq!(active_text(&help_level), Some("path: str"));
    }

    #[test]
    fn an_out_of_range_parameter_highlights_nothing() {
        let view = signature_view(&describe(json!(2)), None).unwrap();
        assert_eq!(view.active, None);
        assert_eq!(view.parameter_documentation, None);
    }

    #[test]
    fn a_missing_active_parameter_is_the_first() {
        let view = signature_view(&describe(Value::Null), None).unwrap();
        assert_eq!(active_text(&view), Some("side: f64"));
        let no_parameters = help(json!({"signatures": [{"label": "now() -> Instant"}]}));
        let view = signature_view(&no_parameters, None).unwrap();
        assert_eq!(view.active, None);
        assert_eq!(view.label, "now() -> Instant");
    }

    #[test]
    fn no_signature_has_no_view() {
        let empty = help(json!({"signatures": []}));
        assert_eq!(signature_view(&empty, None), None);
        assert_eq!(signature_view(&empty, Some(0)), None);
    }

    #[test]
    fn markup_and_plain_documentation_are_given_as_text() {
        let help = help(json!({
            "signatures": [{
                "label": DESCRIBE,
                "documentation": {"kind": "markdown", "value": "Describes a **square**."},
                "parameters": [
                    {"label": "side: f64", "documentation": "The side, in metres."},
                    {"label": "label: &str",
                     "documentation": {"kind": "plaintext", "value": "What to call it."}},
                ],
            }],
            "activeParameter": 1,
        }));
        let view = signature_view(&help, None).unwrap();
        assert_eq!(
            view.documentation.as_deref(),
            Some("Describes a **square**.")
        );
        assert_eq!(
            view.parameter_documentation.as_deref(),
            Some("What to call it.")
        );
        let first = SignatureHelp {
            active_parameter: Some(0),
            ..help
        };
        let view = signature_view(&first, None).unwrap();
        assert_eq!(
            view.parameter_documentation.as_deref(),
            Some("The side, in metres.")
        );
    }

    #[test]
    fn blank_documentation_is_none() {
        let help = help(json!({
            "signatures": [{
                "label": "f(a)",
                "documentation": {"kind": "markdown", "value": "  \n"},
                "parameters": [{"label": "a", "documentation": ""}],
            }],
        }));
        let view = signature_view(&help, None).unwrap();
        assert_eq!(view.documentation, None);
        assert_eq!(view.parameter_documentation, None);
    }

    #[test]
    fn a_reply_without_signatures_is_none() {
        assert_eq!(signature_help(Value::Null).unwrap(), None);
        assert_eq!(signature_help(json!({"signatures": []})).unwrap(), None);
        let some = signature_help(json!({"signatures": [{"label": "f()"}]})).unwrap();
        assert_eq!(some.map(|help| help.signatures.len()), Some(1));
        assert!(signature_help(json!({"signatures": "f()"})).is_err());
    }

    #[test]
    fn invoked_parameter_hints_are_kind_one_and_not_a_retrigger() {
        let text = "fn main() {\n    describe(\n}";
        let cursor = text.find("describe(").unwrap() + "describe(".len();
        let params = signature_params(text, cursor, None, false, None).unwrap();
        assert_eq!(
            params,
            json!({
                "position": {"line": 1, "character": 13},
                "context": {"triggerKind": 1, "isRetrigger": false},
            })
        );
    }

    #[test]
    fn a_trigger_character_is_kind_two_with_the_character() {
        let params = signature_params("f(", 2, Some('('), false, None).unwrap();
        assert_eq!(
            params["context"],
            json!({"triggerKind": 2, "triggerCharacter": "(", "isRetrigger": false})
        );
        // A typed trigger character wins over the content change it also is.
        let params = signature_params("f(a,", 4, Some(','), true, None).unwrap();
        assert_eq!(params["context"]["triggerKind"], json!(2));
        assert_eq!(params["context"]["triggerCharacter"], json!(","));
    }

    #[test]
    fn a_content_change_is_kind_three() {
        let params = signature_params("f(a", 3, None, true, None).unwrap();
        assert_eq!(
            params["context"],
            json!({"triggerKind": 3, "isRetrigger": false})
        );
    }

    #[test]
    fn hints_on_screen_make_a_retrigger_that_carries_them() {
        let shown = describe(json!(0));
        let params = signature_params("describe(1.0, ", 14, None, true, Some(&shown)).unwrap();
        assert_eq!(params["context"]["triggerKind"], json!(3));
        assert_eq!(params["context"]["isRetrigger"], json!(true));
        assert_eq!(
            params["context"]["activeSignatureHelp"],
            serde_json::to_value(&shown).unwrap()
        );
        let typed = signature_params("describe(1.0,", 13, Some(','), false, Some(&shown)).unwrap();
        assert_eq!(typed["context"]["isRetrigger"], json!(true));
        assert_eq!(typed["context"]["triggerKind"], json!(2));
    }

    #[test]
    fn the_position_counts_utf16_units() {
        let text = "let é = f(🙂, ";
        let params = signature_params(text, text.len(), None, false, None).unwrap();
        assert_eq!(
            params["position"],
            json!({"line": 0, "character": utf16_len(text)})
        );
        assert!(signature_params(text, 5, None, false, None).is_err());
    }
}
