use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tandem_checks::{CheckEnvironment, excluded, materialize, run_check, secure_read};
use tandem_companion::{
    CompanionSession, bind_validation_environment, capture_context, credential_like,
    matching_check_evidence,
};
use tandem_core::*;

fn stage_project(run: &AgentRun) -> Result<Project> {
    let directory = PathBuf::from(&run.directory);
    ensure!(
        directory.is_absolute() && directory.file_name().and_then(|s| s.to_str()) == Some(&run.id),
        "Invalid delegation directory"
    );
    let directory = directory.canonicalize()?;
    let original = Path::new(&run.project);
    ensure!(
        !directory.starts_with(original) && !original.starts_with(&directory),
        "Delegation directory overlaps the live project"
    );
    let input = directory.join("input");
    ensure!(
        !input.symlink_metadata()?.file_type().is_symlink(),
        "Delegation root cannot be a symlink"
    );
    let project = Project::open(&input)?;
    ensure!(
        project.root.starts_with(directory),
        "Delegation root escaped its directory"
    );
    Ok(project)
}

pub fn prepare_delegation(
    project: &Project,
    working: &[Document],
    grant: DelegationGrant,
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<AgentRun> {
    grant.validate()?;
    ensure!(
        grant.project == project.key() && grant.confirmed_at_ms == 0,
        "Prepare an unconfirmed grant for this project"
    );
    for path in &grant.paths {
        ensure!(
            excluded(path).is_none(),
            "A delegated path is excluded from source capture"
        );
        validate_import_path(project, path)?;
    }
    let directory = cache.join(&grant.id);
    let snapshot = materialize(project, working, &directory, cancel)?;
    let stage = Project::open(&snapshot.input)?;
    let mut run = AgentRun::pending(grant, directory.display().to_string());
    let identities = snapshot
        .manifest
        .files
        .iter()
        .map(|f| (path_identity(&f.path), f.path.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut size = 0;
    for path in &run.grant.paths {
        ensure!(
            identities
                .get(&path_identity(path))
                .is_none_or(|found| *found == path),
            "Delegated path aliases a differently spelled captured file"
        );
        let entry = snapshot.manifest.files.iter().find(|f| &f.path == path);
        let doc = if let Some(entry) = entry {
            let captured = stage.open_document(path, false)?;
            size += captured.text.len();
            ensure!(
                !credential_like(&captured.text),
                "Credential-like text is excluded from delegation"
            );
            let mut original = working
                .iter()
                .find(|d| &d.path == path)
                .cloned()
                .unwrap_or_else(|| captured.clone());
            original.text = captured.text.clone();
            if !working.iter().any(|d| &d.path == path) {
                original.base_hash = entry.original_hash.clone();
                original.base_text = captured.text;
            }
            Some(original)
        } else {
            ensure!(
                !project.root.join(path).exists() && !working.iter().any(|d| &d.path == path),
                "Existing source was omitted; delegation cannot recreate or overwrite it"
            );
            ensure!(
                !tandem_checks::ignored_new_buffer(project, path)?,
                "New delegated file is ignored by project rules"
            );
            None
        };
        run.baseline.insert(path.clone(), doc);
    }
    ensure!(
        size <= MAX_DELEGATED_BYTES,
        "Delegated source exceeds the 256 KiB scope limit; choose a smaller step"
    );
    run.staged = run.baseline.clone();
    run.baseline_manifest = Some(snapshot.manifest);
    run.stage_verified = true;
    run.status = AgentStatus::Prepared;
    run.summary =
        "Captured working baseline. Review paths and check recipes before authorizing takeover."
            .into();
    run.activity("Prepared isolated source; original project remains untouched");
    Ok(run)
}

/// Reconstruct actual staged state after a run or restart. Extra files, links,
/// deletions and changes to read-only context keep the run unimportable.
pub fn inspect_delegation(run: &mut AgentRun) -> Result<()> {
    run.stage_verified = false;
    let project = stage_project(run)?;
    let manifest = run
        .baseline_manifest
        .as_ref()
        .context("Delegation baseline is missing")?;
    let allowed = run.grant.paths.iter().cloned().collect::<BTreeSet<_>>();
    let original = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect::<BTreeMap<_, _>>();
    let mut directories = vec![project.root.clone()];
    let mut seen = BTreeSet::new();
    let mut traversed = 0;
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            traversed += 1;
            ensure!(traversed <= 80_000, "Staged traversal exceeded its limit");
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let relative = path
                .strip_prefix(&project.root)?
                .to_str()
                .context("Non-UTF-8 staged path")?
                .to_string();
            ensure!(!kind.is_symlink(), "Staged symlinks are not importable");
            if kind.is_dir() {
                ensure!(
                    manifest
                        .files
                        .iter()
                        .any(|f| Path::new(&f.path).starts_with(&relative))
                        || allowed.iter().any(|f| Path::new(f).starts_with(&relative)),
                    "Unexpected staged directory"
                );
                directories.push(path);
                continue;
            }
            ensure!(kind.is_file(), "Special staged files are not importable");
            let (bytes, executable) = secure_read(&project, &relative)?;
            if !allowed.contains(&relative) {
                let before = original
                    .get(relative.as_str())
                    .context("An out-of-scope file appeared in staging")?;
                ensure!(
                    content_hash(&bytes) == before.hash && executable == before.executable,
                    "Read-only staged context changed"
                );
            } else if let Some(before) = original.get(relative.as_str()) {
                ensure!(
                    executable == before.executable,
                    "File mode changes are outside the grant"
                );
            } else {
                ensure!(!executable, "New executable files are outside this grant");
            }
            seen.insert(relative);
        }
    }
    for file in &manifest.files {
        ensure!(
            seen.contains(&file.path),
            "A captured file was removed from staging"
        );
    }
    let mut staged = BTreeMap::new();
    let mut size = 0;
    for path in &run.grant.paths {
        let doc = if seen.contains(path) {
            let doc = project.open_document(path, false)?;
            size += doc.text.len();
            ensure!(
                !credential_like(&doc.text),
                "Credential-like staged content is not importable"
            );
            Some(doc)
        } else {
            None
        };
        staged.insert(path.clone(), doc);
    }
    ensure!(
        size <= MAX_DELEGATED_BYTES,
        "Staged source exceeded its size limit"
    );
    run.staged = staged;
    run.stage_verified = true;
    run.sequence += 1;
    Ok(())
}

fn normalized_context(
    project: &Project,
    paths: &[String],
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<ContextSnapshot> {
    let mut context = capture_context(project, &[], paths, cache, cancel)?;
    for file in &mut context.files {
        let (_, text) = TextFormat::decode(file.text.as_bytes())?;
        file.text = text;
    }
    Ok(context)
}
pub struct AgentExecution<'a> {
    pub session: &'a mut CompanionSession,
    pub store: &'a Mutex<Store>,
    pub original: &'a Project,
    pub task: &'a TaskPlan,
    pub environment: &'a CheckEnvironment,
    pub cancel: &'a AtomicBool,
}
impl AgentExecution<'_> {
    pub fn run(
        &mut self,
        mut run: AgentRun,
        mut observe: impl FnMut(&AgentRun),
    ) -> Result<AgentRun> {
        ensure!(
            run.status == AgentStatus::Prepared
                && run.grant.confirmed_at_ms > 0
                && run.grant.matches_task(self.task),
            "Authorize a prepared delegation for the current plan"
        );
        ensure!(
            run.project == self.original.key(),
            "Delegation project changed"
        );
        let mut environment = self.environment.clone();
        environment.denied_roots.push(self.original.root.clone());
        run.status = AgentStatus::Running;
        run.sequence += 1;
        self.store.lock().unwrap().save_agent_run(&run)?;
        observe(&run);
        let mut binding = None;
        let mut feedback = vec![];
        let mut receipts = Vec::new();
        let result = (|| -> Result<()> {
            inspect_delegation(&mut run)?;
            let model_preference = self.store.lock().unwrap().model_preference()?;
            let resolved_model = self.session.resolve_model(&model_preference)?;
            let stage = stage_project(&run)?;
            let context_cache = PathBuf::from(&run.directory).join("context-captures");
            for round in 1..=run.grant.max_rounds {
                if self.cancel.load(Ordering::Relaxed) {
                    run.cancel_requested = true;
                    run.finish(
                        AgentStatus::Cancelled,
                        "Stopped before another action. Retained staged changes can be inspected.",
                    );
                    break;
                }
                run.round = round;
                run.status = AgentStatus::Running;
                run.activity(format!("Round {round}: analyzing the current staged files"));
                self.store.lock().unwrap().save_agent_run(&run)?;
                observe(&run);
                let mut selected = run.grant.paths.iter().cloned().collect::<BTreeSet<_>>();
                for step in &self.task.steps {
                    selected.extend(step.files.iter().cloned());
                }
                if let Some(manifest) = &run.baseline_manifest {
                    selected.extend(
                        manifest
                            .files
                            .iter()
                            .filter(|f| {
                                f.path.starts_with("tests/")
                                    || Path::new(&f.path)
                                        .file_name()
                                        .and_then(|p| p.to_str())
                                        .is_some_and(|n| {
                                            n.starts_with("test_") && n.ends_with(".py")
                                        })
                            })
                            .take(4)
                            .map(|f| f.path.clone()),
                    );
                }
                let mut context = normalized_context(
                    &stage,
                    &selected.into_iter().take(32).collect::<Vec<_>>(),
                    &context_cache,
                    self.cancel,
                )?;
                for path in &run.grant.paths {
                    if run.staged.get(path).and_then(Option::as_ref).is_some() {
                        ensure!(
                            context.files.iter().any(|f| &f.path == path),
                            "A delegated file did not fit the model context; narrow the scope"
                        );
                    }
                }
                let recipe_ids = run.grant.checks.keys().cloned().collect::<Vec<_>>();
                let identities =
                    bind_validation_environment(&mut context, Some(&environment), &recipe_ids);
                let hashes = run
                    .staged
                    .iter()
                    .map(|(path, doc)| {
                        (
                            path.clone(),
                            doc.as_ref()
                                .map(|d| content_hash(&d.format.encode(&d.text))),
                        )
                    })
                    .collect();
                let language = run
                    .grant
                    .paths
                    .first()
                    .and_then(|p| run.staged.get(p))
                    .and_then(Option::as_ref)
                    .map_or("rust", Document::language)
                    .to_string();
                let request = ModelRequest {
                    model_preference: model_preference.clone(),
                    resolved_model: Some(resolved_model.clone()),
                    conversation_id: Some(run.id.clone()),
                    guided: None,
                    action: CompanionAction::Delegate,
                    question: run.grant.objective.clone(),
                    language: language.clone(),
                    guidance: GuidanceSettings::default().effective(
                        &run.project,
                        &language,
                        Some(self.task),
                    ),
                    task: Some(self.task.clone()),
                    context: context.clone(),
                    evidence: matching_check_evidence(&context, &receipts, &identities),
                    prior_decisions: vec![],
                    delegation: Some(DelegationInput {
                        grant: run.grant.clone(),
                        round,
                        staged_hashes: hashes,
                        broker_feedback: feedback.clone(),
                    }),
                };
                let model = ModelRun::pending(run.project.clone(), request);
                run.model_runs.push(model.id.clone());
                self.store.lock().unwrap().save_model_run(&model)?;
                let (model, next) =
                    self.session
                        .run(model, binding.take(), self.cancel, |model| {
                            self.store.lock().unwrap().save_model_run(model)
                        })?;
                binding = next;
                if self.cancel.load(Ordering::Relaxed)
                    || matches!(
                        model.status,
                        ModelStatus::Cancelled | ModelStatus::Interrupted
                    )
                {
                    run.cancel_requested = self.cancel.load(Ordering::Relaxed);
                    run.finish(if model.status==ModelStatus::Interrupted{AgentStatus::Interrupted}else{AgentStatus::Cancelled},"Delegation stopped; completed staged actions are retained and no further actions were dispatched.");
                    break;
                }
                if model.status != ModelStatus::Completed {
                    run.finish(
                        AgentStatus::Failed,
                        format!("Companion operation stopped: {}", model.detail),
                    );
                    break;
                }
                let Some(CompanionResponse::Agent(reply)) = model.response else {
                    anyhow::bail!("Delegation returned an unsupported response");
                };
                // Re-read actual files before applying a batch, never rely on an
                // earlier transcript to establish the working version.
                inspect_delegation(&mut run)?;
                let batch = match apply_agent_batch(&run.grant, &run.staged, &reply) {
                    Ok(batch) => batch,
                    Err(error) => {
                        feedback = vec![format!(
                            "Batch rejected before writes: {error}. Correct the batch within the original grant."
                        )];
                        run.activity(feedback[0].clone());
                        continue;
                    }
                };
                run.activity(reply.summary.clone());
                run.limitations = reply.limitations.clone();
                run.next_action = reply.next_action.clone();
                for (path, document) in batch {
                    if self.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    ensure!(
                        !credential_like(&document.text),
                        "Credential-like generated content was not written"
                    );
                    validate_import_path(&stage, &path)?;
                    run.activity(format!("Applying staged edit: {path}"));
                    self.store.lock().unwrap().save_agent_run(&run)?;
                    observe(&run);
                    stage.save(&document)?;
                }
                inspect_delegation(&mut run)?;
                run.activity(format!(
                    "{} source file(s) differ from the captured working baseline",
                    run.changed_paths().len()
                ));
                self.store.lock().unwrap().save_agent_run(&run)?;
                observe(&run);
                if self.cancel.load(Ordering::Relaxed) {
                    run.cancel_requested = true;
                    run.finish(AgentStatus::Cancelled,"Stopped after completed staged edits. No additional checks or model turns were dispatched.");
                    break;
                }
                let requested = if reply.done {
                    run.grant.checks.keys().cloned().collect::<Vec<_>>()
                } else {
                    reply.check_requests.clone()
                };
                feedback.clear();
                let round_receipts = receipts.len();
                for id in requested {
                    if self.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let recipe = environment.recipe(&id)?;
                    ensure!(
                        run.grant.checks.get(&id) == Some(&recipe.grant_fingerprint),
                        "An approved recipe's executable or scope changed"
                    );
                    run.status = AgentStatus::Checking;
                    run.activity(format!("Running approved check: {}", recipe.title));
                    self.store.lock().unwrap().save_agent_run(&run)?;
                    observe(&run);
                    let mut check = CheckRecord::pending(
                        run.project.clone(),
                        id.clone(),
                        recipe.fingerprint.clone(),
                    );
                    check.task_id = Some(run.grant.task_id.clone());
                    check.step_id = run.grant.step_id.clone();
                    check.plan_revision = Some(run.grant.plan_revision);
                    run.record_check_intent(check.id.clone());
                    self.store.lock().unwrap().save_agent_run(&run)?;
                    let check = run_check(
                        &environment,
                        &stage,
                        &[],
                        &recipe,
                        check,
                        self.cancel,
                        |check| self.store.lock().unwrap().save_check(check),
                    )?;
                    feedback.push(format!(
                        "{}: {:?}. {}\n{}",
                        id,
                        check.status,
                        check.detail,
                        if credential_like(&check.output) {
                            "Output withheld because credential-like text was detected".into()
                        } else {
                            check.output.chars().take(12000).collect::<String>()
                        }
                    ));
                    run.activity(format!("{id}: {:?}", check.status));
                    receipts.push(check);
                }
                inspect_delegation(&mut run)?;
                if self.cancel.load(Ordering::Relaxed) {
                    run.cancel_requested = true;
                    run.finish(AgentStatus::Cancelled,"Check interrupted or completed before stop. Inspect its receipt and the retained staged diff.");
                    break;
                }
                if receipts[round_receipts..].iter().any(|c| {
                    matches!(
                        c.status,
                        CheckStatus::Unvalidated
                            | CheckStatus::Interrupted
                            | CheckStatus::Cancelled
                    )
                }) {
                    run.finish(AgentStatus::ReadyForReview,"Stopped at a validation limitation; no permissions were broadened. Inspect staged changes and check receipts.");
                    break;
                }
                if reply.done {
                    let all_passed = run.grant.checks.keys().all(|id| {
                        receipts
                            .iter()
                            .rev()
                            .find(|c| &c.recipe_id == id)
                            .is_some_and(|c| c.status == CheckStatus::Passed)
                    });
                    if all_passed {
                        run.finish(AgentStatus::ReadyForReview,if run.grant.checks.is_empty(){"Implementation staged with no check recipe authorized. Review and validate before importing."}else{"Implementation staged and the approved checks passed on its captured version. Review before importing."});
                        break;
                    }
                }
                if reply.edits.is_empty() && reply.check_requests.is_empty() {
                    run.finish(AgentStatus::ReadyForReview,"Agent stopped with a handoff. Inspect remaining limitations and staged changes.");
                    break;
                }
            }
            if run.status.active() {
                run.finish(AgentStatus::ReadyForReview,"Round limit reached. Staged changes and evidence are retained for review; work was not automatically extended.");
            }
            Ok(())
        })();
        if let Err(error) = result {
            run.finish(
                if self.cancel.load(Ordering::Relaxed) {
                    AgentStatus::Cancelled
                } else {
                    AgentStatus::Failed
                },
                format!("Delegation stopped: {error}"),
            );
        }
        if let Err(error) = inspect_delegation(&mut run) {
            run.stage_verified = false;
            run.limitations
                .push(format!("Stage inspection failed: {error}"));
            run.sequence += 1;
        }
        run.next_action = if run.stage_verified && !run.changed_paths().is_empty() {
            "Inspect the retained diff and validation receipts. Import only the changes you accept; the live project has not been changed. Task completion remains your decision.".into()
        } else {
            "Continue manually or prepare a new bounded scope. No delegation will resume automatically.".into()
        };
        run.sequence += 1;
        self.store.lock().unwrap().save_agent_run(&run)?;
        observe(&run);
        Ok(run)
    }
}

