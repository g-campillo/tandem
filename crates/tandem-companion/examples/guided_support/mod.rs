//! Opt-in verification harness only. No production approval or billing bypass.
#![allow(dead_code)] // Shared by independently compiled examples and integration tests.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use tandem_companion::*;
use tandem_core::*;
pub mod sandbox;
pub mod supplemental;

pub fn require_subscription_usage(args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<()> {
    ensure!(
        args.into_iter()
            .any(|a| a.as_ref() == "--allow-subscription-usage"),
        "Refusing before connection: pass --allow-subscription-usage only under the approved 4 + 24 + 24 turn verification scope"
    );
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    counts: BTreeMap<String, usize>,
    reservations: Vec<Reservation>,
    suites: BTreeMap<String, String>,
    #[serde(default)]
    amendments: Vec<BudgetAmendment>,
    #[serde(default)]
    model_selections: Vec<ModelSelectionAmendment>,
    #[serde(default)]
    additional_authorizations: Vec<AdditionalAmendment>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    language: String,
    worker: String,
    at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Caps {
    probe: usize,
    rust: usize,
    python: usize,
    total: usize,
}
impl Default for Caps {
    fn default() -> Self {
        Self {
            probe: 4,
            rust: 24,
            python: 24,
            total: 52,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetAuthorization {
    version: u32,
    authorized_at_utc: String,
    user_message: String,
    interpretation: String,
    plan_path: PathBuf,
    repo: PathBuf,
    suite_directory: PathBuf,
    ledger_path: PathBuf,
    previous_caps: Caps,
    authorized_caps: Caps,
    consumed_at_authorization: Caps,
    constraints: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetAmendment {
    authorization: BudgetAuthorization,
    authorization_hash: String,
    reservation_prefix_hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelectionAuthorization {
    version: u32,
    authorized_at_utc: String,
    user_message: String,
    interpretation: String,
    repo: PathBuf,
    plan_path: PathBuf,
    suite_directory: PathBuf,
    ledger_path: PathBuf,
    budget_authorization_path: PathBuf,
    selected_model: AuthorizedModel,
    consumed_at_authorization: Caps,
    authorized_caps: Caps,
    scope: String,
    constraints: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizedModel {
    catalog_id: String,
    effort: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelectionAmendment {
    authorization: ModelSelectionAuthorization,
    authorization_hash: String,
    reservation_prefix_hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationParent {
    path: PathBuf,
    sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditionalAuthorization {
    version: u32,
    authorized_at_utc: String,
    user_message: String,
    interpretation: String,
    plan_path: PathBuf,
    repo: PathBuf,
    suite_directory: PathBuf,
    ledger_path: PathBuf,
    parent_budget_authorization: AuthorizationParent,
    parent_model_authorization: AuthorizationParent,
    previous_caps: Caps,
    authorized_caps: Caps,
    additional_python_turns: usize,
    consumed_at_authorization: Caps,
    selected_model: AuthorizedModel,
    allowed_continuation_languages: Vec<String>,
    reservation_prefix_count: usize,
    reservation_prefix_semantic_sha256: String,
    reservation_prefix_hash_encoding: String,
    ledger_at_authorization_sha256: String,
    constraints: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditionalAmendment {
    authorization: AdditionalAuthorization,
    authorization_hash: String,
}
pub struct TurnBudget {
    path: PathBuf,
    ledger: Ledger,
    lock: PathBuf,
    caps: Caps,
}
impl Drop for TurnBudget {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock);
    }
}
impl TurnBudget {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path, None, None)
    }
    pub fn open_authorized(
        path: &Path,
        authorization_path: &Path,
        repo: &Path,
        plan: &Path,
    ) -> Result<Self> {
        let bytes =
            fs::read(authorization_path).context("Explicit budget authorization unavailable")?;
        let additional = if serde_json::from_slice::<serde_json::Value>(&bytes)?["version"] == 3 {
            let extra: AdditionalAuthorization = serde_json::from_slice(&bytes)?;
            Some((extra, content_hash(&bytes)))
        } else {
            None
        };
        let base_bytes = if let Some((extra, _)) = &additional {
            let parent = fs::read(&extra.parent_budget_authorization.path)?;
            ensure!(
                content_hash(&parent) == extra.parent_budget_authorization.sha256,
                "Parent budget authorization changed"
            );
            parent
        } else {
            bytes
        };
        let authorization: BudgetAuthorization = serde_json::from_slice(&base_bytes)?;
        ensure!(
            authorization.version == 2
                && authorization.user_message == "Continue on"
                && !authorization.authorized_at_utc.is_empty()
                && !authorization.interpretation.is_empty()
                && !authorization.constraints.is_empty()
                && authorization.repo == repo.canonicalize()?
                && authorization.plan_path == plan
                && authorization.suite_directory
                    == path.parent().context("Suite missing")?.canonicalize()?
                && authorization.ledger_path == path
                && authorization.previous_caps == Caps::default()
                && authorization.authorized_caps
                    == Caps {
                        probe: 4,
                        rust: 56,
                        python: 56,
                        total: 116
                    }
                && authorization.consumed_at_authorization
                    == Caps {
                        probe: 1,
                        rust: 24,
                        python: 24,
                        total: 49
                    },
            "Authorization does not match this approved suite, plan, caps and consumed ledger"
        );
        Self::open_inner(
            path,
            Some((authorization, content_hash(&base_bytes))),
            additional,
        )
    }
    fn open_inner(
        path: &Path,
        authorization: Option<(BudgetAuthorization, String)>,
        additional: Option<(AdditionalAuthorization, String)>,
    ) -> Result<Self> {
        fs::create_dir_all(path.parent().context("Budget directory missing")?)?;
        let lock = path.with_extension("lock");
        let lock_file = fs::OpenOptions::new().write(true).create_new(true).open(&lock)
            .context("Another verification owns the budget, or a prior process stopped without releasing it. Inspect the durable ledger; never reset it")?;
        lock_file.sync_all()?;
        let ledger = if path.exists() {
            serde_json::from_slice(&fs::read(path)?)
        } else {
            Ok(Ledger::default())
        };
        let ledger = match ledger {
            Ok(ledger) => ledger,
            Err(error) => {
                fs::remove_file(&lock)?;
                return Err(error.into());
            }
        };
        let mut budget = Self {
            path: path.into(),
            ledger,
            lock,
            caps: Caps::default(),
        };
        if let Some((authorization, hash)) = authorization {
            let baseline = authorization.consumed_at_authorization.total;
            ensure!(
                budget.ledger.reservations.len() >= baseline,
                "Authorization history was refunded or reset"
            );
            let prefix = &budget.ledger.reservations[..baseline];
            let mut counts = BTreeMap::<String, usize>::new();
            for r in prefix {
                *counts.entry(r.language.clone()).or_default() += 1;
            }
            ensure!(
                counts
                    == BTreeMap::from([
                        ("probe".into(), 1),
                        ("rust".into(), 24),
                        ("python".into(), 24)
                    ]),
                "Authorization consumed history mismatches"
            );
            let prefix_hash = content_hash(&serde_json::to_vec(prefix)?);
            if let Some(amendment) = budget.ledger.amendments.first() {
                ensure!(
                    budget.ledger.amendments.len() == 1
                        && amendment.authorization_hash == hash
                        && amendment.reservation_prefix_hash == prefix_hash
                        && serde_json::to_value(&amendment.authorization)?
                            == serde_json::to_value(&authorization)?,
                    "Immutable authorization amendment changed"
                );
            } else {
                ensure!(
                    budget.total() == baseline && budget.ledger.reservations.len() == baseline,
                    "Initial amendment must match consumed authorization exactly"
                );
                budget.ledger.amendments.push(BudgetAmendment {
                    authorization: authorization.clone(),
                    authorization_hash: hash,
                    reservation_prefix_hash: prefix_hash,
                });
            }
            budget.caps = authorization.authorized_caps;
        } else {
            ensure!(
                budget.ledger.amendments.is_empty(),
                "Extended ledger requires its explicit matching authorization; defaults remain 24/52"
            );
        }
        if let Some((extra, hash)) = additional {
            ensure!(
                extra.version == 3
                    && extra.user_message == "Approved"
                    && !extra.authorized_at_utc.is_empty()
                    && !extra.interpretation.is_empty()
                    && !extra.constraints.is_empty()
                    && extra.repo == budget.ledger.amendments[0].authorization.repo
                    && extra.plan_path == budget.ledger.amendments[0].authorization.plan_path
                    && extra.suite_directory
                        == budget
                            .path
                            .parent()
                            .context("Suite missing")?
                            .canonicalize()?
                    && extra.ledger_path == budget.path
                    && extra.previous_caps == budget.caps
                    && extra.authorized_caps
                        == Caps {
                            probe: 4,
                            rust: 56,
                            python: 72,
                            total: 132
                        }
                    && extra.additional_python_turns == 16
                    && extra.consumed_at_authorization
                        == Caps {
                            probe: 1,
                            rust: 52,
                            python: 51,
                            total: 104
                        }
                    && extra.reservation_prefix_count == 104
                    && extra.reservation_prefix_hash_encoding
                        == "SHA256 compact UTF8JSON, recursivelysortedobjectkeys, arrayorderpreserved, noASCIIescaping"
                    && extra.allowed_continuation_languages == ["python"]
                    && extra.selected_model.catalog_id == "gpt-6-astra"
                    && extra.selected_model.effort == "model_supported_default"
                    && budget.ledger.model_selections.len() == 1,
                "Additional authorization does not match the approved Python-only continuation"
            );
            ensure!(
                content_hash(&fs::read(&extra.parent_model_authorization.path)?)
                    == extra.parent_model_authorization.sha256
                    && budget.ledger.model_selections[0].authorization_hash
                        == extra.parent_model_authorization.sha256
                    && budget.ledger.amendments[0].authorization_hash
                        == extra.parent_budget_authorization.sha256,
                "Additional authorization parent records changed"
            );
            // Revalidate the original 96-reservation model prefix before adding caps.
            let original = &budget.ledger.amendments[0].authorization;
            budget.model_preference(
                Some((
                    &extra.parent_model_authorization.path,
                    &extra.parent_budget_authorization.path,
                )),
                &original.repo.clone(),
                &original.plan_path.clone(),
                "python",
            )?;
            let baseline = extra.reservation_prefix_count;
            ensure!(
                budget.ledger.reservations.len() >= baseline,
                "Additional authorization history was refunded or reset"
            );
            let prefix = &budget.ledger.reservations[..baseline];
            let mut counts = BTreeMap::<String, usize>::new();
            for r in prefix {
                *counts.entry(r.language.clone()).or_default() += 1;
            }
            // Explicit sorted maps keep the external compact-UTF8 hash stable
            // with either serde_json map implementation. Reservation leaves are scalars.
            let semantic_prefix = prefix
                .iter()
                .map(|reservation| {
                    BTreeMap::from([
                        ("at_ms", json!(reservation.at_ms)),
                        ("language", json!(reservation.language)),
                        ("worker", json!(reservation.worker)),
                    ])
                })
                .collect::<Vec<_>>();
            ensure!(
                counts
                    == BTreeMap::from([
                        ("probe".into(), 1),
                        ("rust".into(), 52),
                        ("python".into(), 51)
                    ])
                    && content_hash(&serde_json::to_vec(&semantic_prefix)?)
                        == extra.reservation_prefix_semantic_sha256
                    && budget.ledger.reservations[baseline..]
                        .iter()
                        .all(|r| r.language == "python"),
                "Additional authorization reservation prefix changed"
            );
            if let Some(recorded) = budget.ledger.additional_authorizations.first() {
                ensure!(
                    budget.ledger.additional_authorizations.len() == 1
                        && recorded.authorization_hash == hash
                        && serde_json::to_value(&recorded.authorization)?
                            == serde_json::to_value(&extra)?,
                    "Immutable additional authorization changed"
                );
            } else {
                ensure!(
                    budget.total() == baseline
                        && content_hash(&fs::read(path)?) == extra.ledger_at_authorization_sha256,
                    "Initial additional authorization must match the exact consumed ledger"
                );
                budget
                    .ledger
                    .additional_authorizations
                    .push(AdditionalAmendment {
                        authorization: extra.clone(),
                        authorization_hash: hash,
                    });
            }
            budget.caps = extra.authorized_caps;
        } else {
            ensure!(
                budget.ledger.additional_authorizations.is_empty(),
                "Expanded ledger requires its explicit matching additional authorization"
            );
        }
        let mut actual_counts = BTreeMap::<String, usize>::new();
        for r in &budget.ledger.reservations {
            ensure!(
                matches!(r.worker.as_str(), "private" | "public"),
                "Unknown worker"
            );
            *actual_counts.entry(r.language.clone()).or_default() += 1;
        }
        ensure!(
            actual_counts == budget.ledger.counts,
            "Counts do not match charged reservations"
        );
        for (language, count) in &budget.ledger.counts {
            ensure!(*count <= budget.cap(language)?, "Corrupt turn count");
        }
        ensure!(
            budget.total() <= budget.caps.total
                && budget.total() == budget.ledger.reservations.len(),
            "Corrupt aggregate budget"
        );
        if !budget.ledger.amendments.is_empty() {
            budget.persist()?;
        }
        Ok(budget)
    }
    fn cap(&self, language: &str) -> Result<usize> {
        Ok(match language {
            "probe" => self.caps.probe,
            "rust" => self.caps.rust,
            "python" => self.caps.python,
            _ => anyhow::bail!("Unknown verification budget"),
        })
    }
    pub fn total(&self) -> usize {
        self.ledger.counts.values().sum()
    }
    pub fn counts(&self) -> BTreeMap<String, usize> {
        self.ledger.counts.clone()
    }
    pub fn model_preference(
        &mut self,
        authorization: Option<(&Path, &Path)>,
        repo: &Path,
        plan: &Path,
        language: &str,
    ) -> Result<ModelPreference> {
        let Some((model_path, budget_path)) = authorization else {
            ensure!(
                self.ledger.model_selections.is_empty(),
                "Recorded explicit model selection requires its original authorization; no Automatic fallback"
            );
            return Ok(ModelPreference::Automatic);
        };
        let bytes = fs::read(model_path).context("Explicit model authorization unavailable")?;
        let selected: ModelSelectionAuthorization = serde_json::from_slice(&bytes)?;
        let hash = content_hash(&bytes);
        ensure!(
            language == "python"
                && selected.version == 1
                && selected.user_message == "Continue"
                && !selected.authorized_at_utc.is_empty()
                && !selected.interpretation.is_empty()
                && !selected.scope.is_empty()
                && !selected.constraints.is_empty()
                && selected.repo == repo.canonicalize()?
                && selected.plan_path == plan
                && selected.suite_directory
                    == self
                        .path
                        .parent()
                        .context("Suite missing")?
                        .canonicalize()?
                && selected.ledger_path == self.path
                && (selected.budget_authorization_path == budget_path
                    || self
                        .ledger
                        .additional_authorizations
                        .first()
                        .is_some_and(|extra| selected.budget_authorization_path
                            == extra.authorization.parent_budget_authorization.path
                            && fs::read(budget_path).is_ok_and(
                                |bytes| content_hash(&bytes) == extra.authorization_hash
                            )))
                && self.ledger.amendments.len() == 1
                && self.ledger.amendments[0].authorization_hash
                    == content_hash(&fs::read(&selected.budget_authorization_path)?)
                && selected.selected_model.catalog_id == "gpt-6-astra"
                && selected.selected_model.effort == "model_supported_default"
                && selected.authorized_caps
                    == self.ledger.amendments[0].authorization.authorized_caps
                && selected.consumed_at_authorization
                    == Caps {
                        probe: 1,
                        rust: 52,
                        python: 43,
                        total: 96
                    },
            "Model authorization does not match the existing Python suite, budget, model and consumed history"
        );
        let baseline = selected.consumed_at_authorization.total;
        ensure!(
            self.ledger.reservations.len() >= baseline,
            "Model selection history was refunded or reset"
        );
        let prefix = &self.ledger.reservations[..baseline];
        let mut counts = BTreeMap::<String, usize>::new();
        for reservation in prefix {
            *counts.entry(reservation.language.clone()).or_default() += 1;
        }
        ensure!(
            counts
                == BTreeMap::from([
                    ("probe".into(), 1),
                    ("rust".into(), 52),
                    ("python".into(), 43)
                ])
                && self.ledger.reservations[baseline..]
                    .iter()
                    .all(|r| r.language == "python"),
            "Explicit selection permits only remaining Python turns; prior history must match"
        );
        let prefix_hash = content_hash(&serde_json::to_vec(prefix)?);
        if let Some(recorded) = self.ledger.model_selections.first() {
            ensure!(
                self.ledger.model_selections.len() == 1
                    && recorded.authorization_hash == hash
                    && recorded.reservation_prefix_hash == prefix_hash
                    && serde_json::to_value(&recorded.authorization)?
                        == serde_json::to_value(&selected)?,
                "Immutable explicit model selection changed"
            );
        } else {
            ensure!(
                self.total() == baseline,
                "Initial model selection must match its exact consumed history"
            );
            self.ledger.model_selections.push(ModelSelectionAmendment {
                authorization: selected,
                authorization_hash: hash,
                reservation_prefix_hash: prefix_hash,
            });
            self.persist()?;
        }
        Ok(ModelPreference::Explicit {
            model: "gpt-6-astra".into(),
        })
    }
    fn persist(&self) -> Result<()> {
        let parent = self.path.parent().context("Budget directory missing")?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("budget-{}.tmp", new_id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(&self.ledger)?)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
    /// Durable, conservative charge BEFORE an operation can dispatch its single
    /// turn/start. A pre-dispatch failure is never refunded. Public workers share
    /// this ledger; retries cannot acquire a fresh allowance.
    pub fn reserve(&mut self, language: &str, worker: &str) -> Result<()> {
        let cap = self.cap(language)?;
        ensure!(
            self.ledger.additional_authorizations.is_empty() || language == "python",
            "Additional authorization permits only the existing Python continuation"
        );
        ensure!(matches!(worker, "private" | "public"), "Unknown worker");
        ensure!(
            self.ledger.counts.get(language).copied().unwrap_or(0) < cap
                && self.total() < self.caps.total,
            "Approved subscription turn cap reached; no dispatch permitted"
        );
        *self.ledger.counts.entry(language.into()).or_default() += 1;
        self.ledger.reservations.push(Reservation {
            language: language.into(),
            worker: worker.into(),
            at_ms: now_ms(),
        });
        self.persist()
    }
    pub fn claim(&mut self, suite: &str) -> Result<()> {
        self.cap(suite)?;
        ensure!(
            !self.ledger.suites.contains_key(suite),
            "This suite was already attempted. Inspect its durable receipts; no blind retry or fresh budget is permitted"
        );
        self.ledger.suites.insert(suite.into(), "started".into());
        self.persist()
    }
    pub fn finish(&mut self, suite: &str, outcome: &str) -> Result<()> {
        ensure!(
            self.ledger.suites.contains_key(suite),
            "Suite was not claimed"
        );
        self.ledger.suites.insert(suite.into(), outcome.into());
        self.persist()
    }
    pub fn resume_authorized(&mut self, language: &str) -> Result<()> {
        ensure!(
            !self.ledger.amendments.is_empty()
                && matches!(language, "rust" | "python")
                && self.ledger.suites.get(language).map(String::as_str) == Some("failed")
                && self.ledger.counts.get(language).copied().unwrap_or(0) >= 24
                && self.ledger.counts[language] < self.cap(language)?,
            "No authorized unfinished same-suite continuation"
        );
        self.ledger.suites.insert(
            language.into(),
            "same-suite-authorized-stage-continuation".into(),
        );
        self.persist()
    }
    pub fn resume_schema_failure(&mut self, language: &str) -> Result<()> {
        ensure!(
            matches!(language, "rust" | "python")
                && self.ledger.suites.get(language).map(String::as_str) == Some("failed")
                && self.ledger.counts.get(language) == Some(&1),
            "Only the diagnosed first-turn schema failure may continue this same bounded suite"
        );
        self.ledger
            .suites
            .insert(language.into(), "same-suite-schema-fix-continuation".into());
        self.persist()
    }
    pub fn resume_research_question_failure(&mut self, language: &str) -> Result<()> {
        ensure!(
            matches!(language, "rust" | "python")
                && self.ledger.suites.get(language).map(String::as_str) == Some("failed")
                && self.ledger.counts.get(language) == Some(&2),
            "Only the diagnosed mixed research/question failure may continue this same suite"
        );
        self.ledger.suites.insert(
            language.into(),
            "same-suite-research-question-fix-continuation".into(),
        );
        self.persist()
    }
    pub fn resume_public_vocabulary_failure(&mut self, language: &str) -> Result<()> {
        ensure!(
            matches!(language, "rust" | "python")
                && self.ledger.suites.get(language).map(String::as_str) == Some("failed")
                && self.ledger.counts.get(language) == Some(&10),
            "Only the diagnosed eight-round public vocabulary failure may continue this same suite"
        );
        self.ledger.suites.insert(
            language.into(),
            "same-suite-public-vocabulary-fix-continuation".into(),
        );
        self.persist()
    }
    pub fn resume_independent_checks(&mut self, language: &str) -> Result<()> {
        let expected = match language {
            "rust" => 11,
            "python" => 13,
            _ => anyhow::bail!("Unknown suite"),
        };
        ensure!(
            self.ledger.suites.get(language).map(String::as_str) == Some("failed")
                && self.ledger.counts.get(language) == Some(&expected),
            "Only the diagnosed current suite may continue independent checks"
        );
        self.ledger.suites.insert(
            language.into(),
            "same-suite-independent-checks-continuation".into(),
        );
        self.persist()
    }
    pub fn resume_python_draft_only(&mut self) -> Result<()> {
        ensure!(
            self.ledger.suites.get("python").map(String::as_str) == Some("failed")
                && self.ledger.counts.get("python") == Some(&21),
            "Only the existing Python DraftSpec may use its three remaining authorized turns"
        );
        self.ledger.suites.insert(
            "python".into(),
            "same-suite-draft-spec-only-continuation".into(),
        );
        self.persist()
    }
}
pub type SharedBudget = Arc<Mutex<TurnBudget>>;
pub fn run_directory() -> Result<PathBuf> {
    // One durable approved suite, shared by both executables. Changing the
    // timestamp/working tree does not reset the approved turn allowance.
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("Repository root unavailable")?;
    ensure!(
        std::env::current_dir()?.canonicalize()? == repository.canonicalize()?,
        "Run verification from its repository root; another working directory cannot create a fresh turn allowance"
    );
    let path = repository.join(".tandem-local/guided-companion/approved-suite");
    fs::create_dir_all(&path)?;
    Ok(path)
}
pub fn budget(root: &Path) -> Result<SharedBudget> {
    Ok(Arc::new(Mutex::new(TurnBudget::open(
        &root.join("turn-budget.json"),
    )?)))
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
pub fn sanitized_runtime_error(error: &serde_json::Value) -> serde_json::Value {
    let message = error["message"].as_str().unwrap_or_default();
    let code = match error["codexErrorInfo"].as_str() {
        Some("other") => "other",
        Some("unauthorized") => "unauthorized",
        Some("usageLimitExceeded") => "usageLimitExceeded",
        Some("rateLimitExceeded") => "rateLimitExceeded",
        _ => "unrecognized",
    };
    let safe_schema = message.len() <= 8192
        && message.contains("invalid_json_schema")
        && message.contains("text.format.schema")
        && !credential_like(message)
        && !message.contains("https://")
        && !message.contains("http://");
    if safe_schema {
        json!({"codexErrorInfo":code,"message":message,"additionalDetails":null,"misalignment":null})
    } else {
        json!({"codexErrorInfo":code,"message_withheld":true,"message_hash":content_hash(message.as_bytes())})
    }
}
pub fn checkpoint_overlays(
    store: &mut Store,
    project: &Project,
    documents: &[Document],
) -> Result<()> {
    let mut workspace = store
        .workspace(project.key())?
        .unwrap_or_else(|| WorkspaceState::new(project));
    workspace.sequence = store
        .sequence()?
        .checked_add(1)
        .context("Checkpoint sequence exhausted")?;
    for document in documents {
        if !workspace.tabs.contains(&document.path) {
            workspace.tabs.push(document.path.clone());
        }
    }
    workspace.active = documents.first().map(|document| document.path.clone());
    store.checkpoint(&workspace, documents)
}
pub fn identity() -> Result<serde_json::Value> {
    let installation = tandem_codex::discover_runtime()?;
    let code = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()?;
    let mut sources = BTreeMap::new();
    for path in [
        "crates/tandem-core/src/companion.rs",
        "crates/tandem-core/src/conversation.rs",
        "crates/tandem-codex/src/lib.rs",
        "crates/tandem-companion/src/conversation.rs",
        "crates/tandem-companion/src/runtime.rs",
        "crates/tandem-companion/src/public_research.rs",
        "crates/tandem-companion/examples/guided_journeys.rs",
        "crates/tandem-companion/examples/guided_support/mod.rs",
        "crates/tandem-companion/examples/guided_support/supplemental.rs",
    ] {
        sources.insert(path, file_hash(Path::new(path))?);
    }
    Ok(
        json!({"code_sha":String::from_utf8(code.stdout)?.trim(),"worktree_source_hashes":sources,"public_search_output_schema_hash":public_search_schema_hash(),"runtime":tandem_codex::EXPECTED_RUNTIME,
        "runtime_hash":file_hash(&installation.binary)?,"schema":"pinned app-server 0.169.0; exact per-dispatch outputSchema SHA-256 in dispatch receipts",
        "store_schema":5,"os":std::env::consts::OS,"arch":std::env::consts::ARCH}),
    )
}
pub fn public_search_schema_hash() -> String {
    content_hash(&serde_json::to_vec(&json!({"type":"object","additionalProperties":false,"required":["candidates"],"properties":{"candidates":{"type":"array","maxItems":16,"items":{"type":"string"}}}})).expect("static schema serializes"))
}
pub fn health(root: &Path) -> Result<(CompanionSession, ResolvedModel)> {
    health_with_preference(
        &root.join("prerequisites.json"),
        &ModelPreference::Automatic,
    )
}
pub fn health_with_preference(
    receipt: &Path,
    preference: &ModelPreference,
) -> Result<(CompanionSession, ResolvedModel)> {
    let mut session = CompanionSession::connect()?;
    let catalog = session.list_models()?;
    let model = tandem_companion::resolve_model(&catalog, preference)?;
    write_json(
        receipt,
        &json!({"identity":identity()?,"connection":session.connection,"catalog":catalog,"preference":preference,"selected":model,"model_turns":0}),
    )?;
    println!(
        "Prerequisites: {:?}; model={}, effort={:?}; zero model turns",
        session.connection.kind, model.model, model.effort
    );
    Ok((session, model))
}
pub struct BudgetBackend {
    pub inner: CompanionSession,
    pub budget: SharedBudget,
    pub language: String,
    pub directory: PathBuf,
    pub stop_duplicate_public_pages: bool,
}
pub fn duplicate_public_page_requests(reply: &ConversationReply) -> bool {
    reply.research_requests.len() > 1
        && reply.research_requests.first().is_some_and(|first| {
            matches!(first, ResearchRequestKind::PublicPage { .. })
                && reply
                    .research_requests
                    .iter()
                    .all(|request| request == first)
        })
}
impl ConversationBackend for BudgetBackend {
    fn check_account(&mut self, cancel: &AtomicBool) -> Result<()> {
        ConversationBackend::check_account(&mut self.inner, cancel)
    }
    fn resolve_model(
        &mut self,
        preference: &ModelPreference,
        cancel: &AtomicBool,
    ) -> Result<ResolvedModel> {
        ConversationBackend::resolve_model(&mut self.inner, preference, cancel)
    }
    fn turn(
        &mut self,
        run: ModelRun,
        thread: Option<ThreadBinding>,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&ModelRun) -> Result<()>,
    ) -> Result<(ModelRun, Option<ThreadBinding>)> {
        self.budget
            .lock()
            .unwrap()
            .reserve(&self.language, "private")?;
        let id = run.id.clone();
        let schema = tandem_codex::output_schema_json(&run.request.schema())?;
        fs::write(
            self.directory.join(format!("wire-schema-{id}.json")),
            &schema,
        )?;
        write_json(
            &self.directory.join(format!("dispatch-{id}.json")),
            &json!({"action":run.request.action,"model":run.request.resolved_model,"schema_hash":content_hash(schema.as_bytes()),"schema_encoding":"discriminator-first protocol outputSchema; exact transmitted JSON bytes","identity":identity()?}),
        )?;
        let result = self.inner.run(run, thread, cancel, |run| {
            write_json(&self.directory.join(format!("private-{id}.json")), run)?;
            observe(run)
        })?;
        write_json(
            &self.directory.join(format!("connection-after-{id}.json")),
            &self.inner.connection,
        )?;
        if self.stop_duplicate_public_pages
            && matches!(&result.0.response, Some(CompanionResponse::Discussion(reply)) if duplicate_public_page_requests(reply))
        {
            write_json(
                &self
                    .directory
                    .join(format!("harness-cancellation-{id}.json")),
                &json!({"choice":"Harness human cancels at the completed response boundary after the same duplicate-PublicPage failure; no further broker/model rounds or refund", "run_id":id,"charged_counts":self.budget.lock().unwrap().counts()}),
            )?;
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        println!(
            "{} private turn: {:?}; charged total={}",
            self.language,
            result.0.status,
            self.budget.lock().unwrap().total()
        );
        Ok(result)
    }
}
pub struct BudgetPublic {
    pub inner: PublicDataWorker<tandem_codex::Client, AnonymousPublicHttp>,
    pub budget: SharedBudget,
    pub language: String,
    pub directory: PathBuf,
}
impl BudgetPublic {
    pub fn connect(budget: SharedBudget, language: &str, directory: &Path) -> Result<Self> {
        Ok(Self {
            inner: PublicDataWorker {
                search: PublicResearchSession::connect()?,
                sources: PublicSourceFetcher::with_transport(AnonymousPublicHttp::new()?),
            },
            budget,
            language: language.into(),
            directory: directory.into(),
        })
    }
}
impl PublicWorker for BudgetPublic {
    fn execute(
        &mut self,
        brief: &ScreenedPublicBrief,
        model: &ResolvedModel,
        cancel: &AtomicBool,
        observe: &mut dyn FnMut(&PublicResearchProgress) -> Result<()>,
    ) -> Result<ResearchReceipt> {
        if brief.brief().urls.is_empty() {
            self.budget
                .lock()
                .unwrap()
                .reserve(&self.language, "public")?;
        }
        let mut observations = Vec::new();
        write_json(
            &self
                .directory
                .join(format!("public-brief-{}.json", brief.brief().id)),
            &json!({"brief":brief.brief(),"model":model}),
        )?;
        let receipt = self.inner.execute(brief, model, cancel, &mut |progress| {
            if let PublicResearchProgress::SearchObserved { observation } = progress {
                observations.push(observation.clone());
            }
            observe(progress)
        })?;
        write_json(
            &self.directory.join(format!("public-{}.json", receipt.id)),
            &json!({"receipt":receipt,"observations":observations}),
        )?;
        Ok(receipt)
    }
}

/// Original disposable fixtures. Oracles are authored independently of model
/// suggestions; all alternative code is an explicit harness human choice.
pub struct FixtureTask {
    pub project: Project,
    pub language: String,
    pub path: String,
    pub recipe: String,
    pub wrong: String,
    pub alternative: String,
    pub original_hashes: BTreeMap<String, String>,
    pub owned_module_hash: Option<String>,
}
/// The Store-aware host adapter follows the native workspace's check-origin
/// policy. Raw ProjectConversationCapture has no Store and cannot authenticate
/// execution origins or retrieve a cumulative journal by itself.
pub struct StoredCapture {
    pub inner: ProjectConversationCapture,
    pub store: Arc<Mutex<Store>>,
}
impl ConversationCapture for StoredCapture {
    fn capture(
        &mut self,
        session: &TaskSession,
        action: CompanionAction,
        language: &str,
        guidance: &EffectiveGuidance,
        checks: &[CheckRecord],
        cancel: &AtomicBool,
    ) -> Result<CapturedConversation> {
        let checks = if matches!(
            action,
            CompanionAction::ReviewAction | CompanionAction::Reveal
        ) {
            self.store.lock().unwrap().action_check_journal(session)?
        } else {
            checks.to_vec()
        };
        if matches!(
            action,
            CompanionAction::ReviewAction | CompanionAction::Reveal
        ) && let Some(saved) = self
            .store
            .lock()
            .unwrap()
            .current_action_attachment(session)?
            && !self
                .inner
                .attachments
                .iter()
                .any(|command| command.id == saved.id)
        {
            self.inner.attachments.push(saved);
        }
        let mut captured = self
            .inner
            .capture(session, action, language, guidance, &checks, cancel)?;
        if let Some(review) = captured.review.take() {
            captured.review = Some(authenticate_review(
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
pub fn authenticate_review(
    store: &Store,
    session: &TaskSession,
    mut input: ActionReviewInput,
    language: &str,
    guidance: &EffectiveGuidance,
) -> Result<ActionReviewInput> {
    // Native capture restores the latest immutable current-scope attachment.
    // Keep it alongside fresh harness receipts; never retag or rewrite it.
    if let Some(saved) = store.current_action_attachment(session)?
        && !input.commands.iter().any(|command| command.id == saved.id)
    {
        input.commands.push(saved);
    }
    let mut commands = vec![];
    for mut command in input.commands.drain(..) {
        if let CommandProvenance::LocalCheck { check_id, .. } = &command.provenance {
            let Some(origin) = store.check_action_scope(check_id)? else {
                continue;
            };
            if !origin.is_eligible_check_origin(session) {
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
impl FixtureTask {
    pub fn clarification_facts(&self, argv: &[String]) -> Result<serde_json::Value> {
        self.verify_originals()?;
        let exists = match fs::symlink_metadata(self.project.root.join(&self.path)) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "Owned module is not an ordinary file"
                );
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        Ok(
            json!({"module_path":self.path,"module_exists":exists,"module_observation":"Actual live filesystem; an unsaved overlay does not establish a saved file","installed_check_argv":argv,"check_cwd":".","existing_check_source":if self.language=="rust"{"tests/contract.rs"}else{"test_contract.py"},"independent_fixture_case_count":7,"independent_fixture_cases":[[0,0,0],[0,9,0],[7,9,7],[9,9,9],[12,9,9],[1000000,1,1],[1,0,0]],"case_coverage":"Each listed tuple asserts both the named function and unchanged caller","separate_independent_runner":false,"remaining_evidence_gap":"No separate runner or additional executed evidence is claimed. Any acceptance cases beyond the immutable tests and supplied actual receipts remain unverified; do not invent a runner, change tests or weaken the approved documents."}),
        )
    }
    pub fn create(root: &Path, language: &str) -> Result<Self> {
        ensure!(!root.exists(), "Refusing to overwrite a fixture");
        fs::create_dir_all(root)?;
        let (path, recipe, wrong, alternative, files) = if language == "rust" {
            (
                "src/limits.rs",
                "cargo-test",
                "pub fn bounded_count(value: u32, ceiling: u32) -> u32 { value }\n",
                "pub fn bounded_count(value: u32, ceiling: u32) -> u32 { if ceiling < value { ceiling } else { value } }\n",
                vec![
                    (
                        "Cargo.toml",
                        "[package]\nname='tandem-bounded-fixture'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
                    ),
                    (
                        "Cargo.lock",
                        "version = 4\n[[package]]\nname = \"tandem-bounded-fixture\"\nversion = \"0.1.0\"\n",
                    ),
                    ("rust-toolchain.toml", "[toolchain]\nchannel='1.100.0'\n"),
                    (
                        "src/lib.rs",
                        "pub mod limits;\npub fn accepted(value: u32, ceiling: u32) -> u32 { limits::bounded_count(value, ceiling) }\n",
                    ),
                    (
                        "tests/contract.rs",
                        "#[test] fn contract() { for (value, ceiling, expected) in [(0,0,0),(0,9,0),(7,9,7),(9,9,9),(12,9,9),(1_000_000,1,1),(1,0,0)] { assert_eq!(tandem_bounded_fixture::limits::bounded_count(value, ceiling), expected); assert_eq!(tandem_bounded_fixture::accepted(value, ceiling), expected); } }\n",
                    ),
                ],
            )
        } else {
            ensure!(language == "python", "Only approved Rust/Python fixtures");
            (
                "limits.py",
                "python-unittest",
                "def bounded_count(value: int, ceiling: int) -> int:\n    return value\n",
                "def bounded_count(value: int, ceiling: int) -> int:\n    if ceiling < value:\n        return ceiling\n    return value\n",
                vec![
                    (
                        "pyproject.toml",
                        "[project]\nname='tandem-bounded-fixture'\nversion='0.1.0'\nrequires-python='>=3.10'\n",
                    ),
                    (
                        "consumer.py",
                        "from limits import bounded_count\ndef accepted(value, ceiling):\n    return bounded_count(value, ceiling)\n",
                    ),
                    (
                        "test_contract.py",
                        "import unittest\nfrom limits import bounded_count\nfrom consumer import accepted\nclass Contract(unittest.TestCase):\n    def test_contract(self):\n        for value, ceiling, expected in [(0,0,0),(0,9,0),(7,9,7),(9,9,9),(12,9,9),(1_000_000,1,1),(1,0,0)]:\n            self.assertEqual(bounded_count(value, ceiling), expected)\n            self.assertEqual(accepted(value, ceiling), expected)\n",
                    ),
                ],
            )
        };
        for (path, text) in files {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().unwrap())?;
            fs::write(destination, text)?;
        }
        fs::write(
            root.join("README.md"),
            "Private fixture context: ORCHID_INTERNAL. Add bounded_count(value, ceiling), returning the lesser for nonnegative integer inputs through 1,000,000. Keep the caller and contract tests unchanged. Create the missing module, then implement its symbol, then validate the caller. No dependency installation.\n",
        )?;
        fs::write(root.join(".gitignore"), "target/\n__pycache__/\n.env\n")?;
        fs::write(
            root.join(".env"),
            "fixture private sentinel, never for model research\n",
        )?;
        let git = |args: &[&str]| -> Result<()> {
            let output = std::process::Command::new("/usr/bin/git")
                .args(args)
                .current_dir(root)
                .output()?;
            ensure!(
                output.status.success(),
                "Fixture Git setup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(())
        };
        git(&["init", "-q"])?;
        git(&["add", "."])?;
        git(&[
            "-c",
            "user.name=Tandem fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "Original fixture contract",
        ])?;
        let project = Project::open(root)?;
        let original_hashes = tree_hashes(&project)?;
        write_json(
            &root
                .parent()
                .context("Fixture parent missing")?
                .join("original-hashes.json"),
            &original_hashes,
        )?;
        Ok(Self {
            project,
            language: language.into(),
            path: path.into(),
            recipe: recipe.into(),
            wrong: wrong.into(),
            alternative: alternative.into(),
            original_hashes,
            owned_module_hash: None,
        })
    }
    pub fn reopen_before_implementation(root: &Path, language: &str) -> Result<Self> {
        let project = Project::open(root)?;
        let output = std::process::Command::new("/usr/bin/git")
            .args(["diff", "--exit-code", "HEAD", "--"])
            .current_dir(root)
            .output()?;
        ensure!(
            output.status.success(),
            "Original tracked fixture changed before continuation"
        );
        let mut original_hashes = tree_hashes(&project)?;
        // The disposable human may save only the new module. Existing caller,
        // oracle, metadata and all unrelated paths remain immutable.
        original_hashes.remove(if language == "rust" {
            "src/limits.rs"
        } else {
            "limits.py"
        });
        let hash_path = root
            .parent()
            .context("Fixture parent missing")?
            .join("original-hashes.json");
        if hash_path.exists() {
            let saved: serde_json::Value = serde_json::from_slice(&fs::read(&hash_path)?)?;
            let saved = if saved.get("reconstructed_after_schema_rejection_before_implementation")
                == Some(&json!(true))
            {
                saved
                    .get("hashes")
                    .context("Reconstructed hash receipt missing hashes")?
            } else {
                &saved
            };
            ensure!(
                serde_json::from_value::<BTreeMap<String, String>>(saved.clone())?
                    == original_hashes,
                "Original fixture hashes changed"
            );
        } else {
            write_json(
                &hash_path,
                &json!({"reconstructed_after_schema_rejection_before_implementation":true,"hashes":original_hashes}),
            )?;
        }
        let (path, recipe, wrong, alternative) = if language == "rust" {
            (
                "src/limits.rs",
                "cargo-test",
                "pub fn bounded_count(value: u32, ceiling: u32) -> u32 { value }\n",
                "pub fn bounded_count(value: u32, ceiling: u32) -> u32 { if ceiling < value { ceiling } else { value } }\n",
            )
        } else {
            ensure!(language == "python", "Unknown fixture language");
            (
                "limits.py",
                "python-unittest",
                "def bounded_count(value: int, ceiling: int) -> int:\n    return value\n",
                "def bounded_count(value: int, ceiling: int) -> int:\n    if ceiling < value:\n        return ceiling\n    return value\n",
            )
        };
        let saves_path = root
            .parent()
            .context("Fixture parent missing")?
            .join("module-human-saves.json");
        let saves: Vec<serde_json::Value> = if saves_path.exists() {
            serde_json::from_slice(&fs::read(saves_path)?)?
        } else {
            vec![]
        };
        let owned_module_hash = saves
            .last()
            .and_then(|v| v["hash"].as_str())
            .map(str::to_owned);
        Ok(Self {
            project,
            language: language.into(),
            path: path.into(),
            recipe: recipe.into(),
            wrong: wrong.into(),
            alternative: alternative.into(),
            original_hashes,
            owned_module_hash,
        })
    }
    pub fn guidance(&self) -> EffectiveGuidance {
        GuidanceSettings::default().effective(self.project.key(), &self.language, None)
    }
    pub fn verify_originals(&self) -> Result<()> {
        let mut current = tree_hashes(&self.project)?;
        current.remove(&self.path);
        ensure!(
            current == self.original_hashes,
            "Immutable fixture caller/oracles or unrelated paths changed"
        );
        let module = self.project.root.join(&self.path);
        let actual = if module.exists() {
            Some(file_hash(&module)?)
        } else {
            None
        };
        ensure!(
            actual == self.owned_module_hash,
            "Owned module differs from last explicit human save baseline"
        );
        Ok(())
    }
    pub fn record_human_save(&mut self, saved: &Document) -> Result<()> {
        ensure!(
            saved.path == self.path && !saved.dirty(),
            "Only an ordinary saved owned-module document can update its human baseline"
        );
        let hash = file_hash(&self.project.root.join(&self.path))?;
        ensure!(
            saved.base_hash.as_deref() == Some(hash.as_str()),
            "Saved module baseline mismatch"
        );
        let path = self
            .project
            .root
            .parent()
            .context("Fixture parent missing")?
            .join("module-human-saves.json");
        let mut saves: Vec<serde_json::Value> = if path.exists() {
            serde_json::from_slice(&fs::read(&path)?)?
        } else {
            vec![]
        };
        saves.push(json!({"path":self.path,"hash":hash,"at_ms":now_ms(),"choice":"Explicit harness human ordinary editor save; no research/check mutation authority"}));
        write_json(&path, &saves)?;
        self.owned_module_hash = Some(hash);
        self.verify_originals()
    }
}
fn tree_hashes(project: &Project) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, directory: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                visit(root, &path, out)?;
            } else {
                out.insert(
                    path.strip_prefix(root)?.to_string_lossy().into_owned(),
                    file_hash(&path)?,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(&project.root, &project.root, &mut out)?;
    Ok(out)
}

/// These are explicit human harness choices, not model execution authority.
pub fn fixture_git_command(argv: &[String], task: &FixtureTask) -> bool {
    if argv == ["/usr/bin/git", "status", "--short", "--untracked-files=all"] {
        return true;
    }
    if argv == ["/usr/bin/git", "diff", "--exit-code", "HEAD", "--"] {
        return true;
    }
    argv.len() > 5
        && argv[..3] == ["/usr/bin/git", "diff", "--exit-code"]
        && argv[3].len() == 40
        && argv[3].bytes().all(|b| b.is_ascii_hexdigit())
        && argv[4] == "--"
        && argv[5..]
            .iter()
            .all(|p| task.original_hashes.contains_key(p))
}
pub fn select_fixture_command(
    action: &mut GuidedAction,
    recipe: &tandem_checks::Recipe,
    task: &FixtureTask,
) -> Result<bool> {
    let ActionTarget::Command {
        argv,
        cwd,
        expected_exit_codes,
    } = &action.target
    else {
        return Ok(false);
    };
    ensure!(
        cwd == "." && expected_exit_codes == &[0],
        "Unsupported fixture command location or exit contract"
    );
    if argv == &recipe.argv {
        let receipt = RequiredActionEvidence::CheckReceipt {
            recipe_id: recipe.id.clone(),
        };
        if !action.required_evidence.contains(&receipt) {
            action.required_evidence.push(receipt);
            return Ok(true);
        }
    } else {
        ensure!(
            fixture_git_command(argv, task),
            "No explicit harness authority for this command"
        );
    }
    Ok(false)
}
/// Execute only the fixed read-only fixture forms. Captured Git data excludes
/// repo config/hooks/helpers; the live source is hash-checked before and after.
pub fn fixture_git_output(
    task: &FixtureTask,
    argv: &[String],
    directory: &Path,
) -> Result<std::process::Output> {
    ensure!(
        fixture_git_command(argv, task),
        "Unsupported fixture Git command"
    );
    task.verify_originals()?;
    let git_dir = directory.join(format!("human-git-{}", new_id()));
    let captured = tandem_checks::capture_git(&task.project, &git_dir, &AtomicBool::new(false))?
        .context("Fixture Git metadata missing")?;
    let mut command = std::process::Command::new("/usr/bin/git");
    command
        .args(&argv[1..])
        .current_dir(&task.project.root)
        .env_clear()
        .envs([
            ("PATH", "/usr/bin:/bin"),
            ("LANG", "C.UTF-8"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_CONFIG_COUNT", "5"),
            ("GIT_CONFIG_KEY_0", "core.fsmonitor"),
            ("GIT_CONFIG_VALUE_0", "false"),
            ("GIT_CONFIG_KEY_1", "core.hooksPath"),
            ("GIT_CONFIG_VALUE_1", "/dev/null"),
            ("GIT_CONFIG_KEY_2", "core.attributesFile"),
            ("GIT_CONFIG_VALUE_2", "/dev/null"),
            ("GIT_CONFIG_KEY_3", "diff.external"),
            ("GIT_CONFIG_VALUE_3", ""),
            ("GIT_CONFIG_KEY_4", "core.pager"),
            ("GIT_CONFIG_VALUE_4", "cat"),
        ])
        .env("GIT_DIR", &git_dir)
        .env("GIT_WORK_TREE", &task.project.root);
    let output = command.output()?;
    ensure!(
        captured.unchanged(),
        "Git metadata changed during inspection"
    );
    ensure!(
        output.stdout.len() + output.stderr.len() <= 64 * 1024,
        "Fixture Git output exceeds evidence bound"
    );
    task.verify_originals()?;
    Ok(output)
}

/// Preserve the historical quarantine; classify it only from immutable official
/// thread/read diagnostic bytes, never from a model's assertion about tools.
pub fn historical_compaction_only(run: &ModelRun, directory: &Path) -> Result<bool> {
    if run.status != ModelStatus::Quarantined || run.tool_items == 0 {
        return Ok(false);
    }
    let items: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(
        directory.join(format!("diagnosed-tool-items-private-{}.json", run.id)),
    )?)?;
    let diagnostics: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(
        directory.join(format!("diagnosed-contract-{}.json", run.id)),
    )?)?;
    Ok(items.len() == run.tool_items
        && items.iter().all(|item| {
            item["type"] == "contextCompaction"
                && item["id"].as_str().is_some_and(|id| !id.is_empty())
                && item.as_object().is_some_and(|object| {
                    object.keys().all(|key| {
                        ["id", "type", "startedAtMs", "completedAtMs"].contains(&key.as_str())
                    })
                })
                && serde_json::to_vec(item).is_ok_and(|bytes| {
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic["run_id"] == run.id
                            && diagnostic["unexpected_item_type"] == "contextCompaction"
                            && diagnostic["item_hash"] == content_hash(&bytes)
                    })
                })
        }))
}

/// Re-run the approved baseline comparisons for each cumulative action scope;
/// attachments from consumed actions are historical and cannot be retagged.
pub fn fixture_preservation_commands(
    walkthroughs: &[Walkthrough],
    task: &FixtureTask,
) -> Vec<Vec<String>> {
    let mut commands = vec![
        ["/usr/bin/git", "diff", "--exit-code", "HEAD", "--"]
            .map(str::to_owned)
            .to_vec(),
        ["/usr/bin/git", "status", "--short", "--untracked-files=all"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for action in walkthroughs.iter().flat_map(|walk| &walk.actions) {
        if let ActionTarget::Command { argv, .. } = &action.target
            && fixture_git_command(argv, task)
            && !commands.contains(argv)
        {
            commands.push(argv.clone());
        }
    }
    commands
}
