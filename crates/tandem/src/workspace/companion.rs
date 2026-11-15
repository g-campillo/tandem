use super::companion_state::{self, CompanionTab};
use super::documents_view::DocumentFields;
use super::research_adapter::ProjectInspection;
use super::*;
use gpui_kit::component::input::TextareaState;
use std::collections::BTreeSet;
use tandem_companion::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreferenceScope {
    Global,
    Language,
    Project,
    ProjectLanguage,
    Step,
    StepLanguage,
}

/// The runtime is shared with connection/model listing and separately authorized Takeover.
pub struct SharedBackend(pub Arc<Mutex<Option<CompanionSession>>>);
impl SharedBackend {
    fn with<T>(&self, f: impl FnOnce(&mut CompanionSession) -> Result<T>) -> Result<T> {
        let mut slot = self
            .0
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
        f(slot.as_mut().unwrap())
    }
}
impl ConversationBackend for SharedBackend {
    fn check_account(&mut self, cancel: &AtomicBool) -> Result<()> {
        self.with(|s| ConversationBackend::check_account(s, cancel))
    }
    fn resolve_model(&mut self, p: &ModelPreference, cancel: &AtomicBool) -> Result<ResolvedModel> {
        self.with(|s| ConversationBackend::resolve_model(s, p, cancel))
    }
    fn turn(
        &mut self,
        run: ModelRun,
        thread: Option<ThreadBinding>,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&ModelRun) -> Result<()>,
    ) -> Result<(ModelRun, Option<ThreadBinding>)> {
        self.with(|s| s.run(run, thread, cancel, observe))
    }
}
#[derive(Default)]
pub struct PublicRuntime(Option<PublicDataWorker<tandem_codex::Client, AnonymousPublicHttp>>);
impl PublicWorker for PublicRuntime {
    fn execute(
        &mut self,
        brief: &ScreenedPublicBrief,
        model: &ResolvedModel,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&PublicResearchProgress) -> Result<()>,
    ) -> Result<ResearchReceipt> {
        if self.0.is_none() {
            self.0 = Some(PublicDataWorker {
                search: PublicResearchSession::connect()?,
                sources: PublicSourceFetcher::new()?,
            });
        }
        self.0
            .as_mut()
            .unwrap()
            .execute(brief, model, cancel, observe)
    }
}
type Runner = ConversationRunner<SharedBackend, PublicRuntime>;

