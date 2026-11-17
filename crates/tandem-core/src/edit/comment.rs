//! Line and block comments: the markers of a file type and toggling them.
use super::{Edit, Splice, block_lines, indent_len, line_block, line_end, line_start, splice};
use std::ops::Range;

/// The comment markers of a file type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentTokens {
    pub line: Option<&'static str>,
    pub block: Option<(&'static str, &'static str)>,
}

const fn tokens(
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
) -> CommentTokens {
    CommentTokens { line, block }
}

const C_STYLE: CommentTokens = tokens(Some("//"), Some(("/*", "*/")));
const HASH: CommentTokens = tokens(Some("#"), None);

/// The comment markers for a project-relative path, by file name first, then by extension.
pub fn comment_tokens(path: &str) -> CommentTokens {
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    };
    let hash_name = matches!(
        name,
        "Dockerfile"
            | "Containerfile"
            | "Makefile"
            | "makefile"
            | "GNUmakefile"
            | "CMakeLists.txt"
            | ".gitignore"
            | ".gitattributes"
            | ".dockerignore"
            | ".npmignore"
            | ".editorconfig"
            | ".env"
            | "Gemfile"
            | "Rakefile"
    ) || name.starts_with("Dockerfile.")
        || name.starts_with(".env.");
    if hash_name {
        return HASH;
    }
    let jsonc_name = matches!(
        name,
        "tsconfig.json" | "jsconfig.json" | "devcontainer.json"
    );
    if jsonc_name || (extension == "json" && path.split('/').any(|part| part == ".vscode")) {
        return C_STYLE;
    }
    match extension.as_str() {
        "rs" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "go" | "java"
        | "kt" | "kts" | "swift" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm"
        | "cs" | "scala" | "dart" | "php" | "zig" | "proto" | "groovy" | "gradle" | "jsonc"
        | "json5" | "scss" | "less" => C_STYLE,
        "py" | "pyi" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh" | "fish" | "rb" | "pl"
        | "pm" | "r" | "tf" | "tfvars" | "conf" | "cfg" | "properties" | "mk" | "cmake" | "nix"
        | "ex" | "exs" | "jl" | "graphql" | "gql" | "dockerfile" => HASH,
        "ps1" => tokens(Some("#"), Some(("<#", "#>"))),
        "sql" => tokens(Some("--"), Some(("/*", "*/"))),
        "lua" => tokens(Some("--"), Some(("--[[", "]]"))),
        "hs" | "elm" => tokens(Some("--"), Some(("{-", "-}"))),
        "ini" | "clj" | "cljs" | "cljc" | "edn" | "lisp" | "el" | "scm" => tokens(Some(";"), None),
        "tex" | "sty" | "cls" | "erl" | "hrl" => tokens(Some("%"), None),
        "vim" => tokens(Some("\""), None),
        "html" | "htm" | "xhtml" | "xml" | "svg" | "md" | "markdown" | "vue" | "svelte"
        | "plist" | "xsl" | "xslt" => tokens(None, Some(("<!--", "-->"))),
        "css" => tokens(None, Some(("/*", "*/"))),
        _ => tokens(None, None),
    }
}

/// What `comment_lines` does to the selected lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineComment {
    Toggle,
    Add,
    Remove,
}

/// Comments or uncomments the lines `selection` touches with `tokens.line`, placing the
/// marker at the smallest indentation of the lines that have content. Without a line
/// marker the lines are wrapped in one block comment. `None` when there is nothing to do.
pub fn comment_lines(
    text: &str,
    selection: &Range<usize>,
    tokens: &CommentTokens,
    action: LineComment,
) -> Option<Edit> {
    match (tokens.line, tokens.block) {
        (Some(marker), _) => line_markers(text, selection, marker, action),
        (None, Some((open, close))) => wrap_lines(text, selection, open, close, action),
        (None, None) => None,
    }
}

