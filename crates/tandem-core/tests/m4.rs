#[path = "../../tandem-agent/examples/support/mod.rs"]
mod support;
use std::collections::BTreeMap;
use tandem_core::*;

fn fixture_task(project: &Project) -> TaskPlan {
    let mut task = TaskPlan::new(
        project.key().into(),
        "Implement a bounded change".into(),
        vec!["Preserve behavior".into()],
        vec!["Implement".into()],
    )
    .unwrap();
    task.id = "task".into();
    task.steps[0].id = "step".into();
    task
}
fn grant(project: &Project, paths: &[&str]) -> DelegationGrant {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let session = support::approved_session(&mut store, fixture_task(project)).unwrap();
    DelegationGrant {
        workflow: Some(session.authorize(WorkflowAction::Takeover).unwrap()),
        action_scope: None,
        id: new_id(),
        project: project.key().into(),
        task_id: "task".into(),
        plan_revision: 1,
        step_id: Some("step".into()),
        scope: DelegationScope::Step,
        objective: "Implement a bounded change".into(),
        paths: paths.iter().map(|s| (*s).into()).collect(),
        checks: BTreeMap::new(),
        max_rounds: 3,
        confirmed_at_ms: 1,
    }
}
fn staged(project: &Project, base: Option<Document>, after: Document) -> AgentRun {
    let mut run = AgentRun::pending(grant(project, &[&after.path]), "unused".into());
    run.baseline.insert(after.path.clone(), base);
    run.staged.insert(after.path.clone(), Some(after));
    run.stage_verified = true;
    run.finish(AgentStatus::ReadyForReview, "ready");
    run
}
#[test]
fn three_way_merge_preserves_unrelated_changes_and_refuses_ambiguous_overlaps() {
    let base = "one\ntwo\nthree\n";
    assert_eq!(
        merge_text(base, "one\nagent\nthree\n", "human\ntwo\nthree\n").unwrap(),
        "human\nagent\nthree\n"
    );
    assert!(merge_text(base, "one\nagent\nthree\n", "one\nhuman\nthree\n").is_err());
    assert!(
        merge_text(
            base,
            "insert-a\none\ntwo\nthree\n",
            "insert-b\none\ntwo\nthree\n"
        )
        .is_err()
    );
    assert_eq!(
        merge_text(
            base,
            "insert\none\ntwo\nthree\n",
            "insert\none\ntwo\nthree\n"
        )
        .unwrap(),
        "insert\none\ntwo\nthree\n"
    );
}
#[test]
fn import_and_inverse_keep_preexisting_and_later_user_work() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("code.rs"),
        "header\nimplementation\nfooter\n",
    )
    .unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut base = project.open_document("code.rs", false).unwrap();
    base.snapshot("header\nimplementation\nfooter\npreexisting draft\n".into());
    let mut agent = base.clone();
    agent.snapshot(base.text.replace("implementation", "agent implementation"));
    let run = staged(&project, Some(base.clone()), agent);
    let mut live = base;
    live.snapshot(live.text.replace("header", "later user header"));
    let import = preview_import(&run, &project, &[live])
        .unwrap()
        .transaction
        .unwrap();
    let mut after = import.files[0].after.clone().unwrap();
    assert!(
        after.text.contains("later user header")
            && after.text.contains("preexisting draft")
            && after.text.contains("agent implementation")
    );
    after.snapshot(format!("{}another user line\n", after.text));
    let undo = preview_undo(&import, &project, &[after])
        .unwrap()
        .transaction
        .unwrap();
    let text = &undo.files[0].after.as_ref().unwrap().text;
    assert!(
        text.contains("later user header")
            && text.contains("preexisting draft")
            && text.contains("another user line")
    );
    assert!(!text.contains("agent implementation"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("code.rs")).unwrap(),
        "header\nimplementation\nfooter\n"
    );
}
#[test]
fn overlapping_import_and_undo_preserve_the_user_version_as_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a"), "base\n").unwrap();
    let project = Project::open(temp.path()).unwrap();
    let base = project.open_document("a", false).unwrap();
    let mut agent = base.clone();
    agent.snapshot("agent\n".into());
    let run = staged(&project, Some(base.clone()), agent);
    let mut user = base.clone();
    user.snapshot("user\n".into());
    let conflict = preview_import(&run, &project, &[user]).unwrap();
    assert!(conflict.transaction.is_none());
    assert_eq!(conflict.conflicts[0].current.as_deref(), Some("user\n"));
    let imported = preview_import(&run, &project, &[base])
        .unwrap()
        .transaction
        .unwrap();
    let mut changed = imported.files[0].after.clone().unwrap();
    changed.snapshot("user after import\n".into());
    assert!(
        preview_undo(&imported, &project, &[changed])
            .unwrap()
            .transaction
            .is_none()
    );
}
#[test]
fn created_files_undo_only_while_their_working_version_is_unchanged_and_unsaved() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::open(temp.path()).unwrap();
    let created = Document {
        path: "nested/new.rs".into(),
        text: "agent code\n".into(),
        base_text: String::new(),
        base_hash: None,
        format: TextFormat::default(),
        revision: 1,
        recovered: false,
        conflict: false,
        read_only: false,
    };
    let run = staged(&project, None, created);
    let imported = preview_import(&run, &project, &[])
        .unwrap()
        .transaction
        .unwrap();
    let doc = imported.files[0].after.clone().unwrap();
    assert!(
        preview_undo(&imported, &project, std::slice::from_ref(&doc))
            .unwrap()
            .transaction
            .unwrap()
            .files[0]
            .after
            .is_none()
    );
    let saved = project.save(&doc).unwrap();
    assert!(temp.path().join("nested/new.rs").exists());
    assert!(
        preview_undo(&imported, &project, &[saved])
            .unwrap()
            .transaction
            .is_none()
    );
}
#[test]
fn import_journal_and_all_drafts_commit_together_and_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a"), "base\n").unwrap();
    let project = Project::open(&root).unwrap();
    let base = project.open_document("a", false).unwrap();
    let mut after = base.clone();
    after.snapshot("agent\n".into());
    let run = staged(&project, Some(base), after);
    let import = preview_import(&run, &project, &[])
        .unwrap()
        .transaction
        .unwrap();
    let working = apply_to_working_set(&[], &import);
    let mut workspace = WorkspaceState::new(&project);
    workspace.tabs = vec!["a".into()];
    workspace.sequence = 1;
    {
        let mut store = Store::open(&temp.path().join("state")).unwrap();
        let session = support::approved_session(&mut store, fixture_task(&project)).unwrap();
        let mut retained = run.clone();
        retained.grant.workflow = Some(session.authorize(WorkflowAction::Takeover).unwrap());
        store.save_agent_run(&retained).unwrap();
        store.commit_import(&workspace, &working, &import).unwrap();
        assert!(store.commit_import(&workspace, &working, &import).is_err());
    }
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    assert_eq!(store.drafts(project.key()).unwrap()[0].text, "agent\n");
    assert_eq!(
        store.imports(project.key()).unwrap()[0].status,
        ImportStatus::Applied
    );
    let undo = preview_undo(&import, &project, &working)
        .unwrap()
        .transaction
        .unwrap();
    workspace.sequence = 2;
    store
        .commit_import(&workspace, &apply_to_working_set(&working, &undo), &undo)
        .unwrap();
    assert!(store.drafts(project.key()).unwrap().is_empty());
    assert_eq!(
        store.imports(project.key()).unwrap()[0].status,
        ImportStatus::Undone
    );
    let mut replay = import.clone();
    replay.id = new_id();
    workspace.sequence += 1;
    assert!(store.commit_import(&workspace, &working, &replay).is_err());
}
#[test]
fn broker_rejects_out_of_scope_ambiguous_and_stale_batches_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a"), "same\nsame\n").unwrap();
    let project = Project::open(temp.path()).unwrap();
    let doc = project.open_document("a", false).unwrap();
    let scope = grant(&project, &["a"]);
    let docs = BTreeMap::from([("a".into(), Some(doc.clone()))]);
    let mut reply = AgentReply {
        summary: "change".into(),
        edits: vec![AgentEdit::Modify {
            path: "a".into(),
            before_hash: content_hash(doc.text.as_bytes()),
            replacements: vec![TextReplacement {
                before: "same".into(),
                after: "new".into(),
            }],
        }],
        check_requests: vec![],
        done: true,
        limitations: vec![],
        next_action: String::new(),
    };
    assert!(apply_agent_batch(&scope, &docs, &reply).is_err());
    reply.edits = vec![AgentEdit::Create {
        path: "outside".into(),
        content: "data".into(),
    }];
    assert!(apply_agent_batch(&scope, &docs, &reply).is_err());
    reply.edits.clear();
    reply.check_requests.push("unapproved-command".into());
    assert!(apply_agent_batch(&scope, &docs, &reply).is_err());
    let mut collision = scope.clone();
    collision.paths = vec!["A.rs".into(), "a.rs".into()];
    assert!(collision.validate().is_err());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("a")).unwrap(),
        doc.text
    );
}
#[test]
fn grant_scope_cannot_expand_and_interrupted_delegation_does_not_restart() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut run = AgentRun::pending(grant(&project, &["a"]), "unused".into());
    run.status = AgentStatus::Running;
    {
        let mut store = Store::open(&temp.path().join("state")).unwrap();
        store.save_agent_run(&run).unwrap();
        let mut expanded = run.clone();
        expanded.grant.paths.push("b".into());
        expanded.sequence += 1;
        assert!(store.save_agent_run(&expanded).is_err());
    }
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    let recovered = store.agent_runs(project.key()).unwrap().remove(0);
    assert_eq!(recovered.status, AgentStatus::Interrupted);
    assert!(!recovered.stage_verified);
    run.sequence = recovered.sequence + 1;
    assert!(store.save_agent_run(&run).is_err());
}

