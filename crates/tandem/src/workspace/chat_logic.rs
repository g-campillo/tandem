//! Pure decisions behind the workspace's chat and bottom panels, kept free of GPUI so they
//! are tested directly.

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
}
