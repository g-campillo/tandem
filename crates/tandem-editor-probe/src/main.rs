//! M0-B: two native buffers and a task panel, not the product application.
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

mod editor_check;
mod language;
mod terminal;
actions!(tandem_probe, [Quit]);

struct EditorProbe {
    rust: Entity<EditorState>,
    python: Entity<EditorState>,
    language: Option<Arc<Mutex<tandem_lsp::Session>>>,
    language_status: String,
    terminal: Option<Entity<terminal::TerminalView>>,
    _subscriptions: Vec<Subscription>,
}

impl EditorProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rust = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("rust")
                .default_value(language::RUST_TEXT)
        });
        let python = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("python")
                .default_value("def greet(name: str) -> str:\n    return f\"Hello, {name}\"\n")
        });
        let loading = cx.background_spawn(async { language::start() });
        cx.spawn(async move |view, cx| {
            let result = loading.await;
            let _ = view.update(cx, |view, cx| match result {
                Ok(session) => {
                    let session = Arc::new(Mutex::new(session));
                    let provider = Rc::new(language::Provider(session.clone()));
                    view.rust.update(cx, |editor, cx| {
                        editor.lsp_mut().completion_provider = Some(provider.clone());
                        editor.lsp_mut().definition_provider = Some(provider.clone());
                        editor.lsp_mut().hover_provider = Some(provider);
                        cx.notify();
                    });
                    view.language = Some(session);
                    view.language_status = "rust-analyzer connected".into();
                    cx.notify();
                }
                Err(_) => {
                    view.language_status =
                        "Language server unavailable; editing still works".into();
                    cx.notify();
                }
            });
        })
        .detach();
        let terminal = terminal::start()
            .ok()
            .map(|terminal| cx.new(|cx| terminal::TerminalView::new(terminal, cx)));
        if std::env::args().any(|arg| arg == "--editor-check") {
            editor_check::run(rust.clone(), window, cx);
        }
        let edited = cx.subscribe(&rust, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.language_status = "Buffer edited; check a new diagnostic snapshot".into();
                editor.update(cx, |editor, cx| {
                    if let Some(diagnostics) = editor.diagnostics_mut() {
                        diagnostics.clear();
                    }
                    cx.notify();
                });
                cx.notify();
            }
        });
        Self {
            rust,
            python,
            language: None,
            language_status: "Starting rust-analyzer…".into(),
            terminal,
            _subscriptions: vec![edited],
        }
    }

    fn check_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.language.clone() else {
            return;
        };
        let snapshot = self.rust.read(cx).text().to_string();
        let checked = snapshot.clone();
        self.language_status = "Checking current unsaved Rust buffer…".into();
        let check = cx.background_spawn(async move {
            session
                .lock()
                .map_err(|_| anyhow::anyhow!("language session failed"))?
                .diagnostics(&checked)
        });
        cx.spawn(async move |view, cx| {
            let result = check.await;
            let _ = view.update(cx, |view, cx| {
                if *view.rust.read(cx).text() != snapshot {
                    view.language_status = "Buffer changed; earlier diagnostics discarded".into();
                } else {
                    match result {
                        Ok(items) => {
                            view.language_status =
                                format!("{} diagnostics on current buffer", items.len());
                            view.rust.update(cx, |editor, cx| {
                                if let Some(set) = editor.diagnostics_mut() {
                                    set.clear();
                                    set.extend(
                                        items.into_iter().map(
                                            gpui_kit::component::highlighter::Diagnostic::from,
                                        ),
                                    );
                                }
                                cx.notify();
                            });
                        }
                        Err(_) => {
                            view.language_status =
                                "Diagnostic request failed; editing still works".into()
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for EditorProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .id("mode-status")
                    .role(Role::Label)
                    .aria_label("Tandem editor feasibility. Companion mode. AI cannot edit.")
                    .p_3()
                    .child("Tandem · editor feasibility · Companion · AI cannot edit"),
            )
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .child(div().p_2().child("main.rs"))
                            .child(Editor::new(&self.rust).aria_label("Rust source: main.rs").h(relative(1.0)).size_full()),
                    )
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .child(div().p_2().child("greet.py"))
                            .child(Editor::new(&self.python).aria_label("Python source: greet.py").h(relative(1.0)).size_full()),
                    )
                    .child(
                        div()
                            .id("task-panel")
                            .role(Role::Group)
                            .aria_label("Current task: native editor gate. Verify typing, selection, undo, search and input methods. Solution hidden.")
                            .v_flex()
                            .h_full()
                            .w_64()
                            .p_4()
                            .gap_3()
                            .child("Native editor gate")
                            .child("Verify typing, selection, undo, search and input methods.")
                            .child("Solution: hidden")
                            .child("This probe does not save files or run AI.")
                            .child(div().id("language-status").role(Role::Label).aria_label(self.language_status.clone()).child(self.language_status.clone()))
                            .child(Button::new("diagnostics").label("Check language snapshot").on_click(cx.listener(|this, _, _, cx| this.check_diagnostics(cx)))),
                    ),
            )
            .when_some(self.terminal.clone(), |view, terminal| view.child(terminal))
            .when(self.terminal.is_none(), |view| view.child("Terminal unavailable; editing remains available"))
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--terminal-check") {
        if let Err(error) = terminal::check() {
            eprintln!("Terminal probe failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--lsp-check") {
        if let Err(error) = language::check() {
            eprintln!("Language probe failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(
                [Menu::new("Tandem Editor Probe").items([MenuItem::action("Quit", Quit)])],
            );
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1200.0), px(760.0)),
                        cx,
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| EditorProbe::new(window, cx)),
            )
            .expect("open editor feasibility window");
            cx.activate(true);
        });
    if editor_check::failed() {
        std::process::exit(1);
    }
}
