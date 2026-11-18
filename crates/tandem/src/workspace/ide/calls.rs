//! The call hierarchy (Show Call Hierarchy ⇧⌥H, Show Outgoing Calls): the functions that
//! call the function at the cursor, or the ones it calls, as a tree in the Problems panel
//! whose branches the language server fills in as they are expanded.
use super::icons::symbol_icon;
use super::*;
use crate::appearance::{compact_button, palette, panel_heading, panel_metadata};
use crate::workspace::panels::note;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, h_flex,
};
use serde_json::json;
use tandem_lsp::features::calls::{CallItem, incoming, outgoing, prepared};

/// The deepest level of the tree: the root is at depth 0, and a call at this depth does
/// not expand.
const MAX_DEPTH: usize = 8;
/// The indentation of one level of the tree, in pixels, as in the file tree.
const INDENT: f32 = 12.;
/// The capability of a server that shows call hierarchies.
const CALL_HIERARCHY_PROVIDER: &str = "callHierarchyProvider";
const STALE: &str = "The code changed before the language server answered. Try again.";

/// The calls the language server listed, or `None` when the code changed before it
/// answered.
type CallsReply = Result<Option<Vec<(CallItem, Option<u32>)>>>;
/// The functions at the cursor, or `None` when the code changed before the server
/// answered.
type PrepareReply = Result<Option<Vec<CallItem>>>;

/// The call hierarchy of the Problems panel and the requests that fill it in.
#[derive(Default)]
pub(super) struct CallsUi {
    /// The hierarchy on screen.
    shown: Option<CallHierarchy>,
    /// Counts the hierarchies made, so a list of calls goes only to the one it was asked
    /// for.
    made: u64,
    /// Show Call Hierarchy or Show Outgoing Calls waiting for the language server;
    /// dropping it cancels it.
    preparing: Option<Task<()>>,
}

/// A call tree on screen and where its calls are asked.
struct CallHierarchy {
    id: u64,
    tree: CallTree,
    source: CallSource,
    /// The calls being listed, by node; dropping one cancels it.
    loading: HashMap<usize, Task<()>>,
}

/// The language server a hierarchy was asked of and the file it was asked from: every
/// list of calls is asked of that server in the name of that file, while a tab shows it.
#[derive(Clone)]
struct CallSource {
    manager: LanguageManager,
    /// The file's absolute path.
    file: PathBuf,
    /// The file's project-relative path, as its tab knows it.
    relative: String,
    /// The project `epoch`; the hierarchy disappears with its project.
    epoch: u64,
}

/// The language server of the target's file when it is ready and shows call hierarchies;
/// otherwise what the status line says instead.
fn call_manager(target: &IdeTarget) -> Result<LanguageManager, &'static str> {
    let manager = target
        .manager
        .as_ref()
        .ok_or("Turn on language tools for this file's language in Settings → Language tools.")?;
    if manager.health().state != "ready" {
        return Err("The call hierarchy needs a running language server for this file.");
    }
    if !manager.capabilities().has(CALL_HIERARCHY_PROVIDER) {
        return Err("This file's language server does not show call hierarchies.");
    }
    Ok(manager.clone())
}

/// `path` relative to the project `root` when it is inside it, else as it is.
fn shown_path(path: &Path, root: Option<&Path>) -> String {
    root.and_then(|root| path.strip_prefix(root).ok())
        .and_then(|relative| normalized_relative(relative).ok())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether the tree lists the callers of its root or the functions it calls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallDirection {
    Incoming,
    Outgoing,
}

/// The call hierarchy of one function: its calls in one direction, each of which can be
/// expanded into its own calls in the same direction once the server has listed them.
pub struct CallTree {
    pub direction: CallDirection,
    pub root: CallItem,
    /// The functions of the tree; the root is the first, and children follow their parent.
    nodes: Vec<Node>,
}

/// One function of the tree.
struct Node {
    item: CallItem,
    /// The line of the call in the caller's file (0-based), when the server gave one.
    call_line: Option<u32>,
    depth: usize,
    expanded: bool,
    /// The indexes of its calls, None until the server has listed them.
    children: Option<Vec<usize>>,
}

/// One row of the tree as it is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct CallRow {
    /// The node this row shows, for [`CallTree::toggle`] and [`CallTree::set_children`].
    pub node: usize,
    pub depth: usize,
    pub name: String,
    pub detail: String,
    /// The function's SymbolKind number, for its icon.
    pub kind: u32,
    /// Where clicking the row leads: the call in the caller's file for a caller, the
    /// function itself for the root and for a function called.
    pub path: PathBuf,
    /// The line of that place, 0-based.
    pub line: u32,
    pub expanded: bool,
}

