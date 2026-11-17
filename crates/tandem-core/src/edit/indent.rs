//! Indentation: the unit a document indents with, and the corrections made right after a
//! keystroke (Enter, a closing bracket, the colon of a Python block keyword).
use super::select::matching_bracket;
use super::{Edit, indent_len, is_word_char, line_end, line_start};
use std::ops::Range;

/// One indentation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentUnit {
    /// Columns per level; with `hard_tabs` also the tab width.
    pub width: usize,
    pub hard_tabs: bool,
}

impl IndentUnit {
    /// The text of one level: a tab, or `width` spaces.
    pub fn text(&self) -> String {
        if self.hard_tabs {
            "\t".to_string()
        } else {
            " ".repeat(self.width)
        }
    }
}

const fn spaces(width: usize) -> IndentUnit {
    IndentUnit {
        width,
        hard_tabs: false,
    }
}

/// The tab width `detect` measures with and gives a document indented with tabs.
const TAB_WIDTH: usize = 4;

/// How many lines from the start `detect` reads.
const DETECT_LINES: usize = 1000;

/// 4 spaces for rust, python, java, swift; 2 for javascript, javascriptreact, typescript,
/// typescriptreact, vue, svelte, html, css, scss, less, json, toml, markdown; 4 otherwise.
pub fn language_default(language: &str) -> IndentUnit {
    match language {
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" | "vue" | "svelte"
        | "html" | "css" | "scss" | "less" | "json" | "toml" | "markdown" => spaces(2),
        _ => spaces(4),
    }
}

/// From up to the first 1000 lines: tabs when more lines start with a tab than with
/// spaces; else the most frequent positive indentation step between consecutive
/// non-blank lines among 2, 4 and 8 (ties prefer 4, then 2). None without indented lines.
/// Blank lines are skipped, and tabs measure `TAB_WIDTH` columns.
pub fn detect(text: &str) -> Option<IndentUnit> {
    let lines: Vec<&str> = text
        .lines()
        .take(DETECT_LINES)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tab_lines = lines.iter().filter(|line| line.starts_with('\t')).count();
    let space_lines = lines.iter().filter(|line| line.starts_with(' ')).count();
    if tab_lines > space_lines {
        return Some(IndentUnit {
            width: TAB_WIDTH,
            hard_tabs: true,
        });
    }
    // Steps of 4, 2 and 8 columns, in the order ties prefer them.
    let mut steps = [(4, 0), (2, 0), (8, 0)];
    for pair in lines.windows(2) {
        let [above, below] = [pair[0], pair[1]].map(|line| columns(leading(line), TAB_WIDTH));
        if let Some(step) = below.checked_sub(above)
            && let Some((_, count)) = steps.iter_mut().find(|(width, _)| *width == step)
        {
            *count += 1;
        }
    }
    steps
        .into_iter()
        .filter(|&(_, count)| count > 0)
        .reduce(|kept, next| if next.1 > kept.1 { next } else { kept })
        .map(|(width, _)| spaces(width))
}

/// The unit `detect` finds in `text`, else the language's default.
pub fn for_document(language: &str, text: &str) -> IndentUnit {
    detect(text).unwrap_or_else(|| language_default(language))
}

