//! Pure decisions behind the window's chrome: the activity bar, the tabs, file icons and
//! the status bar. Kept free of GPUI so they are tested directly.
use super::chat_logic::BottomTab;
use lsp_types::DiagnosticSeverity;
use std::path::Path;

/// The label of an icon-only control with its shortcut, as its tooltip shows it:
/// `Explorer  ⇧⌘E`, or the label alone when there is no shortcut.
pub fn tooltip(label: &str, shortcut: &str) -> String {
    if shortcut.is_empty() {
        label.to_owned()
    } else {
        format!("{label}  {shortcut}")
    }
}

/// The buttons of the activity bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Explorer,
    Search,
    Chat,
    Problems,
    Terminal,
    Settings,
}

impl Activity {
    /// The buttons at the top of the bar, in order. Settings sits at the bottom.
    pub const TOP: [Activity; 5] = [
        Activity::Explorer,
        Activity::Search,
        Activity::Chat,
        Activity::Problems,
        Activity::Terminal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Activity::Explorer => "Explorer",
            Activity::Search => "Search",
            Activity::Chat => "Chat",
            Activity::Problems => "Problems",
            Activity::Terminal => "Terminal",
            Activity::Settings => "Settings",
        }
    }

    /// The title of the registry command whose shortcut the button's tooltip shows.
    pub fn command(self) -> &'static str {
        match self {
            Activity::Explorer => "Show Explorer",
            Activity::Search => "Search Project",
            Activity::Chat => "Toggle Chat",
            Activity::Problems => "Show Problems",
            Activity::Terminal => "Toggle Terminal",
            Activity::Settings => "Settings…",
        }
    }
}

/// What the window is showing, as far as the activity bar cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The file explorer pane is on screen.
    pub explorer: bool,
    /// The explorer pane is on its project search, not on the file tree.
    pub searching: bool,
    pub chat: bool,
    pub bottom: Option<BottomTab>,
    pub settings: bool,
}

/// Whether `activity` is drawn as active: what it opens is on screen.
pub fn activity_selected(activity: Activity, shown: Shown) -> bool {
    match activity {
        Activity::Explorer => shown.explorer && !shown.searching,
        Activity::Search => shown.explorer && shown.searching,
        Activity::Chat => shown.chat,
        Activity::Problems => shown.bottom == Some(BottomTab::Problems),
        Activity::Terminal => shown.bottom == Some(BottomTab::Terminal),
        Activity::Settings => shown.settings,
    }
}

/// The kinds of file icon Tandem draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileIcon {
    Code,
    Data,
    Config,
    Text,
    Script,
    Lock,
    Image,
    Plain,
}

/// The icon of the file at `path`, chosen by its name and extension.
pub fn file_icon(path: &str) -> FileIcon {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_lowercase();
    if name.ends_with(".lock") || name.ends_with("-lock.json") || name.ends_with("-lock.yaml") {
        return FileIcon::Lock;
    }
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "rs" | "py" | "pyi" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "java" | "c"
        | "h" | "cc" | "cpp" | "hpp" | "swift" | "kt" | "rb" | "php" | "cs" | "html" | "css"
        | "scss" | "xml" | "sql" => FileIcon::Code,
        "json" | "jsonc" | "jsonl" | "csv" => FileIcon::Data,
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" | "plist" => FileIcon::Config,
        "md" | "markdown" | "txt" | "rst" => FileIcon::Text,
        "sh" | "bash" | "zsh" | "fish" => FileIcon::Script,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "icns" => FileIcon::Image,
        _ => FileIcon::Plain,
    }
}

/// The label of each open tab: the file's name, and for files that share a name also the
/// folder each is in (`mod.rs — hover`), so that every tab can be told apart.
pub fn tab_labels(paths: &[&str]) -> Vec<String> {
    let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    paths
        .iter()
        .map(|path| {
            let own = name(path);
            let shared = paths.iter().filter(|other| name(other) == own).count() > 1;
            match path.rsplit_once('/') {
                Some((folder, _)) if shared => format!("{own} — {}", name(folder)),
                _ => own,
            }
        })
        .collect()
}

