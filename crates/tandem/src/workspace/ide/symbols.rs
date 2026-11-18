//! Go to Symbol in Workspace (⌘T, `#` in the command center): the language server of the
//! active tab's file finds the symbols of the whole project whose names match the query.
//! The command center asks again each time it is drawn with a different query; the
//! server is asked once the query has rested for a moment, and the dialog is drawn again
//! with what it found.
use super::*;
use serde_json::json;
use std::time::Duration;
use tandem_lsp::features::symbols::{SymbolHit, workspace_symbols};

/// How long the query rests before the language server is asked.
const DELAY: Duration = Duration::from_millis(150);
/// The capability of a server that finds workspace symbols.
const WORKSPACE_SYMBOL_PROVIDER: &str = "workspaceSymbolProvider";

/// The search of the command center's symbol query.
#[derive(Default)]
pub(super) struct SymbolSearch {
    /// Counts the searches started, so that only the latest one's reply is kept.
    epoch: u64,
    /// The query last searched for; None until the first search of the open dialog.
    query: Option<String>,
    /// The symbols the server found for the latest query that has a reply.
    hits: Vec<SymbolHit>,
    /// The search waiting or in flight; dropping it cancels it.
    task: Option<Task<()>>,
}

/// The language server's symbols for a query, or `None` when the code changed before the
/// server answered.
type SymbolReply = Result<Option<Vec<SymbolHit>>>;

/// The active tab and its language server when the server is ready and finds workspace
/// symbols; otherwise what the status line says instead.
fn symbol_target(target: Option<IdeTarget>) -> Result<(IdeTarget, LanguageManager), &'static str> {
    let target = target.ok_or("Open a file to search workspace symbols.")?;
    let manager = target
        .manager
        .clone()
        .ok_or("Workspace symbols need the language tools of the open file.")?;
    if manager.health().state != "ready" {
        return Err("Workspace symbols need a running language server for the open file.");
    }
    if !manager.capabilities().has(WORKSPACE_SYMBOL_PROVIDER) {
        return Err("The open file's language server does not search workspace symbols.");
    }
    Ok((target, manager))
}

impl Workspace {
    /// Searches the workspace symbols matching `query` after [`DELAY`], replacing a
    /// search still waiting or in flight; a query already searched for asks nothing. The
    /// command center calls this while it draws its dialog, so nothing here notifies the
    /// workspace at once: the search updates the status line and redraws the window when it
    /// runs.
    pub(in crate::workspace) fn request_workspace_symbols(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = &mut self.ide.symbols;
        if search.query.as_deref() == Some(query) {
            return;
        }
        search.query = Some(query.to_owned());
        search.epoch += 1;
        let (epoch, query) = (search.epoch, query.to_owned());
        search.task = Some(cx.spawn_in(window, async move |view, cx| {
            smol::Timer::after(DELAY).await;
            let reply = view.update_in(cx, |view, window, cx| {
                view.symbol_request(query, window, cx)
            });
            let Ok(Some(reply)) = reply else {
                return;
            };
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                view.symbols_replied(epoch, reply, window, cx)
            })
            .ok();
        }));
    }

    /// The symbols found for the command center's query, as last answered.
    pub(in crate::workspace) fn workspace_symbol_hits(&self) -> &[SymbolHit] {
        &self.ide.symbols.hits
    }

    /// Forgets the last search and cancels it, so that a command center opened again asks
    /// the server afresh even for the same query.
    pub(in crate::workspace) fn reset_workspace_symbols(&mut self) {
        let epoch = self.ide.symbols.epoch;
        self.ide.symbols = SymbolSearch {
            epoch,
            ..SymbolSearch::default()
        };
    }

    /// Asks the language server of the active tab's file for the symbols matching
    /// `query`, once it has the open files' text. Without a server that is ready and finds
    /// workspace symbols, the hits are cleared and the status line says why.
    fn symbol_request(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<SymbolReply>> {
        // The command center also opens over Settings and over a diff, so the active tab
        // counts even while its editor is covered.
        let (target, manager) = match symbol_target(self.active_ide_target()) {
            Ok(found) => found,
            Err(refusal) => {
                self.ide.symbols.hits.clear();
                self.message(refusal, false, cx);
                window.refresh();
                return None;
            }
        };
        let text = target.editor.read(cx).text().to_string();
        self.sync_language(cx);
        let (file, instance) = (target.file, target.instance);
        Some(cx.background_spawn(async move {
            let reply = manager
                .request(
                    "workspace/symbol",
                    &file,
                    &instance,
                    &text,
                    json!({ "query": query }),
                )
                .await?;
            Ok(manager
                .is_current(&reply.stamp)
                .then(|| workspace_symbols(&reply.value)))
        }))
    }

    /// Keeps the symbols of the latest search and redraws the window, so that the open
    /// command center lists them. A reply made stale by an edit is asked for again when
    /// the dialog is next drawn; a failure clears the list and says why.
    fn symbols_replied(
        &mut self,
        epoch: u64,
        reply: SymbolReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = &mut self.ide.symbols;
        if search.epoch != epoch {
            return;
        }
        match reply {
            Ok(Some(hits)) => search.hits = hits,
            Ok(None) => search.query = None,
            Err(error) => {
                search.hits.clear();
                self.message(
                    format!("Could not search workspace symbols: {error:#}"),
                    true,
                    cx,
                );
            }
        }
        window.refresh();
    }
}