impl CallTree {
    /// A tree of `root` alone, expanded, whose calls are still to be listed.
    pub fn new(direction: CallDirection, root: CallItem) -> Self {
        let node = Node {
            item: root.clone(),
            call_line: None,
            depth: 0,
            expanded: true,
            children: None,
        };
        Self {
            direction,
            root,
            nodes: vec![node],
        }
    }

    /// The rows on screen, depth-first: the root, then the calls of each expanded
    /// function right below it.
    pub fn rows(&self) -> Vec<CallRow> {
        let mut rows = Vec::new();
        let mut pending = vec![0];
        while let Some(index) = pending.pop() {
            let node = &self.nodes[index];
            // A caller leads to its call; the root and a function called lead to themselves.
            let line = match (self.direction, node.call_line) {
                (CallDirection::Incoming, Some(line)) => line,
                _ => node.item.line,
            };
            rows.push(CallRow {
                node: index,
                depth: node.depth,
                name: node.item.name.clone(),
                detail: node.item.detail.clone(),
                kind: node.item.kind,
                path: node.item.path.clone(),
                line,
                expanded: node.expanded,
            });
            if node.expanded
                && let Some(children) = &node.children
            {
                pending.extend(children.iter().rev());
            }
        }
        rows
    }

    /// Whether `node` is expanded and the server has not listed its calls yet.
    pub fn needs_children(&self, node: usize) -> bool {
        self.nodes
            .get(node)
            .is_some_and(|node| node.expanded && node.children.is_none())
    }

    /// Sets the calls of `node`, collapsed and one level deeper, in the server's order.
    pub fn set_children(&mut self, node: usize, calls: Vec<(CallItem, Option<u32>)>) {
        let Some(depth) = self.nodes.get(node).map(|parent| parent.depth + 1) else {
            return;
        };
        let first = self.nodes.len();
        self.nodes
            .extend(calls.into_iter().map(|(item, call_line)| Node {
                item,
                call_line,
                depth,
                expanded: false,
                children: None,
            }));
        self.nodes[node].children = Some((first..self.nodes.len()).collect());
    }

    /// Expands or collapses `node`; a node [`MAX_DEPTH`] deep stays collapsed.
    pub fn toggle(&mut self, node: usize) {
        if let Some(node) = self.nodes.get_mut(node)
            && node.depth < MAX_DEPTH
        {
            node.expanded = !node.expanded;
        }
    }

    /// The function `node` shows.
    pub fn item(&self, node: usize) -> Option<&CallItem> {
        self.nodes.get(node).map(|node| &node.item)
    }

    /// Whether expanding `node` could show more: it is less than [`MAX_DEPTH`] deep and the
    /// server has not said it has no calls.
    pub fn can_expand(&self, node: usize) -> bool {
        self.nodes.get(node).is_some_and(|node| {
            node.depth < MAX_DEPTH && node.children.as_ref().is_none_or(|calls| !calls.is_empty())
        })
    }
}

