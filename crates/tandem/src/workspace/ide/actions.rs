//! Quick Fix (⌥↵) and Organize Imports (⇧⌥O): the language server's code actions for the
//! selection, listed in the toolkit's code-action menu, and what the chosen action does.
//! Its edits go to the buffers through `apply_file_edits`, which leaves them unsaved; its
//! command runs on the server only when the server lists that command. The one exception
//! is Pyright's organize-imports command, which Pyright neither offers as an action nor
//! lists: Organize Imports runs it by name on Pyright, and its edits may change only the
//! file whose imports it organizes.
use super::*;
use gpui_kit::component::highlighter::{DiagnosticEntry, DiagnosticSeverity as Severity};
use gpui_kit::component::input::CodeActionProvider;
use lsp_types::{CodeAction, DiagnosticSeverity, NumberOrString};
use serde_json::{Value, json};
use std::ops::Range;
use tandem_lsp::features::actions::{
    self as server, ORGANIZE_IMPORTS, code_action_params, pyright_organize_imports,
};
use tandem_lsp::features::edits::{FileEdits, command_edits, workspace_edit};

const STALE: &str = "The code changed before the language server answered. Try again.";
const NO_ACTIONS: &str = "No code actions are available here.";
const NO_ORGANIZE: &str = "The language server has no organize-imports action for this file.";

/// Offers the language server's code actions to the toolkit's code-action menu of one
/// tab's editor, for the life of the tab. The toolkit calls it from its own updates
/// (choosing an action runs inside the editor's), so both methods hand the work to the
/// workspace in a task of their own, once those updates are over.
pub(in crate::workspace) struct CodeActionSource {
    pub workspace: WeakEntity<Workspace>,
}

impl CodeActionProvider for CodeActionSource {
    fn id(&self) -> SharedString {
        "tandem".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let workspace = self.workspace.clone();
        window.spawn(cx, async move |cx| {
            let request =
                workspace.update(cx, |view, cx| view.code_action_request(&state, range, cx))?;
            let Some(request) = request else {
                return Ok(Vec::new());
            };
            let reply = request.await;
            workspace.update(cx, |view, cx| view.code_actions_replied(reply, cx))
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let workspace = self.workspace.clone();
        window.spawn(cx, async move |cx| {
            workspace.update_in(cx, |view, window, cx| {
                view.perform_code_action(&state, action, window, cx)
            })
        })
    }
}

/// A code action the user asked for and the file it is for, kept from when they asked, so
/// its edits and its command go to that file and its server even when another tab is
/// active by the time the server answers.
#[derive(Clone)]
struct ActionRequest {
    asked: Asked,
    manager: LanguageManager,
    editor: WeakEntity<EditorState>,
    /// The file's absolute path.
    file: PathBuf,
    /// The tab's document instance, as the language server knows it.
    instance: String,
    /// Whether the server is Pyright, whose organize-imports command Organize Imports runs
    /// by name.
    pyright: bool,
}

/// What the user asked for, which decides what the status bar says once its changes are
/// made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    /// A code action chosen in the Quick Fix menu.
    QuickFix,
    OrganizeImports,
    /// The server command of an accepted completion: only failures are reported.
    Completion,
}

/// How Organize Imports organizes a file's imports.
#[derive(Clone, Debug, PartialEq)]
enum ImportsPlan {
    /// With the server's organize-imports action.
    Action(Box<CodeAction>),
    /// With Pyright's own organize-imports command for the file.
    PyrightCommand(lsp_types::Command),
}

/// Which files the edits a server command asks for may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandReach {
    /// Any file in the project: the command is one the server lists.
    Project,
    /// Only the file it was run for: Pyright's organize-imports command, which Pyright
    /// does not list.
    OwnFile,
}

impl Workspace {
    /// Whether the active file's language server is Pyright, the server of Tandem's Python
    /// tool slot. Pyright sends no `serverInfo` to tell it by.
    fn active_server_is_pyright(&self) -> bool {
        match (self.language_manager(), self.dev.python_language.as_ref()) {
            (Some(active), Some(pyright)) => std::ptr::eq(active, pyright),
            _ => false,
        }
    }

