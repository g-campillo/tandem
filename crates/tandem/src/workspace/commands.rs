//! Editor and workbench commands in the style of VS Code: their actions, shortcuts, menu entries and handlers.
mod palette;

use super::*;
use gpui_kit::component::command::CommandState;

actions!(tandem, [ShowCommandPalette, ShowKeyboardShortcuts]);

/// The key context of a code editor that has focus.
const EDITOR_CONTEXT: &str = "Input && mode == editor";

/// Where a command's shortcut applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyScope {
    /// Everywhere.
    Global,
    /// Everywhere except while the terminal has focus.
    NotTerminal,
    /// While a code editor has focus.
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuId {
    App,
    File,
    Edit,
    Selection,
    View,
    Go,
    Code,
    Chat,
}

impl MenuId {
    pub fn title(self) -> &'static str {
        match self {
            MenuId::App => "Tandem",
            MenuId::File => "File",
            MenuId::Edit => "Edit",
            MenuId::Selection => "Selection",
            MenuId::View => "View",
            MenuId::Go => "Go",
            MenuId::Code => "Code",
            MenuId::Chat => "Chat",
        }
    }

    /// The existing menu a new menu is inserted after.
    pub fn anchor(self) -> Option<&'static str> {
        match self {
            MenuId::Selection => Some("Edit"),
            MenuId::Go => Some("View"),
            _ => None,
        }
    }
}

pub struct CommandSpec {
    pub title: &'static str,
    pub menu: MenuId,
    /// The shortcut as shown to the user, e.g. "⇧⌘K" or "⌘K ⌘C"; empty when there is none.
    pub shortcut: &'static str,
    /// Keystrokes this module binds, the primary first. Empty when the command has no
    /// shortcut or is bound in main.rs or by the toolkit.
    pub keys: &'static [&'static str],
    pub scope: KeyScope,
    /// Whether `extend_menus` adds the command to its menu.
    pub in_menu: bool,
    /// Whether `extend_menus` puts a separator before it.
    pub separated: bool,
    action: fn() -> Box<dyn Action>,
    bind: fn(&'static str, Option<&'static str>) -> KeyBinding,
}

#[derive(Default)]
pub struct CommandsUi {
    /// The palette's state, created the first time the palette opens.
    palette: Option<Entity<CommandState>>,
}

fn spec<A: Action + Default>(
    title: &'static str,
    menu: MenuId,
    shortcut: &'static str,
    keys: &'static [&'static str],
    scope: KeyScope,
) -> CommandSpec {
    CommandSpec {
        title,
        menu,
        shortcut,
        keys,
        scope,
        in_menu: true,
        separated: false,
        action: || Box::new(A::default()),
        bind: |keys, context| KeyBinding::new(keys, A::default(), context),
    }
}

impl CommandSpec {
    /// Already in main.rs's menus.
    fn existing(mut self) -> Self {
        self.in_menu = false;
        self
    }

    fn separated(mut self) -> Self {
        self.separated = true;
        self
    }

    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }
}

