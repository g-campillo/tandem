use super::panels::note;
use super::*;
use crate::appearance::{
    navigation_button, palette, panel_body, panel_heading, panel_metadata, panel_warning,
    path_button, wrapping_button,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use std::collections::BTreeSet;

pub(super) struct AgentUi {
    pub busy: bool,
    pub cancel: Option<Arc<AtomicBool>>,
    pub updates: Option<Arc<Mutex<Option<AgentRun>>>>,
    pub runs: Vec<AgentRun>,
    pub selected: Option<String>,
    pub imports: Vec<ImportTransaction>,
    scope: DelegationScope,
    paths: Entity<TextareaState>,
    checks: BTreeSet<String>,
    status: String,
    preview: Option<ImportPreview>,
    undo: Option<String>,
}
impl AgentUi {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {busy:false,cancel:None,updates:None,runs:vec![],selected:None,imports:vec![],scope:DelegationScope::Step,
            paths:cx.new(|cx|TextareaState::new(window,cx).rows(4).placeholder("Exact relative paths, one per line")), checks:BTreeSet::new(),status:"Prepare a scope before delegating. Source changes stay isolated until you import them.".into(),preview:None,undo:None}
    }
    fn selected(&self) -> Option<&AgentRun> {
        self.runs
            .iter()
            .find(|r| Some(&r.id) == self.selected.as_ref())
    }
}
impl Drop for AgentUi {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Workspace {
    fn receive_agent(&mut self, run: AgentRun, cx: &mut Context<Self>) {
        self.agent.selected = Some(run.id.clone());
        self.agent.status = run.summary.clone();
        self.agent.runs.retain(|r| r.id != run.id);
        self.agent.runs.insert(0, run);
        cx.notify();
    }
    pub(super) fn poll_agent(&mut self, cx: &mut Context<Self>) {
        let update = self
            .agent
            .updates
            .as_ref()
            .and_then(|u| u.lock().unwrap().take());
        if let Some(run) = update {
            self.receive_agent(run, cx);
        }
    }
    fn prepare_agent(&mut self, cx: &mut Context<Self>) {
        if self.agent.busy || self.companion.busy || self.dev.check_cancel.is_some() || self.loading
        {
            return;
        }
        let result = (|| -> Result<_> {
            let workflow = super::companion_state::command_binding(
                self.companion.session.as_ref(),
                CompanionAction::Delegate,
                false,
            )?;
            let task = self
                .active_task()
                .cloned()
                .context("Accept a task plan first")?;
            let step = task
                .active_step
                .as_ref()
                .and_then(|id| task.steps.iter().find(|s| &s.id == id));
            anyhow::ensure!(
                self.agent.scope != DelegationScope::Step || step.is_some(),
                "Select an active step first"
            );
            self.flush(cx)?;
            let working = self.working_snapshots(cx)?;
            let project = self.project.clone().context("Open a project")?;
            let paths = self
                .agent
                .paths
                .read(cx)
                .value()
                .lines()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let paths = if paths.is_empty() {
                if self.agent.scope == DelegationScope::Step {
                    step.unwrap().files.clone()
                } else {
                    task.steps
                        .iter()
                        .flat_map(|s| s.files.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                }
            } else {
                paths
            };
            let environment = self
                .dev
                .environment
                .as_ref()
                .context("Wait for check tool discovery")?;
            let checks = self
                .agent
                .checks
                .iter()
                .map(|id| Ok((id.clone(), environment.recipe(id)?.grant_fingerprint)))
                .collect::<Result<_>>()?;
            let grant = DelegationGrant {
                id: new_id(),
                workflow: Some(workflow),
                action_scope: self
                    .companion
                    .session
                    .as_ref()
                    .and_then(DelegationActionScope::current),
                project: project.key().into(),
                task_id: task.id.clone(),
                plan_revision: task.revision,
                step_id: if self.agent.scope == DelegationScope::Step {
                    task.active_step.clone()
                } else {
                    None
                },
                scope: self.agent.scope,
                objective: if self.agent.scope == DelegationScope::Step {
                    format!(
                        "{}\nStep: {}\n{}\nAcceptance: {}",
                        task.goal,
                        step.unwrap().title,
                        step.unwrap().why,
                        if step.unwrap().done_when.is_empty() {
                            task.acceptance.join("; ")
                        } else {
                            step.unwrap().done_when.join("; ")
                        }
                    )
                } else {
                    task.goal.clone()
                },
                paths,
                checks,
                max_rounds: 4,
                confirmed_at_ms: 0,
            };
            grant.validate()?;
            Ok((
                project,
                working,
                grant,
                tandem_agent::delegation_cache(environment)?,
            ))
        })();
        let (project, working, grant, cache) = match result {
            Ok(v) => v,
            Err(e) => {
                self.agent.status = e.to_string();
                cx.notify();
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.agent.cancel = Some(cancel.clone());
        self.agent.busy = true;
        self.agent.preview = None;
        self.agent.status = "Capturing isolated working source…".into();
        let store = self.store.clone();
        let epoch = self.epoch;
        let operation = cx.background_spawn(async move {
            let run = tandem_agent::prepare_delegation(
                &store, &project, &working, grant, &cache, &cancel,
            )?;
            store.lock().unwrap().save_agent_run(&run)?;
            Ok::<_, anyhow::Error>(run)
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                view.agent.busy = false;
                view.agent.cancel = None;
                match result {
                    Ok(run) => view.receive_agent(run, cx),
                    Err(e) => view.agent.status = format!("Preparation stopped: {e}"),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn start_agent(&mut self, cx: &mut Context<Self>) {
        if self.agent.busy || self.companion.busy || self.dev.check_cancel.is_some() || self.loading
        {
            return;
        }
        let result = (|| -> Result<_> {
            let mut run = self
                .agent
                .selected()
                .cloned()
                .context("Prepare a scope first")?;
            let task = self
                .active_task()
                .cloned()
                .context("Select the accepted task")?;
            anyhow::ensure!(
                run.status == AgentStatus::Prepared
                    && run.grant.confirmed_at_ms == 0
                    && run.grant.matches_task(&task),
                "The prepared plan changed; prepare a new scope"
            );
            anyhow::ensure!(
                self.companion
                    .connection
                    .as_ref()
                    .is_some_and(|c| c.kind == tandem_companion::ConnectionKind::Ready),
                "Connect your ChatGPT subscription in Companion first"
            );
            self.store
                .lock()
                .unwrap()
                .authorize_delegation(&run.grant)?;
            run.grant.confirmed_at_ms = now_ms();
            run.sequence += 1;
            self.store.lock().unwrap().save_agent_run(&run)?;
            Ok((
                run,
                task,
                self.project.clone().context("Open a project")?,
                self.dev
                    .environment
                    .clone()
                    .context("Check environment unavailable")?,
            ))
        })();
        let (run, task, project, environment) = match result {
            Ok(v) => v,
            Err(e) => {
                self.agent.status = e.to_string();
                cx.notify();
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(Mutex::new(None));
        self.agent.busy = true;
        self.agent.cancel = Some(cancel.clone());
        self.agent.updates = Some(updates.clone());
        self.agent.preview = None;
        self.receive_agent(run.clone(), cx);
        let store = self.store.clone();
        let runtime = self.companion.runtime.clone();
        let epoch = self.epoch;
        let operation = cx.background_spawn(async move {
            let result = (|| {
                let mut lock = runtime
                    .lock()
                    .map_err(|_| anyhow!("Connection unavailable"))?;
                let session = lock.as_mut().context("Connect ChatGPT before delegation")?;
                tandem_agent::AgentExecution {
                    session,
                    store: &store,
                    original: &project,
                    task: &task,
                    environment: &environment,
                    cancel: &cancel,
                }
                .run(run.clone(), |update| {
                    *updates.lock().unwrap() = Some(update.clone())
                })
            })();
            if let Err(error) = &result {
                let mut retained = store
                    .lock()
                    .unwrap()
                    .agent_runs(project.key())?
                    .into_iter()
                    .find(|r| r.id == run.id)
                    .unwrap_or(run);
                if retained.status.active() || retained.status == AgentStatus::Prepared {
                    retained.finish(
                        AgentStatus::Failed,
                        format!("Execution stopped: {error}. Completed actions remain recorded."),
                    );
                    store.lock().unwrap().save_agent_run(&retained)?;
                }
                return Ok(retained);
            }
            result
        });
        cx.spawn(async move|view,cx|{let result=operation.await;let _=view.update(cx,|view,cx|{if view.epoch!=epoch{return;}view.agent.busy=false;view.agent.cancel=None;view.agent.updates=None;match result{Ok(run)=>view.receive_agent(run,cx),Err(e)=>view.agent.status=format!("Handoff persistence failed: {e}. Do not replay; inspect recovery state.")};if let Some(p)=&view.project{view.dev.checks=view.store.lock().unwrap().checks(p.key()).unwrap_or_default();}view.invalidate_companion(cx);cx.notify();});}).detach();
        cx.notify();
    }
    pub(super) fn agent_preview(
        &mut self,
        undo: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent.busy
            || self.loading
            || self.tabs.iter().any(|t| t.saving)
            || self.copy_inflight
        {
            return;
        }
        let result = (|| -> Result<ImportPreview> {
            self.flush(cx)?;
            let live = self.working_snapshots(cx)?;
            let project = self.project.as_ref().context("Open a project")?;
            if let Some(id) = &undo {
                let group = self
                    .agent
                    .imports
                    .iter()
                    .find(|g| &g.id == id)
                    .context("Import journal unavailable")?;
                preview_undo(group, project, &live)
            } else {
                let mut run = self
                    .agent
                    .selected()
                    .cloned()
                    .context("Select a completed delegation")?;
                anyhow::ensure!(
                    self.active_task()
                        .is_some_and(|t| run.grant.matches_task(t)),
                    "The accepted plan changed; review the retained diff and prepare a new scope"
                );
                anyhow::ensure!(
                    !self.agent.imports.iter().any(|g| g.agent_run_id == run.id),
                    "This delegation already has an import journal; it cannot be replayed"
                );
                self.store
                    .lock()
                    .unwrap()
                    .authorize_delegation(&run.grant)?;
                tandem_agent::inspect_delegation(&mut run)?;
                self.store.lock().unwrap().save_agent_run(&run)?;
                let preview = preview_import(&run, project, &live)?;
                self.receive_agent(run, cx);
                Ok(preview)
            }
        })();
        match result {
            Ok(preview) => {
                let text = if let Some(group) = &preview.transaction {
                    import_diff(&group.files)
                } else {
                    preview
                        .conflicts
                        .iter()
                        .map(|c| {
                            format!(
                                "CONFLICT {}: {}\n\nBASELINE\n{}\nPROPOSED\n{}\nCURRENT BUFFER\n{}",
                                c.path,
                                c.reason,
                                c.baseline.as_deref().unwrap_or("(absent)"),
                                c.proposed.as_deref().unwrap_or("(absent)"),
                                c.current.as_deref().unwrap_or("(not open; inspect disk)")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                self.diff = Some(cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("diff")
                        .default_value(text)
                }));
                self.agent.status = if preview.conflicts.is_empty() {
                    "Review the proposed buffer changes, then confirm. Files on disk remain untouched.".into()
                } else {
                    "Import blocked by conflicts. All versions are retained; no buffer changed. Resolve the overlap, then preview again.".into()
                };
                self.agent.preview = Some(preview);
                self.agent.undo = undo;
            }
            Err(e) => {
                self.agent.preview = None;
                self.agent.status = e.to_string();
            }
        }
        cx.notify();
    }
    fn commit_agent_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reviewed) = self
            .agent
            .preview
            .as_ref()
            .and_then(|p| p.transaction.clone())
        else {
            return;
        };
        let signature = serde_json::to_string(&reviewed.files).unwrap();
        // Recompute against live editor text and current disk before committing.
        self.agent_preview(self.agent.undo.clone(), window, cx);
        let Some(group) = self
            .agent
            .preview
            .as_ref()
            .and_then(|p| p.transaction.clone())
        else {
            return;
        };
        if serde_json::to_string(&group.files).unwrap() != signature {
            self.agent.status="Code changed since the preview. Inspect the refreshed diff before confirming again.".into();
            cx.notify();
            return;
        }
        let result = (|| -> Result<_> {
            let live = self.working_snapshots(cx)?;
            let working = apply_to_working_set(&live, &group);
            let mut state = self.state().context("Open a project")?;
            for file in &group.files {
                if file.after.is_none() {
                    state.tabs.retain(|p| p != &file.path);
                } else if !state.tabs.contains(&file.path) {
                    state.tabs.push(file.path.clone());
                }
            }
            anyhow::ensure!(
                state.tabs.len() <= 32,
                "Close some tabs before importing this group (32-tab limit)"
            );
            if state
                .active
                .as_ref()
                .is_some_and(|p| !state.tabs.contains(p))
            {
                state.active = state.tabs.first().cloned();
            }
            state.sequence = self.generation.max(self.store.lock().unwrap().sequence()?) + 1;
            self.store
                .lock()
                .unwrap()
                .commit_import(&state, &working, &group)?;
            Ok(state)
        })();
        match result {
            Ok(state) => {
                for file in &group.files {
                    if let Some(after) = &file.after {
                        if let Some(tab) =
                            self.tabs.iter_mut().find(|t| t.document.path == file.path)
                        {
                            tab.editor.update(cx, |editor, cx| {
                                editor.replace_all(after.text.clone(), window, cx)
                            });
                            tab.document = after.clone();
                        } else {
                            self.add_tab(after.clone(), window, cx);
                        }
                    } else {
                        self.tabs.retain(|t| t.document.path != file.path);
                    }
                }
                self.active = state
                    .active
                    .as_ref()
                    .and_then(|p| self.tabs.iter().position(|t| &t.document.path == p))
                    .or_else(|| (!self.tabs.is_empty()).then_some(0));
                self.generation = state.sequence;
                self.checkpointed = state.sequence;
                self.diff = None;
                self.agent.preview = None;
                self.agent.imports.retain(|g| g.id != group.id);
                self.agent.imports.insert(0, group.clone());
                self.invalidate_language(cx);
                self.invalidate_companion(cx);
                self.sync_language(cx);
                self.agent.status = if group.status == ImportStatus::Applied {
                    "Imported into unsaved buffers. Inspect and save when ready. Undo Agent Import preserves unrelated edits.".into()
                } else {
                    "Agent import undone in buffers. Unrelated edits were retained; no file on disk was changed.".into()
                };
            }
            Err(e) => self.agent.status = format!("No buffer changes committed: {e}"),
        };
        cx.notify();
    }
    fn show_staged_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.agent.selected() else {
            return;
        };
        let files = run
            .changed_paths()
            .into_iter()
            .map(|path| ImportedFile {
                before: run.baseline.get(&path).cloned().flatten(),
                after: run.staged.get(&path).cloned().flatten(),
                path,
            })
            .collect::<Vec<_>>();
        let text = format!(
            "Captured staging version {} · round {} · {:?}\nThis is a staged diff, not an import preview.\n\n{}",
            run.sequence,
            run.round,
            run.status,
            import_diff(&files)
        );
        self.diff = Some(cx.new(|cx| {
            EditorState::new(window, cx)
                .language("diff")
                .default_value(text)
        }));
        cx.notify();
    }
    pub(super) fn agent_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let busy = self.agent.busy
            || self.companion.busy
            || self.dev.check_cancel.is_some()
            || self.loading;
        let selected = self.agent.selected();
        let mut body=panel_body(cx).child(panel_heading(note(if self.agent.busy{"Agent takeover · isolated copy"}else{"Companion · AI edits off"}))).child(panel_metadata(note(self.agent.status.clone()),cx))
            .when(self.agent.busy,|d|d.child(path_button(Button::new("stop-agent"), "Stop & Return Control").danger().on_click(cx.listener(|this,_,_,cx|{if let Some(c)=&this.agent.cancel{c.store(true,Ordering::Relaxed);}this.agent.status="Stop requested. Completed actions are retained; waiting for the current operation to settle.".into();cx.notify();}))))
            .child(panel_warning(note("Delegate an accepted step or feature. The agent can create or modify only the exact paths below; no deletions, renames, commits, installation or network access."),cx))
            .child(div().h_flex().flex_wrap().min_w_0().gap_2().children([(DelegationScope::Step,"Current Step"),(DelegationScope::Feature,"Whole Feature")].into_iter().map(|(scope,label)|navigation_button(wrapping_button(Button::new(label), label),self.agent.scope==scope,cx).disabled(busy).on_click(cx.listener(move|this,_,_,cx|{this.agent.scope=scope;cx.notify();})))))
            .child(Textarea::new(&self.agent.paths).aria_label("Delegated source paths"))
            .child(note("Empty paths use the selected step/feature files. Preparation captures current buffers, including unsaved work. Editing these fields does not change an already prepared grant; prepare again to change its scope."))
            .children(self.dev.recipes.iter().map(|r|{let id=r.id.clone();let selected=self.agent.checks.contains(&id);navigation_button(path_button(Button::new(SharedString::from(format!("agent-check:{id}"))), format!("{} {}",if selected{"✓"}else{"○"},r.title)),selected,cx).disabled(busy).on_click(cx.listener(move|this,_,_,cx|{if !this.agent.checks.remove(&id){this.agent.checks.insert(id.clone());}cx.notify();}))}))
            .child(path_button(Button::new("prepare-agent"), "Prepare Delegation Scope").primary().disabled(busy||self.active_task().is_none()).on_click(cx.listener(|this,_,_,cx|this.prepare_agent(cx))));
        if let Some(run) = selected {
            body=body.child(panel_heading(note(format!("{:?} · {:?} · round {}/{} · plan {}",run.status,run.grant.scope,run.round,run.grant.max_rounds,run.grant.plan_revision))).when(run.status==AgentStatus::Failed,|d|d.text_color(p.error)).mt_3().pt_3().border_t_1().border_color(cx.theme().border))
                .child(div().font_semibold().child(note(run.grant.objective.clone()))).child(note(format!("Exact writable paths: {}",run.grant.paths.join(", "))))
                .child(note(format!("Allowed checks: {}",if run.grant.checks.is_empty(){"None (unvalidated handoff)".into()}else{run.grant.checks.keys().cloned().collect::<Vec<_>>().join(", ")})))
                .child(panel_warning(note("Checks read an isolated snapshot and approved tool resources; only their artifact directory is writable. Original project access and network are denied. Four rounds maximum; 16 paths / 256 KiB source."),cx));
            for (i, id) in run.grant.checks.keys().enumerate() {
                if let Some(r) = self.dev.recipes.iter().find(|r| &r.id == id) {
                    body = body.child(super::panels::code_note(
                        ("agent-envelope", i),
                        format!(
                            "Command: {} · {} · read resources: {}",
                            r.argv.join(" "),
                            r.version,
                            r.read_roots
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        cx,
                    ));
                }
            }
            body = body
                .when(
                    run.status == AgentStatus::Prepared && run.grant.confirmed_at_ms == 0,
                    |d| {
                        d.child(
                            path_button(
                                Button::new("authorize-agent"),
                                "Authorize Scope & Start Takeover",
                            )
                            .primary()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.start_agent(cx))),
                        )
                    },
                )
                .child(note(format!(
                    "Changed files: {}",
                    run.changed_paths().join(", ")
                )))
                .children(
                    run.activity.iter().rev().take(8).enumerate().map(|(i, s)| {
                        panel_metadata(note(s.clone()), cx).id(("agent-activity", i))
                    }),
                )
                .children(run.limitations.iter().enumerate().map(|(i, s)| {
                    panel_warning(note(format!("Limitation: {s}")), cx).id(("agent-limit", i))
                }))
                .child(note(format!("Handoff: {}", run.next_action)))
                .child(
                    path_button(
                        Button::new("staged-agent-diff"),
                        "View / Refresh Captured Staged Diff",
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.show_staged_diff(window, cx))),
                )
                .when(
                    !run.status.active() && run.status != AgentStatus::Prepared,
                    |d| {
                        d.child(
                            path_button(
                                Button::new("preview-agent-import"),
                                "Inspect Diff & Preview Import",
                            )
                            .disabled(busy)
                            .on_click(cx.listener(
                                |this, _, window, cx| this.agent_preview(None, window, cx),
                            )),
                        )
                    },
                );
        }
        if self
            .agent
            .preview
            .as_ref()
            .is_some_and(|p| p.transaction.is_some())
        {
            body = body.child(
                path_button(
                    Button::new("commit-agent-import"),
                    if self.agent.undo.is_some() {
                        "Confirm Undo in Buffers"
                    } else {
                        "Import Reviewed Changes to Buffers"
                    },
                )
                .primary()
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.commit_agent_import(window, cx))),
            );
        }
        for (i, group) in self.agent.imports.iter().take(6).enumerate() {
            let id = group.id.clone();
            body = body.child(
                path_button(
                    Button::new(("undo-agent-import", i)),
                    format!(
                        "Undo Agent Import · {:?} · {} files",
                        group.status,
                        group.files.len()
                    ),
                )
                .disabled(busy || group.status != ImportStatus::Applied)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.agent_preview(Some(id.clone()), window, cx)
                })),
            );
        }
        body.children(self.agent.runs.iter().take(8).enumerate().map(|(i, run)| {
            let id = run.id.clone();
            navigation_button(
                path_button(
                    Button::new(("agent-history", i)),
                    format!(
                        "{:?} · {}",
                        run.status,
                        run.grant.objective.chars().take(48).collect::<String>()
                    ),
                ),
                self.agent.selected.as_ref() == Some(&run.id),
                cx,
            )
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.agent.selected = Some(id.clone());
                this.agent.preview = None;
                cx.notify();
            }))
        }))
        .into_any_element()
    }
}
