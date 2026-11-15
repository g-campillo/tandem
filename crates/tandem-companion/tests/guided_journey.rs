//! Whole coordinator journeys. Fake protocol/HTTP are S evidence, never R/M.
#[path = "../examples/guided_support/mod.rs"]
mod support;

use anyhow::{Result, bail};
use serde_json::Value;
use serde_json::json;
use std::{
    collections::VecDeque,
    net::IpAddr,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tandem_checks::{InspectionExecutor, InspectionOutput, PreparedInspection};
use tandem_codex::ProtocolExchange;
use tandem_companion::*;
use tandem_core::*;

#[test]
fn subscription_guard_refuses_before_any_connection_or_artifact() {
    assert!(support::require_subscription_usage(["guided_journeys"]).is_err());
    assert!(
        support::require_subscription_usage(["research_probe", "--allow-subscription-usage"])
            .is_ok()
    );
}

#[test]
fn every_generated_output_schema_requires_nullable_fields_and_closed_objects() {
    fn strict(value: &serde_json::Value, path: &str) {
        if let Some(object) = value.as_object() {
            if let Some(properties) = object
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&json!(false)),
                    "{path}: open objects are not strict response schemas"
                );
                let required = object
                    .get("required")
                    .and_then(serde_json::Value::as_array)
                    .expect("all schema properties must be required (nullable where optional)");
                for key in properties.keys() {
                    assert!(
                        required.contains(&json!(key)),
                        "{path}: optional {key} omitted from strict schema required"
                    );
                }
            }
            if object.get("type").and_then(serde_json::Value::as_str) == Some("object") {
                assert!(
                    object.get("properties").is_some(),
                    "{path}: arbitrary map output is unsupported"
                );
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&json!(false)),
                    "{path}: every schema object must be closed"
                );
            }
            assert!(
                !object.contains_key("oneOf"),
                "{path}: strict response schema uses anyOf, not oneOf"
            );
            for keyword in [
                "allOf",
                "not",
                "dependentRequired",
                "dependentSchemas",
                "if",
                "then",
                "else",
                "default",
            ] {
                assert!(
                    !object.contains_key(keyword),
                    "{path}: unsupported schema keyword {keyword}"
                );
            }
            for (key, child) in object {
                strict(child, &format!("{path}.{key}"));
            }
        } else if let Some(array) = value.as_array() {
            for (index, child) in array.iter().enumerate() {
                strict(child, &format!("{path}[{index}]"));
            }
        }
    }
    let f = Journey::new("rust");
    for action in [
        CompanionAction::Discuss,
        CompanionAction::DraftSpec,
        CompanionAction::DraftPlan,
        CompanionAction::PrepareWalkthrough,
        CompanionAction::ReviewAction,
        CompanionAction::Analyze,
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Explain,
        CompanionAction::Ask,
        CompanionAction::Reveal,
        CompanionAction::Review,
        CompanionAction::Delegate,
        CompanionAction::ImproveClarity,
        CompanionAction::ImproveSpeed,
        CompanionAction::ImproveBalanced,
    ] {
        let request = ModelRequest {
            model_preference: ModelPreference::Automatic,
            resolved_model: None,
            conversation_id: Some(f.session.clone()),
            guided: None,
            action,
            question: "Schema contract only".into(),
            language: "rust".into(),
            guidance: f.task.guidance(),
            task: None,
            context: ContextSnapshot {
                id: "capture".into(),
                project_fingerprint: "fingerprint".into(),
                files: vec![],
                tree: vec![],
                omitted: vec![],
                captured_at_ms: 0,
                validation_environments: Default::default(),
            },
            evidence: vec![],
            prior_decisions: vec![],
            delegation: None,
        };
        let schema = request.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema.get("anyOf").is_none());
        let reply_schema = if action == CompanionAction::Discuss {
            Some(&schema)
        } else {
            schema.pointer("/$defs/ConversationReply")
        };
        if let Some(reply_schema) = reply_schema {
            assert_eq!(
                reply_schema["properties"]["research_requests"]["maxItems"], 4,
                "{action:?}: provider schema must enforce the host's four-request bound"
            );
            assert_eq!(
                reply_schema["properties"]["findings"]["maxItems"], 32,
                "{action:?}: provider schema must enforce the host's finding bound"
            );
        }
        if action == CompanionAction::Discuss {
            assert!(
                schema["properties"]["question"]["anyOf"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["type"] == "null"),
                "Optional question remains nullable"
            );
        }
        strict(&schema, &format!("{action:?}"));
    }
}

#[test]
fn durable_caps_include_public_subturns_and_survive_reopen_without_refunds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("budget.json");
    let mut budget = support::TurnBudget::open(&path).unwrap();
    for _ in 0..4 {
        budget.reserve("probe", "public").unwrap();
    }
    assert!(budget.reserve("probe", "private").is_err());
    drop(budget);
    let mut budget = support::TurnBudget::open(&path).unwrap();
    for language in ["rust", "python"] {
        for i in 0..24 {
            budget
                .reserve(language, if i % 2 == 0 { "private" } else { "public" })
                .unwrap();
        }
        assert!(budget.reserve(language, "public").is_err());
    }
    assert_eq!(budget.total(), 52);
    assert!(budget.reserve("unknown", "private").is_err());
    drop(budget);
    assert_eq!(support::TurnBudget::open(&path).unwrap().total(), 52);
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(
        support::TurnBudget::open(&path).is_err(),
        "never reset a damaged budget"
    );
}

#[test]
fn only_one_process_and_one_diagnosed_continuation_can_own_the_same_budget() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("budget.json");
    let mut budget = support::TurnBudget::open(&path).unwrap();
    assert!(support::TurnBudget::open(&path).is_err());
    budget.claim("rust").unwrap();
    assert!(budget.claim("rust").is_err());
    budget.reserve("rust", "private").unwrap();
    budget.finish("rust", "failed").unwrap();
    budget.resume_schema_failure("rust").unwrap();
    assert!(budget.resume_schema_failure("rust").is_err());
    budget.reserve("rust", "private").unwrap();
    drop(budget);
    let mut budget = support::TurnBudget::open(&path).unwrap();
    assert_eq!(budget.total(), 2);
    assert!(budget.claim("rust").is_err());
    assert!(budget.resume_schema_failure("rust").is_err());
    assert!(budget.resume_research_question_failure("rust").is_err());
    budget.finish("rust", "failed").unwrap();
    budget.resume_research_question_failure("rust").unwrap();
    assert!(budget.resume_research_question_failure("rust").is_err());
    budget.reserve("rust", "private").unwrap();
    assert_eq!(budget.total(), 3);
}

#[test]
fn public_vocabulary_continuation_keeps_the_original_per_language_cap() {
    let temp = tempfile::tempdir().unwrap();
    let mut budget = support::TurnBudget::open(&temp.path().join("budget.json")).unwrap();
    budget.claim("python").unwrap();
    for _ in 0..10 {
        budget.reserve("python", "private").unwrap();
    }
    assert!(budget.resume_public_vocabulary_failure("python").is_err());
    budget.finish("python", "failed").unwrap();
    budget.resume_public_vocabulary_failure("python").unwrap();
    assert!(budget.resume_public_vocabulary_failure("python").is_err());
    for _ in 0..14 {
        budget.reserve("python", "private").unwrap();
    }
    assert_eq!(budget.total(), 24);
    assert!(budget.reserve("python", "public").is_err());
    assert!(budget.resume_public_vocabulary_failure("python").is_err());
    for (language, used) in [("rust", 11), ("python", 13)] {
        let mut budget =
            support::TurnBudget::open(&temp.path().join(format!("{language}-remaining.json")))
                .unwrap();
        budget.claim(language).unwrap();
        for _ in 0..used {
            budget.reserve(language, "private").unwrap();
        }
        budget.finish(language, "failed").unwrap();
        budget.resume_independent_checks(language).unwrap();
        assert!(budget.resume_independent_checks(language).is_err());
        for _ in used..24 {
            budget.reserve(language, "private").unwrap();
        }
        assert_eq!(budget.total(), 24);
        assert!(budget.reserve(language, "public").is_err());
    }
    let mut budget =
        support::TurnBudget::open(&temp.path().join("python-draft-only.json")).unwrap();
    budget.claim("python").unwrap();
    for _ in 0..21 {
        budget.reserve("python", "private").unwrap();
    }
    budget.finish("python", "failed").unwrap();
    budget.resume_python_draft_only().unwrap();
    for _ in 0..3 {
        budget.reserve("python", "private").unwrap();
    }
    assert!(budget.reserve("python", "public").is_err());
    budget.finish("python", "failed").unwrap();
    assert!(budget.resume_python_draft_only().is_err());
}