    /// The language server of `target` while it can give code actions, or `None` with a
    /// status message saying why it can't now.
    fn action_manager(
        &mut self,
        target: &IdeTarget,
        cx: &mut Context<Self>,
    ) -> Option<LanguageManager> {
        let refusal = if self.loading {
            "Wait for the project to finish opening."
        } else if self.takeover_running() {
            "The editor is read-only while Claude edits files."
        } else {
            match &target.manager {
                None => {
                    "Turn on language tools for this file's language in Settings → Language tools."
                }
                Some(manager) if manager.health().state != "ready" => {
                    "Code actions need a running language server for this file."
                }
                Some(manager) if !manager.capabilities().has("codeActionProvider") => {
                    "This file's language server does not offer code actions."
                }
                Some(manager) => return Some(manager.clone()),
            }
        };
        self.message(refusal, false, cx);
        None
    }

    /// What `asked` needs to know about `editor`'s file and its language server, while
    /// `editor` is the active tab's on screen and the server can give code actions; else
    /// `None`, with a status message when the server is why.
    fn action_request(
        &mut self,
        editor: EntityId,
        asked: Asked,
        cx: &mut Context<Self>,
    ) -> Option<ActionRequest> {
        let target = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor)?;
        let manager = self.action_manager(&target, cx)?;
        Some(ActionRequest {
            asked,
            manager,
            editor: target.editor.downgrade(),
            file: target.file,
            instance: target.instance,
            pyright: self.active_server_is_pyright(),
        })
    }

    /// Asks the language server of `request` for `method` about `text`, its editor's text
    /// now, once the server has that text: the reply, or `None` when the code changed
    /// before the server answered.
    fn ask_server(
        &mut self,
        request: &ActionRequest,
        method: &'static str,
        text: String,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<Value>>> {
        self.sync_language(cx);
        let (manager, path, instance) = (
            request.manager.clone(),
            request.file.clone(),
            request.instance.clone(),
        );
        cx.background_spawn(async move {
            let reply = manager
                .request(method, &path, &instance, &text, params)
                .await?;
            Ok(manager.is_current(&reply.stamp).then_some(reply.value))
        })
    }

    /// Starts asking for the code actions on `range` of `editor`, with the editor's
    /// diagnostics that touch the range; `None` when they can't be asked for now, with a
    /// status message saying why.
    fn code_action_request(
        &mut self,
        editor: &Entity<EditorState>,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<Option<Value>>>> {
        let request = self.action_request(editor.entity_id(), Asked::QuickFix, cx)?;
        // The toolkit's menu takes the arrow keys and Enter, which an open completion list
        // would otherwise keep.
        self.completion_blurred(editor, cx);
        let (text, params) = {
            let state = editor.read(cx);
            let text = state.text().to_string();
            let diagnostics = state
                .diagnostics()
                .map(|set| range_diagnostics(&text, set.iter(), &range))
                .unwrap_or_default();
            let params = code_action_params(&text, range, diagnostics, None).ok()?;
            (text, params)
        };
        Some(self.ask_server(&request, "textDocument/codeAction", text, params, cx))
    }

    /// The code actions of the server's reply, for the toolkit's menu; none, with a status
    /// message, when the server has none or its reply is stale or an error.
    fn code_actions_replied(
        &mut self,
        reply: Result<Option<Value>>,
        cx: &mut Context<Self>,
    ) -> Vec<CodeAction> {
        let actions = match reply {
            Ok(Some(value)) => server::code_actions(&value),
            Ok(None) => {
                self.message(STALE, false, cx);
                return Vec::new();
            }
            Err(error) => {
                self.message(format!("Could not get code actions: {error:#}"), true, cx);
                return Vec::new();
            }
        };
        if actions.is_empty() {
            self.message(NO_ACTIONS, false, cx);
        }
        actions
    }

    /// Applies the code action chosen in the toolkit's menu of `editor`.
    fn perform_code_action(
        &mut self,
        editor: &Entity<EditorState>,
        action: CodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(request) = self.action_request(editor.entity_id(), Asked::QuickFix, cx) {
            self.resolve_and_apply(request, action, window, cx);
        }
    }

    /// Organize Imports (⇧⌥O): asks the language server of the active file for its
    /// organize-imports action on the whole file and applies it; on Pyright, which offers
    /// no such action, runs Pyright's organize-imports command on the file instead.
    pub(in crate::workspace) fn organize_imports(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An earlier action still waiting for the server must not apply later.
        self.ide.acting = None;
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let editor = target.editor.entity_id();
        let Some(request) = self.action_request(editor, Asked::OrganizeImports, cx) else {
            return;
        };
        let text = target.editor.read(cx).text().to_string();
        let whole = 0..text.len();
        let Ok(params) = code_action_params(&text, whole, Vec::new(), Some(&[ORGANIZE_IMPORTS]))
        else {
            return;
        };
        let reply = self.ask_server(&request, "textDocument/codeAction", text, params, cx);
        self.ide.acting = Some(cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                view.organize_imports_replied(request, reply, window, cx)
            })
            .ok();
        }));
    }

    /// Organizes the imports as the server's reply allows (see `imports_plan`), or says
    /// why it can't.
    fn organize_imports_replied(
        &mut self,
        request: ActionRequest,
        reply: Result<Option<Value>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This runs inside the request's own task; dropping its handle lets it finish.
        self.ide.acting = None;
        let actions = match reply {
            Ok(Some(value)) => server::code_actions(&value),
            Ok(None) => return self.message(STALE, false, cx),
            Err(error) => {
                let message = format!("Could not organize the imports: {error:#}");
                return self.message(message, true, cx);
            }
        };
        match imports_plan(&actions, request.pyright, &request.file) {
            Ok(ImportsPlan::Action(action)) => self.resolve_and_apply(request, *action, window, cx),
            Ok(ImportsPlan::PyrightCommand(command)) => {
                self.execute_command(request, command, CommandReach::OwnFile, 0, window, cx)
            }
            Err(message) => self.message(message, false, cx),
        }
    }

    /// Applies `action`, first asking the server for its edit (`codeAction/resolve`) when
    /// it has none yet and the server fills it in on request.
    fn resolve_and_apply(
        &mut self,
        request: ActionRequest,
        action: CodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // This may run inside the task of an earlier step; dropping its handle lets it
        // finish.
        self.ide.acting = None;
        if !needs_resolve(
            &action,
            request.manager.capabilities().code_action_resolve(),
        ) {
            return self.apply_code_action(request, action, window, cx);
        }
        let Some(editor) = request.editor.upgrade() else {
            return;
        };
        let text = editor.read(cx).text().to_string();
        let Ok(params) = serde_json::to_value(&action) else {
            return;
        };
        let reply = self.ask_server(&request, "codeAction/resolve", text, params, cx);
        self.ide.acting = Some(cx.spawn_in(window, async move |view, cx| {
            let resolved = reply.await.and_then(|value| {
                value
                    .map(serde_json::from_value::<CodeAction>)
                    .transpose()
                    .map_err(anyhow::Error::from)
            });
            view.update_in(cx, |view, window, cx| {
                view.ide.acting = None;
                match resolved {
                    Ok(Some(action)) => view.apply_code_action(request, action, window, cx),
                    Ok(None) => view.message(STALE, false, cx),
                    Err(error) => view.message(
                        format!("Could not apply the code action: {error:#}"),
                        true,
                        cx,
                    ),
                }
            })
            .ok();
        }));
    }

    /// Applies a code action's edit to the buffers, then runs its command.
    fn apply_code_action(
        &mut self,
        request: ActionRequest,
        action: CodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = match &action.edit {
            None => Ok(Vec::new()),
            Some(edit) => serde_json::to_value(edit)
                .map_err(|error| format!("Could not read the language server's edit: {error}."))
                .and_then(|edit| workspace_edit(&edit).map_err(|refusal| refusal.to_string())),
        };
        match files {
            Ok(files) => self.apply_then_run(request, files, action.command, 0, window, cx),
            Err(message) => self.message(message, true, cx),
        }
    }

    /// Applies `files` to the buffers, then runs `command` on the server when there is
    /// one. `before` counts the files earlier steps of the same request changed.
    fn apply_then_run(
        &mut self,
        request: ActionRequest,
        files: Vec<FileEdits>,
        command: Option<lsp_types::Command>,
        before: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        self.apply_file_edits(files, window, cx, move |view, applied, cx| {
            let changed = match applied {
                Ok(changed) => before + changed,
                Err(message) => return view.message(message, true, cx),
            };
            let Some(command) = command else {
                if let Some(message) = applied_message(request.asked, changed) {
                    view.message(message, false, cx);
                }
                return;
            };
            // The command runs once this update is over, with the window it needs.
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    weak.update(cx, |view, cx| {
                        view.run_action_command(request, command, changed, window, cx)
                    })
                });
            });
        });
    }

    /// Runs the server command of an accepted completion, when the active file's language
    /// server lists it, and applies the edits it asks for.
    pub(in crate::workspace) fn run_server_command(
        &mut self,
        command: lsp_types::Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.ide_target() else {
            return;
        };
        let Some(manager) = target
            .manager
            .filter(|manager| manager.health().state == "ready")
        else {
            return;
        };
        let request = ActionRequest {
            asked: Asked::Completion,
            manager,
            editor: target.editor.downgrade(),
            file: target.file,
            instance: target.instance,
            pyright: self.active_server_is_pyright(),
        };
        self.run_action_command(request, command, 0, window, cx);
    }

    /// Runs `command` on the language server of `request` when the server lists it, and
    /// applies the edits it asks for; a command the server does not list is left alone.
    /// `before` counts the files earlier steps of the same request changed.
    fn run_action_command(
        &mut self,
        request: ActionRequest,
        command: lsp_types::Command,
        before: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !request
            .manager
            .capabilities()
            .commands()
            .contains(&command.command)
        {
            if let Some(message) = applied_message(request.asked, before) {
                self.message(message, false, cx);
            }
            return;
        }
        self.execute_command(request, command, CommandReach::Project, before, window, cx);
    }

    /// Runs `command` on the language server of `request`, and applies the edits it asks
    /// for when they change only the files `reach` allows. `before` counts the files
    /// earlier steps of the same request changed.
    fn execute_command(
        &mut self,
        request: ActionRequest,
        command: lsp_types::Command,
        reach: CommandReach,
        before: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = request.editor.upgrade() else {
            return;
        };
        let text = editor.read(cx).text().to_string();
        let params = command_params(&command);
        let reply = self.ask_server(&request, "workspace/executeCommand", text, params, cx);
        self.ide.acting = Some(cx.spawn_in(window, async move |view, cx| {
            let reply = reply.await;
            view.update_in(cx, |view, window, cx| {
                // This runs inside the request's own task; dropping its handle lets it
                // finish.
                view.ide.acting = None;
                let files = match reply {
                    Ok(Some(value)) => command_edits(&value, &request.file)
                        .map_err(|refusal| refusal.to_string())
                        .and_then(|files| match reach {
                            CommandReach::Project => Ok(files),
                            CommandReach::OwnFile => own_file_edits(files, &request.file),
                        }),
                    Ok(None) => Err(STALE.to_string()),
                    Err(error) => Err(format!(
                        "The language server could not run {}: {error:#}",
                        command.title
                    )),
                };
                match files {
                    Ok(files) => view.apply_then_run(request, files, None, before, window, cx),
                    Err(message) => view.message(message, true, cx),
                }
            })
            .ok();
        }));
    }
}

