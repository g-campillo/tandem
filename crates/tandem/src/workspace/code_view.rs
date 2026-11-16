//! Code in the chat panel: syntax colours from the editor's highlight theme, and the
//! explanation of the part of a step's code under the pointer.
use super::chat_logic::hover_after;
use super::*;
use crate::appearance::{palette, panel_code};
use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::component::{ActiveTheme, Rope, StyledExt};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Once;
use tandem_claude::view::{self, BlockKind, BlockPart, StepBlock, StepCard};

/// Registers the fence tags of `view::FENCE_ALIASES` with the toolkit, once, so that
/// fenced code in Claude's replies is coloured for tags such as `shell` or `jsx`.
pub(super) fn register_fence_aliases() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let registry = LanguageRegistry::singleton();
        for (tag, grammar) in view::FENCE_ALIASES {
            if registry.language(tag).is_none()
                && let Some(config) = registry.language(grammar)
            {
                registry.register(tag, &config);
            }
        }
    });
}

/// The syntax colours of `code` in `grammar`: sorted byte ranges that do not overlap.
/// Empty when the toolkit has no grammar of that name.
///
/// A step's code is often a piece of a file that is only valid inside its enclosing item,
/// such as a struct field or a match arm. Such code is coloured inside the first of the
/// grammar's `enclosures` in which it parses without an error, so it gets the colours the
/// editor gives it in the file. Code that is valid alone, or in none of them, is coloured
/// alone.
pub(super) fn syntax_runs(
    code: &str,
    grammar: &str,
    theme: &HighlightTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let language = LanguageRegistry::singleton()
        .language(grammar)
        .and_then(|config| config.language);
    let Some(language) = language.filter(|_| !code.is_empty()) else {
        return Vec::new();
    };
    let (before, after) = std::iter::once(&("", ""))
        .chain(enclosures(grammar))
        .find(|(before, after)| parses_cleanly(&language, &enclosed(before, code, after)))
        .copied()
        .unwrap_or(("", ""));
    // A new highlighter each time: a reused one would need an exact edit of its old text.
    let mut highlighter = SyntaxHighlighter::new(grammar);
    highlighter.update(None, &Rope::from_str(&enclosed(before, code, after)), None);
    let start = before.len();
    highlighter
        .styles(&(start..start + code.len()), theme)
        .into_iter()
        .filter(|(range, style)| !range.is_empty() && *style != HighlightStyle::default())
        .map(|(range, style)| (range.start - start..range.end - start, style))
        .collect()
}

/// `code` between `before` and `after`, with a line break at the end because some grammars
/// (Go) need one after the last statement.
fn enclosed(before: &str, code: &str, after: &str) -> String {
    format!("{before}{code}{after}\n")
}

/// Whether `text` parses in `language` without an error or a missing token. A bare parse,
/// because a highlighter also compiles its queries, which takes far longer.
fn parses_cleanly(language: &tree_sitter::Language, text: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(language).is_ok()
        && parser
            .parse(text, None)
            .is_some_and(|tree| !tree.root_node().has_error())
}

/// The items a piece of code in `grammar` may sit in, as (text before, text after) it, in
/// the order they are tried.
fn enclosures(grammar: &str) -> &'static [(&'static str, &'static str)] {
    match grammar {
        "rust" => &[
            // Fields of a struct, variants of an enum, arms of a match, fields of a struct
            // literal.
            ("struct S {\n", "\n}"),
            ("enum E {\n", "\n}"),
            ("fn f() {\nmatch x {\n", "\n}\n}"),
            ("fn f() {\nS {\n", "\n};\n}"),
        ],
        "go" => &[
            ("type s struct {\n", "\n}"),
            ("type i interface {\n", "\n}"),
            ("func f() {\nswitch {\n", "\n}\n}"),
        ],
        "typescript" => &[
            ("interface I {\n", "\n}"),
            ("class C {\n", "\n}"),
            ("x = {\n", "\n};"),
            ("switch (x) {\n", "\n}"),
        ],
        "javascript" => &[
            ("class C {\n", "\n}"),
            ("x = {\n", "\n};"),
            ("switch (x) {\n", "\n}"),
        ],
        "java" => &[
            ("class C {\n", "\n}"),
            ("enum E {\n", "\n}"),
            ("class C {\nvoid f() {\nswitch (x) {\n", "\n}\n}\n}"),
        ],
        // Cases of a match; other indented code parses alone.
        "python" => &[("match x:\n", "")],
        // Members of an object and elements of an array, the last one or one with more after it.
        "json" => &[
            ("{\n", "\n}"),
            ("{\n", "\n\"\": 0\n}"),
            ("[\n", "\n]"),
            ("[\n", "\n0\n]"),
        ],
        _ => &[],
    }
}

