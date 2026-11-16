//! The Settings view shown in place of the editor: Claude, help levels, language tools,
//! and app and recovery information.
use super::chat_logic::help_languages;
use super::chat_view::{help_level_menu, permission_menu};
use super::panels::{code_note, note};
use super::*;
use crate::appearance::{
    palette, panel_body, panel_error, panel_heading, panel_metadata, panel_warning, path_button,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_claude::{Connection, HelperInstall, view};

const USE_MY_SETUP_NOTE: &str = "On: Claude loads your Claude Code settings, CLAUDE.md files, MCP servers, skills and plugins, and this project's .claude folder, exactly as running claude in this folder would. Turn it off for projects you do not trust.";
const PERMISSIONS_NOTE: &str = "Ask: Claude asks before a command Tandem does not allow by itself. Auto: Claude Code's safety check decides and asks only about risky actions. Bypass: nothing asks, and Claude may also edit project files outside a takeover. Outside a takeover there is no snapshot, so Undo cannot reverse what a command changed or, in Bypass, what Claude edited. Switching to or from Bypass reopens the Claude session and waits until Claude is idle.";
const SUBSCRIPTION_NOTE: &str = "Tandem uses your Claude subscription through Claude Code. It never asks for or stores an API key.";
const HELP_LEVEL_NOTE: &str = "Every level explains why. Beginner gives the full code, explains each part when you point at it, and takes you to the line; Intermediate gives pseudocode and says where the change goes; Expert gives a short description and says where.";
const LAYOUT_NOTE: &str = "Drag the divider between the editor and the file tree or the chat to resize that pane; double-click the divider for its default width. View → Swap File Tree and Chat does the same as this switch.";
const LIMITS_NOTE: &str = "Local ad hoc macOS build. Not notarized; other Macs and Windows are untested. UTF-8 files ≤2 MiB; 32 open tabs. Buffer undo is per session.";

#[derive(Default)]
pub(super) struct SettingsUi {
    pub open: bool,
    pub busy: bool,
    pub lines: Vec<String>,
    /// The app bundle's `Contents/Resources` folder when the helper is bundled.
    pub resources: Option<PathBuf>,
    pub scroll: ScrollHandle,
}

/// What `tandem --runtime-status` prints: the Claude helper and the Node.js that would run it.
pub fn runtime_status() -> Result<String> {
    runtime_line(&tandem_claude::discover_helper()?)
}

fn runtime_line(helper: &HelperInstall) -> Result<String> {
    let home = std::env::home_dir().unwrap_or_default();
    let node = tandem_claude::discover_node(std::env::var_os("PATH").as_deref(), &home)?;
    Ok(format!(
        "Claude helper {} · {} · Node {} · {}",
        if helper.bundled {
            "bundled"
        } else {
            "development"
        },
        helper.entry.display(),
        node.version,
        node.path.display()
    ))
}

/// The status lines at the top of "App and recovery", and the bundle resources folder.
fn settings_lines() -> (Vec<String>, Option<PathBuf>) {
    let mut lines = vec![format!(
        "Tandem {} · macOS Apple Silicon",
        env!("CARGO_PKG_VERSION")
    )];
    let (status, resources) = match tandem_claude::discover_helper() {
        Ok(helper) => (runtime_line(&helper), helper.resources.clone()),
        Err(error) => (Err(error), None),
    };
    match status {
        Ok(line) => lines.push(line),
        Err(error) => {
            lines.push(format!("{error:#}"));
            lines.push("Editing, saving and the terminal still work.".into());
        }
    }
    if let Some(line) = resources.as_deref().and_then(release_line) {
        lines.push(line);
    }
    (lines, resources)
}

/// The largest release manifest Settings reads; it matches `MANIFEST_LIMIT` in
/// `scripts/macos_release.py`. Listing the bundled helper's files makes it about 1.5 MB.
const RELEASE_MANIFEST_LIMIT: usize = 8 * 1024 * 1024;

/// "Release: <id>" from the bundle's release manifest, when there is one.
fn release_line(resources: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(resources.join("release-manifest.json")).ok()?;
    if bytes.len() > RELEASE_MANIFEST_LIMIT {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    Some(format!(
        "Release: {}",
        value["release_id"].as_str().unwrap_or("development")
    ))
}

impl Workspace {
    /// Shows Settings from the top, with freshly checked status lines.
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.open = true;
        self.settings.scroll.set_offset(point(px(0.0), px(0.0)));
        self.refresh_settings(cx);
    }

    pub(super) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.open {
            self.settings.open = false;
            cx.notify();
        } else {
            self.open_settings(cx);
        }
    }

    pub(super) fn refresh_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.busy {
            return;
        }
        self.settings.busy = true;
        let operation = cx.background_spawn(async move { settings_lines() });
        cx.spawn(async move |view, cx| {
            let (lines, resources) = operation.await;
            let _ = view.update(cx, |view, cx| {
                view.settings.busy = false;
                view.settings.lines = lines;
                view.settings.resources = resources;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show_local_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Err(error) = std::process::Command::new("/usr/bin/open")
            .arg(path)
            .spawn()
        {
            self.message(format!("Could not open location: {error}"), true, cx);
        }
    }

    pub(super) fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        div()
            .id("settings-view")
            .track_scroll(&self.settings.scroll)
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .bg(p.surface)
            .child(
                panel_body(cx)
                    .max_w(px(760.))
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(panel_heading(note("Settings")).flex_1())
                            .child(
                                path_button(Button::new("close-settings"), "Close Settings")
                                    .w_auto()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings.open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(self.claude_settings(cx))
                    .child(self.help_level_settings(cx))
                    .child(self.layout_settings(cx))
                    .child(self.appearance_settings(cx))
                    .child(self.font_settings(cx))
                    .child(self.language_settings(cx))
                    .child(self.app_settings(cx)),
            )
            .into_any_element()
    }

    fn claude_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let section = panel_body(cx).child(panel_heading(note("Claude")));
        let Some(state) = self.chat_state() else {
            return section
                .child(note("Open a folder to start a task with Claude."))
                .child(note(SUBSCRIPTION_NOTE))
                .into_any_element();
        };
        let busy = state.turn.is_some();
        let notice = view::connection_notice(&state.connection);
        let connected = match &state.connection {
            Connection::Ready {
                subscription,
                claude_code,
            } => Some(format!(
                "Connected · {subscription} · Claude Code {claude_code}"
            )),
            _ => None,
        };
        let command = notice.as_ref().and_then(|notice| notice.command.clone());
        let use_my_setup = state.prefs.use_my_setup;
        let weak = cx.entity().downgrade();
        let menu_weak = weak.clone();
        section
            .when_some(connected, |d, line| d.child(note(line)))
            .when_some(notice, |d, notice| {
                d.child(div().font_semibold().child(note(notice.title)))
                    .when(!notice.body.is_empty(), |d| d.child(note(notice.body)))
            })
            .when_some(command, |d, command| {
                let copied = command.clone();
                d.child(code_note("settings-sign-in", command, cx)).child(
                    path_button(Button::new("settings-copy-sign-in"), "Copy")
                        .w_auto()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                        }),
                )
            })
            .child(
                path_button(Button::new("settings-reconnect"), "Reconnect")
                    .w_auto()
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.chat_command(Command::Reconnect);
                        this.refresh_settings(cx);
                    })),
            )
            .when_some(state.install.clone(), |d, install| {
                d.child(code_note(
                    "settings-node",
                    format!("Node {} · {}", install.node_version, install.node_path),
                    cx,
                ))
                .child(code_note(
                    "settings-helper",
                    format!(
                        "Claude helper {} · {}",
                        if install.bundled {
                            "bundled"
                        } else {
                            "development"
                        },
                        install.helper_dir
                    ),
                    cx,
                ))
            })
            .child(
                Switch::new("use-my-setup")
                    .label("Use my Claude Code setup")
                    .checked(use_my_setup)
                    .disabled(busy)
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        let _ = weak.update(cx, |view, _| {
                            view.chat_command(Command::SetUseMySetup(checked))
                        });
                    }),
            )
            .child(panel_metadata(note(USE_MY_SETUP_NOTE), cx))
            .child(
                div()
                    .id("settings-permissions-row")
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().child("Permissions"))
                    .child(permission_menu("settings-permissions", &state, &menu_weak)),
            )
            .child(panel_metadata(note(PERMISSIONS_NOTE), cx))
            .child(note(SUBSCRIPTION_NOTE))
            .into_any_element()
    }

    /// One row per language with its help level: the common languages, then any other
    /// language that has a saved level.
    fn help_level_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let section = panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("Help level per language")));
        let Some(state) = self.chat_state() else {
            return section
                .child(note(
                    "Open a folder to choose a help level for each language.",
                ))
                .child(panel_metadata(note(HELP_LEVEL_NOTE), cx))
                .into_any_element();
        };
        let weak = cx.entity().downgrade();
        let rows = help_languages(state.prefs.help_levels.keys().map(String::as_str))
            .into_iter()
            .enumerate()
            .map(|(index, (language, label))| {
                let level = view::level_for(&state, &language);
                div()
                    .id(("settings-help-level-row", index))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().child(label))
                    .child(help_level_menu(
                        ("settings-help-level", index),
                        format!("{} ▾", level.label()),
                        language,
                        level,
                        true,
                        &weak,
                    ))
            });
        section
            .children(rows)
            .child(panel_metadata(note(HELP_LEVEL_NOTE), cx))
            .into_any_element()
    }

    /// Which side the file tree and the chat are on.
    fn layout_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let swapped = self.panes.layout.swapped;
        let weak = cx.entity().downgrade();
        panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("Layout")))
            .child(
                Switch::new("swap-panes")
                    .label("Chat on the left, file tree on the right")
                    .checked(swapped)
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        let _ = weak.update(cx, |view, cx| {
                            if view.panes.layout.swapped != checked {
                                view.swap_panes(cx);
                            }
                        });
                    }),
            )
            .child(panel_metadata(note(LAYOUT_NOTE), cx))
            .into_any_element()
    }

    fn language_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.dev.language.is_some();
        panel_body(cx).mt_3().pt_3().border_t_1().border_color(cx.theme().border).child(panel_heading(note("Language tools")))
            .child(note("Rust tools provide ordinary symbol completion and navigation."))
            .child(note(self.dev.health.clone()))
            .when(self.dev.discovering,|d|d.child(note("Inspecting installed tools…")))
            .when(!self.dev.tool_error.is_empty(),|d|d.child(panel_error(note(self.dev.tool_error.clone()), cx)))
            .when_some(self.dev.rust.as_ref(),|d,tools|d.child(note(format!("Installed toolchain: {}",tools.label)))
                .child(panel_warning(note("Enabling runs rust-analyzer and Cargo metadata with your local user permissions. Open only projects you trust. Build scripts, proc macros, automatic Cargo checks and auto-installation are disabled."), cx))
                .child(path_button(Button::new("enable-rust"), if enabled{"Disable Rust Tools"}else{"Trust Project & Enable Rust"}).disabled(self.loading||self.dev.discovering)
                    .on_click(cx.listener(move|this,_,_,cx|if enabled{this.disable_language(cx)}else{this.authorize_language(cx)}))))
            .when(enabled,|d|d.child(path_button(Button::new("restart-language"), "Restart Language Server").on_click(cx.listener(|this,_,_,cx|{
                if let Some(manager)=&this.dev.language{manager.restart();}this.invalidate_language(cx);cx.notify();}))))
            .child(path_button(Button::new("rescan-tools"), "Recheck Installed Tools").disabled(self.dev.discovering).on_click(cx.listener(|this,_,_,cx|this.discover_tools(cx))))
            .child(panel_heading(note("Current file")).mt_3().pt_3().border_t_1().border_color(cx.theme().border))
            .child(path_button(Button::new("format-file"), "Format Buffer  ⇧⌥F").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/formatting",window,cx))))
            .child(note("Formatting changes the buffer only when requested and remains undoable."))
            .child(path_button(Button::new("file-symbols"), "Symbols  ⌘⇧O").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/documentSymbol",window,cx))))
            .child(path_button(Button::new("file-references"), "References at Cursor  ⇧F12").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/references",window,cx))))
            .child(note("Use F12 for definitions and Control-Space for symbol completion. Hover explains a symbol. Python formatting is not provided by Pyright."))
            .child(panel_heading(note("Python tools")).mt_3().pt_3().border_t_1().border_color(cx.theme().border))
            .child(note(self.dev.python_health.clone()))
            .when(!self.dev.python_error.is_empty(),|d|d.child(panel_error(note(self.dev.python_error.clone()), cx)))
            .when_some(self.dev.python.as_ref(),|d,tools|d.child(note(tools.label.clone())).child(code_note("python-interpreter", tools.interpreter.display().to_string(), cx))
                .child(panel_warning(note("Pyright, Node and the selected interpreter run with your local user permissions. Enable them only for a trusted project/environment."), cx))
                .child(path_button(Button::new("trust-python"), if self.dev.python_language.is_some(){"Disable Python Tools"}else{"Trust Project & Enable Python"}).disabled(tools.node.is_none()||self.dev.discovering).on_click(cx.listener(|this,_,_,cx|this.authorize_python(cx)))))
            .child(Input::new(&self.dev.python_path).aria_label("Python interpreter path"))
            .child(path_button(Button::new("select-python"), "Select Interpreter (empty = default)").disabled(self.dev.discovering).on_click(cx.listener(|this,_,_,cx|this.select_python(cx))))
            .into_any_element()
    }

    fn app_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.store.lock().unwrap().directory.clone();
        let recovery = state.clone();
        let notices = self
            .settings
            .resources
            .as_ref()
            .map(|p| p.join("ThirdPartyNotices"))
            .filter(|p| p.is_dir());
        panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("App and recovery")))
            .when(self.settings.busy && self.settings.lines.is_empty(), |d| {
                d.child(note("Checking the Claude helper…"))
            })
            .children(self.settings.lines.iter().enumerate().map(|(i, line)| {
                div()
                    .id(("settings-line", i))
                    .min_w_0()
                    .h_flex()
                    .overflow_x_scroll()
                    .child(div().flex_none().w_auto().min_w_full().child(line.clone()))
            }))
            .child(code_note(
                "recovery-profile",
                format!("Recovery profile: {}", state.display()),
                cx,
            ))
            .child(
                path_button(Button::new("open-recovery"), "Open Recovery Folder").on_click(
                    cx.listener(move |v, _, _, cx| v.show_local_folder(recovery.clone(), cx)),
                ),
            )
            .when_some(notices, |d, path| {
                d.child(
                    path_button(Button::new("open-notices"), "Open Third-Party Notices").on_click(
                        cx.listener(move |v, _, _, cx| v.show_local_folder(path.clone(), cx)),
                    ),
                )
            })
            .child(
                div()
                    .id("settings-rust")
                    .min_w_0()
                    .h_flex()
                    .overflow_x_scroll()
                    .child(div().flex_none().w_auto().min_w_full().child(note(format!(
                        "Rust: {}",
                        self.dev.rust.as_ref().map_or_else(
                            || if self.dev.tool_error.is_empty() {
                                "Open a project to inspect installed tools".into()
                            } else {
                                self.dev.tool_error.clone()
                            },
                            |t| format!("{} · {}", t.label, t.cargo.display())
                        )
                    )))),
            )
            .child(
                div()
                    .id("settings-python")
                    .min_w_0()
                    .h_flex()
                    .overflow_x_scroll()
                    .child(div().flex_none().w_auto().min_w_full().child(note(format!(
                        "Python: {}",
                        self.dev.python.as_ref().map_or_else(
                            || if self.dev.python_error.is_empty() {
                                "Open a project to inspect installed tools".into()
                            } else {
                                self.dev.python_error.clone()
                            },
                            |t| format!("{} · {}", t.label, t.interpreter.display())
                        )
                    )))),
            )
            .child(
                panel_metadata(note(LIMITS_NOTE), cx)
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::release_line;

    #[test]
    fn release_line_reads_a_manifest_that_lists_the_bundled_helper() {
        // A release manifest lists every file of the helper's npm tree: about 1.5 MB.
        let resources = tempfile::tempdir().unwrap();
        let files: serde_json::Map<String, serde_json::Value> = (0..9000)
            .map(|i| {
                (
                    format!("Contents/Resources/claude-helper/node_modules/pkg/file-{i}.js"),
                    "0".repeat(64).into(),
                )
            })
            .collect();
        let manifest = serde_json::json!({ "release_id": "redesign-1", "files": files });
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        assert!(bytes.len() > 1024 * 1024);
        std::fs::write(resources.path().join("release-manifest.json"), bytes).unwrap();

        assert_eq!(
            release_line(resources.path()).as_deref(),
            Some("Release: redesign-1")
        );
    }

    #[test]
    fn release_line_is_absent_without_a_manifest() {
        let resources = tempfile::tempdir().unwrap();
        assert_eq!(release_line(resources.path()), None);
    }
}
