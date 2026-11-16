use rusqlite::{Connection, params, types::Value};
use std::path::Path;
use std::time::Duration;
use tandem_core::Store;
use tandem_core::chat::{
    ActivityKind, BreakdownItem, ChangeKind, ChangeSet, ChangeSetStatus, ChatPrefs, Decision,
    FileChange, Guidance, HelpLevel, ItemKind, Mode, PermissionMode, Phase, Plan, PlanStep,
    StepKind, TakeoverScope,
};

/// Lets `now_ms()` move on so update-time ordering is observable.
fn tick() {
    std::thread::sleep(Duration::from_millis(3));
}

fn plan() -> Plan {
    Plan {
        goal: "Print results as JSON when --json is set".into(),
        done_when: vec!["Output parses as JSON".into()],
        steps: vec![PlanStep {
            kind: StepKind::EditFile,
            target: "src/cli.rs".into(),
            summary: "Add json to Args".into(),
            language: "rust".into(),
        }],
    }
}

fn guidance(step: usize, level: HelpLevel, summary: &str) -> Guidance {
    Guidance {
        step,
        level,
        summary: summary.into(),
        why: "clap turns each field into a flag".into(),
        code: Some("json: bool,".into()),
        pseudocode: None,
        command: None,
        breakdown: vec![BreakdownItem {
            part: "bool".into(),
            explanation: "a true or false value".into(),
        }],
        path: Some("src/cli.rs".into()),
        anchor: Some("struct Args".into()),
    }
}

fn change_set(id: &str, task_id: &str, status: ChangeSetStatus, created: i64) -> ChangeSet {
    ChangeSet {
        id: id.into(),
        task_id: task_id.into(),
        scope: TakeoverScope::Rest(1),
        status,
        files: vec![FileChange {
            path: "src/cli.rs".into(),
            kind: ChangeKind::Modified,
            decision: Decision::Undecided,
            after_hash: Some("abc".into()),
            undoable: true,
        }],
        created_at_ms: created,
    }
}

#[test]
fn preferences_default_when_absent_and_survive_a_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let mut prefs = {
        let store = Store::open(directory.path()).unwrap();
        let prefs = store.chat_prefs().unwrap();
        assert_eq!(prefs, ChatPrefs::default());
        prefs
    };
    prefs.model = "opus".into();
    prefs.effort = Some("high".into());
    prefs.use_my_setup = false;
    prefs.permission_mode = PermissionMode::Bypass;
    prefs.help_levels.insert("rust".into(), HelpLevel::Expert);
    prefs
        .help_levels
        .insert("python".into(), HelpLevel::Intermediate);
    Store::open(directory.path())
        .unwrap()
        .save_chat_prefs(&prefs)
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.chat_prefs().unwrap(), prefs);
}

#[test]
fn creating_a_task_stores_and_selects_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    assert_eq!(store.selected_chat_task("/p").unwrap(), None);
    let first = store.create_chat_task("/p").unwrap();
    assert_eq!(first.project, "/p");
    assert_eq!(store.chat_task(&first.id).unwrap(), Some(first.clone()));
    assert_eq!(
        store.selected_chat_task("/p").unwrap(),
        Some(first.id.clone())
    );
    let second = store.create_chat_task("/p").unwrap();
    assert_eq!(store.selected_chat_task("/p").unwrap(), Some(second.id));
    assert_eq!(store.chat_task("missing").unwrap(), None);
}

#[test]
fn task_list_is_most_recently_updated_first_and_per_project() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let a = store.create_chat_task("/p").unwrap();
    tick();
    let b = store.create_chat_task("/p").unwrap();
    tick();
    let other = store.create_chat_task("/q").unwrap();
    let ids = |store: &Store| {
        store
            .chat_tasks("/p")
            .unwrap()
            .into_iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&store), [b.id.clone(), a.id.clone()]);
    tick();
    let mut renamed = a.clone();
    renamed.title = "Add a --json flag".into();
    store.save_chat_task(&renamed).unwrap();
    assert_eq!(ids(&store), [a.id.clone(), b.id.clone()]);
    let summary = &store.chat_tasks("/p").unwrap()[0];
    assert_eq!(summary.title, "Add a --json flag");
    assert_eq!(summary.phase, Phase::Discuss);
    assert!(summary.updated_at_ms > a.updated_at_ms);
    let others = store.chat_tasks("/q").unwrap();
    assert_eq!(others.len(), 1);
    assert_eq!(others[0].id, other.id);
}

#[test]
fn saved_task_state_survives_a_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let mut task = Store::open(directory.path())
        .unwrap()
        .create_chat_task("/p")
        .unwrap();
    task.title = "Add a --json flag".into();
    task.phase = Phase::Build;
    task.mode = Mode::Takeover;
    task.plan = Some(plan());
    task.current_step = 2;
    task.session_id = "6d1f6f3c-6a5e-4a40-9a39-0f1e3c1f2a10".into();
    task.session_started = true;
    tick();
    Store::open(directory.path())
        .unwrap()
        .save_chat_task(&task)
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    let loaded = store.chat_task(&task.id).unwrap().unwrap();
    assert!(loaded.updated_at_ms > task.updated_at_ms);
    assert_eq!(
        loaded,
        tandem_core::chat::ChatTask {
            updated_at_ms: loaded.updated_at_ms,
            ..task
        }
    );
}