/// A tab's accessible name: its path, and its state when it is saving or has unsaved
/// changes.
pub fn tab_name(path: &str, dirty: bool, saving: bool) -> String {
    match (saving, dirty) {
        (true, _) => format!("{path}, saving"),
        (false, true) => format!("{path}, unsaved changes"),
        (false, false) => path.to_owned(),
    }
}

/// The status bar's cursor position, from the editor's 0-based line and column.
pub fn cursor_label(line: u32, column: u32) -> String {
    format!("Ln {}, Col {}", u64::from(line) + 1, u64::from(column) + 1)
}

/// The status bar's name for a language id of `Document::language`.
pub fn language_label(language: &str) -> &'static str {
    match language {
        "rust" => "Rust",
        "python" => "Python",
        "json" => "JSON",
        "toml" => "TOML",
        "markdown" => "Markdown",
        _ => "Plain Text",
    }
}

/// How many errors and warnings the language tools report for the open file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProblemCounts {
    pub errors: usize,
    pub warnings: usize,
}

/// Counts diagnostics by severity. A diagnostic without a severity counts as an error;
/// information and hints are not counted.
pub fn problem_counts(
    severities: impl IntoIterator<Item = Option<DiagnosticSeverity>>,
) -> ProblemCounts {
    let mut counts = ProblemCounts::default();
    for severity in severities {
        match severity {
            None | Some(DiagnosticSeverity::ERROR) => counts.errors += 1,
            Some(DiagnosticSeverity::WARNING) => counts.warnings += 1,
            Some(_) => {}
        }
    }
    counts
}

