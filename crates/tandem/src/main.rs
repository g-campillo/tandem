mod appearance;
mod workspace;

use anyhow::{Context as _, Result};
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tandem_core::Store;
use workspace::*;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let mut project = None;
    let mut restore = true;
    let mut state_dir = PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?)
        .join("Library/Application Support/Tandem/workspace");
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--state-dir") => {
                state_dir = arguments
                    .next()
                    .map(PathBuf::from)
                    .context("--state-dir needs a directory")?
            }
            Some("--no-restore") => restore = false,
            Some("--runtime-status") => match runtime_status() {
                Ok(line) => {
                    println!("{line}");
                    return Ok(());
                }
                Err(error) => {
                    eprintln!("{error:#}");
                    std::process::exit(1);
                }
            },
            Some("--help") => {
                println!(
                    "Tandem local editor\nUsage: tandem [FOLDER] [--state-dir DIRECTORY] [--no-restore] [--runtime-status]\n  --runtime-status  print which Claude helper and Node.js Tandem would start, then exit"
                );
                return Ok(());
            }
            Some(value) if value.starts_with('-') => anyhow::bail!("Unknown option: {value}"),
            _ => {
                anyhow::ensure!(project.is_none(), "Open one project per window");
                project = Some(PathBuf::from(argument));
            }
        }
    }
    let store = Arc::new(Mutex::new(Store::open(&state_dir)?));
    if project.is_none() && restore {
        project = store.lock().unwrap().last_project()?.map(PathBuf::from);
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-,", Settings, None),
                KeyBinding::new("cmd-o", OpenProject, None),
                KeyBinding::new("cmd-s", Save, None),
                KeyBinding::new("cmd-alt-s", SaveAll, None),
                KeyBinding::new("cmd-shift-s", SaveCopy, None),
                KeyBinding::new("cmd-p", QuickOpen, None),
                KeyBinding::new("cmd-shift-f", ProjectSearch, None),
                // Override the toolkit's Input-scoped replace shortcut so project
                // search is consistent even inside editors and search fields.
                KeyBinding::new("cmd-shift-f", ProjectSearch, Some("Input")),
                KeyBinding::new("cmd-f", FindInFile, Some("Input")),
                KeyBinding::new("cmd-f", FindInFile, None),
                KeyBinding::new("cmd-alt-f", ReplaceInFile, Some("Input")),
                KeyBinding::new("cmd-alt-f", ReplaceInFile, None),
                KeyBinding::new("cmd-n", NewFile, None),
                KeyBinding::new("cmd-w", CloseFile, None),
                KeyBinding::new("cmd-shift-d", Changes, None),
                KeyBinding::new("cmd-=", ZoomIn, None),
                KeyBinding::new("cmd--", ZoomOut, None),
                KeyBinding::new("ctrl-`", ToggleTerminal, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-shift-l", ToggleChat, None),
                KeyBinding::new("cmd-l", FocusChat, None),
                KeyBinding::new("cmd-shift-m", ShowProblems, None),
                KeyBinding::new("alt-shift-f", FormatFile, None),
                KeyBinding::new("shift-f12", References, None),
                KeyBinding::new("cmd-shift-o", Symbols, None),
                KeyBinding::new("cmd-.", StopClaude, None),
                // The toolkit binds ⌘. to code actions inside inputs, which Tandem does not
                // offer; Stop must work from the editor and the composer too.
                KeyBinding::new("cmd-.", StopClaude, Some("Input")),
                KeyBinding::new("cmd-alt-c", CheckMyWork, None),
            ]);
            cx.set_menus([
                Menu::new("Tandem").items([
                    MenuItem::action("Settings…", Settings),
                    MenuItem::action("Quit Tandem", Quit),
                ]),
                Menu::new("File").items([
                    MenuItem::action("Open Folder…", OpenProject),
                    MenuItem::action("Recent Projects", ShowRecent),
                    MenuItem::action("New File", NewFile),
                    MenuItem::action("Save", Save),
                    MenuItem::action("Save All", SaveAll),
                    MenuItem::action("Save Copy…", SaveCopy),
                    MenuItem::action("Close File", CloseFile),
                ]),
                Menu::new("Edit").items([
                    MenuItem::action("Undo", gpui_kit::component::input::Undo),
                    MenuItem::action("Redo", gpui_kit::component::input::Redo),
                    MenuItem::action("Cut", gpui_kit::component::input::Cut),
                    MenuItem::action("Copy", gpui_kit::component::input::Copy),
                    MenuItem::action("Paste", gpui_kit::component::input::Paste),
                    MenuItem::action("Find in File", FindInFile),
                    MenuItem::action("Replace in File", ReplaceInFile),
                ]),
                Menu::new("View").items([
                    MenuItem::action("Chat", ToggleChat),
                    MenuItem::action("Quick Open", QuickOpen),
                    MenuItem::action("Search Project", ProjectSearch),
                    MenuItem::action("Compare with Disk", Changes),
                    MenuItem::action("Terminal", ToggleTerminal),
                    MenuItem::action("Problems", ShowProblems),
                    MenuItem::action("Increase Text Size", ZoomIn),
                    MenuItem::action("Decrease Text Size", ZoomOut),
                ]),
                Menu::new("Code").items([
                    MenuItem::action("Format Buffer", FormatFile),
                    MenuItem::action("References at Cursor", References),
                    MenuItem::action("Document Symbols", Symbols),
                ]),
                Menu::new("Chat").items([
                    MenuItem::action("Focus Chat", FocusChat),
                    MenuItem::action("New Task", NewTask),
                    MenuItem::action("Stop Claude", StopClaude),
                    MenuItem::action("Check My Work", CheckMyWork),
                ]),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let handle = gpui_kit::open_window(
                WindowOptions {
                    #[cfg(target_os = "macos")]
                    window_background: WindowBackgroundAppearance::Blurred,
                    window_min_size: Some(size(px(900.0), px(600.0))),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1280.0), px(820.0)),
                        cx,
                    ))),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    let view = cx.new(|cx| Workspace::new(store.clone(), project, window, cx));
                    let closing = view.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        closing
                            .update(cx, |view, cx| view.flush_for_close(cx))
                            .unwrap_or(true)
                    });
                    view
                },
            )
            .expect("open Tandem window");
            // The component RootPlugin supplies an opaque background by default.
            // Override only this root so app chrome can expose the native material;
            // component/editor/popover surfaces keep their opaque theme colors.
            #[cfg(target_os = "macos")]
            handle
                .0
                .update(cx, |_, window, cx| {
                    gpui_kit::base::Root::update(window, cx, |root, _, cx| {
                        root.style().background = Some(Hsla::transparent_black().into());
                        cx.notify();
                    });
                })
                .expect("set Tandem root material");
            install_actions(cx, handle.0, handle.1.downgrade());
            let quitting = handle.1.downgrade();
            cx.on_action(move |_: &Quit, cx| {
                if quitting
                    .update(cx, |root, cx| root.flush_for_close(cx))
                    .unwrap_or(true)
                {
                    cx.quit();
                }
            });
            cx.activate(true);
        });
    Ok(())
}
