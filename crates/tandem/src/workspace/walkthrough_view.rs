//! One concrete action, with ordinary editor navigation and explicit evidence.
use super::companion_state::{self, OutputAttachmentDraft, WalkthroughContinuation};
use super::panels::note;
use super::*;
use crate::appearance::{
    panel_body, panel_heading, panel_metadata, panel_warning, path_button, solid_section,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::*;
use tandem_companion::{ConversationCapture, ConversationIntent, ProjectConversationCapture};

pub(super) struct AttachmentFields {
    pub scope: String,
    command: Entity<TextareaState>,
    cwd: Entity<TextareaState>,
    exit: Entity<TextareaState>,
    stdout: Entity<TextareaState>,
    stderr: Entity<TextareaState>,
}
impl AttachmentFields {
    fn new(
        scope: String,
        draft: OutputAttachmentDraft,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        fn field(
            value: String,
            rows: usize,
            window: &mut Window,
            cx: &mut Context<Workspace>,
        ) -> Entity<TextareaState> {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(rows)
                    .default_value(value)
            })
        }
        Self {
            scope,
            command: field(draft.command, 2, window, cx),
            cwd: field(draft.cwd, 1, window, cx),
            exit: field(draft.exit, 1, window, cx),
            stdout: field(draft.stdout, 4, window, cx),
            stderr: field(draft.stderr, 3, window, cx),
        }
    }
    pub fn snapshot(&self, cx: &App) -> OutputAttachmentDraft {
        OutputAttachmentDraft {
            command: self.command.read(cx).value().to_string(),
            cwd: self.cwd.read(cx).value().to_string(),
            exit: self.exit.read(cx).value().to_string(),
            stdout: self.stdout.read(cx).value().to_string(),
            stderr: self.stderr.read(cx).value().to_string(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalAction {
    Attach,
    Next,
    RefreshReview,
}
enum LocalResult {
    Attachment(Box<(ActionCommandEvidence, ActionReviewBinding)>),
    Next(Box<ActionReviewInput>),
}

impl Workspace {
    pub(super) fn refresh_saved_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.review.as_ref().is_some_and(|(r, o)| {
            self.companion.session.as_ref().is_some_and(|s| {
                companion_state::current_review_candidate(
                    s,
                    r,
                    o.as_ref(),
                    &self.companion.language,
                    &self.effective_guidance(),
                )
            })
        }) {
            self.local_action(LocalAction::RefreshReview, window, cx);
        }
    }
    fn action_scope(&self) -> Option<ActionEvidenceScope> {
        let session = self.companion.session.as_ref()?;
        let (walk, action) = session.current_action().ok()?;
        Some(ActionEvidenceScope {
            workflow: session.authorize(WorkflowAction::ReviewAction).ok()?,
            walkthrough_id: walk.id.clone(),
            action_id: action.id.clone(),
            action_hash: action.definition_hash(),
        })
    }
    pub(super) fn current_attachments(&self) -> Vec<ActionCommandEvidence> {
        self.action_scope()
            .and_then(|scope| {
                self.companion
                    .attachments
                    .get(&serde_json::to_string(&scope).unwrap())
                    .cloned()
            })
            .into_iter()
            .collect()
    }
    pub(super) fn load_attachment_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self
            .action_scope()
            .map(|s| serde_json::to_string(&s).unwrap())
        else {
            self.companion.attachment_fields = None;
            return;
        };
        if self
            .companion
            .attachment_fields
            .as_ref()
            .is_some_and(|f| f.scope == scope)
        {
            return;
        }
        if let Some(fields) = &self.companion.attachment_fields {
            self.companion
                .attachment_drafts
                .insert(fields.scope.clone(), fields.snapshot(cx));
        }
        let Some((_, action)) = self
            .companion
            .session
            .as_ref()
            .and_then(|s| s.current_action().ok())
        else {
            return;
        };
        let ActionTarget::Command { argv, cwd, .. } = &action.target else {
            self.companion.attachment_fields = None;
            return;
        };
        let draft = self
            .companion
            .attachment_drafts
            .get(&scope)
            .cloned()
            .or_else(|| {
                self.companion
                    .attachments
                    .get(&scope)
                    .map(OutputAttachmentDraft::from_evidence)
            })
            .unwrap_or_else(|| OutputAttachmentDraft {
                command: serde_json::to_string(argv).unwrap(),
                cwd: cwd.clone(),
                ..Default::default()
            });
        self.companion.attachment_fields = Some(AttachmentFields::new(scope, draft, window, cx));
    }
    fn review_can_advance(&self) -> bool {
        !self.companion.review_stale
            && self.companion.review_epoch == Some(self.companion.code_epoch)
            && self.companion.review_source_generation
                == Some(self.source_generation.load(Ordering::Relaxed))
            && self.companion.review.as_ref().is_some_and(|(r, o)| {
                if self.current_attachments().iter().any(|c| {
                    !r.binding
                        .evidence
                        .iter()
                        .any(|e| e.id == c.id && e.hash == c.fingerprint())
                }) {
                    return false;
                }
                self.companion.session.as_ref().is_some_and(|s| {
                    companion_state::current_review_candidate(
                        s,
                        r,
                        o.as_ref(),
                        &self.companion.language,
                        &self.effective_guidance(),
                    )
                })
            })
    }
    pub(super) fn continue_walkthrough(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.companion.session else {
            return;
        };
        match companion_state::walkthrough_continuation(session) {
            WalkthroughContinuation::PrepareAction => {
                self.start_conversation(ConversationIntent::PrepareWalkthrough, window, cx)
            }
            WalkthroughContinuation::Complete => {
                self.companion.status = "All actions and final task acceptance reviewed.".into();
                cx.notify();
            }
            _ => cx.notify(),
        }
    }
    /// Local capture never contacts an account or model. The UI rechecks all
    /// live buffer versions before committing the atomic Store transition.
    fn local_action(&mut self, kind: LocalAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy || self.loading || self.dev.check_cancel.is_some()
        {
            return;
        }
        if kind == LocalAction::Next && !self.review_can_advance() {
            return;
        }
        let result = (|| -> Result<_> {
            self.preserve_composer(cx)?;
            let attachment = if kind == LocalAction::Attach {
                Some(
                    self.companion
                        .attachment_fields
                        .as_ref()
                        .context("Open Attach Output first")?
                        .snapshot(cx)
                        .attachment()?,
                )
            } else {
                None
            };
            let session = self
                .companion
                .session
                .clone()
                .context("Open a conversation")?;
            let scope = self
                .action_scope()
                .context("Prepare the current action first")?;
            let project = self.project.clone().context("Open a project")?;
            let overlays = self.working_snapshots(cx)?;
            let versions = content_hash(&serde_json::to_vec(&overlays)?);
            Ok((attachment, session, scope, project, overlays, versions))
        })();
        let (attachment, session, scope, project, overlays, versions) = match result {
            Ok(v) => v,
            Err(e) => {
                self.companion.status = e.to_string();
                cx.notify();
                return;
            }
        };
        let id = new_id();
        let session_id = session.id.clone();
        let epoch = self.epoch;
        let code_epoch = self.companion.code_epoch;
        let source_generation = self.source_generation.load(Ordering::Relaxed);
        let language = self.companion.language.clone();
        let guidance = self.effective_guidance();
        let selected = self.selected_context_paths(cx);
        let store = self.store.clone();
        let mut check_environment = self.dev.environment.clone();
        if let Some(env) = &mut check_environment {
            env.tools = self.dev.rust.clone();
            env.python = self.dev.python.clone();
        }
        let mut capture = ProjectConversationCapture {
            project: project.clone(),
            overlays: overlays.clone(),
            selected: selected.clone(),
            cache: super::companion::companion_cache(),
            check_environment,
            recipe_ids: self.dev.recipes.iter().map(|r| r.id.clone()).collect(),
            attachments: self.current_attachments(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.companion.cancel = Some(cancel.clone());
        self.companion.busy = true;
        self.companion.local_action = Some(kind);
        self.companion.operation_id = Some(id.clone());
        self.companion.status = match kind {
            LocalAction::Next => "Checking current inputs before Next…",
            LocalAction::RefreshReview => "Checking saved review against current inputs…",
            LocalAction::Attach => "Capturing inputs for UserAttachedOutput…",
        }
        .into();
        cx.notify();
        let requested_language = language.clone();
        let requested_guidance = guidance.fingerprint();
        let operation = cx.background_spawn(async move {
            if let Some(attachment) = attachment {
                let captured = tandem_companion::capture_action_context(
                    &session,
                    &project,
                    &overlays,
                    &selected,
                    &super::companion::companion_cache(),
                    &cancel,
                )?;
                let binding = ActionReviewInput::capture(
                    &session,
                    captured.context.clone(),
                    captured.targets.clone(),
                    vec![],
                    &language,
                    &guidance,
                )?
                .binding;
                Ok(LocalResult::Attachment(Box::new((
                    tandem_companion::attach_action_output(scope, &captured, attachment)?,
                    binding,
                ))))
            } else {
                let checks = store.lock().unwrap().checks(project.key())?;
                let checks =
                    super::companion::action_checks(&store.lock().unwrap(), &session, &checks)?;
                capture.attachments = store
                    .lock()
                    .unwrap()
                    .current_action_attachment(&session)?
                    .into_iter()
                    .collect();
                let input = capture
                    .capture(
                        &session,
                        CompanionAction::ReviewAction,
                        &language,
                        &guidance,
                        &checks,
                        &cancel,
                    )?
                    .review
                    .context("Missing review capture")?;
                Ok::<_, anyhow::Error>(LocalResult::Next(Box::new(
                    super::companion::authenticate_capture(
                        &store.lock().unwrap(),
                        &session,
                        input,
                        &language,
                        &guidance,
                    )?,
                )))
            }
        });
        let weak = cx.entity().downgrade();
        window.spawn(cx,async move |cx| {
            let result=operation.await;
            let _=weak.update_in(cx,|view,window,cx| {
                if view.epoch!=epoch || view.companion.operation_id.as_ref()!=Some(&id) || view.companion.session.as_ref().is_none_or(|s|s.id!=session_id) { return; }
                let stopped=view.companion.cancel.as_ref().is_some_and(|c|c.load(Ordering::Relaxed));
                view.companion.busy=false;view.companion.cancel=None;view.companion.operation_id=None;view.companion.local_action=None;
                let result=(||->Result<()> {
                    anyhow::ensure!(!stopped,"Cancelled. The current action and attachment draft are retained");
                    anyhow::ensure!(view.companion.code_epoch==code_epoch && view.source_generation.load(Ordering::Relaxed)==source_generation && view.companion.language==requested_language && view.effective_guidance().fingerprint()==requested_guidance
                        && content_hash(&serde_json::to_vec(&view.working_snapshots(cx)?)?)==versions,
                        "Inputs changed during capture. Review or attach output again using current inputs");
                    match result? {
                        LocalResult::Attachment(attachment)=> {
                            let (receipt,binding)=*attachment;
                            anyhow::ensure!(view.action_scope().as_ref()==Some(&receipt.scope),"The current action changed");
                            let receipt=view.store.lock().unwrap().record_action_attachment(&session_id,&receipt,&binding)?;
                            view.companion.attachments.insert(serde_json::to_string(&receipt.scope)?,receipt);
                            view.companion.review_stale=true;
                            view.companion.status="Saved UserAttachedOutput · Unvalidated. Bound to this action and captured inputs; request Review.".into();
                        }
                        LocalResult::Next(input)=> {
                            let session=view.companion.session.as_ref().context("No conversation")?;
                            let review=&view.companion.review.as_ref().context("Review this action first")?.0;
                            companion_state::review_matches_capture(session,review,&input)?;
                            if kind==LocalAction::RefreshReview {
                                let outcome=view.companion.review.as_ref().unwrap().1.as_ref();
                                anyhow::ensure!(companion_state::current_review_candidate(session,review,outcome,&requested_language,&view.effective_guidance()),"The saved review did not complete; request Review again");
                                view.companion.review_stale=false;
                                view.companion.review_epoch=Some(code_epoch);
                                view.companion.review_source_generation=Some(source_generation);
                                view.companion.status="Saved passing review matches current inputs. Next is available.".into();
                                return Ok(());
                            }
                            companion_state::advance_review(&mut view.store.lock().unwrap(),session,review,&input)?;
                            view.companion.review_epoch=None;view.companion.review_stale=false;
                            view.reload_conversation(false,window,cx);
                            view.companion.status="Action completed. Continue with the current action.".into();
                            view.continue_walkthrough(window,cx);
                        }
                    }
                    Ok(())
                })();
                if let Err(e)=result { if kind!=LocalAction::Attach && !stopped {view.companion.review_stale=true;} view.companion.status=e.to_string(); }
                cx.notify();
            });
        }).detach();
    }
    pub(super) fn walkthrough_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = solid_section(panel_body(cx), cx).p_3().border_1();
        let Some(session) = &self.companion.session else {
            return div().into_any_element();
        };
        let Ok((_, action)) = session.current_action() else {
            return body
                .child(note(if session.implementation_complete() {
                    "All actions and final task acceptance reviewed."
                } else {
                    "Prepare the active step to see its first concrete action."
                }))
                .children(
                    companion_state::completed_action_summary(session)
                        .into_iter()
                        .map(|row| panel_metadata(note(row), cx)),
                )
                .into_any_element();
        };
        let busy = self.companion.busy || self.agent.busy;
        body = body.children(
            companion_state::current_action_summary(session)
                .into_iter()
                .map(note),
        );
        match &action.target {
            ActionTarget::File {
                path,
                symbol,
                change_kind,
            } => {
                let path = path.clone();
                let new = *change_kind == FileChangeKind::Create;
                body = body
                    .child(note(format!(
                        "Target: {path}{} · {change_kind:?}",
                        symbol
                            .as_ref()
                            .map(|s| format!(" · {s}"))
                            .unwrap_or_default()
                    )))
                    .child(
                        path_button(
                            Button::new("open-action-target"),
                            if new {
                                "Open / Create in editor"
                            } else {
                                "Open in editor"
                            },
                        )
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open_file(path.clone(), new, None, w, cx)
                        })),
                    );
            }
            ActionTarget::Command {
                argv,
                cwd,
                expected_exit_codes,
            } => {
                let command = argv
                    .iter()
                    .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ");
                body=body.child(note(format!("Command: {command}\nWorking directory (project relative): {cwd}\nExpected exit: {expected_exit_codes:?}")))
                    .child(path_button(Button::new("copy-action-command"),"Copy Command").on_click(move |_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))));
                if let Some(recipe) =
                    companion_state::matching_action_recipe(action, &self.dev.recipes)
                {
                    let id = recipe.to_string();
                    body = body.child(
                        path_button(
                            Button::new("run-action-check"),
                            "Run Check · independently authorized recipe",
                        )
                        .disabled(
                            self.dev.check_cancel.is_some() || self.agent.busy || self.loading,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.prepare_recipe(&id, cx);
                            if this.dev.pending_recipe.is_some() {
                                this.dev.panel = Panel::Checks;
                            }
                        })),
                    );
                } else {
                    body=body.child(note("Run this exact command yourself in the terminal, then deliberately attach its complete output. Copy does not execute it."))
                        .child(path_button(Button::new("action-terminal"),"Open terminal").on_click(cx.listener(|this,_,_,cx| { if !this.show_terminal {this.toggle_terminal(cx);} })));
                }
                if let Some(fields) = &self.companion.attachment_fields {
                    body=body.child(panel_heading(note("Attach Output · UserAttachedOutput / Unvalidated")))
                        .child(note("This is your report of a manual run on the current inputs. It is separate from authenticated local check receipts. Preserve complete output; later source edits require a new run and attachment."));
                    for (label, field) in [
                        ("Exact command (JSON argv array)", &fields.command),
                        ("Working directory (project relative)", &fields.cwd),
                        ("Numeric exit status", &fields.exit),
                        ("Complete stdout", &fields.stdout),
                        ("Complete stderr", &fields.stderr),
                    ] {
                        body = body
                            .child(note(label))
                            .child(Textarea::new(field).aria_label(label));
                    }
                    body = body.child(
                        path_button(
                            Button::new("attach-action-output"),
                            if self.companion.local_action == Some(LocalAction::Attach) {
                                "Capturing attachment…"
                            } else {
                                "Attach Output"
                            },
                        )
                        .loading(self.companion.local_action == Some(LocalAction::Attach))
                        .disabled(busy || self.dev.check_cancel.is_some())
                        .on_click(cx.listener(|this, _, w, cx| {
                            this.local_action(LocalAction::Attach, w, cx)
                        })),
                    );
                }
            }
            ActionTarget::Validation {
                criterion_refs,
                paths,
                ..
            } => {
                body = body.child(note(
                    "Review only · cumulative assessment; no edit or command execution implied.",
                ));
                for criterion in criterion_refs {
                    body = body.child(note(criterion_label(session, criterion)));
                }
                for path in paths {
                    body = body.child(note(format!("Evidence path: {path}")));
                }
            }
        }
        for requirement in &action.required_evidence {
            body = body.child(note(format!(
                "Required evidence: {}",
                match requirement {
                    RequiredActionEvidence::FileSnapshot { path } =>
                        format!("current file snapshot · {path}"),
                    RequiredActionEvidence::CommandReceipt =>
                        "complete exact command/cwd, exit and output on current inputs".into(),
                    RequiredActionEvidence::CheckReceipt { recipe_id } =>
                        format!("authenticated local check · {recipe_id}"),
                    RequiredActionEvidence::CriterionAssessment { criterion_ref } =>
                        criterion_label(session, criterion_ref),
                }
            )));
        }
        for receipt in self.current_attachments() {
            body = body.child(panel_metadata(
                note(format!(
                    "Saved output: {} · UserAttachedOutput · Unvalidated · exit {:?}\nBound to captured inputs: {}. Changed inputs require a new run and attachment.",
                    receipt.id, receipt.exit_code,receipt.snapshot.as_ref().map(|s|s.id.as_str()).unwrap_or("unknown")
                )),
                cx,
            ));
        }
        if let Some((review, outcome)) = &self.companion.review {
            let current = !busy && self.review_can_advance();
            body = body
                .child(panel_heading(note(if current {
                    "Current passing review · Next rechecks inputs"
                } else {
                    "Review history · applies to its captured inputs"
                })))
                .child(note(format!("{:?} · {}", review.outcome, review.summary)));
            if !matches!(outcome, Some(OperationOutcome::Completed { .. })) {
                body = body.child(panel_warning(
                    note("This review operation did not complete; request Review again."),
                    cx,
                ));
            }
            for finding in &review.findings {
                body = body.child(note(format!(
                    "{}\nWhy: {}\nNext: {}",
                    finding.problem, finding.why, finding.next_action
                )));
            }
            for limitation in &review.limitations {
                body = body.child(panel_warning(note(limitation.clone()), cx));
            }
            for command in &self.companion.review_commands {
                let provenance = match &command.provenance {
                    CommandProvenance::LocalCheck { check_id, .. } => {
                        format!("Authenticated local check · {check_id}")
                    }
                    CommandProvenance::UserAttachedOutput => {
                        "UserAttachedOutput · Unvalidated".into()
                    }
                };
                body = body.child(panel_metadata(
                    note(format!(
                        "Review evidence: {provenance} · {:?} · exit {:?}\nargv: {:?}\ncwd: {}",
                        command.status, command.exit_code, command.argv, command.cwd
                    )),
                    cx,
                ));
            }
            if !self.review_can_advance() {
                body = body.child(note(
                    "This saved review does not yet apply to current inputs. Check freshness, or request Review again.",
                ));
                body = body.child(
                    path_button(
                        Button::new("refresh-saved-review"),
                        "Check saved review freshness",
                    )
                    .disabled(
                        busy || !companion_state::current_review_candidate(
                            session,
                            review,
                            outcome.as_ref(),
                            &self.companion.language,
                            &self.effective_guidance(),
                        ),
                    )
                    .on_click(cx.listener(|this, _, w, cx| this.refresh_saved_review(w, cx))),
                );
            }
        } else if !busy {
            body = body.child(note(
                "Complete this action, then request Review. A current passing review enables Next.",
            ));
        }
        if let Some(op) = &self.companion.foreground {
            body = body.child(note(companion_state::phase_label(op.phase)));
        }
        body = body.child(
            path_button(
                Button::new("next-guided-action"),
                if self.companion.local_action == Some(LocalAction::RefreshReview) {
                    "Checking saved review freshness…"
                } else if self.companion.local_action == Some(LocalAction::Next) {
                    "Checking inputs before Next…"
                } else if busy {
                    "Next · Another operation is active"
                } else {
                    "Next"
                },
            )
            .loading(matches!(
                self.companion.local_action,
                Some(LocalAction::Next | LocalAction::RefreshReview)
            ))
            .disabled(busy || self.dev.check_cancel.is_some() || !self.review_can_advance())
            .on_click(cx.listener(|this, _, w, cx| this.local_action(LocalAction::Next, w, cx))),
        );
        body = body.children(
            companion_state::completed_action_summary(session)
                .into_iter()
                .map(|row| panel_metadata(note(row), cx)),
        );
        body.into_any_element()
    }
}
fn criterion_label(session: &TaskSession, criterion: &CriterionRef) -> String {
    match criterion {
        CriterionRef::SpecAcceptance { id } => format!(
            "Spec acceptance {id}: {}",
            session
                .spec
                .as_ref()
                .and_then(|s| s.acceptance.iter().find(|c| c.id == *id))
                .map(|c| c.text.as_str())
                .unwrap_or("unavailable")
        ),
        CriterionRef::StepDoneWhen { step_id, index } => format!(
            "Step {step_id} criterion {}: {}",
            index + 1,
            session
                .plan
                .as_ref()
                .and_then(|p| p.task.steps.iter().find(|s| s.id == *step_id))
                .and_then(|s| s.done_when.get(*index))
                .map(String::as_str)
                .unwrap_or("unavailable")
        ),
    }
}
