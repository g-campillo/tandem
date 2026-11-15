use super::*;
use crate::appearance::{
    compact_button, navigation_button, palette, panel_body, panel_code, panel_error, panel_heading,
    panel_metadata, panel_warning, path_button, solid_section,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn development_panel(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        div()
            .v_flex()
            .w(px(width))
            .min_w_0()
            .flex_shrink_0()
            .min_h_0()
            .border_l_1()
            .border_color(p.border)
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap_1()
                    .p_2()
                    .bg(p.chrome())
                    .border_b_1()
                    .border_color(p.border)
                    .children(
                        [
                            (Panel::Companion, "Companion", IconName::Bot),
                            (Panel::Agent, "Takeover", IconName::Play),
                            (Panel::Tools, "Tools", IconName::Settings2),
                            (Panel::Problems, "Problems", IconName::TriangleAlert),
                            (Panel::Checks, "Checks", IconName::CircleCheck),
                            (Panel::Task, "Task", IconName::FileText),
                            (Panel::Health, "App", IconName::Info),
                        ]
                        .into_iter()
                        .map(|(panel, label, icon)| {
                            navigation_button(
                                compact_button(Button::new(label).icon(icon), label),
                                self.dev.panel == panel,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.dev.panel = panel;
                                    if panel == Panel::Health {
                                        this.refresh_health(cx);
                                    }
                                    this.dev.scroll.set_offset(point(px(0.0), px(0.0)));
                                    cx.notify();
                                },
                            ))
                        }),
                    ),
            )
            .child(
                div()
                    .id("development-content")
                    .track_scroll(&self.dev.scroll)
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_3()
                    .bg(p.surface)
                    .text_size(px(14.))
                    .child(match self.dev.panel {
                        Panel::Tools => self.tools_panel(cx),
                        Panel::Problems => self.problems_panel(cx),
                        Panel::Checks => self.checks_panel(cx),
                        Panel::Task => self.task_panel(cx),
                        Panel::Companion => self.companion_panel(cx),
                        Panel::Agent => self.agent_panel(cx),
                        Panel::Health => self.health_panel(cx),
                    }),
            )
            .into_any_element()
    }
    fn tools_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.dev.language.is_some();
        panel_body(cx).child(panel_heading(note("Project language tools")))
            .child(note("Rust tools provide ordinary symbol completion and navigation. AI implementation and edit authority remain off."))
            .child(note(self.dev.health.clone()))
            .when(self.dev.discovering,|d|d.child(note("Inspecting installed tools and check recipes…")))
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
            .child(path_button(Button::new("select-python"), "Select Interpreter (empty = default)").disabled(self.dev.discovering||self.dev.check_cancel.is_some()).on_click(cx.listener(|this,_,_,cx|this.select_python(cx))))
            .into_any_element()
    }
    fn problems_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .dev
            .diagnostic_stamp
            .as_ref()
            .is_some_and(|s| self.language_manager().is_some_and(|m| m.is_current(s)));
        let navigation_current = self
            .dev
            .navigation_stamp
            .as_ref()
            .is_some_and(|s| self.language_manager().is_some_and(|m| m.is_current(s)));
        panel_body(cx)
            .child(panel_heading(note("Current language buffer")))
            .child(if self.dev.diagnostic_busy {
                "Checking the current language snapshot…"
            } else if current && self.dev.problems.is_empty() {
                "No language diagnostics reported for this buffer. Cargo checks are separate."
            } else if current {
                "Language diagnostics for the inspected open-buffer context"
            } else {
                "No current diagnostics. Enable tools and open a Rust or Python file."
            })
            .child(
                path_button(
                    Button::new("refresh-diagnostics"),
                    "Check Language Snapshot",
                )
                .disabled(self.dev.diagnostic_busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.language_action("textDocument/diagnostic", window, cx)
                })),
            )
            .when(current, |d| {
                d.children(self.dev.problems.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    path_button(Button::new(("problem", i)), item.label).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        },
                    ))
                }))
            })
            .when(navigation_current, |d| {
                d.child(
                    panel_heading(note("Navigation results"))
                        .mt_3()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border),
                )
                .children(self.dev.navigation.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    path_button(Button::new(("location", i)), item.label).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        },
                    ))
                }))
            })
            .into_any_element()
    }
    fn checks_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.dev.check_cancel.is_some() || self.agent.busy;
        let record = self
            .dev
            .checks
            .iter()
            .find(|c| Some(&c.id) == self.dev.selected_check.as_ref());
        let pending = self
            .dev
            .pending_recipe
            .as_ref()
            .and_then(|id| self.dev.recipes.iter().find(|r| &r.id == id));
        panel_body(cx).child(panel_heading(note("Validate captured code")))
            .child(note("Python comparison uses benchmarks/tandem_compare.py: cases() returns 1–12 (label, input) tuples; baseline(input) and candidate(input) return comparable values. Two warmups, seven alternating samples, fresh deep copies outside timing, 120-second limit. These trusted callbacks run with the same isolated check permissions."))
            .child(note("Checks include unsaved buffers. Each run records its own snapshot; a pass is evidence about that version, not a verdict on later edits."))
            .when(!self.dev.check_error.is_empty(),|d|d.child(panel_error(note(self.dev.check_error.clone()), cx)))
            .when(self.dev.discovering,|d|d.child(note("Preparing installed check recipes…")))
            .children(self.dev.recipes.iter().map(|r|{let id=r.id.clone();path_button(Button::new(SharedString::from(format!("recipe:{id}"))), r.title.clone()).disabled(busy||self.loading).on_click(cx.listener(move|this,_,_,cx|this.prepare_recipe(&id,cx)))}))
            .when_some(pending,|d,recipe|d.child(solid_section(div(), cx).v_flex().min_w_0().gap_3().p_3().border_1().border_color(palette(cx.theme().mode).warning)
                .child(panel_heading(note(format!("Authorize {} for this project",recipe.title))))
                .child(code_note("recipe-argv", recipe.argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" "), cx))
                .child(note("Read: captured inputs and the selected tool environments listed below. Write: isolated artifacts. Original source and tool network access are denied; a selected project virtualenv is a read-only exception. Build scripts/tests execute within this scope. Limit: 120 seconds."))
                .children(recipe.read_roots.iter().enumerate().map(|(i,path)|code_note(("recipe-read-root",i), format!("Read-only: {}",path.display()), cx)))
                .child(note("This grant is reused only while the executable and recipe fingerprint stay unchanged. It grants no AI source editing."))
                .child(path_button(Button::new("authorize-check"), "Authorize Recipe & Run").primary().on_click(cx.listener(|this,_,_,cx|this.authorize_recipe(cx))))
                .child(path_button(Button::new("dismiss-recipe"), "Keep Editing").on_click(cx.listener(|this,_,_,cx|{this.dev.pending_recipe=None;cx.notify();})))))
            .when(busy,|d|d.child(path_button(Button::new("cancel-check"), "Cancel Active Check").danger().on_click(cx.listener(|this,_,_,cx|{
                if let Some(cancel)=&this.dev.check_cancel{cancel.store(true,Ordering::Relaxed);}this.message("Cancellation requested. Waiting for the command's terminal result…",false,cx);
            }))))
            .when_some(record,|d,record|{
                let output=record.output.clone();
                let mut d=d.child(panel_heading(note(format!("{} · {:?}",record.recipe_id,record.status)))).child(note(record.detail.clone()))
                    .child(panel_metadata(note(format!("Receipt: {}",record.id)), cx)).child(panel_metadata(note(format!("Exit: {}",record.exit_code.map(|c|c.to_string()).unwrap_or_else(||"not recorded".into()))), cx));
                if let Some(snapshot)=&record.snapshot {d=d.child(note(format!("Snapshot {} · {} files · {} omitted",&snapshot.id[..12],snapshot.files.len(),snapshot.omitted.len())))
                    .children(snapshot.omitted.iter().take(8).enumerate().map(|(i,o)|div().id(("omitted",i)).min_w_0().child(panel_metadata(note(format!("Omitted {}: {}",o.path,o.reason)), cx))));}
                if let Some(measurement)=&record.benchmark {d=d.child(note(measurement.summary())).child(note(format!("Interpreter: {}",record.executable_version)));}
                d.child(path_button(Button::new("copy-check-output"), "Copy Check Output").on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(output.clone()))))
                    .when(record.output_truncated,|d|d.child(panel_warning(note("Output reached its capture limit."), cx)))
                    .children(record.diagnostics.iter().enumerate().map(|(i,item)|{let item=item.clone();path_button(Button::new(("check-diagnostic",i)), format!("Captured {}:{} {}",item.path,item.line,item.message)).on_click(cx.listener(move|this,_,window,cx|this.open_file(item.path.clone(),false,Some(item.line as usize),window,cx)))}))
                    .child(panel_heading(note("Output (last 80 lines)")))
                    .child(panel_code(div().id("check-output"), cx).h_flex().overflow_x_scroll().child(div().v_flex().flex_none().w_auto().min_w_full().whitespace_nowrap().children(record.output.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().enumerate().map(|(i,line)|div().id(("output-line",i)).flex_shrink_0().child(note(line.chars().take(350).collect::<String>()))))))
            })
            .when(!self.dev.checks.is_empty(),|d|d.child(panel_heading(note("Recent receipts"))).children(self.dev.checks.iter().take(10).map(|record|{
                let id=record.id.clone();path_button(Button::new(SharedString::from(format!("receipt:{id}"))), format!("{} · {:?} · {}",record.recipe_id,record.status,&id[..8])).on_click(cx.listener(move|this,_,_,cx|{this.dev.selected_check=Some(id.clone());cx.notify();}))
            })))
            .into_any_element()
    }
    fn task_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        self.companion_panel(cx)
    }
}

#[track_caller]
pub(super) fn note(text: impl Into<SharedString>) -> impl IntoElement {
    let text = text.into();
    let call = std::panic::Location::caller();
    div()
        .id(SharedString::from(format!(
            "note:{}:{}:{text}",
            call.line(),
            call.column()
        )))
        .role(Role::Label)
        .aria_label(text.clone())
        .child(text)
}

/// Full path/argv text remains available even when it contains no wrap points.
pub(super) fn code_note(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    cx: &App,
) -> impl IntoElement {
    panel_code(div().id(id), cx)
        .p_2()
        .rounded(px(6.))
        .h_flex()
        .overflow_x_scroll()
        .child(
            div()
                .flex_none()
                .w_auto()
                .min_w_full()
                .whitespace_nowrap()
                .child(note(text)),
        )
}
