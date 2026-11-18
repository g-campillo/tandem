//! Editor and workbench commands in the style of VS Code: their actions, shortcuts, menu entries and handlers.
mod center;
mod editing;
mod files;
mod history;
mod language;
mod layout;
mod navigation;
mod selection;

use super::*;
use gpui_kit::component::command::CommandState;
use tandem_core::edit::comment::LineComment;
use tandem_core::edit::lines::Direction;

actions!(
    tandem,
    [
        ShowCommandPalette,
        ShowKeyboardShortcuts,
        ToggleLineComment,
        ToggleBlockComment,
        AddLineComment,
        RemoveLineComment,
        DeleteLine,
        InsertLineBelow,
        InsertLineAbove,
        JoinLines,
        TrimTrailingWhitespace,
        SortLinesAscending,
        SortLinesDescending,
        TransformToUppercase,
        TransformToLowercase,
        CopyLineUp,
        CopyLineDown,
        MoveLineUp,
        MoveLineDown,
        ToggleExplorer,
        ShowExplorer,
        ToggleBottomPanel,
        ToggleWordWrap,
        ToggleFullScreen,
        Unfold,
        UnfoldAll,
        ExpandSelection,
        ShrinkSelection,
        SelectLine,
        AddCursorsToLineEnds,
        AddNextOccurrence,
        SelectAllOccurrences,
        GoToBracket,
        FindNext,
        FindPrevious,
        NextTab,
        PreviousTab,
        NextProblem,
        PreviousProblem,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
        GoBack,
        GoForward,
        GoToLine,
        CloseAllFiles,
        ReopenClosedFile,
        CopyFilePath,
        CopyRelativeFilePath,
        RevealInFinder
    ]
);

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
    /// The submenu `extend_menus` puts it in; consecutive commands with the same name share one.
    pub submenu: Option<&'static str>,
    action: fn() -> Box<dyn Action>,
    bind: fn(&'static str, Option<&'static str>) -> KeyBinding,
}

pub struct CommandsUi {
    /// The palette's state, created the first time the palette opens.
    palette: Option<Entity<CommandState>>,
    /// Whether the file explorer is shown (⌘B).
    pub(super) explorer_visible: bool,
    /// The bottom panel's tab when ⌘J last hid it, so ⌘J reopens that tab.
    last_bottom: BottomTab,
    /// The occurrences ⌘D has selected so far.
    occurrences: Option<selection::OccurrenceSession>,
    /// The selections Expand Selection has passed through.
    expansion: Option<selection::ExpansionSession>,
    /// Where Back and Forward go (⌃-, ⌃⇧-).
    history: history::NavHistory,
    /// The project `epoch` the history belongs to.
    history_epoch: u64,
    /// Set while Back or Forward moves, so that move is not recorded as a jump.
    history_paused: bool,
    /// The Go to Line dialog's input, created the first time the dialog opens.
    line_input: Option<Entity<InputState>>,
    /// Paths of closed tabs, most recent last (⇧⌘T).
    closed: Vec<String>,
}

impl Default for CommandsUi {
    fn default() -> Self {
        Self {
            palette: None,
            explorer_visible: true,
            last_bottom: BottomTab::Terminal,
            occurrences: None,
            expansion: None,
            history: history::NavHistory::default(),
            history_epoch: 0,
            history_paused: false,
            line_input: None,
            closed: Vec::new(),
        }
    }
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
        submenu: None,
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