/// The counts in words, for the status bar's accessible name: `2 errors, 1 warning`.
pub fn problems_label(counts: ProblemCounts) -> String {
    let plural = |count: usize, word: &str| {
        if count == 1 {
            format!("1 {word}")
        } else {
            format!("{count} {word}s")
        }
    };
    format!(
        "{}, {}",
        plural(counts.errors, "error"),
        plural(counts.warnings, "warning")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTHING: Shown = Shown {
        explorer: false,
        searching: false,
        chat: false,
        bottom: None,
        settings: false,
    };

    #[test]
    fn tooltip_joins_the_label_and_the_shortcut() {
        assert_eq!(tooltip("Explorer", "⇧⌘E"), "Explorer  ⇧⌘E");
        assert_eq!(tooltip("Refresh Files", ""), "Refresh Files");
    }

    #[test]
    fn nothing_is_selected_when_nothing_is_shown() {
        for activity in Activity::TOP.into_iter().chain([Activity::Settings]) {
            assert!(!activity_selected(activity, NOTHING), "{activity:?}");
        }
    }

    #[test]
    fn explorer_and_search_share_the_pane_and_only_one_is_selected() {
        let files = Shown {
            explorer: true,
            ..NOTHING
        };
        assert!(activity_selected(Activity::Explorer, files));
        assert!(!activity_selected(Activity::Search, files));
        let search = Shown {
            searching: true,
            ..files
        };
        assert!(!activity_selected(Activity::Explorer, search));
        assert!(activity_selected(Activity::Search, search));
        // A hidden pane selects neither, whatever it would show.
        let hidden = Shown {
            explorer: false,
            ..search
        };
        assert!(!activity_selected(Activity::Explorer, hidden));
        assert!(!activity_selected(Activity::Search, hidden));
    }

    #[test]
    fn chat_bottom_tabs_and_settings_follow_what_is_shown() {
        let shown = Shown {
            chat: true,
            bottom: Some(BottomTab::Terminal),
            settings: true,
            ..NOTHING
        };
        assert!(activity_selected(Activity::Chat, shown));
        assert!(activity_selected(Activity::Terminal, shown));
        assert!(!activity_selected(Activity::Problems, shown));
        assert!(activity_selected(Activity::Settings, shown));
        let problems = Shown {
            bottom: Some(BottomTab::Problems),
            ..NOTHING
        };
        assert!(activity_selected(Activity::Problems, problems));
        assert!(!activity_selected(Activity::Terminal, problems));
    }

    #[test]
    fn every_activity_has_a_label_and_a_command() {
        for activity in Activity::TOP.into_iter().chain([Activity::Settings]) {
            assert!(!activity.label().is_empty());
            assert!(!activity.command().is_empty());
        }
        assert_eq!(Activity::TOP.len(), 5);
        assert!(!Activity::TOP.contains(&Activity::Settings));
    }

    #[test]
    fn file_icon_follows_the_extension() {
        let rows = [
            ("src/main.rs", FileIcon::Code),
            ("app/View.TSX", FileIcon::Code),
            ("package.json", FileIcon::Data),
            ("Cargo.toml", FileIcon::Config),
            (".env", FileIcon::Config),
            ("README.md", FileIcon::Text),
            ("scripts/build.sh", FileIcon::Script),
            ("logo.svg", FileIcon::Image),
            ("Makefile", FileIcon::Plain),
            ("notes", FileIcon::Plain),
            ("", FileIcon::Plain),
        ];
        for (path, icon) in rows {
            assert_eq!(file_icon(path), icon, "{path}");
        }
    }

    #[test]
    fn file_icon_knows_lock_files_by_name() {
        for path in ["Cargo.lock", "web/package-lock.json", "pnpm-lock.yaml"] {
            assert_eq!(file_icon(path), FileIcon::Lock, "{path}");
        }
    }

    #[test]
    fn tab_labels_are_file_names() {
        assert_eq!(
            tab_labels(&["src/main.rs", "Cargo.toml", "docs/tandem/ux.md"]),
            ["main.rs", "Cargo.toml", "ux.md"]
        );
        assert!(tab_labels(&[]).is_empty());
    }

    #[test]
    fn tab_labels_add_the_folder_to_files_that_share_a_name() {
        assert_eq!(
            tab_labels(&["src/hover/mod.rs", "src/edit/mod.rs", "mod.rs", "src/a.rs"]),
            ["mod.rs — hover", "mod.rs — edit", "mod.rs", "a.rs"]
        );
    }

    #[test]
    fn tab_name_adds_the_state_of_the_file() {
        assert_eq!(tab_name("src/a.rs", false, false), "src/a.rs");
        assert_eq!(
            tab_name("src/a.rs", true, false),
            "src/a.rs, unsaved changes"
        );
        assert_eq!(tab_name("src/a.rs", true, true), "src/a.rs, saving");
        assert_eq!(tab_name("src/a.rs", false, true), "src/a.rs, saving");
    }

    #[test]
    fn cursor_label_counts_from_one() {
        assert_eq!(cursor_label(0, 0), "Ln 1, Col 1");
        assert_eq!(cursor_label(41, 7), "Ln 42, Col 8");
        assert_eq!(
            cursor_label(u32::MAX, u32::MAX),
            "Ln 4294967296, Col 4294967296"
        );
    }

    #[test]
    fn language_label_names_every_language_tandem_knows() {
        for (id, label) in [
            ("rust", "Rust"),
            ("python", "Python"),
            ("json", "JSON"),
            ("toml", "TOML"),
            ("markdown", "Markdown"),
            ("plain_text", "Plain Text"),
            ("anything else", "Plain Text"),
        ] {
            assert_eq!(language_label(id), label);
        }
    }

    #[test]
    fn problem_counts_counts_errors_and_warnings_only() {
        let counts = problem_counts([
            Some(DiagnosticSeverity::ERROR),
            Some(DiagnosticSeverity::WARNING),
            Some(DiagnosticSeverity::WARNING),
            Some(DiagnosticSeverity::INFORMATION),
            Some(DiagnosticSeverity::HINT),
            None,
        ]);
        assert_eq!(
            counts,
            ProblemCounts {
                errors: 2,
                warnings: 2
            }
        );
        assert_eq!(problem_counts([]), ProblemCounts::default());
    }

    #[test]
    fn problems_label_uses_singular_and_plural() {
        let label = |errors, warnings| problems_label(ProblemCounts { errors, warnings });
        assert_eq!(label(0, 0), "0 errors, 0 warnings");
        assert_eq!(label(1, 1), "1 error, 1 warning");
        assert_eq!(label(2, 12), "2 errors, 12 warnings");
    }
}
