//! Per-language editing rules: the brackets, the pairs typing closes automatically and the
//! lines that indent or dedent the next one. Registered with the toolkit, which applies
//! them while typing and on Enter.
use gpui_kit::App;
use gpui_kit::component::input::language_config::LanguageConfig;
use gpui_kit::component::input::{
    AutoClosingPair, BracketPair, IndentationRules, SyntaxContext, set_language_config,
};
use regex::Regex;

/// One language's editing rules.
pub struct Rules {
    /// Structural pairs, for indentation and splitting Enter between them.
    pub brackets: &'static [(&'static str, &'static str)],
    /// Pairs closed automatically, never inside strings or comments.
    pub pairs: &'static [(&'static str, &'static str)],
    /// A line matching this regex indents the line after it.
    pub increase: &'static str,
    /// Text after the cursor matching this regex dedents the new line on Enter.
    pub decrease: &'static str,
}

/// The language ids Tandem has editing rules for.
const LANGUAGES: [&str; 15] = [
    "rust",
    "python",
    "java",
    "swift",
    "javascript",
    "javascriptreact",
    "typescript",
    "typescriptreact",
    "vue",
    "svelte",
    "html",
    "css",
    "scss",
    "less",
    "json",
];

/// Names the toolkit may use for a language, and the language id whose rules they get.
const ALIASES: [(&str, &str); 2] = [("tsx", "typescriptreact"), ("jsx", "javascriptreact")];

const BRACKETS: &[(&str, &str)] = &[("{", "}"), ("[", "]"), ("(", ")")];

/// Brackets and both quotes.
const PAIRS: &[(&str, &str)] = &[("{", "}"), ("[", "]"), ("(", ")"), ("\"", "\""), ("'", "'")];

/// A line ending in an open bracket, before an optional `//` comment.
const C_INCREASE: &str = r"[\{\(\[]\s*(//.*)?$";

/// A line starting with a closing bracket.
const CLOSER: &str = r"^\s*[\}\]\)]";

const MARKUP_INCREASE: &str = r"<(?:html|head|body|div|span|p|ul|ol|li|table|thead|tbody|tfoot|tr|td|th|section|article|header|footer|nav|main|aside|form|select|option|button|label|textarea|script|style|template|a|em|strong|pre|code|blockquote|figure|figcaption|details|summary|dl|dt|dd|h[1-6]|svg|g|video|audio|canvas|iframe|fieldset|legend|picture|noscript|slot|[a-z]+-[a-z-]+|[A-Z][A-Za-z0-9]*)\b(?:[^>]*[^/>])?>\s*$|[\{\(\[]\s*$";

/// The rules for a language id, or None for a language without them.
pub fn rules(language: &str) -> Option<Rules> {
    let rules = match language {
        // No `'` pair: it starts a lifetime as often as a char.
        "rust" => Rules {
            brackets: BRACKETS,
            pairs: &[("{", "}"), ("[", "]"), ("(", ")"), ("\"", "\"")],
            increase: C_INCREASE,
            decrease: CLOSER,
        },
        "python" => Rules {
            brackets: BRACKETS,
            pairs: PAIRS,
            increase: r"^\s*(?:(?:async\s+)?def|class|for|if|elif|else|except|finally|try|while|with|match|case)\b.*:\s*(#.*)?$|[\{\(\[]\s*(#.*)?$",
            decrease: CLOSER,
        },
        "java" => Rules {
            brackets: BRACKETS,
            pairs: PAIRS,
            increase: C_INCREASE,
            decrease: CLOSER,
        },
        "swift" => Rules {
            brackets: BRACKETS,
            pairs: &[("{", "}"), ("[", "]"), ("(", ")"), ("\"", "\"")],
            increase: C_INCREASE,
            decrease: CLOSER,
        },
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" => Rules {
            brackets: BRACKETS,
            pairs: &[
                ("{", "}"),
                ("[", "]"),
                ("(", ")"),
                ("\"", "\""),
                ("'", "'"),
                ("`", "`"),
            ],
            increase: C_INCREASE,
            decrease: CLOSER,
        },
        "html" | "vue" | "svelte" => Rules {
            brackets: BRACKETS,
            pairs: &[
                ("{", "}"),
                ("[", "]"),
                ("(", ")"),
                ("\"", "\""),
                ("'", "'"),
                ("<!--", "-->"),
            ],
            increase: MARKUP_INCREASE,
            decrease: r"^\s*(?:</|[\}\]\)])",
        },
        "css" => Rules {
            brackets: BRACKETS,
            pairs: PAIRS,
            increase: r"[\{\(\[]\s*(/\*.*\*/)?$",
            decrease: CLOSER,
        },
        "scss" | "less" => Rules {
            brackets: BRACKETS,
            pairs: PAIRS,
            increase: r"[\{\(\[]\s*(//.*|/\*.*\*/)?$",
            decrease: CLOSER,
        },
        "json" => Rules {
            brackets: &[("{", "}"), ("[", "]")],
            pairs: &[("{", "}"), ("[", "]"), ("\"", "\"")],
            increase: r"[\{\[]\s*$",
            decrease: r"^\s*[\}\]]",
        },
        _ => return None,
    };
    Some(rules)
}

