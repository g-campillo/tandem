use super::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;

impl Workspace {
    pub(super) fn development_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .v_flex()
            .w(px(410.0))
            .flex_shrink_0()
            .min_h_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div().h_flex().flex_wrap().gap_1().p_2().children(
                    [
                        (Panel::Companion, "Companion"),
                        (Panel::Agent, "Takeover"),
                        (Panel::Tools, "Tools"),
                        (Panel::Problems, "Problems"),
                        (Panel::Checks, "Checks"),
                        (Panel::Task, "Task"),
                        (Panel::Health, "App"),
                    ]
                    .into_iter()
                    .map(|(panel, label)| {
                        Button::new(label)
                            .label(label)
                            .when(self.dev.panel == panel, |b| b.primary())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dev.panel = panel;
                                if panel == Panel::Health {
                                    this.refresh_health(cx);
                                }
                                this.dev.scroll.set_offset(point(px(0.0), px(0.0)));
                                cx.notify();
                            }))
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
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .text_sm()
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
        div().v_flex().gap_3().child(div().font_semibold().child(note("Project language tools")))
            .child(note("Rust tools provide ordinary symbol completion and navigation. AI implementation and edit authority remain off."))
            .child(note(self.dev.health.clone()))
            .when(self.dev.discovering,|d|d.child(note("Inspecting installed tools and check recipes…")))
            .when(!self.dev.tool_error.is_empty(),|d|d.child(note(self.dev.tool_error.clone())))
            .when_some(self.dev.rust.as_ref(),|d,tools|d.child(note(format!("Installed toolchain: {}",tools.label)))
                .child(note("Enabling runs rust-analyzer and Cargo metadata with your local user permissions. Open only projects you trust. Build scripts, proc macros, automatic Cargo checks and auto-installation are disabled."))
                .child(Button::new("enable-rust").label(if enabled{"Disable Rust Tools"}else{"Trust Project & Enable Rust"}).disabled(self.loading||self.dev.discovering)
                    .on_click(cx.listener(move|this,_,_,cx|if enabled{this.disable_language(cx)}else{this.authorize_language(cx)}))))
            .when(enabled,|d|d.child(Button::new("restart-language").label("Restart Language Server").on_click(cx.listener(|this,_,_,cx|{
                if let Some(manager)=&this.dev.language{manager.restart();}this.invalidate_language(cx);cx.notify();}))))
            .child(Button::new("rescan-tools").label("Recheck Installed Tools").disabled(self.dev.discovering).on_click(cx.listener(|this,_,_,cx|this.discover_tools(cx))))
            .child(div().font_semibold().child(note("Current file")))
            .child(Button::new("format-file").label("Format Buffer  ⇧⌥F").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/formatting",window,cx))))
            .child(note("Formatting changes the buffer only when requested and remains undoable."))
            .child(Button::new("file-symbols").label("Symbols  ⌘⇧O").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/documentSymbol",window,cx))))
            .child(Button::new("file-references").label("References at Cursor  ⇧F12").on_click(cx.listener(|this,_,window,cx|this.language_action("textDocument/references",window,cx))))
            .child(note("Use F12 for definitions and Control-Space for symbol completion. Hover explains a symbol. Python formatting is not provided by Pyright."))
            .child(note("Python tools"))
            .child(note(self.dev.python_health.clone()))
            .when(!self.dev.python_error.is_empty(),|d|d.child(note(self.dev.python_error.clone())))
            .when_some(self.dev.python.as_ref(),|d,tools|d.child(note(tools.label.clone())).child(note(tools.interpreter.display().to_string()))
                .child(note("Pyright, Node and the selected interpreter run with your local user permissions. Enable them only for a trusted project/environment."))
                .child(Button::new("trust-python").label(if self.dev.python_language.is_some(){"Disable Python Tools"}else{"Trust Project & Enable Python"}).disabled(tools.node.is_none()||self.dev.discovering).on_click(cx.listener(|this,_,_,cx|this.authorize_python(cx)))))
            .child(Input::new(&self.dev.python_path).aria_label("Python interpreter path"))
            .child(Button::new("select-python").label("Select Interpreter (empty = default)").disabled(self.dev.discovering||self.dev.check_cancel.is_some()).on_click(cx.listener(|this,_,_,cx|this.select_python(cx))))
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
        div()
            .v_flex()
            .gap_2()
            .child(div().font_semibold().child(note("Current language buffer")))
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
                Button::new("refresh-diagnostics")
                    .label("Check Language Snapshot")
                    .disabled(self.dev.diagnostic_busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.language_action("textDocument/diagnostic", window, cx)
                    })),
            )
            .when(current, |d| {
                d.children(self.dev.problems.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    Button::new(("problem", i))
                        .label(item.label)
                        .w_full()
                        .justify_start()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        }))
                }))
            })
            .when(navigation_current, |d| {
                d.child(
                    div()
                        .font_semibold()
                        .mt_3()
                        .child(note("Navigation results")),
                )
                .children(self.dev.navigation.iter().enumerate().map(|(i, item)| {
                    let item = item.clone();
                    Button::new(("location", i))
                        .label(item.label)
                        .w_full()
                        .justify_start()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file(item.path.clone(), false, Some(item.line), window, cx)
                        }))
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
        div().v_flex().gap_3().child(div().font_semibold().child(note("Validate captured code")))
            .child(note("Python comparison uses benchmarks/tandem_compare.py: cases() returns 1–12 (label, input) tuples; baseline(input) and candidate(input) return comparable values. Two warmups, seven alternating samples, fresh deep copies outside timing, 120-second limit. These trusted callbacks run with the same isolated check permissions."))
            .child(note("Checks include unsaved buffers. Each run records its own snapshot; a pass is evidence about that version, not a verdict on later edits."))
            .when(!self.dev.check_error.is_empty(),|d|d.child(note(self.dev.check_error.clone())))
            .when(self.dev.discovering,|d|d.child(note("Preparing installed check recipes…")))
            .children(self.dev.recipes.iter().map(|r|{let id=r.id.clone();Button::new(SharedString::from(format!("recipe:{id}"))).label(r.title.clone()).disabled(busy||self.loading).on_click(cx.listener(move|this,_,_,cx|this.prepare_recipe(&id,cx)))}))
            .when_some(pending,|d,recipe|d.child(div().v_flex().gap_2().p_2().border_1().border_color(cx.theme().border)
                .child(note(format!("Authorize {} for this project",recipe.title)))
                .child(note(recipe.argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ")))
                .child(note("Read: captured inputs and the selected tool environments listed below. Write: isolated artifacts. Original source and tool network access are denied; a selected project virtualenv is a read-only exception. Build scripts/tests execute within this scope. Limit: 120 seconds."))
                .children(recipe.read_roots.iter().enumerate().map(|(i,path)|div().id(("recipe-read-root",i)).child(note(format!("Read-only: {}",path.display())))))
                .child(note("This grant is reused only while the executable and recipe fingerprint stay unchanged. It grants no AI source editing."))
                .child(Button::new("authorize-check").label("Authorize Recipe & Run").on_click(cx.listener(|this,_,_,cx|this.authorize_recipe(cx))))
                .child(Button::new("dismiss-recipe").label("Keep Editing").on_click(cx.listener(|this,_,_,cx|{this.dev.pending_recipe=None;cx.notify();})))))
            .when(busy,|d|d.child(Button::new("cancel-check").label("Cancel Active Check").on_click(cx.listener(|this,_,_,cx|{
                if let Some(cancel)=&this.dev.check_cancel{cancel.store(true,Ordering::Relaxed);}this.message("Cancellation requested. Waiting for the command's terminal result…",false,cx);
            }))))
            .when_some(record,|d,record|{
                let output=record.output.clone();
                let mut d=d.child(div().font_semibold().child(note(format!("{} · {:?}",record.recipe_id,record.status)))).child(note(record.detail.clone()))
                    .child(note(format!("Receipt: {}",record.id))).child(note(format!("Exit: {}",record.exit_code.map(|c|c.to_string()).unwrap_or_else(||"not recorded".into()))));
                if let Some(snapshot)=&record.snapshot {d=d.child(note(format!("Snapshot {} · {} files · {} omitted",&snapshot.id[..12],snapshot.files.len(),snapshot.omitted.len())))
                    .children(snapshot.omitted.iter().take(8).enumerate().map(|(i,o)|div().id(("omitted",i)).text_xs().child(note(format!("Omitted {}: {}",o.path,o.reason)))));}
                if let Some(measurement)=&record.benchmark {d=d.child(note(measurement.summary())).child(note(format!("Interpreter: {}",record.executable_version)));}
                d.child(Button::new("copy-check-output").label("Copy Check Output").on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(output.clone()))))
                    .when(record.output_truncated,|d|d.child(note("Output reached its capture limit.")))
                    .children(record.diagnostics.iter().enumerate().map(|(i,item)|{let item=item.clone();Button::new(("check-diagnostic",i)).label(format!("Captured {}:{} {}",item.path,item.line,item.message)).on_click(cx.listener(move|this,_,window,cx|this.open_file(item.path.clone(),false,Some(item.line as usize),window,cx)))}))
                    .child(div().font_semibold().child(note("Output (last 80 lines)")))
                    .children(record.output.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().enumerate().map(|(i,line)|div().id(("output-line",i)).text_xs().child(note(line.chars().take(350).collect::<String>()))))
            })
            .when(!self.dev.checks.is_empty(),|d|d.child(div().font_semibold().child(note("Recent receipts"))).children(self.dev.checks.iter().take(10).map(|record|{
                let id=record.id.clone();Button::new(SharedString::from(format!("receipt:{id}"))).label(format!("{} · {:?} · {}",record.recipe_id,record.status,&id[..8])).on_click(cx.listener(move|this,_,_,cx|{this.dev.selected_check=Some(id.clone());cx.notify();}))
            })))
            .into_any_element()
    }
    fn task_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let task = self
            .dev
            .tasks
            .iter()
            .find(|t| Some(&t.id) == self.dev.selected_task.as_ref());
        div().v_flex().gap_3().child(div().font_semibold().child(note("Task and current step")))
            .child(note("Create or revisit a saved plan. Use Companion for AI analysis, guidance and review."))
            .when(self.project.is_none(),|d|d.child(note("Open a project to create a task.")))
            .when(self.project.is_some()&&!self.dev.task_editor,|d|d.child(Button::new("new-task").label("New Task").on_click(cx.listener(|this,_,window,cx|this.edit_task(false,window,cx)))))
            .when(self.dev.task_editor,|d|d
                .child(note("Goal")).child(Input::new(&self.dev.goal).aria_label("Task goal"))
                .child(note("Acceptance criteria")).child(Textarea::new(&self.dev.criteria).aria_label("Task acceptance criteria"))
                .child(note("Steps, one per line, in dependency order")).child(Textarea::new(&self.dev.steps).aria_label("Task steps"))
                .when(self.dev.editing_task.is_some(),|d|d.child(note("Saving a revision keeps the previous plan in history and starts the revised steps as pending.")))
                .child(Button::new("save-plan").label("Save Plan").on_click(cx.listener(|this,_,_,cx|this.save_task_plan(cx))))
                .child(Button::new("cancel-plan").label("Cancel Plan Edit").on_click(cx.listener(|this,_,_,cx|{this.dev.task_editor=false;cx.notify();}))))
            .when(!self.dev.task_editor,|d|d.when_some(task,|d,task|d
                .child(div().font_semibold().child(note(task.goal.clone()))).child(note(format!("Plan revision {} · {}/{} steps marked complete",task.revision,task.steps.iter().filter(|s|s.status==StepStatus::Complete).count(),task.steps.len())))
                .children(task.acceptance.iter().enumerate().map(|(i,a)|div().id(("acceptance",i)).child(note(format!("• {a}")))))
                .children(task.steps.iter().enumerate().map(|(i,step)|{
                    let id=step.id.clone();div().id(("step-card",i)).v_flex().gap_1().p_2().border_1().border_color(cx.theme().border)
                        .child(note(format!("{}. {} · {:?}",i+1,step.title,step.status)))
                        .when(!step.why.is_empty(),|d|d.child(note(step.why.clone())))
                        .when(!step.done_when.is_empty(),|d|d.children(step.done_when.iter().enumerate().map(|(i,c)|div().id(("done-when",i)).child(note(format!("Done when: {c}"))))))
                        .when(step.status!=StepStatus::Active&&step.status!=StepStatus::Complete,|d|d.child(Button::new(SharedString::from(format!("step:{id}"))).label("Work on This Step").on_click(cx.listener(move|this,_,_,cx|this.task_progress(Some(id.clone()),cx)))))
                        .when(step.status==StepStatus::Active,|d|d.child(Button::new("complete-active-step").label("Mark Step Complete").on_click(cx.listener(|this,_,_,cx|this.task_progress(None,cx)))))
                        .when_some(step.completion_note.clone(),|d,text|d.child(note(text))).child(note(format!("{} linked check receipt(s)",step.evidence.len())))
                }))
                .child(Button::new("revise-plan").label("Revise Plan").on_click(cx.listener(|this,_,window,cx|this.edit_task(true,window,cx))))))
            .when(self.dev.tasks.iter().any(|t|Some(&t.id)!=self.dev.selected_task.as_ref())&&!self.dev.task_editor,|d|d.child(note("Saved tasks")).children(self.dev.tasks.iter().filter(|t|Some(&t.id)!=self.dev.selected_task.as_ref()).map(|task|{
                let id=task.id.clone();Button::new(SharedString::from(format!("task:{id}"))).label(task.goal.clone()).on_click(cx.listener(move|this,_,_,cx|{this.dev.selected_task=Some(id.clone());this.companion.selected_run=None;this.companion.proposal_fields=None;this.companion.solution=None;this.companion.status="Saved task restored.".into();this.dev.scroll.set_offset(point(px(0.0),px(0.0)));cx.notify();}))
            })))
            .into_any_element()
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
