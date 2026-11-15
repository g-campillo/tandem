use super::*;
use gpui_kit::component::input::TextareaState;
use std::collections::BTreeSet;
use tandem_companion::{
    CompanionSession, ConnectionKind, ConnectionState, capture_context, credential_like,
    matching_check_evidence,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreferenceScope {
    Global,
    Language,
    Project,
    ProjectLanguage,
    Step,
    StepLanguage,
}
pub struct ProposedFields {
    pub goal: Entity<InputState>,
    pub criteria: Entity<TextareaState>,
    pub steps: Vec<StepFields>,
}
pub struct StepFields {
    pub title: Entity<InputState>,
    pub why: Entity<TextareaState>,
    pub files: Entity<InputState>,
    pub done: Entity<TextareaState>,
    pub dependencies: Entity<InputState>,
}
pub struct CompanionUi {
    pub settings: GuidanceSettings,
    pub scope: PreferenceScope,
    pub show_settings: bool,
    pub show_context: bool,
    pub show_connection: bool,
    pub show_plan: bool,
    pub show_history: bool,
    pub language: String,
    pub runtime: Arc<Mutex<Option<CompanionSession>>>,
    pub connection: Option<ConnectionState>,
    pub busy: bool,
    pub status: String,
    pub cancel: Option<Arc<AtomicBool>>,
    pub updates: Option<Arc<Mutex<Option<ModelRun>>>>,
    pub question: Entity<TextareaState>,
    pub context_paths: Entity<TextareaState>,
    pub context: Option<ContextSnapshot>,
    pub context_busy: bool,
    pub context_job: u64,
    pub code_epoch: u64,
    pub context_epoch: u64,
    pub runs: Vec<ModelRun>,
    pub selected_run: Option<String>,
    pub draft_id: String,
    pub proposal_fields: Option<ProposedFields>,
    pub edit_proposal: bool,
    pub solution: Option<(String, Entity<EditorState>)>,
    pub collapsed: BTreeSet<String>,
    pub auto_requests: BTreeSet<String>,
    pub inflight_policy_sequence: Option<u64>,
    automatic_request: bool,
}
impl CompanionUi {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            settings: GuidanceSettings::default(),
            scope: PreferenceScope::ProjectLanguage,
            show_settings: false,
            show_context: false,
            show_connection: false,
            show_plan: false,
            show_history: false,
            language: "rust".into(),
            runtime: Arc::new(Mutex::new(None)),
            connection: None,
            busy: false,
            status: "Companion ready to connect. You control source changes.".into(),
            cancel: None,
            updates: None,
            question: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("Describe a feature, ask a question, or revise the approach")
            }),
            context_paths: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(2)
                    .placeholder("Optional extra context paths, one per line")
            }),
            context: None,
            context_busy: false,
            context_job: 0,
            code_epoch: 0,
            context_epoch: 0,
            runs: vec![],
            selected_run: None,
            draft_id: new_id(),
            proposal_fields: None,
            edit_proposal: false,
            solution: None,
            collapsed: BTreeSet::new(),
            auto_requests: BTreeSet::new(),
            inflight_policy_sequence: None,
            automatic_request: false,
        }
    }
    pub fn reset_project(
        &mut self,
        settings: GuidanceSettings,
        runs: Vec<ModelRun>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let runtime = self.runtime.clone();
        let connection = self.connection.clone();
        *self = Self::new(window, cx);
        self.runtime = runtime;
        self.connection = connection;
        self.settings = settings;
        self.selected_run = runs.first().map(|r| r.id.clone());
        self.runs = runs;
    }
}
impl Drop for CompanionUi {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Workspace {
    pub(super) fn active_task(&self) -> Option<&TaskPlan> {
        self.dev
            .tasks
            .iter()
            .find(|t| Some(&t.id) == self.dev.selected_task.as_ref())
    }
    pub(super) fn effective_guidance(&self) -> EffectiveGuidance {
        self.companion.settings.effective(
            self.project.as_ref().map_or("", Project::key),
            &self.companion.language,
            self.active_task(),
        )
    }
    pub(super) fn invalidate_companion(&mut self, cx: &mut Context<Self>) {
        self.companion.code_epoch = self.companion.code_epoch.saturating_add(1);
        self.companion.context = None;
        cx.notify();
    }
    pub(super) fn selected_context_paths(&self, cx: &App) -> Vec<String> {
        let mut paths = self
            .tabs
            .iter()
            .map(|t| t.document.path.clone())
            .collect::<BTreeSet<_>>();
        if paths.is_empty() {
            paths.extend(
                self.index
                    .files
                    .iter()
                    .filter(|p| p.ends_with(".rs") || p.ends_with(".py"))
                    .take(6)
                    .cloned(),
            );
        }
        paths.extend(
            self.index
                .files
                .iter()
                .filter(|p| {
                    p.starts_with("tests/")
                        || std::path::Path::new(p)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("test_") && n.ends_with(".py"))
                })
                .take(4)
                .cloned(),
        );
        if let Some(task) = self.active_task() {
            let relevant = task
                .active_step
                .as_ref()
                .and_then(|id| task.steps.iter().find(|s| &s.id == id));
            for step in task
                .steps
                .iter()
                .filter(|s| {
                    relevant
                        .is_none_or(|active| s.id == active.id || active.depends_on.contains(&s.id))
                })
                .take(6)
            {
                paths.extend(step.files.iter().cloned());
            }
        }
        paths.extend(
            self.companion
                .context_paths
                .read(cx)
                .value()
                .lines()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string),
        );
        paths.into_iter().collect()
    }
    pub(super) fn refresh_companion_context(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        self.snapshot_live(cx);
        let paths = self.selected_context_paths(cx);
        let overlays = match self.working_snapshots(cx) {
            Ok(documents) => documents,
            Err(e) => {
                self.message(format!("Could not capture retained drafts: {e}"), true, cx);
                return;
            }
        };
        let epoch = self.epoch;
        let code_epoch = self.companion.code_epoch;
        self.companion.context_job += 1;
        let job = self.companion.context_job;
        self.companion.context_busy = true;
        let cache = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Caches/Tandem/companion-context");
        let environment = self.dev.environment.clone();
        let ids = self
            .dev
            .recipes
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>();
        let operation = cx.background_spawn(async move {
            let mut context =
                capture_context(&project, &overlays, &paths, &cache, &AtomicBool::new(false))?;
            tandem_companion::bind_validation_environment(&mut context, environment.as_ref(), &ids);
            Ok::<_, anyhow::Error>(context)
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch || view.companion.context_job != job {
                    return;
                }
                view.companion.context_busy = false;
                if view.companion.code_epoch != code_epoch {
                    view.companion.status =
                        "Code changed during capture; refresh context again.".into();
                    cx.notify();
                    return;
                }
                match result {
                    Ok(context) => {
                        view.companion.context = Some(context);
                        view.companion.context_epoch = code_epoch;
                    }
                    Err(e) => view.companion.status = format!("Context unavailable: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn companion_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let update = self
            .companion
            .updates
            .as_ref()
            .and_then(|slot| slot.lock().unwrap().take());
        if let Some(run) = update {
            self.receive_model_run(run, cx);
        }
        if self.companion.busy || self.agent.busy || self.loading {
            return;
        }
        let guidance = self.effective_guidance();
        if guidance.solution != SolutionPolicy::AutomaticPreview {
            return;
        }
        let Some(task) = self.active_task() else {
            return;
        };
        let Some(step) = &task.active_step else {
            return;
        };
        if task.steps.iter().find(|s| &s.id == step).is_some_and(|s| {
            !s.languages.is_empty() && !s.languages.contains(&self.companion.language)
        }) {
            return;
        }
        let key = format!(
            "{}:{}:{}:{}:{}",
            task.id,
            task.revision,
            step,
            self.companion.language,
            guidance.fingerprint()
        );
        if self.companion.runs.iter().any(|r| {
            r.status == ModelStatus::Completed
                && r.request.action == CompanionAction::Reveal
                && r.binding.task_id.as_deref() == Some(&task.id)
                && r.binding.plan_revision == task.revision
                && r.binding.step_id.as_ref() == Some(step)
                && r.request.language == self.companion.language
                && r.binding.guidance_fingerprint == guidance.fingerprint()
        }) {
            self.companion.auto_requests.insert(key);
            return;
        }
        if self.companion.auto_requests.insert(key) {
            self.companion.automatic_request = true;
            self.run_companion(CompanionAction::Reveal, window, cx);
            self.companion.automatic_request = false;
        }
    }
    pub(super) fn connect_companion(&mut self, login: bool, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        self.companion.busy = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.companion.cancel = Some(cancel.clone());
        self.companion.status = if login {
            "Opening official ChatGPT sign-in; complete it in the browser."
        } else {
            "Connecting to the subscription runtime…"
        }
        .into();
        let runtime = self.companion.runtime.clone();
        let operation = cx.background_spawn(async move {
            let mut slot = runtime
                .lock()
                .map_err(|_| anyhow!("Companion process unavailable"))?;
            if slot
                .as_ref()
                .is_some_and(|s| s.connection.kind == ConnectionKind::Unavailable)
            {
                slot.take();
            }
            if slot.is_none() {
                *slot = Some(CompanionSession::connect()?);
            }
            let session = slot.as_mut().unwrap();
            if login {
                session.login(&cancel)
            } else {
                session.refresh()
            }
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                view.companion.busy = false;
                view.companion.cancel = None;
                match result {
                    Ok(state) => {
                        view.companion.status = state.detail.clone();
                        view.companion.connection = Some(state);
                    }
                    Err(e) => {
                        view.companion.show_connection = true;
                        view.companion.status = e.to_string();
                        view.companion.connection = Some(ConnectionState {
                            kind: ConnectionKind::Unavailable,
                            detail: "Connection unavailable. Editing remains usable.".into(),
                            plan: None,
                            limits: vec![],
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn disconnect_companion(&mut self, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let runtime = self.companion.runtime.clone();
        cx.background_spawn(async move {
            runtime.lock().unwrap().take();
        })
        .detach();
        self.companion.connection = None;
        self.companion.status =
            "Disconnected. Editing, tasks and completed responses remain available.".into();
        cx.notify();
    }
    fn conversation_name(&self, action: CompanionAction) -> String {
        format!(
            "{}:{}:{}",
            self.dev
                .selected_task
                .as_deref()
                .unwrap_or(&self.companion.draft_id),
            if action == CompanionAction::Reveal {
                "solution"
            } else {
                "main"
            },
            self.companion.language
        )
    }
    pub(super) fn run_companion(
        &mut self,
        action: CompanionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy || self.loading {
            return;
        }
        let Some(project) = self.project.clone() else {
            self.message("Open a project before starting companion work.", false, cx);
            return;
        };
        // A standing preview preference authorizes the accepted step, not an
        // unfinished question the user happens to be drafting in the text box.
        let typed = if self.companion.automatic_request {
            String::new()
        } else {
            self.companion.question.read(cx).value().trim().to_string()
        };
        if typed.len() > 16000 || credential_like(&typed) {
            self.companion.status =
                "Request not sent: remove credential-like content or shorten it to 16 KiB.".into();
            cx.notify();
            return;
        }
        let task = self.active_task().cloned();
        let guidance = self.effective_guidance();
        let language = self.companion.language.clone();
        let question = if typed.is_empty() {
            match action{
            CompanionAction::Discuss | CompanionAction::DraftSpec | CompanionAction::DraftPlan | CompanionAction::PrepareWalkthrough | CompanionAction::ReviewAction => "Use the durable conversation coordinator for this request.".into(),
            CompanionAction::Analyze=>task.as_ref().map(|t|format!("Reassess and revise the plan for: {}",t.goal)).unwrap_or_default(),
            CompanionAction::Guide=>"Guide the current step at my selected level. Explain what to do, why, and how to know it is complete. Keep the implementation hidden.".into(),
            CompanionAction::Hint=>"Give one useful hint for the current step without revealing its implementation.".into(),
            CompanionAction::Explain=>"Explain the relevant concepts in the selected code, proposed plan or current step at my chosen depth, without showing a full implementation.".into(),
            CompanionAction::Reveal=>"Show one complete suggested implementation for the current step in the selected language, with section explanations and tradeoffs. This grants no editing authority.".into(),
            CompanionAction::Review=>"Review the current captured implementation against this step, accepted requirements and architecture. Accept any valid alternative. Cite actual check evidence and distinguish verified problems from concerns. Do not supply a replacement solution.".into(),CompanionAction::Ask=>String::new(),
            CompanionAction::Delegate=>return,
            CompanionAction::ImproveClarity|CompanionAction::ImproveSpeed|CompanionAction::ImproveBalanced=>format!("Evaluate the selected code for {:?}. Preserve behavior; state preconditions, tradeoffs and validation gaps. Keep current code when change is not justified. Separate expected improvements from actual benchmark evidence. Proposals only; no replacement implementation.",action)}
        } else {
            typed.clone()
        };
        if question.is_empty() {
            self.companion.status = "Describe the desired outcome or ask a question first.".into();
            cx.notify();
            return;
        }
        if matches!(
            action,
            CompanionAction::Guide | CompanionAction::Hint | CompanionAction::Review
        ) && task.is_none()
        {
            self.companion.status = "Analyze and accept a task plan first.".into();
            cx.notify();
            return;
        }
        if let Err(e) = self.flush(cx) {
            self.message(
                format!("Request not started: drafts could not be preserved: {e}"),
                true,
                cx,
            );
            return;
        }
        self.snapshot_live(cx);
        let overlays = match self.working_snapshots(cx) {
            Ok(documents) => documents,
            Err(e) => {
                self.message(format!("Could not capture retained drafts: {e}"), true, cx);
                return;
            }
        };
        let paths = self.selected_context_paths(cx);
        let mut prior_decisions = self
            .companion
            .runs
            .iter()
            .filter(|r| {
                r.status == ModelStatus::Completed
                    && r.request.language == language
                    && r.binding.task_id == task.as_ref().map(|t| t.id.clone())
                    && (task.is_some() || Some(&r.id) == self.companion.selected_run.as_ref())
            })
            .filter_map(|r| match &r.response {
                Some(CompanionResponse::Guidance(g)) => Some(format!(
                    "Question: {}\nResponse: {}",
                    r.request.question, g.summary
                )),
                Some(CompanionResponse::Plan(plan)) => Some(format!(
                    "Proposed, not yet accepted plan: {}",
                    serde_json::to_string(plan)
                        .unwrap_or_default()
                        .chars()
                        .take(12000)
                        .collect::<String>()
                )),
                _ => None,
            })
            .take(4)
            .collect::<Vec<_>>();
        if action == CompanionAction::Explain
            && let Some(run) = self
                .companion
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.companion.selected_run.as_ref())
            && run.displayable(&guidance, &language)
            && let Some(CompanionResponse::Solution(solution)) = &run.response
        {
            prior_decisions.push(format!(
                "Already revealed suggestion to explain: {}",
                solution.code
            ));
        }
        let context = ContextSnapshot {
            validation_environments: Default::default(),
            id: String::new(),
            project_fingerprint: String::new(),
            files: vec![],
            tree: vec![],
            omitted: vec![],
            captured_at_ms: now_ms(),
        };
        let preference = self.store.lock().unwrap().model_preference();
        let model_preference = match preference {
            Ok(preference) => preference,
            Err(error) => {
                self.message(format!("Request not started: {error}"), true, cx);
                return;
            }
        };
        let conversation_id = Some(
            self.dev
                .selected_task
                .clone()
                .unwrap_or_else(|| self.companion.draft_id.clone()),
        );
        let request = ModelRequest {
            model_preference,
            resolved_model: None,
            conversation_id,
            guided: None,
            delegation: None,
            action,
            question,
            language,
            guidance,
            task,
            context,
            evidence: vec![],
            prior_decisions,
        };
        let mut run = ModelRun::pending(project.key().into(), request);
        let saved = self.store.lock().unwrap().save_model_run(&run);
        if let Err(e) = saved {
            self.message(format!("Request not started: {e}"), true, cx);
            return;
        }
        self.companion.selected_run = Some(run.id.clone());
        self.companion.proposal_fields = None;
        self.companion.solution = None;
        self.receive_model_run(run.clone(), cx);
        self.companion.inflight_policy_sequence = Some(self.companion.settings.sequence);
        self.companion.busy = true;
        let cancel = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(Mutex::new(None));
        self.companion.cancel = Some(cancel.clone());
        self.companion.updates = Some(updates.clone());
        let runtime = self.companion.runtime.clone();
        let store = self.store.clone();
        let epoch = self.epoch;
        let name = self.conversation_name(action);
        let checks = self.dev.checks.clone();
        let environment = self.dev.environment.clone();
        let recipe_ids = self
            .dev
            .recipes
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>();
        let cache = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Caches/Tandem/companion-context");
        let operation = cx.background_spawn(async move {
            let result = (|| -> Result<(ModelRun, Option<ConnectionState>)> {
                run.request.context =
                    capture_context(&project, &overlays, &paths, &cache, &cancel)?;
                let recipes = tandem_companion::bind_validation_environment(
                    &mut run.request.context,
                    environment.as_ref(),
                    &recipe_ids,
                );
                run.request.evidence =
                    matching_check_evidence(&run.request.context, &checks, &recipes);
                run.binding = run.request.binding();
                run.sequence += 1;
                store.lock().unwrap().save_model_run(&run)?;
                *updates.lock().unwrap() = Some(run.clone());
                let binding = store
                    .lock()
                    .unwrap()
                    .companion_thread(project.key(), &name)?;
                let mut slot = runtime
                    .lock()
                    .map_err(|_| anyhow!("Companion process unavailable"))?;
                if slot
                    .as_ref()
                    .is_some_and(|s| s.connection.kind == ConnectionKind::Unavailable)
                {
                    slot.take();
                }
                if slot.is_none() {
                    *slot = Some(CompanionSession::connect()?);
                }
                let session = slot.as_mut().unwrap();
                let (finished, binding) =
                    session.run(run.clone(), binding, &cancel, |progress| {
                        store.lock().unwrap().save_model_run(progress)?;
                        *updates.lock().unwrap() = Some(progress.clone());
                        Ok(())
                    })?;
                if let Some(binding) = binding {
                    store
                        .lock()
                        .unwrap()
                        .save_companion_thread(project.key(), &name, &binding)?;
                } else {
                    store
                        .lock()
                        .unwrap()
                        .forget_companion_thread(project.key(), &name)?;
                }
                Ok((finished, Some(session.connection.clone())))
            })();
            match result {
                Ok(result) => Ok(result),
                Err(error) => {
                    run.finish(
                        if cancel.load(Ordering::Relaxed) {
                            ModelStatus::Cancelled
                        } else {
                            ModelStatus::Failed
                        },
                        error.to_string(),
                    );
                    store.lock().unwrap().save_model_run(&run)?;
                    Ok((run, None))
                }
            }
        });
        let weak = cx.entity().downgrade();
        window.spawn(cx,async move|cx|{let result:Result<(ModelRun,Option<ConnectionState>)>=operation.await;let _=weak.update_in(cx,|view,window,cx|{
            if view.epoch!=epoch{return;}view.companion.busy=false;view.companion.cancel=None;view.companion.updates=None;
            match result{Ok((mut run,connection))=>{if let Some(connection)=connection{view.companion.connection=Some(connection);}
                let current=view.companion.settings.effective(&run.project,&run.request.language,view.active_task());
                if run.status==ModelStatus::Completed && (run.binding.guidance_fingerprint!=current.fingerprint() || view.companion.inflight_policy_sequence.is_some_and(|n|n!=view.companion.settings.sequence)){run.response=None;run.finish(ModelStatus::Quarantined,"Guidance changed while this response was generated. Its content was withheld; request again with the current settings.");let _=view.store.lock().unwrap().save_model_run(&run);}
                let completed=run.status==ModelStatus::Completed;let id=run.id.clone();view.receive_model_run(run,cx);view.select_model_run(&id,window,cx);
                if completed&&view.companion.question.read(cx).value().trim()==typed{view.companion.question.update(cx,|q,cx|q.set_value("",window,cx));}
                view.refresh_companion_context(cx);
                view.companion.inflight_policy_sequence=None;
            },Err(e)=>view.companion.status=format!("Could not record the final response: {e}. No request was replayed.")};cx.notify();
        });}).detach();
        cx.notify();
    }
    fn receive_model_run(&mut self, mut run: ModelRun, cx: &mut Context<Self>) {
        if run.status == ModelStatus::Completed
            && self
                .companion
                .inflight_policy_sequence
                .is_some_and(|n| n != self.companion.settings.sequence)
        {
            run.response = None;
            run.finish(ModelStatus::Quarantined,"Guidance changed during generation; content was withheld. Request again with current settings.");
            let _ = self.store.lock().unwrap().save_model_run(&run);
        }
        self.companion.status = run.detail.clone();
        if let Some(existing) = self.companion.runs.iter_mut().find(|r| r.id == run.id) {
            if existing.sequence <= run.sequence {
                *existing = run;
            }
        } else {
            self.companion.runs.insert(0, run);
            self.companion.runs.truncate(40);
        }
        cx.notify();
    }
    pub(super) fn run_is_current(&self, run: &ModelRun, cx: &App) -> bool {
        if !run.request.context.files.iter().all(|file| {
            self.tabs
                .iter()
                .find(|t| t.document.path == file.path)
                .is_none_or(|tab| {
                    content_hash(
                        &tab.document
                            .format
                            .encode(&tab.editor.read(cx).text().to_string()),
                    ) == file.hash
                })
        }) {
            return false;
        }

        self.companion.context.as_ref().is_some_and(|context| {
            self.companion.context_epoch == self.companion.code_epoch
                && run.binding.current(
                    context,
                    self.active_task(),
                    &self.effective_guidance(),
                    &self.companion.language,
                )
        })
    }
    pub(super) fn select_model_run(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.companion.selected_run = Some(id.into());
        self.companion.proposal_fields = None;
        self.companion.solution = None;
        self.companion.edit_proposal = false;
        let response = self
            .companion
            .runs
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.response.clone());
        match response {
            Some(CompanionResponse::Plan(plan)) => {
                self.companion.proposal_fields = Some(ProposedFields {
                    goal: cx.new(|cx| InputState::new(window, cx).default_value(plan.goal)),
                    criteria: cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .rows(3)
                            .default_value(plan.acceptance.join("\n"))
                    }),
                    steps: plan
                        .steps
                        .into_iter()
                        .map(|s| StepFields {
                            title: cx.new(|cx| InputState::new(window, cx).default_value(s.title)),
                            why: cx.new(|cx| {
                                TextareaState::new(window, cx).rows(2).default_value(s.why)
                            }),
                            files: cx.new(|cx| {
                                InputState::new(window, cx).default_value(s.files.join(", "))
                            }),
                            done: cx.new(|cx| {
                                TextareaState::new(window, cx)
                                    .rows(2)
                                    .default_value(s.done_when.join("\n"))
                            }),
                            dependencies: cx.new(|cx| {
                                InputState::new(window, cx).default_value(
                                    s.depends_on
                                        .iter()
                                        .map(|i| (i + 1).to_string())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                )
                            }),
                        })
                        .collect(),
                });
            }
            Some(CompanionResponse::Solution(solution)) => {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(&solution.language)
                        .default_value(solution.code)
                });
                self.companion.solution = Some((id.into(), editor));
            }
            _ => {}
        }
        cx.notify();
    }
    pub(super) fn accept_companion_plan(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self
            .companion
            .runs
            .iter()
            .find(|r| Some(&r.id) == self.companion.selected_run.as_ref())
            .cloned()
        else {
            return;
        };
        if !self.run_is_current(&run, cx) {
            self.companion.status="Context or preferences changed. Refresh context, or analyze again before accepting.".into();
            cx.notify();
            return;
        }
        let Some(CompanionResponse::Plan(mut proposal)) = run.response.clone() else {
            return;
        };
        let Some(fields) = &self.companion.proposal_fields else {
            return;
        };
        let result = (|| -> Result<TaskPlan> {
            proposal.goal = fields.goal.read(cx).value().to_string();
            proposal.acceptance = lines(&fields.criteria.read(cx).value());
            for (step, fields) in proposal.steps.iter_mut().zip(&fields.steps) {
                step.title = fields.title.read(cx).value().to_string();
                step.why = fields.why.read(cx).value().to_string();
                step.files = fields
                    .files
                    .read(cx)
                    .value()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                step.done_when = lines(&fields.done.read(cx).value());
                step.depends_on = fields
                    .dependencies
                    .read(cx)
                    .value()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        s.parse::<usize>()
                            .ok()
                            .and_then(|n| n.checked_sub(1))
                            .context("Dependencies use step numbers beginning at 1")
                    })
                    .collect::<Result<_>>()?;
            }
            anyhow::ensure!(
                proposal.requested_context.is_empty(),
                "Include the requested context and analyze again before accepting this provisional plan"
            );
            let task = proposal.into_plan(&run.project, run.request.task.as_ref())?;
            self.store.lock().unwrap().save_task(&task)?;
            let preference = self.store.lock().unwrap().model_preference()?;
            if let Some(binding) = run.legacy_task_thread(&task.id, &preference) {
                let name = format!("{}:main:{}", task.id, run.request.language);
                self.store
                    .lock()
                    .unwrap()
                    .save_companion_thread(&run.project, &name, &binding)?;
            }
            Ok(task)
        })();
        match result {
            Ok(task) => {
                self.dev.tasks.retain(|t| t.id != task.id);
                self.dev.selected_task = Some(task.id.clone());
                self.dev.tasks.insert(0, task);
                self.dev.scroll.set_offset(point(px(0.0), px(0.0)));
                self.companion.selected_run = None;
                self.companion.proposal_fields = None;
                self.companion.status =
                    "Plan accepted. Choose a step; you remain in control of the implementation."
                        .into();
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn include_requested_context(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.question.read(cx).value().trim().is_empty()
            && let Some(question) = self
                .companion
                .runs
                .iter()
                .find(|r| Some(&r.id) == self.companion.selected_run.as_ref())
                .map(|r| r.request.question.clone())
        {
            self.companion
                .question
                .update(cx, |field, cx| field.set_value(question, window, cx));
        }
        let mut selected = lines(&self.companion.context_paths.read(cx).value())
            .into_iter()
            .collect::<BTreeSet<_>>();
        for path in paths {
            if normalized_relative(std::path::Path::new(&path)).is_ok() {
                selected.insert(path);
            }
        }
        self.companion.context_paths.update(cx, |field, cx| {
            field.set_value(
                selected.into_iter().collect::<Vec<_>>().join("\n"),
                window,
                cx,
            )
        });
        self.refresh_companion_context(cx);
        self.companion.status =
            "Requested paths added. Analyze or review again using the expanded context.".into();
        cx.notify();
    }
    pub(super) fn current_patch(&self) -> GuidancePatch {
        let settings = &self.companion.settings;
        let language = &self.companion.language;
        let project = self.project.as_ref().map_or("", Project::key);
        match self.companion.scope {
            PreferenceScope::Global => Some(&settings.defaults),
            PreferenceScope::Language => settings.languages.get(language),
            PreferenceScope::Project => settings.projects.get(project).map(|p| &p.defaults),
            PreferenceScope::ProjectLanguage => settings
                .projects
                .get(project)
                .and_then(|p| p.languages.get(language)),
            PreferenceScope::Step => self
                .active_task()
                .and_then(|t| t.active_step.as_ref())
                .and_then(|id| settings.steps.get(id))
                .map(|p| &p.defaults),
            PreferenceScope::StepLanguage => self
                .active_task()
                .and_then(|t| t.active_step.as_ref())
                .and_then(|id| settings.steps.get(id))
                .and_then(|p| p.languages.get(language)),
        }
        .cloned()
        .unwrap_or_default()
    }
    pub(super) fn change_guidance(&mut self, field: &str, cx: &mut Context<Self>) {
        let mut patch = self.current_patch();
        match field {
            "instructions" => {
                patch.instructions = match patch.instructions {
                    None => Some(InstructionDetail::HighLevel),
                    Some(InstructionDetail::HighLevel) => Some(InstructionDetail::Standard),
                    Some(InstructionDetail::Standard) => Some(InstructionDetail::Detailed),
                    Some(InstructionDetail::Detailed) => None,
                }
            }
            "explanation" => {
                patch.explanation = match patch.explanation {
                    None => Some(ExplanationDepth::Concise),
                    Some(ExplanationDepth::Concise) => Some(ExplanationDepth::Standard),
                    Some(ExplanationDepth::Standard) => Some(ExplanationDepth::Detailed),
                    Some(ExplanationDepth::Detailed) => None,
                }
            }
            "solution" => {
                patch.solution = match patch.solution {
                    None => Some(SolutionPolicy::Hidden),
                    Some(SolutionPolicy::Hidden) => Some(SolutionPolicy::OnRequest),
                    Some(SolutionPolicy::OnRequest) => Some(SolutionPolicy::AutomaticPreview),
                    Some(SolutionPolicy::AutomaticPreview) => None,
                }
            }
            _ => patch = GuidancePatch::default(),
        }
        let Some(project) = &self.project else {
            return;
        };
        let mut settings = self.companion.settings.clone();
        let language = self.companion.language.clone();
        match self.companion.scope {
            PreferenceScope::Global => settings.defaults = patch,
            PreferenceScope::Language => {
                settings.languages.insert(language, patch);
            }
            PreferenceScope::Project => {
                settings
                    .projects
                    .entry(project.key().into())
                    .or_default()
                    .defaults = patch
            }
            PreferenceScope::ProjectLanguage => {
                settings
                    .projects
                    .entry(project.key().into())
                    .or_default()
                    .languages
                    .insert(language, patch);
            }
            PreferenceScope::Step | PreferenceScope::StepLanguage => {
                let Some(task) = self.active_task() else {
                    return;
                };
                let Some(id) = &task.active_step else {
                    return;
                };
                let step = settings.steps.entry(id.clone()).or_default();
                step.plan_revision = task.revision;
                if self.companion.scope == PreferenceScope::Step {
                    step.defaults = patch;
                } else {
                    step.languages.insert(language, patch);
                }
            }
        }
        settings.sequence += 1;
        let result = self.store.lock().unwrap().save_guidance(&settings);
        match result {
            Ok(()) => {
                self.companion.settings = settings;
                self.companion.status =
                    "Guidance preference saved. Editing authority remains off.".into();
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn new_companion_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        self.dev.selected_task = None;
        self.companion.draft_id = new_id();
        self.companion.selected_run = None;
        self.companion.proposal_fields = None;
        self.companion.solution = None;
        self.companion.question.update(cx, |field, cx| {
            field.set_value("", window, cx);
            field.focus(window, cx);
        });
        self.companion.status = "Describe the new goal, then analyze its context and plan.".into();
        cx.notify();
    }
}
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