#[test]
fn benchmark_receipts_require_the_fixed_method_and_complete_bounded_samples() {
    let mut measured = BenchmarkMeasurement {
        method: "perf_counter_ns; alternating order; deepcopy/setup excluded".into(),
        warmups: 2,
        samples: 7,
        cases: vec![BenchmarkCase {
            label: "sorted n=1024; repeated lookups".into(),
            baseline_ns: vec![100; 7],
            candidate_ns: vec![20; 7],
        }],
    };
    measured.validate().unwrap();
    assert!(measured.summary().contains("median 20"));
    measured.cases[0].candidate_ns.pop();
    assert!(measured.validate().is_err());
    measured.cases[0].candidate_ns.push(0);
    assert!(measured.validate().is_err());
    measured.cases[0].candidate_ns[6] = 20;
    measured.cases.push(measured.cases[0].clone());
    assert!(measured.validate().is_err());
    measured.cases.pop();
    measured.method = "model estimate".into();
    assert!(measured.validate().is_err());
}

#[test]
fn import_rejects_external_disk_changes_and_symlink_retargets() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("code.rs"), "base\n").unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut draft = project.open_document("code.rs", false).unwrap();
    draft.snapshot("base\nuser draft\n".into());
    let mut after = draft.clone();
    after.snapshot("agent\nuser draft\n".into());
    let run = staged(&project, Some(draft.clone()), after);
    std::fs::write(temp.path().join("code.rs"), "external\n").unwrap();
    assert!(
        preview_import(&run, &project, &[draft])
            .unwrap()
            .transaction
            .is_none()
    );
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
        assert!(validate_import_path(&project, "link/new.rs").is_err());
    }
}

