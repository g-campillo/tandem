//! Settings → Language tools blocks for the tool groups in tandem_core::SERVER_KINDS. Rust and
//! Python keep their rows in settings.rs.
use super::panels::{code_note, note};
use super::*;
use crate::appearance::{panel_body, panel_error, panel_heading, panel_warning, path_button};
use gpui_kit::component::button::Button;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use std::collections::BTreeSet;
use tandem_core::languages::{language_id_for_path, tool_kind};

/// What enabling the tools of `kind` runs, shown until the project trusts them.
pub(super) fn trust_notice(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Rust => {
            "Enabling runs rust-analyzer and Cargo with your local user permissions, including the build scripts and proc macros of the project and its dependencies, as cargo build would. Open only projects you trust. Automatic Cargo checks and auto-installation stay off."
        }
        ToolKind::Python => {
            "Pyright, Node and the selected interpreter run with your local user permissions. Enable them only for a trusted project/environment."
        }
        ToolKind::Java => {
            "Enabling runs jdtls on the Java runtime shown above, with your local user permissions. It imports Maven and Gradle builds offline, which runs their build logic, and keeps its workspace in Tandem's state folder, not in the project. Enable it only for projects you trust."
        }
        ToolKind::Swift => {
            "Enabling runs SourceKit-LSP with your local user permissions. To index a package it builds it in Tandem's state folder, which runs package plugins and macros and resolves package dependencies as swift build would. In an Xcode project it runs xcode-build-server, which reads Xcode's build logs for the compiler flags. Enable it only for projects you trust."
        }
        ToolKind::TypeScript => {
            "Enabling runs the TypeScript language server with your local user permissions. It uses the project's own TypeScript in node_modules when there is one, which runs code installed by the project. It never installs type packages. Enable it only for projects you trust."
        }
        ToolKind::Vue => {
            "Enabling runs the Vue language server and typescript-language-server with the project's TypeScript and Vue's TypeScript plugin, with your local user permissions. In a Nuxt project it also runs the project's nuxt prepare, which loads nuxt.config and its modules (project code), whenever the server starts or a file is added or removed in an auto-import folder. Enable it only for projects you trust."
        }
        ToolKind::Svelte => {
            "Enabling runs svelte-language-server with your local user permissions. It loads the project's svelte.config.js, which runs project code, and in a SvelteKit project it runs the project's svelte-kit sync whenever the server starts or a route file is added or removed, so route types stay current. Enable it only for projects you trust."
        }
        ToolKind::Web => {
            "Enabling runs the HTML, CSS and JSON language servers from vscode-langservers-extracted with your local user permissions. They do not download JSON schemas. Enable them only for projects you trust."
        }
    }
}

/// The tool groups that serve at least one of `files`.
pub(super) fn project_kinds<'a>(files: impl IntoIterator<Item = &'a str>) -> BTreeSet<ToolKind> {
    files
        .into_iter()
        .filter_map(|file| tool_kind(language_id_for_path(file)))
        .collect()
}

impl Workspace {
    /// One block per registry kind the project trusts, has files for or has open in a tab,
    /// then a note naming the kinds left out.
    pub(super) fn language_tool_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.project.is_none() {
            return div().into_any_element();
        }
        let used = project_kinds(self.index.files.iter().map(String::as_str));
        let open: BTreeSet<ToolKind> = self
            .tabs
            .iter()
            .filter_map(|tab| tool_kind(tab.document.language()))
            .collect();
        let (shown, hidden): (Vec<ToolKind>, Vec<ToolKind>) = SERVER_KINDS
            .iter()
            .map(|(kind, _)| *kind)
            .partition(|kind| {
                self.dev.settings.trust(*kind).is_some()
                    || used.contains(kind)
                    || open.contains(kind)
            });
        let hidden: Vec<&str> = hidden.into_iter().map(ToolKind::label).collect();
        panel_body(cx)
            .children(
                shown
                    .into_iter()
                    .map(|kind| self.language_tool_block(kind, cx)),
            )
            .when(!hidden.is_empty(), |d| {
                d.child(note(format!(
                    "No {} files in this project.",
                    hidden.join(", ")
                )))
            })
            .into_any_element()
    }

    fn language_tool_block(&self, kind: ToolKind, cx: &mut Context<Self>) -> AnyElement {
        let (key, label) = (kind.key(), kind.label());
        let block = panel_body(cx)
            .id(SharedString::from(format!("language-tools-{key}")))
            .child(
                panel_heading(note(label))
                    .mt_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border),
            );
        let Some(slot) = self.dev.servers.get(&kind) else {
            return block.into_any_element();
        };
        let block = block.when_some(slot.missing.as_ref(), |d, missing| {
            d.child(panel_error(note(missing.message.clone()), cx))
                .children(missing.install.iter().enumerate().map(|(i, command)| {
                    code_note(
                        SharedString::from(format!("install-{key}-{i}")),
                        command.clone(),
                        cx,
                    )
                }))
        });
        let Some(tools) = slot.tools.as_ref() else {
            return block.into_any_element();
        };
        let trusted = self.dev.settings.trust(kind) == Some(&tools.fingerprint);
        block
            .child(note(tools.label.clone()))
            .children(tools.notes.iter().enumerate().map(|(i, tool_note)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .min_w_0()
                    .child(note(tool_note.text.clone()))
                    .when_some(tool_note.command.clone(), |d, command| {
                        d.child(code_note(
                            SharedString::from(format!("note-{key}-{i}")),
                            command,
                            cx,
                        ))
                    })
            }))
            .when(!trusted, |d| {
                d.child(panel_warning(note(trust_notice(kind)), cx))
            })
            .child(
                path_button(
                    Button::new(SharedString::from(format!("trust-{key}"))),
                    if trusted {
                        format!("Disable {label} Tools")
                    } else {
                        format!("Trust Project & Enable {label}")
                    },
                )
                .disabled(self.loading || self.dev.discovering)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if trusted {
                        this.distrust_kind(kind, cx)
                    } else {
                        this.trust_kind(kind, cx)
                    }
                })),
            )
            .when(trusted, |d| {
                d.children(
                    slot.health
                        .iter()
                        .map(|line| note(format!("{label} · {line}"))),
                )
                .child(
                    path_button(
                        Button::new(SharedString::from(format!("restart-{key}"))),
                        format!("Restart {label} Tools"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.restart_kind(kind, cx))),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{project_kinds, trust_notice};
    use std::collections::BTreeSet;
    use tandem_core::ToolKind;

    #[test]
    fn project_kinds_come_from_file_names() {
        assert_eq!(
            project_kinds(["a.swift", "b.tsx", "README.md", "c.scss"]),
            BTreeSet::from([ToolKind::Swift, ToolKind::TypeScript, ToolKind::Web])
        );
        assert!(project_kinds([]).is_empty());
    }

    #[test]
    fn every_kind_has_a_trust_notice() {
        for kind in ToolKind::ALL {
            assert!(!trust_notice(kind).is_empty(), "{kind:?}");
        }
    }
}
