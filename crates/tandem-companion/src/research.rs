//! Bounded private research. Captures and receipts are data, never workflow grants.
use crate::credential_like;
use anyhow::{Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use tandem_checks::*;
use tandem_core::*;

struct ResearchFile {
    text: String,
    hash: String,
}
/// The complete eligible capture is available for expansion, without recapture.
/// File contents are owned and cannot change underneath a later broker round.
pub struct ResearchSnapshot {
    project: Project,
    snapshot: MaterializedSnapshot,
    files: BTreeMap<String, ResearchFile>,
    git: Option<GitSnapshot>,
    git_limitation: Option<String>,
    excluded_paths: BTreeSet<String>,
}
impl ResearchSnapshot {
    pub fn capture(
        project: &Project,
        overlays: &[Document],
        cache: &Path,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let directory = cache.join(new_id());
        let mut snapshot = materialize(project, overlays, &directory, cancel)?;
        let captured = Project::open(&snapshot.input)?;
        let mut files = BTreeMap::new();
        let mut eligible = Vec::new();
        let mut excluded_paths = BTreeSet::new();
        // Materialization already applied the sealed hierarchical ignore policy
        // to disk files and overlays. Copied (possibly unsaved) ignore files are
        // research data and cannot reclassify those decisions.
        for file in &snapshot.manifest.files {
            ensure!(
                !cancel.load(Ordering::Relaxed),
                "Research capture cancelled"
            );
            let (bytes, _) = secure_read(&captured, &file.path)?;
            let text = String::from_utf8(bytes).ok();
            if safe_path_shape(&file.path)
                && text
                    .as_ref()
                    .is_some_and(|s| !s.contains('\0') && !credential_like(s))
            {
                files.insert(
                    file.path.clone(),
                    ResearchFile {
                        text: text.unwrap(),
                        hash: file.hash.clone(),
                    },
                );
                eligible.push(file.clone());
            } else {
                // Commands get the same filtered text capture, never excluded bytes.
                excluded_paths.insert(file.path.clone());
                std::fs::remove_file(snapshot.input.join(&file.path))?;
            }
        }
        snapshot.manifest.files = eligible;
        let (git, git_limitation) = match capture_git(project, &directory.join("git"), cancel) {
            Ok(Some(git)) => (Some(git), None),
            Ok(None) => (None, Some("Folder has no Git history".into())),
            Err(_) => (
                None,
                Some("Git metadata could not be captured safely within its bounds".into()),
            ),
        };
        if let Some(git) = &git {
            snapshot.manifest.id =
                content_hash(format!("{}:{}", snapshot.manifest.id, git.identity()).as_bytes());
        }
        Ok(Self {
            project: project.clone(),
            snapshot,
            files,
            git,
            git_limitation,
            excluded_paths,
        })
    }
    pub fn id(&self) -> &str {
        &self.snapshot.manifest.id
    }
    pub fn unchanged(&self) -> bool {
        snapshot_unchanged(&self.snapshot) && self.git.as_ref().is_none_or(GitSnapshot::unchanged)
    }
    fn allowed_history_path(&self, path: &str) -> bool {
        if !safe_path_shape(path) || !self.snapshot.eligible_path(path) {
            return false;
        }
        if self
            .excluded_paths
            .iter()
            .any(|p| Path::new(path).starts_with(p))
        {
            return false;
        }
        // eligible_path uses only the sealed materializer policy and known
        // directories, even for deleted leaves or withheld ignore files. Keep
        // independent link/read exclusions closed to historical expansion too.
        !self
            .snapshot
            .manifest
            .omitted
            .iter()
            .any(|o| Path::new(path).starts_with(&o.path))
    }
}
fn safe_path_shape(path: &str) -> bool {
    normalized_relative(Path::new(path)).is_ok()
        && excluded(path).is_none()
        && !path.chars().any(char::is_control)
        && !credential_like(path)
}

pub struct ResearchContext<'a> {
    capture: &'a ResearchSnapshot,
    session_id: String,
    operation_id: String,
    environment: Option<&'a InspectionEnvironment>,
    events: &'a [ConversationEvent],
    budget: ResearchBudget,
    requests: BTreeSet<String>,
}
impl<'a> ResearchContext<'a> {
    pub fn new(
        capture: &'a ResearchSnapshot,
        session_id: &str,
        operation_id: &str,
        environment: Option<&'a InspectionEnvironment>,
        events: &'a [ConversationEvent],
    ) -> Result<Self> {
        for id in [session_id, operation_id] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')),
                "Invalid research operation identity"
            );
        }
        Ok(Self {
            capture,
            session_id: session_id.into(),
            operation_id: operation_id.into(),
            environment,
            events,
            budget: ResearchBudget::default(),
            requests: BTreeSet::new(),
        })
    }
    pub fn begin_round(&mut self, requests: u8) -> Result<()> {
        self.budget.begin_round(requests)
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn budget(&self) -> &ResearchBudget {
        &self.budget
    }
}