#[test]
fn confirmed_scope_interrupted_before_dispatch_cannot_be_replayed() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut run = AgentRun::pending(grant(&project, &["a"]), "unused".into());
    run.status = AgentStatus::Prepared;
    {
        let mut store = Store::open(&temp.path().join("state")).unwrap();
        store.save_agent_run(&run).unwrap();
    }
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    let mut restored = store.agent_runs(project.key()).unwrap().remove(0);
    assert_eq!(restored.status, AgentStatus::Interrupted);
    restored.status = AgentStatus::Prepared;
    restored.sequence += 1;
    assert!(store.save_agent_run(&restored).is_err());
}

#[test]
fn running_check_intent_is_durable_before_dispatch_with_a_new_journal_version() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut run = AgentRun::pending(grant(&project, &["a"]), "unused".into());
    run.status = AgentStatus::Checking;
    run.activity("Running an approved check");
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    store.save_agent_run(&run).unwrap();
    let previous = run.sequence;
    run.record_check_intent("pending-check-receipt".into());
    store.save_agent_run(&run).unwrap();
    let saved = store.agent_runs(project.key()).unwrap().remove(0);
    assert!(saved.sequence > previous);
    assert_eq!(saved.checks, vec!["pending-check-receipt"]);
    let mut stale = saved.clone();
    stale.summary = "Changed without a new version".into();
    assert!(store.save_agent_run(&stale).is_err());
}

