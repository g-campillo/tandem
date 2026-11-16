//! Turns a language server's hover reply, and a diagnostic, into what the tooltip shows,
//! kept free of windows and entities so it is tested directly.

use gpui_kit::component::highlighter::DiagnosticSeverity;
use lsp_types::{HoverContents, MarkedString, MarkupKind};

/// One block of a hover reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    /// Code, shown in the editor's font. `language` is the name the highlighter is asked
    /// for; `None` is shown without highlighting.
    Code {
        language: Option<String>,
        text: String,
    },
    /// Markdown prose.
    Prose(String),
}

/// A hover reply split into the symbol's signature and its documentation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverContent {
    /// The code sections the reply starts with: the symbol's path and signature.
    pub signature: Vec<Section>,
    /// Everything after them.
    pub documentation: Vec<Section>,
}

impl HoverContent {
    pub fn is_empty(&self) -> bool {
        self.signature.is_empty() && self.documentation.is_empty()
    }

    /// The signature as Markdown for the tooltip's code area.
    pub fn signature_markdown(&self) -> String {
        to_markdown(&self.signature)
    }

    /// The documentation as Markdown for the tooltip's prose area.
    pub fn documentation_markdown(&self) -> String {
        to_markdown(&self.documentation)
    }
}

/// Splits a hover reply for a file written in `file_language` (`Document::language()`).
pub fn parse(contents: &HoverContents, file_language: &str) -> HoverContent {
    let marked = |item: &MarkedString| match item {
        MarkedString::String(markdown) => sections(markdown, file_language),
        MarkedString::LanguageString(code) => {
            code_section(fence_language(&code.language, file_language), &code.value)
        }
    };
    let all = match contents {
        HoverContents::Scalar(item) => marked(item),
        HoverContents::Array(items) => items.iter().flat_map(marked).collect(),
        HoverContents::Markup(markup) if markup.kind == MarkupKind::Markdown => {
            sections(&markup.value, file_language)
        }
        // Plain text is shown as it is, never read as Markdown.
        HoverContents::Markup(markup) => code_section(None, &markup.value),
    };
    let lead = all
        .iter()
        .take_while(|section| matches!(section, Section::Code { .. }))
        .count();
    let mut documentation = all[lead..].to_vec();
    // The signature gets a block of its own in the tooltip, so the rule the server put
    // under it is dropped.
    if let Some(Section::Prose(prose)) = documentation.first_mut() {
        let kept = prose
            .lines()
            .skip_while(|line| line.trim().is_empty() || is_rule(line))
            .collect::<Vec<_>>()
            .join("\n");
        if kept.is_empty() {
            documentation.remove(0);
        } else {
            *prose = kept;
        }
    }
    HoverContent {
        signature: all[..lead].to_vec(),
        documentation,
    }
}

