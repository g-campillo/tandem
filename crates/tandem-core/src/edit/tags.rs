//! Tag auto-closing for markup and JSX: the close tag inserted after an open tag's `>`, and
//! the name completed after `</`. Like the other typing aids these read the text around the
//! cursor with simple rules, so unusual code can fool them; they then insert nothing.
use super::{Edit, is_word_char, syntax};
use std::ops::Range;

/// html, javascriptreact, typescriptreact, vue, svelte.
pub fn has_tags(language: &str) -> bool {
    matches!(
        language,
        "html" | "javascriptreact" | "typescriptreact" | "vue" | "svelte"
    )
}

/// Elements that never have a close tag in HTML.
pub const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// How far back before the `<` of a typed `</` `complete_close_tag` reads.
const SCAN_LIMIT: usize = 64 * 1024;

/// How many `>` the attribute expressions of one open tag may hold in that scan, so the
/// scan stays linear when braces never balance.
const MAX_GREATER_THAN_SIGNS_IN_EXPRESSIONS: usize = 32;

/// After `>` was typed at `start` (`text` is after the edit, the cursor at `start + 1`):
/// the matching close tag inserted after the cursor, the cursor staying where it is.
///
/// Only for an open tag (`<name …>`, or the fragment `<>` in JSX) typed in a language with
/// tags, outside `<script>` and `<style>` elements, strings and comments. Nothing for a
/// self-closing tag, a `>` inside an attribute expression or a JSX arrow, an HTML void
/// element, a `<` that cannot start JSX (`useState<string>`, `a <b>`), or when the close
/// tag already follows the cursor. Tag names are matched ignoring case in html only.
pub fn close_tag(text: &str, start: usize, language: &str) -> Option<Edit> {
    if text.as_bytes().get(start) != Some(&b'>') || !typing_tags(text, start, language) {
        return None;
    }
    let open = text[..start]
        .rfind(['<', '>'])
        .filter(|&at| text.as_bytes()[at] == b'<')?;
    let name = opened_element(text, open, start, language)?;
    if !braces_balanced(&text[open..start]) {
        return None;
    }
    let cursor = start + 1;
    let close = format!("</{name}>");
    let present = text
        .get(cursor..cursor + close.len())
        .is_some_and(|after| same_name(after, &close, language));
    (!present).then_some(Edit {
        range: cursor..cursor,
        text: close,
        selection: cursor..cursor,
    })
}

/// After `/` was typed at `start` right after a `<`: `</` completed with the name of the
/// nearest unclosed tag and `>`, the cursor after it.
///
/// The open elements come from one forward scan over at most the last 64 KiB before the
/// `<`: `<!-- … -->` is skipped, a tag `close_tag` would close opens an element (its end is
/// the first `>` outside its attribute expressions, so `<button onClick={() => go()}>` opens
/// a `button`), and a close tag closes the most recent open element of its name and every
/// element opened after it.
/// When a `>` already follows the cursor only the name is inserted and the cursor moves
/// past that `>`. `None` when nothing is open, or when a fragment's `</` already has its `>`.
pub fn complete_close_tag(text: &str, start: usize, language: &str) -> Option<Edit> {
    if text.as_bytes().get(start) != Some(&b'/') || !text[..start].ends_with('<') {
        return None;
    }
    // The checks read the text before the `<`, so a `</` typed inside an attribute value
    // counts as inside its string.
    let open = start - 1;
    if !typing_tags(text, open, language) {
        return None;
    }
    let name = innermost_open_element(text, open, language)?;
    let cursor = start + 1;
    let insert = if text[cursor..].starts_with('>') {
        name.to_string()
    } else {
        format!("{name}>")
    };
    if insert.is_empty() {
        return None;
    }
    let after = cursor + name.len() + 1;
    Some(Edit {
        range: cursor..cursor,
        text: insert,
        selection: after..after,
    })
}

/// Whether a tag typed at `at` is markup: the language has tags, and `at` is outside
/// `<script>` and `<style>` elements, strings and comments.
fn typing_tags(text: &str, at: usize, language: &str) -> bool {
    has_tags(language)
        && syntax::embedded_block(text, at, language).is_none()
        && !syntax::likely_in_string_or_comment(text, at, language)
}

fn is_jsx(language: &str) -> bool {
    matches!(language, "javascriptreact" | "typescriptreact")
}