    fn submenu(mut self, name: &'static str) -> Self {
        self.submenu = Some(name);
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
        language::view_specs(),
        go(),
        language::go_specs(),
        code(),
        language::code_specs(),
        chat(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The shortcut of the registry command titled `title`, as shown to the user. Empty when
/// the command has no shortcut or there is no such command.
pub fn shortcut(title: &str) -> &'static str {
    commands()
        .into_iter()
        .find(|spec| spec.title == title)
        .map_or("", |spec| spec.shortcut)
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
        spec::<CloseAllFiles>("Close All Files", File, "⌘K ⌘W", &["cmd-k cmd-w"], Global)
            .separated(),
        spec::<ReopenClosedFile>("Reopen Closed File", File, "⇧⌘T", &["cmd-shift-t"], Global),
        spec::<CopyFilePath>(
            "Copy Path of Active File",
            File,
            "⌘K P",
            &["cmd-k p"],
            Global,
        )
        .separated(),
        spec::<CopyRelativeFilePath>(
            "Copy Relative Path of Active File",
            File,
            "⇧⌥⌘C",
            &["cmd-alt-shift-c"],
            Global,
        ),
        spec::<RevealInFinder>(
            "Reveal Active File in Finder",
            File,
            "⌘K R",
            &["cmd-k r"],
            Global,
        ),
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
        spec::<FindNext>("Find Next", Edit, "⌘G", &["cmd-g"], Global),
        spec::<FindPrevious>("Find Previous", Edit, "⇧⌘G", &["cmd-shift-g"], Global),
        spec::<Indent>("Indent Line", Edit, "⌘]", &[], Editor).separated(),
        spec::<Outdent>("Outdent Line", Edit, "⌘[", &[], Editor),
        spec::<ToggleLineComment>("Toggle Line Comment", Edit, "⌘/", &["cmd-/"], Editor)
            .separated(),
        spec::<ToggleBlockComment>(
            "Toggle Block Comment",
            Edit,
            "⇧⌥A",
            &["shift-alt-a"],
            Editor,
        ),
        spec::<AddLineComment>("Add Line Comment", Edit, "⌘K ⌘C", &["cmd-k cmd-c"], Editor),
        spec::<RemoveLineComment>(
            "Remove Line Comment",
            Edit,
            "⌘K ⌘U",
            &["cmd-k cmd-u"],
            Editor,
        ),
        spec::<DeleteLine>("Delete Line", Edit, "⇧⌘K", &["cmd-shift-k"], Editor).separated(),
        spec::<InsertLineBelow>("Insert Line Below", Edit, "⌘↵", &["cmd-enter"], Editor),
        spec::<InsertLineAbove>(
            "Insert Line Above",
            Edit,
            "⇧⌘↵",
            &["cmd-shift-enter"],
            Editor,
        ),
        spec::<JoinLines>("Join Lines", Edit, "⌃J", &["ctrl-j"], Editor),
        spec::<TrimTrailingWhitespace>(
            "Trim Trailing Whitespace",
            Edit,
            "⌘K ⌘X",
            &["cmd-k cmd-x"],
            Editor,
        ),
        spec::<SortLinesAscending>("Sort Lines Ascending", Edit, "", &[], Editor).separated(),
        spec::<SortLinesDescending>("Sort Lines Descending", Edit, "", &[], Editor),
        spec::<TransformToUppercase>("Transform to Uppercase", Edit, "", &[], Editor),
        spec::<TransformToLowercase>("Transform to Lowercase", Edit, "", &[], Editor),
    ]
}

fn selection() -> Vec<CommandSpec> {
    use KeyScope::Editor;
    use MenuId::Selection;
    use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
    use gpui_kit::component::input::SelectAll;
    vec![
        spec::<SelectAll>("Select All", Selection, "⌘A", &[], Editor),
        spec::<ExpandSelection>(
            "Expand Selection",
            Selection,
            "⌃⇧→",
            &["ctrl-shift-right", "ctrl-shift-cmd-right"],
            Editor,
        ),
        spec::<ShrinkSelection>(
            "Shrink Selection",
            Selection,
            "⌃⇧←",
            &["ctrl-shift-left", "ctrl-shift-cmd-left"],
            Editor,
        ),
        spec::<SelectLine>("Select Line", Selection, "", &[], Editor),
        spec::<CopyLineUp>("Copy Line Up", Selection, "⇧⌥↑", &["shift-alt-up"], Editor).separated(),
        spec::<CopyLineDown>(
            "Copy Line Down",
            Selection,
            "⇧⌥↓",
            &["shift-alt-down"],
            Editor,
        ),
        spec::<MoveLineUp>("Move Line Up", Selection, "⌥↑", &["alt-up"], Editor),
        spec::<MoveLineDown>("Move Line Down", Selection, "⌥↓", &["alt-down"], Editor),
        spec::<AddCursorAbove>("Add Cursor Above", Selection, "⌥⌘↑", &[], Editor).separated(),
        spec::<AddCursorBelow>("Add Cursor Below", Selection, "⌥⌘↓", &[], Editor),
        spec::<AddCursorsToLineEnds>(
            "Add Cursors to Line Ends",
            Selection,
            "⇧⌥I",
            &["shift-alt-i"],
            Editor,
        ),
        spec::<AddNextOccurrence>("Add Next Occurrence", Selection, "⌘D", &["cmd-d"], Editor),
        spec::<SelectAllOccurrences>(
            "Select All Occurrences",
            Selection,
            "⇧⌘L",
            &["cmd-shift-l"],
            Editor,
        ),
    ]
}

fn view() -> Vec<CommandSpec> {
    use KeyScope::{Editor, Global};
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
        spec::<ToggleChat>("Toggle Chat", View, "⌥⌘B", &[], Global).existing(),
        spec::<QuickOpen>("Quick Open", View, "⌘P", &[], Global).existing(),
        spec::<ProjectSearch>("Search Project", View, "⇧⌘F", &[], Global).existing(),
        spec::<Changes>("Compare with Disk", View, "⇧⌘D", &[], Global).existing(),
        spec::<ToggleTerminal>("Toggle Terminal", View, "⌃`", &[], Global).existing(),
        spec::<ShowProblems>("Show Problems", View, "⇧⌘M", &[], Global).existing(),
        spec::<ZoomIn>("Increase Text Size", View, "⌘=", &[], Global).existing(),
        spec::<ZoomOut>("Decrease Text Size", View, "⌘-", &[], Global).existing(),
        spec::<ToggleExplorer>("Toggle File Explorer", View, "⌘B", &["cmd-b"], Global).separated(),
        spec::<ShowExplorer>("Show Explorer", View, "⇧⌘E", &["cmd-shift-e"], Global),
        spec::<ToggleBottomPanel>("Toggle Panel", View, "⌘J", &["cmd-j"], Global),
        spec::<ToggleWordWrap>("Toggle Word Wrap", View, "⌥Z", &["alt-z"], Editor),
        spec::<ToggleFullScreen>("Toggle Full Screen", View, "⌃⌘F", &["ctrl-cmd-f"], Global),
        spec::<Unfold>("Unfold", View, "⌥⌘]", &["cmd-alt-]"], Editor).separated(),
        spec::<UnfoldAll>("Unfold All", View, "⌘K ⌘J", &["cmd-k cmd-j"], Editor),
    ]
}