fn line_markers(
    text: &str,
    selection: &Range<usize>,
    marker: &str,
    action: LineComment,
) -> Option<Edit> {
    let lines = block_lines(text, line_block(text, selection));
    let mut content: Vec<Range<usize>> = lines
        .iter()
        .filter(|line| text[(*line).clone()].contains(|c: char| !c.is_whitespace()))
        .cloned()
        .collect();
    if content.is_empty() {
        content = lines;
    }
    let marker_at = |line: &Range<usize>| line.start + indent_len(&text[line.clone()]);
    let is_commented = |line: &Range<usize>| text[marker_at(line)..line.end].starts_with(marker);
    let add = match action {
        LineComment::Toggle => !content.iter().all(is_commented),
        LineComment::Add => true,
        LineComment::Remove => false,
    };
    let insert = format!("{marker} ");
    let splices: Vec<Splice<'_>> = if add {
        let column = content
            .iter()
            .map(|line| indent_len(&text[line.clone()]))
            .min()
            .unwrap_or(0);
        content
            .iter()
            .map(|line| Splice {
                at: line.start + column,
                remove: 0,
                insert: &insert,
            })
            .collect()
    } else {
        content
            .iter()
            .filter(|line| is_commented(line))
            .map(|line| {
                let at = marker_at(line);
                let space = text[at + marker.len()..line.end].starts_with(' ');
                Splice {
                    at,
                    remove: marker.len() + usize::from(space),
                    insert: "",
                }
            })
            .collect()
    };
    if splices.is_empty() {
        return None;
    }
    let (new, selection) = splice(text, &splices, selection);
    Some(Edit::between(text, &new, selection))
}

/// Wraps the lines `selection` touches in one block comment, from the first line's first
/// non-whitespace char to the end of the last line, or unwraps them.
fn wrap_lines(
    text: &str,
    selection: &Range<usize>,
    open: &str,
    close: &str,
    action: LineComment,
) -> Option<Edit> {
    let block = line_block(text, selection);
    let start = block.start + indent_len(&text[block.start..line_end(text, block.start)]);
    let end = block.end;
    let inner = &text[start..end];
    let wrapped = inner.len() >= open.len() + close.len()
        && inner.starts_with(open)
        && inner.ends_with(close);
    let unwrap = match action {
        LineComment::Toggle => wrapped,
        LineComment::Add => false,
        LineComment::Remove if wrapped => true,
        LineComment::Remove => return None,
    };
    if unwrap {
        let splices = unwrap_splices(text, start, open, end - close.len(), close);
        let (new, selection) = splice(text, &splices, selection);
        return Some(Edit::between(text, &new, selection));
    }
    if start == end {
        return Some(insert_empty_comment(text, start, open, close));
    }
    let (open, close) = (format!("{open} "), format!(" {close}"));
    let splices = [
        Splice {
            at: start,
            remove: 0,
            insert: &open,
        },
        Splice {
            at: end,
            remove: 0,
            insert: &close,
        },
    ];
    let (new, selection) = splice(text, &splices, selection);
    Some(Edit::between(text, &new, selection))
}

/// Wraps the selection in `open` and `close`, or removes the block comment that the
/// selection is or that surrounds it or the cursor.
pub fn toggle_block_comment(
    text: &str,
    selection: &Range<usize>,
    open: &str,
    close: &str,
) -> Option<Edit> {
    let start = selection.start.min(text.len());
    let end = selection.end.min(text.len()).max(start);
    if start == end {
        return Some(toggle_at_cursor(text, start, open, close));
    }
    let unwrapped = |splices: &[Splice<'_>]| {
        let (new, selection) = splice(text, splices, &(start..end));
        Some(Edit::between(text, &new, selection))
    };

    let selected = &text[start..end];
    let inner = selected.trim();
    let inner_start = start + (selected.len() - selected.trim_start().len());
    if inner.len() >= open.len() + close.len() && inner.starts_with(open) && inner.ends_with(close)
    {
        let close_at = inner_start + inner.len() - close.len();
        return unwrapped(&unwrap_splices(text, inner_start, open, close_at, close));
    }

    let (open_spaced, close_spaced) = (format!("{open} "), format!(" {close}"));
    let before = &text[..start];
    let after = &text[end..];
    let open_len = if before.ends_with(&open_spaced) {
        Some(open_spaced.len())
    } else {
        before.ends_with(open).then_some(open.len())
    };
    let close_len = if after.starts_with(&close_spaced) {
        Some(close_spaced.len())
    } else {
        after.starts_with(close).then_some(close.len())
    };
    if let (Some(open_len), Some(close_len)) = (open_len, close_len) {
        return unwrapped(&[
            Splice {
                at: start - open_len,
                remove: open_len,
                insert: "",
            },
            Splice {
                at: end,
                remove: close_len,
                insert: "",
            },
        ]);
    }

    let splices = [
        Splice {
            at: start,
            remove: 0,
            insert: &open_spaced,
        },
        Splice {
            at: end,
            remove: 0,
            insert: &close_spaced,
        },
    ];
    let (new, _) = splice(text, &splices, &(start..end));
    let shift = open.len() + 1;
    Some(Edit::between(text, &new, start + shift..end + shift))
}