/// A correction right after a single keystroke. `text` is after the edit; the edit
/// inserted `inserted` at `start`, and the cursor is at `start + inserted.len()`.
/// Returns the edit and the cursor after it as `Edit::selection` (empty range).
///
/// - Enter (`\n` or `\r\n` and spaces or tabs), with `prev` the line the newline ended:
///   - python: after a line starting with the word `return`, `pass`, `break`, `continue`
///     or `raise` that leaves no bracket open and does not end in a backslash, the new
///     line is indented one unit less than `prev`.
///   - rust and swift: `///` on `prev` continues on the new line; rust also `//!`.
///   - rust, java, swift, javascript, javascriptreact, typescript, typescriptreact, css,
///     vue, svelte: after a `/**` that is not closed on `prev`, the new line becomes
///     `prev`'s indentation and ` * `; after a `*` line inside a block comment that does
///     not close it, `prev`'s indentation and `* `.
/// - A `}`, `]` or `)` alone on its line gets the indentation of the line holding its
///   opener.
/// - python, `:` ending an `elif`, `else`, `except` or `finally` line indented as deep
///   as the line above it (blank lines skipped): the line loses one unit.
///
/// `None` when no rule matches or the correction would change nothing.
pub fn after_typed(
    text: &str,
    start: usize,
    inserted: &str,
    language: &str,
    unit: IndentUnit,
) -> Option<Edit> {
    let cursor = start.checked_add(inserted.len())?;
    if inserted.is_empty() || text.get(start..cursor) != Some(inserted) {
        return None;
    }
    if let Some(new_indent) = enter_indentation(inserted) {
        let new_indent = cursor - new_indent.len()..cursor;
        after_enter(text, start, new_indent, language, unit)
    } else if matches!(inserted, "}" | "]" | ")") {
        after_closer(text, start, cursor)
    } else if inserted == ":" && language == "python" {
        after_block_keyword_colon(text, start, cursor, unit)
    } else {
        None
    }
}

/// The indentation an Enter inserted after its line break, when `inserted` is an Enter.
fn enter_indentation(inserted: &str) -> Option<&str> {
    let rest = inserted.strip_prefix('\r').unwrap_or(inserted);
    let indent = rest.strip_prefix('\n')?;
    is_blank(indent).then_some(indent)
}

/// The Enter rules: `start` is where the line break went, `new_indent` the indentation
/// the Enter gave the new line, ending at the cursor.
fn after_enter(
    text: &str,
    start: usize,
    new_indent: Range<usize>,
    language: &str,
    unit: IndentUnit,
) -> Option<Edit> {
    let prev_start = line_start(text, start);
    let prev = &text[prev_start..start];
    let prev_indent = leading(prev);
    let content = prev.trim();
    let cursor = new_indent.end;

    if language == "python" {
        let leaves_block = starts_with_word(content, &PYTHON_BLOCK_EXITS)
            && !leaves_brackets_open(content)
            && !content.ends_with('\\');
        if !leaves_block {
            return None;
        }
        return correction(text, new_indent, dedent(prev_indent, unit), cursor);
    }
    if matches!(language, "rust" | "swift") && content.starts_with("///") {
        return correction(text, cursor..cursor, "/// ".to_string(), cursor);
    }
    if language == "rust" && content.starts_with("//!") {
        return correction(text, cursor..cursor, "//! ".to_string(), cursor);
    }
    if BLOCK_DOC_LANGUAGES.contains(&language) && !content.contains("*/") {
        if content.starts_with("/**") {
            return correction(text, new_indent, format!("{prev_indent} * "), cursor);
        }
        if content.starts_with('*') && inside_block_comment(&text[..prev_start]) {
            return correction(text, new_indent, format!("{prev_indent}* "), cursor);
        }
    }
    None
}

/// The Python statements after which the next line leaves the block.
const PYTHON_BLOCK_EXITS: [&str; 5] = ["return", "pass", "break", "continue", "raise"];

/// The Python keywords that continue the block statement above them.
const PYTHON_BLOCK_CONTINUATIONS: [&str; 4] = ["elif", "else", "except", "finally"];

/// The languages whose `/** … */` comments continue with ` * ` lines.
const BLOCK_DOC_LANGUAGES: [&str; 10] = [
    "rust",
    "java",
    "swift",
    "javascript",
    "javascriptreact",
    "typescript",
    "typescriptreact",
    "css",
    "vue",
    "svelte",
];

/// A closing bracket at `start`: when it is alone on its line, the indentation of the
/// line that holds its opener.
fn after_closer(text: &str, start: usize, cursor: usize) -> Option<Edit> {
    let line = line_start(text, start);
    if !is_blank(&text[line..start]) || !is_blank(&text[start + 1..line_end(text, start)]) {
        return None;
    }
    let opener = matching_bracket(text, start)?;
    let opener_line = line_start(text, opener);
    let indent = leading(&text[opener_line..line_end(text, opener)]);
    correction(text, line..start, indent.to_string(), cursor)
}

