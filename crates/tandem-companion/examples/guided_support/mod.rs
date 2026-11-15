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
        Self::open_inner(path, None)
    }
    pub fn open_authorized(
        path: &Path,
        authorization_path: &Path,
        repo: &Path,
        plan: &Path,
    ) -> Result<Self> {
        let bytes =
            fs::read(authorization_path).context("Explicit budget authorization unavailable")?;
        let authorization: BudgetAuthorization = serde_json::from_slice(&bytes)?;
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
        Self::open_inner(path, Some((authorization, content_hash(&bytes))))
    }
    fn open_inner(
        path: &Path,
        authorization: Option<(BudgetAuthorization, String)>,
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
        "crates/tandem-companion/src/conversation.rs",
        "crates/tandem-companion/src/runtime.rs",
        "crates/tandem-companion/src/public_research.rs",
        "crates/tandem-companion/examples/guided_journeys.rs",
        "crates/tandem-companion/examples/guided_support/mod.rs",
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
    let mut session = CompanionSession::connect()?;
    let catalog = session.list_models()?;
    let model = tandem_companion::resolve_model(&catalog, &ModelPreference::Automatic)?;
    write_json(
        &root.join("prerequisites.json"),
        &json!({"identity":identity()?,"connection":session.connection,"catalog":catalog,"selected":model,"model_turns":0}),
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
        write_json(
            &self.directory.join(format!("dispatch-{id}.json")),
            &json!({"action":run.request.action,"model":run.request.resolved_model,"schema_hash":content_hash(&serde_json::to_vec(&run.request.schema())?),"identity":identity()?}),
        )?;
        let result = self.inner.run(run, thread, cancel, |run| {
            write_json(&self.directory.join(format!("private-{id}.json")), run)?;
            observe(run)
        })?;
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