/// The toolkit configuration for `rules`; fails only on a regex that does not compile.
fn language_config(rules: &Rules) -> Result<LanguageConfig, regex::Error> {
    let indentation =
        IndentationRules::new(Regex::new(rules.increase)?, Regex::new(rules.decrease)?);
    Ok(LanguageConfig::default()
        .brackets(
            rules
                .brackets
                .iter()
                .map(|&(open, close)| BracketPair::new(open, close)),
        )
        .auto_closing_pairs(rules.pairs.iter().map(|&(open, close)| {
            AutoClosingPair::new(open, close)
                .not_in([SyntaxContext::String, SyntaxContext::Comment])
        }))
        .indentation_rules(indentation))
}

/// The names `register_language_rules` registers, each with the language id whose rules
/// it gets.
fn registrations() -> impl Iterator<Item = (&'static str, &'static str)> {
    LANGUAGES
        .into_iter()
        .map(|language| (language, language))
        .chain(ALIASES)
}

/// Registers a toolkit LanguageConfig for every language id, and for the aliases
/// "tsx" (typescriptreact rules) and "jsx" (javascriptreact rules).
pub fn register_language_rules(cx: &mut App) {
    for (name, language) in registrations() {
        if let Some(config) = rules(language).and_then(|rules| language_config(&rules).ok()) {
            set_language_config(name, config, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn increase(language: &str) -> Regex {
        Regex::new(rules(language).unwrap().increase).unwrap()
    }

    fn decrease(language: &str) -> Regex {
        Regex::new(rules(language).unwrap().decrease).unwrap()
    }

    #[test]
    fn every_registered_name_has_rules_whose_regexes_compile() {
        let mut count = 0;
        for (name, language) in registrations() {
            let rules = rules(language).unwrap_or_else(|| panic!("{name} has no rules"));
            Regex::new(rules.increase).unwrap_or_else(|e| panic!("{name} increase: {e}"));
            Regex::new(rules.decrease).unwrap_or_else(|e| panic!("{name} decrease: {e}"));
            assert!(language_config(&rules).is_ok(), "{name}");
            count += 1;
        }
        assert_eq!(count, LANGUAGES.len() + ALIASES.len());
    }

    #[test]
    fn rust_does_not_close_single_quotes() {
        let rust = rules("rust").unwrap();
        assert!(!rust.pairs.iter().any(|(open, _)| *open == "'"));
        assert!(rust.pairs.contains(&("\"", "\"")));
    }

    #[test]
    fn python_indents_after_block_headers_and_open_brackets() {
        let increase = increase("python");
        assert!(increase.is_match("def f(x):"));
        assert!(increase.is_match("    async def f(x):"));
        assert!(increase.is_match("else:  # note"));
        assert!(increase.is_match("items = ["));
        assert!(!increase.is_match("x = 1"));
    }

    #[test]
    fn javascript_dedents_before_a_closing_brace() {
        assert!(decrease("javascript").is_match("  }"));
        assert!(!decrease("javascript").is_match("  x }"));
    }

    #[test]
    fn html_indents_after_container_tags_only() {
        let increase = increase("html");
        assert!(increase.is_match("<div>"));
        assert!(increase.is_match("<div class=\"a\">"));
        assert!(increase.is_match("<MyComponent>"));
        assert!(!increase.is_match("<br>"));
        assert!(!increase.is_match("<meta charset=\"utf-8\">"));
        assert!(!increase.is_match("<MyComponent />"));
        assert!(decrease("html").is_match("  </div>"));
    }

    #[test]
    fn json_has_no_parentheses() {
        let json = rules("json").unwrap();
        assert!(!json.brackets.iter().any(|(open, _)| *open == "("));
        assert!(!json.pairs.iter().any(|(open, _)| *open == "("));
    }

    #[test]
    fn markup_closes_comments() {
        for language in ["html", "vue", "svelte"] {
            assert!(rules(language).unwrap().pairs.contains(&("<!--", "-->")));
        }
    }

    #[test]
    fn plain_text_has_no_rules() {
        assert!(rules("plain_text").is_none());
    }
}
