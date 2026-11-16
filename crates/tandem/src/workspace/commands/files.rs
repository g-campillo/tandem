//! Commands on the open files: close all, reopen the last closed one, copy a path, reveal in Finder.
use super::*;

/// How many closed files Reopen Closed File remembers.
const CLOSED_LIMIT: usize = 20;

/// Records `path` as the most recently closed file, keeping each path once and at most
/// `CLOSED_LIMIT` of them.
fn remember_closed(closed: &mut Vec<String>, path: String) {
    closed.retain(|other| *other != path);
    closed.push(path);
    if closed.len() > CLOSED_LIMIT {
        closed.remove(0);
    }
}

/// Takes the most recently closed path that `is_open` does not report as open, dropping the
/// open ones it passes.
fn take_reopenable(closed: &mut Vec<String>, is_open: impl Fn(&str) -> bool) -> Option<String> {
    while let Some(path) = closed.pop() {
        if !is_open(&path) {
            return Some(path);
        }
    }
    None
}

/// The status message after Close All Files closed `closed` tabs and kept `kept` tabs with
/// unsaved changes.
fn close_all_message(closed: usize, kept: usize) -> String {
    let files = if closed == 1 { "file" } else { "files" };
    if kept == 0 {
        format!("Closed {closed} {files}.")
    } else {
        format!("Closed {closed} {files}; {kept} with unsaved changes stay open.")
    }
}

impl Workspace {
    /// Remembers a closed tab's path for Reopen Closed File.
    pub(in crate::workspace) fn note_closed(&mut self, path: String) {
        // A list left from the previous project is dropped before this project's path joins it.
        self.sync_history();
        remember_closed(&mut self.commands.closed, path);
    }

    /// Closes every tab without unsaved changes (⌘K ⌘W).
    pub(super) fn close_all_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.copy_inflight || self.tabs.is_empty() {
            return;
        }
        self.snapshot_live(cx);
        if let Err(error) = self.flush(cx) {
            self.message(format!("Could not preserve drafts: {error:#}"), true, cx);
            return;
        }
        let active = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|tab| tab.document.path.clone());
        let closing: Vec<String> = self
            .tabs
            .iter()
            .filter(|tab| !tab.saving && !tab.document.dirty())
            .map(|tab| tab.document.path.clone())
            .collect();
        for path in &closing {
            self.note_closed(path.clone());
        }
        self.tabs.retain(|tab| tab.saving || tab.document.dirty());
        self.invalidate_language(cx);
        self.active = active
            .and_then(|path| self.tabs.iter().position(|tab| tab.document.path == path))
            .or_else(|| (!self.tabs.is_empty()).then_some(0));
        self.close_pending = false;
        self.diff = None;
        if self.active.is_some() {
            self.go_to_line(None, window, cx);
        } else {
            self.query.update(cx, |query, cx| query.focus(window, cx));
        }
        self.changed(cx);
        let message = close_all_message(closing.len(), self.tabs.len());
        self.message(message, false, cx);
    }

    /// Opens the most recently closed file that is not open (⇧⌘T).
    pub(super) fn reopen_closed_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_history();
        let tabs = &self.tabs;
        let reopen = take_reopenable(&mut self.commands.closed, |path| {
            tabs.iter().any(|tab| tab.document.path == path)
        });
        match reopen {
            Some(path) => self.open_file(path, false, None, window, cx),
            None => self.message("No closed file to reopen.", false, cx),
        }
    }

    /// Copies the active file's absolute path, or its project-relative path (⌘K P, ⇧⌥⌘C).
    pub(super) fn copy_file_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        let Some((absolute, relative_path)) = self.active_file() else {
            self.message("Open a file first.", false, cx);
            return;
        };
        let text = if relative {
            relative_path
        } else {
            absolute.display().to_string()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.message(format!("Copied {text}"), false, cx);
    }

    /// Shows the active file in Finder (⌘K R).
    pub(super) fn reveal_in_finder(&mut self, cx: &mut Context<Self>) {
        let Some((absolute, _)) = self.active_file() else {
            self.message("Open a file first.", false, cx);
            return;
        };
        cx.reveal_path(&absolute);
    }

    /// The active tab's file as an absolute path and as a project-relative path.
    fn active_file(&self) -> Option<(PathBuf, String)> {
        let project = self.project.as_ref()?;
        let tab = self.active.and_then(|i| self.tabs.get(i))?;
        Some((
            project.root.join(&tab.document.path),
            tab.document.path.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{CLOSED_LIMIT, close_all_message, remember_closed, take_reopenable};

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn remember_closed_puts_the_newest_path_last() {
        let mut closed = Vec::new();
        remember_closed(&mut closed, "a.rs".into());
        remember_closed(&mut closed, "b.rs".into());
        assert_eq!(closed, paths(&["a.rs", "b.rs"]));
    }

    #[test]
    fn remember_closed_moves_a_path_closed_again_to_the_end() {
        let mut closed = paths(&["a.rs", "b.rs", "c.rs"]);
        remember_closed(&mut closed, "a.rs".into());
        assert_eq!(closed, paths(&["b.rs", "c.rs", "a.rs"]));
    }

    #[test]
    fn remember_closed_drops_the_oldest_beyond_the_limit() {
        let mut closed = Vec::new();
        for n in 0..=CLOSED_LIMIT {
            remember_closed(&mut closed, format!("{n}.rs"));
        }
        assert_eq!(closed.len(), CLOSED_LIMIT);
        assert_eq!(closed[0], "1.rs");
        assert_eq!(closed[CLOSED_LIMIT - 1], format!("{CLOSED_LIMIT}.rs"));
    }

    #[test]
    fn take_reopenable_takes_the_most_recent_path() {
        let mut closed = paths(&["a.rs", "b.rs"]);
        assert_eq!(take_reopenable(&mut closed, |_| false), Some("b.rs".into()));
        assert_eq!(closed, paths(&["a.rs"]));
    }

    #[test]
    fn take_reopenable_drops_paths_that_are_open_again() {
        let mut closed = paths(&["a.rs", "b.rs", "c.rs"]);
        let reopened = take_reopenable(&mut closed, |path| path == "c.rs" || path == "b.rs");
        assert_eq!(reopened, Some("a.rs".into()));
        assert!(closed.is_empty());
    }

    #[test]
    fn take_reopenable_is_none_when_nothing_is_left() {
        let mut empty = Vec::new();
        assert_eq!(take_reopenable(&mut empty, |_| false), None);
        let mut all_open = paths(&["a.rs"]);
        assert_eq!(take_reopenable(&mut all_open, |_| true), None);
        assert!(all_open.is_empty());
    }

    #[test]
    fn close_all_message_counts_closed_and_kept_files() {
        assert_eq!(close_all_message(3, 0), "Closed 3 files.");
        assert_eq!(close_all_message(1, 0), "Closed 1 file.");
        assert_eq!(
            close_all_message(2, 1),
            "Closed 2 files; 1 with unsaved changes stay open."
        );
        assert_eq!(
            close_all_message(0, 2),
            "Closed 0 files; 2 with unsaved changes stay open."
        );
    }
}