/// How Organize Imports organizes the imports of `file`, given the code actions its server
/// offers for the whole file: with the server's organize-imports action; failing that, on
/// Pyright (`pyright`), with Pyright's own organize-imports command for the file. Else the
/// message saying the server has no way to.
fn imports_plan(
    actions: &[CodeAction],
    pyright: bool,
    file: &Path,
) -> Result<ImportsPlan, &'static str> {
    if let Some(action) = server::organize_imports(actions) {
        return Ok(ImportsPlan::Action(Box::new(action)));
    }
    match tandem_lsp::file_uri(file) {
        Ok(uri) if pyright => Ok(ImportsPlan::PyrightCommand(pyright_organize_imports(&uri))),
        _ => Err(NO_ORGANIZE),
    }
}

/// `files` when they are all `file`: any edit for another file refuses them all.
fn own_file_edits(files: Vec<FileEdits>, file: &Path) -> Result<Vec<FileEdits>, String> {
    match files.iter().find(|edits| edits.path != file) {
        Some(other) => Err(format!(
            "Organizing the imports would also change {}, so nothing was changed.",
            other.path.display()
        )),
        None => Ok(files),
    }
}

/// What the status bar says once the changes `asked` for edited `changed` files; nothing
/// for an accepted completion.
fn applied_message(asked: Asked, changed: usize) -> Option<String> {
    let message = match (asked, changed) {
        (Asked::Completion, _) => return None,
        (Asked::QuickFix, 0) => "The code action changed nothing.".to_string(),
        (Asked::QuickFix, 1) => "Applied the code action.".to_string(),
        (Asked::QuickFix, files) => {
            format!("Applied the code action to {files} files. Review and save the changes.")
        }
        (Asked::OrganizeImports, 0) => "The imports are already organized.".to_string(),
        (Asked::OrganizeImports, _) => "Organized the imports.".to_string(),
    };
    Some(message)
}

