//! Commands of the language features of the code editor, in the View, Go and Code menus.
use super::*;
use gpui_kit::component::input::ToggleCodeActions;

actions!(
    tandem,
    [
        InsertSnippet,
        TriggerParameterHints,
        RenameSymbol,
        OrganizeImports,
        ShowOutline
    ]
);

/// The language commands of the View menu, listed after the others there.
pub(super) fn view_specs() -> Vec<CommandSpec> {
    vec![spec::<ShowOutline>(
        "Show Outline",
        MenuId::View,
        "",
        &[],
        KeyScope::Global,
    )]
}

/// The language commands of the Go menu, listed after the others there.
pub(super) fn go_specs() -> Vec<CommandSpec> {
    Vec::new()
}

/// The language commands of the Code menu, listed after the others there.
pub(super) fn code_specs() -> Vec<CommandSpec> {
    use KeyScope::Editor;
    use MenuId::Code;
    vec![
        spec::<InsertSnippet>("Insert Snippet…", Code, "", &[], Editor),
        spec::<TriggerParameterHints>(
            "Trigger Parameter Hints",
            Code,
            "⇧⌘Space",
            &["cmd-shift-space"],
            Editor,
        ),
        spec::<RenameSymbol>("Rename Symbol", Code, "F2", &["f2"], Editor),
        // The toolkit editor handles this action: it opens its code-action menu, which
        // asks the tab's code-action source.
        spec::<ToggleCodeActions>("Quick Fix…", Code, "⌥↵", &["alt-enter"], Editor),
        spec::<OrganizeImports>("Organize Imports", Code, "⇧⌥O", &["shift-alt-o"], Editor),
    ]
}

/// Routes the language commands to the workspace.
pub(super) fn install(cx: &mut App, window: AnyWindowHandle, view: &WeakEntity<Workspace>) {
    route::<InsertSnippet>(cx, window, view, |v, w, c| v.insert_snippet(w, c));
    route::<TriggerParameterHints>(cx, window, view, |v, w, c| v.trigger_signature_help(w, c));
    route::<RenameSymbol>(cx, window, view, |v, w, c| v.rename_symbol(w, c));
    route::<OrganizeImports>(cx, window, view, |v, w, c| v.organize_imports(w, c));
    route::<ShowOutline>(cx, window, view, |v, w, c| v.show_outline(w, c));
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::super::{CommandSpec, KeyScope, MenuId, commands};
    use gpui_kit::component::input::ToggleCodeActions;

    fn command(title: &str) -> CommandSpec {
        commands()
            .into_iter()
            .find(|spec| spec.title == title)
            .unwrap_or_else(|| panic!("{title} is a registry command"))
    }

    #[test]
    fn quick_fix_is_option_enter_in_the_editor_and_opens_the_toolkit_menu() {
        let spec = command("Quick Fix…");
        assert_eq!(spec.menu, MenuId::Code);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "⌥↵");
        assert_eq!(spec.keys, ["alt-enter"]);
        assert_eq!(spec.scope, KeyScope::Editor);
        assert!(spec.action().partial_eq(&ToggleCodeActions));
    }

    #[test]
    fn show_outline_is_a_view_command_without_a_key() {
        let spec = command("Show Outline");
        assert_eq!(spec.menu, MenuId::View);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "");
        assert!(spec.keys.is_empty());
        assert_eq!(spec.scope, KeyScope::Global);
    }

    #[test]
    fn organize_imports_is_shift_option_o_in_the_editor() {
        let spec = command("Organize Imports");
        assert_eq!(spec.menu, MenuId::Code);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "⇧⌥O");
        assert_eq!(spec.keys, ["shift-alt-o"]);
        assert_eq!(spec.scope, KeyScope::Editor);
    }
}
