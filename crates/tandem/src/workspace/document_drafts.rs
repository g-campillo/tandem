//! Editable, non-authoritative document drafts. Raw invalid input is retained.
use anyhow::{Context, Result};
use tandem_core::*;
#[derive(Clone, Debug)]
pub struct DraftField {
    pub key: String,
    pub label: String,
    pub value: String,
}
#[derive(Clone, Debug)]
pub struct DocumentEditorDraft {
    pub spec: SpecDocument,
    pub plan: Option<PlanDocument>,
    pub fields: Vec<DraftField>,
}
fn draft_field(key: impl Into<String>, label: impl Into<String>, value: String) -> DraftField {
    DraftField {
        key: key.into(),
        label: label.into(),
        value,
    }
}
fn latest_proposal(
    events: &[ConversationEvent],
    kind: WorkflowDocumentKind,
) -> Option<DocumentProposal> {
    events
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
}
impl DocumentEditorDraft {
    pub fn new(spec: SpecDocument, plan: Option<PlanDocument>) -> Self {
        let mut fields = vec![
            draft_field("summary", "Summary", spec.summary.clone()),
            draft_field(
                "requirements",
                "Requirements · stable-id | text",
                criterion_text(&spec.requirements),
            ),
            draft_field(
                "in_scope",
                "In scope · one item per line",
                spec.in_scope.join("\n"),
            ),
            draft_field("out_of_scope", "Out of scope", spec.out_of_scope.join("\n")),
            draft_field(
                "spec_constraints",
                "Spec constraints",
                spec.constraints.join("\n"),
            ),
            draft_field(
                "acceptance",
                "Acceptance · stable-id | text",
                criterion_text(&spec.acceptance),
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
                fields.push(draft_field(key, label, value));
            }
            for (i, step) in plan.task.steps.iter().enumerate() {
                fields.push(draft_field(
                    format!("step.{i}.title"),
                    format!("Step {} title · {}", i + 1, step.id),
                    step.title.clone(),
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
                    fields.push(draft_field(format!("step.{i}.{key}"), label, value));
                }
            }
        }
        Self { spec, plan, fields }
    }
    pub fn from_session(session: &TaskSession, events: &[ConversationEvent]) -> Self {
        let mut spec = match latest_proposal(events, WorkflowDocumentKind::Spec) {
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
        let mut plan = match latest_proposal(events, WorkflowDocumentKind::Plan) {
            Some(DocumentProposal::Plan(d)) => Some(d),
            _ => session.plan.clone(),
        };
        // A no-op save emits no DocumentSaved event. Equivalent proposals must
        // resolve to the actual saved identity on every reload, including restart.
        if let Some(saved) = &session.spec
            && saved.definition_hash() == spec.definition_hash()
        {
            spec = saved.clone();
        }
        if let Some(saved) = &session.plan
            && plan
                .as_ref()
                .is_some_and(|p| p.definition_hash() == saved.definition_hash())
        {
            plan = Some(saved.clone());
        }
        Self::new(spec, plan)
    }
    fn edited(&self, kind: WorkflowDocumentKind) -> bool {
        let baseline = Self::new(self.spec.clone(), self.plan.clone());
        self.fields
            .iter()
            .filter(|f| field_kind(&f.key) == kind)
            .any(|f| {
                baseline
                    .fields
                    .iter()
                    .find(|b| b.key == f.key)
                    .is_none_or(|b| b.value != f.value)
            })
    }
    pub fn refresh(
        &mut self,
        session: &TaskSession,
        events: &[ConversationEvent],
        saved: Option<WorkflowDocumentKind>,
    ) {
        let incoming = Self::from_session(session, events);
        for kind in [WorkflowDocumentKind::Spec, WorkflowDocumentKind::Plan] {
            let populated = match kind {
                WorkflowDocumentKind::Spec => {
                    session.spec.is_some() || self.spec.validate().is_ok()
                }
                WorkflowDocumentKind::Plan => self.plan.is_some(),
            };
            // Unsaved proposal content is still pristine until its raw fields
            // change. A newer explicit proposal can replace that baseline.
            if saved == Some(kind)
                || (!self.edited(kind)
                    && (latest_proposal(events, kind).is_some()
                        || !(populated && self.dirty(kind, session))))
            {
                self.fields.retain(|f| field_kind(&f.key) != kind);
                self.fields.extend(
                    incoming
                        .fields
                        .iter()
                        .filter(|f| field_kind(&f.key) == kind)
                        .cloned(),
                );
                match kind {
                    WorkflowDocumentKind::Spec => self.spec = incoming.spec.clone(),
                    WorkflowDocumentKind::Plan => self.plan = incoming.plan.clone(),
                }
            }
        }
    }
    fn text(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    }
    pub fn read_spec(&self) -> Result<SpecDocument> {
        let mut spec = self.spec.clone();
        spec.summary = self.text("summary");
        spec.requirements = criteria(&self.text("requirements"))?;
        spec.acceptance = criteria(&self.text("acceptance"))?;
        spec.in_scope = lines(&self.text("in_scope"));
        spec.out_of_scope = lines(&self.text("out_of_scope"));
        spec.constraints = lines(&self.text("spec_constraints"));
        spec.validate()?;
        Ok(spec)
    }
    pub fn read_plan(&self) -> Result<PlanDocument> {
        let mut plan = self
            .plan
            .clone()
            .context("Save a spec and draft a plan first")?;
        plan.task.goal = self.text("goal");
        plan.task.analysis = self.text("analysis");
        plan.task.acceptance = lines(&self.text("plan_acceptance"));
        plan.task.constraints = lines(&self.text("plan_constraints"));
        plan.task.unknowns = lines(&self.text("unknowns"));
        for (i, step) in plan.task.steps.iter_mut().enumerate() {
            step.title = self.text(&format!("step.{i}.title"));
            step.why = self.text(&format!("step.{i}.why"));
            step.files = lines(&self.text(&format!("step.{i}.files")));
            step.symbols = lines(&self.text(&format!("step.{i}.symbols")));
            step.languages = lines(&self.text(&format!("step.{i}.languages")));
            step.depends_on = lines(&self.text(&format!("step.{i}.dependencies")));
            step.done_when = lines(&self.text(&format!("step.{i}.done")));
        }
        plan.validate()?;
        Ok(plan)
    }
    pub fn dirty(&self, kind: WorkflowDocumentKind, session: &TaskSession) -> bool {
        match kind {
            WorkflowDocumentKind::Spec => self
                .read_spec()
                .ok()
                .zip(session.spec.as_ref())
                .is_none_or(|(d, s)| d.document_ref() != s.document_ref()),
            WorkflowDocumentKind::Plan => self
                .read_plan()
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
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn field_kind(key: &str) -> WorkflowDocumentKind {
    if matches!(
        key,
        "summary"
            | "requirements"
            | "in_scope"
            | "out_of_scope"
            | "spec_constraints"
            | "acceptance"
    ) {
        WorkflowDocumentKind::Spec
    } else {
        WorkflowDocumentKind::Plan
    }
}

/// In-memory raw drafts are independent of saved workflow authority.
#[derive(Default)]
pub struct SessionDocumentDrafts(std::collections::BTreeMap<String, DocumentEditorDraft>);
impl SessionDocumentDrafts {
    pub fn preserve(&mut self, session_id: &str, draft: DocumentEditorDraft) {
        self.0.insert(session_id.into(), draft);
    }
    pub fn restore(
        &mut self,
        session: &TaskSession,
        events: &[ConversationEvent],
    ) -> DocumentEditorDraft {
        let mut draft = self
            .0
            .remove(&session.id)
            .unwrap_or_else(|| DocumentEditorDraft::from_session(session, events));
        draft.refresh(session, events, None);
        draft
    }
}