#[test]
fn saving_refuses_unknown_tasks_and_project_moves() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let never_created = tandem_core::chat::ChatTask::new("/p");
    assert!(store.save_chat_task(&never_created).is_err());
    assert_eq!(store.chat_task(&never_created.id).unwrap(), None);
    let task = store.create_chat_task("/p").unwrap();
    let mut moved = task.clone();
    moved.project = "/q".into();
    assert!(store.save_chat_task(&moved).is_err());
    assert_eq!(store.chat_task(&task.id).unwrap(), Some(task));
    assert!(store.chat_tasks("/q").unwrap().is_empty());
}

#[test]
fn selection_survives_a_reopen_and_stays_in_its_project() {
    let directory = tempfile::tempdir().unwrap();
    let (a, other) = {
        let mut store = Store::open(directory.path()).unwrap();
        let a = store.create_chat_task("/p").unwrap();
        store.create_chat_task("/p").unwrap();
        let other = store.create_chat_task("/q").unwrap();
        store.select_chat_task("/p", &a.id).unwrap();
        assert!(store.select_chat_task("/p", &other.id).is_err());
        assert!(store.select_chat_task("/p", "missing").is_err());
        (a, other)
    };
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.selected_chat_task("/p").unwrap(), Some(a.id));
    assert_eq!(store.selected_chat_task("/q").unwrap(), Some(other.id));
}

#[test]
fn items_keep_their_order_and_belong_to_one_task() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let task = store.create_chat_task("/p").unwrap();
    let other = store.create_chat_task("/p").unwrap();
    let kinds = [
        ItemKind::User {
            text: "Add a --json flag".into(),
        },
        ItemKind::Activity {
            kind: ActivityKind::Read,
            label: "src/cli.rs".into(),
        },
        ItemKind::Permission {
            title: "Run npm install".into(),
            detail: "Installs a package".into(),
            command: Some("npm install left-pad".into()),
            allowed: None,
        },
    ];
    let mut appended = Vec::new();
    for kind in &kinds {
        appended.push(store.append_chat_item(&task.id, kind).unwrap());
        store
            .append_chat_item(
                &other.id,
                &ItemKind::Notice {
                    text: "elsewhere".into(),
                },
            )
            .unwrap();
    }
    assert!(appended.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert_eq!(
        appended.iter().map(|item| &item.kind).collect::<Vec<_>>(),
        kinds.iter().collect::<Vec<_>>()
    );
    assert_eq!(store.chat_items(&task.id).unwrap(), appended);

    let answered = ItemKind::Permission {
        title: "Run npm install".into(),
        detail: "Installs a package".into(),
        command: Some("npm install left-pad".into()),
        allowed: Some(true),
    };
    store.update_chat_item(appended[2].id, &answered).unwrap();
    let items = store.chat_items(&task.id).unwrap();
    assert_eq!(items[..2], appended[..2]);
    assert_eq!(items[2].id, appended[2].id);
    assert_eq!(items[2].created_at_ms, appended[2].created_at_ms);
    assert_eq!(items[2].kind, answered);
    assert!(store.update_chat_item(i64::MAX, &answered).is_err());

    let others = store.chat_items(&other.id).unwrap();
    assert_eq!(others.len(), 3);
    assert!(others.iter().all(|item| matches!(
        &item.kind,
        ItemKind::Notice { text } if text == "elsewhere"
    )));
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.chat_items(&task.id).unwrap(), items);
}

#[test]
fn guidance_is_stored_per_task_step_and_level() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let task = store.create_chat_task("/p").unwrap();
    let other = store.create_chat_task("/p").unwrap();
    let beginner = guidance(0, HelpLevel::Beginner, "first");
    store.save_chat_guidance(&task.id, &beginner).unwrap();
    assert_eq!(
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap(),
        Some(beginner.clone())
    );
    assert_eq!(
        store.chat_guidance(&task.id, 0, HelpLevel::Expert).unwrap(),
        None
    );
    assert_eq!(
        store
            .chat_guidance(&task.id, 1, HelpLevel::Beginner)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .chat_guidance(&other.id, 0, HelpLevel::Beginner)
            .unwrap(),
        None
    );

    let replaced = guidance(0, HelpLevel::Beginner, "second");
    store.save_chat_guidance(&task.id, &replaced).unwrap();
    let expert = guidance(0, HelpLevel::Expert, "expert");
    store.save_chat_guidance(&task.id, &expert).unwrap();
    let next_step = guidance(1, HelpLevel::Beginner, "next");
    store.save_chat_guidance(&task.id, &next_step).unwrap();
    assert_eq!(
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap(),
        Some(replaced)
    );

    store
        .delete_chat_guidance(&task.id, 0, HelpLevel::Beginner)
        .unwrap();
    assert_eq!(
        store
            .chat_guidance(&task.id, 0, HelpLevel::Beginner)
            .unwrap(),
        None
    );
    assert_eq!(
        store.chat_guidance(&task.id, 0, HelpLevel::Expert).unwrap(),
        Some(expert)
    );
    assert_eq!(
        store
            .chat_guidance(&task.id, 1, HelpLevel::Beginner)
            .unwrap(),
        Some(next_step)
    );
}