impl Workspace {
    /// Asks the language server of the active file for the function at the cursor and
    /// shows its calls in `direction` in the Problems panel (Show Call Hierarchy, Show
    /// Outgoing Calls). No function there, or no server that shows call hierarchies, is
    /// said in the status line.
    pub(in crate::workspace) fn show_call_hierarchy(
        &mut self,
        direction: CallDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An earlier request still waiting for the server must not show its tree later.
        self.ide.calls.preparing = None;
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let manager = match call_manager(&target) {
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
        let source = CallSource {
            manager: manager.clone(),
            file: target.file,
            relative: target.relative,
            epoch: self.epoch,
        };
        let reply = cx.background_spawn({
            let (file, instance) = (source.file.clone(), target.instance);
            async move {
                let reply = manager
                    .request(
                        "textDocument/prepareCallHierarchy",
                        &file,
                        &instance,
                        &text,
                        params,
                    )
                    .await?;
                Ok(manager
                    .is_current(&reply.stamp)
                    .then(|| prepared(&reply.value)))
            }
        });
        self.ide.calls.preparing = Some(cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                view.call_hierarchy_prepared(direction, source, reply, window, cx)
            })
            .ok();
        }));
    }

    /// Shows the calls of the first function the server found at the cursor, while the
    /// same project is open, or says why there are none.
    fn call_hierarchy_prepared(
        &mut self,
        direction: CallDirection,
        source: CallSource,
        reply: PrepareReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.calls.preparing = None;
        if self.epoch != source.epoch {
            return;
        }
        match reply {
            Err(error) => self.message(
                format!("Could not show the call hierarchy: {error:#}"),
                true,
                cx,
            ),
            Ok(None) => self.message(STALE, false, cx),
            Ok(Some(items)) => match items.into_iter().next() {
                None => self.message("No call hierarchy is available here.", false, cx),
                Some(root) => {
                    self.show_call_tree(CallTree::new(direction, root), source, window, cx)
                }
            },
        }
        cx.notify();
    }

    /// Puts `tree` on screen in the Problems panel in place of the hierarchy shown before,
    /// and lists the calls of its root.
    fn show_call_tree(
        &mut self,
        tree: CallTree,
        source: CallSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ide.calls.made += 1;
        self.ide.calls.shown = Some(CallHierarchy {
            id: self.ide.calls.made,
            tree,
            source,
            loading: HashMap::new(),
        });
        self.bottom = Some(BottomTab::Problems);
        self.load_calls(0, window, cx);
        cx.notify();
    }

    /// The hierarchy on screen, while its project is open.
    fn shown_calls(&self) -> Option<&CallHierarchy> {
        let epoch = self.epoch;
        self.ide
            .calls
            .shown
            .as_ref()
            .filter(|calls| calls.source.epoch == epoch)
    }

    /// The hierarchy on screen, while its project is open, to change.
    fn shown_calls_mut(&mut self) -> Option<&mut CallHierarchy> {
        let epoch = self.epoch;
        self.ide
            .calls
            .shown
            .as_mut()
            .filter(|calls| calls.source.epoch == epoch)
    }

    /// Asks the hierarchy's language server for the calls of `node` when it is expanded
    /// and they are neither listed nor already asked for. The request is made in the name
    /// of the file the hierarchy was asked from, so a tab must still show that file.
    fn load_calls(&mut self, node: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(calls) = self.shown_calls() else {
            return;
        };
        if !calls.tree.needs_children(node) || calls.loading.contains_key(&node) {
            return;
        }
        let Some(item) = calls.tree.item(node) else {
            return;
        };
        let (id, direction, source) = (calls.id, calls.tree.direction, calls.source.clone());
        let params = json!({"item": item.raw});
        let Some((instance, text)) = self
            .tabs
            .iter()
            .find(|tab| tab.document.path == source.relative)
            .map(|tab| (tab.instance.clone(), tab.editor.read(cx).text().to_string()))
        else {
            let relative = source.relative;
            self.message(
                format!("Open {relative} again to list more calls."),
                false,
                cx,
            );
            self.calls_not_listed(node);
            return;
        };
        self.sync_language(cx);
        let method = match direction {
            CallDirection::Incoming => "callHierarchy/incomingCalls",
            CallDirection::Outgoing => "callHierarchy/outgoingCalls",
        };
        let reply = cx.background_spawn(async move {
            let manager = source.manager;
            let reply = manager
                .request(method, &source.file, &instance, &text, params)
                .await?;
            Ok(manager.is_current(&reply.stamp).then(|| match direction {
                CallDirection::Incoming => incoming(&reply.value),
                CallDirection::Outgoing => outgoing(&reply.value),
            }))
        });
        let task = cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update(cx, |view, cx| view.calls_listed(id, node, reply, cx))
                .ok();
        });
        if let Some(calls) = self.shown_calls_mut() {
            calls.loading.insert(node, task);
        }
    }

    /// Puts the calls the server listed for `node` of hierarchy `id` under it, or says why
    /// there are none and collapses it, so that expanding it again asks again.
    fn calls_listed(&mut self, id: u64, node: usize, reply: CallsReply, cx: &mut Context<Self>) {
        let Some(calls) = self.shown_calls_mut().filter(|calls| calls.id == id) else {
            return;
        };
        // This runs inside the request's own task; dropping its handle lets it finish.
        calls.loading.remove(&node);
        let name = calls
            .tree
            .item(node)
            .map(|item| item.name.clone())
            .unwrap_or_default();
        match reply {
            Ok(Some(found)) => calls.tree.set_children(node, found),
            Ok(None) => {
                self.calls_not_listed(node);
                self.message(STALE, false, cx);
            }
            Err(error) => {
                self.calls_not_listed(node);
                self.message(
                    format!("Could not list the calls of {name}: {error:#}"),
                    true,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Collapses `node` while its calls are still to be listed, so that expanding it again
    /// asks the server again.
    fn calls_not_listed(&mut self, node: usize) {
        if let Some(calls) = self.shown_calls_mut()
            && calls.tree.needs_children(node)
        {
            calls.tree.toggle(node);
        }
    }

    /// Expands or collapses `node`, listing its calls the first time it expands.
    fn toggle_call(&mut self, node: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(calls) = self.shown_calls_mut() {
            calls.tree.toggle(node);
            self.load_calls(node, window, cx);
            cx.notify();
        }
    }

    /// Shows the calls of the hierarchy's root in `direction` instead, from the start.
    fn reroot_calls(
        &mut self,
        direction: CallDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(calls) = self.shown_calls() else {
            return;
        };
        let tree = CallTree::new(direction, calls.tree.root.clone());
        let source = calls.source.clone();
        self.show_call_tree(tree, source, window, cx);
    }

    /// Removes the hierarchy from the Problems panel and cancels its requests.
    fn clear_calls(&mut self, cx: &mut Context<Self>) {
        self.ide.calls.shown = None;
        cx.notify();
    }

    /// Opens the file at `path` at `line` (0-based) when it is in the project.
    fn open_call(&mut self, path: &Path, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        let relative = path
            .strip_prefix(&root)
            .ok()
            .and_then(|relative| normalized_relative(relative).ok());
        match relative {
            Some(relative) => {
                self.open_file(relative, false, Some(line as usize + 1), window, cx);
            }
            None => self.message("That file is outside the project.", false, cx),
        }
    }

    /// The call hierarchy at the end of the Problems panel, under a top border: a heading
    /// naming the root, the Incoming, Outgoing and Clear buttons, then one row per call
    /// shown, indented by depth. None while no hierarchy is shown.
    pub(in crate::workspace) fn call_hierarchy_section(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let calls = self.shown_calls()?;
        let tree = &calls.tree;
        let muted = palette(cx.theme().mode).secondary_foreground;
        let root = self.project.as_ref().map(|project| project.root.as_path());
        let heading = match tree.direction {
            CallDirection::Incoming => format!("Calls to {}", tree.root.name),
            CallDirection::Outgoing => format!("Calls from {}", tree.root.name),
        };
        let direction_button =
            |id: &'static str, label: &'static str, direction| {
                compact_button(Button::new(id), label)
                    .selected(tree.direction == direction)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reroot_calls(direction, window, cx)
                    }))
            };
        let header = h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(panel_heading(note(heading)).flex_1())
            .child(direction_button(
                "calls-incoming",
                "Incoming",
                CallDirection::Incoming,
            ))
            .child(direction_button(
                "calls-outgoing",
                "Outgoing",
                CallDirection::Outgoing,
            ))
            .child(
                compact_button(Button::new("calls-clear"), "Clear")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_calls(cx))),
            );
        let rows = tree.rows().into_iter().map(|row| {
            let node = row.node;
            let chevron = if tree.can_expand(node) {
                let (icon, action) = if row.expanded {
                    (IconName::ChevronDown, "Collapse")
                } else {
                    (IconName::ChevronRight, "Expand")
                };
                Button::new(("call-toggle", node))
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .accessibility_label(format!("{action} {}", row.name))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.toggle_call(node, window, cx)),
                    )
                    .into_any_element()
            } else {
                // The width of the chevron, so the icons of a level line up.
                div().size_5().flex_shrink_0().into_any_element()
            };
            let place = format!("{}:{}", shown_path(&row.path, root), row.line + 1);
            let (path, line) = (row.path, row.line);
            h_flex()
                .items_center()
                .min_w_0()
                .pl(px(row.depth as f32 * INDENT))
                .child(chevron)
                .child(
                    Button::new(("call", node))
                        .ghost()
                        .small()
                        .flex_1()
                        .min_w_0()
                        .justify_start()
                        .accessibility_label(format!("{}, {place}", row.name))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .child(Icon::new(symbol_icon(row.kind)).xsmall())
                                .child(div().flex_shrink_0().child(row.name))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(muted)
                                        .child(row.detail),
                                )
                                .child(div().flex_shrink_0().text_color(muted).child(place)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_call(&path, line, window, cx)
                        })),
                )
        });
        let status = if calls.loading.contains_key(&0) {
            Some("Listing the calls…")
        } else if !tree.can_expand(0) {
            // The root is never too deep, so the server listed no calls.
            Some("No calls found.")
        } else {
            None
        };
        Some(
            div()
                .v_flex()
                .gap_1()
                .min_w_0()
                .mt_3()
                .pt_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(header)
                .children(rows)
                .children(status.map(|status| panel_metadata(status, cx)))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CallDirection, CallRow, CallTree, MAX_DEPTH};
    use serde_json::json;
    use std::path::PathBuf;
    use tandem_lsp::features::calls::CallItem;

    fn item(name: &str, path: &str, line: u32) -> CallItem {
        CallItem {
            name: name.into(),
            detail: format!("fn {name}()"),
            kind: 12,
            path: PathBuf::from(path),
            line,
            raw: json!({"name": name}),
        }
    }

    /// The node, depth, name, path, line and expanded flag of each row.
    fn rows(tree: &CallTree) -> Vec<(usize, usize, String, String, u32, bool)> {
        tree.rows()
            .into_iter()
            .map(|row| {
                let path = row.path.display().to_string();
                (row.node, row.depth, row.name, path, row.line, row.expanded)
            })
            .collect()
    }

    fn row(
        node: usize,
        depth: usize,
        name: &str,
        path: &str,
        line: u32,
        expanded: bool,
    ) -> (usize, usize, String, String, u32, bool) {
        (node, depth, name.into(), path.into(), line, expanded)
    }

    fn total_area() -> CallItem {
        item("total_area", "/project/src/lib.rs", 15)
    }

    #[test]
    fn a_new_tree_shows_its_root_expanded_with_its_calls_still_to_load() {
        let tree = CallTree::new(CallDirection::Incoming, total_area());
        assert_eq!(tree.root, total_area());
        assert_eq!(
            tree.rows(),
            [CallRow {
                node: 0,
                depth: 0,
                name: "total_area".into(),
                detail: "fn total_area()".into(),
                kind: 12,
                path: PathBuf::from("/project/src/lib.rs"),
                line: 15,
                expanded: true,
            }]
        );
        assert!(tree.needs_children(0));
        assert!(tree.can_expand(0));
        assert_eq!(tree.item(0), Some(&total_area()));
    }

    #[test]
    fn calls_follow_their_function_depth_first_and_load_when_expanded() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        tree.set_children(
            0,
            vec![
                (item("describe", "/project/src/lib.rs", 23), Some(24)),
                (item("main", "/project/src/main.rs", 2), Some(5)),
            ],
        );
        assert!(!tree.needs_children(0));
        // Callers start collapsed, so nothing is asked for them yet.
        assert!(!tree.needs_children(1));
        assert_eq!(
            rows(&tree),
            [
                row(0, 0, "total_area", "/project/src/lib.rs", 15, true),
                row(1, 1, "describe", "/project/src/lib.rs", 24, false),
                row(2, 1, "main", "/project/src/main.rs", 5, false),
            ]
        );
        tree.toggle(1);
        assert!(tree.needs_children(1));
        tree.set_children(
            1,
            vec![(item("report", "/project/src/report.rs", 7), Some(9))],
        );
        assert!(!tree.needs_children(1));
        assert_eq!(
            rows(&tree),
            [
                row(0, 0, "total_area", "/project/src/lib.rs", 15, true),
                row(1, 1, "describe", "/project/src/lib.rs", 24, true),
                row(3, 2, "report", "/project/src/report.rs", 9, false),
                row(2, 1, "main", "/project/src/main.rs", 5, false),
            ]
        );
        assert_eq!(tree.item(3).map(|item| item.name.as_str()), Some("report"));
        assert_eq!(tree.item(4), None);
    }

    #[test]
    fn a_caller_without_a_call_line_leads_to_its_own_line() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        tree.set_children(0, vec![(item("describe", "/project/src/lib.rs", 23), None)]);
        assert_eq!(
            rows(&tree)[1],
            row(1, 1, "describe", "/project/src/lib.rs", 23, false)
        );
    }

    #[test]
    fn a_function_called_leads_to_itself_not_to_the_call() {
        let mut tree = CallTree::new(CallDirection::Outgoing, total_area());
        tree.set_children(
            0,
            vec![(item("area", "/project/src/shapes.rs", 32), Some(18))],
        );
        assert_eq!(
            rows(&tree),
            [
                row(0, 0, "total_area", "/project/src/lib.rs", 15, true),
                row(1, 1, "area", "/project/src/shapes.rs", 32, false),
            ]
        );
    }

    #[test]
    fn collapsing_hides_every_descendant_and_expanding_shows_them_again() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        tree.set_children(
            0,
            vec![(item("describe", "/project/src/lib.rs", 23), Some(24))],
        );
        tree.toggle(1);
        tree.set_children(
            1,
            vec![(item("report", "/project/src/report.rs", 7), Some(9))],
        );
        tree.toggle(0);
        assert_eq!(
            rows(&tree),
            [row(0, 0, "total_area", "/project/src/lib.rs", 15, false)]
        );
        assert!(!tree.needs_children(0));
        tree.toggle(0);
        assert_eq!(rows(&tree).len(), 3);
        tree.toggle(1);
        assert_eq!(rows(&tree).len(), 2);
    }

    #[test]
    fn needs_children_only_while_expanded_and_not_listed() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        tree.toggle(0);
        assert!(!tree.needs_children(0), "collapsed");
        tree.toggle(0);
        assert!(tree.needs_children(0));
        tree.set_children(0, Vec::new());
        assert!(!tree.needs_children(0), "listed, with no calls");
        assert!(!tree.can_expand(0), "the server said it has no calls");
        assert!(!tree.needs_children(7), "no such node");
    }

    #[test]
    fn calls_stop_expanding_at_the_maximum_depth() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        for depth in 0..MAX_DEPTH {
            assert!(tree.can_expand(depth));
            tree.set_children(
                depth,
                vec![(item(&format!("caller{depth}"), "/project/a.rs", 1), None)],
            );
            tree.toggle(depth + 1);
        }
        let deepest = MAX_DEPTH;
        assert_eq!(tree.rows().last().map(|row| row.depth), Some(MAX_DEPTH));
        assert!(!tree.rows().last().unwrap().expanded);
        assert!(!tree.needs_children(deepest));
        assert!(!tree.can_expand(deepest));
        tree.toggle(deepest);
        assert!(!tree.needs_children(deepest));
        assert_eq!(tree.rows().len(), MAX_DEPTH + 1);
    }

    #[test]
    fn nodes_that_do_not_exist_are_left_alone() {
        let mut tree = CallTree::new(CallDirection::Incoming, total_area());
        tree.toggle(3);
        tree.set_children(3, vec![(item("describe", "/project/src/lib.rs", 23), None)]);
        assert_eq!(rows(&tree).len(), 1);
        assert!(!tree.can_expand(3));
    }
}
