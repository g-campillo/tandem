//! Editable, non-authoritative document drafts. Raw invalid input is retained.
use anyhow::{Context, Result, ensure};
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
        let mut fields = vec![draft_field("summary", "Summary", spec.summary.clone())];
        for (key, criteria) in [
            ("requirements", &spec.requirements),
            ("acceptance", &spec.acceptance),
        ] {
            for (index, criterion) in criteria.iter().enumerate() {
                fields.push(draft_field(
                    format!("{key}.{index}.id"),
                    "Stable ID",
                    criterion.id.clone(),
                ));
                fields.push(draft_field(
                    format!("{key}.{index}.text"),
                    "Text",
                    criterion.text.clone(),
                ));
            }
        }
        for (key, entries) in [
            ("in_scope", &spec.in_scope),
            ("out_of_scope", &spec.out_of_scope),
            ("spec_constraints", &spec.constraints),
        ] {
            list_fields(&mut fields, key, entries);
        }
        if let Some(plan) = &plan {
            fields.push(draft_field("goal", "Goal", plan.task.goal.clone()));
            fields.push(draft_field(
                "analysis",
                "Analysis",
                plan.task.analysis.clone(),
            ));
            for (key, entries) in [
                ("plan_acceptance", &plan.task.acceptance),
                ("plan_constraints", &plan.task.constraints),
                ("unknowns", &plan.task.unknowns),
            ] {
                list_fields(&mut fields, key, entries);
            }
            for (i, step) in plan.task.steps.iter().enumerate() {
                fields.push(draft_field(
                    format!("step.{i}.title"),
                    format!("Step {} title · {}", i + 1, step.id),
                    step.title.clone(),
                ));
                fields.push(draft_field(
                    format!("step.{i}.why"),
                    "Why",
                    step.why.clone(),
                ));
                for (key, entries) in [
                    ("files", &step.files),
                    ("symbols", &step.symbols),
                    ("languages", &step.languages),
                    ("dependencies", &step.depends_on),
                    ("done", &step.done_when),
                ] {
                    list_fields(&mut fields, &format!("step.{i}.{key}"), entries);
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
        let values = |draft: &Self| {
            draft
                .fields
                .iter()
                .filter(|f| field_kind(&f.key) == kind)
                .map(|f| (f.key.clone(), f.value.clone()))
                .collect::<Vec<_>>()
        };
        values(self) != values(&baseline)
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
        spec.requirements = self.criteria("requirements")?;
        spec.acceptance = self.criteria("acceptance")?;
        spec.in_scope = self.entries("in_scope");
        spec.out_of_scope = self.entries("out_of_scope");
        spec.constraints = self.entries("spec_constraints");
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
        plan.task.acceptance = self.entries("plan_acceptance");
        plan.task.constraints = self.entries("plan_constraints");
        plan.task.unknowns = self.entries("unknowns");
        for (i, step) in plan.task.steps.iter_mut().enumerate() {
            step.title = self.text(&format!("step.{i}.title"));
            step.why = self.text(&format!("step.{i}.why"));
            step.files = self.entries(&format!("step.{i}.files"));
            step.symbols = self.entries(&format!("step.{i}.symbols"));
            step.languages = self.entries(&format!("step.{i}.languages"));
            step.depends_on = self.entries(&format!("step.{i}.dependencies"));
            step.done_when = self.entries(&format!("step.{i}.done"));
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
fn list_fields(fields: &mut Vec<DraftField>, key: &str, entries: &[String]) {
    for (index, text) in entries.iter().enumerate() {
        fields.push(draft_field(format!("{key}.{index}"), "Text", text.clone()));
    }
}

#[derive(Clone)]
pub struct DraftSection {
    pub key: String,
    pub label: String,
    pub list: bool,
    pub criteria: bool,
}
impl DraftSection {
    pub fn kind(&self) -> WorkflowDocumentKind {
        field_kind(&self.key)
    }
    pub fn contains(&self, key: &str) -> bool {
        key == self.key || (self.list && key.starts_with(&format!("{}.", self.key)))
    }
}
impl DocumentEditorDraft {
    /// Every vector entry has its own input. Text is never split or normalized.
    pub fn sections(&self) -> Vec<DraftSection> {
        let section = |key: &str, label: &str, list, criteria| DraftSection {
            key: key.into(),
            label: label.into(),
            list,
            criteria,
        };
        let mut sections = vec![
            section("summary", "Summary", false, false),
            section("requirements", "Requirements", true, true),
            section("in_scope", "In scope", true, false),
            section("out_of_scope", "Out of scope", true, false),
            section("spec_constraints", "Spec constraints", true, false),
            section("acceptance", "Acceptance criteria", true, true),
        ];
        if let Some(plan) = &self.plan {
            for (key, label, list) in [
                ("goal", "Goal", false),
                ("analysis", "Analysis", false),
                ("plan_acceptance", "Plan acceptance", true),
                ("plan_constraints", "Plan constraints", true),
                ("unknowns", "Unknowns", true),
            ] {
                sections.push(section(key, label, list, false));
            }
            for (i, step) in plan.task.steps.iter().enumerate() {
                sections.push(section(
                    &format!("step.{i}.title"),
                    &format!("Step {} title · {}", i + 1, step.id),
                    false,
                    false,
                ));
                sections.push(section(&format!("step.{i}.why"), "Why", false, false));
                for (key, label) in [
                    ("files", "Files"),
                    ("symbols", "Symbols"),
                    ("languages", "Languages"),
                    ("dependencies", "Dependencies · stable step IDs"),
                    ("done", "Done conditions"),
                ] {
                    sections.push(section(&format!("step.{i}.{key}"), label, true, false));
                }
            }
        }
        sections
    }
    pub fn add_entry(&mut self, key: &str) -> Result<()> {
        let section = self
            .sections()
            .into_iter()
            .find(|s| s.key == key && s.list)
            .context("Unknown document list")?;
        let index = self
            .fields
            .iter()
            .filter_map(|f| entry_index(&f.key, key))
            .max()
            .map_or(0, |i| i + 1);
        if section.criteria {
            self.fields.push(draft_field(
                format!("{key}.{index}.id"),
                "Stable ID",
                String::new(),
            ));
            self.fields.push(draft_field(
                format!("{key}.{index}.text"),
                "Text",
                String::new(),
            ));
        } else {
            self.fields
                .push(draft_field(format!("{key}.{index}"), "Text", String::new()));
        }
        Ok(())
    }
    pub fn remove_entry(&mut self, key: &str, index: usize) -> Result<()> {
        ensure!(
            self.sections().iter().any(|s| s.key == key && s.list),
            "Unknown document list"
        );
        self.fields
            .retain(|f| entry_index(&f.key, key) != Some(index));
        for field in &mut self.fields {
            if let Some(i) = entry_index(&field.key, key)
                && i > index
            {
                let suffix = &field.key[format!("{key}.{i}").len()..];
                field.key = format!("{key}.{}{suffix}", i - 1);
            }
        }
        Ok(())
    }
    fn entries(&self, key: &str) -> Vec<String> {
        let prefix = format!("{key}.");
        self.fields
            .iter()
            .filter(|f| {
                f.key
                    .strip_prefix(&prefix)
                    .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
            })
            .map(|f| f.value.clone())
            .collect()
    }
    fn criteria(&self, key: &str) -> Result<Vec<Criterion>> {
        self.fields
            .iter()
            .filter_map(|f| {
                entry_index(&f.key, key)
                    .filter(|_| f.key.ends_with(".text"))
                    .map(|index| (f, index))
            })
            .map(|(field, index)| {
                let id = self
                    .fields
                    .iter()
                    .find(|f| f.key == format!("{key}.{index}.id"))
                    .context("Criterion needs a stable ID")?;
                Ok(Criterion {
                    id: id.value.clone(),
                    text: field.value.clone(),
                })
            })
            .collect()
    }
}
pub fn entry_index(field: &str, list: &str) -> Option<usize> {
    field
        .strip_prefix(&format!("{list}."))?
        .split('.')
        .next()?
        .parse()
        .ok()
}

pub fn field_kind(key: &str) -> WorkflowDocumentKind {
    if matches!(
        key.split('.').next().unwrap_or(key),
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
