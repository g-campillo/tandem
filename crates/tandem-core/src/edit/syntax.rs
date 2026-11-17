//! Rough syntax checks for the typing aids: which embedded element of a markup file the
//! cursor is in, and whether the cursor is likely inside a string or a comment. They read
//! only the text before the cursor, so unusual code can fool them.
use super::line_start;

/// html, vue, svelte: whether `cursor` is inside a `<script …>…</script>` or
/// `<style …>…</style>` element (the last such open tag before the cursor has no close
/// tag between it and the cursor). Always None for other languages.
pub fn embedded_block(text: &str, cursor: usize, language: &str) -> Option<&'static str> {
    embedded_element(text, clamp_cursor(text, cursor), language).map(|(name, _)| name)
}

/// Heuristic: the cursor is inside a comment or an unclosed string. Code is read from the
/// start of the cursor's line. Markup (html, vue, svelte) uses the javascript rules inside
/// a `<script>` element and the css rules inside a `<style>` element; elsewhere the cursor
/// is in a comment after a `<!--` that is not closed yet, and in a string inside a quoted
/// attribute value of the open tag it is in, so apostrophes in text content do not count.
pub fn likely_in_string_or_comment(text: &str, cursor: usize, language: &str) -> bool {
    let cursor = clamp_cursor(text, cursor);
    let line = line_start(text, cursor);
    match embedded_element(text, cursor, language) {
        Some((name, content)) => {
            let language = if name == "script" {
                "javascript"
            } else {
                "css"
            };
            in_string_or_comment(&text[line.max(content)..cursor], &code_rules(language))
        }
        None if is_markup(language) => in_markup_string_or_comment(&text[..cursor]),
        None => in_string_or_comment(&text[line..cursor], &code_rules(language)),
    }
}

/// `cursor` within `text` and on a char boundary.
fn clamp_cursor(text: &str, cursor: usize) -> usize {
    text.floor_char_boundary(cursor)
}

/// Languages whose files are markup that can embed `<script>` and `<style>` elements.
fn is_markup(language: &str) -> bool {
    matches!(language, "html" | "vue" | "svelte")
}

/// The `<script>` or `<style>` element `cursor` is inside, and where its content starts
/// (after the `>` of its open tag). A cursor inside the open tag is not inside the element.
fn embedded_element(text: &str, cursor: usize, language: &str) -> Option<(&'static str, usize)> {
    if !is_markup(language) {
        return None;
    }
    let before = &text.as_bytes()[..cursor];
    let (name, open) = ["script", "style"]
        .into_iter()
        .filter_map(|name| last_open_tag(before, name).map(|at| (name, at)))
        .max_by_key(|&(_, at)| at)?;
    let content = open + before[open..].iter().position(|&byte| byte == b'>')? + 1;
    let self_closing = before[content - 2] == b'/';
    let close = format!("</{name}");
    let closed = find_ignoring_case(&before[content..], close.as_bytes()).is_some();
    (!self_closing && !closed).then_some((name, content))
}

/// Where the last `<name` in `before` that opens a `name` element starts: one followed by
/// whitespace, `>`, `/` or the end of `before`. ASCII case is ignored.
fn last_open_tag(before: &[u8], name: &str) -> Option<usize> {
    let needle = format!("<{name}");
    let mut end = before.len();
    while let Some(at) = rfind_ignoring_case(&before[..end], needle.as_bytes()) {
        match before.get(at + needle.len()) {
            Some(&byte) if !(byte.is_ascii_whitespace() || byte == b'>' || byte == b'/') => {
                end = at;
            }
            _ => return Some(at),
        }
    }
    None
}

fn find_ignoring_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

fn rfind_ignoring_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window.eq_ignore_ascii_case(needle))
}

/// How a code language writes comments and strings.
struct CodeRules {
    line_comment: Option<&'static str>,
    /// Whether `/* … */` is a comment.
    block_comment: bool,
    quotes: &'static [char],
}

fn code_rules(language: &str) -> CodeRules {
    let (line_comment, block_comment, quotes): (_, _, &'static [char]) = match language {
        "rust" | "java" | "swift" => (Some("//"), true, &['"']),
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" => {
            (Some("//"), true, &['"', '\'', '`'])
        }
        "scss" | "less" => (Some("//"), true, &['"', '\'']),
        "css" => (None, true, &['"', '\'']),
        "python" => (Some("#"), false, &['"', '\'']),
        _ => (None, false, &['"']),
    };
    CodeRules {
        line_comment,
        block_comment,
        quotes,
    }
}