/// Every command Tandem lists in the palette, in menu order.
pub fn commands() -> Vec<CommandSpec> {
    [
        app(),
        file(),
        edit(),
        selection(),
        view(),
        go(),
        code(),
        chat(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn app() -> Vec<CommandSpec> {
    use KeyScope::Global;
    use MenuId::App;
    vec![
        spec::<Settings>("Settings…", App, "⌘,", &[], Global).existing(),
        spec::<Quit>("Quit Tandem", App, "⌘Q", &[], Global).existing(),
    ]
}

fn file() -> Vec<CommandSpec> {
    use KeyScope::{Global, NotTerminal};
    use MenuId::File;
    vec![
        spec::<OpenProject>("Open Folder…", File, "⌘O", &[], Global).existing(),
        spec::<ShowRecent>("Recent Projects", File, "⌃R", &["ctrl-r"], NotTerminal).existing(),
        spec::<NewFile>("New File", File, "⌘N", &[], Global).existing(),
        spec::<Save>("Save", File, "⌘S", &[], Global).existing(),
        spec::<SaveAll>("Save All", File, "⌥⌘S", &[], Global).existing(),
        spec::<SaveCopy>("Save Copy…", File, "⇧⌘S", &[], Global).existing(),
        spec::<CloseFile>("Close File", File, "⌘W", &[], Global).existing(),
    ]
}

fn edit() -> Vec<CommandSpec> {
    use KeyScope::{Editor, Global};
    use MenuId::Edit;
    use gpui_kit::component::input::{Cut, Indent, Outdent, Paste, Redo, Undo};
    vec![
        spec::<Undo>("Undo", Edit, "⌘Z", &[], Editor).existing(),
        spec::<Redo>("Redo", Edit, "⇧⌘Z", &[], Editor).existing(),
        spec::<Cut>("Cut", Edit, "⌘X", &[], Editor).existing(),
        spec::<gpui_kit::component::input::Copy>("Copy", Edit, "⌘C", &[], Editor).existing(),
        spec::<Paste>("Paste", Edit, "⌘V", &[], Editor).existing(),
        spec::<FindInFile>("Find in File", Edit, "⌘F", &[], Global).existing(),
        spec::<ReplaceInFile>("Replace in File", Edit, "⌥⌘F", &[], Global).existing(),
        spec::<Indent>("Indent Line", Edit, "⌘]", &[], Editor).separated(),
        spec::<Outdent>("Outdent Line", Edit, "⌘[", &[], Editor),
    ]
}

fn selection() -> Vec<CommandSpec> {
    use KeyScope::Editor;
    use MenuId::Selection;
    use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
    use gpui_kit::component::input::SelectAll;
    vec![
        spec::<SelectAll>("Select All", Selection, "⌘A", &[], Editor),
        spec::<AddCursorAbove>("Add Cursor Above", Selection, "⌥⌘↑", &[], Editor).separated(),
        spec::<AddCursorBelow>("Add Cursor Below", Selection, "⌥⌘↓", &[], Editor),
    ]
}

fn view() -> Vec<CommandSpec> {
    use KeyScope::Global;
    use MenuId::View;
    vec![
        spec::<ShowCommandPalette>(
            "Command Palette…",
            View,
            "⇧⌘P",
            &["cmd-shift-p", "f1"],
            Global,
        )
        .separated(),
        spec::<ShowKeyboardShortcuts>(
            "Keyboard Shortcuts",
            View,
            "⌘K ⌘S",
            &["cmd-k cmd-s"],
            Global,
        ),
        spec::<ToggleChat>("Toggle Chat", View, "⇧⌘L", &[], Global).existing(),
        spec::<QuickOpen>("Quick Open", View, "⌘P", &[], Global).existing(),
        spec::<ProjectSearch>("Search Project", View, "⇧⌘F", &[], Global).existing(),
        spec::<Changes>("Compare with Disk", View, "⇧⌘D", &[], Global).existing(),
        spec::<ToggleTerminal>("Toggle Terminal", View, "⌃`", &[], Global).existing(),
        spec::<ShowProblems>("Show Problems", View, "⇧⌘M", &[], Global).existing(),
        spec::<ZoomIn>("Increase Text Size", View, "⌘=", &[], Global).existing(),
        spec::<ZoomOut>("Decrease Text Size", View, "⌘-", &[], Global).existing(),
    ]
}

fn go() -> Vec<CommandSpec> {
    use gpui_kit::component::input::GoToDefinition;
    vec![spec::<GoToDefinition>(
        "Go to Definition",
        MenuId::Go,
        "F12",
        &["f12"],
        KeyScope::Editor,
    )]
}

fn code() -> Vec<CommandSpec> {
    use KeyScope::{Editor, Global};
    use MenuId::Code;
    use gpui_kit::component::input::ShowCompletions;
    vec![
        spec::<FormatFile>("Format Buffer", Code, "⇧⌥F", &[], Global).existing(),
        spec::<References>("References at Cursor", Code, "⇧F12", &[], Global).existing(),
        spec::<Symbols>("Document Symbols", Code, "⇧⌘O", &[], Global).existing(),
        spec::<ShowCompletions>("Trigger Suggest", Code, "⌃Space", &["ctrl-space"], Editor)
            .separated(),
    ]
}

fn chat() -> Vec<CommandSpec> {
    use KeyScope::Global;
    use MenuId::Chat;
    vec![
        spec::<FocusChat>("Focus Chat", Chat, "⌘L", &[], Global).existing(),
        spec::<NewTask>("New Task", Chat, "", &[], Global).existing(),
        spec::<StopClaude>("Stop Claude", Chat, "⌘.", &[], Global).existing(),
        spec::<CheckMyWork>("Check My Work", Chat, "⌥⌘C", &[], Global).existing(),
    ]
}

/// The key bindings of every registry command, in registry order.
pub fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for spec in commands() {
        for &key in spec.keys {
            match spec.scope {
                KeyScope::Global => bindings.push((spec.bind)(key, None)),
                KeyScope::Editor => bindings.push((spec.bind)(key, Some(EDITOR_CONTEXT))),
                KeyScope::NotTerminal => {
                    bindings.push((spec.bind)(key, None));
                    // The terminal view's key context is `Terminal`: there the key goes to
                    // the shell.
                    bindings.push(KeyBinding::new(key, NoAction, Some("Terminal")));
                }
            }
        }
    }
    bindings
}

/// Adds the registry's menu entries to `menus`, creating the menus that do not exist yet.
pub fn extend_menus(mut menus: Vec<Menu>) -> Vec<Menu> {
    let specs = commands();
    // Selection and Go come after their anchors, Edit and View.
    for id in [
        MenuId::File,
        MenuId::Edit,
        MenuId::Selection,
        MenuId::View,
        MenuId::Go,
        MenuId::Code,
        MenuId::Chat,
        MenuId::App,
    ] {
        let mut items = Vec::new();
        for spec in specs.iter().filter(|spec| spec.in_menu && spec.menu == id) {
            if spec.separated {
                items.push(MenuItem::separator());
            }
            items.push(MenuItem::Action {
                name: spec.title.into(),
                action: spec.action(),
                os_action: None,
                checked: false,
                disabled: false,
            });
        }
        if items.is_empty() {
            continue;
        }
        if let Some(menu) = menus
            .iter_mut()
            .find(|menu| menu.name.as_ref() == id.title())
        {
            menu.items.extend(items);
        } else {
            let at = id
                .anchor()
                .and_then(|anchor| menus.iter().position(|menu| menu.name.as_ref() == anchor))
                .map_or(menus.len(), |index| index + 1);
            menus.insert(at, Menu::new(id.title()).items(items));
        }
    }
    menus
}

/// Routes the registry's own actions to the workspace, so they work from keys, menus and
/// the palette.
pub fn install(cx: &mut App, window: AnyWindowHandle, view: WeakEntity<Workspace>) {
    route::<ShowCommandPalette>(cx, window, &view, |v, w, c| v.open_palette(w, c));
    route::<ShowKeyboardShortcuts>(cx, window, &view, |v, w, c| v.open_palette(w, c));
}

/// The pattern of `install_actions`: dispatch runs while GPUI has borrowed the window, so
/// the handler is deferred until that borrow is released.
fn route<A: Action>(
    cx: &mut App,
    window: AnyWindowHandle,
    view: &WeakEntity<Workspace>,
    handler: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
) {
    let view = view.clone();
    cx.on_action(move |_: &A, cx| {
        let view = view.clone();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                view.update(cx, |view, cx| handler(view, window, cx))
            });
        });
    });
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{KeyScope, MenuId, commands, extend_menus, key_bindings};
    use gpui_kit::{Keystroke, Menu, MenuItem};
    use std::collections::HashSet;

    /// The keys main.rs binds itself.
    const MAIN_KEYS: &[&str] = &[
        "cmd-,",
        "cmd-o",
        "cmd-s",
        "cmd-alt-s",
        "cmd-shift-s",
        "cmd-p",
        "cmd-shift-f",
        "cmd-f",
        "cmd-alt-f",
        "cmd-n",
        "cmd-w",
        "cmd-shift-d",
        "cmd-=",
        "cmd--",
        "ctrl-`",
        "cmd-q",
        "cmd-shift-l",
        "cmd-l",
        "cmd-shift-m",
        "alt-shift-f",
        "shift-f12",
        "cmd-shift-o",
        "cmd-.",
        "cmd-alt-c",
    ];

    fn parse(sequence: &'static str) -> Vec<Keystroke> {
        sequence
            .split_whitespace()
            .map(|part| Keystroke::parse(part).unwrap())
            .collect()
    }

    /// The name of every action item, descending into submenus.
    fn action_names(items: &[MenuItem]) -> Vec<String> {
        let mut names = Vec::new();
        for item in items {
            match item {
                MenuItem::Action { name, .. } => names.push(name.to_string()),
                MenuItem::Submenu(menu) => names.extend(action_names(&menu.items)),
                _ => {}
            }
        }
        names
    }

    fn menu_names(menus: &[Menu]) -> Vec<String> {
        menus.iter().map(|menu| menu.name.to_string()).collect()
    }

    #[test]
    fn key_bindings_has_one_binding_per_key_and_a_terminal_override_per_not_terminal_key() {
        let expected: usize = commands()
            .iter()
            .map(|spec| match spec.scope {
                KeyScope::NotTerminal => spec.keys.len() * 2,
                KeyScope::Global | KeyScope::Editor => spec.keys.len(),
            })
            .sum();
        let bindings = key_bindings();
        assert!(expected > 0);
        assert_eq!(bindings.len(), expected);
        let disabled = bindings
            .iter()
            .filter(|binding| gpui_kit::is_no_action(binding.action()))
            .count();
        let not_terminal: usize = commands()
            .iter()
            .filter(|spec| spec.scope == KeyScope::NotTerminal)
            .map(|spec| spec.keys.len())
            .sum();
        assert_eq!(disabled, not_terminal);
    }

    #[test]
    fn titles_are_unique_and_every_bound_command_shows_its_shortcut() {
        let specs = commands();
        let mut titles = HashSet::new();
        for spec in &specs {
            assert!(!spec.title.is_empty());
            assert!(titles.insert(spec.title), "{} is listed twice", spec.title);
            if !spec.keys.is_empty() {
                assert!(
                    !spec.shortcut.is_empty(),
                    "{} has no shortcut label",
                    spec.title
                );
            }
        }
    }

    #[test]
    fn registry_keys_clash_neither_with_each_other_nor_with_main_rs() {
        let registry: Vec<(&str, Vec<Keystroke>)> = commands()
            .iter()
            .flat_map(|spec| spec.keys.iter().map(|&key| (key, parse(key))))
            .collect();
        let main: Vec<(&str, Vec<Keystroke>)> =
            MAIN_KEYS.iter().map(|&key| (key, parse(key))).collect();
        for (index, (key, sequence)) in registry.iter().enumerate() {
            for (other_index, (other, other_sequence)) in registry.iter().enumerate() {
                if index == other_index {
                    continue;
                }
                assert_ne!(sequence, other_sequence, "{key} is bound twice");
                assert!(
                    !other_sequence.starts_with(sequence),
                    "{key} is a prefix of {other}"
                );
            }
            for (main_key, main_sequence) in &main {
                assert_ne!(sequence, main_sequence, "{key} is already bound in main.rs");
                assert!(
                    !sequence.starts_with(main_sequence),
                    "{key} starts with {main_key}, which main.rs binds"
                );
            }
        }
    }

    #[test]
    fn extend_menus_adds_every_new_command_once_to_its_own_menu() {
        let menus = extend_menus(vec![
            Menu::new("Tandem"),
            Menu::new("File"),
            Menu::new("Edit"),
            Menu::new("View"),
            Menu::new("Code"),
            Menu::new("Chat"),
        ]);
        assert_eq!(
            menu_names(&menus),
            [
                "Tandem",
                "File",
                "Edit",
                "Selection",
                "View",
                "Go",
                "Code",
                "Chat"
            ]
        );
        for spec in commands() {
            let found: Vec<&Menu> = menus
                .iter()
                .filter(|menu| action_names(&menu.items).contains(&spec.title.to_string()))
                .collect();
            if spec.in_menu {
                assert_eq!(found.len(), 1, "{} is not in exactly one menu", spec.title);
                assert_eq!(found[0].name.as_ref(), spec.menu.title(), "{}", spec.title);
                let count = action_names(&found[0].items)
                    .iter()
                    .filter(|name| *name == spec.title)
                    .count();
                assert_eq!(count, 1, "{} is listed {count} times", spec.title);
            } else {
                assert!(
                    found.is_empty(),
                    "{} is already in main.rs's menus",
                    spec.title
                );
            }
        }
        let view = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "View")
            .unwrap();
        let palette = view
            .items
            .iter()
            .position(
                |item| matches!(item, MenuItem::Action { name, .. } if name.as_ref() == "Command Palette…"),
            )
            .unwrap();
        assert!(palette > 0);
        assert!(matches!(view.items[palette - 1], MenuItem::Separator));
    }

    #[test]
    fn extend_menus_creates_only_menus_with_entries() {
        let menus = extend_menus(Vec::new());
        let expected: Vec<&str> = [
            MenuId::App,
            MenuId::File,
            MenuId::Edit,
            MenuId::Selection,
            MenuId::View,
            MenuId::Go,
            MenuId::Code,
            MenuId::Chat,
        ]
        .into_iter()
        .filter(|id| {
            commands()
                .iter()
                .any(|spec| spec.in_menu && spec.menu == *id)
        })
        .map(MenuId::title)
        .collect();
        assert!(!expected.is_empty());
        let mut names = menu_names(&menus);
        names.sort();
        let mut expected: Vec<String> = expected.into_iter().map(String::from).collect();
        expected.sort();
        assert_eq!(names, expected);
        assert!(
            menus
                .iter()
                .all(|menu| !action_names(&menu.items).is_empty())
        );
    }
}