/// Whether `action` must be resolved before it is applied: it has no edit yet, carries
/// the `data` the server resolves it from, and the server resolves code actions.
fn needs_resolve(action: &CodeAction, server_resolves: bool) -> bool {
    action.edit.is_none() && action.data.is_some() && server_resolves
}

/// Params for `workspace/executeCommand`: the command's name and its arguments, if any.
fn command_params(command: &lsp_types::Command) -> Value {
    let mut params = json!({"command": command.command});
    if let Some(arguments) = &command.arguments {
        params["arguments"] = json!(arguments);
    }
    params
}

/// The diagnostics among `entries` that touch `range` of `text` (overlap it, or meet it
/// at either end, so an empty range at the cursor finds the diagnostic it is in), as the
/// server's diagnostics, with the data the server attached to them.
fn range_diagnostics<'a>(
    text: &str,
    entries: impl IntoIterator<Item = &'a DiagnosticEntry>,
    range: &Range<usize>,
) -> Vec<lsp_types::Diagnostic> {
    entries
        .into_iter()
        .filter(|entry| entry.range.start <= range.end && range.start <= entry.range.end)
        .filter_map(|entry| {
            let mut diagnostic = lsp_diagnostic(
                text,
                entry.range.clone(),
                Some(lsp_severity(entry.severity)),
                entry.code.as_deref(),
                entry.source.as_deref(),
                &entry.message,
            )?;
            diagnostic.data = entry.data.clone();
            Some(diagnostic)
        })
        .collect()
}