/// Whether the end of `code` is inside a comment or an unclosed string, reading it left to
/// right. A backslash outside a comment escapes the character after it.
fn in_string_or_comment(code: &str, rules: &CodeRules) -> bool {
    let mut string = None;
    let mut block_comment = false;
    let mut chars = code.char_indices();
    while let Some((index, c)) = chars.next() {
        let rest = &code[index..];
        if block_comment {
            if rest.starts_with("*/") {
                block_comment = false;
                chars.next();
            }
        } else if c == '\\' {
            chars.next();
        } else if let Some(quote) = string {
            if c == quote {
                string = None;
            }
        } else if rules
            .line_comment
            .is_some_and(|marker| rest.starts_with(marker))
        {
            return true;
        } else if rules.block_comment && rest.starts_with("/*") {
            block_comment = true;
            chars.next();
        } else if rules.quotes.contains(&c) {
            string = Some(c);
        }
    }
    string.is_some() || block_comment
}

/// Markup outside script and style: whether the end of `before` is inside a `<!--` comment
/// that is not closed yet, or inside a quoted attribute value of the open tag it is in.
fn in_markup_string_or_comment(before: &str) -> bool {
    if let Some(open) = before.rfind("<!--")
        && !before[open + "<!--".len()..].contains("-->")
    {
        return true;
    }
    let Some(tag) = before.rfind('<') else {
        return false;
    };
    let tag = &before[tag + 1..];
    if !tag.starts_with(|c: char| c.is_ascii_alphabetic()) || tag.contains('>') {
        return false;
    }
    let mut quote = None;
    let mut chars = tag.chars();
    while let Some(c) = chars.next() {
        match quote {
            _ if c == '\\' => {
                chars.next();
            }
            Some(open) if c == open => quote = None,
            None if c == '"' || c == '\'' => quote = Some(c),
            _ => {}
        }
    }
    quote.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of `marked` without its `|` cursor marker, and the cursor.
    fn unmarked(marked: &str) -> (String, usize) {
        let cursor = marked.find('|').expect("a cursor marker");
        (marked.replacen('|', "", 1), cursor)
    }

    /// Whether the `|` of `marked` is likely inside a string or a comment.
    fn inside(marked: &str, language: &str) -> bool {
        let (text, cursor) = unmarked(marked);
        likely_in_string_or_comment(&text, cursor, language)
    }

    /// The embedded element the `|` of `marked` is in.
    fn block(marked: &str, language: &str) -> Option<&'static str> {
        let (text, cursor) = unmarked(marked);
        embedded_block(&text, cursor, language)
    }

    #[test]
    fn a_line_comment_counts_to_the_end_of_its_line() {
        assert!(inside("let x = 1; // fo|", "rust"));
        assert!(inside("// fo|", "typescript"));
        assert!(inside("x = 1  # fo|", "python"));
        assert!(!inside("// note\nlet x|", "rust"));
        assert!(!inside("let x|", "rust"));
    }

    #[test]
    fn an_unclosed_double_quoted_string_counts_in_every_language() {
        assert!(inside("let s = \"abc fo|", "rust"));
        assert!(inside("{\"abc fo|", "json"));
        assert!(!inside("let s = \"abc\"; fo|", "rust"));
    }

    #[test]
    fn python_quotes_pair_up_left_to_right() {
        assert!(inside("s = 'it''s|", "python"));
        assert!(!inside("s = 'it' + 's' |", "python"));
    }

    #[test]
    fn an_escaped_quote_does_not_close_the_string() {
        assert!(inside(r#"let s = "say \"hi|"#, "rust"));
        assert!(!inside(r#"let s = "say \"hi\""; |"#, "rust"));
    }

    #[test]
    fn backticks_are_strings_in_javascript_and_typescript() {
        assert!(inside("const s = `hello ${na|", "typescript"));
        assert!(inside("const s = `hello|", "javascriptreact"));
        assert!(!inside("let s = `a|", "rust"));
    }

    #[test]
    fn a_hash_is_not_a_comment_in_rust() {
        assert!(!inside("#[derive(Debu|", "rust"));
    }

    #[test]
    fn rust_apostrophes_are_lifetimes_not_strings() {
        assert!(!inside("fn f<'a>(x: &'a st|", "rust"));
    }

    #[test]
    fn a_block_comment_counts_until_it_closes() {
        assert!(inside("a { color: red; /* fo|", "css"));
        assert!(inside("let x = 1; /* fo|", "java"));
        assert!(!inside("a { /* x */ color: re|", "css"));
        assert!(!inside("x = 1 /* fo|", "python"));
    }

    #[test]
    fn double_slash_comments_count_in_scss_and_less_but_not_in_css() {
        assert!(inside("// fo|", "scss"));
        assert!(inside("// fo|", "less"));
        assert!(!inside("// fo|", "css"));
    }

    #[test]
    fn a_comment_marker_inside_a_string_is_text() {
        assert!(!inside("let url = \"http://x\"; ur|", "rust"));
        assert!(!inside("s = \"#tag\"; t|", "python"));
    }

    #[test]
    fn an_apostrophe_in_markup_text_is_not_a_string() {
        assert!(!inside("<p>Don't <b|", "html"));
        assert!(!inside("<p>Don't |", "html"));
    }

    #[test]
    fn an_open_attribute_value_is_a_string() {
        assert!(inside("<a title=\"x|", "html"));
        assert!(inside("<a\n  title='x|", "vue"));
        assert!(inside("<a title=\"it's|", "svelte"));
        assert!(!inside("<a title=\"x\" |", "html"));
        assert!(!inside("<a title=\"it's\" |", "html"));
        assert!(!inside("<a title=\"x\">te|", "html"));
    }

    #[test]
    fn an_unclosed_markup_comment_counts() {
        assert!(inside("<!-- a <b|", "html"));
        assert!(!inside("<!-- a --> <b|", "html"));
    }

    #[test]
    fn script_and_style_elements_use_their_own_rules() {
        assert!(inside("<script>\nconst s = 'a|", "html"));
        assert!(inside("<script>const s = 'a|", "html"));
        assert!(!inside("<script>\nconst s = 'a';|", "html"));
        assert!(inside("<script setup lang=\"ts\">\n// fo|", "vue"));
        assert!(inside("<style>\na { /* fo|", "svelte"));
        assert!(!inside("<style>\na { colo|", "svelte"));
        assert!(!inside("<script>let s = 1;</script>\n<p>Don't |", "html"));
    }

    #[test]
    fn embedded_block_finds_an_open_script_or_style_element() {
        assert_eq!(block("<script>\nlet x|", "html"), Some("script"));
        assert_eq!(
            block(
                "<template></template>\n<script setup lang=\"ts\">\nconst a|",
                "vue"
            ),
            Some("script")
        );
        assert_eq!(block("<style scoped>\n.a { colo|", "vue"), Some("style"));
        assert_eq!(block("<STYLE>\n.a { colo|", "html"), Some("style"));
    }

    #[test]
    fn embedded_block_ends_at_the_close_tag() {
        assert_eq!(block("<script>let x;</script>\n<p|", "html"), None);
        assert_eq!(
            block("<script>x</script><style>a{}</style>\n<p|", "html"),
            None
        );
        assert_eq!(
            block("<style>a{}</style>\n<script>let |", "svelte"),
            Some("script")
        );
    }

    #[test]
    fn embedded_block_is_none_inside_the_open_tag_and_for_other_elements() {
        assert_eq!(block("<script lang=\"ts|", "vue"), None);
        assert_eq!(block("<script|", "html"), None);
        assert_eq!(block("<scripts>\nx|", "html"), None);
        assert_eq!(block("<script src=\"a.js\" />\n<p|", "html"), None);
    }

    #[test]
    fn embedded_block_is_none_outside_markup_languages() {
        assert_eq!(block("<script>\nlet x|", "javascript"), None);
        assert_eq!(block("<style>\na|", "css"), None);
    }

    #[test]
    fn a_cursor_past_the_end_or_inside_a_character_is_clamped() {
        // "é" is bytes 1..3: offset 2 is inside it.
        assert!(likely_in_string_or_comment("\"é", 2, "rust"));
        assert!(likely_in_string_or_comment("\"é", 99, "rust"));
        // "é" is bytes 7..9: offset 8 is inside it.
        assert_eq!(embedded_block("<style>é", 8, "html"), Some("style"));
        assert_eq!(embedded_block("<style>é", 99, "html"), Some("style"));
    }
}