type Answer = Box<dyn FnMut(&Value) -> Value + Send>;
#[derive(Default)]
struct Peer {
    answers: VecDeque<Answer>,
    requests: Vec<(String, Value)>,
    events: VecDeque<Value>,
    threads: usize,
    turns: usize,
    public: bool,
    model: String,
}
#[derive(Clone)]
struct Wire(Arc<Mutex<Peer>>);
impl ProtocolExchange for Wire {
    fn request(&mut self, method: &str, params: Value, _: Duration) -> Result<Value> {
        let mut peer = self.0.lock().unwrap();
        peer.requests.push((method.into(), params.clone()));
        Ok(match method {
            "account/read" => json!({"account":{"type":"chatgpt"}}),
            "account/rateLimits/read" => json!({"ordinaryUsageAllowed":true}),
            "model/list" => {
                json!({"data":[{"id":format!("entry-{}",peer.model),"model":peer.model,"displayName":"Selected","description":"Fixture","hidden":false,"isDefault":true,"inputModalities":["text"],"supportedReasoningEfforts":[{"reasoningEffort":"medium","description":"Default"}],"defaultReasoningEffort":"medium"}],"nextCursor":null})
            }
            "thread/start" | "thread/resume" => {
                let id = if method == "thread/start" {
                    peer.threads += 1;
                    format!("thread-{}", peer.threads)
                } else {
                    params["threadId"].as_str().unwrap().into()
                };
                json!({"thread":{"id":id},"model":params["model"],"modelProvider":"openai","reasoningEffort":params.pointer("/config/model_reasoning_effort"),"instructionSources":[]})
            }
            "turn/start" => {
                peer.turns += 1;
                let turn = format!("turn-{}", peer.turns);
                let thread = params["threadId"].clone();
                peer.events.push_back(
                    json!({"method":"turn/started","params":{"threadId":thread,"turnId":turn}}),
                );
                if peer.public {
                    for method in ["item/started", "item/completed"] {
                        peer.events.push_back(json!({"method":method,"params":{"threadId":thread,"turnId":turn,"item":{"type":"webSearch","id":"search-1","query":"Rust Iterator documentation","action":{"type":"search","query":"Rust Iterator documentation","queries":null}}}}));
                    }
                }
                let answer = if peer.public {
                    json!({"candidates":["https://example.org/docs"]}).to_string()
                } else {
                    peer.answers.pop_front().expect("unexpected model dispatch")(&params)
                        .to_string()
                };
                let item = json!({"type":"agentMessage","text":answer});
                peer.events.push_back(json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,"item":item}}));
                peer.events.push_back(json!({"method":"turn/completed","params":{"threadId":thread,"turnId":turn,"turn":{"id":turn,"status":"completed"}}}));
                json!({"turn":{"id":turn}})
            }
            "turn/interrupt" => json!({}),
            _ => bail!("unexpected protocol method {method}"),
        })
    }
    fn poll_event(&mut self, _: Duration) -> Result<Option<Value>> {
        Ok(self.0.lock().unwrap().events.pop_front())
    }
    fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct Http {
    urls: Vec<String>,
    html: Option<String>,
}
impl PublicHttpTransport for Http {
    fn resolve(&mut self, _: &str, _: &AtomicBool, _: Instant) -> Result<Vec<IpAddr>> {
        Ok(vec!["93.184.216.34".parse().unwrap()])
    }
    fn get(
        &mut self,
        request: &PublicHttpRequest,
        _: &AtomicBool,
        _: Instant,
        receive: &mut dyn FnMut(&PublicHttpResponse, &[u8]) -> Result<()>,
    ) -> Result<PublicHttpResponse> {
        self.urls.push(request.url().to_string());
        let response = PublicHttpResponse {
            status: 200,
            content_type: Some(
                if self.html.is_some() {
                    "text/html"
                } else {
                    "text/plain"
                }
                .into(),
            ),
            content_encoding: None,
            location: None,
        };
        receive(
            &response,
            self.html
                .as_ref()
                .map(String::as_bytes)
                .unwrap_or(b"Iterators yield successive values."),
        )?;
        Ok(response)
    }
}
#[derive(Default)]
struct Inspect;
impl InspectionExecutor for Inspect {
    fn execute(
        &mut self,
        command: &PreparedInspection,
        _: &AtomicBool,
        started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
    ) -> Result<InspectionOutput> {
        started(&command.provenance())?;
        // Test adapter executes only the host-prepared Git argv against the
        // captured repository. It is account-free; OS sandbox proof is separate.
        anyhow::ensure!(
            command.argv()[0] == "/usr/bin/git",
            "Unexpected fixture tool"
        );
        let output = std::process::Command::new(&command.argv()[0])
            .args(&command.argv()[1..])
            .env_clear()
            .envs(command.environment())
            .current_dir(command.cwd())
            .output()?;
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        Ok(InspectionOutput {
            exit_code: output.status.code().map(i64::from),
            output: bytes,
            ..Default::default()
        })
    }
}