/// Removes the block comment around the cursor on its line, or inserts an empty one.
fn toggle_at_cursor(text: &str, cursor: usize, open: &str, close: &str) -> Edit {
    let (start, end) = (line_start(text, cursor), line_end(text, cursor));
    let open_at = (start..=cursor.min(end))
        .rev()
        .find(|&i| text.is_char_boundary(i) && text[i..end].starts_with(open));
    let close_at = open_at.and_then(|open_at| {
        let after_open = open_at + open.len();
        text[after_open..end].find(close).map(|i| after_open + i)
    });
    if let (Some(open_at), Some(close_at)) = (open_at, close_at)
        && cursor <= close_at + close.len()
    {
        let splices = unwrap_splices(text, open_at, open, close_at, close);
        let (new, selection) = splice(text, &splices, &(cursor..cursor));
        return Edit::between(text, &new, selection);
    }
    insert_empty_comment(text, cursor, open, close)
}

/// Inserts `"{open}  {close}"` at `at` with the cursor between the two spaces.
fn insert_empty_comment(text: &str, at: usize, open: &str, close: &str) -> Edit {
    let comment = format!("{open}  {close}");
    let (new, _) = splice(
        text,
        &[Splice {
            at,
            remove: 0,
            insert: &comment,
        }],
        &(at..at),
    );
    let cursor = at + open.len() + 1;
    Edit::between(text, &new, cursor..cursor)
}