/// A `:` typed at `start` in Python: an `elif`, `else`, `except` or `finally` line that
/// ends with it and is as deep as the line above it moves out one unit.
fn after_block_keyword_colon(
    text: &str,
    start: usize,
    cursor: usize,
    unit: IndentUnit,
) -> Option<Edit> {
    let line = line_start(text, start);
    let current = &text[line..line_end(text, start)];
    let content = current.trim();
    if !content.ends_with(':') || !starts_with_word(content, &PYTHON_BLOCK_CONTINUATIONS) {
        return None;
    }
    let indent = leading(current);
    let above = leading(previous_content_line(text, line)?);
    if columns(above, unit.width) != columns(indent, unit.width) {
        return None;
    }
    correction(
        text,
        line..line + indent.len(),
        dedent(indent, unit),
        cursor,
    )
}

/// Replaces `range` of `text` with `new`, moving the keystroke's `cursor` (at or after
/// the end of `range`) along. `None` when `range` already holds `new`.
fn correction(text: &str, range: Range<usize>, new: String, cursor: usize) -> Option<Edit> {
    if text[range.clone()] == new {
        return None;
    }
    let mut edit = Edit {
        range,
        text: new,
        selection: 0..0,
    };
    let cursor = edit.map_offset(cursor);
    edit.selection = cursor..cursor;
    Some(edit)
}

/// Whether `text` is only spaces and tabs.
fn is_blank(text: &str) -> bool {
    text.chars().all(|c| c == ' ' || c == '\t')
}

/// The leading spaces and tabs of `line`.
fn leading(line: &str) -> &str {
    &line[..indent_len(line)]
}

/// Whether `content` starts with one of `words` as a whole word.
fn starts_with_word(content: &str, words: &[&str]) -> bool {
    words.iter().any(|word| {
        content
            .strip_prefix(word)
            .is_some_and(|rest| !rest.starts_with(is_word_char))
    })
}

/// Whether `line` opens more brackets than it closes.
fn leaves_brackets_open(line: &str) -> bool {
    let depth = line.chars().fold(0i64, |depth, c| match c {
        '(' | '[' | '{' => depth + 1,
        ')' | ']' | '}' => depth - 1,
        _ => depth,
    });
    depth > 0
}

/// Whether the end of `before` is inside a `/* … */` comment.
fn inside_block_comment(before: &str) -> bool {
    before
        .rfind("/*")
        .is_some_and(|open| !before[open + "/*".len()..].contains("*/"))
}

/// The nearest line above the line starting at `line` that is not blank, without its
/// line break.
fn previous_content_line(text: &str, mut line: usize) -> Option<&str> {
    while line > 0 {
        line = line_start(text, line - 1);
        let content = &text[line..line_end(text, line)];
        if !content.trim().is_empty() {
            return Some(content);
        }
    }
    None
}

/// The column `indent` reaches, with tabs stopping at multiples of `tab_width`.
fn columns(indent: &str, tab_width: usize) -> usize {
    indent
        .chars()
        .fold(0, |column, c| advance(column, c, tab_width))
}

/// The column after `c` when it starts at `column`.
fn advance(column: usize, c: char, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    if c == '\t' {
        (column / tab_width + 1) * tab_width
    } else {
        column + 1
    }
}