struct Capture {
    inner: ProjectConversationCapture,
    recipe: String,
    store: Arc<Mutex<Store>>,
}
impl ConversationCapture for Capture {
    fn capture(
        &mut self,
        session: &TaskSession,
        action: CompanionAction,
        language: &str,
        guidance: &EffectiveGuidance,
        checks: &[CheckRecord],
        cancel: &AtomicBool,
    ) -> Result<CapturedConversation> {
        let mut captured = self
            .inner
            .capture(session, action, language, guidance, checks, cancel)?;
        captured
            .context
            .validation_environments
            .insert(self.recipe.clone(), "simulated-recipe".into());
        if let Some(review) = captured.review.take() {
            let commands = collect_action_check_evidence(
                review.scope(),
                &captured.context,
                checks,
                &[(self.recipe.clone(), "simulated-recipe".into())],
            );
            let input = ActionReviewInput::capture(
                session,
                captured.context.clone(),
                review.targets,
                commands,
                language,
                guidance,
            )?;
            captured.review = Some(support::authenticate_review(
                &self.store.lock().unwrap(),
                session,
                input,
                language,
                guidance,
            )?);
        }
        Ok(captured)
    }
}
type Runner = ConversationRunner<CompanionSession<Wire>, PublicDataWorker<Wire, Http>, Inspect>;
struct Journey {
    temp: tempfile::TempDir,
    task: support::FixtureTask,
    store: Arc<Mutex<Store>>,
    session: String,
    capture: Capture,
    peer: Arc<Mutex<Peer>>,
    public: Arc<Mutex<Peer>>,
    inspection: tandem_checks::InspectionEnvironment,
}
impl Journey {
    fn new(language: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let task =
            support::FixtureTask::create(&temp.path().join("private_project"), language).unwrap();
        let mut store = Store::open(&temp.path().join("state")).unwrap();
        let session = store.create_session(task.project.key()).unwrap().id;
        let store = Arc::new(Mutex::new(store));
        let checks = tandem_checks::CheckEnvironment {
            runtime: PathBuf::from("/fake-runtime-unused-by-inspection-adapter"),
            tools: None,
            python: None,
            runs: temp.path().join("inspection-runs"),
            cargo_cache: temp.path().join("cargo-cache"),
            clang: PathBuf::from("/unused-linker"),
            sdk: None,
            denied_roots: vec![],
        };
        let inspection = tandem_checks::InspectionEnvironment::configured(
            checks,
            &ProjectTools::default(),
            Some(tandem_checks::InspectionTool {
                executable: PathBuf::from("/usr/bin/git"),
                fingerprint: file_hash(std::path::Path::new("/usr/bin/git")).unwrap(),
                read_roots: vec![],
            }),
        )
        .unwrap();
        let capture = Capture {
            store: store.clone(),
            recipe: task.recipe.clone(),
            inner: ProjectConversationCapture {
                project: task.project.clone(),
                overlays: vec![],
                selected: vec!["README.md".into()],
                cache: temp.path().join("captures"),
                check_environment: None,
                recipe_ids: vec![],
                attachments: vec![],
            },
        };
        Self {
            temp,
            task,
            store,
            session,
            capture,
            inspection,
            peer: Arc::new(Mutex::new(Peer {
                model: "model-a".into(),
                ..Default::default()
            })),
            public: Arc::new(Mutex::new(Peer {
                model: "model-a".into(),
                public: true,
                ..Default::default()
            })),
        }
    }
    fn runner(&self) -> Runner {
        ConversationRunner::new(
            self.store.clone(),
            CompanionSession::with_exchange(
                Wire(self.peer.clone()),
                PathBuf::from("/private-scratch"),
            ),
            PublicDataWorker {
                search: PublicResearchSession::with_exchange(
                    Wire(self.public.clone()),
                    PathBuf::from("/public-scratch"),
                ),
                sources: PublicSourceFetcher::with_transport(Http::default()),
            },
            ResearchBroker::with_executor(Inspect),
        )
    }
    fn reopen(self) -> Self {
        let directory = self.temp.path().join("state");
        drop(self.capture.store);
        drop(self.store);
        let store = Arc::new(Mutex::new(Store::open(&directory).unwrap()));
        let mut inner = self.capture.inner;
        inner.overlays = store
            .lock()
            .unwrap()
            .working_documents(self.task.project.key(), &[])
            .unwrap();
        Self {
            capture: Capture {
                store: store.clone(),
                inner,
                ..self.capture
            },
            store,
            ..self
        }
    }
    fn state(&self) -> TaskSession {
        self.store
            .lock()
            .unwrap()
            .load_session(&self.session)
            .unwrap()
            .unwrap()
    }
    fn human(&self, commands: Vec<SessionCommand>) {
        let s = self.state();
        self.store
            .lock()
            .unwrap()
            .apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(new_id(), commands),
            )
            .unwrap();
    }
    fn reply(&self, value: Value) {
        self.dynamic(move |_| value.clone());
    }
    fn dynamic(&self, f: impl FnMut(&Value) -> Value + Send + 'static) {
        self.peer.lock().unwrap().answers.push_back(Box::new(f));
    }
    fn input(&self, id: &str, intent: ConversationIntent) -> ConversationInput {
        ConversationInput {
            id: id.into(),
            session_id: self.session.clone(),
            intent,
            language: self.task.language.clone(),
            guidance: self.task.guidance(),
            model_preference: ModelPreference::Automatic,
        }
    }
    fn run(&mut self, runner: &mut Runner, intent: ConversationIntent) -> ConversationResult {
        runner
            .run(
                self.input(&new_id(), intent),
                &mut self.capture,
                Some(&self.inspection),
                Arc::new(AtomicBool::new(false)),
                |_| Ok(()),
            )
            .unwrap()
    }
    fn review(&mut self, runner: &mut Runner, negative: bool) -> ActionReview {
        self.dynamic(move |p| assessment(&guided(p), negative));
        let result = self.run(runner, ConversationIntent::ReviewAction);
        completed(&result);
        result.review.unwrap()
    }
    fn current_input(&mut self) -> ActionReviewInput {
        let s = self.state();
        let checks = self.store.lock().unwrap().checks(&s.project).unwrap();
        self.capture
            .capture(
                &s,
                CompanionAction::ReviewAction,
                &self.task.language,
                &self.task.guidance(),
                &checks,
                &AtomicBool::new(false),
            )
            .unwrap()
            .review
            .unwrap()
    }
    fn next(&mut self, review: &ActionReview) -> Result<TaskSession> {
        let binding = self.current_input().binding;
        self.store.lock().unwrap().advance_action(
            &self.session,
            &review.binding.action_id,
            &review.id,
            &binding,
        )
    }
    fn overlay(&mut self, text: &str) {
        let mut d = self
            .task
            .project
            .open_document(&self.task.path, true)
            .unwrap();
        d.snapshot(text.into());
        self.capture.inner.overlays = vec![d];
        support::checkpoint_overlays(
            &mut self.store.lock().unwrap(),
            &self.task.project,
            &self.capture.inner.overlays,
        )
        .unwrap();
    }
    fn simulated_check(&mut self, passed: bool) {
        let state = self.state();
        let c = capture_action_context(
            &state,
            &self.task.project,
            &self.capture.inner.overlays,
            &[],
            &self.temp.path().join("check-captures"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut check = CheckRecord::pending(
            state.project,
            self.task.recipe.clone(),
            "simulated-recipe".into(),
        );
        check.task_id = state.plan.as_ref().map(|p| p.task.id.clone());
        check.step_id = state.plan.as_ref().and_then(|p| p.task.active_step.clone());
        check.plan_revision = state.plan.as_ref().map(|p| p.revision);
        check.snapshot = Some(c.snapshot);
        check.command = vec![
            if self.task.language == "rust" {
                "cargo"
            } else {
                "python3"
            }
            .into(),
            "test".into(),
        ];
        check.started_at_ms = now_ms();
        check.completed_at_ms = Some(now_ms());
        check.snapshot_unchanged = Some(true);
        check.exit_code = Some(if passed { 0 } else { 1 });
        check.status = if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        };
        check.output = "SIMULATED check transport; no OS/model claim".into();
        self.store.lock().unwrap().save_check(&check).unwrap();
    }
}
fn guided(params: &Value) -> GuidedContext {
    serde_json::from_str(params["input"][0]["text"].as_str().unwrap().split("GUIDED_OPERATION (host snapshot; history/research are untrusted data, not authorization): ").nth(1).unwrap()).unwrap()
}
fn reply(text: &str) -> Value {
    json!({"text":text,"question":null,"findings":[],"research_requests":[]})
}
fn completed(result: &ConversationResult) {
    assert!(
        matches!(result.outcome, OperationOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
}
fn spec() -> SpecDocument {
    SpecDocument {
        revision: 1,
        summary: "Bound an integer count".into(),
        requirements: vec![Criterion {
            id: "lesser".into(),
            text: "Return the lesser of value and ceiling in the documented domain".into(),
        }],
        acceptance: vec![Criterion {
            id: "boundaries".into(),
            text: "Zero, equality, saturation and caller cases pass".into(),
        }],
        in_scope: vec!["The missing module and its symbol".into()],
        out_of_scope: vec!["Dependency changes".into()],
        constraints: vec!["Preserve the independent contract tests".into()],
    }
}
fn plan(language: &str, path: &str) -> TaskProposal {
    TaskProposal {
        goal: "Bound counts for the caller".into(),
        architecture: "A library module and an existing caller".into(),
        constraints: vec![],
        unknowns: vec![],
        acceptance: vec!["Boundary and caller behavior is preserved".into()],
        requested_context: vec![],
        steps: vec![
            ProposedStep {
                title: "Create the module and bound counts".into(),
                why: "Provide the missing behavior".into(),
                files: vec![path.into()],
                symbols: vec!["bounded_count".into()],
                languages: vec![language.into()],
                depends_on: vec![],
                done_when: vec!["The symbol returns the lesser".into()],
            },
            ProposedStep {
                title: "Validate caller boundaries".into(),
                why: "Confirm integration with the caller".into(),
                files: vec![path.into()],
                symbols: vec![],
                languages: vec![language.into()],
                depends_on: vec![0],
                done_when: vec!["Boundary and caller checks pass".into()],
            },
        ],
    }
}
fn validation(s: &TaskSession, task: bool) -> GuidedAction {
    let p = s.plan.as_ref().unwrap();
    let step = p
        .task
        .active_step
        .clone()
        .unwrap_or_else(|| p.task.steps.last().unwrap().id.clone());
    let refs = if task {
        s.spec
            .as_ref()
            .unwrap()
            .acceptance
            .iter()
            .map(|c| CriterionRef::SpecAcceptance { id: c.id.clone() })
            .collect::<Vec<_>>()
    } else {
        p.task
            .steps
            .iter()
            .find(|x| x.id == step)
            .unwrap()
            .done_when
            .iter()
            .enumerate()
            .map(|(index, _)| CriterionRef::StepDoneWhen {
                step_id: step.clone(),
                index,
            })
            .collect()
    };
    GuidedAction {
        id: if task { "full-spec" } else { "step-validation" }.into(),
        revision: 1,
        kind: if task {
            GuidedActionKind::ValidateTask
        } else {
            GuidedActionKind::ValidateStep
        },
        what: "Assess the cumulative acceptance conditions".into(),
        why: "Check approved behavior".into(),
        expected_result: "Every named criterion is satisfied".into(),
        target: ActionTarget::Validation {
            spec_ref: p.spec_ref.clone(),
            plan_ref: p.document_ref(),
            step_id: if task { None } else { Some(step) },
            criterion_refs: refs.clone(),
            paths: p.task.steps.iter().flat_map(|x| x.files.clone()).collect(),
        },
        required_evidence: refs
            .into_iter()
            .map(|criterion_ref| RequiredActionEvidence::CriterionAssessment { criterion_ref })
            .collect(),
    }
}
fn actions(s: &TaskSession, path: &str, language: &str) -> Vec<GuidedAction> {
    let p = s.plan.as_ref().unwrap();
    if p.task
        .steps
        .iter()
        .all(|x| x.status == StepStatus::Complete)
    {
        return vec![validation(s, true)];
    }
    let mut out = vec![];
    if p.task.active_step.as_ref() == Some(&p.task.steps[0].id) {
        for (id, symbol, change_kind) in [
            ("create-module", None, FileChangeKind::Create),
            (
                "implement-symbol",
                Some("bounded_count".into()),
                FileChangeKind::Modify,
            ),
        ] {
            out.push(GuidedAction {
                id: id.into(),
                revision: 1,
                kind: GuidedActionKind::Edit,
                what: if symbol.is_none() {
                    "Create the missing module"
                } else {
                    "Implement the bounded count symbol"
                }
                .into(),
                why: "Meet the approved interface".into(),
                expected_result: if symbol.is_none() {
                    "The module exists"
                } else {
                    "The symbol implements the approved behavior"
                }
                .into(),
                target: ActionTarget::File {
                    path: path.into(),
                    symbol,
                    change_kind,
                },
                required_evidence: vec![RequiredActionEvidence::FileSnapshot { path: path.into() }],
            });
        }
        out.push(GuidedAction {
            id: "run-check".into(),
            revision: 1,
            kind: GuidedActionKind::Command,
            what: "Run the independent contract checks".into(),
            why: "Check actual behavior".into(),
            expected_result: "The checks pass".into(),
            target: ActionTarget::Command {
                argv: vec![
                    if language == "rust" {
                        "cargo"
                    } else {
                        "python3"
                    }
                    .into(),
                    "test".into(),
                ],
                cwd: ".".into(),
                expected_exit_codes: vec![0],
            },
            required_evidence: vec![RequiredActionEvidence::CommandReceipt],
        });
    }
    out.push(validation(s, false));
    out
}
fn assessment(g: &GuidedContext, negative: bool) -> Value {
    let i = g.review.as_ref().unwrap();
    let (_, a) = g.session.current_action().unwrap();
    let ids = i
        .binding
        .evidence
        .iter()
        .map(|e| e.id.clone())
        .collect::<Vec<_>>();
    let assessments=a.required_evidence.iter().filter_map(|e|if let RequiredActionEvidence::CriterionAssessment{criterion_ref}=e{Some(json!({"criterion_ref":criterion_ref,"satisfied":!negative,"explanation":"The captured interface and independent evidence meet the condition.","evidence_ids":ids}))}else{None}).collect::<Vec<_>>();
    json!({"reply":reply("The current action has been assessed."),"assessment":{"outcome":if negative{"NeedsChanges"}else{"Passed"},"summary":if negative{"The current behavior ignores the ceiling"}else{"The captured action meets its criterion"},"findings":if negative{json!([{"problem":"Values above the ceiling remain unbounded","why":"The approved saturation requirement would fail","next_action":"Correct the comparison in the named symbol","blocking":true,"criterion_refs":[],"evidence_ids":ids}])}else{json!([])},"evidence_used":ids,"limitations":[],"assessments":assessments}})
}

#[test]
fn rust_and_python_complete_coordinator_journeys_preserve_history_and_advance_exactly_once() {
    for language in ["rust", "python"] {
        let mut f = Journey::new(language);
        let mut r = f.runner();
        assert!(f.state().spec.is_none());
        f.reply(reply(
            "We can discuss the intended contract before drafting.",
        ));
        completed(&f.run(
            &mut r,
            ConversationIntent::Message(
                "Discuss the missing count limit; implementation and solutions stay hidden.".into(),
            ),
        ));
        for (id, custom) in [("domain", false), ("boundary", true)] {
            let mut question = reply("Clarify the intended input domain.");
            question["question"] = json!({"id":id,"prompt":"Which domain?","choices":[{"id":"bounded","label":"Bounded","description":"Nonnegative bounded integers"},{"id":"all","label":"All","description":"All integers"}]});
            f.reply(question);
            completed(&f.run(&mut r, ConversationIntent::Discuss));
            drop(r);
            f = f.reopen();
            r = f.runner();
            assert_eq!(f.state().pending_question.unwrap().id, id);
            f.reply(reply(
                "The explicit answer is preserved; no implementation is authorized.",
            ));
            completed(&f.run(
                &mut r,
                ConversationIntent::Answer(ClarificationAnswer {
                    question_id: id.into(),
                    choice_id: if custom { None } else { Some("bounded".into()) },
                    custom_text: if custom {
                        Some("From zero through one million, including equality.".into())
                    } else {
                        None
                    },
                }),
            ));
        }
        let ecosystem = if language == "rust" { "Rust" } else { "Python" };
        let mut research =
            reply("I will inspect the captured documentation and declared dependencies.");
        research["research_requests"] = json!([{"type":"ReadFileRange","path":"README.md","start_line":1,"end_line":1},{"type":"DependencyMetadata","ecosystem":ecosystem},{"type":"PublicSearch","query":"ORCHID_INTERNAL Rust documentation"},{"type":"PublicSearch","query":"integer minimum official documentation"}]);
        f.reply(research);
        let mut page = reply("I will retrieve the public source before citing it.");
        page["research_requests"] = json!([{"type":"GitHistory","path":null,"limit":2},{"type":"PublicPage","url":"https://example.org/docs"}]);
        f.reply(page);
        f.dynamic(|params| {let g=guided(params);let receipt=g.receipts.iter().find(|r|r.public_web.as_ref().is_some_and(|w|w.body_hash.is_some())).unwrap();let source=&receipt.sources[0];let mut answer=reply("Local documentation and the public receipt establish the contract context.");answer["findings"]=json!([{"summary":"The source documents iterative values","citations":[{"receipt_id":receipt.id,"source_id":source.id,"locator":source.locator}]}]);answer});
        completed(&f.run(&mut r, ConversationIntent::Discuss));
        f.task.verify_originals().unwrap();
        let public = f.public.lock().unwrap();
        let sent = serde_json::to_string(&public.requests).unwrap();
        assert!(
            !sent.contains("ORCHID_INTERNAL")
                && !sent.contains("README.md")
                && !sent.contains(&f.session)
                && !sent.contains(".env")
        );
        assert_eq!(public.turns, 1);
        let search = public
            .requests
            .iter()
            .find(|(method, _)| method == "turn/start")
            .unwrap();
        assert_eq!(
            content_hash(&serde_json::to_vec(&search.1["outputSchema"]).unwrap()),
            support::public_search_schema_hash()
        );
        drop(public);
        let events = all_conversation_events(&f.store, &f.session).unwrap();
        assert!(
            events
                .iter()
                .filter_map(|event| {
                    if let ConversationPayload::ResearchReceiptRecorded { receipt_id } =
                        &event.payload
                    {
                        f.store
                            .lock()
                            .unwrap()
                            .research_receipt(&f.session, receipt_id)
                            .unwrap()
                    } else {
                        None
                    }
                })
                .any(|receipt| matches!(
                    receipt.request.kind,
                    ResearchRequestKind::GitHistory { .. }
                ) && receipt.status == ResearchReceiptStatus::Complete
                    && !receipt.content.trim().is_empty()),
            "The coordinator must persist actual captured Git history, not an unavailable or invented finding"
        );
        assert!(events.iter().any(|e| matches!(
            &e.payload,
            ConversationPayload::ResearchActivity {
                activity: ResearchActivity::PublicSearchObserved { .. }
            }
        )));
        drop(r);
        f = f.reopen();
        r = f.runner();
        assert!(f.state().plan.is_none());
        f.reply(json!({"reply":reply("Here is a spec proposal to inspect."),"spec":spec()}));
        let proposal = f.run(&mut r, ConversationIntent::DraftSpec);
        completed(&proposal);
        assert!(f.state().spec.is_none());
        let mut saved = spec();
        saved
            .constraints
            .push("Human fixture edit: preserve caller semantics".into());
        f.human(vec![SessionCommand::SaveSpec { spec: saved }]);
        let s = f.state();
        f.human(vec![SessionCommand::ApproveSpec {
            document_ref: s.spec.unwrap().document_ref(),
            approved_at_ms: now_ms(),
        }]);
        f.reply(json!({"reply":reply("Here is the dependency-aware plan proposal."),"plan":plan(language,&f.task.path)}));
        completed(&f.run(&mut r, ConversationIntent::DraftPlan));
        assert!(f.state().plan.is_none());
        let s = f.state();
        let mut task = plan(language, &f.task.path)
            .into_plan(&s.project, None)
            .unwrap();
        task.steps[1].why = "Human fixture edit: protect callers and boundary cases".into();
        f.human(vec![SessionCommand::SavePlan {
            plan: PlanDocument {
                revision: 1,
                spec_ref: s.spec.unwrap().document_ref(),
                task,
            },
        }]);
        assert!(f.state().spec_approval.is_some());
        assert!(f.state().authorize(WorkflowAction::ReviewAction).is_err());
        let s = f.state();
        f.human(vec![SessionCommand::ApprovePlan {
            document_ref: s.plan.unwrap().document_ref(),
            approved_at_ms: now_ms(),
        }]);
        assert!(
            f.state()
                .authorize(WorkflowAction::PrepareWalkthrough)
                .is_err()
        );
        f.human(vec![SessionCommand::StartImplementation]);
        let mut saw_symbol = false;
        let mut saw_command = false;
        let mut reviews = 0;
        while !f.state().implementation_complete() {
            if f.state().current_action().is_err() {
                let path = f.task.path.clone();
                let lang = f.task.language.clone();
                f.dynamic(move|p|json!({"reply":reply("Take one tiny action at a time."),"actions":actions(&guided(p).session,&path,&lang)}));
                completed(&f.run(&mut r, ConversationIntent::PrepareWalkthrough));
            }
            let a = f.state().current_action().unwrap().1.clone();
            match &a.target {
                ActionTarget::File { symbol: None, .. } => {
                    let missing = f.review(&mut r, false);
                    assert_eq!(missing.outcome, ActionReviewOutcome::NeedsChanges);
                    assert!(f.next(&missing).is_err());
                    f.overlay(if language == "rust" {
                        "// Module shell, human fixture choice\n"
                    } else {
                        "# Module shell, human fixture choice\n"
                    });
                }
                ActionTarget::File {
                    symbol: Some(_), ..
                } => {
                    saw_symbol = true;
                    let wrong = f.task.wrong.clone();
                    f.overlay(&wrong);
                    let bad = f.review(&mut r, true);
                    assert_eq!(bad.outcome, ActionReviewOutcome::NeedsChanges);
                    assert!(f.next(&bad).is_err());
                    let valid = f.task.alternative.clone();
                    f.overlay(&valid);
                }
                ActionTarget::Command { .. } => {
                    saw_command = true;
                    let missing = f.review(&mut r, false);
                    assert_eq!(missing.outcome, ActionReviewOutcome::InsufficientEvidence);
                    f.simulated_check(false);
                    let bad = f.review(&mut r, false);
                    assert_eq!(bad.outcome, ActionReviewOutcome::NeedsChanges);
                    let valid = format!("{}\n", f.task.alternative);
                    f.overlay(&valid);
                    f.simulated_check(true);
                }
                _ => {}
            }
            let review = f.review(&mut r, false);
            assert_eq!(review.outcome, ActionReviewOutcome::Passed);
            reviews += 1;
            if reviews == 1 {
                let old_binding = review.binding.clone();
                f.overlay("// changed after review\n");
                assert!(f.next(&review).is_err());
                assert_ne!(f.current_input().binding, old_binding);
                f.overlay(if language == "rust" {
                    "// Module shell, human fixture choice\n"
                } else {
                    "# Module shell, human fixture choice\n"
                });
                drop(r);
                f = f.reopen();
                r = f.runner();
            }
            let before = f.state();
            let next = f.next(&review).unwrap();
            let duplicate = f.next_with_binding(&review).unwrap();
            assert_eq!(next.sequence, duplicate.sequence);
            assert!(next.sequence > before.sequence);
        }
        assert!(saw_symbol && saw_command);
        assert!(reviews >= 6);
        f.task.verify_originals().unwrap();
        let before = f.peer.lock().unwrap().threads;
        f.peer.lock().unwrap().model = "model-b".into();
        f.reply(reply(
            "The saved session history remains authoritative after changing models.",
        ));
        completed(&f.run(&mut r, ConversationIntent::Discuss));
        assert!(f.peer.lock().unwrap().threads > before);
        let runs = f
            .store
            .lock()
            .unwrap()
            .model_runs(f.task.project.key())
            .unwrap();
        assert!(
            runs.iter()
                .all(|r| r.request.action != CompanionAction::Reveal && r.tool_items == 0)
        );
        let events = all_conversation_events(&f.store, &f.session).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(&e.payload, ConversationPayload::QuestionAnswered { .. }))
                .count(),
            2
        );
    }
}
impl Journey {
    fn next_with_binding(&self, review: &ActionReview) -> Result<TaskSession> {
        self.store.lock().unwrap().advance_action(
            &self.session,
            &review.binding.action_id,
            &review.id,
            &review.binding,
        )
    }
}

