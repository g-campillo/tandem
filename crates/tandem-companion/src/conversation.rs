//! Durable conversation orchestration. Model output is data; only explicit
//! human Store commands save/approve documents, Start, or consume a review.
use crate::*;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};
use tandem_checks::{InspectionEnvironment, InspectionExecutor, RuntimeInspectionExecutor};
use tandem_codex::ProtocolExchange;
use tandem_core::*;

pub trait ConversationBackend {
    fn check_account(&mut self, cancel: &AtomicBool) -> Result<()>;
    fn resolve_model(
        &mut self,
        preference: &ModelPreference,
        cancel: &AtomicBool,
    ) -> Result<ResolvedModel>;
    fn turn(
        &mut self,
        run: ModelRun,
        thread: Option<ThreadBinding>,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&ModelRun) -> Result<()>,
    ) -> Result<(ModelRun, Option<ThreadBinding>)>;
}
impl<E: ProtocolExchange> ConversationBackend for CompanionSession<E> {
    fn check_account(&mut self, cancel: &AtomicBool) -> Result<()> {
        check_cancel(cancel)?;
        ensure!(
            self.refresh()?.kind == ConnectionKind::Ready,
            "{}",
            self.connection.detail
        );
        Ok(())
    }
    fn resolve_model(
        &mut self,
        preference: &ModelPreference,
        cancel: &AtomicBool,
    ) -> Result<ResolvedModel> {
        check_cancel(cancel)?;
        CompanionSession::resolve_model(self, preference)
    }
    fn turn(
        &mut self,
        run: ModelRun,
        thread: Option<ThreadBinding>,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&ModelRun) -> Result<()>,
    ) -> Result<(ModelRun, Option<ThreadBinding>)> {
        self.run(run, thread, cancel, observe)
    }
}

/// This boundary accepts only screened public data and the frozen model.
/// Private requests, sessions and captured files cannot enter it.
pub trait PublicWorker {
    fn execute(
        &mut self,
        brief: &ScreenedPublicBrief,
        model: &ResolvedModel,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&PublicResearchProgress) -> Result<()>,
    ) -> Result<ResearchReceipt>;
}
pub struct PublicDataWorker<E: ProtocolExchange, H: PublicHttpTransport> {
    pub search: PublicResearchSession<E>,
    pub sources: PublicSourceFetcher<H>,
}
impl<E: ProtocolExchange, H: PublicHttpTransport> PublicWorker for PublicDataWorker<E, H> {
    fn execute(
        &mut self,
        brief: &ScreenedPublicBrief,
        model: &ResolvedModel,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&PublicResearchProgress) -> Result<()>,
    ) -> Result<ResearchReceipt> {
        check_cancel(cancel)?;
        if let Some(url) = brief.brief().urls.first() {
            self.sources.fetch(
                ResearchRequest::new(ResearchRequestKind::PublicPage { url: url.clone() })?,
                cancel,
            )
        } else {
            Ok(self.search.search(brief, model, cancel, observe)?.receipt)
        }
    }
}