/// `indent` one unit shallower, not below zero: the longest start of `indent` that stays
/// within the new depth, padded to it with spaces when a tab would pass it.
fn dedent(indent: &str, unit: IndentUnit) -> String {
    let target = columns(indent, unit.width).saturating_sub(unit.width.max(1));
    let mut kept = String::new();
    let mut column = 0;
    for c in indent.chars() {
        let next = advance(column, c, unit.width);
        if next > target {
            break;
        }
        kept.push(c);
        column = next;
    }
    kept.extend(std::iter::repeat_n(' ', target - column));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOUR: IndentUnit = IndentUnit {
        width: 4,
        hard_tabs: false,
    };
    const TWO: IndentUnit = IndentUnit {
        width: 2,
        hard_tabs: false,
    };
    const TAB: IndentUnit = IndentUnit {
        width: 4,
        hard_tabs: true,
    };

    /// Types `inserted` at the `|` of `marked` and returns the corrected text with `|` at
    /// the cursor after the correction; `None` when nothing is corrected.
    fn typed(marked: &str, inserted: &str, language: &str) -> Option<String> {
        typed_with(marked, inserted, language, FOUR)
    }

    fn typed_with(
        marked: &str,
        inserted: &str,
        language: &str,
        unit: IndentUnit,
    ) -> Option<String> {
        let start = marked.find('|').expect("a cursor marker");
        let text = format!("{}{inserted}{}", &marked[..start], &marked[start + 1..]);
        let edit = after_typed(&text, start, inserted, language, unit)?;
        assert!(
            edit.selection.is_empty(),
            "the cursor is a collapsed selection"
        );
        assert!(edit.changes_text(), "a correction changes the text");
        let mut corrected = edit.apply(&text);
        corrected.insert(edit.selection.start, '|');
        Some(corrected)
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn unit_text_is_a_tab_or_width_spaces() {
        assert_eq!(FOUR.text(), "    ");
        assert_eq!(TWO.text(), "  ");
        assert_eq!(TAB.text(), "\t");
    }

    #[test]
    fn language_defaults() {
        for language in ["rust", "python", "java", "swift", "go", "plaintext", ""] {
            assert_eq!(language_default(language), FOUR, "{language}");
        }
        for language in [
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
            "toml",
            "markdown",
        ] {
            assert_eq!(language_default(language), TWO, "{language}");
        }
    }

    #[test]
    fn detect_finds_two_space_indentation() {
        let text = "function f() {\n  if (x) {\n    y();\n  }\n}\n";
        assert_eq!(detect(text), Some(TWO));
    }

    #[test]
    fn detect_finds_four_space_indentation_across_blank_lines() {
        let text = "fn f() {\n    let a = 1;\n\n    if a {\n        b();\n    }\n}\n";
        assert_eq!(detect(text), Some(FOUR));
    }

    #[test]
    fn detect_finds_tabs() {
        let text = "fn f() {\n\tlet a = 1;\n\tif a {\n\t\tb();\n\t}\n}\n";
        assert_eq!(detect(text), Some(TAB));
    }

    #[test]
    fn detect_prefers_spaces_when_as_many_lines_start_with_them() {
        let text = "a:\n\tb\nc:\n    d\n";
        assert_eq!(detect(text), Some(FOUR));
    }

    #[test]
    fn detect_without_indented_lines_is_none() {
        assert_eq!(detect(""), None);
        assert_eq!(detect("a\nb\n\nc\n"), None);
        // Blank lines made of spaces are not indentation.
        assert_eq!(detect("a\n    \nb\n"), None);
    }

    #[test]
    fn detect_breaks_ties_towards_four_then_two() {
        assert_eq!(detect("a\n  b\nc\n    d\n"), Some(FOUR));
        assert_eq!(detect("a\n  b\nc\n        d\n"), Some(TWO));
        assert_eq!(detect("a\n  b\nc\n  d\ne\n    f\n"), Some(TWO));
    }

    #[test]
    fn detect_reads_only_the_first_thousand_lines() {
        let text = format!("{}  late\n", "x\n".repeat(1000));
        assert_eq!(detect(&text), None);
        let text = format!("{}  early\n", "x\n".repeat(999));
        assert_eq!(detect(&text), Some(TWO));
    }

    #[test]
    fn for_document_detects_before_falling_back_to_the_default() {
        assert_eq!(for_document("typescript", "a\n    b\n"), FOUR);
        assert_eq!(for_document("typescript", "a\nb\n"), TWO);
        assert_eq!(for_document("rust", ""), FOUR);
    }

    #[test]
    fn enter_after_a_python_return_dedents_the_new_line() {
        assert_eq!(
            typed("def f():\n    return 1|", "\n    ", "python"),
            some("def f():\n    return 1\n|")
        );
        assert_eq!(
            typed(
                "for x in y:\n    if x:\n        continue|",
                "\n        ",
                "python"
            ),
            some("for x in y:\n    if x:\n        continue\n    |")
        );
        for keyword in ["pass", "break", "raise ValueError()", "return"] {
            assert_eq!(
                typed(&format!("if a:\n    {keyword}|"), "\n    ", "python"),
                some(&format!("if a:\n    {keyword}\n|")),
                "{keyword}"
            );
        }
    }

    #[test]
    fn enter_after_a_python_return_dedents_by_one_tab() {
        assert_eq!(
            typed_with("def f():\n\tif x:\n\t\treturn|", "\n\t\t", "python", TAB),
            some("def f():\n\tif x:\n\t\treturn\n\t|")
        );
    }

    #[test]
    fn enter_after_a_python_return_pads_with_spaces_where_a_tab_would_go_too_far() {
        // A tab and two spaces reach column 6; one unit less is column 2.
        assert_eq!(
            typed_with("if a:\n\t  return|", "\n\t  ", "python", TAB),
            some("if a:\n\t  return\n  |")
        );
    }

    #[test]
    fn enter_after_a_python_return_keeps_crlf_line_breaks() {
        assert_eq!(
            typed("if a:\r\n    pass|", "\r\n    ", "python"),
            some("if a:\r\n    pass\r\n|")
        );
    }

    #[test]
    fn enter_after_a_word_that_only_starts_with_return_does_not_dedent() {
        assert_eq!(typed("    returned = 1|", "\n    ", "python"), None);
        assert_eq!(typed("    pass_count += 1|", "\n    ", "python"), None);
        assert_eq!(typed("    x = 1|", "\n    ", "python"), None);
    }

    #[test]
    fn enter_inside_open_brackets_of_a_python_return_does_not_dedent() {
        assert_eq!(
            typed("    raise ValueError(|", "\n        ", "python"),
            None
        );
        assert_eq!(typed("    return [a,|", "\n        ", "python"), None);
        assert_eq!(typed("    return a + \\|", "\n        ", "python"), None);
    }

    #[test]
    fn enter_after_return_in_rust_changes_nothing() {
        assert_eq!(typed("fn f() {\n    return 1;|", "\n    ", "rust"), None);
    }

    #[test]
    fn enter_after_a_doc_comment_continues_it() {
        assert_eq!(
            typed("    /// Docs|", "\n    ", "rust"),
            some("    /// Docs\n    /// |")
        );
        assert_eq!(typed("/// Docs|", "\n", "swift"), some("/// Docs\n/// |"));
        assert_eq!(
            typed("//! Crate docs|", "\n", "rust"),
            some("//! Crate docs\n//! |")
        );
    }

    #[test]
    fn enter_after_a_doc_comment_in_the_wrong_language_changes_nothing() {
        assert_eq!(typed("    /// Docs|", "\n    ", "python"), None);
        assert_eq!(typed("//! Docs|", "\n", "swift"), None);
        assert_eq!(typed("// Note|", "\n", "rust"), None);
    }

    #[test]
    fn enter_after_a_block_comment_opener_starts_a_star_line() {
        assert_eq!(
            typed("    /**|", "\n    ", "typescript"),
            some("    /**\n     * |")
        );
        assert_eq!(
            typed("/** Summary|", "\n", "java"),
            some("/** Summary\n * |")
        );
        assert_eq!(typed("/**|", "\r\n", "css"), some("/**\r\n * |"));
    }

    #[test]
    fn enter_on_a_star_line_inside_a_block_comment_continues_it() {
        assert_eq!(
            typed("    /**\n     * Line|", "\n    ", "rust"),
            some("    /**\n     * Line\n     * |")
        );
        assert_eq!(typed("/*\n *|", "\n", "javascript"), some("/*\n *\n * |"));
    }

    #[test]
    fn enter_after_a_one_line_block_comment_changes_nothing() {
        assert_eq!(typed("    /** Docs */|", "\n    ", "typescript"), None);
        assert_eq!(typed("/**\n * Last line */|", "\n", "java"), None);
        assert_eq!(typed("/**\n */|", "\n", "java"), None);
    }

    #[test]
    fn enter_after_a_star_outside_a_comment_changes_nothing() {
        assert_eq!(typed("* {|", "\n  ", "css"), None);
        assert_eq!(typed("fn f() {\n    *count += 1;|", "\n    ", "rust"), None);
        assert_eq!(typed("/* a */\n* {|", "\n  ", "css"), None);
    }

    #[test]
    fn enter_after_a_block_comment_opener_in_html_changes_nothing() {
        assert_eq!(typed("/**|", "\n", "html"), None);
    }

    #[test]
    fn a_closing_bracket_alone_on_its_line_takes_the_opener_line_indentation() {
        assert_eq!(
            typed("fn f() {\n    x();\n    |", "}", "rust"),
            some("fn f() {\n    x();\n}|")
        );
        assert_eq!(
            typed("    call(\n        a,\n        |", ")", "python"),
            some("    call(\n        a,\n    )|")
        );
        assert_eq!(
            typed("let a = [\n  1,\n  |  ", "]", "typescript"),
            some("let a = [\n  1,\n]|  ")
        );
        assert_eq!(
            typed_with("\tif x {\n\t\ty\n\t\t|", "}", "rust", TAB),
            some("\tif x {\n\t\ty\n\t}|")
        );
    }

    #[test]
    fn a_closing_bracket_without_an_opener_changes_nothing() {
        assert_eq!(typed("    |", "}", "rust"), None);
        assert_eq!(typed("(\n    |", "]", "rust"), None);
    }

    #[test]
    fn a_closing_bracket_in_the_middle_of_a_line_changes_nothing() {
        assert_eq!(typed("f(\n    x|", ")", "rust"), None);
        assert_eq!(typed("f(\n    |x", ")", "rust"), None);
    }

    #[test]
    fn a_closing_bracket_already_indented_right_changes_nothing() {
        assert_eq!(typed("fn f() {\n    x();\n|", "}", "rust"), None);
    }

    #[test]
    fn a_python_block_keyword_colon_dedents_its_line() {
        assert_eq!(
            typed("if x:\n    y = 1\n    else|", ":", "python"),
            some("if x:\n    y = 1\nelse:|")
        );
        assert_eq!(
            typed(
                "try:\n    a()\n\n    except ValueError as e|",
                ":",
                "python"
            ),
            some("try:\n    a()\n\nexcept ValueError as e:|")
        );
        assert_eq!(
            typed(
                "def f():\n    if a:\n        b\n        elif c|",
                ":",
                "python"
            ),
            some("def f():\n    if a:\n        b\n    elif c:|")
        );
        assert_eq!(
            typed("try:\n    a()\n    finally|", ":", "python"),
            some("try:\n    a()\nfinally:|")
        );
    }

    #[test]
    fn a_python_block_keyword_colon_on_a_line_already_dedented_changes_nothing() {
        assert_eq!(typed("if x:\n    y = 1\nelse|", ":", "python"), None);
        // After a `return`, Enter already dedented the line `else` is typed on.
        assert_eq!(
            typed(
                "def f():\n    if a:\n        return 1\n    else|",
                ":",
                "python"
            ),
            None
        );
    }

    #[test]
    fn a_colon_that_does_not_end_a_block_keyword_line_changes_nothing() {
        assert_eq!(
            typed("if x:\n    y = 1\n    elsewhere|", ":", "python"),
            None
        );
        assert_eq!(
            typed("if x:\n    y = 1\n    elif d[a|]", ":", "python"),
            None
        );
        assert_eq!(typed("if x:\n    y = 1\n    z = {1|", ":", "python"), None);
        assert_eq!(typed("else|", ":", "python"), None);
        assert_eq!(typed("if x {\n    y\n    else|", ":", "rust"), None);
    }

    #[test]
    fn other_inserts_change_nothing() {
        assert_eq!(typed("fn f() {\n    |", "})", "rust"), None);
        assert_eq!(typed("def f():\n    return 1|", "\n\n    ", "python"), None);
        assert_eq!(typed("def f():\n    return 1|", "\nx", "python"), None);
        assert_eq!(typed("fn f() {\n    |", "x", "rust"), None);
        assert_eq!(typed("|", "", "rust"), None);
    }

    #[test]
    fn an_insert_that_is_not_in_the_text_changes_nothing() {
        assert_eq!(after_typed("abc", 1, "}", "rust", FOUR), None);
        assert_eq!(after_typed("abc", 3, "}", "rust", FOUR), None);
        assert_eq!(after_typed("é}", 1, "}", "rust", FOUR), None);
    }
}