#[test]
fn cancellation_and_process_restart_never_replay_model_work_or_lose_the_human_message() {
    let mut f = Journey::new("python");
    let mut r = f.runner();
    let cancel = Arc::new(AtomicBool::new(false));
    let slot = r.operations.clone();
    let result = r
        .run(
            f.input(
                "cancelled",
                ConversationIntent::Message(
                    "Preserve this question while cancellation settles.".into(),
                ),
            ),
            &mut f.capture,
            None,
            cancel,
            |op| {
                if op.phase == OperationPhase::Discussing {
                    assert!(slot.cancel(&op.id));
                }
                Ok(())
            },
        )
        .unwrap();
    assert!(matches!(result.outcome, OperationOutcome::Cancelled { .. }));
    assert_eq!(f.peer.lock().unwrap().turns, 0);
    f.human(vec![SessionCommand::BeginOperation {
        operation: SessionOperation {
            id: "interrupted".into(),
            session_id: f.session.clone(),
            kind: SessionOperationKind::Discuss,
            started_at_ms: now_ms(),
        },
    }]);
    drop(r);
    f = f.reopen();
    let mut r = f.runner();
    assert!(matches!(
        f.store
            .lock()
            .unwrap()
            .session_operation_outcome("interrupted")
            .unwrap(),
        Some(OperationOutcome::Interrupted { .. })
    ));
    f.reply(reply("The previous interrupted work was not replayed."));
    completed(&f.run(&mut r, ConversationIntent::Discuss));
    assert_eq!(f.peer.lock().unwrap().turns, 1);
    assert!(all_conversation_events(&f.store,&f.session).unwrap().iter().any(|e|matches!(&e.payload,ConversationPayload::HumanMessage{text} if text.contains("Preserve this question"))));
}

