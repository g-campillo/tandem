//! Pure decisions behind the workspace's chat and bottom panels, kept free of GPUI so they
//! are tested directly.

use tandem_claude::view::BlockKind;
use tandem_core::chat::{CheckResult, Finding, HELP_LANGUAGES};

/// The tabs of the panel below the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomTab {
    Terminal,
    Problems,
}

/// The bottom panel after a click on `clicked`: clicking the visible tab hides the panel,
/// clicking any other tab (or any tab while the panel is hidden) shows that tab.
pub fn next_bottom(current: Option<BottomTab>, clicked: BottomTab) -> Option<BottomTab> {
    if current == Some(clicked) {
        None
    } else {
        Some(clicked)
    }
}

/// The status bar's line count for the open file, counted the way Github counts: a final
/// trailing newline does not start another line, so an empty file has 0 lines.
pub fn line_count_label(text: &str) -> String {
    let count = text.lines().count();

    if count == 1 {
        "1 line".to_owned()
    } else {
        format!("{count} lines")
    }
}

/// The rows of Settings' "Help level per language", as (language id, label): every
/// language in `HELP_LANGUAGES`, then each other language that has a saved level.
pub fn help_languages<'a>(saved: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = HELP_LANGUAGES
        .iter()
        .map(|(id, label)| ((*id).to_owned(), (*label).to_owned()))
        .collect();
    for language in saved {
        if !language.is_empty() && !rows.iter().any(|(id, _)| id == language) {
            let mut letters = language.chars();
            let label = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default();
            rows.push((language.to_owned(), label));
        }
    }
    rows
}

/// The title of a check card: Claude's verdict on one step or on the whole task.
pub fn check_title(result: &CheckResult) -> String {
    match (result.step, result.passed) {
        (Some(step), true) => format!("Step {} looks right", step + 1),
        (Some(step), false) => format!("Step {} needs changes", step + 1),
        (None, true) => "The task meets its \"done when\" criteria".into(),
        (None, false) => "Not done yet".into(),
    }
}

/// The first line of a finding on a check card: `<path>:<line>  <problem>`, leaving out
/// the parts the finding does not have.
pub fn finding_label(finding: &Finding) -> String {
    match (&finding.path, finding.line) {
        (Some(path), Some(line)) => format!("{path}:{line}  {}", finding.problem),
        (Some(path), None) => format!("{path}  {}", finding.problem),
        (None, _) => finding.problem.clone(),
    }
}

/// The explained part of the step card's code that is under the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeHover {
    /// The step's index in the plan.
    pub step: usize,
    pub block: BlockKind,
    /// Index into the block's `parts`.
    pub part: usize,
    /// Where the pointer was, in window pixels, when it reached the part.
    pub x: f32,
    pub y: f32,
}

/// The hover after the pointer moved over `part` of the block (`step`, `block`), or off
/// every part of it (`None`). Returns `None` when nothing changes: the same part keeps
/// its first position, and leaving a block that is not the hovered one clears nothing.
pub fn hover_after(
    current: Option<&CodeHover>,
    step: usize,
    block: BlockKind,
    part: Option<usize>,
    x: f32,
    y: f32,
) -> Option<Option<CodeHover>> {
    let same_block = current.is_some_and(|hover| hover.step == step && hover.block == block);
    match part {
        Some(part) if same_block && current.is_some_and(|hover| hover.part == part) => None,
        Some(part) => Some(Some(CodeHover {
            step,
            block,
            part,
            x,
            y,
        })),
        None if same_block => Some(None),
        None => None,
    }
}

/// The 1-based number of the first line of `text` that contains `anchor` (trimmed), so
/// "Open file" can put the cursor where a step's code goes. `None` for an empty anchor or
/// when no line contains it.
pub fn anchor_line(text: &str, anchor: &str) -> Option<usize> {
    let anchor = anchor.trim();
    if anchor.is_empty() {
        return None;
    }
    text.lines()
        .position(|line| line.contains(anchor))
        .map(|index| index + 1)
}