/// The protocol severity of a toolkit severity.
fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

/// A diagnostic of the editor at the byte `range` of `text`, as the language server's
/// diagnostic: its range in UTF-16 positions and its code as a string. `None` when the
/// range does not fit `text`.
fn lsp_diagnostic(
    text: &str,
    range: Range<usize>,
    severity: Option<DiagnosticSeverity>,
    code: Option<&str>,
    source: Option<&str>,
    message: &str,
) -> Option<lsp_types::Diagnostic> {
    if range.start > range.end {
        return None;
    }
    let start = tandem_lsp::utf16_position(text, range.start).ok()?;
    let end = tandem_lsp::utf16_position(text, range.end).ok()?;
    Some(lsp_types::Diagnostic {
        range: lsp_types::Range::new(start, end),
        severity,
        code: code.map(|code| NumberOrString::String(code.to_string())),
        source: source.map(str::to_string),
        message: message.to_string(),
        ..lsp_types::Diagnostic::default()
    })
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{
        Asked, ImportsPlan, NO_ORGANIZE, applied_message, command_params, imports_plan,
        lsp_diagnostic, needs_resolve, own_file_edits, range_diagnostics,
    };
    use gpui_kit::component::highlighter::{Diagnostic, DiagnosticEntry, DiagnosticSeverity};
    use lsp_types::{CodeAction, CodeActionKind, NumberOrString, Position, Range, TextEdit};
    use serde_json::json;
    use std::path::Path;
    use tandem_lsp::features::edits::FileEdits;

    #[test]
    fn a_diagnostic_after_an_emoji_gets_utf16_columns() {
        let text = "let 🦀 = 1;\nlet x = 🦀 + y;\n";
        let start = text.rfind('y').unwrap();
        let diagnostic = lsp_diagnostic(
            text,
            start..start + 1,
            Some(lsp_types::DiagnosticSeverity::ERROR),
            Some("E0425"),
            Some("rustc"),
            "cannot find value `y` in this scope",
        )
        .unwrap();
        // `🦀` is two UTF-16 units and four UTF-8 bytes.
        assert_eq!(
            diagnostic.range,
            Range::new(Position::new(1, 13), Position::new(1, 14))
        );
        assert_eq!(
            diagnostic.severity,
            Some(lsp_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("E0425".into()))
        );
        assert_eq!(diagnostic.source.as_deref(), Some("rustc"));
        assert_eq!(diagnostic.message, "cannot find value `y` in this scope");
    }

    #[test]
    fn a_numeric_code_is_sent_as_a_string_and_missing_fields_stay_missing() {
        let diagnostic = lsp_diagnostic("abc", 0..3, None, Some("2304"), None, "m").unwrap();
        assert_eq!(diagnostic.code, Some(NumberOrString::String("2304".into())));
        assert_eq!(diagnostic.severity, None);
        assert_eq!(diagnostic.source, None);
        let bare = lsp_diagnostic("abc", 1..1, None, None, None, "m").unwrap();
        assert_eq!(bare.code, None);
        assert_eq!(
            bare.range,
            Range::new(Position::new(0, 1), Position::new(0, 1))
        );
    }

    #[test]
    fn a_range_outside_the_text_gives_no_diagnostic() {
        assert_eq!(lsp_diagnostic("abc", 2..9, None, None, None, "m"), None);
        assert_eq!(lsp_diagnostic("abc", 7..9, None, None, None, "m"), None);
        // Inside a character.
        assert_eq!(lsp_diagnostic("é", 1..2, None, None, None, "m"), None);
        // Backwards.
        let backwards = std::ops::Range { start: 2, end: 1 };
        assert_eq!(
            lsp_diagnostic("abc", backwards, None, None, None, "m"),
            None
        );
    }

    fn entry(range: std::ops::Range<usize>, message: &str) -> DiagnosticEntry {
        DiagnosticEntry {
            range,
            diagnostic: Diagnostic {
                message: message.to_string().into(),
                severity: DiagnosticSeverity::Warning,
                ..Diagnostic::default()
            },
        }
    }

    #[test]
    fn the_diagnostics_touching_the_range_are_sent() {
        let text = "let unused = 1;\nlet other = 2;\n";
        let entries = [
            entry(4..10, "first"),
            entry(20..25, "second"),
            entry(0..3, "keyword"),
        ];
        let messages = |range: std::ops::Range<usize>| {
            range_diagnostics(text, &entries, &range)
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>()
        };
        // A cursor at the start, inside or at the end of a diagnostic finds it.
        assert_eq!(messages(4..4), ["first"]);
        assert_eq!(messages(7..7), ["first"]);
        assert_eq!(messages(10..10), ["first"]);
        // A cursor between two diagnostics finds neither.
        assert!(messages(13..13).is_empty());
        // A selection finds every diagnostic it overlaps, in their order.
        assert_eq!(messages(2..22), ["first", "second", "keyword"]);
        let sent = &range_diagnostics(text, &entries, &(5..5))[0];
        assert_eq!(sent.severity, Some(lsp_types::DiagnosticSeverity::WARNING));
        assert_eq!(
            sent.range,
            Range::new(Position::new(0, 4), Position::new(0, 10))
        );
    }

    #[test]
    fn a_diagnostic_keeps_the_data_the_server_attached() {
        let mut with_data = entry(0..3, "fixable");
        with_data.diagnostic.data = Some(json!({"fix": 1}));
        let sent = range_diagnostics("abc", [&with_data], &(1..1));
        assert_eq!(sent[0].data, Some(json!({"fix": 1})));
    }

    #[test]
    fn a_diagnostic_whose_range_no_longer_fits_the_text_is_left_out() {
        let entries = [entry(0..3, "kept"), entry(2..40, "stale")];
        let sent = range_diagnostics("abc", &entries, &(0..3));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message, "kept");
    }

    #[test]
    fn an_action_is_resolved_only_without_an_edit_with_data_and_a_resolving_server() {
        let bare = CodeAction {
            title: "Fix".into(),
            ..CodeAction::default()
        };
        let with_data = CodeAction {
            data: Some(json!({"id": 1})),
            ..bare.clone()
        };
        let with_edit = CodeAction {
            edit: Some(lsp_types::WorkspaceEdit::default()),
            ..with_data.clone()
        };
        assert!(needs_resolve(&with_data, true));
        assert!(!needs_resolve(&with_data, false));
        assert!(!needs_resolve(&bare, true));
        assert!(!needs_resolve(&with_edit, true));
    }

    #[test]
    fn command_params_carry_the_name_and_the_arguments_when_there_are_some() {
        let command = lsp_types::Command {
            title: "Organize Imports".into(),
            command: "pyright.organizeimports".into(),
            arguments: Some(vec![json!("file:///project/main.py")]),
        };
        assert_eq!(
            command_params(&command),
            json!({"command": "pyright.organizeimports", "arguments": ["file:///project/main.py"]})
        );
        let bare = lsp_types::Command {
            arguments: None,
            ..command
        };
        assert_eq!(
            command_params(&bare),
            json!({"command": "pyright.organizeimports"})
        );
    }

    #[test]
    fn the_status_bar_says_what_changed() {
        assert_eq!(
            applied_message(Asked::QuickFix, 1).as_deref(),
            Some("Applied the code action.")
        );
        assert_eq!(
            applied_message(Asked::QuickFix, 3).as_deref(),
            Some("Applied the code action to 3 files. Review and save the changes.")
        );
        assert_eq!(
            applied_message(Asked::QuickFix, 0).as_deref(),
            Some("The code action changed nothing.")
        );
        assert_eq!(
            applied_message(Asked::OrganizeImports, 1).as_deref(),
            Some("Organized the imports.")
        );
        assert_eq!(
            applied_message(Asked::OrganizeImports, 0).as_deref(),
            Some("The imports are already organized.")
        );
        assert_eq!(applied_message(Asked::Completion, 2), None);
    }

    fn organize_action() -> CodeAction {
        CodeAction {
            title: "Organize Imports".into(),
            kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            ..CodeAction::default()
        }
    }

    #[test]
    fn organize_imports_on_pyright_without_an_action_runs_pyrights_command() {
        let main = Path::new("/project/main.py");
        let Ok(ImportsPlan::PyrightCommand(command)) = imports_plan(&[], true, main) else {
            panic!("Pyright's own command, for want of an action");
        };
        assert_eq!(command.command, "pyright.organizeimports");
        assert_eq!(
            command.arguments,
            Some(vec![json!("file:///project/main.py")])
        );
        // Pyright's quick fixes are no organize-imports action either.
        let fix = CodeAction {
            title: "Add import".into(),
            kind: Some(CodeActionKind::QUICKFIX),
            ..CodeAction::default()
        };
        assert!(matches!(
            imports_plan(&[fix], true, main),
            Ok(ImportsPlan::PyrightCommand(_))
        ));
    }

    #[test]
    fn organize_imports_without_an_action_on_another_server_says_there_is_none() {
        assert_eq!(
            imports_plan(&[], false, Path::new("/project/src/lib.rs")),
            Err(NO_ORGANIZE)
        );
        assert_eq!(
            NO_ORGANIZE,
            "The language server has no organize-imports action for this file."
        );
    }

    #[test]
    fn organize_imports_applies_the_servers_action_when_it_offers_one() {
        for pyright in [true, false] {
            assert_eq!(
                imports_plan(&[organize_action()], pyright, Path::new("/project/main.py")),
                Ok(ImportsPlan::Action(Box::new(organize_action()))),
                "{pyright}"
            );
        }
    }

    fn edits_for(path: &str) -> FileEdits {
        FileEdits {
            path: path.into(),
            edits: vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(1, 9)),
                new_text: "import os\nimport sys".into(),
            }],
        }
    }

    #[test]
    fn pyrights_command_may_change_only_its_own_file() {
        let main = Path::new("/project/main.py");
        assert_eq!(
            own_file_edits(vec![edits_for("/project/main.py")], main),
            Ok(vec![edits_for("/project/main.py")])
        );
        assert_eq!(own_file_edits(vec![], main), Ok(vec![]));
    }

    #[test]
    fn edits_of_pyrights_command_for_another_file_are_refused() {
        let main = Path::new("/project/main.py");
        let files = vec![
            edits_for("/project/main.py"),
            edits_for("/project/shapes.py"),
        ];
        assert_eq!(
            own_file_edits(files, main),
            Err(
                "Organizing the imports would also change /project/shapes.py, so nothing was changed."
                    .to_string()
            )
        );
        assert!(own_file_edits(vec![edits_for("/project/shapes.py")], main).is_err());
    }
}