/// Each capture asks the UI for a new versioned buffer snapshot. The original
/// project/session guard is checked before the UI supplies any editor content.
#[derive(Default)]
pub struct CaptureMailbox {
    project: String,
    session_id: String,
    operation_id: String,
    requested: bool,
    code_epoch: u64,
    source_generation: u64,
    response: Option<std::result::Result<Vec<Document>, String>>,
}
struct FreshCapture {
    capture: ProjectConversationCapture,
    mailbox: Arc<Mutex<CaptureMailbox>>,
    store: Arc<Mutex<Store>>,
}
impl ConversationCapture for FreshCapture {
    fn capture(
        &mut self,
        session: &TaskSession,
        action: CompanionAction,
        language: &str,
        guidance: &EffectiveGuidance,
        checks: &[CheckRecord],
        cancel: &AtomicBool,
    ) -> Result<CapturedConversation> {
        {
            let mut m = self.mailbox.lock().unwrap();
            m.requested = true;
            m.response = None;
        }
        loop {
            anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Capture cancelled");
            if let Some(response) = self.mailbox.lock().unwrap().response.take() {
                self.capture.overlays = response.map_err(|e| anyhow!(e))?;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let checks = if matches!(
            action,
            CompanionAction::ReviewAction | CompanionAction::Reveal
        ) {
            action_checks(&self.store.lock().unwrap(), session, checks)?
        } else {
            checks.to_vec()
        };
        if matches!(
            action,
            CompanionAction::ReviewAction | CompanionAction::Reveal
        ) {
            self.capture.attachments = self
                .store
                .lock()
                .unwrap()
                .current_action_attachment(session)?
                .into_iter()
                .collect();
        }
        let mut captured = self
            .capture
            .capture(session, action, language, guidance, &checks, cancel)?;
        if let Some(review) = captured.review.take() {
            captured.review = Some(authenticate_capture(
                &self.store.lock().unwrap(),
                session,
                review,
                language,
                guidance,
            )?);
        }
        Ok(captured)
    }
}

fn eligible_check_origin(
    session: &TaskSession,
    scope: &ActionEvidenceScope,
    origin: &ActionEvidenceScope,
) -> bool {
    origin == scope
        || (matches!(
            session.current_action().map(|(_, a)| a.kind),
            Ok(GuidedActionKind::ValidateStep | GuidedActionKind::ValidateTask)
        ) && origin.workflow == scope.workflow
            && session.walkthroughs.iter().any(|w| {
                w.id == origin.walkthrough_id
                    && w.actions[..w.cursor].iter().any(|a| {
                        a.id == origin.action_id && a.definition_hash() == origin.action_hash
                    })
            }))
}
pub(super) fn action_checks(
    store: &Store,
    session: &TaskSession,
    checks: &[CheckRecord],
) -> Result<Vec<CheckRecord>> {
    let (walk, action) = session.current_action()?;
    let scope = ActionEvidenceScope {
        workflow: session.authorize(WorkflowAction::ReviewAction)?,
        walkthrough_id: walk.id.clone(),
        action_id: action.id.clone(),
        action_hash: action.definition_hash(),
    };
    let mut scoped = vec![];
    for check in checks {
        if store
            .check_action_scope(&check.id)?
            .is_some_and(|origin| eligible_check_origin(session, &scope, &origin))
        {
            scoped.push(check.clone());
        }
    }
    Ok(scoped)
}

/// Retain authenticated execution origins, including completed action history
/// for cumulative validation. Never relabel an old check as a new action run.
pub(super) fn authenticate_capture(
    store: &Store,
    session: &TaskSession,
    mut input: ActionReviewInput,
    language: &str,
    guidance: &EffectiveGuidance,
) -> Result<ActionReviewInput> {
    let scope = input.scope();
    let mut commands = vec![];
    for mut command in input.commands.drain(..) {
        if let CommandProvenance::LocalCheck { check_id, .. } = &command.provenance {
            let Some(origin) = store.check_action_scope(check_id)? else {
                continue;
            };
            if !eligible_check_origin(session, &scope, &origin) {
                continue;
            }
            command.scope = origin;
        }
        commands.push(command);
    }
    ActionReviewInput::capture(
        session,
        input.context,
        input.targets,
        commands,
        language,
        guidance,
    )
}

pub struct CompanionUi {
    pub settings: GuidanceSettings,
    pub scope: PreferenceScope,
    pub show_settings: bool,
    pub show_context: bool,
    pub show_connection: bool,
    pub show_history: bool,
    pub language: String,
    pub runtime: Arc<Mutex<Option<CompanionSession>>>,
    runner: Arc<Mutex<Option<Runner>>>,
    pub connection: Option<ConnectionState>,
    pub busy: bool,
    pub status: String,
    pub cancel: Option<Arc<AtomicBool>>,
    pub operation_id: Option<String>,
    pub foreground: Option<companion_state::ForegroundOperation>,
    pub progress: Arc<Mutex<Option<ActiveOperation>>>,
    pub capture: Option<Arc<Mutex<CaptureMailbox>>>,
    pub question: Entity<TextareaState>,
    pub custom_answer: Entity<TextareaState>,
    pub context_paths: Entity<TextareaState>,
    pub context: Option<ContextSnapshot>,
    pub context_busy: bool,
    pub context_job: u64,
    pub code_epoch: u64,
    pub context_epoch: u64,
    pub runs: Vec<ModelRun>,
    pub selected_run: Option<String>,
    pub session: Option<TaskSession>,
    pub sessions: Vec<TaskSession>,
    pub events: Vec<ConversationEvent>,
    pub document_events: Vec<ConversationEvent>,
    pub legacy_history: Vec<companion_state::HistoricalRow>,
    pub receipts: Vec<ResearchReceipt>,
    pub composer: ComposerDraft,
    pub tab: CompanionTab,
    pub documents: Option<DocumentFields>,
    pub document_drafts: companion_state::SessionDocumentDrafts,
    pub solution: Option<(String, Entity<EditorState>)>,
    pub solution_scope: Option<companion_state::SolutionScope>,
    pub solution_history_session: Option<String>,
    pub auto_requests: BTreeSet<String>,
    pub inspected_step: Option<String>,
    pub model_preference: ModelPreference,
    pub catalog: Option<ModelCatalog>,
    pub model_error: Option<String>,
    pub resolved_model: Option<ResolvedModel>,
    pub document_fingerprint: String,
    pub review: Option<(ActionReview, Option<OperationOutcome>)>,
    pub review_commands: Vec<ActionCommandEvidence>,
    pub review_epoch: Option<u64>,
    pub review_source_generation: Option<u64>,
    pub review_stale: bool,
    pub review_refresh_pending: bool,
    pub local_action: Option<super::walkthrough_view::LocalAction>,
    pub attachments: HashMap<String, ActionCommandEvidence>,
    pub attachment_drafts: HashMap<String, companion_state::OutputAttachmentDraft>,
    pub attachment_fields: Option<super::walkthrough_view::AttachmentFields>,
}
impl CompanionUi {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            settings: GuidanceSettings::default(),
            scope: PreferenceScope::ProjectLanguage,
            show_settings: false,
            show_context: false,
            show_connection: false,
            show_history: false,
            language: "rust".into(),
            runtime: Arc::new(Mutex::new(None)),
            runner: Arc::new(Mutex::new(None)),
            connection: None,
            busy: false,
            status: "Discuss your goal. Editing works offline.".into(),
            cancel: None,
            operation_id: None,
            foreground: None,
            progress: Arc::new(Mutex::new(None)),
            capture: None,
            question: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("Discuss your goal or ask a follow-up")
            }),
            custom_answer: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(2)
                    .placeholder("Your own answer")
            }),
            context_paths: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(2)
                    .placeholder("Additional relative context paths")
            }),
            context: None,
            context_busy: false,
            context_job: 0,
            code_epoch: 0,
            context_epoch: 0,
            runs: vec![],
            selected_run: None,
            session: None,
            sessions: vec![],
            events: vec![],
            document_events: vec![],
            legacy_history: vec![],
            receipts: vec![],
            composer: ComposerDraft::default(),
            tab: CompanionTab::Conversation,
            documents: None,
            document_drafts: Default::default(),
            solution: None,
            solution_scope: None,
            solution_history_session: None,
            auto_requests: BTreeSet::new(),
            inspected_step: None,
            model_preference: ModelPreference::Automatic,
            catalog: None,
            model_error: None,
            resolved_model: None,
            document_fingerprint: String::new(),
            review: None,
            review_commands: vec![],
            review_epoch: None,
            review_source_generation: None,
            review_stale: false,
            review_refresh_pending: false,
            local_action: None,
            attachments: HashMap::new(),
            attachment_drafts: HashMap::new(),
            attachment_fields: None,
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
        let runner = self.runner.clone();
        let connection = self.connection.clone();
        let catalog = self.catalog.clone();
        let attachment_drafts = std::mem::take(&mut self.attachment_drafts);
        let attachments = std::mem::take(&mut self.attachments);
        *self = Self::new(window, cx);
        self.attachment_drafts = attachment_drafts;
        self.attachments = attachments;
        self.runtime = runtime;
        self.runner = runner;
        self.connection = connection;
        self.catalog = catalog;
        self.settings = settings;
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
        // Session authority wins over the legacy compatibility projection.
        self.companion
            .session
            .as_ref()
            .and_then(|s| s.plan.as_ref())
            .map(|p| &p.task)
            .or_else(|| {
                self.dev
                    .tasks
                    .iter()
                    .find(|t| Some(&t.id) == self.dev.selected_task.as_ref())
            })
    }
    pub(super) fn effective_guidance(&self) -> EffectiveGuidance {
        self.companion.settings.effective(
            self.project.as_ref().map_or("", Project::key),
            &self.companion.language,
            self.active_task(),
        )
    }
    pub(super) fn invalidate_companion(&mut self, cx: &mut Context<Self>) {
        if self.companion.review.is_some() {
            self.companion.review_stale = true;
        }
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
        if let Some(task) = self.active_task() {
            paths.extend(task.steps.iter().flat_map(|s| s.files.clone()));
        }
        paths.extend(lines(&self.companion.context_paths.read(cx).value()));
        paths.into_iter().collect()
    }
    pub(super) fn preserve_composer(&mut self, cx: &App) -> Result<()> {
        let Some(session) = &self.companion.session else {
            return Ok(());
        };
        if let Some(fields) = &self.companion.documents {
            self.companion
                .document_drafts
                .preserve(&session.id, fields.snapshot(cx));
        }
        if let Some(fields) = &self.companion.attachment_fields {
            self.companion
                .attachment_drafts
                .insert(fields.scope.clone(), fields.snapshot(cx));
        }
        let text = self.companion.question.read(cx).value().to_string();
        if text != self.companion.composer.text {
            let sequence = self.store.lock().unwrap().save_composer_draft(
                &session.id,
                self.companion.composer.sequence,
                &text,
            )?;
            self.companion.composer = ComposerDraft { sequence, text };
        }
        Ok(())
    }
    pub(super) fn restore_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> Result<String> {
            let project = self.project.as_ref().context("Open a project")?;
            let mut store = self.store.lock().unwrap();
            self.companion.model_preference = store.model_preference()?;
            Ok(match store.selected_session(project.key())? {
                Some(id) => id,
                None => store.create_session(project.key())?.id,
            })
        })();
        match result {
            Ok(id) => self.select_conversation(&id, window, cx),
            Err(e) => self.companion.status = e.to_string(),
        }
    }
    pub(super) fn select_conversation(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = (|| -> Result<_> {
            self.preserve_composer(cx)?;
            let project = self.project.as_ref().context("Open a project")?;
            let mut store = self.store.lock().unwrap();
            store.select_session(project.key(), id)?;
            Ok((
                store
                    .load_session(id)?
                    .context("Conversation unavailable")?,
                store.composer_draft(id)?,
            ))
        })();
        match result {
            Ok((session, draft)) => {
                self.dev.selected_task = session
                    .plan
                    .as_ref()
                    .map(|p| p.task.id.clone())
                    .or_else(|| session.legacy_task.as_ref().map(|l| l.task_id.clone()));
                self.companion.events.clear();
                self.companion.document_events.clear();
                self.companion.legacy_history.clear();
                self.companion.session = Some(session);
                self.companion.review = None;
                self.companion.review_commands.clear();
                self.companion.review_epoch = None;
                self.companion.review_stale = true;
                self.companion.review_source_generation = None;
                self.companion.attachment_fields = None;
                self.companion.documents = None;
                self.companion.composer = draft.clone();
                self.companion
                    .question
                    .update(cx, |q, cx| q.set_value(draft.text, window, cx));
                self.companion.receipts.clear();
                self.companion
                    .custom_answer
                    .update(cx, |q, cx| q.set_value("", window, cx));
                self.companion.solution = None;
                self.companion.selected_run = None;
                self.companion.inspected_step = None;
                self.reload_conversation(true, window, cx);
                self.companion.status =
                    "Conversation restored. Approvals belong to the displayed saved versions."
                        .into();
                self.companion.review_refresh_pending = true;
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn reload_conversation(
        &mut self,
        documents: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = (|| -> Result<_> {
            let id = &self
                .companion
                .session
                .as_ref()
                .context("No conversation")?
                .id;
            let store = self.store.lock().unwrap();
            let session = store
                .load_session(id)?
                .context("Conversation unavailable")?;
            let events = store.conversation_events(id, None, 100)?;
            let document_events = store.document_editor_events(id)?;
            let legacy_history = companion_state::linked_legacy_history(&store, &session)?;
            let review = store.current_action_review(&session)?;
            let attachment = store.current_action_attachment(&session)?;
            let commands = review
                .as_ref()
                .map(|(r, _)| store.action_review_input(id, &r.id))
                .transpose()?
                .flatten()
                .map(|i| i.commands)
                .unwrap_or_default();
            let mut receipts = vec![];
            for event in &events {
                if let ConversationPayload::ResearchReceiptRecorded { receipt_id } = &event.payload
                    && let Some(receipt) = store.research_receipt(id, receipt_id)?
                {
                    receipts.push(receipt);
                }
            }
            Ok((
                session,
                review,
                attachment,
                commands,
                events,
                document_events,
                legacy_history,
                receipts,
                store.sessions(self.project.as_ref().context("Open a project")?.key())?,
            ))
        })();
        match result {
            Ok((
                session,
                review,
                attachment,
                commands,
                events,
                document_events,
                legacy_history,
                receipts,
                sessions,
            )) => {
                self.dev.selected_task = session
                    .plan
                    .as_ref()
                    .map(|p| p.task.id.clone())
                    .or_else(|| session.legacy_task.as_ref().map(|l| l.task_id.clone()));
                self.companion.session = Some(session);
                self.companion.review = review;
                self.companion.review_commands = commands.clone();
                if let Some(attachment) = attachment {
                    self.companion.attachments.insert(
                        serde_json::to_string(&attachment.scope).unwrap(),
                        attachment,
                    );
                }
                for command in commands {
                    if matches!(command.provenance, CommandProvenance::UserAttachedOutput) {
                        self.companion
                            .attachments
                            .entry(serde_json::to_string(&command.scope).unwrap())
                            .or_insert(command);
                    }
                }
                self.companion.document_events = document_events;
                self.companion.legacy_history = legacy_history;
                for event in events {
                    if !self.companion.events.iter().any(|e| e.id == event.id) {
                        self.companion.events.push(event);
                    }
                }
                self.companion.events.sort_by_key(|e| e.sequence);
                for receipt in receipts {
                    if !self.companion.receipts.iter().any(|r| r.id == receipt.id) {
                        self.companion.receipts.push(receipt);
                    }
                }
                self.companion.sessions = sessions;
                if documents {
                    self.load_document_fields(window, cx);
                }
                self.load_attachment_fields(window, cx);
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn older_conversation(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.companion.session else {
            return;
        };
        let result = self.store.lock().unwrap().conversation_events(
            &session.id,
            self.companion.events.first().map(|e| e.sequence),
            100,
        );
        match result {
            Ok(mut older) => {
                older.append(&mut self.companion.events);
                self.companion.events = older;
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn legacy_conversation(
        &mut self,
        id: &str,
        adopt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        if let Err(e) = self.preserve_composer(cx) {
            self.companion.status = e.to_string();
            return;
        }
        if adopt {
            let result = self.store.lock().unwrap().adopt_legacy_task(id);
            match result {
                Ok(session) => self.select_conversation(&session.id, window, cx),
                Err(e) => self.companion.status = e.to_string(),
            }
        } else {
            self.companion.session = None;
            self.dev.selected_task = Some(id.into());
            self.companion.events.clear();
            self.companion.receipts.clear();
            self.companion.selected_run = None;
            self.companion.solution = None;
            self.companion.documents = None;
            self.companion.tab = CompanionTab::Conversation;
            self.companion.status="Legacy history: continue in a conversation to save and approve a spec and dependent plan.".into();
        }
        cx.notify();
    }
    pub(super) fn legacy_run(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        if let Err(e) = self.preserve_composer(cx) {
            self.companion.status = e.to_string();
            return;
        }
        self.companion.session = None;
        self.dev.selected_task = None;
        self.companion.selected_run = Some(id.into());
        self.companion.events.clear();
        self.companion.receipts.clear();
        self.companion.documents = None;
        self.companion.solution = None;
        self.companion
            .question
            .update(cx, |q, cx| q.set_value("", window, cx));
        self.companion.status="Legacy conversation history. New implementation requires saved, approved documents and Start.".into();
        cx.notify();
    }
    pub(super) fn new_companion_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = (|| -> Result<_> {
            self.preserve_composer(cx)?;
            Ok(self
                .store
                .lock()
                .unwrap()
                .create_session(self.project.as_ref().context("Open a project")?.key())?
                .id)
        })();
        match result {
            Ok(id) => self.select_conversation(&id, window, cx),
            Err(e) => self.companion.status = e.to_string(),
        }
    }
    pub(super) fn companion_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = self.preserve_composer(cx) {
            self.companion.status = format!("Composer could not be preserved: {e}");
        }
        if let Some(mailbox) = self.companion.capture.clone() {
            let requested = mailbox.lock().unwrap().requested;
            if requested {
                let in_scope = {
                    let m = mailbox.lock().unwrap();
                    self.project.as_ref().is_some_and(|p| p.key() == m.project)
                        && self.companion.operation_id.as_ref() == Some(&m.operation_id)
                        && self
                            .companion
                            .session
                            .as_ref()
                            .is_some_and(|s| s.id == m.session_id)
                };
                let result = if in_scope {
                    self.working_snapshots(cx).map_err(|e| e.to_string())
                } else {
                    Err("Project or conversation changed during capture".into())
                };
                let mut m = mailbox.lock().unwrap();
                m.response = Some(result);
                m.code_epoch = self.companion.code_epoch;
                m.source_generation = self.source_generation.load(Ordering::Relaxed);
                m.requested = false;
            }
        }
        let progress = self.companion.progress.lock().unwrap().take();
        if let Some(progress) = progress
            && self.companion.operation_id.as_ref() == Some(&progress.id)
            && self
                .companion
                .session
                .as_ref()
                .is_some_and(|s| s.id == progress.session_id)
        {
            if let Some(op) = &mut self.companion.foreground {
                op.phase = if op.stopping {
                    OperationPhase::Stopping
                } else {
                    progress.phase
                };
                self.companion.status = companion_state::phase_label(op.phase).into();
            }
            self.reload_conversation(false, window, cx);
        }
        let fingerprint = self
            .companion
            .documents
            .as_ref()
            .map(|d| d.fingerprint(cx))
            .unwrap_or_default();
        if fingerprint != self.companion.document_fingerprint {
            self.companion.document_fingerprint = fingerprint;
            cx.notify();
        }
        if self.companion.busy || self.agent.busy || self.loading {
            return;
        }
        if self.companion.review_refresh_pending {
            // Project restore supplies its editor buffers and discovers local
            // tools before a saved review can be compared with current inputs.
            if self.dev.discovering {
                return;
            }
            self.companion.review_refresh_pending = false;
            self.refresh_saved_review(window, cx);
            if self.companion.busy {
                return;
            }
        }
        let Some(session) = &self.companion.session else {
            return;
        };
        if let Some(key) = companion_state::preview_key(
            session,
            &self.companion.language,
            &self.effective_guidance(),
        ) && self.companion.auto_requests.insert(key)
        {
            self.start_conversation(
                ConversationIntent::AutomaticPreview(SolutionPolicy::AutomaticPreview),
                window,
                cx,
            );
        }
    }
    pub(super) fn run_companion(
        &mut self,
        action: CompanionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = companion_state::command_binding(
            self.companion.session.as_ref(),
            action,
            self.companion.busy || self.agent.busy || self.loading,
        ) {
            self.companion.status = e.to_string();
            cx.notify();
            return;
        }
        let typed = self.companion.question.read(cx).value().trim().to_string();
        let intent = match action {
            CompanionAction::DraftSpec => ConversationIntent::DraftSpec,
            CompanionAction::DraftPlan => ConversationIntent::DraftPlan,
            CompanionAction::Guide | CompanionAction::PrepareWalkthrough => {
                ConversationIntent::PrepareWalkthrough
            }
            CompanionAction::Review | CompanionAction::ReviewAction => {
                ConversationIntent::ReviewAction
            }
            CompanionAction::Reveal => ConversationIntent::Reveal,
            CompanionAction::Delegate => return,
            CompanionAction::Hint => ConversationIntent::GatedDiscussion(format!(
                "Give one conceptual hint for the current action. Keep full implementations hidden. {typed}"
            )),
            CompanionAction::Explain => ConversationIntent::GatedDiscussion(format!(
                "Explain only the current action at the selected language and explanation level. Keep implementations hidden. {typed}"
            )),
            CompanionAction::ImproveClarity
            | CompanionAction::ImproveSpeed
            | CompanionAction::ImproveBalanced => ConversationIntent::Improve {
                objective: match action {
                    CompanionAction::ImproveClarity => ImprovementObjective::Clarity,
                    CompanionAction::ImproveSpeed => ImprovementObjective::Speed,
                    _ => ImprovementObjective::Balanced,
                },
                question: format!(
                    "Evaluate selected code; preserve behavior, state invariants, preconditions and tradeoffs. Keep current code when change is unjustified. Performance is unmeasured without actual benchmark receipts. Proposals only; keep implementations hidden. {typed}"
                ),
            },
            _ if !typed.is_empty() => ConversationIntent::Message(typed),
            CompanionAction::Ask => {
                self.companion.status = "Enter a message first.".into();
                cx.notify();
                return;
            }
            _ => ConversationIntent::Discuss,
        };
        self.start_conversation(intent, window, cx);
        if let Some(op) = &mut self.companion.foreground {
            op.action = action;
        }
    }
    pub(super) fn answer_companion(
        &mut self,
        question: &str,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy || self.loading {
            return;
        }
        let control = choice
            .as_ref()
            .map(|id| format!("answer:{id}"))
            .unwrap_or_else(|| "custom-answer".into());
        let custom = self
            .companion
            .custom_answer
            .read(cx)
            .value()
            .trim()
            .to_string();
        let answer = ClarificationAnswer {
            question_id: question.into(),
            choice_id: choice,
            custom_text: (!custom.is_empty()).then_some(custom),
        };
        let result = self
            .companion
            .session
            .as_ref()
            .and_then(|s| s.pending_question.as_ref())
            .context("This question has already been answered")
            .and_then(|q| answer.validate(q));
        if let Err(e) = result {
            self.companion.status = e.to_string();
            cx.notify();
            return;
        }
        self.start_conversation(ConversationIntent::Answer(answer), window, cx);
        if let Some(op) = &mut self.companion.foreground {
            op.control = Some(control);
        }
    }
    pub(super) fn start_conversation(
        &mut self,
        intent: ConversationIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy || self.loading {
            return;
        }
        let initial_action = match &intent {
            ConversationIntent::DraftSpec => CompanionAction::DraftSpec,
            ConversationIntent::DraftPlan => CompanionAction::DraftPlan,
            ConversationIntent::PrepareWalkthrough => CompanionAction::PrepareWalkthrough,
            ConversationIntent::ReviewAction => CompanionAction::ReviewAction,
            ConversationIntent::Reveal | ConversationIntent::AutomaticPreview(_) => {
                CompanionAction::Reveal
            }
            _ => CompanionAction::Discuss,
        };
        let id = if matches!(intent, ConversationIntent::AutomaticPreview(_)) {
            let Some(key) = self.companion.session.as_ref().and_then(|s| {
                companion_state::preview_key(
                    s,
                    &self.companion.language,
                    &self.effective_guidance(),
                )
            }) else {
                return;
            };
            format!("preview-{}", content_hash(key.as_bytes()))
        } else {
            new_id()
        };
        let session_id = self
            .companion
            .session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        self.companion.foreground = Some(companion_state::ForegroundOperation::new(
            id.clone(),
            session_id.clone(),
            initial_action,
        ));
        self.companion.operation_id = Some(id.clone());
        self.companion.busy = true;
        self.companion.status =
            companion_state::phase_label(OperationPhase::PreservingInput).into();
        if initial_action == CompanionAction::ReviewAction {
            self.companion.review_stale = true;
        }
        cx.notify();
        let result = (|| -> Result<_> {
            self.flush(cx)?;
            self.preserve_composer(cx)?;
            let project = self.project.clone().context("Open a project")?;
            let session = self
                .companion
                .session
                .as_ref()
                .context("Continue the legacy task in a conversation first")?;
            let session_id = session.id.clone();
            let input = ConversationInput {
                id: id.clone(),
                session_id: session_id.clone(),
                intent,
                language: self.companion.language.clone(),
                guidance: self.effective_guidance(),
                model_preference: self.companion.model_preference.clone(),
            };
            Ok((project, id, session_id, input))
        })();
        let (project, id, session_id, input) = match result {
            Ok(v) => v,
            Err(e) => {
                self.companion.busy = false;
                self.companion.foreground = None;
                self.companion.operation_id = None;
                self.companion.status = e.to_string();
                cx.notify();
                return;
            }
        };
        let submitted_answer = match &input.intent {
            ConversationIntent::Answer(answer) => {
                Some(answer.custom_text.clone().unwrap_or_default())
            }
            _ => None,
        };
        let requested_language = input.language.clone();
        let requested_guidance = input.guidance.fingerprint();
        let submitted = match &input.intent {
            ConversationIntent::Message(t) => Some(t.clone()),
            _ => None,
        };
        let mailbox = Arc::new(Mutex::new(CaptureMailbox {
            project: project.key().into(),
            session_id: session_id.clone(),
            operation_id: id.clone(),
            ..Default::default()
        }));
        // Bind research to the same selected tools and saved trust as this UI
        // operation. No discovery, tool activation or model executable authority.
        let mut check_environment = self.dev.environment.clone();
        if let Some(environment) = &mut check_environment {
            environment.tools = self.dev.rust.clone();
            environment.python = self.dev.python.clone();
        }
        let inspection =
            ProjectInspection::snapshot(check_environment.clone(), self.dev.settings.clone());
        let mut capture = FreshCapture {
            capture: ProjectConversationCapture {
                project: project.clone(),
                overlays: vec![],
                selected: self.selected_context_paths(cx),
                cache: companion_cache(),
                check_environment,
                recipe_ids: self.dev.recipes.iter().map(|r| r.id.clone()).collect(),
                attachments: self.current_attachments(),
            },
            mailbox: mailbox.clone(),
            store: self.store.clone(),
        };
        self.companion.capture = Some(mailbox.clone());
        self.companion.busy = true;
        self.companion.operation_id = Some(id.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.companion.cancel = Some(cancel.clone());
        let runner = self.companion.runner.clone();
        let runtime = self.companion.runtime.clone();
        let store = self.store.clone();
        let progress = self.companion.progress.clone();
        let epoch = self.epoch;
        let project_key = project.key().to_string();
        self.companion.status = "Preserving input…".into();
        let operation = cx.background_spawn(async move {
            let inspection_environment = inspection.environment()?;
            let mut runner = runner
                .lock()
                .map_err(|_| anyhow!("Conversation coordinator unavailable"))?;
            if runner.is_none() {
                *runner = Some(ConversationRunner::new(
                    store.clone(),
                    SharedBackend(runtime),
                    PublicRuntime::default(),
                    ResearchBroker::default(),
                ));
            }
            runner.as_mut().unwrap().run(
                input,
                &mut capture,
                inspection_environment.as_ref(),
                cancel,
                |op| {
                    *progress.lock().unwrap() = Some(op.clone());
                    Ok(())
                },
            )
        });
        let weak = cx.entity().downgrade();
        window.spawn(cx,async move |cx| {
            let result=operation.await;
            let _=weak.update_in(cx,|view,window,cx| {
                if view.epoch!=epoch || view.project.as_ref().is_none_or(|p|p.key()!=project_key) || view.companion.operation_id.as_ref()!=Some(&id) || view.companion.foreground.as_ref().is_none_or(|op|!op.owns(&id,&session_id)) || view.companion.session.as_ref().is_none_or(|s|s.id!=session_id) { return; }
                let captured_epoch=mailbox.lock().unwrap().code_epoch;
                let captured_source=mailbox.lock().unwrap().source_generation;
                view.companion.busy=false; view.companion.cancel=None; view.companion.capture=None; view.companion.operation_id=None; view.companion.foreground=None;
                let document_proposal=result.as_ref().is_ok_and(|r|matches!(r.outcome,OperationOutcome::Completed{..})&&!r.historical&&matches!(r.response,Some(CompanionResponse::SpecDraft(_)|CompanionResponse::PlanDraft(_))));
                view.reload_conversation(document_proposal,window,cx);
                if matches!(initial_action, CompanionAction::ReviewAction) || result.as_ref().is_ok_and(|r|r.review.is_some()) { view.companion.review_epoch=Some(captured_epoch); view.companion.review_source_generation=Some(captured_source); view.companion.review_stale=captured_epoch!=view.companion.code_epoch || captured_source!=view.source_generation.load(Ordering::Relaxed) || !result.as_ref().is_ok_and(|r|matches!(r.outcome,OperationOutcome::Completed{..})&&!r.historical&&r.review.is_some()); }
                match result {
                    Ok(result)=>{
                        let completed=matches!(result.outcome,OperationOutcome::Completed{..}) && !result.historical;
                        view.companion.status=match result.outcome {OperationOutcome::Completed{summary}=>summary,OperationOutcome::Failed{detail}|OperationOutcome::Cancelled{detail}|OperationOutcome::Interrupted{detail}=>detail};
                        if completed && view.companion.language==requested_language && view.effective_guidance().fingerprint()==requested_guidance && let Some(CompanionResponse::Solution(solution))=result.response {
                            view.companion.solution_history_session=None;
                            let editor=cx.new(|cx|EditorState::new(window,cx).language(&solution.language).default_value(solution.code));
                            view.companion.solution_scope=view.companion.session.as_ref().and_then(|s|s.authorize(WorkflowAction::Reveal).ok().zip(s.current_action().ok())).map(|(workflow,_)|companion_state::SolutionScope{workflow,action_scope:DelegationActionScope::current(view.companion.session.as_ref().unwrap()).unwrap(),language:requested_language.clone(),guidance:requested_guidance.clone()});
                            view.companion.solution=Some((solution.summary,editor));
                        }
                        if companion_state::clear_submitted_input(completed,submitted.as_deref(),&view.companion.question.read(cx).value()) {
                            view.companion.question.update(cx,|q,cx|q.set_value("",window,cx)); let _=view.preserve_composer(cx);
                        }
                        if companion_state::clear_submitted_input(completed,submitted_answer.as_deref(),&view.companion.custom_answer.read(cx).value()){view.companion.custom_answer.update(cx,|q,cx|q.set_value("",window,cx));}
                    }
                    Err(e)=>view.companion.status=format!("Conversation stopped: {e}. Saved input is retained; uncertain work was not replayed."),
                }
                let latest=view.store.lock().unwrap().model_runs(&project_key);
                if let Ok(runs)=latest { view.companion.resolved_model=runs.iter().find(|r|r.request.conversation_id.as_deref()==Some(&session_id)).and_then(|r|r.request.resolved_model.clone()); view.companion.runs=runs; }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
    pub(super) fn connect_companion(&mut self, login: bool, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        self.companion.busy = true;
        let id = format!("{}-{}", if login { "login" } else { "models" }, new_id());
        self.companion.operation_id = Some(id.clone());
        let session_id = self
            .companion
            .session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        self.companion.foreground = Some(companion_state::ForegroundOperation::new(
            id.clone(),
            session_id.clone(),
            CompanionAction::Discuss,
        ));
        self.companion.foreground.as_mut().unwrap().phase = OperationPhase::CheckingAccount;
        self.companion.status = "Checking account and available models…".into();
        let cancel = Arc::new(AtomicBool::new(false));
        self.companion.cancel = Some(cancel.clone());
        let runtime = SharedBackend(self.companion.runtime.clone());
        let epoch = self.epoch;
        let operation = cx.background_spawn(async move {
            runtime.with(|s| {
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Connection cancelled");
                let state = if login {
                    s.login(&cancel)?
                } else {
                    s.refresh()?
                };
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Connection cancelled");
                let catalog = s.list_models();
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Connection cancelled");
                Ok((state, catalog))
            })
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch
                    || view.companion.operation_id.as_ref() != Some(&id)
                    || view
                        .companion
                        .foreground
                        .as_ref()
                        .is_none_or(|op| !op.owns(&id, &session_id))
                {
                    return;
                }
                view.companion.busy = false;
                view.companion.operation_id = None;
                view.companion.cancel = None;
                view.companion.foreground = None;
                match result {
                    Ok((state, catalog)) => {
                        view.companion.status = state.detail.clone();
                        view.companion.connection = Some(state);
                        match catalog {
                            Ok(c) => {
                                view.companion.catalog = Some(c);
                                view.companion.model_error = None;
                            }
                            Err(e) => {
                                view.companion.catalog = None;
                                view.companion.model_error = Some(e.to_string());
                            }
                        }
                    }
                    Err(e) => {
                        view.companion.catalog = None;
                        view.companion.status = e.to_string();
                        view.companion.model_error = Some(e.to_string());
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
        self.companion.catalog = None;
        self.companion.status =
            "Disconnected. Editing and saved conversations remain available.".into();
        cx.notify();
    }
    pub(super) fn choose_model(&mut self, preference: ModelPreference, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = self
            .store
            .lock()
            .unwrap()
            .save_model_preference(&preference);
        match result {
            Ok(()) => self.companion.model_preference = preference,
            Err(e) => self.companion.model_error = Some(e.to_string()),
        }
        cx.notify();
    }
    pub(super) fn refresh_companion_context(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.companion.context_busy {
            return;
        }
        let Some(project) = self.project.clone() else {
            return;
        };
        let working = match self.working_snapshots(cx) {
            Ok(w) => w,
            Err(e) => {
                self.companion.status = e.to_string();
                return;
            }
        };
        let paths = self.selected_context_paths(cx);
        let epoch = self.epoch;
        let code_epoch = self.companion.code_epoch;
        self.companion.context_job += 1;
        let job = self.companion.context_job;
        self.companion.context_busy = true;
        let operation = cx.background_spawn(async move {
            capture_context(
                &project,
                &working,
                &paths,
                &companion_cache(),
                &AtomicBool::new(false),
            )
        });
        cx.spawn(async move |view, cx| {
            let result = operation.await;
            let _ = view.update(cx, |view, cx| {
                if view.epoch != epoch || view.companion.context_job != job {
                    return;
                }
                view.companion.context_busy = false;
                if view.companion.code_epoch == code_epoch {
                    match result {
                        Ok(c) => {
                            view.companion.context = Some(c);
                            view.companion.context_epoch = code_epoch;
                        }
                        Err(e) => view.companion.status = e.to_string(),
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
}
pub(super) fn companion_cache() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Caches/Tandem/companion-context")
}
pub(super) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