/// What a takeover needs to know about one open editor tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabFacts {
    pub path: String,
    pub dirty: bool,
    /// The buffer was never written to disk (`document.base_hash` is `None`).
    pub never_saved: bool,
    pub empty: bool,
    /// The file changed on disk under the buffer.
    pub conflict: bool,
    pub read_only: bool,
    /// A save of this tab is still running.
    pub saving: bool,
}

/// How the open buffers get onto disk before Claude takes over, since Claude edits the
/// files there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavePlan {
    /// Nothing is unsaved: Claude can start.
    Ready,
    /// Claude cannot start yet; the message says why.
    Blocked(String),
    /// Save the tabs in `save` and close the ones in `close`, then start Claude.
    Save {
        save: Vec<String>,
        close: Vec<String>,
    },
}

/// The save plan for the open tabs. A running save, or an unsaved buffer that cannot be
/// written (its file changed on disk, or is read-only), blocks the takeover. A new buffer
/// that is still empty is closed instead of saved, because saving it would create an empty
/// file; every other unsaved buffer is saved.
pub fn takeover_save_plan(tabs: &[TabFacts]) -> SavePlan {
    if tabs.iter().any(|tab| tab.saving) {
        return SavePlan::Blocked("Wait for the current save to finish.".into());
    }
    let dirty = || tabs.iter().filter(|tab| tab.dirty);
    if let Some(tab) = dirty().find(|tab| tab.conflict) {
        return SavePlan::Blocked(format!(
            "{} changed on disk. Resolve that before Claude takes over.",
            tab.path
        ));
    }
    if let Some(tab) = dirty().find(|tab| tab.read_only) {
        return SavePlan::Blocked(format!(
            "{} is read-only, so your changes cannot be saved before the takeover.",
            tab.path
        ));
    }
    if dirty().next().is_none() {
        return SavePlan::Ready;
    }
    let (close, save): (Vec<&TabFacts>, Vec<&TabFacts>) =
        dirty().partition(|tab| tab.never_saved && tab.empty);
    let paths = |tabs: Vec<&TabFacts>| tabs.into_iter().map(|tab| tab.path.clone()).collect();
    SavePlan::Save {
        save: paths(save),
        close: paths(close),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_bottom_hides_the_visible_tab_and_shows_any_other() {
        use BottomTab::{Problems, Terminal};
        let cases = [
            (None, Terminal, Some(Terminal)),
            (None, Problems, Some(Problems)),
            (Some(Terminal), Terminal, None),
            (Some(Problems), Problems, None),
            (Some(Terminal), Problems, Some(Problems)),
            (Some(Problems), Terminal, Some(Terminal)),
        ];
        for (current, clicked, expected) in cases {
            assert_eq!(
                next_bottom(current, clicked),
                expected,
                "{current:?} clicked {clicked:?}"
            );
        }
    }

    #[test]
    fn line_count_label_counts_lines_like_github() {
        let cases = [
            ("", "0 lines"),
            ("a", "1 line"),
            ("a\n", "1 line"),
            ("a\nb\n", "2 lines"),
            ("a\r\nb", "2 lines"),
        ];

        for (text, expected) in cases {
            assert_eq!(line_count_label(text), expected, "{text:?}");
        }
    }

    #[test]
    fn help_languages_lists_the_built_in_languages_then_other_saved_ones() {
        let rows = help_languages(["rust", "kotlin", "python", "elixir"]);
        let ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "rust",
                "go",
                "typescript",
                "python",
                "java",
                "javascript",
                "shell",
                "kotlin",
                "elixir"
            ]
        );
        assert_eq!(rows[0].1, "Rust");
        assert_eq!(rows[2].1, "TypeScript");
        assert_eq!(rows[7].1, "Kotlin");
        assert_eq!(help_languages([]).len(), 7);
    }

    #[test]
    fn hover_after_tracks_the_part_under_the_pointer() {
        use BlockKind::{Code, Command};
        let first = CodeHover {
            step: 0,
            block: Code,
            part: 2,
            x: 10.,
            y: 20.,
        };
        assert_eq!(
            hover_after(None, 0, Code, Some(2), 10., 20.),
            Some(Some(first.clone()))
        );
        // The same part keeps the position where the pointer reached it.
        assert_eq!(hover_after(Some(&first), 0, Code, Some(2), 50., 60.), None);
        assert_eq!(
            hover_after(Some(&first), 0, Code, Some(3), 50., 60.),
            Some(Some(CodeHover {
                step: 0,
                block: Code,
                part: 3,
                x: 50.,
                y: 60.,
            }))
        );
        assert_eq!(
            hover_after(Some(&first), 0, Code, None, 50., 60.),
            Some(None)
        );
        // Leaving another block, or another step's block, clears nothing.
        assert_eq!(hover_after(Some(&first), 0, Command, None, 50., 60.), None);
        assert_eq!(hover_after(Some(&first), 1, Code, None, 50., 60.), None);
        assert_eq!(hover_after(None, 0, Code, None, 50., 60.), None);
        assert_eq!(
            hover_after(Some(&first), 0, Command, Some(0), 1., 2.),
            Some(Some(CodeHover {
                step: 0,
                block: Command,
                part: 0,
                x: 1.,
                y: 2.,
            }))
        );
    }

    #[test]
    fn check_title_names_the_step_or_the_task_and_the_verdict() {
        let result = |step, passed| CheckResult {
            step,
            passed,
            explanation: String::new(),
            findings: Vec::new(),
        };
        assert_eq!(check_title(&result(Some(0), true)), "Step 1 looks right");
        assert_eq!(check_title(&result(Some(2), false)), "Step 3 needs changes");
        assert_eq!(
            check_title(&result(None, true)),
            "The task meets its \"done when\" criteria"
        );
        assert_eq!(check_title(&result(None, false)), "Not done yet");
    }

    fn finding(path: Option<&str>, line: Option<u32>) -> Finding {
        Finding {
            path: path.map(str::to_owned),
            line,
            problem: "json is never read".into(),
            why: "unused".into(),
            hint: None,
        }
    }

    #[test]
    fn finding_label_names_the_place_then_the_problem() {
        assert_eq!(
            finding_label(&finding(Some("src/cli.rs"), Some(7))),
            "src/cli.rs:7  json is never read"
        );
        assert_eq!(
            finding_label(&finding(Some("src/cli.rs"), None)),
            "src/cli.rs  json is never read"
        );
        assert_eq!(finding_label(&finding(None, Some(7))), "json is never read");
    }

    const CLI: &str =
        "use clap::Parser;\n\n#[derive(Parser)]\npub struct Args {\n    pub name: String,\n}\n";

    #[test]
    fn anchor_line_finds_a_line_in_the_middle_of_a_file() {
        assert_eq!(anchor_line(CLI, "pub struct Args {"), Some(4));
    }

    #[test]
    fn anchor_line_takes_the_first_of_several_matches() {
        let text = "fn a() {}\nlet x = 1;\nlet x = 2;\n";
        assert_eq!(anchor_line(text, "let x"), Some(2));
    }

    #[test]
    fn anchor_line_ignores_whitespace_around_the_anchor() {
        assert_eq!(anchor_line(CLI, "   pub name: String,\t\n"), Some(5));
    }

    #[test]
    fn anchor_line_is_none_for_an_empty_anchor() {
        assert_eq!(anchor_line(CLI, ""), None);
        assert_eq!(anchor_line(CLI, "  \n "), None);
    }

    #[test]
    fn anchor_line_is_none_without_a_match() {
        assert_eq!(anchor_line(CLI, "pub struct Options {"), None);
    }

    /// A clean tab of a file that exists on disk.
    fn tab(path: &str) -> TabFacts {
        TabFacts {
            path: path.into(),
            dirty: false,
            never_saved: false,
            empty: false,
            conflict: false,
            read_only: false,
            saving: false,
        }
    }

    fn dirty(path: &str) -> TabFacts {
        TabFacts {
            dirty: true,
            ..tab(path)
        }
    }

    #[test]
    fn takeover_save_plan_is_ready_without_dirty_tabs() {
        assert_eq!(takeover_save_plan(&[]), SavePlan::Ready);
        assert_eq!(
            takeover_save_plan(&[tab("src/cli.rs"), tab("src/main.rs")]),
            SavePlan::Ready
        );
    }

    #[test]
    fn takeover_save_plan_saves_a_dirty_saved_file() {
        assert_eq!(
            takeover_save_plan(&[tab("src/main.rs"), dirty("src/cli.rs")]),
            SavePlan::Save {
                save: vec!["src/cli.rs".into()],
                close: vec![],
            }
        );
    }

    #[test]
    fn takeover_save_plan_closes_a_never_saved_empty_tab() {
        let new_empty = TabFacts {
            never_saved: true,
            empty: true,
            ..dirty("tests/cli.rs")
        };
        assert_eq!(
            takeover_save_plan(&[dirty("src/cli.rs"), new_empty]),
            SavePlan::Save {
                save: vec!["src/cli.rs".into()],
                close: vec!["tests/cli.rs".into()],
            }
        );
    }

    #[test]
    fn takeover_save_plan_saves_a_never_saved_tab_with_text() {
        let new_with_text = TabFacts {
            never_saved: true,
            ..dirty("tests/cli.rs")
        };
        assert_eq!(
            takeover_save_plan(&[new_with_text]),
            SavePlan::Save {
                save: vec!["tests/cli.rs".into()],
                close: vec![],
            }
        );
    }

    #[test]
    fn takeover_save_plan_blocks_on_a_dirty_tab_with_a_conflict() {
        let conflict = TabFacts {
            conflict: true,
            ..dirty("src/cli.rs")
        };
        assert_eq!(
            takeover_save_plan(&[dirty("src/main.rs"), conflict]),
            SavePlan::Blocked(
                "src/cli.rs changed on disk. Resolve that before Claude takes over.".into()
            )
        );
    }

    #[test]
    fn takeover_save_plan_blocks_on_a_dirty_read_only_tab() {
        let read_only = TabFacts {
            read_only: true,
            ..dirty("src/cli.rs")
        };
        assert_eq!(
            takeover_save_plan(&[read_only]),
            SavePlan::Blocked(
                "src/cli.rs is read-only, so your changes cannot be saved before the takeover."
                    .into()
            )
        );
    }

    #[test]
    fn takeover_save_plan_blocks_while_a_tab_is_saving() {
        let saving = TabFacts {
            saving: true,
            ..tab("src/cli.rs")
        };
        assert_eq!(
            takeover_save_plan(&[dirty("src/main.rs"), saving]),
            SavePlan::Blocked("Wait for the current save to finish.".into())
        );
    }

    #[test]
    fn takeover_save_plan_ignores_conflicts_and_read_only_on_clean_tabs() {
        let conflict = TabFacts {
            conflict: true,
            ..tab("src/cli.rs")
        };
        let read_only = TabFacts {
            read_only: true,
            ..tab("src/main.rs")
        };
        assert_eq!(
            takeover_save_plan(&[conflict.clone(), read_only.clone()]),
            SavePlan::Ready
        );
        assert_eq!(
            takeover_save_plan(&[conflict, read_only, dirty("Cargo.toml")]),
            SavePlan::Save {
                save: vec!["Cargo.toml".into()],
                close: vec![],
            }
        );
    }
}
