//! The command center in the title bar: one dialog that finds files by name (Quick Open,
//! ⌘P) and, when the query starts with `>`, lists the commands (the command palette, ⇧⌘P).
use super::*;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::command::{Command as Palette, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The most files Quick Open lists for one query.
pub const MAX_FILE_ROWS: usize = 50;

/// What choosing a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CenterEntry {
    /// Opens the file at this project-relative path.
    File(String),
    /// Runs the command at this index of `commands()`.
    Command(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterRow {
    pub entry: CenterEntry,
    /// The file's name, or the command's title.
    pub label: String,
    /// The file's folder, or the command's shortcut. May be empty.
    pub detail: String,
}

/// Whether `query` asks for commands instead of files.
pub fn is_command_query(query: &str) -> bool {
    query.trim_start().starts_with('>')
}

/// The rows the command center shows for `query`.
///
/// A query that starts with `>` lists the commands whose title or menu contains the rest
/// of the query, in registry order. Any other query lists at most `MAX_FILE_ROWS` files
/// whose path contains it: first the files whose name starts with it, then those whose
/// name contains it, then those with it elsewhere in their path, each group in the order
/// of `files`. Matching ignores case; an empty query matches everything.
pub fn center_rows(query: &str, files: &[String], specs: &[CommandSpec]) -> Vec<CenterRow> {
    if is_command_query(query) {
        let needle = query.trim_start()[1..].trim().to_lowercase();
        return specs
            .iter()
            .enumerate()
            .filter(|(_, spec)| {
                spec.title.to_lowercase().contains(&needle)
                    || spec.menu.title().to_lowercase().contains(&needle)
            })
            .map(|(index, spec)| CenterRow {
                entry: CenterEntry::Command(index),
                label: spec.title.to_owned(),
                detail: spec.shortcut.to_owned(),
            })
            .collect();
    }
    let needle = query.trim().to_lowercase();
    let mut ranked: Vec<(u8, &String)> = files
        .iter()
        .filter_map(|path| {
            let lower = path.to_lowercase();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            if name.starts_with(&needle) {
                Some((0, path))
            } else if name.contains(&needle) {
                Some((1, path))
            } else if lower.contains(&needle) {
                Some((2, path))
            } else {
                None
            }
        })
        .collect();
    // A stable sort keeps the order of `files` inside each group.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked
        .into_iter()
        .take(MAX_FILE_ROWS)
        .map(|(_, path)| {
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
            CenterRow {
                entry: CenterEntry::File(path.clone()),
                label: name.to_owned(),
                detail: folder.to_owned(),
            }
        })
        .collect()
}

impl Workspace {
    /// Opens the command center with `prefix` as its query: `""` finds files (⌘P), `">"`
    /// lists the commands (⇧⌘P). Closes it instead when a dialog is already open.
    pub(in crate::workspace) fn open_center(
        &mut self,
        prefix: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let state = self
            .commands
            .palette
            .get_or_insert_with(|| cx.new(|cx| CommandState::new(window, cx)))
            .clone();
        state.update(cx, |state, cx| state.set_query(prefix, window, cx));
        let weak = cx.entity().downgrade();
        let first = Cell::new(true);
        // What each row on screen does, for `on_confirm`, which is given a row's index.
        let shown: Rc<RefCell<Vec<CenterEntry>>> = Rc::default();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                let focus = state.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }
            // The dialog is drawn again whenever the query changes, so the rows always
            // answer the query on screen. The toolkit's own filter is off.
            let query = state.read(cx).query(cx);
            let rows = weak
                .upgrade()
                .map(|view| center_rows(&query, &view.read(cx).index.files, &commands()))
                .unwrap_or_default();
            *shown.borrow_mut() = rows.iter().map(|row| row.entry.clone()).collect();
            let items = rows.into_iter().map(|row| {
                let icon = match row.entry {
                    CenterEntry::File(_) => Lucide::File,
                    CenterEntry::Command(_) => Lucide::ChevronRight,
                };
                let (label, detail) = (row.label.clone(), row.detail);
                // No `.action(…)`: the toolkit would dispatch it while the search field
                // has focus. `run_command` dispatches it once the focus is back.
                CommandItem::new()
                    .label(row.label)
                    .icon(icon)
                    .child(move |_, cx| {
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_between()
                            .gap_4()
                            .child(label.clone())
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(detail.clone()),
                            )
                    })
            });
            let (weak, shown) = (weak.clone(), shown.clone());
            dialog.close_button(false).w(px(560.)).p_0().child(
                Palette::new(&state)
                    .bordered(false)
                    .filterable(false)
                    .placeholder("Search files by name · type > for commands")
                    .items(items)
                    .on_confirm(move |index, window, cx| {
                        let entry = shown.borrow().get(index.row).cloned();
                        window.close_dialog(cx);
                        let _ = weak.update(cx, |view, cx| match entry {
                            Some(CenterEntry::File(path)) => {
                                view.settings.open = false;
                                view.open_file(path, false, None, window, cx);
                            }
                            Some(CenterEntry::Command(index)) => {
                                view.run_command(index, window, cx)
                            }
                            None => {}
                        });
                    }),
            )
        });
    }

    /// Runs the registry command at `index`. `close_dialog` has already put the focus back
    /// where it was before the command center opened, and `dispatch_action` targets the
    /// focused element, so toolkit actions reach the editor and Tandem actions their
    /// app-level handler.
    fn run_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = commands().into_iter().nth(index) else {
            return;
        };
        if spec.scope == KeyScope::Editor {
            let editor = (self.diff.is_none() && !self.settings.open)
                .then(|| self.active.and_then(|index| self.tabs.get(index)))
                .flatten()
                .map(|tab| tab.editor.clone());
            let Some(editor) = editor else {
                self.message("Open a file to use this command.", false, cx);
                return;
            };
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
        window.dispatch_action(spec.action(), cx);
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::super::commands;
    use super::{CenterEntry, MAX_FILE_ROWS, center_rows, is_command_query};

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    fn labels(query: &str, paths: &[&str]) -> Vec<String> {
        center_rows(query, &files(paths), &commands())
            .into_iter()
            .map(|row| row.label)
            .collect()
    }

    #[test]
    fn a_query_that_starts_with_an_angle_bracket_asks_for_commands() {
        assert!(is_command_query(">"));
        assert!(is_command_query(">save"));
        assert!(is_command_query("  > save"));
        assert!(!is_command_query(""));
        assert!(!is_command_query("save >"));
    }

    #[test]
    fn an_empty_query_lists_the_first_files_in_index_order() {
        let rows = center_rows("", &files(&["b.rs", "src/a.rs"]), &commands());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].entry, CenterEntry::File("b.rs".into()));
        assert_eq!(
            (rows[0].label.as_str(), rows[0].detail.as_str()),
            ("b.rs", "")
        );
        assert_eq!(
            (rows[1].label.as_str(), rows[1].detail.as_str()),
            ("a.rs", "src")
        );
    }

    #[test]
    fn files_are_ranked_by_where_the_query_matches() {
        let paths = [
            "view/readme.md",
            "src/preview.rs",
            "src/view.rs",
            "src/other.rs",
            "docs/View.md",
        ];
        assert_eq!(
            labels("view", &paths),
            ["view.rs", "View.md", "preview.rs", "readme.md"]
        );
        assert_eq!(labels(" VIEW ", &paths), labels("view", &paths));
        assert!(labels("nothing", &paths).is_empty());
    }

    #[test]
    fn at_most_fifty_files_are_listed() {
        let many: Vec<String> = (0..200)
            .map(|index| format!("src/file{index}.rs"))
            .collect();
        let rows = center_rows("file", &many, &commands());
        assert_eq!(rows.len(), MAX_FILE_ROWS);
        assert_eq!(rows[0].entry, CenterEntry::File("src/file0.rs".into()));
    }

    #[test]
    fn the_command_prefix_alone_lists_every_command_in_registry_order() {
        let specs = commands();
        let rows = center_rows(">", &files(&["src/save.rs"]), &specs);
        assert_eq!(rows.len(), specs.len());
        for (index, (row, spec)) in rows.iter().zip(&specs).enumerate() {
            assert_eq!(row.entry, CenterEntry::Command(index));
            assert_eq!(row.label, spec.title);
            assert_eq!(row.detail, spec.shortcut);
        }
    }

    #[test]
    fn commands_are_filtered_by_title_and_by_menu() {
        let specs = commands();
        let rows = center_rows("> save all", &[], &specs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Save All");
        assert_eq!(rows[0].detail, "⌥⌘S");
        let CenterEntry::Command(index) = rows[0].entry else {
            panic!("a command row must carry a command");
        };
        assert_eq!(specs[index].title, "Save All");
        // A menu's name finds its commands.
        let selection = center_rows(">selection", &[], &specs);
        assert!(selection.iter().any(|row| row.label == "Select Line"));
        assert!(center_rows(">no such command", &[], &specs).is_empty());
    }
}
