//! The editor settings (one set for every project, kept in the store) and the Editor
//! section of Settings, which also points to the folder of the user's own snippets.
use super::*;
use crate::appearance::{panel_body, panel_heading, panel_metadata, path_button};
use crate::workspace::panels::note;
use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IconName};

const FORMAT_ON_SAVE_NOTE: &str = "⌘S formats the file with its language server first, when the server can format. Save All does not format.";
const SNIPPETS_NOTE: &str = "Add your own snippets in VS Code's format: one file per language id (rust.json, python.json, typescriptreact.json …) or all.json for every language.";

impl Workspace {
    /// The editor settings, read from the store the first time they are needed; damaged
    /// or unreadable settings give the defaults.
    pub(super) fn editor_prefs(&self) -> EditorPrefs {
        *self.ide.prefs.get_or_init(|| {
            self.store
                .lock()
                .unwrap()
                .editor_prefs()
                .unwrap_or_default()
        })
    }

    /// Turns Format on save on or off and saves the editor settings; the change holds for
    /// this session even when it could not be saved, which the status bar then says.
    fn set_format_on_save(&mut self, on: bool, cx: &mut Context<Self>) {
        let prefs = EditorPrefs {
            format_on_save: on,
            ..self.editor_prefs()
        };
        self.ide.prefs = OnceCell::from(prefs);
        let saved = self
            .store
            .lock()
            .map_err(|_| anyhow!("Recovery store unavailable"))
            .and_then(|mut store| store.save_editor_prefs(&prefs));
        if let Err(error) = saved {
            self.message(
                format!("Could not save the editor settings: {error:#}"),
                true,
                cx,
            );
        }
        cx.notify();
    }

    /// Shows the folder of the user's snippets in Finder, creating it first.
    fn reveal_snippets_folder(&mut self, cx: &mut Context<Self>) {
        let folder = self.store.lock().unwrap().directory.join("snippets");
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.message(
                format!("Could not create the snippets folder: {error}"),
                true,
                cx,
            );
            return;
        }
        if let Err(error) = std::process::Command::new("/usr/bin/open")
            .arg(folder)
            .spawn()
        {
            self.message(format!("Could not open location: {error}"), true, cx);
        }
    }

    /// The Editor section of Settings: Format on save, and where the user's snippets go.
    pub(in crate::workspace) fn editor_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let prefs = self.editor_prefs();
        let weak = cx.entity().downgrade();
        panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("Editor")))
            .child(
                Switch::new("format-on-save")
                    .label("Format on save")
                    .checked(prefs.format_on_save)
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        let _ = weak.update(cx, |view, cx| view.set_format_on_save(checked, cx));
                    }),
            )
            .child(panel_metadata(note(FORMAT_ON_SAVE_NOTE), cx))
            .child(panel_heading(note("Snippets")).mt_3())
            .child(panel_metadata(note(SNIPPETS_NOTE), cx))
            .child(
                path_button(Button::new("reveal-snippets"), "Reveal Snippets Folder")
                    .icon(IconName::FolderOpen)
                    .on_click(cx.listener(|this, _, _, cx| this.reveal_snippets_folder(cx))),
            )
            .into_any_element()
    }
}
