//! One bounded production-coordinator suite over disposable original fixtures.
//! Programmatic human decisions/code below are harness choices, not UI evidence.
mod guided_support;
use anyhow::{Context, Result, ensure};
use guided_support::*;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use tandem_checks::*;
use tandem_companion::*;
use tandem_core::*;

type Runner = ConversationRunner<BudgetBackend, BudgetPublic>;
struct Harness {
    task: FixtureTask,
    store: Arc<Mutex<Store>>,
    session: String,
    capture: StoredCapture,
    environment: CheckEnvironment,
    inspection: InspectionEnvironment,
    directory: PathBuf,
    choices: Vec<String>,
    saw_create: bool,
    saw_symbol: bool,
    saw_command: bool,
    rejected_wrong: bool,
    rejected_missing: bool,
    preference: ModelPreference,
}
impl Harness {
    fn new(directory: &Path, language: &str, resume: bool) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let task = if resume {
            FixtureTask::reopen_before_implementation(&directory.join("project"), language)?
        } else {
            FixtureTask::create(&directory.join("project"), language)?
        };
        let environment = CheckEnvironment::discover(&task.project.root, directory.join("checks"))?;
        let trust = ProjectTools {
            trusted_fingerprint: environment.tools.as_ref().map(|t| t.fingerprint.clone()),
            python_trusted_fingerprint: environment.python.as_ref().map(|t| t.fingerprint.clone()),
            ..Default::default()
        };
        let inspection = InspectionEnvironment::configured(
            environment.clone(),
            &trust,
            Some(InspectionTool {
                executable: PathBuf::from("/usr/bin/git"),
                fingerprint: file_hash(Path::new("/usr/bin/git"))?,
                read_roots: vec![],
            }),
        )?;
        let mut store = Store::open(&directory.join("state"))?;
        let session = if resume {
            store
                .selected_session(task.project.key())?
                .context("Original session missing")?
        } else {
            store.create_session(task.project.key())?.id
        };
        let store = Arc::new(Mutex::new(store));
        let overlays = store
            .lock()
            .unwrap()
            .working_documents(task.project.key(), &[])?;
        let overlays = supplemental::check_input_overlays(&task, overlays)?;
        let capture = StoredCapture {
            store: store.clone(),
            inner: ProjectConversationCapture {
                project: task.project.clone(),
                overlays,
                selected: if language == "rust" {
                    [
                        "README.md",
                        "src/limits.rs",
                        "src/lib.rs",
                        "tests/contract.rs",
                        "Cargo.lock",
                    ]
                    .map(str::to_owned)
                    .to_vec()
                } else {
                    [
                        "README.md",
                        "limits.py",
                        "consumer.py",
                        "test_contract.py",
                        supplemental::PYTHON_DIRECT_PATH,
                    ]
                    .map(str::to_owned)
                    .to_vec()
                },
                cache: directory.join("captures"),
                check_environment: Some(environment.clone()),
                recipe_ids: vec![task.recipe.clone()],
                attachments: vec![],
            },
        };
        let progress: serde_json::Value = if directory.join("action-progress.json").exists() {
            serde_json::from_slice(&fs::read(directory.join("action-progress.json"))?)?
        } else {
            json!({})
        };
        let mut choices: Vec<String> = if directory.join("harness-human-choices.json").exists() {
            serde_json::from_slice(&fs::read(directory.join("harness-human-choices.json"))?)?
        } else {
            vec![]
        };
        if language == "python" {
            choices.push(format!("Harness human supplies independent {} only as a check-input overlay; source hash {}; original live oracle/caller untouched. Same overlay is captured for checks and reviews; no executed evidence is inferred", supplemental::PYTHON_DIRECT_PATH, content_hash(supplemental::PYTHON_DIRECT_TESTS.as_bytes())));
            write_json(
                &directory.join(format!("supplemental-check-input-{}.json", new_id())),
                &json!({"class":"D harness input choice only","path":supplemental::PYTHON_DIRECT_PATH,"hash":content_hash(supplemental::PYTHON_DIRECT_TESTS.as_bytes()),"live_file_created":false,"existing_oracle_changed":false,"executed":false,"model_turns":0}),
            )?;
        }
        Ok(Self {
            task,
            store,
            session,
            capture,
            environment,
            inspection,
            directory: directory.into(),
            choices,
            preference: ModelPreference::Automatic,
            saw_create: progress["create_file"] == true,
            saw_symbol: progress["named_symbol"] == true,
            saw_command: progress["command"] == true,
            rejected_wrong: progress["wrong_rejected"] == true,
            rejected_missing: progress["missing_evidence_rejected"] == true,
        })
    }
    fn save_progress(&self) -> Result<()> {
        write_json(
            &self.directory.join("action-progress.json"),
            &json!({"create_file":self.saw_create,"named_symbol":self.saw_symbol,"command":self.saw_command,"wrong_rejected":self.rejected_wrong,"missing_evidence_rejected":self.rejected_missing}),
        )?;
        write_json(
            &self.directory.join("harness-human-choices.json"),
            &self.choices,
        )
    }
    fn runner(&self, budget: SharedBudget) -> Result<Runner> {
        Ok(ConversationRunner::new(
            self.store.clone(),
            BudgetBackend {
                inner: CompanionSession::connect()?,
                budget: budget.clone(),
                language: self.task.language.clone(),
                directory: self.directory.clone(),
                stop_duplicate_public_pages: matches!(&self.preference, ModelPreference::Explicit { model } if model == "gpt-6-astra"),
            },
            BudgetPublic::connect(budget, &self.task.language, &self.directory)?,
            ResearchBroker::default(),
        ))
    }
    fn state(&self) -> Result<TaskSession> {
        self.store
            .lock()
            .unwrap()
            .load_session(&self.session)?
            .context("Fixture session missing")
    }
    fn human(&mut self, description: &str, commands: Vec<SessionCommand>) -> Result<()> {
        self.choices.push(description.into());
        write_json(
            &self.directory.join("harness-human-choices.json"),
            &self.choices,
        )?;
        let s = self.state()?;
        self.store.lock().unwrap().apply_session_transition(
            &s.id,
            s.sequence,
            &SessionTransition::new(new_id(), commands),
        )?;
        Ok(())
    }
    fn input(&self, intent: ConversationIntent, preference: ModelPreference) -> ConversationInput {
        ConversationInput {
            id: new_id(),
            session_id: self.session.clone(),
            intent,
            language: self.task.language.clone(),
            guidance: self.task.guidance(),
            model_preference: preference,
        }
    }
    fn clarification_answer(&self) -> Result<String> {
        let recipe = self.environment.recipe(&self.task.recipe)?;
        let mut facts = self.task.clarification_facts(&recipe.argv)?;
        if self.task.language == "python" {
            facts["supplemental_independent_check_input"] = json!({"path":supplemental::PYTHON_DIRECT_PATH,"hash":content_hash(supplemental::PYTHON_DIRECT_TESTS.as_bytes()),"source":"Explicit harness human check-input overlay, present in captured snapshot only; never saved to the live project","case_count":9,"coverage":"Explicit expected values for zero, equality, both orders, upperboundary in both positions and integer type","runner":"The same installed python-unittest recipe discovers this additional module in its isolated snapshot; no separate installed runner or inline script","execution_status":"No success inferred; require actual matching CheckRecord before passing a review"});
        }
        Ok(format!(
            "{} Disposable harness human observations: {}",
            domain_answer(),
            serde_json::to_string(&facts)?
        ))
    }
    fn run(
        &mut self,
        runner: &mut Runner,
        intent: ConversationIntent,
    ) -> Result<ConversationResult> {
        let mut result = self.run_once(runner, intent)?;
        for _ in 0..2 {
            let unfinished = matches!(&result.response,Some(CompanionResponse::SpecDraft(draft)) if draft.spec.is_none())
                || matches!(&result.response,Some(CompanionResponse::PlanDraft(draft)) if draft.plan.is_none())
                || matches!(&result.response,Some(CompanionResponse::WalkthroughDraft(draft)) if draft.actions.is_none())
                || matches!(&result.response,Some(CompanionResponse::ActionAssessment(draft)) if draft.assessment.is_none());
            if !unfinished {
                return Ok(result);
            }
            let Some(question) = self.state()?.pending_question else {
                return Ok(result);
            };
            let answer = self.clarification_answer()?;
            self.choices.push(format!("Harness human answers actual clarification {} with the agreed domain, observed live module existence, installed recipe and exact immutable fixture coverage; missing evidence is not invented", question.id));
            result = self.run_once(
                runner,
                ConversationIntent::Answer(ClarificationAnswer {
                    question_id: question.id,
                    choice_id: None,
                    custom_text: Some(answer),
                }),
            )?;
        }
        Ok(result)
    }
    fn run_once(
        &mut self,
        runner: &mut Runner,
        intent: ConversationIntent,
    ) -> Result<ConversationResult> {
        let result = runner.run(
            self.input(intent, self.preference.clone()),
            &mut self.capture,
            Some(&self.inspection),
            Arc::new(AtomicBool::new(false)),
            |_| Ok(()),
        )?;
        write_json(
            &self
                .directory
                .join(format!("operation-{}.json", result.operation_id)),
            &json!({"operation_id":result.operation_id,"outcome":result.outcome,"historical":result.historical,"review":result.review,"walkthrough":result.walkthrough}),
        )?;
        ensure!(
            matches!(result.outcome, OperationOutcome::Completed { .. }),
            "Coordinator operation failed: {:?}",
            result.outcome
        );
        Ok(result)
    }
    fn research_kinds(&self) -> Result<std::collections::BTreeSet<String>> {
        let mut kinds = std::collections::BTreeSet::new();
        for event in all_conversation_events(&self.store, &self.session)? {
            if let ConversationPayload::ResearchReceiptRecorded { receipt_id } = event.payload {
                let receipt = self
                    .store
                    .lock()
                    .unwrap()
                    .research_receipt(&self.session, &receipt_id)?
                    .context("Missing persisted receipt")?;
                if !matches!(
                    receipt.status,
                    ResearchReceiptStatus::Complete | ResearchReceiptStatus::Partial
                ) {
                    continue;
                }
                let kind = match receipt.request.kind {
                    ResearchRequestKind::ReadFileRange { .. } => "doc",
                    ResearchRequestKind::DependencyMetadata { .. } => "dependencies",
                    ResearchRequestKind::GitHistory { .. } => "git",
                    ResearchRequestKind::PublicPage { .. }
                        if receipt
                            .public_web
                            .as_ref()
                            .is_some_and(|w| w.body_hash.is_some()) =>
                    {
                        "public"
                    }
                    _ => "other",
                };
                kinds.insert(kind.into());
            }
        }
        Ok(kinds)
    }
    fn structured_answers(&self) -> Result<usize> {
        Ok(all_conversation_events(&self.store, &self.session)?
            .iter()
            .filter(|event| matches!(&event.payload, ConversationPayload::QuestionAnswered { .. }))
            .count())
    }
    fn current_input(&mut self) -> Result<ActionReviewInput> {
        let s = self.state()?;
        let checks = self.store.lock().unwrap().checks(&s.project)?;
        self.capture
            .capture(
                &s,
                CompanionAction::ReviewAction,
                &self.task.language,
                &self.task.guidance(),
                &checks,
                &AtomicBool::new(false),
            )?
            .review
            .context("Missing action capture")
    }
    fn next(&mut self, review: &ActionReview) -> Result<()> {
        let current = self.current_input()?.binding;
        let mut store = self.store.lock().unwrap();
        let next = store.advance_action(
            &self.session,
            &review.binding.action_id,
            &review.id,
            &current,
        )?;
        let duplicate = store.advance_action(
            &self.session,
            &review.binding.action_id,
            &review.id,
            &current,
        )?;
        ensure!(next.sequence == duplicate.sequence, "Next advanced twice");
        Ok(())
    }
    fn overlay(&mut self, text: String) -> Result<()> {
        self.choices.push("Harness human enters original fixture code in an unsaved overlay; live source/tests stay unchanged".into());
        let mut document = self.task.project.open_document(&self.task.path, true)?;
        document.snapshot(text);
        self.capture.inner.overlays = vec![document];
        checkpoint_overlays(
            &mut self.store.lock().unwrap(),
            &self.task.project,
            &self.capture.inner.overlays,
        )?;
        self.capture.inner.overlays = supplemental::check_input_overlays(
            &self.task,
            std::mem::take(&mut self.capture.inner.overlays),
        )?;
        Ok(())
    }
    fn save_owned_module(&mut self, text: String) -> Result<()> {
        self.task.verify_originals()?;
        let mut document = self.task.project.open_document(&self.task.path, true)?;
        document.snapshot(text);
        let saved = self.task.project.save(&document)?;
        self.task.record_human_save(&saved)?;
        self.capture.inner.overlays = vec![saved];
        self.choices.push("Harness human explicitly saves only the owned fixture module using the ordinary editor save API; caller/oracles and other live files stay hash-immutable".into());
        checkpoint_overlays(
            &mut self.store.lock().unwrap(),
            &self.task.project,
            &self.capture.inner.overlays,
        )?;
        self.capture.inner.overlays = supplemental::check_input_overlays(
            &self.task,
            std::mem::take(&mut self.capture.inner.overlays),
        )?;
        self.task.verify_originals()
    }
    fn check(&self, expected: CheckStatus) -> Result<()> {
        let overlays =
            supplemental::check_input_overlays(&self.task, self.capture.inner.overlays.clone())?;
        ensure!(
            overlays.len() == self.capture.inner.overlays.len(),
            "Check/review supplemental input parity missing"
        );
        let recipe = self.environment.recipe(&self.task.recipe)?;
        let mut pending = CheckRecord::pending(
            self.task.project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        );
        let s = self.state()?;
        pending.task_id = s.plan.as_ref().map(|p| p.task.id.clone());
        pending.step_id = s.plan.as_ref().and_then(|p| p.task.active_step.clone());
        pending.plan_revision = s.plan.as_ref().map(|p| p.revision);
        self.store.lock().unwrap().save_check(&pending)?; // Origin BEFORE dispatch.
        let record = run_check(
            &self.environment,
            &self.task.project,
            &overlays,
            &recipe,
            pending,
            &AtomicBool::new(false),
            |r| self.store.lock().unwrap().save_check(r),
        )?;
        self.store.lock().unwrap().save_check(&record)?;
        write_json(
            &self.directory.join(format!("check-{}.json", record.id)),
            &record,
        )?;
        ensure!(
            record.status == expected,
            "Independent isolated check expected {expected:?}, got {:?}: {}\n{}",
            record.status,
            record.detail,
            record.output
        );
        self.task.verify_originals()
    }
    fn review(&mut self, runner: &mut Runner) -> Result<ActionReview> {
        self.run(runner, ConversationIntent::ReviewAction)?
            .review
            .context("No immutable action review")
    }
    fn attach_preservation(&mut self) -> Result<()> {
        self.task.verify_originals()?;
        self.capture.inner.attachments.clear();
        let state = self.state()?;
        let capture = capture_action_context(
            &state,
            &self.task.project,
            &self.capture.inner.overlays,
            &self.capture.inner.selected,
            &self.directory.join("captures"),
            &AtomicBool::new(false),
        )?;
        let scope = self.current_input()?.scope();
        for args in [
            vec!["diff", "--exit-code", "HEAD", "--"],
            vec!["status", "--short", "--untracked-files=all"],
        ] {
            let start = now_ms();
            let output = std::process::Command::new("/usr/bin/git")
                .args(&args)
                .current_dir(&self.task.project.root)
                .output()?;
            ensure!(
                output.status.success(),
                "Actual fixture preservation command failed"
            );
            let evidence = attach_action_output(
                scope.clone(),
                &capture,
                UserCommandAttachment {
                    argv: std::iter::once("/usr/bin/git")
                        .chain(args)
                        .map(str::to_owned)
                        .collect(),
                    cwd: ".".into(),
                    exit_code: output.status.code().unwrap_or(-1).into(),
                    stdout: String::from_utf8(output.stdout)?,
                    stderr: String::from_utf8(output.stderr)?,
                    started_at_ms: start,
                    completed_at_ms: now_ms(),
                },
            )?;
            write_json(
                &self
                    .directory
                    .join(format!("human-attached-preservation-{}.json", evidence.id)),
                &evidence,
            )?;
            self.capture.inner.attachments.push(evidence);
        }
        self.choices.push("Harness human runs fixed read-only Git diff/status in the owned fixture and explicitly attaches its real complete output as UserAttachedOutput/Unvalidated, not authenticated check evidence".into());
        self.task.verify_originals()
    }
    fn walkthrough(&mut self, runner: &mut Runner) -> Result<()> {
        let result = self.run(runner, ConversationIntent::PrepareWalkthrough)?;
        let mut walk = result
            .walkthrough
            .context("Model did not prepare atomic actions")?;
        // The harness explicitly selects its installed, separately authorized
        // recipe. Do not infer shell authority from model command metadata.
        let recipe = self.environment.recipe(&self.task.recipe)?;
        let mut changed = false;
        for a in &mut walk.actions {
            if let ActionTarget::Command {
                argv,
                cwd,
                expected_exit_codes,
            } = &mut a.target
            {
                *argv = recipe.argv.clone();
                *cwd = ".".into();
                *expected_exit_codes = vec![0];
                let receipt = RequiredActionEvidence::CheckReceipt {
                    recipe_id: self.task.recipe.clone(),
                };
                if !a.required_evidence.contains(&receipt) {
                    a.required_evidence.push(receipt);
                }
                changed = true;
            }
        }
        if changed {
            walk.id = new_id(); // Changed definitions never reuse an immutable walk ID.
            let s = self.state()?;
            self.choices.push("Harness human chooses the exact installed validation recipe for displayed commands; model metadata grants no execution authority".into());
            self.store.lock().unwrap().apply_session_transition(
                &s.id,
                s.sequence,
                &SessionTransition::new(
                    new_id(),
                    vec![SessionCommand::SaveWalkthrough { walkthrough: walk }],
                )
                .with_authorization(
                    WorkflowAction::PrepareWalkthrough,
                    s.authorize(WorkflowAction::PrepareWalkthrough)?,
                ),
            )?;
        }
        Ok(())
    }
}
fn journey(
    root: &Path,
    language: &str,
    budget: SharedBudget,
    resume_kind: &str,
    preference: ModelPreference,
) -> Result<()> {
    let resume = resume_kind != "none";
    if resume_kind == "authorized" {
        budget.lock().unwrap().resume_authorized(language)?;
        let receipt = root.join(language).join("receipt.json");
        if receipt.exists() {
            fs::copy(
                &receipt,
                root.join(language)
                    .join(format!("receipt-before-authorized-{}.json", new_id())),
            )?;
        }
    } else if resume_kind == "schema" {
        let error: serde_json::Value = serde_json::from_slice(&fs::read(
            root.join(language).join("failed-turn-error-private.json"),
        )?)?;
        ensure!(
            error["message"].as_str().is_some_and(
                |m| m.contains("invalid_json_schema") && m.contains("Missing 'question'")
            ),
            "No diagnosed schema failure receipt"
        );
        budget.lock().unwrap().resume_schema_failure(language)?;
        fs::rename(
            root.join(language).join("receipt.json"),
            root.join(language)
                .join("receipt-initial-schema-failure.json"),
        )?;
    } else if resume_kind == "research-question" {
        let previous: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(language).join("receipt.json"))?)?;
        ensure!(
            previous["error"]
                .as_str()
                .is_some_and(|e| e.contains("Resolve research before asking a clarification")),
            "No matching mixed research/question failure receipt"
        );
        budget
            .lock()
            .unwrap()
            .resume_research_question_failure(language)?;
        fs::rename(
            root.join(language).join("receipt.json"),
            root.join(language)
                .join("receipt-research-question-failure.json"),
        )?;
    } else if resume_kind == "public-vocabulary" {
        let previous: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(language).join("receipt.json"))?)?;
        ensure!(
            previous["error"] == "No structured clarification was offered"
                && root
                    .join(language)
                    .join("receipt-research-question-failure.json")
                    .is_file(),
            "No matching public-vocabulary failure receipt"
        );
        budget
            .lock()
            .unwrap()
            .resume_public_vocabulary_failure(language)?;
        fs::rename(
            root.join(language).join("receipt.json"),
            root.join(language)
                .join("receipt-public-vocabulary-failure.json"),
        )?;
    } else if resume_kind == "remaining" {
        let directory = root.join(language);
        ensure!(
            !directory
                .join("receipt-before-independent-checks.json")
                .exists(),
            "Independent continuation was already attempted"
        );
        if language == "rust" {
            let diagnostics: serde_json::Value = serde_json::from_slice(&fs::read(
                directory.join("failed-turn-contract-diagnostics.json"),
            )?)?;
            ensure!(
                diagnostics
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|r| r["research_request_count"] == 32
                        && r["contract_error"] == "Reply exceeds research bounds")),
                "No actual32-request diagnosis"
            );
        } else {
            let previous: serde_json::Value =
                serde_json::from_slice(&fs::read(directory.join("receipt.json"))?)?;
            ensure!(
                previous["error"] == "No structured clarification was offered",
                "No matching premature-question assertion"
            );
        }
        budget.lock().unwrap().resume_independent_checks(language)?;
        fs::rename(
            directory.join("receipt.json"),
            directory.join("receipt-before-independent-checks.json"),
        )?;
    } else {
        budget.lock().unwrap().claim(language)?;
    }
    let mut h = Harness::new(&root.join(language), language, resume)?;
    h.preference = preference;
    if let ModelPreference::Explicit { model } = &h.preference {
        h.choices.push(format!("User-authorized explicit {model} selection at the live catalog's supported default effort for the remaining Python allowance; original session/documents/approvals/progress and all prior reservations retained"));
        h.save_progress()?;
    }
    let mut r = h.runner(budget.clone())?;
    let recipe = h.environment.recipe(&h.task.recipe)?;
    let url = if language == "rust" {
        "https://doc.rust-lang.org/std/cmp/fn.min.html"
    } else {
        "https://docs.python.org/3/library/functions.html#min"
    };
    let result = (|| -> Result<()> {
        if resume_kind == "authorized" {
            h.choices.push("Explicitly authorized same-suite continuation from durable documents/current action; no replay of initial message, no count refunds and no fallback".into());
            if language == "python" && h.structured_answers()? == 0 {
                h.choices.push("Harness human explicitly requests missing local research and one genuine structured domain confirmation before implementation; approved documents remain unchanged".into());
                h.run(&mut r,ConversationIntent::Message("Before we return to the approved implementation, inspect the local README.md using ReadFileRange, inspect local GitHistory, and inspect Python DependencyMetadata through the host research broker. Public documentation cannot substitute for these local sources. Then ask me one structured clarification confirming the supported integer input domain and inclusive boundaries, with stable choices and a custom answer. Do not revise the approved spec or plan and do not provide code.".into()))?;
            }
        } else if resume {
            let s = h.state()?;
            ensure!(
                s.spec.is_none() && s.plan.is_none() && s.pending_question.is_none(),
                "Continuation is only for the first discussion; do not replay later work"
            );
            h.choices.push(format!("Targeted same-suite continuation after the diagnosed {resume_kind} contract fix. Original failed turns, transcript, session identity and charged counts are preserved; no human message is re-delivered."));
            if resume_kind != "remaining" || language == "rust" {
                h.run(&mut r, ConversationIntent::Discuss)?;
            } else {
                h.choices.push("Continue Python's explicit later intents after the earlier research-budget/missing-question record. A first Discussion need not ask a question; the final structured-answer criterion remains required.".into());
            }
        } else {
            h.run(&mut r,ConversationIntent::Message(format!("Let's discuss the missing bounded_count behavior for nonnegative integers through one million before drafting documents. Research captured README.md with ReadFileRange, declared {language} DependencyMetadata, GitHistory and the public page {url}; cite actual receipts and keep private names out of public contexts. Ask one structured clarification about the input domain, with stable choices and custom text. Do not draft documents yet. We want a small dependency-aware plan covering module implementation and caller verification. For implementation, guide separate tiny actions: create only the empty module file, implement one symbol, run independent checks, assess the cumulative step. The future displayed check argv is {}. Keep all solutions hidden.",serde_json::to_string(&recipe.argv)?)))?;
        }
        if let Some(question) = h.state()?.pending_question {
            h.choices.push("Harness human chooses a custom domain answer through the authoritative question command".into());
            let answer = h.clarification_answer()?;
            h.run(
                &mut r,
                ConversationIntent::Answer(ClarificationAnswer {
                    question_id: question.id,
                    choice_id: None,
                    custom_text: Some(answer),
                }),
            )?;
        } else {
            h.choices.push("No initial structured question was observed. Do not synthesize one; continue independent typed work and keep the final structured-answer criterion unmet until a real question is answered.".into());
        }
        h.task.verify_originals()?;
        if h.state()?.spec.is_none() {
            let spec = h.run(&mut r, ConversationIntent::DraftSpec)?;
            let Some(CompanionResponse::SpecDraft(draft)) = spec.response else {
                anyhow::bail!("No spec proposal")
            };
            let mut spec = draft
                .spec
                .context("Spec clarification/research still pending")?;
            spec.constraints
                .push("Harness human edit: retain original contract tests and caller bytes".into());
            h.human(
                "Harness human edits and saves the generated spec; saving does not approve",
                vec![SessionCommand::SaveSpec { spec }],
            )?;
            let s = h.state()?;
            h.human(
                "Harness human explicitly approves this exact saved spec",
                vec![SessionCommand::ApproveSpec {
                    document_ref: s.spec.unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                }],
            )?;
        }
        if h.state()?.plan.is_none() {
            let plan = h.run(&mut r, ConversationIntent::DraftPlan)?;
            let Some(CompanionResponse::PlanDraft(draft)) = plan.response else {
                anyhow::bail!("No plan proposal")
            };
            let proposal = draft
                .plan
                .context("Plan clarification/research still pending")?;
            ensure!(
                (1..=32).contains(&proposal.steps.len()),
                "Fixture needs a small multi-step plan; received {} steps (not coerced to a single step)",
                proposal.steps.len()
            );
            let s = h.state()?;
            let mut task = proposal.into_plan(&s.project, None)?;
            task.constraints.push(
                "Harness human edit: implement only the fixture module; no dependencies".into(),
            );
            h.human(
                "Harness human independently edits and saves the generated plan",
                vec![SessionCommand::SavePlan {
                    plan: PlanDocument {
                        revision: 1,
                        spec_ref: s.spec.unwrap().document_ref(),
                        task,
                    },
                }],
            )?;
            let s = h.state()?;
            ensure!(
                s.authorize(WorkflowAction::PrepareWalkthrough).is_err(),
                "Unapproved implementation gate bypassed"
            );
            h.human(
                "Harness human explicitly approves the exact saved plan",
                vec![SessionCommand::ApprovePlan {
                    document_ref: s.plan.unwrap().document_ref(),
                    approved_at_ms: now_ms(),
                }],
            )?;
        }
        if h.state()?.started_generation.is_none() {
            ensure!(
                h.state()?
                    .authorize(WorkflowAction::PrepareWalkthrough)
                    .is_err(),
                "Start gate bypassed"
            );
            h.human(
                "Harness human explicitly selects Start Implementation",
                vec![SessionCommand::StartImplementation],
            )?;
        }
        // Rebuild the runtime transport; transcript/documents remain in Store.
        drop(r);
        r = h.runner(budget.clone())?;
        let mut actions = 0;
        while !h.state()?.implementation_complete() {
            ensure!(
                actions < 1024,
                "Production plan/walkthrough action bound reached"
            );
            if h.state()?.current_action().is_err() {
                h.walkthrough(&mut r)?;
            }
            let action = h.state()?.current_action()?.1.clone();
            match &action.target {
                ActionTarget::File {
                    path,
                    symbol,
                    change_kind,
                } => {
                    ensure!(
                        path == &h.task.path,
                        "Model action exceeds fixture module scope: {path}"
                    );
                    if *change_kind == FileChangeKind::Create && symbol.is_none() {
                        h.saw_create = true;
                        h.save_owned_module(String::new())?;
                    } else {
                        ensure!(
                            symbol.as_deref().is_some_and(
                                |symbol| symbol.rsplit("::").next() == Some("bounded_count")
                            ),
                            "Expected one explicitly named symbol action"
                        );
                        h.saw_symbol = true;
                        if !h.rejected_wrong {
                            h.overlay(h.task.wrong.clone())?;
                            h.check(CheckStatus::Failed)?;
                            let bad = h.review(&mut r)?;
                            ensure!(
                                bad.outcome == ActionReviewOutcome::NeedsChanges,
                                "Actual review accepted the wrong implementation: {:?}",
                                bad.outcome
                            );
                            ensure!(h.next(&bad).is_err(), "Wrong implementation advanced");
                            h.rejected_wrong = true;
                        }
                        h.save_owned_module(h.task.alternative.clone())?;
                        h.check(CheckStatus::Passed)?;
                    }
                }
                ActionTarget::Command { .. } => {
                    h.saw_command = true;
                    if !h.rejected_missing {
                        let state = h.state()?;
                        let saved = h.store.lock().unwrap().current_action_review(&state)?;
                        let current = h.current_input()?;
                        let missing = if let Some((
                            review,
                            Some(OperationOutcome::Completed { .. }),
                        )) = saved.filter(|(review, _)| {
                            review.binding == current.binding
                                && matches!(
                                    review.outcome,
                                    ActionReviewOutcome::NeedsChanges
                                        | ActionReviewOutcome::InsufficientEvidence
                                )
                                && current.evidence_gate(&action).0 != ActionReviewOutcome::Passed
                        }) {
                            h.choices.push("Resume the immutable current negative review against identical missing-evidence inputs; no duplicate model call or invented outcome".into());
                            review
                        } else {
                            h.review(&mut r)?
                        };
                        ensure!(
                            matches!(
                                missing.outcome,
                                ActionReviewOutcome::InsufficientEvidence
                                    | ActionReviewOutcome::NeedsChanges
                            ),
                            "Command without action-bound evidence was not rejected: {:?}",
                            missing.outcome
                        );
                        ensure!(h.next(&missing).is_err(), "Missing evidence advanced");
                        h.rejected_missing = true;
                    }
                    h.check(CheckStatus::Passed)?;
                }
                ActionTarget::Validation { .. } => {}
            }
            if !matches!(action.target, ActionTarget::Command { .. }) {
                h.attach_preservation()?;
            }
            let review = h.review(&mut r)?;
            ensure!(
                review.outcome == ActionReviewOutcome::Passed,
                "Actual review did not accept the valid current action: {:?}; {:?}; {:?}",
                review.outcome,
                review.findings,
                review.limitations
            );
            // Actual store restart between pass/Next, without an extra model turn.
            if actions == 0 {
                drop(r);
                let directory = h.store.lock().unwrap().directory.clone();
                let parking = Arc::new(Mutex::new(Store::open(
                    &h.directory.join("restart-parking"),
                )?));
                let old = std::mem::replace(&mut h.store, parking.clone());
                h.capture.store = parking;
                drop(old);
                h.store = Arc::new(Mutex::new(Store::open(&directory)?));
                h.capture.store = h.store.clone();
                h.capture.inner.overlays = h
                    .store
                    .lock()
                    .unwrap()
                    .working_documents(h.task.project.key(), &[])?;
                h.capture.inner.overlays = supplemental::check_input_overlays(
                    &h.task,
                    std::mem::take(&mut h.capture.inner.overlays),
                )?;
                r = h.runner(budget.clone())?;
            }
            h.next(&review)?;
            h.capture.inner.attachments.clear();
            h.save_progress()?;
            actions += 1;
            h.task.verify_originals()?;
        }
        ensure!(
            h.saw_create && h.saw_symbol && h.saw_command && h.rejected_wrong && h.rejected_missing,
            "Required multiple tiny action/rejection scenarios were not all exercised"
        );
        ensure!(
            h.structured_answers()? > 0,
            "No real structured clarification answer was observed"
        );
        let kinds = h.research_kinds()?;
        ensure!(
            ["doc", "dependencies", "git", "public"]
                .iter()
                .all(|kind| kinds.contains(*kind)),
            "Required actual research sources remain missing: {kinds:?}"
        );
        let runs = h.store.lock().unwrap().model_runs(h.task.project.key())?;
        ensure!(
            runs.iter()
                .all(|run| run.request.action != CompanionAction::Reveal && run.tool_items == 0),
            "Unexpected solution request or model tool execution"
        );
        write_json(&h.directory.join("output-schemas.json"),&runs.iter().map(|run|json!({"action":run.request.action,"current_wire_schema_hash":content_hash(tandem_codex::output_schema_json(&run.request.schema()).unwrap().as_bytes()),"original_dispatch_schema_hash":"see immutable dispatch receipt; historical wire encodings are not rewritten","resolved_model":run.request.resolved_model,"thread_id":run.thread_id,"turn_id":run.turn_id,"status":run.status})).collect::<Vec<_>>())?;
        Ok(())
    })();
    let status = if result.is_ok() { "passed" } else { "failed" };
    h.save_progress()?;
    budget.lock().unwrap().finish(language, status)?;
    let state = h.state()?;
    write_json(
        &h.directory.join("unfinished-stage.json"),
        &json!({"implementation_complete":state.implementation_complete(),"has_saved_spec":state.spec.is_some(),"has_saved_plan":state.plan.is_some(),"started_generation":state.started_generation,"pending_question":state.pending_question.as_ref().map(|q|&q.id),"current_action":state.current_action().ok().map(|(walk,action)|json!({"walkthrough":walk.id,"step":walk.step_id,"cursor":walk.cursor,"action":action.id,"kind":action.kind,"what":action.what})),"steps":state.plan.as_ref().map(|plan|plan.task.steps.iter().map(|step|json!({"id":step.id,"title":step.title,"status":step.status})).collect::<Vec<_>>())}),
    )?;
    write_json(
        &h.directory.join("receipt.json"),
        &json!({"class":"R","status":status,"error":result.as_ref().err().map(|e|format!("{e:#}")),"identity":identity()?,"charged_turns":budget.lock().unwrap().counts(),"harness_choices":h.choices,"structured_answer_count":h.structured_answers()?,"actual_research_kinds":h.research_kinds()?,"assertions":{"create_file":h.saw_create,"named_symbol":h.saw_symbol,"command":h.saw_command,"wrong_rejected":h.rejected_wrong,"missing_evidence_rejected":h.rejected_missing,"implementation_complete":h.state()?.implementation_complete(),"immutable_originals_and_human_save_baseline_match":h.task.verify_originals().is_ok(),"owned_module_last_human_save_hash":h.task.owned_module_hash},"limitations":["These programmatic human decisions are disposable harness choices, not native usability proof.","Solutions remain hidden; no Reveal is requested.","Transport restart is real; changing models without another quota-consuming example is covered deterministically."]}),
    )?;
    println!(
        "{language} {status}; charged totals={:?}; receipts={}",
        budget.lock().unwrap().counts(),
        h.directory.display()
    );
    result
}
fn main() -> Result<()> {
    require_subscription_usage(std::env::args())?;
    ensure!(
        !std::env::args().any(|a| a == "--model-authorization")
            || std::env::args().any(|a| a == "--resume-authorized"),
        "Explicit model authorization is only supported for the authorized same-suite continuation"
    );
    let directory = run_directory()?;
    if std::env::args().any(|a| a == "--verify-supplemental-check-inputs") {
        return supplemental::verify(&directory);
    }
    if let Some(language) = std::env::args()
        .skip_while(|a| a != "--dump-latest-schema")
        .nth(1)
    {
        ensure!(
            matches!(language.as_str(), "rust" | "python"),
            "Unknown suite"
        );
        let root = directory.join(language);
        let latest = fs::read_dir(&root)?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|e| e.file_name().to_string_lossy().starts_with("private-"))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .context("No private dispatch")?;
        let run: ModelRun = serde_json::from_slice(&fs::read(latest.path())?)?;
        let schema = run.request.schema();
        write_json(
            &root.join(format!("schema-diagnostic-{}.json", run.id)),
            &schema,
        )?;
        fs::write(
            root.join(format!("wire-schema-diagnostic-{}.json", run.id)),
            tandem_codex::output_schema_json(&schema)?,
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&schema["$defs"]["ResearchRequestKind"])?
        );
        return Ok(());
    }
    if let Some(language) = std::env::args()
        .skip_while(|a| a != "--diagnose-latest")
        .nth(1)
    {
        return diagnose_latest(&directory, &language);
    }
    if std::env::args().any(|a| a == "--diagnose-availability") {
        return diagnose_availability(&directory);
    }
    if std::env::args().any(|a| a == "--resume-authorized") {
        let args = std::env::args().collect::<Vec<_>>();
        let auth = args
            .windows(2)
            .find(|a| a[0] == "--authorization")
            .context("--authorization path required")?;
        let language = args
            .windows(2)
            .find(|a| a[0] == "--language")
            .map(|a| a[1].as_str())
            .context("--language rust|python required for targeted continuation")?;
        ensure!(
            matches!(language, "rust" | "python"),
            "Only existing language suites may continue"
        );
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .context("Repository missing")?;
        let plan = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?)
            .join(".codex/plans/tandem/2026-11-14-guided-companion.md");
        let budget = Arc::new(Mutex::new(TurnBudget::open_authorized(
            &directory.join("turn-budget.json"),
            Path::new(&auth[1]),
            repo,
            &plan,
        )?));
        {
            let b = budget.lock().unwrap();
            println!(
                "Same-suite authorized {language} continuation BEFORE: {:?}; total={}",
                b.counts(),
                b.total()
            );
        }
        let before = budget.lock().unwrap().counts();
        let model_authorization = args.windows(2).find(|a| a[0] == "--model-authorization");
        ensure!(
            !args.iter().any(|a| a == "--model-authorization") || model_authorization.is_some(),
            "--model-authorization path required"
        );
        let preference = budget.lock().unwrap().model_preference(
            model_authorization.map(|a| (Path::new(&a[1]), Path::new(&auth[1]))),
            repo,
            &plan,
            language,
        )?;
        if model_authorization.is_some() || args.iter().any(|a| a == "--check-prerequisites") {
            let receipt = directory
                .join(language)
                .join(format!("model-selection-prerequisites-{}.json", new_id()));
            let _ = health_with_preference(&receipt, &preference)?;
            if args.iter().any(|a| a == "--check-prerequisites") {
                return Ok(());
            }
        }
        let receipt_path = directory
            .join(language)
            .join(format!("authorized-continuation-{}.json", new_id()));
        write_json(
            &receipt_path,
            &json!({"language":language,"before":before,"status":"started","authorization_hash":content_hash(&fs::read(&auth[1])?),"model_preference":preference,"model_authorization_hash":model_authorization.map(|a|fs::read(&a[1]).map(|bytes|content_hash(&bytes))).transpose()?}),
        )?;
        let result = journey(
            &directory,
            language,
            budget.clone(),
            "authorized",
            preference.clone(),
        );
        println!(
            "Same-suite authorized {language} continuation AFTER: {:?}",
            budget.lock().unwrap().counts()
        );
        write_json(
            &receipt_path,
            &json!({"language":language,"before":before,"after":budget.lock().unwrap().counts(),"status":if result.is_ok(){"passed"}else{"failed"},"error":result.as_ref().err().map(|e|format!("{e:#}")),"authorization_hash":content_hash(&fs::read(&auth[1])?),"model_preference":preference,"model_authorization_hash":model_authorization.map(|a|fs::read(&a[1]).map(|bytes|content_hash(&bytes))).transpose()?,"identity":identity()?}),
        )?;
        return result;
    }
    if std::env::args().any(|a| a == "--continue-python-draft-spec") {
        return continue_python_draft_only(&directory);
    }
    if std::env::args().any(|a| a == "--diagnose-python-input") {
        return diagnose_python_input(&directory);
    }
    if std::env::args().any(|a| a == "--finish-rust-documents") {
        return finish_rust_documents(&directory);
    }
    if std::env::args().any(|a| a == "--inspect-failures") {
        return inspect_failures(&directory);
    }
    let budget = budget(&directory)?;
    let _ = health(&directory)?;
    let mut errors = vec![];
    let schema = std::env::args().any(|a| a == "--resume-after-schema-fix");
    let research = std::env::args().any(|a| a == "--resume-after-research-question-fix");
    let public = std::env::args().any(|a| a == "--resume-after-public-vocabulary-fix");
    let remaining = std::env::args().any(|a| a == "--resume-remaining-checks");
    ensure!(
        usize::from(schema) + usize::from(research) + usize::from(public) + usize::from(remaining)
            <= 1,
        "Choose one diagnosed continuation"
    );
    let resume = if schema {
        "schema"
    } else if research {
        "research-question"
    } else if public {
        "public-vocabulary"
    } else if remaining {
        "remaining"
    } else {
        "none"
    };
    for language in ["rust", "python"] {
        if let Err(error) = journey(
            &directory,
            language,
            budget.clone(),
            resume,
            ModelPreference::Automatic,
        ) {
            errors.push(format!("{language}: {error:#}"));
        }
    }
    ensure!(
        errors.is_empty(),
        "Bounded journeys stopped honestly:\n{}",
        errors.join("\n")
    );
    Ok(())
}
fn diagnose_availability(directory: &Path) -> Result<()> {
    // One bounded status read, no thread/start, turn/start, retry or model work.
    let runtime = tandem_codex::discover_runtime()?;
    tandem_codex::verify_runtime(&runtime.binary)?;
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?)
        .join("Library/Application Support/Tandem/codex-m0");
    let mut client = tandem_codex::Client::start_with_overrides(
        &runtime.binary,
        &home,
        directory,
        &[
            "forced_login_method=\"chatgpt\"".into(),
            "model_provider=\"openai\"".into(),
            "cli_auth_credentials_store=\"keyring\"".into(),
        ],
    )?;
    let account = client.request(
        "account/read",
        json!({"refreshToken":false}),
        std::time::Duration::from_secs(15),
    )?;
    let limits = client.request(
        "account/rateLimits/read",
        json!({"excludeResetCreditDetails":true,"supportsLunaReserve":false}),
        std::time::Duration::from_secs(15),
    );
    let mut windows = json!({});
    if let Ok(value) = &limits {
        for name in ["primary", "secondary"] {
            let window = &value["rateLimits"][name];
            windows[name] = json!({"used_percent":window["usedPercent"].as_f64(),"resets_at":window["resetsAt"].as_i64(),"limit_window_seconds":window["limitWindowSeconds"].as_i64()});
        }
    }
    let allowed = limits
        .as_ref()
        .ok()
        .and_then(|v| v["ordinaryUsageAllowed"].as_bool());
    let receipt = json!({"model_turns":0,"account_is_chatgpt":account.pointer("/account/type").and_then(serde_json::Value::as_str)==Some("chatgpt"),"rate_limits_request_success":limits.is_ok(),"ordinary_usage_allowed":allowed,"ready_from_included_usage_field":allowed==Some(true),"windows":windows,"error_hash":limits.err().map(|e|content_hash(e.to_string().as_bytes())),"limitations":["A later status read cannot reconstruct the earlier failed preflight response. No model work or availability retry was dispatched."]});
    let path = directory.join(format!("availability-diagnostic-{}.json", new_id()));
    write_json(&path, &receipt)?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