#[test]
fn mixed_research_and_question_researches_first_publishes_one_question_and_no_artifact() {
    for (action, intent) in [
        (CompanionAction::Discuss, ConversationIntent::Discuss),
        (CompanionAction::DraftSpec, ConversationIntent::DraftSpec),
        (CompanionAction::DraftPlan, ConversationIntent::DraftPlan),
        (
            CompanionAction::PrepareWalkthrough,
            ConversationIntent::PrepareWalkthrough,
        ),
        (
            CompanionAction::ReviewAction,
            ConversationIntent::ReviewAction,
        ),
    ] {
        let mut f = Journey::new("rust");
        let mut r = f.runner();
        prepare_typed(&mut f, &mut r, action);
        let original = f.state();
        let original_walks = serde_json::to_value(&original.walkthroughs).unwrap();
        f.dynamic(move|params|{
            let g=guided(params);let mut response=typed_artifact(&g,action);
            let mut mixed=reply("Inspect these sources, then clarify the intended domain.");
            mixed["question"]=json!({"id":"bounded_count_input_domain","prompt":"Does the range constrain both inputs?","choices":[{"id":"bounded_domain","label":"Both inputs bounded","description":"Both are zero through one million"},{"id":"nonnegative_domain","label":"All nonnegative inputs","description":"The range is only for tests"}]});
            mixed["research_requests"]=json!([{"type":"ReadFileRange","path":"README.md","start_line":1,"end_line":1},{"type":"PublicSearch","query":"ORCHID_INTERNAL minimum"},{"type":"PublicPage","url":"https://example.org/docs"}]);
            if action==CompanionAction::Discuss{mixed}else{response["reply"]=mixed;response}
        });
        let store = f.store.clone();
        let session = f.session.clone();
        let turns = f.peer.lock().unwrap().turns;
        let result = r
            .run(
                f.input("mixed", intent),
                &mut f.capture,
                Some(&f.inspection),
                Arc::new(AtomicBool::new(false)),
                |op| {
                    if matches!(
                        op.phase,
                        OperationPhase::ReadingSource | OperationPhase::PublicRetrieval
                    ) {
                        assert!(
                            store
                                .lock()
                                .unwrap()
                                .load_session(&session)?
                                .unwrap()
                                .pending_question
                                .is_none(),
                            "Question cannot be exposed before research settles"
                        );
                    }
                    Ok(())
                },
            )
            .unwrap();
        completed(&result);
        assert_eq!(
            f.peer.lock().unwrap().turns,
            turns + 1,
            "No extra model turn is required to stage the question"
        );
        assert!(result.review.is_none() && result.walkthrough.is_none());
        let state = f.state();
        assert_eq!(
            state.pending_question.unwrap().id,
            "bounded_count_input_domain"
        );
        assert_eq!(
            serde_json::to_value(&state.walkthroughs).unwrap(),
            original_walks
        );
        assert_eq!(state.execution_generation, original.execution_generation);
        assert_eq!(state.started_generation, original.started_generation);
        assert_eq!(
            state.spec.as_ref().map(SpecDocument::document_ref),
            original.spec.as_ref().map(SpecDocument::document_ref)
        );
        assert_eq!(
            state.plan.as_ref().map(PlanDocument::document_ref),
            original.plan.as_ref().map(PlanDocument::document_ref)
        );
        let events = all_conversation_events(&f.store, &f.session).unwrap();
        let question=events.iter().find(|e|matches!(&e.payload,ConversationPayload::QuestionAsked{question} if question.id=="bounded_count_input_domain")).unwrap();
        let receipts = events
            .iter()
            .filter(|e| {
                matches!(
                    &e.payload,
                    ConversationPayload::ResearchReceiptRecorded { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 3);
        assert!(receipts.iter().all(|e| e.sequence < question.sequence));
        assert!(
            !events
                .iter()
                .any(|e| matches!(&e.payload, ConversationPayload::DocumentProposed { .. }))
        );
        assert_eq!(
            f.public.lock().unwrap().turns,
            0,
            "Private query was denied; direct public GET uses no search turn"
        );
        f.task.verify_originals().unwrap();
        drop(store);
        drop(r);
        f = f.reopen();
        r = f.runner();
        f.dynamic(move |p| typed_artifact(&guided(p), action));
        let resumed = f.run(
            &mut r,
            ConversationIntent::Answer(ClarificationAnswer {
                question_id: "bounded_count_input_domain".into(),
                choice_id: Some("bounded_domain".into()),
                custom_text: None,
            }),
        );
        completed(&resumed);
        assert!(f.state().pending_question.is_none());
        if action == CompanionAction::PrepareWalkthrough {
            assert!(resumed.walkthrough.is_some());
        }
        if action == CompanionAction::ReviewAction {
            assert!(resumed.review.is_some());
        }
    }
}
fn typed_artifact(g: &GuidedContext, action: CompanionAction) -> Value {
    match action {
        CompanionAction::Discuss => {
            reply("The explicit answer is recorded; discussion grants no implementation authority.")
        }
        CompanionAction::DraftSpec => {
            json!({"reply":reply("Inspect this spec proposal."),"spec":spec()})
        }
        CompanionAction::DraftPlan => {
            json!({"reply":reply("Inspect this plan proposal."),"plan":plan("rust","src/limits.rs")})
        }
        CompanionAction::PrepareWalkthrough => {
            json!({"reply":reply("Use these atomic actions."),"actions":actions(&g.session,"src/limits.rs","rust")})
        }
        CompanionAction::ReviewAction => assessment(g, false),
        _ => unreachable!(),
    }
}
fn prepare_typed(f: &mut Journey, r: &mut Runner, action: CompanionAction) {
    if action == CompanionAction::Discuss || action == CompanionAction::DraftSpec {
        return;
    }
    f.human(vec![SessionCommand::SaveSpec { spec: spec() }]);
    if action == CompanionAction::DraftPlan {
        return;
    }
    let s = f.state();
    f.human(vec![SessionCommand::SavePlan {
        plan: PlanDocument {
            revision: 1,
            spec_ref: s.spec.unwrap().document_ref(),
            task: plan("rust", "src/limits.rs")
                .into_plan(&s.project, None)
                .unwrap(),
        },
    }]);
    let s = f.state();
    f.human(vec![
        SessionCommand::ApproveSpec {
            document_ref: s.spec.unwrap().document_ref(),
            approved_at_ms: now_ms(),
        },
        SessionCommand::ApprovePlan {
            document_ref: s.plan.unwrap().document_ref(),
            approved_at_ms: now_ms(),
        },
        SessionCommand::StartImplementation,
    ]);
    if action == CompanionAction::ReviewAction {
        f.dynamic(|p| typed_artifact(&guided(p), CompanionAction::PrepareWalkthrough));
        completed(&f.run(r, ConversationIntent::PrepareWalkthrough));
    }
}

#[test]
fn mixed_research_cancellation_keeps_receipts_but_never_exposes_a_question_or_artifact() {
    for stop in [
        OperationPhase::ReadingSource,
        OperationPhase::PublicRetrieval,
    ] {
        let mut f = Journey::new("rust");
        let mut r = f.runner();
        let mut mixed = reply("Read these sources before clarifying.");
        mixed["question"] = json!({"id":"domain","prompt":"Which domain?","choices":[{"id":"bounded","label":"Bounded","description":"Bounded inputs"},{"id":"all","label":"All","description":"All inputs"}]});
        mixed["research_requests"] = json!([{"type":"ReadFileRange","path":"README.md","start_line":1,"end_line":1},{"type":"PublicPage","url":"https://example.org/docs"}]);
        f.reply(json!({"reply":mixed,"spec":spec()}));
        let slot = r.operations.clone();
        let result = r
            .run(
                f.input("mixed-cancel", ConversationIntent::DraftSpec),
                &mut f.capture,
                Some(&f.inspection),
                Arc::new(AtomicBool::new(false)),
                |op| {
                    if op.phase == stop {
                        assert!(slot.cancel(&op.id));
                    }
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(result.outcome, OperationOutcome::Cancelled { .. }));
        assert!(result.response.is_none());
        let state = f.state();
        assert!(state.pending_question.is_none() && state.spec.is_none() && state.plan.is_none());
        let events = all_conversation_events(&f.store, &f.session).unwrap();
        assert!(!events.iter().any(|e| matches!(
            &e.payload,
            ConversationPayload::QuestionAsked { .. }
                | ConversationPayload::DocumentProposed { .. }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    &e.payload,
                    ConversationPayload::ResearchReceiptRecorded { .. }
                ))
                .count(),
            usize::from(stop == OperationPhase::PublicRetrieval)
        );
        f.task.verify_originals().unwrap();
    }
}

#[test]
fn explicit_official_documentation_urls_remain_public_while_distinctive_private_paths_are_withheld()
{
    for url in [
        "https://doc.rust-lang.org/std/cmp/fn.min.html",
        "https://docs.python.org/3/library/functions.html",
    ] {
        let mut f = Journey::new("rust");
        let mut r = f.runner();
        let mut research = reply("Retrieve the supplied public documentation.");
        research["research_requests"] = json!([{"type":"PublicPage","url":url},{"type":"PublicPage","url":format!("{url}/ORCHID_INTERNAL")},{"type":"PublicSearch","query":"ORCHID_INTERNAL documentation"}]);
        f.reply(research);
        f.reply(reply(
            "The public source and withheld private requests are visible in their actual receipts.",
        ));
        completed(&f.run(&mut r,ConversationIntent::Message(format!("Discuss the official public documentation at {url}. Keep project identifiers private."))));
        assert_eq!(
            r.public.sources.transport().urls,
            vec![url],
            "Generic documentation hostname words must not be reclassified as private identifiers"
        );
        let receipts = all_conversation_events(&f.store, &f.session)
            .unwrap()
            .into_iter()
            .filter_map(|e| {
                if let ConversationPayload::ResearchReceiptRecorded { receipt_id } = e.payload {
                    f.store
                        .lock()
                        .unwrap()
                        .research_receipt(&f.session, &receipt_id)
                        .unwrap()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r.status == ResearchReceiptStatus::Complete
                    && r.public_web.as_ref().is_some_and(|w| w.body_hash.is_some()))
                .count(),
            1
        );
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r.status == ResearchReceiptStatus::Denied)
                .count(),
            2
        );
        assert_eq!(f.public.lock().unwrap().turns, 0);
    }
}

#[test]
fn requested_html_fragment_returns_its_relevant_section_with_original_download_and_text_limits() {
    let html = format!(
        "<html><head><title>Functions</title></head><body><p>{}</p><dl><dt id='min'>min(iterable)</dt><dt>min(first, second)</dt><dd>Return the smallest item.<script>hidden()</script><span hidden>private hidden text</span></dd><dt id='max'>unrelated maximum</dt><dd>Unrelated following text</dd></dl></body></html>",
        "unrelated earlier text ".repeat(4000)
    );
    let mut fetcher = PublicSourceFetcher::with_transport(Http {
        urls: vec![],
        html: Some(html.clone()),
    });
    let request = ResearchRequest::new(ResearchRequestKind::PublicPage {
        url: "https://example.org/functions.html#min".into(),
    })
    .unwrap();
    let receipt = fetcher.fetch(request, &AtomicBool::new(false)).unwrap();
    assert_eq!(receipt.status, ResearchReceiptStatus::Complete);
    assert!(receipt.content.contains("Return the smallest item"));
    assert!(receipt.content.contains("min(first, second)"));
    for excluded in [
        "unrelated earlier",
        "Unrelated following",
        "unrelated maximum",
        "hidden()",
        "private hidden",
    ] {
        assert!(!receipt.content.contains(excluded), "{excluded}");
    }
    assert_eq!(
        fetcher.transport().urls,
        vec!["https://example.org/functions.html"]
    );
    assert_eq!(
        receipt.public_web.as_ref().unwrap().final_url,
        "https://example.org/functions.html#min"
    );
    assert_eq!(
        receipt.public_web.as_ref().unwrap().body_hash,
        Some(content_hash(html.as_bytes()))
    );
    assert!(receipt.content.len() < 1024);
    receipt.validate().unwrap();
    for fragment in ["missing", "hidden", "duplicate"] {
        let html = "<p id='duplicate'>first</p><p id='duplicate'>second</p><div hidden><section id='hidden'>never expose</section></div>";
        let mut fetcher = PublicSourceFetcher::with_transport(Http {
            urls: vec![],
            html: Some(html.into()),
        });
        let r = fetcher
            .fetch(
                ResearchRequest::new(ResearchRequestKind::PublicPage {
                    url: format!("https://example.org/docs#{fragment}"),
                })
                .unwrap(),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(r.status, ResearchReceiptStatus::Unavailable);
        assert!(r.content.is_empty() && r.sources.is_empty());
        assert!(r.omissions.iter().any(|o| o.reason.contains("section")));
    }
}

#[test]
fn html_sections_respect_heading_boundaries_containers_and_the_same_text_ceiling() {
    for (html,fragment,expected,excluded) in [
        ("<h2><span id='topic'></span>Topic</h2><p>Relevant paragraph</p><h3>Nested detail</h3><p>Relevant nested detail</p><h2>Other</h2><p>Outside section</p>".to_string(),"topic","Relevant nested detail","Outside section"),
        ("<section id='topic'><p>Relevant container</p></section><section>Outside section</section>".to_string(),"topic","Relevant container","Outside section"),
    ] {
        let mut fetcher=PublicSourceFetcher::with_transport(Http{urls:vec![],html:Some(html)});let receipt=fetcher.fetch(ResearchRequest::new(ResearchRequestKind::PublicPage{url:format!("https://example.org/docs#{fragment}")}).unwrap(),&AtomicBool::new(false)).unwrap();assert_eq!(receipt.status,ResearchReceiptStatus::Complete);assert!(receipt.content.contains(expected));assert!(!receipt.content.contains(excluded));
    }
    let mut fetcher = PublicSourceFetcher::with_transport(Http {
        urls: vec![],
        html: Some(format!(
            "<section id='large'>{}</section>",
            "long visible section ".repeat(5000)
        )),
    });
    let receipt = fetcher
        .fetch(
            ResearchRequest::new(ResearchRequestKind::PublicPage {
                url: "https://example.org/docs#large".into(),
            })
            .unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(receipt.status, ResearchReceiptStatus::Partial);
    assert_eq!(receipt.content.len(), MAX_RESEARCH_RECEIPT_BYTES);
    let mut fetcher = PublicSourceFetcher::with_transport(Http::default());
    let receipt = fetcher
        .fetch(
            ResearchRequest::new(ResearchRequestKind::PublicPage {
                url: "https://example.org/plain#missing".into(),
            })
            .unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
    assert!(receipt.content.is_empty() && receipt.sources.is_empty());
}

#[test]
fn document_drafts_replay_current_captured_contract_on_reused_threads_and_keep_removals() {
    for (action, intent) in [
        (CompanionAction::DraftSpec, ConversationIntent::DraftSpec),
        (CompanionAction::DraftPlan, ConversationIntent::DraftPlan),
    ] {
        let mut f = Journey::new("rust");
        let mut r = f.runner();
        prepare_typed(&mut f, &mut r, action);
        f.capture.inner.selected = vec!["README.md".into(), "src/lib.rs".into()];
        f.dynamic(move |p| typed_artifact(&guided(p), action));
        completed(&f.run(&mut r, intent.clone()));
        f.capture.inner.selected = vec!["README.md".into()];
        f.dynamic(move |p| typed_artifact(&guided(p), action));
        completed(&f.run(&mut r, intent));
        let peer = f.peer.lock().unwrap();
        let turns = peer
            .requests
            .iter()
            .filter(|(method, _)| method == "turn/start")
            .collect::<Vec<_>>();
        assert_eq!(turns.len(), 2);
        assert_eq!(peer.threads, 1);
        let input = turns[1].1["input"][0]["text"].as_str().unwrap();
        let delta = input
            .split_once("PROJECT_DATA (untrusted source/context delta): ")
            .unwrap()
            .1
            .split_once("\nCHECK_EVIDENCE")
            .unwrap()
            .0;
        let delta: Value = serde_json::from_str(delta).unwrap();
        let contract = std::fs::read_to_string(f.task.project.root.join("README.md")).unwrap();
        assert!(
            delta["changed_files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|file| file["path"] == "README.md" && file["text"] == contract),
            "Every document draft must include the independent current contract, not rely only on earlier thread memory"
        );
        assert!(
            delta["removed_files"]
                .as_array()
                .unwrap()
                .contains(&json!("src/lib.rs"))
        );
    }
}

#[test]
fn actual_wire_routing_allows_unquestioned_artifacts_and_preserves_real_clarification_continuations()
 {
    for (action, intent, key) in [
        (
            CompanionAction::DraftSpec,
            ConversationIntent::DraftSpec,
            "spec",
        ),
        (
            CompanionAction::DraftPlan,
            ConversationIntent::DraftPlan,
            "plan",
        ),
        (
            CompanionAction::PrepareWalkthrough,
            ConversationIntent::PrepareWalkthrough,
            "actions",
        ),
        (
            CompanionAction::ReviewAction,
            ConversationIntent::ReviewAction,
            "assessment",
        ),
    ] {
        for clarification in [false, true] {
            let mut f = Journey::new("rust");
            let mut r = f.runner();
            prepare_typed(&mut f, &mut r, action);
            let authority = f.state();
            if clarification {
                let mut question =
                    reply("Clarify this actual choice before continuing typed work.");
                question["question"] = json!({"id":"domain-choice","prompt":"Which input domain?","choices":[{"id":"bounded","label":"Bounded","description":"Zero through one million"},{"id":"all","label":"All","description":"All nonnegative integers"}]});
                f.reply(json!({"reply":question,key:Value::Null}));
                let result = f.run(&mut r, intent.clone());
                completed(&result);
                assert!(result.walkthrough.is_none() && result.review.is_none());
                assert_eq!(f.state().pending_question.unwrap().id, "domain-choice");
                f.reply(reply(
                    "Ordinary chat leaves the actual clarification pending.",
                ));
                completed(&f.run(
                    &mut r,
                    ConversationIntent::Message(
                        "Discuss this while the choice is still pending.".into(),
                    ),
                ));
                let peer = f.peer.lock().unwrap();
                let turn = peer
                    .requests
                    .iter()
                    .rev()
                    .find(|(method, _)| method == "turn/start")
                    .unwrap();
                assert!(
                    turn.1["input"][0]["text"]
                        .as_str()
                        .unwrap()
                        .starts_with("APPLICATION_ACTION: Discuss\n")
                );
                assert!(guided(&turn.1).session.pending_question.is_some());
                assert!(turn.1["outputSchema"]["properties"].get("text").is_some());
                drop(peer);
                assert_eq!(f.state().answered_questions.len(), 0);
                f.dynamic(move |params| typed_artifact(&guided(params), action));
                completed(&f.run(
                    &mut r,
                    ConversationIntent::Answer(ClarificationAnswer {
                        question_id: "domain-choice".into(),
                        choice_id: Some("bounded".into()),
                        custom_text: None,
                    }),
                ));
                assert!(f.state().pending_question.is_none());
                assert_eq!(f.state().answered_questions.len(), 1);
            } else {
                f.dynamic(move |params| typed_artifact(&guided(params), action));
                completed(&f.run(&mut r, intent.clone()));
                assert!(f.state().pending_question.is_none());
                assert!(f.state().answered_questions.is_empty());
            }
            let peer = f.peer.lock().unwrap();
            let turn = peer
                .requests
                .iter()
                .rev()
                .find(|(method, _)| method == "turn/start")
                .unwrap();
            assert!(
                turn.1["input"][0]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("APPLICATION_ACTION: {action:?}\n"))
            );
            assert!(turn.1["outputSchema"]["properties"].get(key).is_some());
            assert!(guided(&turn.1).session.pending_question.is_none());
            let start = peer
                .requests
                .iter()
                .find(|(method, _)| method == "thread/start")
                .unwrap();
            let base = start.1["baseInstructions"].as_str().unwrap();
            assert!(
                !base.contains(
                    "Return null for any typed artifact until the human explicitly answers;"
                ),
                "No unconditional answer prerequisite may contradict a current typed request"
            );
            assert!(
                !start.1["developerInstructions"]
                    .as_str()
                    .unwrap()
                    .contains("Discuss")
            );
            drop(peer);
            let current = f.state();
            assert_eq!(current.execution_generation, authority.execution_generation);
            assert_eq!(current.started_generation, authority.started_generation);
            assert_eq!(
                current.spec_approval.as_ref().map(|a| &a.document_ref),
                authority.spec_approval.as_ref().map(|a| &a.document_ref)
            );
            assert_eq!(
                current.plan_approval.as_ref().map(|a| &a.document_ref),
                authority.plan_approval.as_ref().map(|a| &a.document_ref)
            );
            if action == CompanionAction::DraftSpec {
                assert!(current.spec.is_none());
            }
            if action == CompanionAction::DraftPlan {
                assert!(current.plan.is_none());
            }
            if let Ok((walk, _)) = current.current_action() {
                assert_eq!(
                    walk.cursor, 0,
                    "A response or explicit answer never advances an action"
                );
            }
        }
    }
}

#[test]
fn runtime_diagnostics_keep_schema_errors_but_withhold_credentials_login_urls_and_unclassified_text()
 {
    let message = "Invalid schema: text.format.schema invalid_json_schema Missing question";
    let safe =
        support::sanitized_runtime_error(&json!({"codexErrorInfo":"other","message":message}));
    assert_eq!(safe["message"], message);
    for message in [
        "https://auth.example.org/login?token=private-sentinel",
        "sk-abcdefghijklmnopqrstuv",
        "private fixture prompt must never be copied into error logs",
    ] {
        let safe = support::sanitized_runtime_error(
            &json!({"codexErrorInfo":"unauthorized","message":message,"additionalDetails":"private data"}),
        );
        assert_eq!(safe["message_withheld"], true);
        assert_eq!(safe["message_hash"], content_hash(message.as_bytes()));
        assert!(!safe.to_string().contains(message));
        assert!(safe.get("additionalDetails").is_none());
    }
}