/// Splices that remove `open` at `open_at` and `close` at `close_at`, each with one space
/// on its inner side when there is one that the other token does not already remove.
fn unwrap_splices(
    text: &str,
    open_at: usize,
    open: &str,
    close_at: usize,
    close: &str,
) -> [Splice<'static>; 2] {
    let open_end = open_at + open.len();
    let open_space = open_end < close_at && text.as_bytes()[open_end] == b' ';
    let open_remove = open.len() + usize::from(open_space);
    let close_space = close_at > open_at + open_remove && text.as_bytes()[close_at - 1] == b' ';
    let close_start = close_at - usize::from(close_space);
    [
        Splice {
            at: open_at,
            remove: open_remove,
            insert: "",
        },
        Splice {
            at: close_start,
            remove: close_at + close.len() - close_start,
            insert: "",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLASH: CommentTokens = CommentTokens {
        line: Some("//"),
        block: Some(("/*", "*/")),
    };
    const HASH: CommentTokens = CommentTokens {
        line: Some("#"),
        block: None,
    };
    const HTML: CommentTokens = CommentTokens {
        line: None,
        block: Some(("<!--", "-->")),
    };
    const CSS: CommentTokens = CommentTokens {
        line: None,
        block: Some(("/*", "*/")),
    };
    const NONE: CommentTokens = CommentTokens {
        line: None,
        block: None,
    };

    fn commented(
        text: &str,
        selection: Range<usize>,
        tokens: &CommentTokens,
        action: LineComment,
    ) -> (String, Range<usize>) {
        let edit = comment_lines(text, &selection, tokens, action).expect("an edit");
        (edit.apply(text), edit.selection)
    }

    fn toggled(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        commented(text, selection, &SLASH, LineComment::Toggle)
    }

    fn block_toggled(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        let edit = toggle_block_comment(text, &selection, "/*", "*/").expect("an edit");
        (edit.apply(text), edit.selection)
    }

    fn owned(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
        (text.to_string(), selection)
    }

    #[test]
    fn comment_tokens_by_file_name_and_extension() {
        let c = (Some("//"), Some(("/*", "*/")));
        let hash = (Some("#"), None);
        let html = (None, Some(("<!--", "-->")));
        let none = (None, None);
        let cases = [
            ("src/main.rs", c),
            ("a/b.py", hash),
            ("Cargo.toml", hash),
            ("x.ts", c),
            ("x.tsx", c),
            ("x.js", c),
            ("main.go", c),
            ("A.java", c),
            ("run.sh", hash),
            ("ci.yml", hash),
            ("a.yaml", hash),
            ("README.md", html),
            ("index.html", html),
            ("App.svelte", html),
            ("style.css", (None, Some(("/*", "*/")))),
            ("a.jsonc", c),
            ("tsconfig.json", c),
            (".vscode/settings.json", c),
            ("web/.vscode/launch.json", c),
            ("package.json", none),
            ("Dockerfile", hash),
            ("docker/Dockerfile.dev", hash),
            ("Makefile", hash),
            (".gitignore", hash),
            ("web/.env.local", hash),
            ("notes.txt", none),
            ("MAIN.RS", c),
            ("query.sql", (Some("--"), Some(("/*", "*/")))),
            ("init.lua", (Some("--"), Some(("--[[", "]]")))),
            ("Main.hs", (Some("--"), Some(("{-", "-}")))),
            ("profile.ps1", (Some("#"), Some(("<#", "#>")))),
            ("core.clj", (Some(";"), None)),
            ("paper.tex", (Some("%"), None)),
            ("init.vim", (Some("\""), None)),
            ("LICENSE", none),
            (".hidden", none),
        ];
        for (path, (line, block)) in cases {
            assert_eq!(
                comment_tokens(path),
                CommentTokens { line, block },
                "{path}"
            );
        }
    }

    #[test]
    fn comment_goes_before_the_first_non_whitespace_char() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        assert_eq!(
            toggled(text, 16..16),
            owned("fn main() {\n    // let x = 1;\n}\n", 19..19)
        );
        assert_eq!(
            toggled(text, 14..14),
            owned("fn main() {\n    // let x = 1;\n}\n", 14..14)
        );
    }

    #[test]
    fn toggling_twice_restores_text_and_selection() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        for selection in [16..16, 14..14, 12..26, 0..29] {
            let (once, after) = toggled(text, selection.clone());
            assert_eq!(toggled(&once, after), owned(text, selection));
        }
    }

    #[test]
    fn uncomment_removes_the_marker_and_one_space() {
        assert_eq!(toggled("//x", 3..3), owned("x", 1..1));
        assert_eq!(toggled("//  x", 0..0).0, " x");
        assert_eq!(toggled("    // x", 8..8), owned("    x", 5..5));
    }

    #[test]
    fn lines_with_different_indentation_share_the_smallest_column() {
        assert_eq!(
            toggled("    a\n        b\n", 0..16).0,
            "    // a\n    //     b\n"
        );
    }

    #[test]
    fn blank_lines_stay_unchanged_and_do_not_lower_the_column() {
        assert_eq!(
            toggled("    a\n\n        b\n", 0..17).0,
            "    // a\n\n    //     b\n"
        );
        assert_eq!(
            toggled("    a\n  \n    b", 0..14).0,
            "    // a\n  \n    // b"
        );
    }

    #[test]
    fn a_mixed_selection_is_commented() {
        assert_eq!(toggled("// a\nb\n", 0..6).0, "// // a\n// b\n");
    }

    #[test]
    fn commented_lines_with_different_indentation_are_uncommented() {
        assert_eq!(toggled("  // a\n      // b\n", 0..18).0, "  a\n      b\n");
    }

    #[test]
    fn a_selection_ending_at_column_zero_leaves_that_line_alone() {
        assert_eq!(toggled("a\nb\n", 0..2), owned("// a\nb\n", 0..5));
    }

    #[test]
    fn a_selection_starting_at_the_insertion_point_keeps_its_start() {
        assert_eq!(toggled("    abc", 4..7), owned("    // abc", 4..10));
    }

    #[test]
    fn empty_and_whitespace_only_lines_are_commented() {
        assert_eq!(toggled("", 0..0), owned("// ", 3..3));
        assert_eq!(toggled("    ", 4..4), owned("    // ", 7..7));
    }

    #[test]
    fn tab_indentation_is_kept() {
        assert_eq!(toggled("\tfoo", 0..0).0, "\t// foo");
    }

    #[test]
    fn crlf_line_breaks_are_kept() {
        let (once, selection) = toggled("    a\r\n    b\r\n", 0..12);
        assert_eq!(once, "    // a\r\n    // b\r\n");
        assert_eq!(toggled(&once, selection).0, "    a\r\n    b\r\n");
    }

    #[test]
    fn the_last_line_without_a_trailing_newline() {
        assert_eq!(toggled("a\n  b", 4..4), owned("a\n  // b", 7..7));
    }

    #[test]
    fn non_ascii_lines() {
        let (once, selection) = toggled("    héllo", 10..10);
        assert_eq!((once.as_str(), selection.clone()), ("    // héllo", 13..13));
        assert_eq!(toggled(&once, selection), owned("    héllo", 10..10));
    }

    #[test]
    fn add_and_remove_ignore_the_current_state() {
        assert_eq!(
            commented("// a", 0..0, &SLASH, LineComment::Add).0,
            "// // a"
        );
        assert_eq!(
            comment_lines("a", &(0..0), &SLASH, LineComment::Remove),
            None
        );
        assert_eq!(
            commented("// a\nb\n", 0..6, &SLASH, LineComment::Remove).0,
            "a\nb\n"
        );
    }

    #[test]
    fn hash_comments() {
        assert_eq!(
            commented("  x = 1", 0..0, &HASH, LineComment::Toggle).0,
            "  # x = 1"
        );
    }

    #[test]
    fn no_comment_markers_means_no_edit() {
        assert_eq!(
            comment_lines("x", &(0..0), &NONE, LineComment::Toggle),
            None
        );
    }

    #[test]
    fn block_fallback_wraps_a_line_at_its_indentation() {
        let text = "  <p>hi</p>\n";
        let (once, selection) = commented(text, 5..5, &HTML, LineComment::Toggle);
        assert_eq!(
            (once.as_str(), selection.clone()),
            ("  <!-- <p>hi</p> -->\n", 10..10)
        );
        assert_eq!(
            commented(&once, selection, &HTML, LineComment::Toggle),
            owned(text, 5..5)
        );
    }

    #[test]
    fn block_fallback_wraps_several_lines_in_one_comment() {
        assert_eq!(
            commented("  <a>\n    <b>\n  <c>\n", 0..20, &HTML, LineComment::Toggle).0,
            "  <!-- <a>\n    <b>\n  <c> -->\n"
        );
    }

    #[test]
    fn block_fallback_on_a_blank_line_puts_the_cursor_inside() {
        let (once, selection) = commented("a\n\nb", 2..2, &HTML, LineComment::Toggle);
        assert_eq!(
            (once.as_str(), selection.clone()),
            ("a\n<!--  -->\nb", 7..7)
        );
        assert_eq!(
            commented(&once, selection, &HTML, LineComment::Toggle),
            owned("a\n\nb", 2..2)
        );
    }

    #[test]
    fn block_fallback_for_css() {
        assert_eq!(
            commented("  color: red;", 0..0, &CSS, LineComment::Toggle).0,
            "  /* color: red; */"
        );
    }

    #[test]
    fn block_fallback_remove_on_unwrapped_lines_is_none() {
        assert_eq!(
            comment_lines("  <p>hi</p>", &(0..0), &HTML, LineComment::Remove),
            None
        );
        assert_eq!(
            commented("<!-- a -->", 0..0, &HTML, LineComment::Remove).0,
            "a"
        );
    }

    #[test]
    fn block_comment_wraps_the_selection_and_keeps_it_selected() {
        let (once, selection) = block_toggled("let x = 1;", 4..9);
        assert_eq!(
            (once.as_str(), selection.clone()),
            ("let /* x = 1 */;", 7..12)
        );
        assert_eq!(block_toggled(&once, selection), owned("let x = 1;", 4..9));
    }

    #[test]
    fn block_comment_unwraps_a_selection_that_includes_the_tokens() {
        assert_eq!(
            block_toggled("let /* x = 1 */;", 4..15),
            owned("let x = 1;", 4..9)
        );
        assert_eq!(
            block_toggled("let /* x = 1 */;", 3..15),
            owned("let x = 1;", 3..9)
        );
        assert_eq!(block_toggled("a /* */ b", 2..7), owned("a  b", 2..2));
    }

    #[test]
    fn block_comment_cursor_inside_a_comment_removes_it() {
        assert_eq!(block_toggled("a /* b */ c", 5..5), owned("a b c", 2..2));
        assert_eq!(block_toggled("a /* b */ c", 9..9), owned("a b c", 3..3));
    }

    #[test]
    fn block_comment_cursor_without_a_comment_inserts_an_empty_one() {
        assert_eq!(block_toggled("ab", 1..1), owned("a/*  */b", 4..4));
        assert_eq!(block_toggled("", 0..0), owned("/*  */", 3..3));
        assert_eq!(
            block_toggled("/* a */ b", 9..9),
            owned("/* a */ b/*  */", 12..12)
        );
    }

    #[test]
    fn block_comment_wraps_a_multi_line_selection() {
        let (once, selection) = block_toggled("a\nb\nc", 0..3);
        assert_eq!((once.as_str(), selection.clone()), ("/* a\nb */\nc", 3..6));
        assert_eq!(block_toggled(&once, selection), owned("a\nb\nc", 0..3));
    }

    #[test]
    fn block_comment_with_html_tokens() {
        let edit = toggle_block_comment("<p>hi</p>", &(3..5), "<!--", "-->").unwrap();
        assert_eq!(edit.apply("<p>hi</p>"), "<p><!-- hi --></p>");
        assert_eq!(edit.selection, 8..10);
    }

    #[test]
    fn block_comment_with_non_ascii_content() {
        let (once, selection) = block_toggled("é = ü", 0..2);
        assert_eq!((once.as_str(), selection.clone()), ("/* é */ = ü", 3..5));
        assert_eq!(block_toggled(&once, selection), owned("é = ü", 0..2));
    }
}