/// Splits Markdown at its fenced code blocks, in order. Prose loses its surrounding blank
/// lines; empty prose and empty code blocks are dropped.
pub fn sections(markdown: &str, file_language: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut code: Option<(char, usize, Option<String>, Vec<&str>)> = None;
    for line in markdown.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some((mark, length, language, lines)) = code.as_mut() {
            let trimmed = line.trim();
            if trimmed.len() >= *length && trimmed.chars().all(|c| c == *mark) {
                out.extend(code_section(language.take(), &lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
        } else if let Some((mark, length, info)) = fence(line) {
            out.extend(prose_section(&prose));
            prose.clear();
            code = Some((
                mark,
                length,
                fence_language(info, file_language),
                Vec::new(),
            ));
        } else {
            prose.push(line);
        }
    }
    // A fence that is never closed runs to the end.
    if let Some((_, _, language, lines)) = code {
        out.extend(code_section(language, &lines.join("\n")));
    }
    out.extend(prose_section(&prose));
    out
}

/// The highlighter language for a fence's info string: its first word, lower-cased; the
/// file's language when the fence names none; `None` for plain text.
pub fn fence_language(info: &str, file_language: &str) -> Option<String> {
    let name = info
        .trim()
        .split([',', ' ', '\t', '{'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    let name = if name.is_empty() {
        file_language
    } else {
        &name
    };
    (!matches!(
        name,
        "" | "text" | "txt" | "plain" | "plaintext" | "plain_text"
    ))
    .then(|| name.to_owned())
}

/// Markdown for `sections`: prose as it is, code in a fence long enough to hold it.
pub fn to_markdown(sections: &[Section]) -> String {
    sections
        .iter()
        .map(|section| match section {
            Section::Prose(prose) => prose.clone(),
            Section::Code { language, text } => {
                let longest = text
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or_default();
                let fence = "`".repeat(longest.max(2) + 1);
                let language = language.as_deref().unwrap_or_default();
                format!("{fence}{language}\n{text}\n{fence}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// An opening code fence: its character, its length and its info string.
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let mark = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = rest.len() - rest.trim_start_matches(mark).len();
    let info = rest[length..].trim();
    (indent <= 3 && length >= 3 && !(mark == '`' && info.contains('`')))
        .then_some((mark, length, info))
}

/// A thematic break: three or more `-`, `*` or `_`, with nothing else on the line.
fn is_rule(line: &str) -> bool {
    let marks: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|c| *c == marks[0])
}

fn code_section(language: Option<String>, text: &str) -> Vec<Section> {
    let text = text.trim_matches(['\r', '\n']);
    if text.trim().is_empty() {
        return Vec::new();
    }
    vec![Section::Code {
        language,
        text: text.to_owned(),
    }]
}

fn prose_section(lines: &[&str]) -> Vec<Section> {
    let blank = |line: &&str| line.trim().is_empty();
    let Some(first) = lines.iter().position(|line| !blank(line)) else {
        return Vec::new();
    };
    let last = lines.iter().rposition(|line| !blank(line)).unwrap_or(first);
    vec![Section::Prose(lines[first..=last].join("\n"))]
}

/// How serious a diagnostic is. It picks the colours of the diagnostic's block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Error,
    Warning,
    /// Information and hints.
    Note,
}

/// What the tooltip shows for a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemContent {
    pub tone: Tone,
    /// The severity, then the source and the code where the server gave them:
    /// `Error · rust-analyzer · E0061`.
    pub title: String,
    /// The server's message. It is shown as Markdown, as the toolkit's popover showed it.
    pub message: String,
}

/// The tooltip content for a diagnostic.
pub fn problem(
    severity: DiagnosticSeverity,
    source: Option<&str>,
    code: Option<&str>,
    message: &str,
) -> ProblemContent {
    let (tone, label) = match severity {
        DiagnosticSeverity::Error => (Tone::Error, "Error"),
        DiagnosticSeverity::Warning => (Tone::Warning, "Warning"),
        DiagnosticSeverity::Info => (Tone::Note, "Info"),
        DiagnosticSeverity::Hint => (Tone::Note, "Hint"),
    };
    let title = [Some(label), source, code]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    ProblemContent {
        tone,
        title,
        message: message.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{LanguageString, MarkupContent};

    fn code(language: &str, text: &str) -> Section {
        Section::Code {
            language: (!language.is_empty()).then(|| language.to_owned()),
            text: text.to_owned(),
        }
    }

    fn prose(text: &str) -> Section {
        Section::Prose(text.to_owned())
    }

    fn markdown(value: &str) -> HoverContents {
        HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: value.to_owned(),
        })
    }

    #[test]
    fn a_rust_analyzer_reply_splits_into_signature_and_documentation() {
        let reply = "\n```rust\nprobe\n```\n\n```rust\nfn add(left: u32, right: u32) -> u32\n```\n\n---\n\nAdds two numbers.\n\n# Examples\n\n```rust\nlet x = add(1, 2);\n```";
        let content = parse(&markdown(reply), "rust");
        assert_eq!(
            content.signature,
            vec![
                code("rust", "probe"),
                code("rust", "fn add(left: u32, right: u32) -> u32")
            ]
        );
        assert_eq!(
            content.documentation,
            vec![
                prose("Adds two numbers.\n\n# Examples"),
                code("rust", "let x = add(1, 2);")
            ]
        );
        assert_eq!(
            content.signature_markdown(),
            "```rust\nprobe\n```\n\n```rust\nfn add(left: u32, right: u32) -> u32\n```"
        );
        assert_eq!(
            content.documentation_markdown(),
            "Adds two numbers.\n\n# Examples\n\n```rust\nlet x = add(1, 2);\n```"
        );
    }

    #[test]
    fn a_fence_without_a_language_takes_the_language_of_the_file() {
        let reply = "```python\n(function) def add(\n    left: int,\n    right: int\n) -> int\n```\n---\nAdd two numbers.\n\nExample:\n\n```\n>>> add(1, 2)\n3\n```";
        let content = parse(&markdown(reply), "python");
        assert_eq!(
            content.signature,
            vec![code(
                "python",
                "(function) def add(\n    left: int,\n    right: int\n) -> int"
            )]
        );
        assert_eq!(
            content.documentation,
            vec![
                prose("Add two numbers.\n\nExample:"),
                code("python", ">>> add(1, 2)\n3")
            ]
        );
        // A file the highlighter has no language for leaves the block plain.
        assert_eq!(sections("```\nx\n```", "plain_text"), vec![code("", "x")]);
        assert_eq!(sections("```\nx\n```", ""), vec![code("", "x")]);
    }

    #[test]
    fn the_fence_language_is_its_first_word() {
        assert_eq!(fence_language("rust", "python"), Some("rust".into()));
        assert_eq!(
            fence_language(" Rust,no_run ", "python"),
            Some("rust".into())
        );
        assert_eq!(fence_language("py title=\"x\"", "rust"), Some("py".into()));
        assert_eq!(fence_language("toml{1,2}", "rust"), Some("toml".into()));
        assert_eq!(fence_language("", "rust"), Some("rust".into()));
        assert_eq!(fence_language("   ", "python"), Some("python".into()));
        for plain in ["text", "TXT", "plain", "plaintext"] {
            assert_eq!(fence_language(plain, "rust"), None);
        }
        assert_eq!(fence_language("", "plain_text"), None);
    }

    #[test]
    fn several_fenced_blocks_keep_their_order() {
        let text = "Intro\n\n```rust\nlet a = 1;\n```\nBetween\n~~~toml\nkey = 1\n~~~\n\n\n```text\nplain\n```\nEnd\n";
        assert_eq!(
            sections(text, "rust"),
            vec![
                prose("Intro"),
                code("rust", "let a = 1;"),
                prose("Between"),
                code("toml", "key = 1"),
                code("", "plain"),
                prose("End"),
            ]
        );
    }

    #[test]
    fn fences_follow_the_markdown_rules_that_matter() {
        // A longer fence may hold a shorter one.
        assert_eq!(
            sections("````md\n```rust\nfn f() {}\n```\n````", "rust"),
            vec![code("md", "```rust\nfn f() {}\n```")]
        );
        // A tilde fence is not closed by backticks.
        assert_eq!(
            sections("~~~\na\n```\nb\n~~~", "rust"),
            vec![code("rust", "a\n```\nb")]
        );
        // An unclosed fence runs to the end.
        assert_eq!(
            sections("Doc\n```rust\nlet a = 1;\n", "rust"),
            vec![prose("Doc"), code("rust", "let a = 1;")]
        );
        // Inline code is not a fence, nor is a block indented four spaces.
        assert_eq!(
            sections("```not a fence```\n    ```\n", "rust"),
            vec![prose("```not a fence```\n    ```")]
        );
        // Blank code is dropped; indentation inside code is kept.
        assert_eq!(sections("```rust\n\n```", "rust"), vec![]);
        assert_eq!(
            sections("```rust\n    indented\n```", "rust"),
            vec![code("rust", "    indented")]
        );
        // Windows line endings.
        assert_eq!(
            sections("```rust\r\nlet a = 1;\r\n```\r\nDoc\r\n", "rust"),
            vec![code("rust", "let a = 1;"), prose("Doc")]
        );
    }

    #[test]
    fn marked_strings_are_code_and_markdown() {
        let scalar = HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
            language: "rust".into(),
            value: "fn f()\n".into(),
        }));
        assert_eq!(
            parse(&scalar, "rust").signature,
            vec![code("rust", "fn f()")]
        );
        let array = HoverContents::Array(vec![
            MarkedString::LanguageString(LanguageString {
                language: String::new(),
                value: "def f()".into(),
            }),
            MarkedString::String("Docs with `code`.\n\n```\nf()\n```".into()),
            MarkedString::String("  ".into()),
        ]);
        let content = parse(&array, "python");
        assert_eq!(content.signature, vec![code("python", "def f()")]);
        assert_eq!(
            content.documentation,
            vec![prose("Docs with `code`."), code("python", "f()")]
        );
        let text = HoverContents::Scalar(MarkedString::String("Only *prose*.".into()));
        let content = parse(&text, "rust");
        assert!(content.signature.is_empty());
        assert_eq!(content.documentation, vec![prose("Only *prose*.")]);
    }

    #[test]
    fn plain_text_is_shown_as_it_is() {
        let plain = HoverContents::Markup(MarkupContent {
            kind: MarkupKind::PlainText,
            value: "probe\n\nfn add(a: *const u8) -> u32\n\n# not a heading\n".into(),
        });
        let content = parse(&plain, "rust");
        assert_eq!(
            content.signature,
            vec![code(
                "",
                "probe\n\nfn add(a: *const u8) -> u32\n\n# not a heading"
            )]
        );
        assert!(content.documentation.is_empty());
        assert_eq!(
            content.signature_markdown(),
            "```\nprobe\n\nfn add(a: *const u8) -> u32\n\n# not a heading\n```"
        );
    }

    #[test]
    fn the_rule_under_the_signature_is_dropped_and_later_rules_are_kept() {
        let literal = "```rust\nu32\n```\n---\n\nvalue of literal: ` 3 (0x3|0b11) `";
        let content = parse(&markdown(literal), "rust");
        assert_eq!(content.signature, vec![code("rust", "u32")]);
        assert_eq!(
            content.documentation,
            vec![prose("value of literal: ` 3 (0x3|0b11) `")]
        );
        let only_rule = parse(&markdown("```rust\nu32\n```\n\n---\n"), "rust");
        assert!(only_rule.documentation.is_empty());
        let later = parse(
            &markdown("```rust\nu32\n```\n* * *\nFirst\n\n---\n\nSecond"),
            "rust",
        );
        assert_eq!(later.documentation, vec![prose("First\n\n---\n\nSecond")]);
        // Without a signature a leading rule is still only a separator.
        let prose_only = parse(&markdown("___\nText"), "rust");
        assert_eq!(prose_only.documentation, vec![prose("Text")]);
    }

    #[test]
    fn empty_replies_have_nothing_to_show() {
        for value in ["", "  \n\n", "```rust\n```", "---"] {
            assert!(parse(&markdown(value), "rust").is_empty(), "{value:?}");
        }
        assert!(parse(&HoverContents::Array(vec![]), "rust").is_empty());
        assert!(!parse(&markdown("x"), "rust").is_empty());
    }

    #[test]
    fn emitted_fences_are_longer_than_any_backticks_in_the_code() {
        let blocks = vec![
            code("rust", "let s = \"```\";"),
            code("", "a ```` b"),
            prose("Text"),
        ];
        let markdown = to_markdown(&blocks);
        assert_eq!(
            markdown,
            "````rust\nlet s = \"```\";\n````\n\n`````\na ```` b\n`````\n\nText"
        );
        assert_eq!(sections(&markdown, "plain_text"), blocks);
        assert_eq!(to_markdown(&[]), "");
    }

    #[test]
    fn a_diagnostic_is_titled_by_severity_source_and_code() {
        let error = problem(
            DiagnosticSeverity::Error,
            Some("rust-analyzer"),
            Some("E0061"),
            "expected 2 arguments, found 1",
        );
        assert_eq!(error.tone, Tone::Error);
        assert_eq!(error.title, "Error · rust-analyzer · E0061");
        assert_eq!(error.message, "expected 2 arguments, found 1");
        let warning = problem(DiagnosticSeverity::Warning, Some("Pyright"), None, "w");
        assert_eq!(
            (warning.tone, warning.title.as_str()),
            (Tone::Warning, "Warning · Pyright")
        );
        let code_only = problem(DiagnosticSeverity::Warning, None, Some("W291"), "w");
        assert_eq!(code_only.title, "Warning · W291");
        // Information and hints share the quiet colours; empty parts are left out.
        let info = problem(DiagnosticSeverity::Info, Some("  "), Some(""), "i");
        assert_eq!((info.tone, info.title.as_str()), (Tone::Note, "Info"));
        let hint = problem(DiagnosticSeverity::Hint, None, None, "h");
        assert_eq!((hint.tone, hint.title.as_str()), (Tone::Note, "Hint"));
    }

    #[test]
    fn a_diagnostic_message_keeps_its_lines() {
        let list = problem(
            DiagnosticSeverity::Error,
            None,
            None,
            "\nmissing structure fields:\n- y\n\n",
        );
        assert_eq!(list.message, "missing structure fields:\n- y");
        let empty = problem(DiagnosticSeverity::Error, None, None, " \n");
        assert_eq!(empty.message, "");
        assert_eq!(empty.title, "Error");
    }
}