/// The element the tag from the `<` at `open` to the `>` at `close` opens: its name, or
/// `""` for a JSX fragment. Every rule of `close_tag` but the brace count, which the
/// callers check their own way.
fn opened_element<'a>(text: &'a str, open: usize, close: usize, language: &str) -> Option<&'a str> {
    let jsx = is_jsx(language);
    let content = &text[open + 1..close];
    let name = if content.is_empty() {
        jsx.then_some("")?
    } else {
        let name = tag_name(content)?;
        let rest = &content[name.len()..];
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            return None;
        }
        name
    };
    let last = text[..close].chars().next_back();
    if last == Some('/') || (jsx && last == Some('=')) {
        return None;
    }
    let void = match language {
        "html" => VOID_ELEMENTS
            .iter()
            .any(|void| void.eq_ignore_ascii_case(name)),
        "vue" | "svelte" => VOID_ELEMENTS.contains(&name),
        _ => false,
    };
    if void || (jsx && !starts_jsx(text, open)) {
        return None;
    }
    Some(name)
}

/// The tag name `content` starts with: `[A-Za-z][A-Za-z0-9:._-]*`.
fn tag_name(content: &str) -> Option<&str> {
    if !content.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let end = content
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-')))
        .unwrap_or(content.len());
    Some(&content[..end])
}

/// Whether the `<` at `open` can start JSX: nothing but whitespace before it, or the
/// nearest character before it is an opening bracket, an operator or a separator, or the
/// word before it is `return`, `yield`, `await` or `default`.
fn starts_jsx(text: &str, open: usize) -> bool {
    let before = text[..open].trim_end();
    let Some(last) = before.chars().next_back() else {
        return true;
    };
    if "([{},;:?=>&|!".contains(last) {
        return true;
    }
    let word_len: usize = before
        .chars()
        .rev()
        .take_while(|&c| is_word_char(c))
        .map(char::len_utf8)
        .sum();
    matches!(
        &before[before.len() - word_len..],
        "return" | "yield" | "await" | "default"
    )
}

/// Whether `tag` holds as many `}` as `{`, so a `>` after it is not inside an attribute
/// expression.
fn braces_balanced(tag: &str) -> bool {
    tag.bytes().map(brace_step).sum::<i64>() == 0
}

/// What `byte` adds to the count of `{` minus `}`.
fn brace_step(byte: u8) -> i64 {
    match byte {
        b'{' => 1,
        b'}' => -1,
        _ => 0,
    }
}

/// Whether two tag texts name the same element: ignoring ASCII case in html, exactly
/// elsewhere.
fn same_name(a: &str, b: &str, language: &str) -> bool {
    if language == "html" {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// The name of the innermost element still open at `end` (`""` for a JSX fragment),
/// reading forward from at most `SCAN_LIMIT` bytes before it.
fn innermost_open_element<'a>(text: &'a str, end: usize, language: &str) -> Option<&'a str> {
    let begin = text.ceil_char_boundary(end.saturating_sub(SCAN_LIMIT));
    let braces = BraceCounts::new(text, begin..end);
    let closes: Vec<usize> = text[begin..end]
        .match_indices('>')
        .map(|(i, _)| begin + i)
        .collect();
    let mut open: Vec<&str> = Vec::new();
    let mut at = begin;
    // The index in `closes` of the first `>` after the current `<`.
    let mut next_close = 0;
    while let Some(tag) = text[at..end].find('<').map(|i| at + i) {
        if text[tag..end].starts_with("<!--") {
            let Some(comment_end) = text[tag..end].find("-->") else {
                break;
            };
            at = tag + comment_end + "-->".len();
            continue;
        }
        next_close += closes[next_close..].partition_point(|&close| close < tag);
        let Some(&close) = closes.get(next_close) else {
            break;
        };
        if let Some(content) = text[tag + 1..close].strip_prefix('/') {
            let content = content.trim_end();
            if (content.is_empty() || tag_name(content) == Some(content))
                && let Some(index) = open.iter().rposition(|&o| same_name(o, content, language))
            {
                open.truncate(index);
            }
            at = close + 1;
        } else if let Some(close) = open_tag_end(&braces, tag, &closes[next_close..])
            && let Some(name) = opened_element(text, tag, close, language)
        {
            open.push(name);
            at = close + 1;
        } else {
            at = tag + 1;
        }
    }
    open.last().copied()
}

