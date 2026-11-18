//! Go to Type Definition and Go to Implementation (⌘F12): the language server of the active
//! file finds the places of the symbol at the cursor. One place opens its file at its line;
//! several are listed in the navigation results of the Problems panel.
use super::*;
use crate::workspace::tools::Navigation;
use lsp_types::Range as LspRange;
use tandem_lsp::LanguageStamp;
use tandem_lsp::features::locations::locations;

/// The most places listed in the navigation results.
const MAX_RESULTS: usize = 200;
const STALE: &str = "The code changed before the language server answered. Try again.";

/// The places the language server found, with the stamp of the code it looked at, or
/// `None` when the code changed before the server answered.
type LocationsReply = Result<Option<(LanguageStamp, Vec<(PathBuf, LspRange)>)>>;

/// The rows of the navigation results for the places in `locations` inside the project at
/// `root`, in the server's order: the project-relative path, the 1-based line and the label
/// `path:line  title`. Places outside the project are skipped, and at most
/// [`MAX_RESULTS`] are kept.
fn navigation_rows(locations: &[(PathBuf, LspRange)], root: &Path, title: &str) -> Vec<Navigation> {
    locations
        .iter()
        .filter_map(|(path, range)| {
            let path = path
                .strip_prefix(root)
                .ok()
                .and_then(|relative| normalized_relative(relative).ok())?;
            let line = range.start.line as usize + 1;
            Some(Navigation {
                label: format!("{path}:{line}  {title}"),
                path,
                line,
            })
        })
        .take(MAX_RESULTS)
        .collect()
}

/// The language server of the target's file when it is ready and offers `capability`;
/// otherwise what the status line says instead, where `title` names what is looked for.
fn lookup_manager(
    target: &IdeTarget,
    capability: &str,
    title: &str,
) -> Result<LanguageManager, String> {
    let manager = target
        .manager
        .as_ref()
        .ok_or("Turn on language tools for this file's language in Settings → Language tools.")?;
    if manager.health().state != "ready" {
        return Err(format!(
            "Finding the {title} needs a running language server for this file."
        ));
    }
    if !manager.capabilities().has(capability) {
        return Err(format!(
            "This file's language server does not find {title}s."
        ));
    }
    Ok(manager.clone())
}

/// Where a lookup was asked from, so that its reply is dropped once the user moved on.
struct Lookup {
    editor: EntityId,
    cursor: usize,
    epoch: u64,
    title: &'static str,
}

impl Workspace {
    /// Asks the language server of the active file for the `method` places of the symbol
    /// at the cursor (`textDocument/typeDefinition` or `textDocument/implementation`, which
    /// a server offers with `capability`). One place opens its file at its line; several
    /// are listed in the Problems panel; none, or no server that finds them, is said in the
    /// status line, where `title` names what was looked for.
    pub(in crate::workspace) fn go_to_locations(
        &mut self,
        method: &'static str,
        capability: &'static str,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An earlier lookup still waiting for the server must not jump later.
        self.ide.navigating = None;
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let manager = match lookup_manager(&target, capability, title) {
            Ok(manager) => manager,
            Err(refusal) => {
                self.message(refusal, false, cx);
                return;
            }
        };
        let (text, cursor) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        let Ok(params) = tandem_lsp::position_params(&text, cursor) else {
            return;
        };
        self.sync_language(cx);
        let reply = cx.background_spawn({
            let (file, instance) = (target.file, target.instance);
            async move {
                let reply = manager
                    .request(method, &file, &instance, &text, params)
                    .await?;
                Ok(manager
                    .is_current(&reply.stamp)
                    .then(|| (reply.stamp, locations(&reply.value))))
            }
        });
        let lookup = Lookup {
            editor: target.editor.entity_id(),
            cursor,
            epoch: self.epoch,
            title,
        };
        self.ide.navigating = Some(cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                view.locations_replied(lookup, reply, window, cx)
            })
            .ok();
        }));
    }

    /// Opens the one place the server found, lists several in the Problems panel or says
    /// that there is none, while the same project is open and the cursor of the editor the
    /// lookup was asked from is still on screen where it was.
    fn locations_replied(
        &mut self,
        lookup: Lookup,
        reply: LocationsReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.navigating = None;
        let unmoved = self.ide_target().is_some_and(|target| {
            target.editor.entity_id() == lookup.editor
                && target.editor.read(cx).cursor() == lookup.cursor
        });
        if self.epoch != lookup.epoch || !unmoved {
            return;
        }
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        let title = lookup.title;
        match reply {
            Err(error) => self.message(format!("Could not find the {title}: {error:#}"), true, cx),
            Ok(None) => self.message(STALE, false, cx),
            Ok(Some((stamp, places))) => {
                let mut rows = navigation_rows(&places, &root, title);
                match rows.len() {
                    0 => self.message(format!("No {title} found."), false, cx),
                    1 => {
                        let row = rows.remove(0);
                        self.open_file(row.path, false, Some(row.line), window, cx);
                    }
                    count => {
                        self.dev.navigation = rows;
                        self.dev.navigation_stamp = Some(stamp);
                        self.bottom = Some(BottomTab::Problems);
                        self.message(format!("{count} results"), false, cx);
                    }
                }
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RESULTS, navigation_rows};
    use lsp_types::{Position, Range};
    use std::path::{Path, PathBuf};

    fn on_line(line: u32) -> Range {
        Range::new(Position::new(line, 4), Position::new(line, 10))
    }

    fn rows(locations: &[(PathBuf, Range)]) -> Vec<(String, usize, String)> {
        navigation_rows(locations, Path::new("/project"), "implementation")
            .into_iter()
            .map(|row| (row.path, row.line, row.label))
            .collect()
    }

    #[test]
    fn rows_give_the_relative_path_the_1_based_line_and_the_title() {
        let locations = [
            (PathBuf::from("/project/src/lib.rs"), on_line(8)),
            (PathBuf::from("/project/src/shapes/circle.rs"), on_line(0)),
        ];
        assert_eq!(
            rows(&locations),
            [
                (
                    "src/lib.rs".to_owned(),
                    9,
                    "src/lib.rs:9  implementation".to_owned()
                ),
                (
                    "src/shapes/circle.rs".to_owned(),
                    1,
                    "src/shapes/circle.rs:1  implementation".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn places_outside_the_project_are_skipped() {
        let locations = [
            (
                PathBuf::from("/rustlib/src/core/src/fmt/mod.rs"),
                on_line(3),
            ),
            (PathBuf::from("/project-other/src/lib.rs"), on_line(3)),
            (PathBuf::from("/project"), on_line(3)),
            (PathBuf::from("/project/src/../../etc/passwd"), on_line(3)),
            (PathBuf::from("/project/src/kept.rs"), on_line(3)),
        ];
        assert_eq!(
            rows(&locations),
            [(
                "src/kept.rs".to_owned(),
                4,
                "src/kept.rs:4  implementation".to_owned()
            )]
        );
    }

    #[test]
    fn at_most_two_hundred_rows_are_kept() {
        let locations: Vec<_> = (0..300)
            .map(|line| (PathBuf::from("/project/src/lib.rs"), on_line(line)))
            .collect();
        let rows = rows(&locations);
        assert_eq!(rows.len(), MAX_RESULTS);
        assert_eq!(rows[MAX_RESULTS - 1].1, MAX_RESULTS);
    }
}