pub struct CapturedConversation {
    pub context: ContextSnapshot,
    pub research: ResearchSnapshot,
    pub review: Option<ActionReviewInput>,
    /// Local-only privacy screening input; never passed to the public worker.
    pub private_identifiers: Vec<String>,
}
pub trait ConversationCapture {
    fn capture(
        &mut self,
        session: &TaskSession,
        action: CompanionAction,
        language: &str,
        guidance: &EffectiveGuidance,
        checks: &[CheckRecord],
        cancel: &AtomicBool,
    ) -> Result<CapturedConversation>;
}
pub struct ProjectConversationCapture {
    pub project: Project,
    pub overlays: Vec<Document>,
    pub selected: Vec<String>,
    pub cache: PathBuf,
    pub check_environment: Option<tandem_checks::CheckEnvironment>,
    pub recipe_ids: Vec<String>,
    /// Only deliberately attached output, already scoped by the evidence adapter.
    pub attachments: Vec<ActionCommandEvidence>,
}
impl ConversationCapture for ProjectConversationCapture {
    fn capture(
        &mut self,
        session: &TaskSession,
        action: CompanionAction,
        language: &str,
        guidance: &EffectiveGuidance,
        checks: &[CheckRecord],
        cancel: &AtomicBool,
    ) -> Result<CapturedConversation> {
        ensure!(
            session.project == self.project.key(),
            "Capture belongs to another project"
        );
        let (mut context, review) = if matches!(
            action,
            CompanionAction::ReviewAction | CompanionAction::Reveal
        ) {
            let mut capture = capture_action_context(
                session,
                &self.project,
                &self.overlays,
                &self.selected,
                &self.cache,
                cancel,
            )?;
            let recipes = bind_validation_environment(
                &mut capture.context,
                self.check_environment.as_ref(),
                &self.recipe_ids,
            );
            let empty = ActionReviewInput::capture(
                session,
                capture.context.clone(),
                capture.targets.clone(),
                vec![],
                language,
                guidance,
            )?;
            let mut commands =
                collect_action_check_evidence(empty.scope(), &capture.context, checks, &recipes);
            commands.extend(self.attachments.clone());
            let review = ActionReviewInput::capture(
                session,
                capture.context.clone(),
                capture.targets,
                commands,
                language,
                guidance,
            )?;
            (capture.context, Some(review))
        } else {
            (
                capture_context(
                    &self.project,
                    &self.overlays,
                    &self.selected,
                    &self.cache,
                    cancel,
                )?,
                None,
            )
        };
        if review.is_none() {
            bind_validation_environment(
                &mut context,
                self.check_environment.as_ref(),
                &self.recipe_ids,
            );
        }
        let research =
            ResearchSnapshot::capture(&self.project, &self.overlays, &self.cache, cancel)?;
        let mut private_identifiers = vec![
            session.id.clone(),
            session.project.clone(),
            self.project.root.to_string_lossy().into_owned(),
        ];
        if let Some(name) = self.project.root.file_name() {
            private_identifiers.push(name.to_string_lossy().into_owned());
        }
        // Capture symbols/path components locally; conservatively withhold public
        // queries containing private identifiers or copied non-generic tokens.
        for file in &context.files {
            private_identifiers.push(file.path.clone());
            for word in file.text.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if (word.contains('_') || word.chars().any(|c| c.is_ascii_uppercase()))
                    && word.len() >= 4
                {
                    private_identifiers.push(word.to_string());
                }
            }
        }
        Ok(CapturedConversation {
            context,
            research,
            review,
            private_identifiers,
        })
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub enum ConversationIntent {
    Message(String),
    Answer(ClarificationAnswer),
    Discuss,
    DraftSpec,
    DraftPlan,
    PrepareWalkthrough,
    ReviewAction,
    Reveal,
    /// The UI supplies its already selected explicit preference. Central gates
    /// and action/language binding still apply here.
    AutomaticPreview(SolutionPolicy),
}
impl ConversationIntent {
    fn action(&self) -> CompanionAction {
        match self {
            Self::Message(_) | Self::Answer(_) | Self::Discuss => CompanionAction::Discuss,
            Self::DraftSpec => CompanionAction::DraftSpec,
            Self::DraftPlan => CompanionAction::DraftPlan,
            Self::PrepareWalkthrough => CompanionAction::PrepareWalkthrough,
            Self::ReviewAction => CompanionAction::ReviewAction,
            Self::Reveal | Self::AutomaticPreview(_) => CompanionAction::Reveal,
        }
    }
    fn kind(&self) -> SessionOperationKind {
        match self.action() {
            CompanionAction::DraftSpec => SessionOperationKind::DraftSpec,
            CompanionAction::DraftPlan => SessionOperationKind::DraftPlan,
            CompanionAction::PrepareWalkthrough => SessionOperationKind::PrepareAction,
            CompanionAction::ReviewAction => SessionOperationKind::ReviewAction,
            CompanionAction::Reveal => SessionOperationKind::Reveal,
            _ => SessionOperationKind::Discuss,
        }
    }
    fn question(&self) -> String {
        match self {
            Self::Message(text) => text.clone(),
            Self::Answer(_) => {
                "Continue using the explicit answer recorded in the authoritative transcript."
                    .into()
            }
            _ => format!("Explicit {:?} request", self.action()),
        }
    }
}
pub struct ConversationInput {
    /// Stable caller delivery identity. Re-delivery never dispatches a second turn.
    pub id: String,
    pub session_id: String,
    pub intent: ConversationIntent,
    pub language: String,
    pub guidance: EffectiveGuidance,
    pub model_preference: ModelPreference,
}
#[derive(Clone, Debug)]
pub struct ConversationResult {
    pub operation_id: String,
    pub outcome: OperationOutcome,
    pub response: Option<CompanionResponse>,
    pub duplicate: bool,
    pub historical: bool,
    pub review: Option<ActionReview>,
    pub walkthrough: Option<Walkthrough>,
}
pub struct ConversationRunner<B, P, I = RuntimeInspectionExecutor> {
    pub store: Arc<Mutex<Store>>,
    pub backend: B,
    pub public: P,
    pub broker: ResearchBroker<I>,
    pub operations: OperationSlot,
    // A process restart never resumes a possibly compacted/uncertain context.
    // Persisted transport bindings are independent from transcript history.
    threads: BTreeMap<String, ThreadBinding>,
}
impl<B: ConversationBackend, P: PublicWorker, I: InspectionExecutor> ConversationRunner<B, P, I> {
    pub fn new(store: Arc<Mutex<Store>>, backend: B, public: P, broker: ResearchBroker<I>) -> Self {
        Self {
            store,
            backend,
            public,
            broker,
            operations: OperationSlot::default(),
            threads: BTreeMap::new(),
        }
    }
    pub fn run(
        &mut self,
        input: ConversationInput,
        capture: &mut impl ConversationCapture,
        environment: Option<&InspectionEnvironment>,
        cancel: Arc<AtomicBool>,
        mut observe: impl FnMut(&ActiveOperation) -> Result<()>,
    ) -> Result<ConversationResult> {
        let id = input.id.clone();
        let session_id = input.session_id.clone();
        let input_hash = content_hash(&serde_json::to_vec(&(
            &input.session_id,
            &input.intent,
            &input.language,
            &input.guidance,
            &input.model_preference,
        ))?);
        let previous_input = lock_store(&self.store)?.operation_input_hash(&session_id, &id)?;
        if let Some(hash) = previous_input {
            ensure!(hash == input_hash, "Operation delivery ID is immutable");
            if let Some(outcome) = lock_store(&self.store)?.session_operation_outcome(&id)? {
                return Ok(ConversationResult {
                    operation_id: id,
                    outcome,
                    response: None,
                    duplicate: true,
                    historical: true,
                    review: None,
                    walkthrough: None,
                });
            }
            bail!("Operation is already active or has an unknown outcome; it was not replayed");
        }
        ensure!(
            lock_store(&self.store)?
                .session_operation_outcome(&id)?
                .is_none(),
            "Operation ID belongs to another session"
        );
        let mut historical = false;
        let mut result_review = None;
        let mut result_walkthrough = None;
        let operation = ActiveOperation {
            id: id.clone(),
            session_id: session_id.clone(),
            kind: input.intent.kind(),
            phase: OperationPhase::PreservingInput,
            started_at_ms: now_ms(),
            cancel_requested: false,
        };
        self.operations.begin(operation.clone(), cancel.clone())?;
        let result = (|| -> Result<Option<CompanionResponse>> {
            let mut commands = vec![
                SessionCommand::RecordOperationInput {
                    operation_id: id.clone(),
                    input_hash: input_hash.clone(),
                },
                SessionCommand::BeginOperation {
                    operation: SessionOperation {
                        id: id.clone(),
                        session_id: session_id.clone(),
                        kind: operation.kind,
                        started_at_ms: operation.started_at_ms,
                    },
                },
            ];
            match &input.intent {
                ConversationIntent::Message(text) => {
                    ensure!(!credential_like(text), "Credential-like input was withheld");
                    commands.insert(0, SessionCommand::HumanMessage { text: text.clone() });
                }
                ConversationIntent::Answer(answer) => {
                    ensure!(
                        !credential_like_data(&serde_json::to_value(answer)?),
                        "Credential-like answer was withheld"
                    );
                    commands.insert(
                        0,
                        SessionCommand::AnswerQuestion {
                            answer: answer.clone(),
                        },
                    );
                }
                _ => {}
            }
            apply(
                &self.store,
                &session_id,
                SessionTransition::new(format!("{id}-input"), commands),
            )?;
            phase(
                &self.store,
                &self.operations,
                &session_id,
                &id,
                OperationPhase::PreservingInput,
                &cancel,
                &mut observe,
            )?;
            let session = load(&self.store, &session_id)?;
            let action = input.intent.action();
            let workflow_action = workflow_action(action);
            let workflow = if let Some(a) = workflow_action {
                Some(session.authorize(a)?)
            } else {
                None
            };
            if action == CompanionAction::DraftPlan {
                ensure!(session.spec.is_some(), "Save a spec before drafting a plan");
            }
            if let ConversationIntent::AutomaticPreview(preference) = input.intent {
                ensure!(
                    preference == SolutionPolicy::AutomaticPreview
                        && input.guidance.solution == SolutionPolicy::AutomaticPreview,
                    "Automatic preview needs the explicit saved preference"
                );
            }
            phase(
                &self.store,
                &self.operations,
                &session_id,
                &id,
                OperationPhase::CapturingContext,
                &cancel,
                &mut observe,
            )?;
            let checks = lock_store(&self.store)?.checks(&session.project)?;
            let captured = capture.capture(
                &session,
                action,
                &input.language,
                &input.guidance,
                &checks,
                &cancel,
            )?;
            check_cancel(&cancel)?;
            phase(
                &self.store,
                &self.operations,
                &session_id,
                &id,
                OperationPhase::CheckingAccount,
                &cancel,
                &mut observe,
            )?;
            self.backend.check_account(&cancel)?;
            phase(
                &self.store,
                &self.operations,
                &session_id,
                &id,
                OperationPhase::CheckingModels,
                &cancel,
                &mut observe,
            )?;
            let model = self
                .backend
                .resolve_model(&input.model_preference, &cancel)?;
            model.validate()?;
            check_cancel(&cancel)?;
            let events = all_events(&self.store, &session_id)?;
            let transcript = build_transcript(&events)?;
            if let Some(checkpoint) = &transcript.checkpoint {
                apply(
                    &self.store,
                    &session_id,
                    SessionTransition::new(
                        new_id(),
                        vec![SessionCommand::SummaryCheckpoint {
                            checkpoint: checkpoint.clone(),
                        }],
                    )
                    .for_operation(&id),
                )?;
            }
            apply(
                &self.store,
                &session_id,
                SessionTransition::new(
                    new_id(),
                    vec![SessionCommand::OperationSnapshot {
                        snapshot: ConversationSnapshot {
                            operation_id: id.clone(),
                            context_id: captured.context.id.clone(),
                            project_fingerprint: captured.context.project_fingerprint.clone(),
                            research_snapshot_id: captured.research.id().into(),
                            model: model.clone(),
                            guidance_fingerprint: input.guidance.fingerprint(),
                            workflow: workflow.clone(),
                            spec_ref: session.spec.as_ref().map(SpecDocument::document_ref),
                            plan_ref: session.plan.as_ref().map(PlanDocument::document_ref),
                        },
                    }],
                )
                .for_operation(&id),
            )?;
            let mut prompt_session = session.clone();
            prompt_session.history.clear();
            prompt_session.answered_questions.clear();
            if let Ok((walk, _)) = session.current_action() {
                prompt_session.walkthroughs.retain(|w| w.id == walk.id);
            } else {
                prompt_session.walkthroughs.clear();
            }
            let mut guided = GuidedContext {
                operation_id: id.clone(),
                session: prompt_session,
                workflow,
                transcript,
                receipts: historical_receipts(&self.store, &session_id, &events)?,
                review: captured.review,
            };
            let mut research =
                ResearchContext::new(&captured.research, &session_id, &id, environment, &events)?;
            let mut budget = ResearchBudget::default();
            let key = format!(
                "guided:{}:{}:{}",
                session_id,
                if action == CompanionAction::Reveal {
                    "solution"
                } else {
                    "main"
                },
                content_hash(&serde_json::to_vec(&model)?)
            );
            loop {
                if budget.rounds >= MAX_RESEARCH_ROUNDS {
                    record_stop(&self.store, &session_id, &id)?;
                    return Ok(None);
                }
                phase(
                    &self.store,
                    &self.operations,
                    &session_id,
                    &id,
                    model_phase(action),
                    &cancel,
                    &mut observe,
                )?;
                let request = ModelRequest {
                    model_preference: input.model_preference.clone(),
                    resolved_model: Some(model.clone()),
                    conversation_id: Some(session_id.clone()),
                    guided: Some(guided.clone()),
                    action,
                    question: input.intent.question(),
                    language: input.language.clone(),
                    guidance: input.guidance.clone(),
                    task: session.plan.as_ref().map(|p| p.task.clone()),
                    context: captured.context.clone(),
                    evidence: vec![],
                    prior_decisions: vec![],
                    delegation: None,
                };
                request.validate_guided()?;
                let original_request = request.clone();
                let run = ModelRun::pending(session.project.clone(), request);
                lock_store(&self.store)?.save_model_run(&run)?;
                let store = self.store.clone();
                let (mut run, thread) = self.backend.turn(
                    run,
                    self.threads.get(&key).cloned(),
                    &cancel,
                    &mut |run| lock_store(&store)?.save_model_run(run),
                )?;
                lock_store(&self.store)?.save_model_run(&run)?;
                check_cancel(&cancel)?;
                ensure!(run.status == ModelStatus::Completed, "{}", run.detail);
                // Revalidate fake/adapted backends through the same parser as production.
                let response = run
                    .response
                    .take()
                    .context("No structured response was returned")?;
                let response = match original_request.parse_response(response_value(&response)?) {
                    Ok(response) => response,
                    Err(error) => {
                        run.finish(
                            ModelStatus::Quarantined,
                            "Response quarantined by its content/evidence contract",
                        );
                        lock_store(&self.store)?.save_model_run(&run)?;
                        return Err(error);
                    }
                };
                let reply = response_reply(&response);
                if let Some(reply) = reply {
                    reply.validate()?;
                    for finding in &reply.findings {
                        finding.validate(&guided.receipts)?;
                    }
                    // A pending question may remain during ordinary chat, but a
                    // model cannot replace it or answer it implicitly.
                    if let Some(question) = &reply.question {
                        let mut state = load(&self.store, &session_id)?;
                        state.ask_question(question.clone(), Some(run.id.clone()))?;
                    }
                    if !reply.research_requests.is_empty() {
                        ensure!(
                            reply.question.is_none(),
                            "Resolve research before asking a clarification"
                        );
                        if budget
                            .begin_round(reply.research_requests.len() as u8)
                            .is_err()
                            || budget.returned_bytes >= MAX_RESEARCH_RETURNED_BYTES
                        {
                            record_stop(&self.store, &session_id, &id)?;
                            return Ok(None);
                        }
                        research.begin_round(reply.research_requests.len() as u8)?;
                        for kind in &reply.research_requests {
                            check_cancel(&cancel)?;
                            let request = ResearchRequest::new(kind.clone())?;
                            let receipt = if matches!(
                                kind,
                                ResearchRequestKind::PublicSearch { .. }
                                    | ResearchRequestKind::PublicPage { .. }
                            ) {
                                let brief = PublicResearchBrief {
                                    id: new_id(),
                                    topics: vec![],
                                    queries: match kind {
                                        ResearchRequestKind::PublicSearch { query } => {
                                            vec![query.clone()]
                                        }
                                        _ => vec![],
                                    },
                                    urls: match kind {
                                        ResearchRequestKind::PublicPage { url } => {
                                            vec![url.clone()]
                                        }
                                        _ => vec![],
                                    },
                                };
                                let screened =
                                    screen_public_brief(brief, &captured.private_identifiers)?;
                                if let Some(brief) = screened.brief {
                                    phase(
                                        &self.store,
                                        &self.operations,
                                        &session_id,
                                        &id,
                                        if matches!(kind, ResearchRequestKind::PublicPage { .. }) {
                                            OperationPhase::PublicRetrieval
                                        } else {
                                            OperationPhase::PublicSearch
                                        },
                                        &cancel,
                                        &mut observe,
                                    )?;
                                    activity(
                                        &self.store,
                                        &session_id,
                                        &id,
                                        ResearchActivity::PublicBriefPrepared {
                                            brief: brief.brief().clone(),
                                            model: model.clone(),
                                        },
                                    )?;
                                    let store = self.store.clone();
                                    let mut receipt = self.public.execute(
                                        &brief,
                                        &model,
                                        &cancel,
                                        &mut |progress| match progress {
                                            PublicResearchProgress::BriefPrepared {
                                                brief,
                                                model,
                                            } => activity(
                                                &store,
                                                &session_id,
                                                &id,
                                                ResearchActivity::PublicBriefPrepared {
                                                    brief: brief.clone(),
                                                    model: model.clone(),
                                                },
                                            ),
                                            PublicResearchProgress::SearchObserved {
                                                observation,
                                            } => activity(
                                                &store,
                                                &session_id,
                                                &id,
                                                ResearchActivity::PublicSearchObserved {
                                                    observation: observation.clone(),
                                                },
                                            ),
                                            PublicResearchProgress::Completed { .. } => Ok(()),
                                        },
                                    )?;
                                    receipt.request = request;
                                    receipt
                                } else {
                                    ResearchReceipt::new(
                                        request,
                                        vec![],
                                        ResearchReceiptStatus::Denied,
                                        String::new(),
                                        screened
                                            .omissions
                                            .into_iter()
                                            .map(|reason| ResearchOmission {
                                                locator: None,
                                                reason,
                                            })
                                            .collect(),
                                    )?
                                }
                            } else {
                                phase(
                                    &self.store,
                                    &self.operations,
                                    &session_id,
                                    &id,
                                    research_phase(kind),
                                    &cancel,
                                    &mut observe,
                                )?;
                                activity(
                                    &self.store,
                                    &session_id,
                                    &id,
                                    ResearchActivity::Requested {
                                        request: request.clone(),
                                    },
                                )?;
                                self.broker
                                    .execute(request, &mut research, &cancel, |_| Ok(()))?
                            };
                            check_cancel(&cancel)?;
                            if budget.record_receipt(&receipt).is_err() {
                                record_stop(&self.store, &session_id, &id)?;
                                return Ok(None);
                            }
                            apply(
                                &self.store,
                                &session_id,
                                SessionTransition::new(
                                    new_id(),
                                    vec![
                                        SessionCommand::RecordResearchReceipt {
                                            receipt: receipt.clone(),
                                        },
                                        SessionCommand::ResearchActivity {
                                            activity: ResearchActivity::Received {
                                                receipt_id: receipt.id.clone(),
                                                status: receipt.status,
                                            },
                                        },
                                    ],
                                )
                                .for_operation(&id),
                            )?;
                            guided.receipts.push(receipt);
                        }
                        // Only confirmed complete subturns are reusable. No
                        // model/private context is ever reused by the public worker.
                        if let Some(thread) = thread {
                            self.threads.insert(key.clone(), thread.clone());
                            lock_store(&self.store)?.save_companion_thread(
                                &session.project,
                                &key,
                                &thread,
                            )?;
                        }
                        continue;
                    }
                }
                if matches!(
                    action,
                    CompanionAction::ReviewAction | CompanionAction::Reveal
                ) {
                    phase(
                        &self.store,
                        &self.operations,
                        &session_id,
                        &id,
                        OperationPhase::CapturingContext,
                        &cancel,
                        &mut observe,
                    )?;
                    let latest = load(&self.store, &session_id)?;
                    let checks = lock_store(&self.store)?.checks(&session.project)?;
                    let live = capture.capture(
                        &latest,
                        action,
                        &input.language,
                        &input.guidance,
                        &checks,
                        &cancel,
                    )?;
                    historical = live.review.as_ref().map(|r| &r.binding)
                        != guided.review.as_ref().map(|r| &r.binding);
                    if historical {
                        apply(&self.store,&session_id,SessionTransition::new(new_id(),vec![SessionCommand::OperationResultHistorical{operation_id:id.clone(),detail:"Source, policy or evidence changed during this operation. The result applies only to its captured inputs.".into()}]).for_operation(&id))?;
                    }
                }
                let commands =
                    final_commands(&session, &run.id, &response, guided.review.as_ref())?;
                for command in &commands {
                    match command {
                        SessionCommand::RecordActionReview { review, .. } => {
                            result_review = Some(review.clone())
                        }
                        SessionCommand::SaveWalkthrough { walkthrough } => {
                            result_walkthrough = Some(walkthrough.clone())
                        }
                        _ => {}
                    }
                }
                let mut transition = SessionTransition::new(new_id(), commands).for_operation(&id);
                if let Some(a) = workflow_action {
                    transition = transition.with_authorization(
                        a,
                        guided
                            .workflow
                            .clone()
                            .context("Missing workflow binding")?,
                    );
                }
                // Drafts are bound to the exact definitions at the operation
                // boundary; unrelated chat does not invalidate an action review.
                let current = load(&self.store, &session_id)?;
                if matches!(
                    action,
                    CompanionAction::DraftSpec | CompanionAction::DraftPlan
                ) {
                    transition = transition.with_definitions(&session);
                    ensure!(
                        current.spec.as_ref().map(SpecDocument::document_ref)
                            == session.spec.as_ref().map(SpecDocument::document_ref)
                            && current.plan.as_ref().map(PlanDocument::document_ref)
                                == session.plan.as_ref().map(PlanDocument::document_ref),
                        "Document draft is historical because the definition changed"
                    );
                }
                apply(&self.store, &session_id, transition)?;
                if let Some(review) = &result_review {
                    result_review =
                        lock_store(&self.store)?.action_review(&session_id, &review.id)?;
                }
                if let Some(thread) = thread {
                    lock_store(&self.store)?.save_companion_thread(
                        &session.project,
                        &key,
                        &thread,
                    )?;
                    self.threads.insert(key.clone(), thread);
                }
                return Ok(Some(response));
            }
        })();
        let outcome = match &result {
            Ok(_) => OperationOutcome::Completed {
                summary: "Operation completed; proposals require explicit human save and approval."
                    .into(),
            },
            Err(_) if cancel.load(Ordering::Acquire) => OperationOutcome::Cancelled {
                detail: "Cancelled; no further model or research work was dispatched.".into(),
            },
            Err(error) => OperationOutcome::Failed {
                detail: safe_error(error),
            },
        };
        // Persist the terminal fence before releasing busy state. If persistence
        // itself failed, restart reconciliation will record Interrupted. Never
        // leave the UI stuck or let a cleanup error release another operation.
        let terminal = (|| -> Result<()> {
            if lock_store(&self.store)?
                .active_session_operation(&session_id)?
                .is_some_and(|o| o.id == id)
            {
                if cancel.load(Ordering::Acquire) {
                    let _ = phase(
                        &self.store,
                        &self.operations,
                        &session_id,
                        &id,
                        OperationPhase::Stopping,
                        &cancel,
                        &mut observe,
                    );
                }
                apply(
                    &self.store,
                    &session_id,
                    SessionTransition::new(
                        format!("{id}-terminal"),
                        vec![SessionCommand::FinishOperation {
                            operation_id: id.clone(),
                            outcome: outcome.clone(),
                        }],
                    )
                    .for_operation(&id),
                )?;
            }
            Ok(())
        })();
        self.operations.finish(&id);
        if result.is_err() || terminal.is_err() {
            self.threads
                .retain(|key, _| !key.starts_with(&format!("guided:{session_id}:")));
        }
        terminal?;
        if result.is_err() {
            result_review = None;
            result_walkthrough = None;
        }
        let cancelled = cancel.load(Ordering::Acquire);
        Ok(ConversationResult {
            operation_id: id,
            outcome,
            response: if cancelled {
                None
            } else {
                result.ok().flatten()
            },
            duplicate: false,
            historical: historical || cancelled,
            review: result_review,
            walkthrough: result_walkthrough,
        })
    }
}

fn lock_store(store: &Mutex<Store>) -> Result<MutexGuard<'_, Store>> {
    store
        .lock()
        .map_err(|_| anyhow::anyhow!("Conversation store is unavailable"))
}
fn load(store: &Mutex<Store>, session: &str) -> Result<TaskSession> {
    lock_store(store)?
        .load_session(session)?
        .context("Unknown session")
}
fn apply(
    store: &Mutex<Store>,
    session: &str,
    transition: SessionTransition,
) -> Result<TaskSession> {
    let mut store = lock_store(store)?;
    let sequence = store
        .load_session(session)?
        .context("Unknown session")?
        .sequence;
    store.apply_session_transition(session, sequence, &transition)
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Operation cancelled");
    Ok(())
}
fn safe_error(error: &anyhow::Error) -> String {
    let text = error.to_string();
    if credential_like(&text) {
        "Operation failed; sensitive error details withheld".into()
    } else {
        text.chars().take(2048).collect()
    }
}
fn activity(
    store: &Mutex<Store>,
    session: &str,
    id: &str,
    activity: ResearchActivity,
) -> Result<()> {
    apply(
        store,
        session,
        SessionTransition::new(
            new_id(),
            vec![SessionCommand::ResearchActivity { activity }],
        )
        .for_operation(id),
    )?;
    Ok(())
}
fn phase(
    store: &Mutex<Store>,
    slot: &OperationSlot,
    session: &str,
    id: &str,
    phase: OperationPhase,
    cancel: &AtomicBool,
    observe: &mut impl FnMut(&ActiveOperation) -> Result<()>,
) -> Result<()> {
    let op = slot.phase(id, phase)?;
    apply(
        store,
        session,
        SessionTransition::new(
            new_id(),
            vec![SessionCommand::OperationPhase {
                operation_id: id.into(),
                phase: op.phase,
            }],
        )
        .for_operation(id),
    )?;
    observe(&op)?;
    if phase != OperationPhase::Stopping {
        check_cancel(cancel)?;
    }
    Ok(())
}
fn workflow_action(action: CompanionAction) -> Option<WorkflowAction> {
    match action {
        CompanionAction::PrepareWalkthrough => Some(WorkflowAction::PrepareWalkthrough),
        CompanionAction::ReviewAction => Some(WorkflowAction::ReviewAction),
        CompanionAction::Reveal => Some(WorkflowAction::Reveal),
        _ => None,
    }
}
fn model_phase(action: CompanionAction) -> OperationPhase {
    match action {
        CompanionAction::DraftSpec => OperationPhase::DraftingSpec,
        CompanionAction::DraftPlan => OperationPhase::DraftingPlan,
        CompanionAction::PrepareWalkthrough => OperationPhase::PreparingAction,
        CompanionAction::ReviewAction => OperationPhase::Reviewing,
        CompanionAction::Reveal => OperationPhase::Revealing,
        _ => OperationPhase::Discussing,
    }
}
fn research_phase(kind: &ResearchRequestKind) -> OperationPhase {
    match kind {
        ResearchRequestKind::SearchText { .. } => OperationPhase::SearchingSource,
        ResearchRequestKind::GitHistory { .. }
        | ResearchRequestKind::GitShow { .. }
        | ResearchRequestKind::GitDiff { .. }
        | ResearchRequestKind::Inspect { .. } => OperationPhase::InspectingSource,
        _ => OperationPhase::ReadingSource,
    }
}
fn record_stop(store: &Mutex<Store>, session: &str, id: &str) -> Result<()> {
    activity(
        store,
        session,
        id,
        ResearchActivity::Stopped {
            reason: ResearchStopReason::BudgetExhausted,
            detail:
                "Research budget exhausted. Continue explicitly to start another bounded operation."
                    .into(),
            continue_available: true,
        },
    )
}
fn response_reply(response: &CompanionResponse) -> Option<&ConversationReply> {
    match response {
        CompanionResponse::Discussion(r) => Some(r),
        CompanionResponse::SpecDraft(r) => Some(&r.reply),
        CompanionResponse::PlanDraft(r) => Some(&r.reply),
        CompanionResponse::WalkthroughDraft(r) => Some(&r.reply),
        CompanionResponse::ActionAssessment(r) => Some(&r.reply),
        _ => None,
    }
}
fn final_commands(
    session: &TaskSession,
    run: &str,
    response: &CompanionResponse,
    review: Option<&ActionReviewInput>,
) -> Result<Vec<SessionCommand>> {
    let mut commands = vec![];
    if let Some(reply) = response_reply(response) {
        commands.push(SessionCommand::AssistantMessage {
            text: reply.text.clone(),
            run_id: run.into(),
        });
        if !reply.findings.is_empty() {
            commands.push(SessionCommand::ResearchFindings {
                findings: reply.findings.clone(),
            });
        }
        if let Some(question) = &reply.question {
            commands.push(SessionCommand::AskQuestion {
                question: question.clone(),
                run_id: Some(run.into()),
            });
        }
    }
    match response {
        CompanionResponse::Discussion(_) => {}
        CompanionResponse::SpecDraft(draft) => commands.push(SessionCommand::ProposeSpec {
            spec: draft.spec.clone().context("No spec proposal returned")?,
            run_id: Some(run.into()),
        }),
        CompanionResponse::PlanDraft(draft) => commands.push(SessionCommand::ProposePlan {
            task: draft
                .plan
                .as_ref()
                .context("No plan proposal returned")?
                .into_plan(&session.project, session.plan.as_ref().map(|p| &p.task))?,
            run_id: Some(run.into()),
        }),
        CompanionResponse::WalkthroughDraft(draft) => {
            let walk = walkthrough_proposal(
                session,
                draft.actions.clone().context("No actions returned")?,
            )?;
            commands.push(SessionCommand::SaveWalkthrough { walkthrough: walk });
        }
        CompanionResponse::ActionAssessment(draft) => {
            let input = review.context("Missing action evidence")?;
            let assessment = draft
                .assessment
                .as_ref()
                .context("No assessment returned")?;
            let review =
                parse_action_review_response(session, input, serde_json::to_value(assessment)?)?;
            commands.push(SessionCommand::RecordActionReview {
                review,
                input: Box::new(input.clone()),
            });
        }
        CompanionResponse::Solution(_) => {
            session.authorize(WorkflowAction::Reveal)?;
            session.validate_action_binding(&review.context("Missing action scope")?.binding)?;
            // Bodies live only in the separately stored model run/solution channel.
            commands.push(SessionCommand::AssistantMessage {
                text: "A requested example is available in the separate solution view.".into(),
                run_id: run.into(),
            });
        }
        _ => bail!("Unexpected response for managed conversation"),
    }
    Ok(commands)
}
pub fn all_conversation_events(
    store: &Mutex<Store>,
    session: &str,
) -> Result<Vec<ConversationEvent>> {
    all_events(store, session)
}
fn all_events(store: &Mutex<Store>, session: &str) -> Result<Vec<ConversationEvent>> {
    let store = lock_store(store)?;
    let mut before = None;
    let mut pages = vec![];
    loop {
        let page = store.conversation_events(session, before, 200)?;
        if page.is_empty() {
            break;
        }
        before = page.first().map(|e| e.sequence);
        pages.push(page);
    }
    Ok(pages.into_iter().rev().flatten().collect())
}
fn replayable(event: &ConversationEvent) -> bool {
    matches!(
        event.payload,
        ConversationPayload::HumanMessage { .. }
            | ConversationPayload::AssistantMessage { .. }
            | ConversationPayload::QuestionAsked { .. }
            | ConversationPayload::QuestionAnswered { .. }
            | ConversationPayload::ResearchFindings { .. }
    )
}
fn historical_receipts(
    store: &Mutex<Store>,
    session: &str,
    events: &[ConversationEvent],
) -> Result<Vec<ResearchReceipt>> {
    let store = lock_store(store)?;
    let mut receipts = vec![];
    let mut ids = BTreeSet::new();
    let mut bytes = 0;
    for event in events.iter().rev() {
        if let ConversationPayload::ResearchFindings { findings } = &event.payload {
            for citation in findings.iter().flat_map(|f| &f.citations) {
                if ids.insert(citation.receipt_id.clone()) {
                    let receipt = store
                        .research_receipt(session, &citation.receipt_id)?
                        .context("Saved citation receipt is missing")?;
                    if receipts.len() >= 32
                        || bytes + receipt.content.len() > MAX_RESEARCH_RETURNED_BYTES
                    {
                        return Ok(receipts);
                    }
                    bytes += receipt.content.len();
                    receipts.push(receipt);
                }
            }
        }
    }
    Ok(receipts)
}
fn build_transcript(events: &[ConversationEvent]) -> Result<TranscriptContext> {
    let mut recent = vec![];
    let mut bytes = 0;
    let mut first_recent = u64::MAX;
    for event in events.iter().rev().filter(|e| replayable(e)) {
        let size = serde_json::to_vec(event)?.len();
        if bytes + size > 48 * 1024 {
            break;
        }
        bytes += size;
        first_recent = event.sequence;
        recent.push(event.clone());
    }
    recent.reverse();
    let covered = events
        .iter()
        .filter(|e| e.sequence < first_recent)
        .cloned()
        .collect::<Vec<_>>();
    let checkpoint = if covered.iter().any(replayable) {
        let mut text = String::from(
            "Bounded excerpts of older discussion, not a complete transcript. Use ConversationEvents research to read omitted history.\n",
        );
        // Preserve chronological excerpts within a declared bound. The full
        // authoritative events remain readable, including resolved answers.
        for event in covered.iter().filter(|e| replayable(e)) {
            let excerpt = serde_json::to_string(&event.payload)?
                .chars()
                .take(160)
                .collect::<String>();
            let row = format!("{}: {excerpt}\n", event.sequence);
            if text.len() + row.len() > 12 * 1024 {
                text.push_str("Additional older decisions omitted; read scoped history.\n");
                break;
            }
            text.push_str(&row);
        }
        Some(SummaryCheckpoint {
            first_sequence: covered[0].sequence,
            covered_sequence: covered.last().unwrap().sequence,
            covered_hash: content_hash(&serde_json::to_vec(&covered)?),
            text,
        })
    } else {
        None
    };
    Ok(TranscriptContext {
        omitted_history_available: checkpoint.is_some(),
        checkpoint,
        recent,
    })
}

fn response_value(response: &CompanionResponse) -> Result<serde_json::Value> {
    Ok(match response {
        CompanionResponse::Discussion(r) => serde_json::to_value(r)?,
        CompanionResponse::SpecDraft(r) => serde_json::to_value(r)?,
        CompanionResponse::PlanDraft(r) => serde_json::to_value(r)?,
        CompanionResponse::WalkthroughDraft(r) => serde_json::to_value(r)?,
        CompanionResponse::ActionAssessment(r) => serde_json::to_value(r)?,
        CompanionResponse::Solution(r) => serde_json::to_value(r)?,
        _ => bail!("Unexpected managed response contract"),
    })
}