/// The `>` that ends the tag whose `<` is at `tag`, from `closes`, the `>` after it in
/// order: the first one outside the tag's attribute expressions, so the `>` of
/// `onClick={() => go()}` or `show={a > b}` does not end it. `None` when the braces stay
/// open past `MAX_GREATER_THAN_SIGNS_IN_EXPRESSIONS` of them, or when a `}` closes more than
/// the tag opened, which puts the `<` itself inside an expression.
fn open_tag_end(braces: &BraceCounts, tag: usize, closes: &[usize]) -> Option<usize> {
    for &close in closes
        .iter()
        .take(MAX_GREATER_THAN_SIGNS_IN_EXPRESSIONS + 1)
    {
        match braces.depth(tag..close) {
            0 => return Some(close),
            depth if depth < 0 => return None,
            _ => {}
        }
    }
    None
}

/// Running brace counts over a stretch of text, so the brace count of any tag in it is
/// two lookups however many tags share the same `>`.
struct BraceCounts {
    begin: usize,
    /// `{` minus `}` in the text from `begin` up to each offset.
    prefix: Vec<i64>,
}

impl BraceCounts {
    fn new(text: &str, range: Range<usize>) -> Self {
        let mut prefix = Vec::with_capacity(range.len() + 1);
        prefix.push(0);
        let mut balance = 0;
        for byte in text[range.clone()].bytes() {
            balance += brace_step(byte);
            prefix.push(balance);
        }
        BraceCounts {
            begin: range.start,
            prefix,
        }
    }