/// The runs to paint: the syntax colours, an underline on `marked` spans and
/// `background` on `hovered` spans.
pub(super) fn painted_runs(
    syntax: &[(Range<usize>, HighlightStyle)],
    marked: &[Range<usize>],
    underline: Hsla,
    hovered: &[Range<usize>],
    background: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    // Neither extra style sets a colour, so the syntax colour stays under both.
    let line = HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(underline),
            wavy: false,
        }),
        ..Default::default()
    };
    let fill = HighlightStyle {
        background_color: Some(background),
        ..Default::default()
    };
    let extra = marked
        .iter()
        .map(|range| (range.clone(), line))
        .chain(hovered.iter().map(|range| (range.clone(), fill)));
    combine_highlights(syntax.iter().cloned(), extra).collect()
}

/// The syntax colours of one block, kept so a block is parsed once and not on every frame.
struct CachedRuns {
    text: SharedString,
    grammar: &'static str,
    theme: Arc<HighlightTheme>,
    runs: Rc<[(Range<usize>, HighlightStyle)]>,
}

thread_local! {
    static RUNS: RefCell<Vec<CachedRuns>> = const { RefCell::new(Vec::new()) };
}

/// How many blocks the cache keeps; the least recently used one goes first.
const CACHED_BLOCKS: usize = 16;

/// `syntax_runs` of `text`, from the cache when the same text, grammar and theme were
/// coloured before. No grammar means no colours.
fn cached_syntax_runs(
    text: &SharedString,
    grammar: Option<&'static str>,
    theme: &Arc<HighlightTheme>,
) -> Rc<[(Range<usize>, HighlightStyle)]> {
    let Some(grammar) = grammar else {
        return Rc::new([]);
    };
    RUNS.with_borrow_mut(|cache| {
        let hit = cache.iter().position(|entry| {
            entry.grammar == grammar && Arc::ptr_eq(&entry.theme, theme) && entry.text == *text
        });
        if let Some(at) = hit {
            let entry = cache.remove(at);
            let runs = entry.runs.clone();
            cache.push(entry);
            return runs;
        }
        let runs: Rc<[_]> = syntax_runs(text, grammar, theme).into();
        if cache.len() >= CACHED_BLOCKS {
            cache.remove(0);
        }
        cache.push(CachedRuns {
            text: text.clone(),
            grammar,
            theme: theme.clone(),
            runs: runs.clone(),
        });
        runs
    })
}

