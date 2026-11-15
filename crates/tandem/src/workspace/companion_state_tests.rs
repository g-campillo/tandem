use super::*;

#[test]
fn every_implementation_route_requires_current_approvals_and_start() {
    let session = TaskSession::new("/fixture".into()).unwrap();
    for action in [
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Review,
        CompanionAction::Reveal,
        CompanionAction::Delegate,
        CompanionAction::ImproveClarity,
        CompanionAction::ImproveSpeed,
        CompanionAction::ImproveBalanced,
    ] {
        assert!(
            command_binding(Some(&session), action, false).is_err(),
            "{action:?} bypassed approvals"
        );
    }
    for action in [
        CompanionAction::Analyze,
        CompanionAction::Ask,
        CompanionAction::Discuss,
        CompanionAction::Explain,
        CompanionAction::DraftSpec,
        CompanionAction::DraftPlan,
    ] {
        assert!(command_binding(Some(&session), action, false).is_ok());
        assert!(command_binding(Some(&session), action, true).is_err());
    }
    assert!(command_binding(None, CompanionAction::Guide, false).is_err());
}

#[test]
fn ordinary_chat_never_selects_reveal_and_dirty_approvals_are_refused() {
    assert_eq!(
        workflow_action(CompanionAction::Ask),
        WorkflowAction::Discuss
    );
    let saved = DocumentRef {
        revision: 1,
        hash: "saved".into(),
    };
    assert!(approval_matches(true, Some(&saved), Some(&saved)).is_err());
    assert!(approval_matches(false, Some(&saved), Some(&saved)).is_ok());
    assert!(approval_matches(false, Some(&saved), None).is_err());
}

#[test]
fn unavailable_model_choice_is_retained_and_default_is_hidden() {
    let preference = ModelPreference::Explicit {
        model: "chosen-model".into(),
    };
    assert!(model_label(&preference, None).contains("chosen-model · unavailable"));
    assert_eq!(
        preference,
        ModelPreference::Explicit {
            model: "chosen-model".into()
        }
    );
    let session = TaskSession::new("/fixture".into()).unwrap();
    let guidance = GuidanceSettings::default().effective("/fixture", "rust", None);
    assert!(preview_key(&session, "rust", &guidance).is_none());
}

