use super::companion::lines;
use super::companion_state::{CompanionTab, approval_matches};
use super::panels::note;
use super::*;
use crate::appearance::{
    panel_body, panel_heading, panel_metadata, panel_warning, path_button, solid_section,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_companion::ConversationIntent;

pub struct Field {
    key: String,
    label: String,
    input: Entity<TextareaState>,
}
pub struct DocumentFields {
    pub spec: SpecDocument,
    pub plan: Option<PlanDocument>,
    pub fields: Vec<Field>,
}
impl DocumentFields {
    pub fn fingerprint(&self, cx: &App) -> String {
        content_hash(
            self.fields
                .iter()
                .map(|f| f.input.read(cx).value().to_string())
                .collect::<Vec<_>>()
                .join("\0")
                .as_bytes(),
        )
    }

    fn text(&self, key: &str, cx: &App) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.input.read(cx).value().to_string())
            .unwrap_or_default()
    }
    fn read_spec(&self, cx: &App) -> Result<SpecDocument> {
        let mut spec = self.spec.clone();
        spec.summary = self.text("summary", cx);
        spec.requirements = criteria(&self.text("requirements", cx))?;
        spec.acceptance = criteria(&self.text("acceptance", cx))?;
        spec.in_scope = lines(&self.text("in_scope", cx));
        spec.out_of_scope = lines(&self.text("out_of_scope", cx));
        spec.constraints = lines(&self.text("spec_constraints", cx));
        spec.validate()?;
        Ok(spec)
    }
    fn read_plan(&self, cx: &App) -> Result<PlanDocument> {
        let mut plan = self
            .plan
            .clone()
            .context("Save a spec and draft a plan first")?;
        plan.task.goal = self.text("goal", cx);
        plan.task.analysis = self.text("analysis", cx);
        plan.task.acceptance = lines(&self.text("plan_acceptance", cx));
        plan.task.constraints = lines(&self.text("plan_constraints", cx));
        plan.task.unknowns = lines(&self.text("unknowns", cx));
        for (i, step) in plan.task.steps.iter_mut().enumerate() {
            step.title = self.text(&format!("step.{i}.title"), cx);
            step.why = self.text(&format!("step.{i}.why"), cx);
            step.files = lines(&self.text(&format!("step.{i}.files"), cx));
            step.symbols = lines(&self.text(&format!("step.{i}.symbols"), cx));
            step.languages = lines(&self.text(&format!("step.{i}.languages"), cx));
            step.depends_on = lines(&self.text(&format!("step.{i}.dependencies"), cx));
            step.done_when = lines(&self.text(&format!("step.{i}.done"), cx));
        }
        plan.validate()?;
        Ok(plan)
    }
    fn dirty(&self, kind: WorkflowDocumentKind, session: &TaskSession, cx: &App) -> bool {
        match kind {
            WorkflowDocumentKind::Spec => self
                .read_spec(cx)
                .ok()
                .zip(session.spec.as_ref())
                .is_none_or(|(d, s)| d.document_ref() != s.document_ref()),
            WorkflowDocumentKind::Plan => self
                .read_plan(cx)
                .ok()
                .zip(session.plan.as_ref())
                .is_none_or(|(d, s)| d.document_ref() != s.document_ref()),
        }
    }
}
fn criteria(text: &str) -> Result<Vec<Criterion>> {
    lines(text)
        .into_iter()
        .map(|line| {
            let (id, text) = line
                .split_once('|')
                .context("Criteria use stable-id | text, one per line")?;
            Ok(Criterion {
                id: id.trim().into(),
                text: text.trim().into(),
            })
        })
        .collect()
}
fn criterion_text(criteria: &[Criterion]) -> String {
    criteria
        .iter()
        .map(|c| format!("{} | {}", c.id, c.text))
        .collect::<Vec<_>>()
        .join("\n")
}
fn field(
    key: impl Into<String>,
    label: impl Into<String>,
    value: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        input: cx.new(|cx| TextareaState::new(window, cx).rows(3).default_value(value)),
    }
}
impl Workspace {
    pub(super) fn load_document_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.companion.session.clone() else {
            self.companion.documents = None;
            return;
        };
        let proposal = |kind| {
            self.companion
                .events
                .iter()
                .rev()
                .find_map(|e| match (&e.payload, kind) {
                    (ConversationPayload::DocumentSaved { kind: saved, .. }, requested)
                        if *saved == requested =>
                    {
                        Some(None)
                    }
                    (
                        ConversationPayload::DocumentProposed {
                            proposal: DocumentProposal::Spec(spec),
                        },
                        WorkflowDocumentKind::Spec,
                    ) => Some(Some(DocumentProposal::Spec(spec.clone()))),
                    (
                        ConversationPayload::DocumentProposed {
                            proposal: DocumentProposal::Plan(plan),
                        },
                        WorkflowDocumentKind::Plan,
                    ) => Some(Some(DocumentProposal::Plan(plan.clone()))),
                    _ => None,
                })
                .flatten()
        };
        let spec = match proposal(WorkflowDocumentKind::Spec) {
            Some(DocumentProposal::Spec(d)) => d,
            _ => session.spec.clone().unwrap_or(SpecDocument {
                revision: 1,
                summary: String::new(),
                requirements: vec![Criterion {
                    id: "requirement-1".into(),
                    text: String::new(),
                }],
                in_scope: vec![],
                out_of_scope: vec![],
                constraints: vec![],
                acceptance: vec![Criterion {
                    id: "acceptance-1".into(),
                    text: String::new(),
                }],
            }),
        };
        let plan = match proposal(WorkflowDocumentKind::Plan) {
            Some(DocumentProposal::Plan(d)) => Some(d),
            _ => session.plan.clone(),
        };
        self.set_document_fields(spec, plan, window, cx);
    }
    fn set_document_fields(
        &mut self,
        spec: SpecDocument,
        plan: Option<PlanDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut fields = vec![
            field("summary", "Summary", spec.summary.clone(), window, cx),
            field(
                "requirements",
                "Requirements · stable-id | text",
                criterion_text(&spec.requirements),
                window,
                cx,
            ),
            field(
                "in_scope",
                "In scope · one item per line",
                spec.in_scope.join("\n"),
                window,
                cx,
            ),
            field(
                "out_of_scope",
                "Out of scope",
                spec.out_of_scope.join("\n"),
                window,
                cx,
            ),
            field(
                "spec_constraints",
                "Spec constraints",
                spec.constraints.join("\n"),
                window,
                cx,
            ),
            field(
                "acceptance",
                "Acceptance · stable-id | text",
                criterion_text(&spec.acceptance),
                window,
                cx,
            ),
        ];
        if let Some(plan) = &plan {
            for (key, label, value) in [
                ("goal", "Goal", plan.task.goal.clone()),
                ("analysis", "Analysis", plan.task.analysis.clone()),
                (
                    "plan_acceptance",
                    "Plan acceptance",
                    plan.task.acceptance.join("\n"),
                ),
                (
                    "plan_constraints",
                    "Plan constraints",
                    plan.task.constraints.join("\n"),
                ),
                ("unknowns", "Unknowns", plan.task.unknowns.join("\n")),
            ] {
                fields.push(field(key, label, value, window, cx));
            }
            for (i, step) in plan.task.steps.iter().enumerate() {
                fields.push(field(
                    format!("step.{i}.title"),
                    format!("Step {} title · {}", i + 1, step.id),
                    step.title.clone(),
                    window,
                    cx,
                ));
                for (key, label, value) in [
                    ("why", "Why", step.why.clone()),
                    ("files", "Files · one path per line", step.files.join("\n")),
                    ("symbols", "Symbols", step.symbols.join("\n")),
                    ("languages", "Languages", step.languages.join("\n")),
                    (
                        "dependencies",
                        "Dependencies · stable step IDs, one per line",
                        step.depends_on.join("\n"),
                    ),
                    ("done", "Done conditions", step.done_when.join("\n")),
                ] {
                    fields.push(field(format!("step.{i}.{key}"), label, value, window, cx));
                }
            }
        }
        self.companion.documents = Some(DocumentFields { spec, plan, fields });
    }
    pub(super) fn document_command(
        &mut self,
        kind: WorkflowDocumentKind,
        approve: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = (|| -> Result<()> {
            let session = self
                .companion
                .session
                .as_ref()
                .context("Open a conversation")?;
            let fields = self
                .companion
                .documents
                .as_ref()
                .context("Open the document")?;
            let command = if approve {
                let displayed = match kind {
                    WorkflowDocumentKind::Spec => fields.spec.document_ref(),
                    WorkflowDocumentKind::Plan => fields
                        .plan
                        .as_ref()
                        .context("Save a plan first")?
                        .document_ref(),
                };
                let saved = match kind {
                    WorkflowDocumentKind::Spec => {
                        session.spec.as_ref().map(SpecDocument::document_ref)
                    }
                    WorkflowDocumentKind::Plan => {
                        session.plan.as_ref().map(PlanDocument::document_ref)
                    }
                };
                approval_matches(
                    fields.dirty(kind, session, cx),
                    Some(&displayed),
                    saved.as_ref(),
                )?;
                match kind {
                    WorkflowDocumentKind::Spec => SessionCommand::ApproveSpec {
                        document_ref: displayed,
                        approved_at_ms: now_ms(),
                    },
                    WorkflowDocumentKind::Plan => SessionCommand::ApprovePlan {
                        document_ref: displayed,
                        approved_at_ms: now_ms(),
                    },
                }
            } else {
                match kind {
                    WorkflowDocumentKind::Spec => SessionCommand::SaveSpec {
                        spec: fields.read_spec(cx)?,
                    },
                    WorkflowDocumentKind::Plan => SessionCommand::SavePlan {
                        plan: fields.read_plan(cx)?,
                    },
                }
            };
            self.store.lock().unwrap().apply_session_transition(
                &session.id,
                session.sequence,
                &SessionTransition::new(new_id(), vec![command]),
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.reload_conversation(true, window, cx);
                self.companion.solution = None;
                self.companion.status = if approve {
                    "The displayed saved version is approved."
                } else {
                    "Document saved. Changed definitions reopen the affected approvals."
                }
                .into();
            }
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn start_implementation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = (|| -> Result<()> {
            let session = self
                .companion
                .session
                .as_ref()
                .context("Open a conversation")?;
            let fields = self
                .companion
                .documents
                .as_ref()
                .context("Inspect the saved documents first")?;
            anyhow::ensure!(
                !fields.dirty(WorkflowDocumentKind::Spec, session, cx)
                    && !fields.dirty(WorkflowDocumentKind::Plan, session, cx),
                "Save document edits before starting implementation"
            );
            self.store.lock().unwrap().apply_session_transition(
                &session.id,
                session.sequence,
                &SessionTransition::new(new_id(), vec![SessionCommand::StartImplementation]),
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.reload_conversation(false, window, cx);
                self.companion.tab = CompanionTab::Conversation;
                self.start_conversation(ConversationIntent::PrepareWalkthrough, window, cx);
            }
            Err(e) => {
                self.companion.status = e.to_string();
                cx.notify();
            }
        }
    }
    pub(super) fn blank_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> Result<_> {
            let session = self
                .companion
                .session
                .as_ref()
                .context("Open a conversation")?;
            let spec = session.spec.clone().context("Save a spec first")?;
            let mut task = TaskPlan::new(
                session.project.clone(),
                spec.summary.clone(),
                spec.acceptance.iter().map(|c| c.text.clone()).collect(),
                vec!["Describe the first step".into()],
            )?;
            if let Some(link) = &session.legacy_task {
                task.id = link.task_id.clone();
            }
            Ok((
                spec.clone(),
                PlanDocument {
                    revision: 1,
                    spec_ref: spec.document_ref(),
                    task,
                },
            ))
        })();
        match result {
            Ok((spec, plan)) => self.set_document_fields(spec, Some(plan), window, cx),
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn add_plan_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.companion.busy || self.agent.busy {
            return;
        }
        let result = (|| -> Result<_> {
            let fields = self.companion.documents.as_ref().context("Open a plan")?;
            let spec = fields.read_spec(cx)?;
            let mut plan = fields.read_plan(cx)?;
            anyhow::ensure!(plan.task.steps.len() < 64, "Plans allow at most 64 steps");
            let mut extra = TaskPlan::new(
                plan.task.project.clone(),
                plan.task.goal.clone(),
                plan.task.acceptance.clone(),
                vec!["Describe the next step".into()],
            )?
            .steps
            .remove(0);
            extra.depends_on = plan
                .task
                .steps
                .last()
                .map(|s| vec![s.id.clone()])
                .unwrap_or_default();
            plan.task.steps.push(extra);
            Ok((spec, plan))
        })();
        match result {
            Ok((spec, plan)) => self.set_document_fields(spec, Some(plan), window, cx),
            Err(e) => self.companion.status = e.to_string(),
        }
        cx.notify();
    }
    pub(super) fn documents_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = if self.companion.tab == CompanionTab::Spec {
            WorkflowDocumentKind::Spec
        } else {
            WorkflowDocumentKind::Plan
        };
        let mut body = panel_body(cx);
        let Some(session) = &self.companion.session else {
            return body
                .child(note("Continue the legacy task in a conversation first."))
                .into_any_element();
        };
        let Some(fields) = &self.companion.documents else {
            return body.child(note("Document unavailable")).into_any_element();
        };
        let dirty = fields.dirty(kind, session, cx);
        let busy = self.companion.busy || self.agent.busy;
        let (saved, approval) = match kind {
            WorkflowDocumentKind::Spec => (
                session.spec.as_ref().map(SpecDocument::document_ref),
                &session.spec_approval,
            ),
            WorkflowDocumentKind::Plan => (
                session.plan.as_ref().map(PlanDocument::document_ref),
                &session.plan_approval,
            ),
        };
        body = body
            .child(panel_heading(note(format!("{kind:?}"))))
            .child(panel_metadata(
                note(format!(
                    "{} · {} · {}",
                    saved
                        .as_ref()
                        .map(|r| format!("Saved version {}", r.revision))
                        .unwrap_or("No saved version".into()),
                    if dirty {
                        "Unsaved edits / proposal"
                    } else {
                        "Saved"
                    },
                    if approval
                        .as_ref()
                        .is_some_and(|a| Some(&a.document_ref) == saved.as_ref())
                    {
                        "Approved"
                    } else {
                        "Needs approval"
                    }
                )),
                cx,
            ));
        if kind == WorkflowDocumentKind::Plan {
            if let Some(plan) = &fields.plan {
                body = body.child(panel_metadata(
                    note(format!("Based on spec version {}", plan.spec_ref.revision)),
                    cx,
                ));
                if session
                    .spec
                    .as_ref()
                    .is_none_or(|s| s.document_ref() != plan.spec_ref)
                {
                    body=body.child(panel_warning(note("Outdated: this plan uses an older spec. Draft a replacement plan for the current saved spec."),cx));
                }
            } else {
                body = body.child(
                    path_button(Button::new("blank-plan"), "Create an editable plan")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, w, cx| this.blank_plan(w, cx))),
                );
            }
        }
        body = body.children(
            fields
                .fields
                .iter()
                .filter(|f| {
                    if kind == WorkflowDocumentKind::Spec {
                        [
                            "summary",
                            "requirements",
                            "in_scope",
                            "out_of_scope",
                            "spec_constraints",
                            "acceptance",
                        ]
                        .contains(&f.key.as_str())
                    } else {
                        ![
                            "summary",
                            "requirements",
                            "in_scope",
                            "out_of_scope",
                            "spec_constraints",
                            "acceptance",
                        ]
                        .contains(&f.key.as_str())
                    }
                })
                .map(|f| {
                    solid_section(panel_body(cx), cx)
                        .p_2()
                        .child(note(f.label.clone()))
                        .child(Textarea::new(&f.input).aria_label(f.label.clone()))
                }),
        );
        body.when(
            kind == WorkflowDocumentKind::Plan && fields.plan.is_some(),
            |d| {
                d.child(
                    path_button(Button::new("add-plan-step"), "Add step")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, w, cx| this.add_plan_step(w, cx))),
                )
            },
        )
        .child(
            path_button(
                Button::new("save-workflow-document"),
                format!("Save {kind:?}"),
            )
            .disabled(busy)
            .on_click(cx.listener(move |this, _, w, cx| this.document_command(kind, false, w, cx))),
        )
        .child(
            path_button(
                Button::new("approve-workflow-document"),
                format!("Approve {kind:?}"),
            )
            .disabled(busy || dirty || saved.is_none())
            .on_click(cx.listener(move |this, _, w, cx| this.document_command(kind, true, w, cx))),
        )
        .child(
            path_button(Button::new("start-implementation"), "Start Implementation")
                .primary()
                .disabled(busy || session.started_generation == Some(session.execution_generation))
                .on_click(cx.listener(|this, _, w, cx| this.start_implementation(w, cx))),
        )
        .into_any_element()
    }
}
