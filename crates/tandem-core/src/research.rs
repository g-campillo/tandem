//! Data-only research contracts. Requests are broker inputs, never tool grants.
use crate::conversation::{bounded_structure, bounded_text, stable_id};
use crate::{content_hash, new_id, normalized_relative, now_ms};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub const MAX_RESEARCH_ROUNDS: u8 = 8;
pub const MAX_RESEARCH_REQUESTS_PER_ROUND: u8 = 4;
pub const MAX_RESEARCH_RECEIPTS: u8 = 32;
pub const MAX_RESEARCH_RECEIPT_BYTES: usize = 64 * 1024;
pub const MAX_RESEARCH_RETURNED_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchRequest {
    /// Minted by the host, not chosen by a model when it cites findings.
    pub id: String,
    pub kind: ResearchRequestKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyEcosystem {
    Rust,
    Python,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectionRecipe {
    GitStatus,
    RustWorkspaceMetadata,
    RustToolchain,
    PythonProjectMetadata,
    PythonToolchain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ResearchRequestKind {
    ListFiles {
        prefix: Option<String>,
        limit: u32,
    },
    ReadFileRange {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    SearchText {
        query: String,
        paths: Vec<String>,
        limit: u32,
    },
    GitHistory {
        path: Option<String>,
        limit: u32,
    },
    GitShow {
        commit: String,
        path: Option<String>,
    },
    GitDiff {
        base: String,
        head: String,
        paths: Vec<String>,
    },
    DependencyMetadata {
        ecosystem: DependencyEcosystem,
    },
    Inspect {
        recipe: InspectionRecipe,
    },
    ConversationEvents {
        before_sequence: Option<u64>,
        limit: u32,
    },
    PublicSearch {
        query: String,
    },
    PublicPage {
        url: String,
    },
}

impl ResearchRequest {
    pub fn new(kind: ResearchRequestKind) -> Result<Self> {
        let request = Self { id: new_id(), kind };
        request.validate()?;
        Ok(request)
    }

    /// Structural bounds only. The broker must still apply path, secret, ignored
    /// file and public-network protections before materializing any request.
    pub fn validate(&self) -> Result<()> {
        stable_id(&self.id)?;
        match &self.kind {
            ResearchRequestKind::ListFiles { prefix, limit } => {
                if let Some(path) = prefix {
                    normalized_relative(Path::new(path))?;
                }
                bounded_limit(*limit)?;
            }
            ResearchRequestKind::ReadFileRange {
                path,
                start_line,
                end_line,
            } => {
                normalized_relative(Path::new(path))?;
                ensure!(
                    *start_line > 0 && end_line >= start_line && end_line - start_line < 2000,
                    "Read a bounded 1-based file range of at most 2000 lines"
                );
            }
            ResearchRequestKind::SearchText {
                query,
                paths,
                limit,
            } => {
                bounded_text(query, 4096, "Research search")?;
                ensure!(paths.len() <= 64, "Search at most 64 explicit paths");
                for path in paths {
                    normalized_relative(Path::new(path))?;
                }
                bounded_limit(*limit)?;
            }
            ResearchRequestKind::GitHistory { path, limit } => {
                if let Some(path) = path {
                    normalized_relative(Path::new(path))?;
                }
                ensure!((1..=50).contains(limit), "Git history permits 1–50 commits");
            }
            ResearchRequestKind::GitShow { commit, path } => {
                ensure!(
                    (7..=64).contains(&commit.len())
                        && commit.bytes().all(|c| c.is_ascii_hexdigit()),
                    "Git show requires an immutable hexadecimal commit ID"
                );
                if let Some(path) = path {
                    normalized_relative(Path::new(path))?;
                }
            }
            ResearchRequestKind::GitDiff { base, head, paths } => {
                for commit in [base, head] {
                    ensure!(
                        (7..=64).contains(&commit.len())
                            && commit.bytes().all(|c| c.is_ascii_hexdigit()),
                        "Git diff requires immutable hexadecimal commit IDs"
                    );
                }
                ensure!(
                    !paths.is_empty() && paths.len() <= 32,
                    "Git diff needs 1–32 explicit paths"
                );
                for path in paths {
                    normalized_relative(Path::new(path))?;
                }
            }
            ResearchRequestKind::ConversationEvents { limit, .. } => bounded_limit(*limit)?,
            ResearchRequestKind::PublicSearch { query } => {
                bounded_text(query, 4096, "Public search")?
            }
            ResearchRequestKind::PublicPage { url } => validate_public_url_shape(url)?,
            ResearchRequestKind::DependencyMetadata { .. }
            | ResearchRequestKind::Inspect { .. } => {}
        }
        bounded_structure(self)
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit())
}

fn bounded_limit(limit: u32) -> Result<()> {
    ensure!(
        (1..=200).contains(&limit),
        "Research result count must be 1–200"
    );
    Ok(())
}

fn validate_public_url_shape(url: &str) -> Result<()> {
    bounded_text(url, 4096, "Public URL")?;
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    ensure!(
        rest.is_some_and(|rest| !rest.is_empty()
            && !rest.starts_with('/')
            && !rest.split('/').next().unwrap_or_default().contains('@'))
            && !url.chars().any(char::is_whitespace),
        "Public pages need an anonymous HTTP(S) URL"
    );
    Ok(())
}

/// A minimal host-created brief, after privacy screening. It intentionally has
/// no transcript, task documents, local paths or repository-content fields.
/// Structural validation is not a substitute for that host privacy screening.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicResearchBrief {
    pub id: String,
    pub topics: Vec<String>,
    pub queries: Vec<String>,
    pub urls: Vec<String>,
}

impl PublicResearchBrief {
    pub fn validate(&self) -> Result<()> {
        stable_id(&self.id)?;
        ensure!(
            !self.topics.is_empty() || !self.queries.is_empty() || !self.urls.is_empty(),
            "Public research brief is empty"
        );
        for entries in [&self.topics, &self.queries, &self.urls] {
            ensure!(
                entries.len() <= 16,
                "Public research brief exceeds 16 entries per field"
            );
            for entry in entries {
                bounded_text(entry, 4096, "Public research entry")?;
            }
        }
        for url in &self.urls {
            validate_public_url_shape(url)?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 16 * 1024,
            "Public research brief exceeds 16 KiB"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum SourceLocator {
    FileRange {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    Git {
        commit: String,
        path: Option<String>,
    },
    GitHistory {
        path: Option<String>,
    },
    GitDiff {
        base: String,
        head: String,
        paths: Vec<String>,
    },
    FileListing {
        prefix: Option<String>,
    },
    TextSearch {
        query: String,
        paths: Vec<String>,
    },
    DependencyMetadata {
        ecosystem: DependencyEcosystem,
    },
    Inspection {
        recipe: InspectionRecipe,
    },
    Conversation {
        session_id: String,
        first_sequence: u64,
        last_sequence: u64,
    },
    PublicUrl {
        url: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResearchProvenance {
    ProjectFile,
    Git,
    DependencyMetadata,
    FixedInspection,
    Conversation,
    PublicWeb,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSource {
    pub id: String,
    pub locator: SourceLocator,
    pub source_hash: String,
    pub captured_at_ms: i64,
    pub provenance: ResearchProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResearchReceiptStatus {
    Complete,
    Partial,
    Unavailable,
    Denied,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchOmission {
    pub locator: Option<SourceLocator>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchReceiptLimits {
    pub max_content_bytes: usize,
    pub returned_bytes: usize,
}

/// Observed host execution only. This is never implementation-check evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchCommand {
    pub argv: Vec<String>,
    pub cwd: String,
    pub executable_hash: String,
    pub exit_code: Option<i64>,
    pub output_truncated: bool,
    pub cancelled: bool,
    pub timed_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchReceipt {
    pub id: String,
    pub request: ResearchRequest,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub command: Option<ResearchCommand>,
    pub sources: Vec<ResearchSource>,
    pub status: ResearchReceiptStatus,
    pub content: String,
    pub content_hash: String,
    pub created_at_ms: i64,
    pub limits: ResearchReceiptLimits,
    pub omissions: Vec<ResearchOmission>,
}

impl ResearchReceipt {
    pub fn new(
        request: ResearchRequest,
        sources: Vec<ResearchSource>,
        status: ResearchReceiptStatus,
        content: String,
        omissions: Vec<ResearchOmission>,
    ) -> Result<Self> {
        let receipt = Self {
            id: new_id(),
            request,
            snapshot_id: None,
            command: None,
            sources,
            status,
            content_hash: content_hash(content.as_bytes()),
            limits: ResearchReceiptLimits {
                max_content_bytes: MAX_RESEARCH_RECEIPT_BYTES,
                returned_bytes: content.len(),
            },
            content,
            created_at_ms: now_ms(),
            omissions,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<()> {
        stable_id(&self.id)?;
        self.request.validate()?;
        if let Some(id) = &self.snapshot_id {
            ensure!(valid_hash(id), "Invalid research snapshot hash");
        }
        if let Some(command) = &self.command {
            ensure!(
                !command.argv.is_empty()
                    && command.argv.len() <= 128
                    && command
                        .argv
                        .iter()
                        .all(|s| s.len() <= 8192 && !s.contains('\0'))
                    && command.cwd.len() <= 4096
                    && !command.cwd.contains('\0')
                    && valid_hash(&command.executable_hash),
                "Invalid inspection command provenance"
            );
        }
        ensure!(
            self.limits.max_content_bytes <= MAX_RESEARCH_RECEIPT_BYTES
                && self.content.len() <= self.limits.max_content_bytes
                && self.limits.returned_bytes == self.content.len(),
            "Research receipt content exceeds its declared limit"
        );
        ensure!(
            self.content_hash == content_hash(self.content.as_bytes()),
            "Research receipt content hash differs"
        );
        ensure!(
            self.sources.len() <= 64 && self.omissions.len() <= 64,
            "Research receipt metadata exceeds its entry limit"
        );
        ensure!(
            serde_json::to_vec(&(&self.sources, &self.omissions))?.len() <= 16 * 1024,
            "Research source metadata exceeds 16 KiB"
        );
        let mut ids = BTreeSet::new();
        for source in &self.sources {
            stable_id(&source.id)?;
            ensure!(ids.insert(&source.id), "Research source IDs must be unique");
            ensure!(
                source.source_hash.len() == 64
                    && source.source_hash.bytes().all(|c| c.is_ascii_hexdigit()),
                "Research source needs a SHA-256 hash"
            );
        }
        ensure!(
            self.status != ResearchReceiptStatus::Complete || self.omissions.is_empty(),
            "A receipt with omissions must disclose a partial result"
        );
        for omission in &self.omissions {
            bounded_text(&omission.reason, 4096, "Research omission")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchCitation {
    pub receipt_id: String,
    pub source_id: String,
    pub locator: SourceLocator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchFinding {
    pub summary: String,
    pub citations: Vec<ResearchCitation>,
}

impl ResearchFinding {
    pub fn validate(&self, receipts: &[ResearchReceipt]) -> Result<()> {
        bounded_text(&self.summary, 4096, "Research finding")?;
        ensure!(
            !self.citations.is_empty() && self.citations.len() <= 32,
            "A finding needs 1–32 real source citations"
        );
        let mut ids = BTreeSet::new();
        ensure!(
            receipts.iter().all(|r| ids.insert(&r.id)),
            "Receipt identity is ambiguous"
        );
        for citation in &self.citations {
            let receipt = receipts
                .iter()
                .find(|r| r.id == citation.receipt_id)
                .ok_or_else(|| anyhow::anyhow!("Finding cites an unknown receipt"))?;
            receipt.validate()?;
            ensure!(
                matches!(
                    receipt.status,
                    ResearchReceiptStatus::Complete | ResearchReceiptStatus::Partial
                ),
                "Finding cites unavailable research"
            );
            ensure!(
                receipt
                    .sources
                    .iter()
                    .any(|s| s.id == citation.source_id && s.locator == citation.locator),
                "Finding cites an unknown source or changed locator"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ResearchActivity {
    Requested {
        request: ResearchRequest,
    },
    Received {
        receipt_id: String,
        status: ResearchReceiptStatus,
    },
    Stopped {
        reason: ResearchStopReason,
        detail: String,
        continue_available: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResearchStopReason {
    Complete,
    BudgetExhausted,
    Cancelled,
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchBudget {
    pub rounds: u8,
    pub requests: u8,
    pub receipts: u8,
    pub returned_bytes: usize,
}

impl ResearchBudget {
    pub fn begin_round(&mut self, requests: u8) -> Result<()> {
        ensure!(
            (1..=MAX_RESEARCH_REQUESTS_PER_ROUND).contains(&requests),
            "A research round permits 1–4 requests"
        );
        ensure!(
            self.rounds < MAX_RESEARCH_ROUNDS && self.requests <= MAX_RESEARCH_RECEIPTS - requests,
            "Research budget exhausted; offer Continue"
        );
        self.rounds += 1;
        self.requests += requests;
        Ok(())
    }

    pub fn record_receipt(&mut self, receipt: &ResearchReceipt) -> Result<()> {
        receipt.validate()?;
        ensure!(
            self.receipts < self.requests
                && self.receipts < MAX_RESEARCH_RECEIPTS
                && self.returned_bytes
                    <= MAX_RESEARCH_RETURNED_BYTES.saturating_sub(receipt.content.len()),
            "Research budget exhausted; offer Continue"
        );
        self.receipts += 1;
        self.returned_bytes += receipt.content.len();
        Ok(())
    }
}
