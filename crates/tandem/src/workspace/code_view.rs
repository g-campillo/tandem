//! Code in the chat panel: syntax colours from the editor's highlight theme.
use super::*;
use crate::appearance::panel_code;
use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::component::{ActiveTheme, Rope, StyledExt};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Once;
use tandem_claude::view::{self, StepBlock};

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
pub(super) fn syntax_runs(
    code: &str,
    grammar: &str,
    theme: &HighlightTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let has_grammar = LanguageRegistry::singleton()
        .language(grammar)
        .is_some_and(|config| config.has_grammar());
    if code.is_empty() || !has_grammar {
        return Vec::new();
    }
    // A new highlighter each time: a reused one would need an exact edit of its old text.
    let rope = Rope::from_str(code);
    let mut highlighter = SyntaxHighlighter::new(grammar);
    highlighter.update(None, &rope, None);
    highlighter
        .styles(&(0..code.len()), theme)
        .into_iter()
        .filter(|(range, style)| !range.is_empty() && *style != HighlightStyle::default())
        .collect()
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
        let id = step * 4 + block.kind as usize;
        // Shown exactly as given, one line per line, scrolling sideways when it is wide.
        panel_code(div().id(("chat-code", id)), cx)
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
                    .child(InteractiveText::new(
                        ("chat-code-text", id),
                        StyledText::new(text).with_highlights(runs.iter().cloned()),
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Named imports: the parent's glob brings in gpui's `test` macro.
    use super::{cached_syntax_runs, register_fence_aliases, syntax_runs};
    use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry};
    use gpui_kit::{HighlightStyle, Hsla, SharedString};
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