#[derive(Clone, Debug)]
pub enum ResearchProgress {
    Started {
        session_id: String,
        operation_id: String,
        snapshot_id: String,
        request: ResearchRequest,
    },
    CommandStarted {
        request_id: String,
        command: ResearchCommand,
    },
    /// Persist the full receipt (including sources/actual command) atomically in the caller.
    Completed {
        session_id: String,
        operation_id: String,
        receipt: Box<ResearchReceipt>,
    },
}

pub struct ResearchBroker<E = RuntimeInspectionExecutor> {
    executor: E,
}
impl Default for ResearchBroker {
    fn default() -> Self {
        Self {
            executor: RuntimeInspectionExecutor,
        }
    }
}
impl<E: InspectionExecutor> ResearchBroker<E> {
    pub fn with_executor(executor: E) -> Self {
        Self { executor }
    }
    pub fn executor(&self) -> &E {
        &self.executor
    }
    pub fn execute(
        &mut self,
        request: ResearchRequest,
        context: &mut ResearchContext<'_>,
        cancel: &AtomicBool,
        mut observe: impl FnMut(&ResearchProgress) -> Result<()>,
    ) -> Result<ResearchReceipt> {
        request.validate()?;
        ensure!(
            context.budget.receipts < context.budget.requests
                && context.budget.returned_bytes < MAX_RESEARCH_RETURNED_BYTES,
            "Research budget exhausted; offer Continue"
        );
        ensure!(
            context.requests.insert(request.id.clone()),
            "Research request was already dispatched"
        );
        observe(&ResearchProgress::Started {
            session_id: context.session_id.clone(),
            operation_id: context.operation_id.clone(),
            snapshot_id: context.capture.id().into(),
            request: request.clone(),
        })?;
        let allowance = MAX_RESEARCH_RECEIPT_BYTES
            .min(MAX_RESEARCH_RETURNED_BYTES - context.budget.returned_bytes);
        let mut result = ResearchResult::default();
        if cancel.load(Ordering::Relaxed) {
            result.status = ResearchReceiptStatus::Cancelled;
        } else if !context.capture.unchanged() {
            result.unavailable("Captured inputs changed; capture a new research snapshot");
        } else {
            self.resolve(&request, context, cancel, &mut observe, &mut result)?;
        }
        if cancel.load(Ordering::Relaxed) && result.status == ResearchReceiptStatus::Complete {
            result.status = ResearchReceiptStatus::Cancelled;
        }
        if result.content.len() > allowance {
            truncate_utf8(&mut result.content, allowance);
            result.omit(
                "Returned text reached its byte budget; Continue can request a smaller range",
            );
        }
        if result.sources.len() > 64 {
            result.sources.truncate(64);
            result.omit("Source metadata reached its entry budget");
        }
        // Large path names cannot bypass the independent metadata budget.
        while serde_json::to_vec(&(&result.sources, &result.omissions))?.len() > 16 * 1024 {
            if result.sources.pop().is_none() {
                break;
            }
            result.omit("Source metadata reached its byte budget");
        }
        let mut receipt = ResearchReceipt::new(
            request,
            result.sources,
            result.status,
            result.content,
            result.omissions,
        )?;
        receipt.snapshot_id = Some(context.capture.id().into());
        receipt.command = result.command;
        receipt.validate()?;
        context.budget.record_receipt(&receipt)?;
        observe(&ResearchProgress::Completed {
            session_id: context.session_id.clone(),
            operation_id: context.operation_id.clone(),
            receipt: Box::new(receipt.clone()),
        })?;
        Ok(receipt)
    }
    fn resolve(
        &mut self,
        request: &ResearchRequest,
        context: &ResearchContext<'_>,
        cancel: &AtomicBool,
        observe: &mut impl FnMut(&ResearchProgress) -> Result<()>,
        result: &mut ResearchResult,
    ) -> Result<()> {
        let capture = context.capture;
        match &request.kind {
            ResearchRequestKind::ListFiles { prefix, limit } => {
                let paths = capture.files.keys().filter(|p| {
                    prefix
                        .as_ref()
                        .is_none_or(|prefix| *p == prefix || p.starts_with(&format!("{prefix}/")))
                });
                for (index, path) in paths.enumerate() {
                    if index >= *limit as usize {
                        result.omit("File listing reached its result limit");
                        break;
                    }
                    result.content.push_str(path);
                    result.content.push('\n');
                }
                result.source(
                    SourceLocator::FileListing {
                        prefix: prefix.clone(),
                    },
                    content_hash(result.content.as_bytes()),
                    ResearchProvenance::ProjectFile,
                );
            }
            ResearchRequestKind::ReadFileRange {
                path,
                start_line,
                end_line,
            } => {
                let Some(file) = capture.files.get(path) else {
                    result.unavailable("File is missing, ignored or excluded from this capture");
                    return Ok(());
                };
                let lines = file
                    .text
                    .split_inclusive('\n')
                    .enumerate()
                    .filter(|(i, _)| *i + 1 >= *start_line as usize && *i < *end_line as usize)
                    .collect::<Vec<_>>();
                if lines.is_empty() {
                    result.unavailable("Requested range is outside the captured file");
                    return Ok(());
                }
                result.content = lines.iter().map(|(_, line)| *line).collect();
                result.source(
                    SourceLocator::FileRange {
                        path: path.clone(),
                        start_line: *start_line,
                        end_line: lines.last().unwrap().0 as u32 + 1,
                    },
                    file.hash.clone(),
                    ResearchProvenance::ProjectFile,
                );
            }
            ResearchRequestKind::SearchText {
                query,
                paths,
                limit,
            } => {
                'files: for (path, file) in &capture.files {
                    if !paths.is_empty() && !paths.contains(path) {
                        continue;
                    }
                    for (index, line) in file.text.lines().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            result.status = ResearchReceiptStatus::Cancelled;
                            break 'files;
                        }
                        if !line.contains(query) {
                            continue;
                        }
                        if result.sources.len() >= (*limit as usize).min(64) {
                            result.omit("Search reached its result/source limit");
                            break 'files;
                        }
                        if result
                            .content
                            .len()
                            .saturating_add(line.len() + path.len() + 16)
                            > MAX_RESEARCH_RECEIPT_BYTES
                        {
                            result.omit("Search reached its byte limit; request a smaller range");
                            break 'files;
                        }
                        result
                            .content
                            .push_str(&format!("{path}:{}:{line}\n", index + 1));
                        result.source(
                            SourceLocator::FileRange {
                                path: path.clone(),
                                start_line: index as u32 + 1,
                                end_line: index as u32 + 1,
                            },
                            file.hash.clone(),
                            ResearchProvenance::ProjectFile,
                        );
                    }
                }
                if paths.iter().any(|p| !capture.files.contains_key(p)) {
                    result.omit("One or more search paths are missing, ignored or excluded");
                }
            }
            ResearchRequestKind::DependencyMetadata { ecosystem } => {
                dependency_metadata(capture, *ecosystem, result)?
            }
            ResearchRequestKind::ConversationEvents {
                before_sequence,
                limit,
            } => {
                let mut events = context
                    .events
                    .iter()
                    .filter(|e| {
                        e.session_id == context.session_id
                            && before_sequence.is_none_or(|s| e.sequence < s)
                    })
                    .collect::<Vec<_>>();
                events.sort_by_key(|e| e.sequence);
                if events.len() > *limit as usize {
                    events.drain(..events.len() - *limit as usize);
                    result.omit("Conversation reached its event limit");
                }
                let mut sequences = Vec::new();
                for event in events {
                    // Documents/action artifacts are separate channels, never transcript expansion.
                    let value = match &event.payload {
                        ConversationPayload::HumanMessage { text }
                        | ConversationPayload::AssistantMessage { text } => text.clone(),
                        ConversationPayload::QuestionAsked { question } => {
                            serde_json::to_string(question)?
                        }
                        ConversationPayload::QuestionAnswered { answer } => {
                            serde_json::to_string(answer)?
                        }
                        ConversationPayload::ResearchReceiptRecorded { .. }
                        | ConversationPayload::ResearchFindings { .. } => {
                            serde_json::to_string(&event.payload)?
                        }
                        _ => {
                            result.omit("Non-discussion artifacts are available through their separate channels");
                            continue;
                        }
                    };
                    if credential_like(&value) {
                        result.omit("Credential-like conversation content was withheld");
                        continue;
                    }
                    result
                        .content
                        .push_str(&format!("{}: {value}\n", event.sequence));
                    sequences.push(event.sequence);
                }
                if let (Some(first), Some(last)) = (sequences.first(), sequences.last()) {
                    result.source(
                        SourceLocator::Conversation {
                            session_id: context.session_id.clone(),
                            first_sequence: *first,
                            last_sequence: *last,
                        },
                        content_hash(result.content.as_bytes()),
                        ResearchProvenance::Conversation,
                    );
                }
            }
            ResearchRequestKind::ResearchReceipts { .. } => {
                result.unavailable("Receipt bodies require the session-scoped host store")
            }
            ResearchRequestKind::PublicSearch { .. } | ResearchRequestKind::PublicPage { .. } => {
                result.unavailable("Public research requires the separate public-only worker")
            }
            _ => {
                let history_paths: Vec<&str> = match &request.kind {
                    ResearchRequestKind::GitHistory { path, .. }
                    | ResearchRequestKind::GitShow { path, .. } => {
                        path.iter().map(String::as_str).collect()
                    }
                    ResearchRequestKind::GitDiff { paths, .. } => {
                        paths.iter().map(String::as_str).collect()
                    }
                    _ => vec![],
                };
                if history_paths
                    .iter()
                    .any(|p| !capture.allowed_history_path(p))
                {
                    result.unavailable("Historical path or its ignore policy is unavailable, ignored or excluded from this capture");
                    return Ok(());
                }
                if matches!(
                    request.kind,
                    ResearchRequestKind::GitHistory { .. }
                        | ResearchRequestKind::GitShow { .. }
                        | ResearchRequestKind::GitDiff { .. }
                        | ResearchRequestKind::Inspect {
                            recipe: InspectionRecipe::GitStatus
                        }
                ) && capture.git.is_none()
                {
                    result.unavailable(
                        capture
                            .git_limitation
                            .as_deref()
                            .unwrap_or("Git unavailable"),
                    );
                    return Ok(());
                }
                let Some(environment) = context.environment else {
                    result.unavailable("Configured inspection tools are unavailable");
                    return Ok(());
                };
                let command = match prepare_inspection(
                    environment,
                    &capture.project,
                    &capture.snapshot,
                    capture.git.as_ref(),
                    request,
                ) {
                    Ok(command) => command,
                    Err(error) => {
                        let detail = format!("Fixed inspection unavailable: {error}");
                        result.unavailable(if credential_like(&detail) {
                            "Fixed inspection unavailable; sensitive error details were withheld"
                        } else {
                            &detail
                        });
                        return Ok(());
                    }
                };
                let mut leaf_outputs = Vec::new();
                for check in command.git_leaf_inspections() {
                    let Some(output) =
                        self.run_inspection(&check, request, context, cancel, observe, result)
                    else {
                        return Ok(());
                    };
                    leaf_outputs.push(output);
                }
                if command.validate_git_leaves(&leaf_outputs).is_err() {
                    result.unavailable("History requires existing literal regular file leaves; directories, links and incomplete validation are unavailable");
                    return Ok(());
                }
                if let Some(output) =
                    self.run_inspection(&command, request, context, cancel, observe, result)
                {
                    let text = String::from_utf8(output.output).ok();
                    let git_status = matches!(
                        request.kind,
                        ResearchRequestKind::Inspect {
                            recipe: InspectionRecipe::GitStatus
                        }
                    );
                    if let Some(text) =
                        text.filter(|s| (git_status || !s.contains('\0')) && !credential_like(s))
                    {
                        if git_status {
                            // Porcelain -z uses exact, unquoted file paths. Only
                            // currently captured eligible leaves can be exposed.
                            for line in text.split_inclusive('\0') {
                                if line.ends_with('\0')
                                    && line
                                        .get(3..line.len() - 1)
                                        .is_some_and(|path| capture.files.contains_key(path))
                                {
                                    result.content.push_str(&line[..line.len() - 1]);
                                    result.content.push('\n');
                                } else {
                                    result.omit("Excluded Git status paths were withheld");
                                }
                            }
                        } else {
                            result.content = text;
                        }
                    } else {
                        result.omit("Binary or credential-like inspection output was withheld");
                    }
                    if output.truncated {
                        result.omit("Inspection output reached its byte limit");
                    }
                    if !capture.unchanged() {
                        result.content.clear();
                        result.unavailable("Inspection changed captured inputs; result withheld");
                    }
                    let (locator, provenance) = match &request.kind {
                        ResearchRequestKind::GitHistory { path, .. } => (
                            SourceLocator::GitHistory { path: path.clone() },
                            ResearchProvenance::Git,
                        ),
                        ResearchRequestKind::GitShow { commit, path } => (
                            SourceLocator::Git {
                                commit: commit.clone(),
                                path: path.clone(),
                            },
                            ResearchProvenance::Git,
                        ),
                        ResearchRequestKind::GitDiff { base, head, paths } => (
                            SourceLocator::GitDiff {
                                base: base.clone(),
                                head: head.clone(),
                                paths: paths.clone(),
                            },
                            ResearchProvenance::Git,
                        ),
                        ResearchRequestKind::Inspect { recipe } => (
                            SourceLocator::Inspection { recipe: *recipe },
                            ResearchProvenance::FixedInspection,
                        ),
                        _ => unreachable!(),
                    };
                    if matches!(
                        result.status,
                        ResearchReceiptStatus::Complete | ResearchReceiptStatus::Partial
                    ) {
                        result.source(locator, content_hash(result.content.as_bytes()), provenance);
                    }
                }
            }
        }
        Ok(())
    }
    fn run_inspection(
        &mut self,
        command: &PreparedInspection,
        request: &ResearchRequest,
        context: &ResearchContext<'_>,
        cancel: &AtomicBool,
        observe: &mut impl FnMut(&ResearchProgress) -> Result<()>,
        result: &mut ResearchResult,
    ) -> Option<InspectionOutput> {
        if cancel.load(Ordering::Relaxed) {
            result.status = ResearchReceiptStatus::Cancelled;
            return None;
        }
        if !context.capture.unchanged() {
            result.unavailable("Captured inputs changed; capture a new research snapshot");
            return None;
        }
        result.command = None;
        let output = self.executor.execute(command, cancel, &mut |command| {
            result.command = Some(command.clone());
            observe(&ResearchProgress::CommandStarted {
                request_id: request.id.clone(),
                command: command.clone(),
            })
        });
        let output = match output {
            Ok(output) => output,
            Err(_) => {
                if result.command.is_some() {
                    result.status = ResearchReceiptStatus::Interrupted;
                    result.omit(
                        "Inspection terminal result is unknown; no check success was recorded",
                    );
                } else {
                    result.unavailable("Inspection could not start with the configured runtime");
                }
                return None;
            }
        };
        if let Some(command) = &mut result.command {
            command.exit_code = output.exit_code;
            command.output_truncated = output.truncated;
            command.cancelled = output.cancelled;
            command.timed_out = output.timed_out;
        }
        if output.cancelled {
            result.status = ResearchReceiptStatus::Cancelled;
        } else if output.timed_out {
            result.unavailable("Inspection timed out after 15 seconds");
        } else if output.exit_code != Some(0) {
            result.unavailable("Inspection did not complete successfully; tooling or offline metadata may be unavailable");
        } else {
            return Some(output);
        }
        // Failed Git commands can echo unvalidated paths/objects in diagnostics.
        None
    }
}
struct ResearchResult {
    content: String,
    sources: Vec<ResearchSource>,
    status: ResearchReceiptStatus,
    omissions: Vec<ResearchOmission>,
    command: Option<ResearchCommand>,
}
impl Default for ResearchResult {
    fn default() -> Self {
        Self {
            content: String::new(),
            sources: vec![],
            status: ResearchReceiptStatus::Complete,
            omissions: vec![],
            command: None,
        }
    }
}
impl ResearchResult {
    fn source(&mut self, locator: SourceLocator, hash: String, provenance: ResearchProvenance) {
        self.sources.push(ResearchSource {
            id: new_id(),
            locator,
            source_hash: hash,
            captured_at_ms: now_ms(),
            provenance,
        });
    }
    fn omit(&mut self, reason: &str) {
        if self.status == ResearchReceiptStatus::Complete {
            self.status = ResearchReceiptStatus::Partial;
        }
        if self.omissions.len() < 64 && !self.omissions.iter().any(|o| o.reason == reason) {
            self.omissions.push(ResearchOmission {
                locator: None,
                reason: reason.into(),
            });
        }
    }
    fn unavailable(&mut self, reason: &str) {
        self.status = ResearchReceiptStatus::Unavailable;
        self.omit(reason);
    }
}
fn truncate_utf8(text: &mut String, max: usize) {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1
    }
    text.truncate(end);
}