#[path = "../../../tandem-agent/examples/support/mod.rs"]
mod support;
fn approved() -> (tempfile::TempDir, Store, TaskSession) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut task = TaskPlan::new(
        "/fixture".into(),
        "Add a greeting".into(),
        vec!["Return the greeting".into()],
        vec!["Implement".into()],
    )
    .unwrap();
    task.steps[0].languages = vec!["rust".into()];
    task.steps[0].done_when = vec!["A greeting is available".into()];
    let session = support::approved_session(&mut store, task).unwrap();
    (dir, store, session)
}
fn with_actions(session: &mut TaskSession) {
    let walkthrough = support::fixture_walkthrough(session);
    walkthrough
        .validate(
            session.spec.as_ref().unwrap(),
            session.plan.as_ref().unwrap(),
        )
        .unwrap();
    session.walkthroughs = vec![walkthrough];
}
#[test]
fn command_matrix_tracks_each_approval_start_and_document_replacement() {
    let (_dir, mut store, session) = approved();
    let routes = [
        CompanionAction::Guide,
        CompanionAction::Hint,
        CompanionAction::Review,
        CompanionAction::ReviewAction,
        CompanionAction::Reveal,
        CompanionAction::Delegate,
        CompanionAction::ImproveClarity,
        CompanionAction::ImproveSpeed,
        CompanionAction::ImproveBalanced,
    ];
    for action in routes {
        assert!(command_binding(Some(&session), action, false).is_ok());
    }
    for missing in 0..4 {
        let mut s = session.clone();
        match missing {
            0 => s.spec = None,
            1 => s.spec_approval = None,
            2 => s.plan_approval = None,
            _ => s.started_generation = None,
        };
        for action in routes {
            assert!(command_binding(Some(&s), action, false).is_err());
        }
    }
    let binding = session.authorize(WorkflowAction::Reveal).unwrap();
    let same = store
        .append_user_message(
            &session.id,
            session.sequence,
            &new_id(),
            "Write a function, but discuss it first",
        )
        .unwrap();
    assert!(same.binding_is_current(WorkflowAction::Reveal, &binding));
    let mut changed = same.spec.clone().unwrap();
    changed.summary = "A changed greeting".into();
    let revised = store
        .apply_session_transition(
            &same.id,
            same.sequence,
            &SessionTransition::new(new_id(), vec![SessionCommand::SaveSpec { spec: changed }]),
        )
        .unwrap();
    assert!(revised.spec_approval.is_none() && revised.plan_approval.is_none());
    for action in routes {
        assert!(command_binding(Some(&revised), action, false).is_err());
    }
    assert!(!revised.binding_is_current(WorkflowAction::Reveal, &binding));
}
#[test]
fn preview_stays_on_current_action_language_and_never_exposes_stale_solution() {
    let (_dir, _store, mut session) = approved();
    with_actions(&mut session);
    let mut settings = GuidanceSettings::default();
    settings.defaults.solution = Some(SolutionPolicy::AutomaticPreview);
    let guidance = settings.effective("/fixture", "rust", session.plan.as_ref().map(|p| &p.task));
    let key = preview_key(&session, "rust", &guidance).unwrap();
    assert_eq!(key, preview_key(&session, "rust", &guidance).unwrap());
    assert!(preview_key(&session, "python", &guidance).is_none());
    let scope = SolutionScope {
        workflow: session.authorize(WorkflowAction::Reveal).unwrap(),
        action_scope: DelegationActionScope::current(&session).unwrap(),
        language: "rust".into(),
        guidance: guidance.fingerprint(),
    };
    assert!(solution_visible(&scope, Some(&session), "rust", &guidance));
    assert!(!solution_visible(
        &scope,
        Some(&session),
        "python",
        &guidance
    ));
    session.walkthroughs[0].cursor = 1;
    assert_ne!(key, preview_key(&session, "rust", &guidance).unwrap());
    assert!(!solution_visible(&scope, Some(&session), "rust", &guidance));
    session.started_generation = None;
    assert!(preview_key(&session, "rust", &guidance).is_none());
}
#[test]
fn stable_question_answer_and_composer_survive_session_switch_and_restart() {
    let (dir, mut store, mut session) = approved();
    let question = ClarificationQuestion {
        id: "question".into(),
        prompt: "Which greeting?".into(),
        choices: vec![
            QuestionChoice {
                id: "short".into(),
                label: "Short".into(),
                description: "Brief".into(),
            },
            QuestionChoice {
                id: "long".into(),
                label: "Long".into(),
                description: "Detailed".into(),
            },
        ],
    };
    session = store
        .apply_session_transition(
            &session.id,
            session.sequence,
            &SessionTransition::new(
                new_id(),
                vec![SessionCommand::AskQuestion {
                    question,
                    run_id: None,
                }],
            ),
        )
        .unwrap();
    store
        .save_composer_draft(&session.id, 0, "Follow-up in progress")
        .unwrap();
    let other = store.create_session("/fixture").unwrap();
    store.select_session("/fixture", &session.id).unwrap();
    drop(store);
    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store.selected_session("/fixture").unwrap(),
        Some(session.id.clone())
    );
    assert_eq!(
        store.composer_draft(&session.id).unwrap().text,
        "Follow-up in progress"
    );
    assert!(store.composer_draft(&other.id).unwrap().text.is_empty());
    let answer = ClarificationAnswer {
        question_id: "question".into(),
        choice_id: None,
        custom_text: Some("A friendly greeting".into()),
    };
    let answered = store
        .record_question_answer(&session.id, session.sequence, &new_id(), answer.clone())
        .unwrap();
    assert!(answered.pending_question.is_none());
    assert!(
        store
            .record_question_answer(&session.id, answered.sequence, &new_id(), answer)
            .is_err()
    );
    assert!(answered.spec_approval.is_some() && answered.plan_approval.is_some());
}

#[test]
fn pending_answer_draft_is_retained_during_chat_failure_and_new_typing() {
    assert!(!clear_submitted_input(true, None, "My custom answer"));
    assert!(!clear_submitted_input(
        false,
        Some("My custom answer"),
        "My custom answer"
    ));
    assert!(!clear_submitted_input(
        true,
        Some("My custom answer"),
        "A revised answer"
    ));
    assert!(clear_submitted_input(
        true,
        Some("My custom answer"),
        "My custom answer"
    ));
}