fn go() -> Vec<CommandSpec> {
    use KeyScope::{Editor, Global, NotTerminal};
    use MenuId::Go;
    use gpui_kit::component::input::GoToDefinition;
    const SWITCH_TAB: &str = "Switch Tab";
    vec![
        spec::<GoBack>("Back", Go, "⌃-", &["ctrl--"], NotTerminal),
        spec::<GoForward>("Forward", Go, "⌃⇧-", &["ctrl-_"], NotTerminal),
        spec::<GoToLine>("Go to Line…", Go, "⌃G", &["ctrl-g"], NotTerminal).separated(),
        spec::<GoToDefinition>("Go to Definition", Go, "F12", &["f12"], Editor),
        spec::<GoToBracket>("Go to Bracket", Go, "⇧⌘\\", &["cmd-|"], Editor),
        spec::<NextProblem>("Next Problem", Go, "F8", &["f8"], Global).separated(),
        spec::<PreviousProblem>("Previous Problem", Go, "⇧F8", &["shift-f8"], Global),
        spec::<NextTab>(
            "Next Tab",
            Go,
            "⌃Tab",
            &["ctrl-tab", "cmd-alt-right", "cmd-}"],
            Global,
        )
        .separated(),
        spec::<PreviousTab>(
            "Previous Tab",
            Go,
            "⌃⇧Tab",
            &["ctrl-shift-tab", "cmd-alt-left", "cmd-{"],
            Global,
        ),
        spec::<SelectTab1>("Go to Tab 1", Go, "⌘1", &["cmd-1"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab2>("Go to Tab 2", Go, "⌘2", &["cmd-2"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab3>("Go to Tab 3", Go, "⌘3", &["cmd-3"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab4>("Go to Tab 4", Go, "⌘4", &["cmd-4"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab5>("Go to Tab 5", Go, "⌘5", &["cmd-5"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab6>("Go to Tab 6", Go, "⌘6", &["cmd-6"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab7>("Go to Tab 7", Go, "⌘7", &["cmd-7"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab8>("Go to Tab 8", Go, "⌘8", &["cmd-8"], Global).submenu(SWITCH_TAB),
        spec::<SelectTab9>("Go to Tab 9", Go, "⌘9", &["cmd-9"], Global).submenu(SWITCH_TAB),
    ]
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
        // The submenu of the previous command, which the next command joins when it names
        // the same one.
        let mut open_submenu = None;
        for spec in specs.iter().filter(|spec| spec.in_menu && spec.menu == id) {
            let item = MenuItem::Action {
                name: spec.title.into(),
                action: spec.action(),
                os_action: None,
                checked: false,
                disabled: false,
            };
            if spec.submenu.is_some()
                && spec.submenu == open_submenu
                && let Some(MenuItem::Submenu(submenu)) = items.last_mut()
            {
                if spec.separated {
                    submenu.items.push(MenuItem::separator());
                }
                submenu.items.push(item);
                continue;
            }
            if spec.separated {
                items.push(MenuItem::separator());
            }
            items.push(match spec.submenu {
                Some(name) => MenuItem::submenu(Menu::new(name).items([item])),
                None => item,
            });
            open_submenu = spec.submenu;
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
    route::<ShowCommandPalette>(cx, window, &view, |v, w, c| v.open_center(">", w, c));
    route::<ShowKeyboardShortcuts>(cx, window, &view, |v, w, c| v.open_center(">", w, c));
    route::<ToggleLineComment>(cx, window, &view, |v, w, c| {
        v.comment_lines(LineComment::Toggle, w, c)
    });
    route::<AddLineComment>(cx, window, &view, |v, w, c| {
        v.comment_lines(LineComment::Add, w, c)
    });
    route::<RemoveLineComment>(cx, window, &view, |v, w, c| {
        v.comment_lines(LineComment::Remove, w, c)
    });
    route::<ToggleBlockComment>(cx, window, &view, |v, w, c| v.toggle_block_comment(w, c));
    route::<CopyLineUp>(cx, window, &view, |v, w, c| {
        v.duplicate_lines(Direction::Up, w, c)
    });
    route::<CopyLineDown>(cx, window, &view, |v, w, c| {
        v.duplicate_lines(Direction::Down, w, c)
    });
    route::<MoveLineUp>(cx, window, &view, |v, w, c| {
        v.move_lines(Direction::Up, w, c)
    });
    route::<MoveLineDown>(cx, window, &view, |v, w, c| {
        v.move_lines(Direction::Down, w, c)
    });
    route::<DeleteLine>(cx, window, &view, |v, w, c| v.delete_line(w, c));
    route::<InsertLineBelow>(cx, window, &view, |v, w, c| v.insert_line(false, w, c));
    route::<InsertLineAbove>(cx, window, &view, |v, w, c| v.insert_line(true, w, c));
    route::<JoinLines>(cx, window, &view, |v, w, c| v.join_lines(w, c));
    route::<TrimTrailingWhitespace>(cx, window, &view, |v, w, c| {
        v.trim_trailing_whitespace(w, c)
    });
    route::<SortLinesAscending>(cx, window, &view, |v, w, c| v.sort_lines(false, w, c));
    route::<SortLinesDescending>(cx, window, &view, |v, w, c| v.sort_lines(true, w, c));
    route::<TransformToUppercase>(cx, window, &view, |v, w, c| v.transform_case(true, w, c));
    route::<TransformToLowercase>(cx, window, &view, |v, w, c| v.transform_case(false, w, c));
    route::<ToggleExplorer>(cx, window, &view, |v, w, c| v.toggle_explorer(w, c));
    route::<ShowExplorer>(cx, window, &view, |v, w, c| v.show_explorer(w, c));
    route::<ToggleBottomPanel>(cx, window, &view, |v, _, c| v.toggle_bottom_panel(c));
    route::<ToggleWordWrap>(cx, window, &view, |v, w, c| v.toggle_word_wrap(w, c));
    route::<ToggleFullScreen>(cx, window, &view, |_, w, _| w.toggle_fullscreen());
    route::<Unfold>(cx, window, &view, |v, w, c| v.unfold(w, c));
    route::<UnfoldAll>(cx, window, &view, |v, w, c| v.unfold_all(w, c));
    route::<ExpandSelection>(cx, window, &view, |v, w, c| v.expand_selection(w, c));
    route::<ShrinkSelection>(cx, window, &view, |v, w, c| v.shrink_selection(w, c));
    route::<SelectLine>(cx, window, &view, |v, w, c| v.select_line(w, c));
    route::<AddCursorsToLineEnds>(cx, window, &view, |v, w, c| {
        v.add_cursors_to_line_ends(w, c)
    });
    route::<AddNextOccurrence>(cx, window, &view, |v, w, c| v.add_next_occurrence(w, c));
    route::<SelectAllOccurrences>(cx, window, &view, |v, w, c| v.select_all_occurrences(w, c));
    route::<GoToBracket>(cx, window, &view, |v, w, c| v.go_to_bracket(w, c));
    route::<FindNext>(cx, window, &view, |v, _, c| v.find_match(true, c));
    route::<FindPrevious>(cx, window, &view, |v, _, c| v.find_match(false, c));
    route::<NextTab>(cx, window, &view, |v, w, c| v.cycle_tab(true, w, c));
    route::<PreviousTab>(cx, window, &view, |v, w, c| v.cycle_tab(false, w, c));
    route::<SelectTab1>(cx, window, &view, |v, w, c| v.select_tab_number(1, w, c));
    route::<SelectTab2>(cx, window, &view, |v, w, c| v.select_tab_number(2, w, c));
    route::<SelectTab3>(cx, window, &view, |v, w, c| v.select_tab_number(3, w, c));
    route::<SelectTab4>(cx, window, &view, |v, w, c| v.select_tab_number(4, w, c));
    route::<SelectTab5>(cx, window, &view, |v, w, c| v.select_tab_number(5, w, c));
    route::<SelectTab6>(cx, window, &view, |v, w, c| v.select_tab_number(6, w, c));
    route::<SelectTab7>(cx, window, &view, |v, w, c| v.select_tab_number(7, w, c));
    route::<SelectTab8>(cx, window, &view, |v, w, c| v.select_tab_number(8, w, c));
    route::<SelectTab9>(cx, window, &view, |v, w, c| v.select_tab_number(9, w, c));
    route::<NextProblem>(cx, window, &view, |v, w, c| v.go_to_problem(true, w, c));
    route::<PreviousProblem>(cx, window, &view, |v, w, c| v.go_to_problem(false, w, c));
    route::<GoBack>(cx, window, &view, |v, w, c| v.go_history(false, w, c));
    route::<GoForward>(cx, window, &view, |v, w, c| v.go_history(true, w, c));
    route::<GoToLine>(cx, window, &view, |v, w, c| v.open_go_to_line(w, c));
    route::<CloseAllFiles>(cx, window, &view, |v, w, c| v.close_all_files(w, c));
    route::<ReopenClosedFile>(cx, window, &view, |v, w, c| v.reopen_closed_file(w, c));
    route::<CopyFilePath>(cx, window, &view, |v, _, c| v.copy_file_path(false, c));
    route::<CopyRelativeFilePath>(cx, window, &view, |v, _, c| v.copy_file_path(true, c));
    route::<RevealInFinder>(cx, window, &view, |v, _, c| v.reveal_in_finder(c));
    language::install(cx, window, &view);
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
        // A shortcut or menu command used while another window (Settings) is in front is
        // not for the editor's window.
        if cx.active_window().is_some_and(|active| active != window) {
            return;
        }
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
    use super::{KeyScope, MenuId, commands, extend_menus, key_bindings, shortcut};
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
        "cmd-alt-b",
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
    fn comment_commands_are_a_separate_group_in_edit() {
        let menus = extend_menus(vec![Menu::new("Edit")]);
        let edit = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Edit")
            .unwrap();
        let position = |title: &str| {
            edit.items
                .iter()
                .position(
                    |item| matches!(item, MenuItem::Action { name, .. } if name.as_ref() == title),
                )
                .unwrap_or_else(|| panic!("{title} is not in Edit"))
        };
        let toggle = position("Toggle Line Comment");
        assert!(toggle > 0);
        assert!(matches!(edit.items[toggle - 1], MenuItem::Separator));
        assert_eq!(position("Toggle Block Comment"), toggle + 1);
        assert_eq!(position("Add Line Comment"), toggle + 2);
        assert_eq!(position("Remove Line Comment"), toggle + 3);
    }

    #[test]
    fn line_commands_are_in_edit_and_selection() {
        let menus = extend_menus(Vec::new());
        let items = |title: &str| {
            let menu = menus
                .iter()
                .find(|menu| menu.name.as_ref() == title)
                .unwrap_or_else(|| panic!("there is no {title} menu"));
            action_names(&menu.items)
        };
        assert_eq!(
            items("Selection"),
            [
                "Select All",
                "Expand Selection",
                "Shrink Selection",
                "Select Line",
                "Copy Line Up",
                "Copy Line Down",
                "Move Line Up",
                "Move Line Down",
                "Add Cursor Above",
                "Add Cursor Below",
                "Add Cursors to Line Ends",
                "Add Next Occurrence",
                "Select All Occurrences"
            ]
        );
        let edit = items("Edit");
        for title in [
            "Delete Line",
            "Insert Line Below",
            "Insert Line Above",
            "Join Lines",
            "Trim Trailing Whitespace",
            "Sort Lines Ascending",
            "Sort Lines Descending",
            "Transform to Uppercase",
            "Transform to Lowercase",
        ] {
            assert!(
                edit.iter().any(|name| name == title),
                "{title} is not in Edit"
            );
        }
    }

    #[test]
    fn layout_commands_are_in_view() {
        let menus = extend_menus(Vec::new());
        let view = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "View")
            .unwrap();
        let names = action_names(&view.items);
        for title in ["Toggle File Explorer", "Toggle Panel", "Unfold All"] {
            assert!(
                names.iter().any(|name| name == title),
                "{title} is not in View"
            );
        }
    }

    #[test]
    fn selection_commands_are_in_selection_and_go() {
        let menus = extend_menus(Vec::new());
        let items = |title: &str| {
            let menu = menus
                .iter()
                .find(|menu| menu.name.as_ref() == title)
                .unwrap_or_else(|| panic!("there is no {title} menu"));
            action_names(&menu.items)
        };
        assert!(
            items("Go").iter().any(|name| name == "Go to Bracket"),
            "Go to Bracket is not in Go"
        );
        assert!(
            items("Selection")
                .iter()
                .any(|name| name == "Select All Occurrences"),
            "Select All Occurrences is not in Selection"
        );
    }

    #[test]
    fn go_menu_has_tab_commands_and_a_switch_tab_submenu() {
        let menus = extend_menus(Vec::new());
        let go = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Go")
            .unwrap();
        let submenus: Vec<&Menu> = go
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Submenu(menu) => Some(menu),
                _ => None,
            })
            .collect();
        assert_eq!(submenus.len(), 1);
        assert_eq!(submenus[0].name.as_ref(), "Switch Tab");
        let expected: Vec<String> = (1..=9).map(|n| format!("Go to Tab {n}")).collect();
        assert_eq!(action_names(&submenus[0].items), expected);
        let next_tab = go
            .items
            .iter()
            .position(
                |item| matches!(item, MenuItem::Action { name, .. } if name.as_ref() == "Next Tab"),
            )
            .expect("Next Tab is not in Go");
        let switch_tab = go
            .items
            .iter()
            .position(|item| matches!(item, MenuItem::Submenu(_)))
            .unwrap();
        assert!(next_tab < switch_tab);
        let names = action_names(&go.items);
        for title in ["Next Problem", "Previous Problem", "Previous Tab"] {
            assert!(
                names.iter().any(|name| name == title),
                "{title} is not in Go"
            );
        }
    }

    #[test]
    fn go_menu_starts_with_back_forward_and_go_to_line() {
        let menus = extend_menus(Vec::new());
        let go = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Go")
            .unwrap();
        assert_eq!(
            action_names(&go.items)[..3],
            ["Back", "Forward", "Go to Line…"]
        );
        assert!(matches!(go.items[2], MenuItem::Separator));
    }

    #[test]
    fn keys_the_shell_needs_are_disabled_in_the_terminal() {
        let bindings = key_bindings();
        for key in ["ctrl-g", "ctrl--", "ctrl-_", "ctrl-r"] {
            let keystroke = Keystroke::parse(key).unwrap();
            assert!(
                bindings.iter().any(|binding| {
                    gpui_kit::is_no_action(binding.action())
                        && binding.match_keystrokes(std::slice::from_ref(&keystroke)) == Some(false)
                        && binding.predicate().map(|predicate| predicate.to_string())
                            == Some("Terminal".into())
                }),
                "{key} is not disabled in the terminal"
            );
        }
    }

    #[test]
    fn find_next_and_previous_follow_replace_in_edit() {
        let menus = extend_menus(vec![
            Menu::new("Edit").items([MenuItem::action("Replace in File", super::ReplaceInFile)]),
        ]);
        let edit = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Edit")
            .unwrap();
        assert_eq!(
            action_names(&edit.items)[..3],
            ["Replace in File", "Find Next", "Find Previous"]
        );
        assert!(matches!(edit.items[1], MenuItem::Action { .. }));
    }

    #[test]
    fn file_commands_follow_close_file_in_two_groups() {
        let menus = extend_menus(vec![
            Menu::new("File").items([MenuItem::action("Close File", super::CloseFile)]),
        ]);
        let file = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "File")
            .unwrap();
        assert_eq!(
            action_names(&file.items),
            [
                "Close File",
                "Close All Files",
                "Reopen Closed File",
                "Copy Path of Active File",
                "Copy Relative Path of Active File",
                "Reveal Active File in Finder"
            ]
        );
        assert!(matches!(file.items[1], MenuItem::Separator));
        assert!(
            matches!(&file.items[2], MenuItem::Action { name, .. } if name.as_ref() == "Close All Files")
        );
        assert!(matches!(file.items[4], MenuItem::Separator));
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

    #[test]
    fn insert_snippet_is_in_the_code_menu_without_a_key() {
        let spec = commands()
            .into_iter()
            .find(|spec| spec.title == "Insert Snippet…")
            .expect("Insert Snippet… is a registry command");
        assert_eq!(spec.menu, MenuId::Code);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "");
        assert!(spec.keys.is_empty());
        let menus = extend_menus(Vec::new());
        let code = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Code")
            .unwrap();
        assert!(
            action_names(&code.items)
                .iter()
                .any(|name| name == "Insert Snippet…"),
            "Insert Snippet… is not in Code"
        );
    }

    #[test]
    fn trigger_parameter_hints_is_shift_cmd_space_in_the_editor() {
        let spec = commands()
            .into_iter()
            .find(|spec| spec.title == "Trigger Parameter Hints")
            .expect("Trigger Parameter Hints is a registry command");
        assert_eq!(spec.menu, MenuId::Code);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "⇧⌘Space");
        assert_eq!(spec.keys, ["cmd-shift-space"]);
        assert_eq!(spec.scope, KeyScope::Editor);
    }

    #[test]
    fn rename_symbol_is_f2_in_the_editor() {
        let spec = commands()
            .into_iter()
            .find(|spec| spec.title == "Rename Symbol")
            .expect("Rename Symbol is a registry command");
        assert_eq!(spec.menu, MenuId::Code);
        assert!(spec.in_menu);
        assert_eq!(spec.shortcut, "F2");
        assert_eq!(spec.keys, ["f2"]);
        assert_eq!(spec.scope, KeyScope::Editor);
    }

    #[test]
    fn shortcut_finds_a_command_by_its_title() {
        assert_eq!(shortcut("Quick Open"), "⌘P");
        assert_eq!(shortcut("Toggle Panel"), "⌘J");
        assert_eq!(shortcut("New Task"), "");
        assert_eq!(shortcut("No Such Command"), "");
    }

    #[test]
    fn every_command_the_chrome_names_is_in_the_registry() {
        use crate::workspace::chrome_logic::Activity;
        let titles: HashSet<&str> = commands().iter().map(|spec| spec.title).collect();
        let named = [
            "Quick Open",
            "Command Palette…",
            "Toggle File Explorer",
            "Toggle Panel",
            "Toggle Chat",
            "New File",
            "Save",
            "Compare with Disk",
            "Close File",
            "Go to Line…",
            "Stop Claude",
        ]
        .into_iter()
        .chain(
            Activity::TOP
                .into_iter()
                .chain([Activity::Settings])
                .map(Activity::command),
        );
        for title in named {
            assert!(titles.contains(title), "{title} is not a registry command");
        }
    }
}
