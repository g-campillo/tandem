//! Commands of the language features of the code editor, in the View, Go and Code menus.
use super::*;

actions!(tandem, [InsertSnippet]);

/// The language commands of the View menu, listed after the others there.
pub(super) fn view_specs() -> Vec<CommandSpec> {
    Vec::new()
}

/// The language commands of the Go menu, listed after the others there.
pub(super) fn go_specs() -> Vec<CommandSpec> {
    Vec::new()
}

/// The language commands of the Code menu, listed after the others there.
pub(super) fn code_specs() -> Vec<CommandSpec> {
    use KeyScope::Editor;
    use MenuId::Code;
    vec![spec::<InsertSnippet>(
        "Insert Snippet…",
        Code,
        "",
        &[],
        Editor,
    )]
}

/// Routes the language commands to the workspace.
pub(super) fn install(cx: &mut App, window: AnyWindowHandle, view: &WeakEntity<Workspace>) {
    route::<InsertSnippet>(cx, window, view, |v, w, c| v.insert_snippet(w, c));
}
