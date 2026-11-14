use tandem_core::*;

#[test]
fn schema_one_migration_preserves_drafts_and_makes_a_backup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.sqlite");
    let project_dir = tempfile::tempdir().unwrap();
    let project = Project::open(project_dir.path()).unwrap();
    let mut draft = project.open_document("draft.rs", true).unwrap();
    draft.snapshot("unsaved before migration 🦀".into());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE workspace(root TEXT PRIMARY KEY,data TEXT NOT NULL,opened INTEGER NOT NULL);CREATE TABLE draft(root TEXT NOT NULL,path TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(root,path));CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);PRAGMA user_version=1;").unwrap();
        connection
            .execute(
                "INSERT INTO settings VALUES('checkpoint_sequence','37')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO draft(root,path,data) VALUES(?1,?2,?3)",
                rusqlite::params![
                    project.key(),
                    draft.path,
                    serde_json::to_string(&draft).unwrap()
                ],
            )
            .unwrap();
    }
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.sequence().unwrap(), 37);
    assert_eq!(store.drafts(project.key()).unwrap()[0].text, draft.text);
    assert!(std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("workspace-schema1-backup-")
    }));
}

#[test]
fn task_progress_survives_restart_and_stale_or_unversioned_edits_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let mut task = TaskPlan::new(
        "project".into(),
        "Goal".into(),
        vec!["Criterion".into()],
        vec!["Step".into()],
    )
    .unwrap();
    {
        let mut store = Store::open(directory.path()).unwrap();
        store.save_task(&task).unwrap();
        let stale = task.clone();
        task.start_step(&task.steps[0].id.clone()).unwrap();
        store.save_task(&task).unwrap();
        assert!(store.save_task(&stale).is_err());
        let mut invalid = task.clone();
        invalid.goal = "Changed without a revision".into();
        invalid.state_sequence += 1;
        assert!(store.save_task(&invalid).is_err());
    }
    let store = Store::open(directory.path()).unwrap();
    let loaded = store.tasks("project").unwrap();
    assert_eq!(loaded[0].steps[0].status, StepStatus::Active);
}

#[test]
fn restart_marks_only_unfinished_checks_interrupted_and_preserves_terminal_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let mut running = CheckRecord::pending("p".into(), "recipe".into(), "fingerprint".into());
    running.status = CheckStatus::Running;
    running.output = "already completed output".into();
    let mut passed = CheckRecord::pending("p".into(), "recipe".into(), "fingerprint".into());
    passed.finish(CheckStatus::Passed, "done");
    {
        let mut store = Store::open(directory.path()).unwrap();
        store.save_check(&running).unwrap();
        store.save_check(&passed).unwrap();
    }
    let mut store = Store::open(directory.path()).unwrap();
    let records = store.checks("p").unwrap();
    let recovered = records.iter().find(|r| r.id == running.id).unwrap();
    assert_eq!(recovered.status, CheckStatus::Interrupted);
    assert_eq!(recovered.output, running.output);
    store.save_check(&running).unwrap();
    assert_eq!(
        store
            .checks("p")
            .unwrap()
            .iter()
            .find(|r| r.id == running.id)
            .unwrap()
            .status,
        CheckStatus::Interrupted
    );
    assert_eq!(
        records.iter().find(|r| r.id == passed.id).unwrap().status,
        CheckStatus::Passed
    );
}