#[test]
fn takeover_requires_current_durable_approvals_start_and_generation() {
    let temp = tempfile::tempdir().unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    let mut session = support::approved_session(&mut store, fixture_task(&project)).unwrap();
    let mut scope = grant(&project, &["a"]);
    scope.workflow = Some(session.authorize(WorkflowAction::Takeover).unwrap());
    store.authorize_delegation(&scope).unwrap();
    let mut action_session = session.clone();
    action_session.walkthroughs = vec![support::fixture_walkthrough(&action_session)];
    let mut action_grant = scope.clone();
    action_grant.action_scope = DelegationActionScope::current(&action_session);
    action_grant.validate_workflow(&action_session).unwrap();
    let mut next = action_session.clone();
    next.walkthroughs[0].cursor = 1;
    assert!(action_grant.validate_workflow(&next).is_err());
    let mut replacement = action_session.clone();
    replacement.walkthroughs[0].id = new_id();
    assert!(action_grant.validate_workflow(&replacement).is_err());
    // A discussion is conversation state only and cannot revoke or create authority.
    store
        .append_user_message(
            &session.id,
            session.sequence,
            &new_id(),
            "Discuss the approach",
        )
        .unwrap();
    session = store.load_session(&session.id).unwrap().unwrap();
    store.authorize_delegation(&scope).unwrap();
    let mut changed = session.spec.clone().unwrap();
    changed.summary = "Changed requirements".into();
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(new_id(), vec![SessionCommand::SaveSpec { spec: changed }]),
        )
        .unwrap();
    assert!(store.authorize_delegation(&scope).is_err());
    assert!(scope.validate_workflow(&session).is_err());
    let mut legacy = scope.clone();
    legacy.workflow = None;
    assert!(store.authorize_delegation(&legacy).is_err());
    // Legacy grant data remains readable without gaining new authority.
    let mut value = serde_json::to_value(&scope).unwrap();
    value.as_object_mut().unwrap().remove("workflow");
    value.as_object_mut().unwrap().remove("action_scope");
    let decoded: DelegationGrant = serde_json::from_value(value).unwrap();
    decoded.validate().unwrap();
    assert!(store.authorize_delegation(&decoded).is_err());
}

#[test]
fn import_revalidates_current_approval_and_preserves_staged_work() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a"), "base\n").unwrap();
    let project = Project::open(temp.path()).unwrap();
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    let mut session = support::approved_session(&mut store, fixture_task(&project)).unwrap();
    let base = project.open_document("a", false).unwrap();
    let mut after = base.clone();
    after.snapshot("agent\n".into());
    let mut run = staged(&project, Some(base), after);
    run.grant.workflow = Some(session.authorize(WorkflowAction::Takeover).unwrap());
    store.save_agent_run(&run).unwrap();
    let group = preview_import(&run, &project, &[])
        .unwrap()
        .transaction
        .unwrap();
    let working = apply_to_working_set(&[], &group);
    let mut state = WorkspaceState::new(&project);
    state.sequence = 1;
    let mut plan = session.plan.clone().unwrap();
    plan.task.goal = "A revised objective".into();
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(new_id(), vec![SessionCommand::SavePlan { plan }]),
        )
        .unwrap();
    assert!(session.authorize(WorkflowAction::Takeover).is_err());
    assert!(store.commit_import(&state, &working, &group).is_err());
    assert!(store.drafts(project.key()).unwrap().is_empty());
    assert_eq!(
        store.agent_runs(project.key()).unwrap()[0].staged["a"]
            .as_ref()
            .unwrap()
            .text,
        "agent\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("a")).unwrap(),
        "base\n"
    );
}