#[test]
fn change_sets_are_saved_replaced_and_listed() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let task = store.create_chat_task("/p").unwrap();
    let sibling = store.create_chat_task("/p").unwrap();
    let elsewhere = store.create_chat_task("/q").unwrap();
    let first = change_set("first", &task.id, ChangeSetStatus::Open, 10);
    let second = change_set("second", &task.id, ChangeSetStatus::Review, 20);
    let sibling_open = change_set("sibling", &sibling.id, ChangeSetStatus::Open, 15);
    let other_project = change_set("other", &elsewhere.id, ChangeSetStatus::Open, 5);
    for set in [&second, &first, &sibling_open, &other_project] {
        store.save_change_set(set).unwrap();
    }
    assert_eq!(
        store.change_sets(&task.id).unwrap(),
        [first.clone(), second.clone()]
    );
    assert_eq!(
        store.open_change_sets("/p").unwrap(),
        [first.clone(), sibling_open.clone()]
    );

    let mut resolved = first.clone();
    resolved.status = ChangeSetStatus::Resolved;
    resolved.files[0].decision = Decision::Kept;
    store.save_change_set(&resolved).unwrap();
    assert_eq!(store.change_sets(&task.id).unwrap(), [resolved, second]);
    assert_eq!(store.open_change_sets("/p").unwrap(), [sibling_open]);
    assert_eq!(store.open_change_sets("/q").unwrap(), [other_project]);
}

fn rows(connection: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn user_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

#[test]
fn schema_five_migration_backs_up_and_keeps_old_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.sqlite");
    let before = {
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("
            CREATE TABLE task(id TEXT PRIMARY KEY, root TEXT NOT NULL, revision INTEGER NOT NULL, updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE task_session(id TEXT PRIMARY KEY, root TEXT NOT NULL, sequence INTEGER NOT NULL, task_id TEXT UNIQUE, updated INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE TABLE conversation_event(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,sequence));
            PRAGMA user_version=5;").unwrap();
        connection
            .execute(
                "INSERT INTO task VALUES('old-task','/p',3,1700000000000,?1)",
                ["{\"title\":\"Old-workflow task \u{e9}\"}"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_session VALUES('old-session','/p',7,'old-task',1700000000001,?1)",
                ["{\"phase\":\"Implementing\"}"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_event VALUES('old-event','old-session',1,?1)",
                params!["{\"text\":\"hello\"}"],
            )
            .unwrap();
        ["task", "task_session", "conversation_event"].map(|table| rows(&connection, table))
    };

    let store = Store::open(directory.path()).unwrap();
    assert!(store.chat_tasks("/p").unwrap().is_empty());
    assert_eq!(store.selected_chat_task("/p").unwrap(), None);
    drop(store);

    let backup = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("workspace-schema5-backup-")
        })
        .expect("a schema 5 backup");
    assert_eq!(user_version(&backup), 5);
    assert_eq!(user_version(&path), 6);
    let connection = Connection::open(&path).unwrap();
    let after =
        ["task", "task_session", "conversation_event"].map(|table| rows(&connection, table));
    assert_eq!(after, before);
    assert!(before.iter().all(|table| table.len() == 1));
}

#[test]
fn a_new_store_is_created_at_schema_six_without_a_backup() {
    let directory = tempfile::tempdir().unwrap();
    drop(Store::open(directory.path()).unwrap());
    assert_eq!(user_version(&directory.path().join("workspace.sqlite")), 6);
    assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("backup")
    }));
    // Reopening a schema-6 store does not make a backup either.
    drop(Store::open(directory.path()).unwrap());
    assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("backup")
    }));
}

#[test]
fn a_new_store_has_only_the_tables_still_in_use() {
    let directory = tempfile::tempdir().unwrap();
    drop(Store::open(directory.path()).unwrap());
    let connection = Connection::open(directory.path().join("workspace.sqlite")).unwrap();
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for old in [
        "task",
        "task_session",
        "model_run",
        "check_run",
        "agent_run",
        "import_group",
        "conversation_event",
    ] {
        assert!(
            !tables.iter().any(|table| table == old),
            "{old} is in {tables:?}"
        );
    }
    for kept in [
        "workspace",
        "draft",
        "settings",
        "project_tools",
        "chat_task",
        "chat_selection",
        "chat_item",
        "chat_guidance",
        "chat_change_set",
    ] {
        assert!(
            tables.iter().any(|table| table == kept),
            "{kept} is missing from {tables:?}"
        );
    }
}