/// Staging and command-run roots must be siblings: the check sandbox rejects
/// sources nested in its artifact/cache root to prevent permission overlap.
pub fn delegation_cache(environment: &CheckEnvironment) -> Result<PathBuf> {
    let parent = environment
        .runs
        .parent()
        .context("Check runs need an absolute parent directory")?;
    ensure!(parent.is_absolute(), "Check cache must be absolute");
    let cache = parent.join("delegations");
    ensure!(
        !cache.starts_with(&environment.runs) && !environment.runs.starts_with(&cache),
        "Staging and validation caches overlap"
    );
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stage_verification_refuses_out_of_scope_files_links_and_content_changes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("project");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a.rs"), "a\n").unwrap();
        std::fs::write(source.join("context.rs"), "context\n").unwrap();
        let project = Project::open(&source).unwrap();
        let grant = DelegationGrant {
            id: new_id(),
            project: project.key().into(),
            task_id: "accepted".into(),
            plan_revision: 1,
            step_id: None,
            scope: DelegationScope::Feature,
            objective: "Edit a.rs".into(),
            paths: vec!["a.rs".into()],
            checks: Default::default(),
            max_rounds: 1,
            confirmed_at_ms: 0,
        };
        let mut run = prepare_delegation(
            &project,
            &[],
            grant,
            &temp.path().join("staging"),
            &AtomicBool::new(false),
        )
        .unwrap();
        inspect_delegation(&mut run).unwrap();
        let root = PathBuf::from(&run.directory).join("input");
        std::fs::write(root.join("context.rs"), "changed\n").unwrap();
        assert!(inspect_delegation(&mut run).is_err());
        assert!(!run.stage_verified);
        std::fs::write(root.join("context.rs"), "context\n").unwrap();
        std::fs::write(root.join("outside.rs"), "extra\n").unwrap();
        assert!(inspect_delegation(&mut run).is_err());
        std::fs::remove_file(root.join("outside.rs")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(source.join("a.rs"), root.join("linked.rs")).unwrap();
            assert!(inspect_delegation(&mut run).is_err());
            std::fs::remove_file(root.join("linked.rs")).unwrap();
        }
        inspect_delegation(&mut run).unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("context.rs")).unwrap(),
            "context\n"
        );
    }
}