fn dependency_metadata(
    capture: &ResearchSnapshot,
    ecosystem: DependencyEcosystem,
    result: &mut ResearchResult,
) -> Result<()> {
    let mut found = false;
    for (path, file) in &capture.files {
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let toml_file = match ecosystem {
            DependencyEcosystem::Rust => matches!(name, "Cargo.toml" | "Cargo.lock"),
            DependencyEcosystem::Python => {
                matches!(name, "pyproject.toml" | "poetry.lock" | "uv.lock")
            }
        };
        let requirements = ecosystem == DependencyEcosystem::Python
            && (name == "requirements.txt"
                || (name.starts_with("requirements-") && name.ends_with(".txt")));
        if !toml_file && !requirements {
            continue;
        }
        found = true;
        let mut rows = Vec::new();
        if requirements {
            for line in file
                .text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
            {
                if line.starts_with('-') {
                    result.omit("Requirements directives are data only and were not followed");
                    continue;
                }
                rows.push(
                    serde_json::json!({"requirement":line,"source":"requirements declaration"}),
                );
            }
        } else {
            match toml::from_str::<toml::Value>(&file.text) {
                Ok(value) => collect_dependencies(&value, &mut rows, ecosystem, None),
                Err(_) => {
                    result.omit(
                        "A manifest or lockfile could not be parsed; it remains readable as text",
                    );
                    continue;
                }
            }
        }
        for row in rows {
            result
                .content
                .push_str(&format!("{path}: {}\n", serde_json::to_string(&row)?));
        }
        result.source(
            SourceLocator::FileRange {
                path: path.clone(),
                start_line: 1,
                end_line: file.text.lines().count().max(1) as u32,
            },
            file.hash.clone(),
            ResearchProvenance::DependencyMetadata,
        );
        if result.sources.len() >= 32 {
            result.omit("Dependency metadata reached its source limit");
            break;
        }
    }
    if !found {
        result.unavailable("No captured manifest or lockfile for this ecosystem");
    }
    Ok(())
}
fn collect_dependencies(
    value: &toml::Value,
    rows: &mut Vec<serde_json::Value>,
    ecosystem: DependencyEcosystem,
    key: Option<&str>,
) {
    if let Some(table) = value.as_table() {
        if key == Some("package")
            && let Some(name) = table.get("name").and_then(toml::Value::as_str)
        {
            rows.push(serde_json::json!({"name":name,"version":table.get("version"),"source":table.get("source").or_else(||table.get("git")).or_else(||table.get("path")),"provenance":"lockfile or manifest package"}));
        }
        for (name, item) in table {
            if matches!(
                name.as_str(),
                "dependencies" | "dev-dependencies" | "build-dependencies"
            ) {
                if let Some(deps) = item.as_table() {
                    for (name, spec) in deps {
                        rows.push(serde_json::json!({"name":spec.get("package").and_then(toml::Value::as_str).unwrap_or(name),"version_requirement":spec.as_str().map(str::to_string).or_else(||spec.get("version").and_then(toml::Value::as_str).map(str::to_string)),"declaration":spec,"source":spec.get("git").or_else(||spec.get("path")).or_else(||spec.get("registry")).or_else(||spec.get("source")),"provenance":"manifest dependency"}));
                    }
                } else if let Some(deps) = item.as_array() {
                    for spec in deps {
                        rows.push(spec.as_str().map(|text|python_requirement(text,"manifest dependency")).unwrap_or_else(||serde_json::json!({"requirement":spec,"provenance":"lockfile dependency"})));
                    }
                }
            } else if ecosystem == DependencyEcosystem::Python
                && matches!(name.as_str(), "optional-dependencies" | "requires")
            {
                rows.push(serde_json::json!({"declarations":item,"provenance":"optional or build dependencies"}));
            } else {
                collect_dependencies(item, rows, ecosystem, Some(name));
            }
        }
    } else if let Some(items) = value.as_array() {
        for item in items {
            collect_dependencies(item, rows, ecosystem, key);
        }
    }
}

// Preserve constraints/markers instead of pretending a declaration is a resolved version.
fn python_requirement(text: &str, provenance: &str) -> serde_json::Value {
    let text = text.trim();
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(text.len());
    let (name, rest) = text.split_at(end);
    let constraint = rest.trim().split(';').next().unwrap_or_default().trim();
    let source = constraint.split_once('@').map(|(_, url)| url.trim());
    serde_json::json!({"name":name,"version_requirement":constraint,"source":source,"declaration":text,"provenance":provenance})
}