impl Workspace {
    /// One code block of the step card.
    pub(super) fn code_view(
        &self,
        step: usize,
        block: &StepBlock,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().highlight_theme.clone();
        let text: SharedString = block.text.clone().into();
        let runs = cached_syntax_runs(&text, block.grammar, &theme);
        let p = palette(cx.theme().mode);
        let marked: Vec<Range<usize>> = if block.mark_parts {
            block
                .parts
                .iter()
                .flat_map(|part| part.spans.iter().cloned())
                .collect()
        } else {
            Vec::new()
        };
        let hovered = self
            .chat
            .hover
            .as_ref()
            .filter(|hover| hover.step == step && hover.block == block.kind)
            .and_then(|hover| block.parts.get(hover.part))
            .map_or(&[][..], |part| part.spans.as_slice());
        let painted = painted_runs(&runs, &marked, p.secondary_foreground, hovered, p.selected);
        let id = step * 4 + block.kind as usize;
        let kind = block.kind;
        let face = self.look.fonts.chat_code.clone();
        let mut code = InteractiveText::new(
            ("chat-code-text", id),
            StyledText::new(text).with_highlights(painted),
        );
        if !block.parts.is_empty() {
            let parts: Rc<Vec<BlockPart>> = Rc::new(block.parts.clone());
            let weak = cx.entity().downgrade();
            code = code.on_hover(move |index, event, _, cx| {
                let part = index.and_then(|index| view::part_at(&parts, index));
                let _ = weak.update(cx, |workspace, cx| {
                    workspace.code_hovered(step, kind, part, event.position, cx)
                });
            });
        }
        // Shown exactly as given, one line per line, scrolling sideways when it is wide.
        let panel = panel_code(div().id(("chat-code", id)), cx)
            .font_family(face.family.clone())
            .font_features(face.features.clone())
            .text_size(px(face.size))
            .line_height(px(face.line_height()))
            .p_2()
            .rounded(px(6.))
            .h_flex()
            .overflow_x_scroll()
            .child(
                div()
                    .flex_none()
                    .w_auto()
                    .min_w_full()
                    .whitespace_nowrap()
                    .child(code),
            );
        if block.parts.is_empty() {
            return panel.into_any_element();
        }
        // The text element does not report the pointer leaving it, and after a scroll the
        // code under a resting pointer is different: both hide the explanation.
        div()
            .id(("chat-code-hover", id))
            .min_w_0()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.code_hovered(step, kind, None, Point::default(), cx);
                }
            }))
            .on_scroll(cx.listener(move |this, _, _, cx| {
                this.code_hovered(step, kind, None, Point::default(), cx);
            }))
            .child(panel)
            .into_any_element()
    }

    /// The pointer moved over `part` of a block of the step card, or off every part of it.
    pub(super) fn code_hovered(
        &mut self,
        step: usize,
        block: BlockKind,
        part: Option<usize>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(next) = hover_after(
            self.chat.hover.as_ref(),
            step,
            block,
            part,
            f32::from(position.x),
            f32::from(position.y),
        ) {
            self.chat.hover = next;
            cx.notify();
        }
    }

    /// The explanation of the part under the pointer, in a small card above the pointer.
    /// The card takes no mouse events, so the pointer keeps reaching the code under it.
    pub(super) fn code_hover_card(
        &self,
        card: &StepCard,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let hover = self
            .chat
            .hover
            .as_ref()
            .filter(|hover| hover.step == card.index)?;
        let guidance = card.guidance.as_ref()?;
        let blocks = view::step_blocks(guidance, &card.language);
        let part = blocks
            .blocks
            .iter()
            .find(|block| block.kind == hover.block)?
            .parts
            .get(hover.part)?;
        let (explanation, code) = view::inline_code(&part.explanation);
        let (chat, face) = (&self.look.fonts.chat, &self.look.fonts.chat_code);
        let p = palette(cx.theme().mode);
        let code_style = HighlightStyle {
            background_color: Some(p.hover()),
            ..Default::default()
        };
        Some(
            deferred(
                anchored()
                    .position(point(px(hover.x), px(hover.y) - px(14.)))
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .max_w(px(340.))
                            .p_2()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(p.border)
                            .bg(p.surface)
                            .shadow_md()
                            .text_size(px(chat.size))
                            .line_height(relative(1.4))
                            .text_color(p.foreground)
                            .child(
                                div()
                                    .font_family(face.family.clone())
                                    .font_features(face.features.clone())
                                    .text_size(px(face.size))
                                    .text_color(p.secondary_foreground)
                                    .child(view::part_label(&part.part)),
                            )
                            .child(StyledText::new(explanation).with_highlights(
                                code.into_iter().map(|range| (range, code_style)),
                            )),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Named imports: the parent's glob brings in gpui's `test` macro.
    use super::{cached_syntax_runs, painted_runs, register_fence_aliases, syntax_runs};
    use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry};
    use gpui_kit::{HighlightStyle, Hsla, SharedString, UnderlineStyle, px, rgb};
    use std::ops::Range;
    use std::rc::Rc;
    use tandem_claude::view;

    const RUST: &str = "fn main() { let x = 1; }";

    fn colours(runs: &[(Range<usize>, HighlightStyle)]) -> Vec<Option<Hsla>> {
        runs.iter().map(|(_, style)| style.color).collect()
    }

    #[test]
    fn every_supported_language_gets_colours() {
        let theme = HighlightTheme::default_dark();
        let rows = [
            ("rust", RUST),
            ("go", "package main\n\nfunc main() {}\n"),
            ("typescript", "const x: number = 1;"),
            ("javascript", "function f() { return 1; }"),
            ("python", "def f():\n    return 1\n"),
            ("java", "class A { int x = 1; }"),
            ("bash", "cargo test --quiet && echo \"done\""),
            ("json", "{\"a\": 1}"),
            ("toml", "[package]\nname = \"x\"\n"),
        ];
        for (grammar, sample) in rows {
            let runs = syntax_runs(sample, grammar, &theme);
            assert!(!runs.is_empty(), "{grammar}: no runs");
            assert!(
                runs.iter().any(|(_, style)| style.color.is_some()),
                "{grammar}: no run has a colour"
            );
            for pair in runs.windows(2) {
                assert!(
                    pair[0].0.end <= pair[1].0.start,
                    "{grammar}: {:?} and {:?} are out of order or overlap",
                    pair[0].0,
                    pair[1].0
                );
            }
            for (range, _) in &runs {
                assert!(
                    range.end <= sample.len(),
                    "{grammar}: {range:?} ends past the code"
                );
                assert!(
                    sample.is_char_boundary(range.start) && sample.is_char_boundary(range.end),
                    "{grammar}: {range:?} cuts a character"
                );
            }
        }
    }

    /// The runs of `file` inside `range`, cut to it and counted from its start.
    fn runs_within(
        file: &[(Range<usize>, HighlightStyle)],
        range: Range<usize>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        file.iter()
            .filter(|(run, _)| run.start < range.end && range.start < run.end)
            .map(|(run, style)| {
                let start = run.start.max(range.start) - range.start;
                let end = run.end.min(range.end) - range.start;
                (start..end, *style)
            })
            .collect()
    }

    #[test]
    fn a_snippet_is_coloured_as_in_its_file() {
        let theme = HighlightTheme::default_dark();
        // (grammar, file, snippet): each snippet is only valid inside its enclosing item.
        let rows = [
            (
                "rust",
                "pub struct Args {\n    #[arg(long)]\n    json: bool,\n}\n",
                "    #[arg(long)]\n    json: bool,",
            ),
            (
                "rust",
                "pub struct Args {\n    pub name: String,\n}\n",
                "    pub name: String,",
            ),
            (
                "rust",
                "pub enum Mode {\n    Plan,\n    Build(u8),\n}\n",
                "    Build(u8),",
            ),
            (
                "rust",
                "fn f(o: Option<i32>) -> i32 {\n    match o {\n        Some(x) => x + 1,\n        None => 0,\n    }\n}\n",
                "        Some(x) => x + 1,",
            ),
            (
                "rust",
                "fn args() -> Args {\n    Args {\n        name: \"x\".into(),\n    }\n}\n",
                "        name: \"x\".into(),",
            ),
            (
                "go",
                "package main\n\ntype Args struct {\n    Name string\n    JSON bool\n}\n",
                "    JSON bool",
            ),
            (
                "go",
                "package main\n\ntype Closer interface {\n    Close() error\n}\n",
                "    Close() error",
            ),
            (
                "go",
                "package main\n\nfunc main() {\n    switch {\n    case done:\n        return\n    }\n}\n",
                "    case done:\n        return",
            ),
            (
                "typescript",
                "interface Args {\n  name?: string;\n}\n",
                "  name?: string;",
            ),
            (
                "typescript",
                "class Counter {\n  private count: number = 0;\n}\n",
                "  private count: number = 0;",
            ),
            (
                "javascript",
                "const args = {\n  json: true,\n};\n",
                "  json: true,",
            ),
            (
                "java",
                "class Args {\n    int size(int n) {\n        switch (n) {\n        case 1:\n            return 0;\n        }\n        return n;\n    }\n}\n",
                "        case 1:\n            return 0;",
            ),
            (
                "java",
                "enum Mode {\n    PLAN(\"plan\"),\n    BUILD(\"build\");\n    Mode(String name) {}\n}\n",
                "    PLAN(\"plan\"),",
            ),
            (
                "python",
                "def size(n):\n    match n:\n        case 1:\n            return 0\n",
                "        case 1:\n            return 0",
            ),
            (
                "json",
                "{\n  \"name\": \"x\",\n  \"version\": \"1.0.0\"\n}\n",
                "  \"name\": \"x\",",
            ),
        ];
        for (grammar, file, snippet) in rows {
            let start = file.find(snippet).expect("the snippet is in the file");
            let expected = runs_within(
                &syntax_runs(file, grammar, &theme),
                start..start + snippet.len(),
            );
            assert!(
                expected.iter().any(|(_, style)| style.color.is_some()),
                "{grammar}: {snippet:?} has no colour in its file"
            );
            assert_eq!(
                syntax_runs(snippet, grammar, &theme),
                expected,
                "{grammar}: {snippet:?}"
            );
        }
    }

    #[test]
    fn colours_follow_the_theme() {
        let light = syntax_runs(RUST, "rust", &HighlightTheme::default_light());
        let dark = syntax_runs(RUST, "rust", &HighlightTheme::default_dark());
        assert_ne!(colours(&light), colours(&dark));
    }

    #[test]
    fn unknown_grammars_and_empty_code_have_no_runs() {
        let theme = HighlightTheme::default_dark();
        assert!(syntax_runs("x", "no-such-grammar", &theme).is_empty());
        assert!(syntax_runs("", "rust", &theme).is_empty());
    }

    #[test]
    fn fence_tags_resolve_to_a_grammar() {
        register_fence_aliases();
        register_fence_aliases();
        let tags = [
            "rust",
            "rs",
            "go",
            "golang",
            "typescript",
            "ts",
            "mts",
            "cts",
            "javascript",
            "js",
            "jsx",
            "mjs",
            "cjs",
            "node",
            "python",
            "py",
            "pyi",
            "python3",
            "java",
            "bash",
            "sh",
            "shell",
            "zsh",
            "console",
            "shellscript",
            "terminal",
            "json",
            "jsonc",
            "toml",
            "markdown",
            "md",
            "diff",
            "patch",
        ];
        let registry = LanguageRegistry::singleton();
        for tag in tags {
            let config = registry
                .language(tag)
                .unwrap_or_else(|| panic!("{tag}: not registered"));
            assert!(config.has_grammar(), "{tag}: no grammar");
            assert_eq!(
                config.name.as_ref(),
                view::grammar_for_language(tag).unwrap(),
                "{tag}"
            );
        }
    }

    /// The fields of a painted run that `painted_runs` sets.
    type Painted = (
        Range<usize>,
        Option<Hsla>,
        Option<Hsla>,
        Option<UnderlineStyle>,
    );

    fn painted(runs: Vec<(Range<usize>, HighlightStyle)>) -> Vec<Painted> {
        runs.into_iter()
            .map(|(range, style)| (range, style.color, style.background_color, style.underline))
            .collect()
    }

    #[test]
    // A part found once has a list of one span, so `&[2..6]` is the intended value.
    #[allow(clippy::single_range_in_vec_init)]
    fn painted_runs_splits_colours_at_the_hovered_part() {
        let red: Hsla = rgb(0xFF0000).into();
        let background: Hsla = rgb(0x0000FF).into();
        let underline: Hsla = rgb(0x00FF00).into();
        let syntax = [(
            0..4,
            HighlightStyle {
                color: Some(red),
                ..Default::default()
            },
        )];

        let hovered = painted_runs(&syntax, &[], underline, &[2..6], background);
        assert_eq!(
            painted(hovered),
            [
                (0..2, Some(red), None, None),
                (2..4, Some(red), Some(background), None),
                (4..6, None, Some(background), None),
            ]
        );

        let marked = painted_runs(&syntax, &[1..3], underline, &[], background);
        let line = UnderlineStyle {
            thickness: px(1.),
            color: Some(underline),
            wavy: false,
        };
        assert_eq!(
            painted(marked),
            [
                (0..1, Some(red), None, None),
                (1..3, Some(red), None, Some(line)),
                (3..4, Some(red), None, None),
            ]
        );

        assert!(painted_runs(&[], &[], underline, &[], background).is_empty());
    }

    #[test]
    fn runs_are_cached_per_text_grammar_and_theme() {
        let dark = HighlightTheme::default_dark();
        let light = HighlightTheme::default_light();
        let text: SharedString = RUST.into();
        let first = cached_syntax_runs(&text, Some("rust"), &dark);
        let again = cached_syntax_runs(&text, Some("rust"), &dark);
        assert!(Rc::ptr_eq(&first, &again));
        let other_theme = cached_syntax_runs(&text, Some("rust"), &light);
        assert!(!Rc::ptr_eq(&first, &other_theme));
        let other_text: SharedString = "fn other() {}".into();
        let other_text = cached_syntax_runs(&other_text, Some("rust"), &dark);
        assert!(!Rc::ptr_eq(&first, &other_text));
        assert!(cached_syntax_runs(&text, None, &dark).is_empty());
    }
}