    /// `{` minus `}` in `range` (within the counted stretch).
    fn depth(&self, range: Range<usize>) -> i64 {
        self.prefix[range.end - self.begin] - self.prefix[range.start - self.begin]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `typed` at the `|` of `marked`, runs `fix` on the result and returns the text
    /// after its edit with `|` at the cursor; `None` when `fix` changes nothing.
    fn after(
        marked: &str,
        typed: &str,
        language: &str,
        fix: fn(&str, usize, &str) -> Option<Edit>,
    ) -> Option<String> {
        let start = marked.find('|').expect("a cursor marker");
        let text = format!("{}{typed}{}", &marked[..start], &marked[start + 1..]);
        let edit = fix(&text, start, language)?;
        assert!(edit.changes_text(), "a fix changes the text");
        assert!(
            edit.selection.is_empty(),
            "the cursor is a collapsed selection"
        );
        let mut fixed = edit.apply(&text);
        fixed.insert(edit.selection.start, '|');
        Some(fixed)
    }

    /// Types `>` at the `|` of `marked` and closes the tag.
    fn closed(marked: &str, language: &str) -> Option<String> {
        after(marked, ">", language, close_tag)
    }

    /// Types `/` at the `|` of `marked` and completes the close tag.
    fn completed(marked: &str, language: &str) -> Option<String> {
        after(marked, "/", language, complete_close_tag)
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn markup_and_jsx_languages_have_tags() {
        for language in [
            "html",
            "javascriptreact",
            "typescriptreact",
            "vue",
            "svelte",
        ] {
            assert!(has_tags(language), "{language}");
        }
        for language in ["javascript", "typescript", "rust", "css", "plain_text"] {
            assert!(!has_tags(language), "{language}");
        }
    }

    #[test]
    fn an_html_open_tag_gets_its_close_tag_after_the_cursor() {
        assert_eq!(closed("<div|", "html"), some("<div>|</div>"));
        assert_eq!(
            closed("<div class=\"a\"|", "html"),
            some("<div class=\"a\">|</div>")
        );
        assert_eq!(closed("<ul>\n  <li|", "html"), some("<ul>\n  <li>|</li>"));
        assert_eq!(closed("<DIV|", "html"), some("<DIV>|</DIV>"));
    }

    #[test]
    fn an_attribute_list_over_several_lines_is_one_open_tag() {
        assert_eq!(
            closed("<div\n  class=\"a\"\n|", "html"),
            some("<div\n  class=\"a\"\n>|</div>")
        );
    }

    #[test]
    fn html_void_elements_get_no_close_tag_whatever_their_case() {
        assert_eq!(closed("<br|", "html"), None);
        assert_eq!(closed("<img src=\"x\"|", "html"), None);
        assert_eq!(closed("<INPUT|", "html"), None);
    }

    #[test]
    fn a_self_closing_tag_gets_no_close_tag() {
        assert_eq!(closed("<div/|", "html"), None);
        assert_eq!(closed("<div /|", "html"), None);
        assert_eq!(closed("<Foo /|", "typescriptreact"), None);
    }

    #[test]
    fn close_tags_declarations_and_processing_instructions_get_nothing() {
        assert_eq!(closed("<div></div|", "html"), None);
        assert_eq!(closed("<!DOCTYPE html|", "html"), None);
        assert_eq!(closed("<?xml version=\"1.0\"?|", "html"), None);
    }

    #[test]
    fn a_greater_than_sign_in_a_comment_or_an_attribute_value_closes_nothing() {
        assert_eq!(closed("<!-- <b|", "html"), None);
        assert_eq!(closed("<a title=\"x|", "html"), None);
    }

    #[test]
    fn an_existing_close_tag_is_not_inserted_again() {
        assert_eq!(closed("<div|</div>", "html"), None);
        assert_eq!(closed("<div|</DIV>", "html"), None);
        assert_eq!(closed("<b|</i>", "html"), some("<b>|</b></i>"));
    }

    #[test]
    fn script_and_style_elements_close_no_tags_inside_them() {
        assert_eq!(closed("<script>\nif (a<b|", "html"), None);
        assert_eq!(closed("<style>\na<b|", "html"), None);
        assert_eq!(
            closed("<script>x</script>\n<p|", "html"),
            some("<script>x</script>\n<p>|</p>")
        );
    }

    #[test]
    fn the_script_open_tag_itself_is_closed() {
        assert_eq!(closed("<script|", "html"), some("<script>|</script>"));
    }

    #[test]
    fn an_apostrophe_in_markup_text_does_not_stop_closing() {
        assert_eq!(closed("<p>Don't <b|", "html"), some("<p>Don't <b>|</b>"));
    }

    #[test]
    fn a_fragment_closes_only_in_jsx() {
        assert_eq!(closed("<|", "html"), None);
        assert_eq!(closed("<|", "vue"), None);
        assert_eq!(
            closed("return <|", "typescriptreact"),
            some("return <>|</>")
        );
        assert_eq!(closed("return <|</>", "typescriptreact"), None);
    }

    #[test]
    fn vue_components_close_and_lowercase_void_elements_do_not() {
        assert_eq!(closed("<MyComp|", "vue"), some("<MyComp>|</MyComp>"));
        assert_eq!(closed("<Input|", "vue"), some("<Input>|</Input>"));
        assert_eq!(closed("<input|", "vue"), None);
        assert_eq!(closed("<Br|", "svelte"), some("<Br>|</Br>"));
        assert_eq!(closed("<br|", "svelte"), None);
    }

    #[test]
    fn a_quoted_greater_than_sign_in_vue_closes_nothing() {
        assert_eq!(closed("<div :class=\"a |", "vue"), None);
        assert_eq!(closed("<div :class=\"a > b\"|", "vue"), None);
    }

    #[test]
    fn an_arrow_inside_a_svelte_attribute_expression_closes_nothing() {
        assert_eq!(closed("<button on:click={() =|", "svelte"), None);
        assert_eq!(
            closed("<button on:click={go}|", "svelte"),
            some("<button on:click={go}>|</button>")
        );
    }

    #[test]
    fn svelte_special_elements_close() {
        assert_eq!(
            closed("<svelte:head|", "svelte"),
            some("<svelte:head>|</svelte:head>")
        );
    }

    #[test]
    fn jsx_after_return_or_an_open_bracket_closes() {
        assert_eq!(
            closed("return <div|", "typescriptreact"),
            some("return <div>|</div>")
        );
        assert_eq!(
            closed("return (\n  <div|", "typescriptreact"),
            some("return (\n  <div>|</div>")
        );
        assert_eq!(
            closed("const a = <Foo.Bar|", "javascriptreact"),
            some("const a = <Foo.Bar>|</Foo.Bar>")
        );
        assert_eq!(
            closed("<ul>{items.map((i) => <li|", "typescriptreact"),
            some("<ul>{items.map((i) => <li>|</li>")
        );
        assert_eq!(closed("<div|", "typescriptreact"), some("<div>|</div>"));
    }

    #[test]
    fn jsx_closes_every_name_including_html_void_elements() {
        assert_eq!(
            closed("return <input|", "javascriptreact"),
            some("return <input>|</input>")
        );
    }

    #[test]
    fn jsx_keywords_count_only_as_whole_words() {
        assert_eq!(
            closed("export default <App|", "typescriptreact"),
            some("export default <App>|</App>")
        );
        assert_eq!(closed("myreturn <b|", "typescriptreact"), None);
    }

    #[test]
    fn type_arguments_and_comparisons_in_jsx_files_close_nothing() {
        assert_eq!(
            closed("const [a, b] = useState<string|", "typescriptreact"),
            None
        );
        assert_eq!(closed("if (a <b|", "typescriptreact"), None);
        assert_eq!(closed("const f = <T,|", "typescriptreact"), None);
    }

    #[test]
    fn arrows_in_jsx_files_close_nothing() {
        assert_eq!(closed("<div onClick={() =|", "typescriptreact"), None);
        assert_eq!(closed("const f = x =|", "typescriptreact"), None);
        assert_eq!(closed("const f = (x) =|", "javascriptreact"), None);
    }

    #[test]
    fn a_tag_in_a_jsx_string_or_comment_closes_nothing() {
        assert_eq!(closed("const s = \"<div|", "typescriptreact"), None);
        assert_eq!(closed("// <div|", "typescriptreact"), None);
    }

    #[test]
    fn languages_without_tags_close_nothing() {
        assert_eq!(closed("let v: Vec<u8|", "rust"), None);
        assert_eq!(closed("<div|", "plain_text"), None);
        assert_eq!(closed("<div|", "typescript"), None);
    }

    #[test]
    fn close_tag_needs_a_typed_greater_than_sign() {
        assert_eq!(close_tag("<div", 4, "html"), None);
        assert_eq!(close_tag("<div x", 4, "html"), None);
        assert_eq!(close_tag("<div>", 99, "html"), None);
    }

    #[test]
    fn slash_after_a_less_than_sign_completes_the_innermost_open_element() {
        assert_eq!(
            completed("<div><span></span><|", "html"),
            some("<div><span></span></div>|")
        );
        assert_eq!(
            completed("<section>\n  <p>a</p>\n  <|", "html"),
            some("<section>\n  <p>a</p>\n  </section>|")
        );
    }

    #[test]
    fn a_greater_than_sign_after_the_cursor_is_kept_and_skipped() {
        assert_eq!(
            completed("<div><span></span><|>", "html"),
            some("<div><span></span></div>|")
        );
    }

    #[test]
    fn void_and_self_closing_elements_are_never_open() {
        assert_eq!(completed("<div><br><|", "html"), some("<div><br></div>|"));
        assert_eq!(
            completed("<div><img/><|", "html"),
            some("<div><img/></div>|")
        );
        assert_eq!(
            completed("<div><img src=\"a\" /><|", "html"),
            some("<div><img src=\"a\" /></div>|")
        );
    }

    #[test]
    fn a_close_tag_drops_the_unclosed_elements_opened_after_its_element() {
        assert_eq!(completed("<ul><li><p>a</ul><|", "html"), None);
        assert_eq!(
            completed("<div><ul><li>a</ul><|", "html"),
            some("<div><ul><li>a</ul></div>|")
        );
    }

    #[test]
    fn a_close_tag_without_an_open_element_is_ignored() {
        assert_eq!(completed("<div></b><|", "html"), some("<div></b></div>|"));
    }

    #[test]
    fn html_close_tags_match_their_element_whatever_the_case() {
        assert_eq!(
            completed("<div><P>a</p><|", "html"),
            some("<div><P>a</p></div>|")
        );
        assert_eq!(
            completed("<div><Comp>a</comp><|", "vue"),
            some("<div><Comp>a</comp></Comp>|")
        );
    }

    #[test]
    fn jsx_fragments_open_and_close() {
        assert_eq!(
            completed("return <><b></b><|", "typescriptreact"),
            some("return <><b></b></>|")
        );
        assert_eq!(
            completed("return <div><></><|", "typescriptreact"),
            some("return <div><></></div>|")
        );
        assert_eq!(completed("return <><|>", "typescriptreact"), None);
    }

    #[test]
    fn jsx_type_arguments_and_arrows_open_no_element() {
        assert_eq!(
            completed("return <div>{f<string>(x)}<|", "typescriptreact"),
            some("return <div>{f<string>(x)}</div>|")
        );
    }

    #[test]
    fn a_greater_than_sign_inside_an_attribute_expression_does_not_end_the_open_tag() {
        assert_eq!(
            completed(
                "return (\n  <div>\n    <button onClick={() => go()}>\n      <|",
                "typescriptreact"
            ),
            some("return (\n  <div>\n    <button onClick={() => go()}>\n      </button>|")
        );
        assert_eq!(
            completed(
                "return <button onClick={() => go()}>\n  <|",
                "typescriptreact"
            ),
            some("return <button onClick={() => go()}>\n  </button>|")
        );
        assert_eq!(
            completed(
                "return <ul>\n  <Item show={a > b}>\n    <|",
                "typescriptreact"
            ),
            some("return <ul>\n  <Item show={a > b}>\n    </Item>|")
        );
        assert_eq!(
            completed("<div>\n  <button on:click={() => n++}>\n    <|", "svelte"),
            some("<div>\n  <button on:click={() => n++}>\n    </button>|")
        );
        assert_eq!(
            completed(
                "<ul>{items.map((i) => <li key={i}>\n  <|",
                "typescriptreact"
            ),
            some("<ul>{items.map((i) => <li key={i}>\n  </li>|")
        );
    }

    #[test]
    fn tags_inside_an_attribute_expression_belong_to_that_expression() {
        assert_eq!(
            completed("return <Route element={<Home />}>\n  <|", "typescriptreact"),
            some("return <Route element={<Home />}>\n  </Route>|")
        );
        assert_eq!(
            completed(
                "return <List render={() => <li>a</li>}>\n  <|",
                "javascriptreact"
            ),
            some("return <List render={() => <li>a</li>}>\n  </List>|")
        );
    }

    #[test]
    fn a_tag_whose_attribute_expression_never_ends_opens_no_element() {
        assert_eq!(
            completed(
                "return <div>\n  <b onClick={() => go()>\n  <|",
                "typescriptreact"
            ),
            some("return <div>\n  <b onClick={() => go()>\n  </div>|")
        );
        let long = "x > ".repeat(MAX_GREATER_THAN_SIGNS_IN_EXPRESSIONS + 1);
        let marked = format!("<div><b title={{{long}}}><|");
        let expected = format!("<div><b title={{{long}}}></div>|");
        assert_eq!(completed(&marked, "svelte"), Some(expected));
        let short = "x > ".repeat(MAX_GREATER_THAN_SIGNS_IN_EXPRESSIONS);
        let marked = format!("<div><b title={{{short}}}><|");
        let expected = format!("<div><b title={{{short}}}></b>|");
        assert_eq!(completed(&marked, "svelte"), Some(expected));
    }

    #[test]
    fn nothing_open_completes_nothing() {
        assert_eq!(completed("<|", "html"), None);
        assert_eq!(completed("<div></div><|", "html"), None);
        assert_eq!(completed("<|", "rust"), None);
    }

    #[test]
    fn markup_comments_are_skipped_and_slash_inside_one_completes_nothing() {
        assert_eq!(
            completed("<div><!-- <span> --><|", "html"),
            some("<div><!-- <span> --></div>|")
        );
        assert_eq!(completed("<div><!-- a <|", "html"), None);
    }

    #[test]
    fn slash_inside_an_attribute_value_or_a_script_completes_nothing() {
        assert_eq!(completed("<div><a title=\"x <|", "html"), None);
        assert_eq!(completed("<div><script>\nif (a <|", "html"), None);
    }

    #[test]
    fn complete_close_tag_needs_a_slash_typed_after_a_less_than_sign() {
        assert_eq!(complete_close_tag("<div>a/", 6, "html"), None);
        assert_eq!(complete_close_tag("<div><", 5, "html"), None);
        assert_eq!(complete_close_tag("<div></", 99, "html"), None);
    }

    #[test]
    fn only_the_last_64_kib_before_the_slash_are_read() {
        let filler = "<p>a</p>\n".repeat(SCAN_LIMIT / 9 + 1);
        assert_eq!(completed(&format!("<div>\n{filler}<|"), "html"), None);
        let marked = format!("<div>\n{filler}<section><|");
        let expected = format!("<div>\n{filler}<section></section>|");
        assert_eq!(completed(&marked, "html"), Some(expected));
    }

    #[test]
    fn multibyte_text_keeps_offsets_on_char_boundaries() {
        assert_eq!(closed("<p>héllo <b|", "html"), some("<p>héllo <b>|</b>"));
        assert_eq!(completed("<p>héllo <|", "html"), some("<p>héllo </p>|"));
        // The scan starts 64 KiB before the `<`, which here is inside an "é".
        let filler = "é".repeat(40_000);
        let marked = format!("<div>{filler}<b><|");
        assert_eq!(
            completed(&marked, "html"),
            Some(format!("<div>{filler}<b></b>|"))
        );
    }
}