fn domain_answer() -> String {
    "Both arguments are nonnegative integers from zero through one million inclusive; return their lesser, including zero and equality. Keep the independent tests and caller unchanged.".into()
}
fn continue_python_draft_only(directory: &Path) -> Result<()> {
    let path = directory.join("python/final-draft-spec-continuation.json");
    ensure!(
        !path.exists(),
        "This one explicit continuation was already attempted; preserve its result"
    );
    let budget = budget(directory)?;
    budget.lock().unwrap().resume_python_draft_only()?;
    let mut h = Harness::new(&directory.join("python"), "python", true)?;
    let state = h.state()?;
    ensure!(
        state.spec.is_none() && state.plan.is_none() && state.started_generation.is_none(),
        "Resume only the actual unfinished Python DraftSpec"
    );
    h.choices.push("One explicit same-session DraftSpec continuation with current captured contract in every turn; no initial message replay, synthetic question, plan/action request or extra allowance".into());
    let mut runner = h.runner(budget.clone())?;
    let result = (|| -> Result<()> {
        let response = h.run(&mut runner, ConversationIntent::DraftSpec)?;
        let Some(CompanionResponse::SpecDraft(draft)) = response.response else {
            anyhow::bail!("No spec proposal after the bounded explicit continuation")
        };
        let mut spec = draft
            .spec
            .context("DraftSpec still needs research or a real clarification")?;
        spec.constraints.push(
            "Harness human edit: preserve the independent fixture contract tests and caller".into(),
        );
        h.human("Fixture human edits and saves the actual generated spec; no plan or implementation authority is inferred",vec![SessionCommand::SaveSpec{spec}])?;
        let state = h.state()?;
        h.human(
            "Fixture human explicitly approves the exact saved spec only",
            vec![SessionCommand::ApproveSpec {
                document_ref: state.spec.unwrap().document_ref(),
                approved_at_ms: now_ms(),
            }],
        )?;
        Ok(())
    })();
    budget.lock().unwrap().finish(
        "python",
        if result.is_ok() {
            "partial-spec-saved"
        } else {
            "failed"
        },
    )?;
    let state = h.state()?;
    write_json(
        &path,
        &json!({"model_turns_allowed_at_start":3,"same_suite":true,"status":if result.is_ok(){"draft_spec_saved_approved_partial_journey"}else{"stopped_partial"},"error":result.as_ref().err().map(|error|format!("{error:#}")),"spec":state.spec,"spec_approval":state.spec_approval,"has_plan":state.plan.is_some(),"started_generation":state.started_generation,"pending_question":state.pending_question,"structured_answer_count":h.structured_answers()?,"actual_research_kinds":h.research_kinds()?,"implementation_complete":state.implementation_complete(),"charged_turns":budget.lock().unwrap().counts(),"next_stage":if state.spec.is_some(){"DraftPlan remains unverified; this command requests no further model work"}else{"DraftSpec or its genuine clarification remains unfinished"},"harness_choices":h.choices,"identity":identity()?}),
    )?;
    println!(
        "Python single DraftSpec continuation settled; charges={:?}; artifact={}",
        budget.lock().unwrap().counts(),
        path.display()
    );
    result
}
fn diagnose_python_input(directory: &Path) -> Result<()> {
    let root = directory.join("python");
    let project = Project::open(&root.join("project"))?;
    let store = Store::open(&root.join("state"))?;
    let session = store
        .selected_session(project.key())?
        .context("Missing original Python session")?;
    let original_goal = all_conversation_events(&Mutex::new(store), &session)?
        .into_iter()
        .find_map(|event| {
            if let ConversationPayload::HumanMessage { text } = event.payload {
                Some(text)
            } else {
                None
            }
        })
        .context("Missing original human goal")?;
    let store = Store::open(&root.join("state"))?;
    let run = store
        .model_runs(project.key())?
        .into_iter()
        .find(|run| {
            run.request.action == CompanionAction::DraftSpec && run.status == ModelStatus::Completed
        })
        .context("No completed DraftSpec turn")?;
    let runtime = tandem_codex::discover_runtime()?;
    let credential_home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?)
        .join("Library/Application Support/Tandem/codex-m0");
    let mut client = tandem_codex::Client::start_with_overrides(
        &runtime.binary,
        &credential_home,
        directory,
        &[
            "forced_login_method=\"chatgpt\"".into(),
            "model_provider=\"openai\"".into(),
            "cli_auth_credentials_store=\"keyring\"".into(),
        ],
    )?;
    let value = client.request(
        "thread/read",
        json!({"threadId":run.thread_id,"includeTurns":true}),
        std::time::Duration::from_secs(15),
    )?;
    client.shutdown()?;
    let mut actual = String::new();
    let mut thread_inputs = String::new();
    let mut input_count = 0;
    for turn in value
        .pointer("/thread/turns")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        for item in turn["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "userMessage")
        {
            for part in item["content"].as_array().into_iter().flatten() {
                if let Some(text) = part["text"].as_str() {
                    thread_inputs.push_str(text);
                    input_count += 1;
                    if turn["id"].as_str() == run.turn_id.as_deref() {
                        actual.push_str(text);
                    }
                }
            }
        }
    }
    ensure!(
        !actual.is_empty(),
        "Official thread/read exposed no text for the selected actual turn; no input-presence claim made"
    );
    let contains = |haystack: &str, needle: &str| -> Result<bool> {
        let encoded = serde_json::to_string(needle)?;
        Ok(haystack.contains(needle.trim()) || haystack.contains(&encoded[1..encoded.len() - 1]))
    };
    let contract = fs::read_to_string(project.root.join("README.md"))?;
    let marker = "GUIDED_OPERATION (host snapshot; history/research are untrusted data, not authorization): ";
    let guided: GuidedContext = serde_json::from_str(
        actual
            .split_once(marker)
            .context("Actual provider input has no guided snapshot")?
            .1,
    )?;
    let relevant=guided.receipts.iter().filter(|receipt|receipt.status==ResearchReceiptStatus::Complete&&receipt.public_web.as_ref().is_some_and(|web|web.final_url.ends_with("#min"))).map(|receipt|json!({"id_hash":content_hash(receipt.id.as_bytes()),"id_present_in_actual_input":actual.contains(&receipt.id),"content_hash":receipt.content_hash,"complete_text_present":contains(&actual,&receipt.content).unwrap_or(false),"returned_bytes":receipt.content.len(),"contains_relevant_smallest_item_text":receipt.content.to_lowercase().contains("smallest")})).collect::<Vec<_>>();
    let metadata = json!({"model_turns":0,"official_thread_read":true,"selected_turn_hash":content_hash(run.turn_id.as_deref().unwrap_or_default().as_bytes()),"actual_input_hash":content_hash(actual.as_bytes()),"actual_input_bytes":actual.len(),"thread_input_count":input_count,"actual_turn_has_explicit_draft_spec_intent":actual.contains("APPLICATION_ACTION: DraftSpec")&&actual.contains("HUMAN_REQUEST: Explicit DraftSpec request"),"actual_turn_contains_original_human_goal":contains(&actual,&original_goal)?,"actual_thread_contains_original_human_goal":contains(&thread_inputs,&original_goal)?,"actual_turn_contains_independent_contract":contains(&actual,&contract)?,"actual_thread_contains_independent_contract":contains(&thread_inputs,&contract)?,"host_request_contains_independent_contract":run.request.context.files.iter().any(|file|file.path=="README.md"&&file.text==contract),"guided_operation_matches_saved_request":run.request.guided.as_ref().is_some_and(|expected|expected.operation_id==guided.operation_id),"pending_question_in_authoritative_input":guided.session.pending_question.is_some(),"recent_transcript_event_count":guided.transcript.recent.len(),"older_history_checkpoint_present":guided.transcript.checkpoint.is_some(),"complete_receipt_count":guided.receipts.iter().filter(|r|r.status==ResearchReceiptStatus::Complete).count(),"partial_receipt_count":guided.receipts.iter().filter(|r|r.status==ResearchReceiptStatus::Partial).count(),"complete_relevant_selected_sources":relevant});
    let runtime_path = Path::new("crates/tandem-companion/src/runtime.rs");
    let source = fs::read_to_string(runtime_path)?;
    let base = raw_instruction(&source, "INSTRUCTIONS")?;
    let improvement = raw_instruction(&source, "IMPROVEMENT_RULES")?;
    let instruction_hash = content_hash(format!("{base}\n{improvement}").as_bytes());
    let dispatch: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(format!("dispatch-{}.json", run.id)))?)?;
    let routing = json!({"model_turns":0,"actual_top_level_action_is_draft_spec":actual.starts_with("APPLICATION_ACTION: DraftSpec\n"),"actual_top_level_human_request_is_draft_spec":actual.lines().any(|line|line=="HUMAN_REQUEST: Explicit DraftSpec request"),"saved_request_action_is_draft_spec":run.request.action==CompanionAction::DraftSpec,"wire_schema_has_spec_proposal_field":run.request.schema()["properties"].get("spec").is_some(),"wire_schema_is_discussion_only":run.request.schema()["properties"].get("text").is_some(),"runtime_source_hash_matches_actual_dispatch":dispatch["identity"]["worktree_source_hashes"]["crates/tandem-companion/src/runtime.rs"]==file_hash(runtime_path)?,"recorded_base_instruction_hash_matches_current_source":run.thread_binding.as_ref().and_then(|binding|binding.instructions_hash.as_deref())==Some(instruction_hash.as_str()),"actual_recorded_instruction_hash":run.thread_binding.as_ref().and_then(|binding|binding.instructions_hash.as_ref()),"developer_routing_source_uses_current_action":source.contains("Only the current Tandem action and any recorded broker grant apply."),"developer_routing_source_labels_request_discuss":source.contains("developerInstructions\":\"Discuss"),"unconditional_null_until_answer_directive_present":base.contains("Return null for any typed artifact until the human explicitly answers;"),"pending_question":guided.session.pending_question.is_some(),"historical_original_goal_contains_defer_draft_instruction":original_goal.contains("Do not draft documents yet"),"current_runtime_source_directive_snippets":["Managed Discuss always stays discussion", "DraftSpec and DraftPlan return proposals only", "Only the current Tandem action and any recorded broker grant apply", if base.contains("Return null for any typed artifact until the human explicitly answers;"){ "Return null for any typed artifact until the human explicitly answers" }else{"Only when you are asking a clarification or one is pending, return null for the typed artifact"}],"limitations":["Thread/read does not expose provider system/developer text verbatim. Recorded instruction hash and matching dispatched runtime source establish host routing; no claim about hidden provider instructions."]});
    write_json(&root.join("instruction-routing-diagnostics.json"), &routing)?;
    write_json(&root.join("actual-input-diagnostics.json"), &metadata)?;
    println!(
        "Python actual-input presence/count/hash diagnostics saved; no prompts printed; zero model turns"
    );
    Ok(())
}
fn raw_instruction<'a>(source: &'a str, name: &str) -> Result<&'a str> {
    let start = format!("const {name}: &str = r#\"");
    let value = source
        .split_once(&start)
        .context("Instruction constant missing")?
        .1;
    Ok(value
        .split_once("\"#;")
        .context("Instruction constant terminator missing")?
        .0)
}
fn finish_rust_documents(directory: &Path) -> Result<()> {
    let budget = budget(directory)?;
    ensure!(
        budget.lock().unwrap().counts().get("rust") == Some(&24),
        "This zero-turn finalize requires the original exhausted Rust allowance"
    );
    let path = directory.join("rust/approved-documents.json");
    ensure!(
        !path.exists(),
        "Document finalize already recorded; never overwrite it"
    );
    let mut h = Harness::new(&directory.join("rust"), "rust", true)?;
    let state = h.state()?;
    ensure!(
        state.plan.is_none()
            && state.started_generation.is_none()
            && state
                .spec_approval
                .as_ref()
                .is_some_and(|approval| Some(&approval.document_ref)
                    == state.spec.as_ref().map(SpecDocument::document_ref).as_ref()),
        "Expected the exact previously saved/approved spec and unsaved plan proposal"
    );
    let generated=h.store.lock().unwrap().model_runs(h.task.project.key())?.into_iter().find(|run|run.status==ModelStatus::Completed&&run.request.action==CompanionAction::DraftPlan&&matches!(&run.response,Some(CompanionResponse::PlanDraft(draft)) if draft.plan.is_some())).context("No completed real plan proposal")?;
    let Some(CompanionResponse::PlanDraft(draft)) = generated.response else {
        unreachable!()
    };
    let proposal = draft.plan.unwrap();
    ensure!(
        (1..=32).contains(&proposal.steps.len()),
        "Keep the unchanged legitimate multi-step plan within production bounds"
    );
    let task = proposal.into_plan(&state.project, None)?;
    h.human("Zero-model fixture human saves the unchanged real multi-step proposal; earlier arbitrary two-to-three-step rejection remains recorded",vec![SessionCommand::SavePlan{plan:PlanDocument{revision:1,spec_ref:state.spec.unwrap().document_ref(),task}}])?;
    let state = h.state()?;
    ensure!(
        state.authorize(WorkflowAction::PrepareWalkthrough).is_err(),
        "Unapproved plan gate bypassed"
    );
    h.human(
        "Zero-model fixture human approves the exact saved plan",
        vec![SessionCommand::ApprovePlan {
            document_ref: state.plan.unwrap().document_ref(),
            approved_at_ms: now_ms(),
        }],
    )?;
    ensure!(
        h.state()?
            .authorize(WorkflowAction::PrepareWalkthrough)
            .is_err(),
        "Start gate bypassed"
    );
    h.human(
        "Zero-model fixture human explicitly selects Start; no model turn or review is invented",
        vec![SessionCommand::StartImplementation],
    )?;
    let state = h.state()?;
    state.authorize(WorkflowAction::PrepareWalkthrough)?;
    h.task.verify_originals()?;
    write_json(
        &path,
        &json!({"class":"R production Store transitions with explicit disposable harness human choices","model_turns":0,"proposal_run_id":generated.id,"resolved_model":generated.request.resolved_model,"spec":state.spec,"plan":state.plan,"spec_approval":state.spec_approval,"plan_approval":state.plan_approval,"execution_generation":state.execution_generation,"started_generation":state.started_generation,"next_stage":"PrepareWalkthrough blocked by exhausted24-turn Rust cap; no action or review prepared","implementation_complete":state.implementation_complete(),"live_source_unchanged":true,"charged_turns":budget.lock().unwrap().counts(),"harness_choices":h.choices}),
    )?;
    println!(
        "Rust real documents saved/approved and Start recorded with zero model turns; next PrepareWalkthrough blocked by cap; artifact={}",
        path.display()
    );
    Ok(())
}
fn diagnose_latest(directory: &Path, language: &str) -> Result<()> {
    ensure!(matches!(language, "rust" | "python"), "Unknown suite");
    let root = directory.join(language);
    let project = Project::open(&root.join("project"))?;
    let store = Store::open(&root.join("state"))?;
    let run = store
        .model_runs(project.key())?
        .into_iter()
        .max_by_key(|r| r.started_at_ms)
        .context("No run")?;
    let runtime = tandem_codex::discover_runtime()?;
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME missing")?)
        .join("Library/Application Support/Tandem/codex-m0");
    let mut client = tandem_codex::Client::start_with_overrides(
        &runtime.binary,
        &home,
        directory,
        &[
            "forced_login_method=\"chatgpt\"".into(),
            "model_provider=\"openai\"".into(),
            "cli_auth_credentials_store=\"keyring\"".into(),
        ],
    )?;
    let thread = client.request(
        "thread/read",
        json!({"threadId":run.thread_id,"includeTurns":true}),
        std::time::Duration::from_secs(15),
    )?;
    let mut diagnostics = vec![];
    for turn in thread["thread"]["turns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|turn| turn["id"].as_str() == run.turn_id.as_deref())
    {
        for item in turn["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["type"] == "agentMessage")
        {
            let Some(text) = item["text"].as_str() else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(text)?;
            let error = run
                .request
                .parse_response(value.clone())
                .err()
                .map(|e| format!("{e:#}"));
            write_json(
                &root.join(format!("diagnosed-response-private-{}.json", run.id)),
                &value,
            )?;
            diagnostics.push(json!({"run_id":run.id,"action":run.request.action,"status":run.status,"response_hash":content_hash(text.as_bytes()),"contract_error":error,"keys":value.as_object().map(|v|v.keys().collect::<Vec<_>>()),"action_targets":value["actions"].as_array().map(|v|v.iter().map(|a|&a["target"]).collect::<Vec<_>>())}));
        }
    }
    client.shutdown()?;
    write_json(
        &root.join(format!("diagnosed-contract-{}.json", run.id)),
        &diagnostics,
    )?;
    println!("{}", serde_json::to_string_pretty(&diagnostics)?);
    Ok(())
}
fn inspect_failures(directory: &Path) -> Result<()> {
    let runtime = tandem_codex::discover_runtime()?;
    let credential_home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?)
        .join("Library/Application Support/Tandem/codex-m0");
    let mut client = tandem_codex::Client::start_with_overrides(
        &runtime.binary,
        &credential_home,
        directory,
        &[
            "forced_login_method=\"chatgpt\"".into(),
            "model_provider=\"openai\"".into(),
            "cli_auth_credentials_store=\"keyring\"".into(),
        ],
    )?;
    for language in ["rust", "python"] {
        let store = Store::open(&directory.join(language).join("state"))?;
        let project = Project::open(&directory.join(language).join("project"))?;
        let mut diagnostics = vec![];
        let mut contracts = vec![];
        for run in store.model_runs(project.key())? {
            let Some(thread) = &run.thread_id else {
                continue;
            };
            let value = client.request(
                "thread/read",
                json!({"threadId":thread,"includeTurns":true}),
                std::time::Duration::from_secs(15),
            )?;
            for turn in value
                .pointer("/thread/turns")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if !turn["error"].is_null() {
                    let error = &turn["error"];
                    // Error-only receipt; never echo the private input/items.
                    write_json(
                        &directory
                            .join(language)
                            .join("failed-turn-error-private.json"),
                        &sanitized_runtime_error(error),
                    )?;
                    diagnostics.push(json!({"turn_id":turn["id"],"status":turn["status"],"code":error["codexErrorInfo"],"message_hash":content_hash(error["message"].as_str().unwrap_or_default().as_bytes())}));
                }
                if run.status == ModelStatus::Quarantined
                    && Some(turn["id"].as_str().unwrap_or_default()) == run.turn_id.as_deref()
                {
                    for item in turn["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|item| item["type"] == "agentMessage")
                    {
                        let Some(text) = item["text"].as_str() else {
                            continue;
                        };
                        let error = serde_json::from_str(text)
                            .map_err(anyhow::Error::from)
                            .and_then(|value| run.request.parse_response(value))
                            .err()
                            .map(|e| format!("{e:#}"));
                        let error = error.map(|error| {
                            if credential_like(&error) {
                                "Sensitive contract diagnostic withheld".into()
                            } else {
                                error
                            }
                        });
                        let shape = serde_json::from_str::<serde_json::Value>(text).ok();
                        let reply = shape
                            .as_ref()
                            .map(|value| value.get("reply").unwrap_or(value));
                        contracts.push(json!({"run_id":run.id,"turn_id":run.turn_id,"response_hash":content_hash(text.as_bytes()),"response_bytes":text.len(),"contract_error":error,"research_request_count":reply.and_then(|r|r["research_requests"].as_array()).map(Vec::len),"finding_count":reply.and_then(|r|r["findings"].as_array()).map(Vec::len)}));
                    }
                }
            }
        }
        write_json(
            &directory
                .join(language)
                .join("failed-turn-diagnostics.json"),
            &diagnostics,
        )?;
        write_json(
            &directory
                .join(language)
                .join("failed-turn-contract-diagnostics.json"),
            &contracts,
        )?;
        println!(
            "{language}: official thread/read retained {} error-only diagnostics; zero model turns",
            diagnostics.len()
        );
    }
    client.shutdown()?;
    Ok(())
}
