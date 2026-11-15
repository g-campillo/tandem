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
    /// Explicit Hint request; discussion prose with implementation authority checked throughout.
    GatedDiscussion(String),
    Improve {
        objective: ImprovementObjective,
        question: String,
    },
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
            Self::Message(_) | Self::GatedDiscussion(_) | Self::Answer(_) | Self::Discuss => {
                CompanionAction::Discuss
            }
            Self::Improve { objective, .. } => match objective {
                ImprovementObjective::Clarity => CompanionAction::ImproveClarity,
                ImprovementObjective::Speed => CompanionAction::ImproveSpeed,
                ImprovementObjective::Balanced => CompanionAction::ImproveBalanced,
            },
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
            Self::Message(text)
            | Self::GatedDiscussion(text)
            | Self::Improve { question: text, .. } => text.clone(),
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
    // Retain every screened private input for transport reuse across operations,
    // even when bounded receipt/history replay has since omitted it.
    privacy: BTreeMap<String, PrivateResearchScreening>,
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
            privacy: BTreeMap::new(),
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
        // Only an explicit answer resumes unfinished typed work. The origin is
        // host-recorded durable metadata, never a model-selected operation.
        let continuation = if let ConversationIntent::Answer(answer) = &input.intent {
            all_events(&self.store, &session_id)?
                .into_iter()
                .rev()
                .find_map(|event| match event.payload {
                    ConversationPayload::QuestionContinuationRecorded {
                        question_id,
                        continuation,
                    } if question_id == answer.question_id => Some(continuation),
                    _ => None,
                })
        } else {
            None
        };
        let action = match continuation.as_ref().map(|c| c.kind) {
            Some(SessionOperationKind::DraftSpec) => CompanionAction::DraftSpec,
            Some(SessionOperationKind::DraftPlan) => CompanionAction::DraftPlan,
            Some(SessionOperationKind::PrepareAction) => CompanionAction::PrepareWalkthrough,
            Some(SessionOperationKind::ReviewAction) => CompanionAction::ReviewAction,
            Some(_) => bail!("Unsupported clarification continuation"),
            None => input.intent.action(),
        };
        let mut historical = false;
        let mut result_review = None;
        let mut result_walkthrough = None;
        let mut pending_transition = None;
        let mut pending_thread = None;
        let mut operation_thread = None;
        let operation = ActiveOperation {
            id: id.clone(),
            session_id: session_id.clone(),
            kind: continuation
                .as_ref()
                .map_or_else(|| input.intent.kind(), |c| c.kind),
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
                ConversationIntent::Message(text)
                | ConversationIntent::GatedDiscussion(text)
                | ConversationIntent::Improve { question: text, .. } => {
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
            if let Some(continuation) = &continuation {
                continuation.validate(&session)?;
            }
            let workflow_action = if matches!(input.intent, ConversationIntent::GatedDiscussion(_))
            {
                Some(WorkflowAction::PrepareAction)
            } else {
                workflow_action(action)
            };
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
                // Cumulative review inputs keep each check's authenticated
                // execution origin. Retain those consumed action definitions
                // too, so validation never relabels a prior check as current or
                // rejects it merely because its walkthrough was pruned here.
                let origins = captured
                    .review
                    .as_ref()
                    .map(|review| {
                        review
                            .commands
                            .iter()
                            .map(|command| &command.scope.walkthrough_id)
                            .collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                prompt_session
                    .walkthroughs
                    .retain(|w| w.id == walk.id || origins.contains(&w.id));
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
            let mut privacy = self.privacy.get(&session_id).cloned().unwrap_or_default();
            privacy.include_identifiers(&captured.private_identifiers);
            // Include older durable inputs too: a reused private transport may
            // still know them even when the bounded replay window omits them.
            for event in &events {
                if replayable(event) {
                    privacy.include(&event.payload)?;
                }
            }
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
            operation_thread = Some((session.project.clone(), key.clone()));
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
                    evidence: if matches!(
                        action,
                        CompanionAction::ImproveClarity
                            | CompanionAction::ImproveSpeed
                            | CompanionAction::ImproveBalanced
                    ) {
                        matching_check_evidence(
                            &captured.context,
                            &checks,
                            &captured
                                .context
                                .validation_environments
                                .iter()
                                .filter(|(_, fingerprint)| fingerprint.as_str() != "unavailable")
                                .map(|(id, fingerprint)| (id.clone(), fingerprint.clone()))
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        vec![]
                    },
                    prior_decisions: vec![],
                    delegation: None,
                };
                request.validate_guided()?;
                privacy.include_request(&request)?;
                self.privacy.insert(session_id.clone(), privacy.clone());
                let original_request = request.clone();
                let run = ModelRun::pending(session.project.clone(), request);
                lock_store(&self.store)?.save_model_run(&run)?;
                let store = self.store.clone();
                if let Some(action) = workflow_action {
                    ensure!(
                        load(&self.store, &session_id)?.binding_is_current(
                            action,
                            guided
                                .workflow
                                .as_ref()
                                .context("Missing workflow binding")?
                        ),
                        "Workflow authority changed before dispatch"
                    );
                }
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
                let mut response = match original_request.parse_response(response_value(&response)?)
                {
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
                let reply = response_reply(&response).cloned();
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
                                // A preceding request in this very round may
                                // have expanded files, receipts or older history.
                                privacy.include_guided(&guided)?;
                                let screened = privacy.screen(brief)?;
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
                                if matches!(kind, ResearchRequestKind::ResearchReceipts { .. }) {
                                    read_saved_receipts(
                                        &self.store,
                                        &session_id,
                                        request,
                                        &mut guided.receipts,
                                        &mut budget,
                                    )?
                                } else {
                                    self.broker
                                        .execute(request, &mut research, &cancel, |_| Ok(()))?
                                }
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
                            include_receipt(&mut guided.receipts, receipt)?;
                        }
                        // Only confirmed complete subturns are reusable. No
                        // model/private context is ever reused by the public worker.
                        if let Some(thread) = &thread {
                            self.threads.insert(key.clone(), thread.clone());
                            lock_store(&self.store)?.save_companion_thread(
                                &session.project,
                                &key,
                                thread,
                            )?;
                        }
                        if reply.question.is_some() {
                            // Research and a clarification can arrive together
                            // under the wire schema. Finish the bounded broker
                            // work first, then atomically publish only the
                            // unanswered question. An artifact in this mixed
                            // response cannot acquire implementation authority.
                            response = question_after_research(response)?;
                        } else {
                            continue;
                        }
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
                    check_cancel(&cancel)?;
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
                if let Some(continuation) = &continuation {
                    transition = transition.with_question_continuation(continuation.clone());
                }
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
                check_cancel(&cancel)?;
                pending_transition = Some(transition);
                pending_thread =
                    thread.map(|thread| (session.project.clone(), key.clone(), thread));
                return Ok(Some(response));
            }
        })();
        // Nothing actionable is published before the terminal decision. Holding
        // the slot lock across the Store transaction linearizes accepted Cancel
        // against completion, and the transaction prevents provisional authority.
        let terminal = self.operations.settle(&id, |cancelled| {
            let outcome = if cancelled {
                OperationOutcome::Cancelled {
                    detail: "Cancelled; no further model or research work was dispatched.".into(),
                }
            } else if let Err(error) = &result {
                OperationOutcome::Failed {
                    detail: safe_error(error),
                }
            } else {
                OperationOutcome::Completed {
                    summary:
                        "Operation completed; proposals require explicit human save and approval."
                            .into(),
                }
            };
            let publication = publish_terminal(
                &self.store,
                &session_id,
                &id,
                outcome,
                pending_transition.take(),
                pending_thread.as_ref(),
            );
            if !publication
                .as_ref()
                .is_ok_and(|outcome| matches!(outcome, OperationOutcome::Completed { .. }))
                && let Some((project, key)) = &operation_thread
            {
                lock_store(&self.store)?.forget_companion_thread(project, key)?;
            }
            publication
        });
        let completed = terminal
            .as_ref()
            .is_ok_and(|outcome| matches!(outcome, OperationOutcome::Completed { .. }));
        if !completed {
            self.threads
                .retain(|key, _| !key.starts_with(&format!("guided:{session_id}:")));
            result_review = None;
            result_walkthrough = None;
        } else {
            if let Some(review) = &result_review {
                result_review = lock_store(&self.store)?.action_review(&session_id, &review.id)?;
            }
            if let Some((_, key, thread)) = pending_thread {
                self.threads.insert(key, thread);
            }
        }
        let outcome = terminal?;
        Ok(ConversationResult {
            operation_id: id,
            outcome,
            response: if completed {
                result.ok().flatten()
            } else {
                None
            },
            duplicate: false,
            historical: historical || !completed,
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
/// Called with the operation slot locked. Artifacts and the Completed fence
/// commit together, so a failed terminal write cannot leave usable reviews.
fn publish_terminal(
    store: &Mutex<Store>,
    session: &str,
    id: &str,
    mut outcome: OperationOutcome,
    pending: Option<SessionTransition>,
    thread: Option<&(String, String, ThreadBinding)>,
) -> Result<OperationOutcome> {
    let active = lock_store(store)?
        .active_session_operation(session)?
        .is_some_and(|op| op.id == id);
    if !active {
        ensure!(
            !matches!(outcome, OperationOutcome::Completed { .. }),
            "Operation was superseded before publication"
        );
        return Ok(outcome);
    }
    let finish = |outcome: &OperationOutcome| SessionCommand::FinishOperation {
        operation_id: id.into(),
        outcome: outcome.clone(),
    };
    let empty = || SessionTransition::new(new_id(), vec![]).for_operation(id);
    let completed = matches!(outcome, OperationOutcome::Completed { .. });
    let mut transition = if completed {
        pending.unwrap_or_else(empty)
    } else {
        empty()
    };
    if matches!(outcome, OperationOutcome::Cancelled { .. }) {
        transition.commands.push(SessionCommand::OperationPhase {
            operation_id: id.into(),
            phase: OperationPhase::Stopping,
        });
    }
    transition.commands.push(finish(&outcome));
    let publication = (|| -> Result<()> {
        if completed && let Some((project, key, thread)) = thread {
            lock_store(store)?.save_companion_thread(project, key, thread)?;
        }
        apply(store, session, transition)?;
        Ok(())
    })();
    if let Err(error) = publication {
        outcome = OperationOutcome::Failed {
            detail: safe_error(&error),
        };
        // Publication rolled back. If even this failure fence cannot persist,
        // restart reconciles Interrupted rather than claiming completion.
        apply(
            store,
            session,
            SessionTransition::new(format!("{id}-terminal"), vec![finish(&outcome)])
                .for_operation(id),
        )?;
    }
    Ok(outcome)
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
        CompanionAction::ImproveClarity
        | CompanionAction::ImproveSpeed
        | CompanionAction::ImproveBalanced => Some(WorkflowAction::PrepareAction),
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
fn question_after_research(mut response: CompanionResponse) -> Result<CompanionResponse> {
    let reply = match &mut response {
        CompanionResponse::Discussion(reply) => reply,
        CompanionResponse::SpecDraft(draft) => {
            draft.spec = None;
            &mut draft.reply
        }
        CompanionResponse::PlanDraft(draft) => {
            draft.plan = None;
            &mut draft.reply
        }
        CompanionResponse::WalkthroughDraft(draft) => {
            draft.actions = None;
            &mut draft.reply
        }
        CompanionResponse::ActionAssessment(draft) => {
            draft.assessment = None;
            &mut draft.reply
        }
        _ => bail!("Only a validated conversational response can defer a question"),
    };
    ensure!(reply.question.is_some(), "No clarification was requested");
    reply.research_requests.clear();
    Ok(response)
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
    // A validated clarification is a complete conversational result while its
    // typed artifact is still pending. Publish the question and origin together,
    // without inventing a proposal, walkthrough or review.
    let unfinished = match response {
        CompanionResponse::SpecDraft(draft) if draft.spec.is_none() => {
            Some(SessionOperationKind::DraftSpec)
        }
        CompanionResponse::PlanDraft(draft) if draft.plan.is_none() => {
            Some(SessionOperationKind::DraftPlan)
        }
        CompanionResponse::WalkthroughDraft(draft) if draft.actions.is_none() => {
            Some(SessionOperationKind::PrepareAction)
        }
        CompanionResponse::ActionAssessment(draft) if draft.assessment.is_none() => {
            Some(SessionOperationKind::ReviewAction)
        }
        _ => None,
    };
    if let Some(kind) = unfinished
        && let Some(question) = response_reply(response).and_then(|reply| reply.question.as_ref())
    {
        commands.push(SessionCommand::RecordQuestionContinuation {
            question_id: question.id.clone(),
            continuation: QuestionContinuation::capture(session, kind)?,
        });
        return Ok(commands);
    }
    match response {
        CompanionResponse::Discussion(_) => {}
        CompanionResponse::Improvement(proposal) => {
            commands.push(SessionCommand::AssistantMessage {
                text: format!(
                    "Structured read-only improvement · {:?} · {:?}\n{}",
                    proposal.objective, proposal.decision, proposal.summary
                ),
                run_id: run.into(),
            })
        }
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
            | ConversationPayload::ResearchReceiptRecorded { .. }
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
    let mut bytes = 2;
    'events: for event in events.iter().rev() {
        let event_ids = match &event.payload {
            ConversationPayload::ResearchReceiptRecorded { receipt_id } => {
                vec![receipt_id.as_str()]
            }
            ConversationPayload::ResearchFindings { findings } => findings
                .iter()
                .flat_map(|finding| &finding.citations)
                .map(|citation| citation.receipt_id.as_str())
                .collect(),
            _ => continue,
        };
        for id in event_ids {
            if ids.len() >= MAX_RESEARCH_RECEIPTS as usize {
                break 'events;
            }
            if ids.insert(id.to_string()) {
                let receipt = store
                    .research_receipt(session, id)?
                    .context("Saved research receipt is missing")?;
                let size = serde_json::to_vec(&receipt)?.len() + 1;
                if receipts.len() < MAX_RESEARCH_RECEIPTS as usize
                    && bytes + size <= MAX_RESEARCH_RETURNED_BYTES
                {
                    bytes += size;
                    receipts.push(receipt);
                }
            }
        }
    }
    receipts.reverse();
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
        CompanionResponse::Improvement(r) => serde_json::to_value(r)?,
        _ => bail!("Unexpected managed response contract"),
    })
}

/// Local, conservative matching of concrete private inputs, not a semantic
/// privacy classifier. Only the resulting screened brief crosses PublicWorker.
/// Saturation withholds public work instead of silently dropping known inputs.
#[derive(Clone, Default)]
struct PrivateResearchScreening {
    identifiers: BTreeSet<String>,
    bytes: usize,
    saturated: bool,
}
impl PrivateResearchScreening {
    fn include_identifiers(&mut self, identifiers: &[String]) {
        for identifier in identifiers {
            self.insert(identifier.to_lowercase());
        }
    }
    fn insert(&mut self, identifier: String) {
        if identifier.trim().is_empty() || self.identifiers.contains(&identifier) {
            return;
        }
        if self.identifiers.len() >= 32_768 || self.bytes + identifier.len() > 1024 * 1024 {
            self.saturated = true;
            return;
        }
        self.bytes += identifier.len();
        self.identifiers.insert(identifier);
    }
    fn include_guided(&mut self, guided: &GuidedContext) -> Result<()> {
        let mut private = guided.clone();
        private.receipts.retain(|receipt| {
            !matches!(
                receipt.request.kind,
                ResearchRequestKind::PublicSearch { .. } | ResearchRequestKind::PublicPage { .. }
            )
        });
        self.include(&private)
    }
    fn include_request(&mut self, request: &ModelRequest) -> Result<()> {
        let mut private = request.clone();
        if let Some(guided) = private.guided.take() {
            self.include_guided(&guided)?;
        }
        self.include(&private)
    }
    fn include(&mut self, value: &impl serde::Serialize) -> Result<()> {
        self.include_value(&serde_json::to_value(value)?);
        Ok(())
    }
    fn include_value(&mut self, value: &serde_json::Value) {
        if self.saturated {
            return;
        }
        match value {
            serde_json::Value::String(text) => {
                for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
                    let word = word.to_lowercase();
                    if word.len() < 4
                        || generic_public_word(&word)
                        || self.identifiers.contains(&word)
                    {
                        continue;
                    }
                    self.insert(word);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.include_value(value);
                }
            }
            serde_json::Value::Object(values) => {
                // Public source URLs are already screened at dispatch; they do
                // not become private merely because the private model cites them.
                if values.get("type").and_then(|v| v.as_str()) == Some("PublicUrl") {
                    return;
                }
                for value in values.values() {
                    self.include_value(value);
                }
            }
            _ => {}
        }
    }
    fn screen(&self, brief: PublicResearchBrief) -> Result<PublicBriefScreening> {
        if self.saturated {
            return Ok(PublicBriefScreening {
                brief: None,
                omissions: vec!["Public research withheld because local private-input screening reached its bound".into()],
            });
        }
        screen_public_brief(brief, &self.identifiers.iter().cloned().collect::<Vec<_>>())
    }
}
fn generic_public_word(word: &str) -> bool {
    // Common prose and generic technical vocabulary may form public topics.
    // Distinctive names, identifiers, hashes and copied non-generic words remain
    // local. This deliberately prefers a visible omission when uncertain.
    const WORDS: &str = "about after also another before between both could does each from have into more only other same should some than that their them then there these they this those through using when where which while will with would your \
        rust python javascript typescript iterator iterators documentation docs official reference standard library public technical programming language lang function functions type types string strings integer integers array arrays vector vectors hash hashmap list lists \
        async await future futures thread threads mutex memory safety ownership borrow lifetime lifetimes sort sorting search algorithm algorithms complexity performance testing tests unit integration cargo dependency dependencies package packages version \
        error errors handling result option trait traits method methods module modules struct enum impl const usize bool true false none null http https html json unicode encoding database migration migrations sqlite postgresql mysql linux macos windows tokio serde pytest";
    WORDS.split_whitespace().any(|generic| generic == word)
}

/// Keep immutable historical identity/status and a bounded most-recent window.
/// Omitted receipts remain available through the session-scoped typed read.
fn include_receipt(receipts: &mut Vec<ResearchReceipt>, receipt: ResearchReceipt) -> Result<()> {
    receipt.validate()?;
    receipts.retain(|existing| existing.id != receipt.id);
    receipts.push(receipt);
    while receipts.len() > MAX_RESEARCH_RECEIPTS as usize
        || serde_json::to_vec(receipts)?.len() > MAX_RESEARCH_RETURNED_BYTES
    {
        receipts.remove(0);
    }
    Ok(())
}

fn read_saved_receipts(
    store: &Mutex<Store>,
    session: &str,
    request: ResearchRequest,
    receipts: &mut Vec<ResearchReceipt>,
    budget: &mut ResearchBudget,
) -> Result<ResearchReceipt> {
    let ResearchRequestKind::ResearchReceipts { receipt_ids } = &request.kind else {
        bail!("Expected a scoped receipt read");
    };
    let store = lock_store(store)?;
    let mut content =
        String::from("Historical receipt bodies restored into the bounded receipt context:\n");
    let mut omissions = vec![];
    for id in receipt_ids {
        let Some(receipt) = store.research_receipt(session, id)? else {
            omissions.push(ResearchOmission {
                locator: None,
                reason: "Requested receipt is unavailable in this session".into(),
            });
            continue;
        };
        let size = serde_json::to_vec(&receipt)?.len();
        // Reserve the complete original body/metadata plus the small retrieval
        // receipt. Never truncate evidence or exceed this operation's return cap.
        if budget.returned_bytes + size + 4096 > MAX_RESEARCH_RETURNED_BYTES {
            omissions.push(ResearchOmission {
                locator: None,
                reason: "Receipt retrieval reached the operation byte budget; Continue explicitly"
                    .into(),
            });
            continue;
        }
        budget.returned_bytes += size;
        content.push_str(&format!(
            "{}: {:?}; snapshot {:?}; original source identities and capture times retained.\n",
            receipt.id, receipt.status, receipt.snapshot_id
        ));
        include_receipt(receipts, receipt)?;
    }
    let status = if omissions.is_empty() {
        ResearchReceiptStatus::Complete
    } else {
        ResearchReceiptStatus::Partial
    };
    ResearchReceipt::new(request, vec![], status, content, omissions)
}
