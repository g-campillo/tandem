use std::fs;
use std::sync::atomic::AtomicBool;
use tandem_core::*;

fn fixture() -> (tempfile::TempDir, Project) {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::open(directory.path()).unwrap();
    (directory, project)
}

#[test]
fn save_preserves_bom_crlf_unicode_and_executable_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_directory, project) = fixture();
    let path = project.root.join("script.py");
    fs::write(&path, "\u{feff}# café 🦀\r\nprint('hello')\r\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut document = project.open_document("script.py", false).unwrap();
    assert_eq!(document.format.line_ending, LineEnding::CrLf);
    document.snapshot(format!("{}# e\u{301}\n", document.text));
    let saved = project.save(&document).unwrap();
    assert!(!saved.dirty());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "\u{feff}# café 🦀\r\nprint('hello')\r\n# e\u{301}\r\n"
    );
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn external_edits_and_deletion_block_save_without_losing_either_version() {
    let (_directory, project) = fixture();
    let path = project.root.join("code.rs");
    fs::write(&path, "original").unwrap();
    let mut document = project.open_document("code.rs", false).unwrap();
    document.snapshot("unsaved work".into());
    fs::write(&path, "external work").unwrap();
    assert!(project.save(&document).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external work");
    assert_eq!(document.text, "unsaved work");
    assert!(matches!(
        project.disk_change(&document),
        DiskChange::Changed(_)
    ));
    fs::remove_file(&path).unwrap();
    assert!(project.save(&document).is_err());
    assert!(!path.exists());
}

#[test]
fn new_file_does_not_overwrite_a_concurrent_creation() {
    let (_directory, project) = fixture();
    let mut document = project.open_document("new.py", true).unwrap();
    document.snapshot("mine".into());
    fs::write(project.root.join("new.py"), "theirs").unwrap();
    assert!(project.save(&document).is_err());
    assert_eq!(
        fs::read_to_string(project.root.join("new.py")).unwrap(),
        "theirs"
    );
}

#[test]
fn unmodified_mixed_endings_round_trip_without_normalization() {
    let (_directory, project) = fixture();
    fs::write(project.root.join("mixed.txt"), "one\r\ntwo\n").unwrap();
    let document = project.open_document("mixed.txt", false).unwrap();
    assert_eq!(document.format.line_ending, LineEnding::Mixed);
    project.save(&document).unwrap();
    assert_eq!(
        fs::read_to_string(project.root.join("mixed.txt")).unwrap(),
        "one\r\ntwo\n"
    );
}

#[test]
fn readonly_binary_oversized_and_escaping_paths_fail_explicitly() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_directory, project) = fixture();
    fs::write(project.root.join("binary"), [0, 1, 2]).unwrap();
    assert!(project.open_document("binary", false).is_err());
    let file = fs::File::create(project.root.join("large")).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    assert!(project.open_document("large", false).is_err());
    fs::write(project.root.join("readonly"), "original").unwrap();
    fs::set_permissions(
        project.root.join("readonly"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    let mut document = project.open_document("readonly", false).unwrap();
    document.snapshot("draft".into());
    assert!(project.save(&document).is_err());
    assert!(project.resolve("../outside").is_err());
    assert!(project.resolve("/absolute").is_err());
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "outside").unwrap();
    symlink(outside.path().join("secret"), project.root.join("link")).unwrap();
    assert!(project.open_document("link", false).is_err());
}

#[test]
fn plain_folder_index_honors_ignores_and_search_is_bounded_and_cancelable() {
    let (_directory, project) = fixture();
    fs::write(project.root.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(project.root.join("ignored.txt"), "needle").unwrap();
    fs::write(
        project.root.join("visible.txt"),
        "needle\n".repeat(MAX_SEARCH_RESULTS + 10),
    )
    .unwrap();
    let index = project.index();
    assert!(!index.files.contains(&"ignored.txt".to_string()));
    assert!(index.files.contains(&"visible.txt".to_string()));
    assert_eq!(
        project
            .search(&index.files, "needle", &AtomicBool::new(false))
            .len(),
        MAX_SEARCH_RESULTS
    );
    assert!(
        project
            .search(&index.files, "needle", &AtomicBool::new(true))
            .is_empty()
    );
}

#[test]
#[cfg(unix)]
fn store_releases_lock_after_drop_while_a_forked_child_waits_to_exec() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::{net::UnixStream, process::CommandExt};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    let (mut parent, child) = UnixStream::pair().unwrap();
    for socket in [&parent, &child] {
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
    }
    // A pre-exec hook forces Command's fork path and holds the child before
    // CLOEXEC closes its inherited workspace.lock descriptor. Use only
    // async-signal-safe calls in the child of this multithreaded test process.
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--list")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            let fd = child.as_raw_fd();
            let pid = libc::getpid().to_ne_bytes();
            if libc::write(fd, pid.as_ptr().cast(), pid.len()) != pid.len() as isize {
                return Err(std::io::Error::last_os_error());
            }
            let mut release = 0_u8;
            if libc::read(fd, (&mut release as *mut u8).cast(), 1) != 1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let process = std::thread::spawn(move || command.spawn()?.wait());
    let mut child_pid = [0_u8; std::mem::size_of::<libc::pid_t>()];
    let ready = parent.read_exact(&mut child_pid);
    let child_pid = libc::pid_t::from_ne_bytes(child_pid);
    let blocked_while_owned = Store::open(state.path()).is_err();
    drop(store);
    let reopened = Store::open(state.path());
    let blocked_after_reopen = reopened
        .as_ref()
        .ok()
        .map(|_| Store::open(state.path()).is_err());
    eprintln!(
        "forked child {child_pid}, state {}, live-owner blocked: {blocked_while_owned}, after-drop reopen: {reopened_result}",
        state.path().display(),
        reopened_result = reopened
            .as_ref()
            .map(|_| "opened".to_string())
            .map_err(|e| format!("{e:#}"))
            .unwrap_or_else(|e| e),
    );
    // Always release and reap our child before reporting a failed assertion.
    let released = parent.write_all(&[1]);
    let status = process.join().unwrap();
    ready.unwrap();
    released.unwrap();
    assert!(status.unwrap().success());
    assert!(
        blocked_while_owned,
        "a live Store must retain exclusive ownership"
    );
    let reopened = reopened.expect("a forked child must not extend a dropped Store's lock");
    assert!(blocked_after_reopen.unwrap());
    assert!(
        Store::open(state.path()).is_err(),
        "child exec must not release the reopened Store's new lock"
    );
    drop(reopened);
    Store::open(state.path()).unwrap();
}

#[test]
fn recovery_survives_restart_and_external_changes_without_writing_project() {
    let (_directory, project) = fixture();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(project.root.join("one.rs"), "original").unwrap();
    let mut document = project.open_document("one.rs", false).unwrap();
    document.snapshot("draft 🦀".into());
    let mut state = WorkspaceState::new(&project);
    state.tabs.push(document.path.clone());
    state.active = Some(document.path.clone());
    {
        let mut store = Store::open(state_dir.path()).unwrap();
        store.checkpoint(&state, &[document.clone()]).unwrap();
        assert!(
            Store::open(state_dir.path()).is_err(),
            "concurrent state writer must be refused"
        );
    }
    fs::write(project.root.join("one.rs"), "external").unwrap();
    let store = Store::open(state_dir.path()).unwrap();
    assert_eq!(
        store.last_project().unwrap().as_deref(),
        Some(project.key())
    );
    assert_eq!(
        store
            .workspace(project.key())
            .unwrap()
            .unwrap()
            .active
            .as_deref(),
        Some("one.rs")
    );
    let recovered = store.drafts(project.key()).unwrap().remove(0);
    assert_eq!(recovered.text, "draft 🦀");
    assert!(project.save(&recovered).is_err());
    assert_eq!(
        fs::read_to_string(project.root.join("one.rs")).unwrap(),
        "external"
    );
}

#[test]
fn checkpoint_clears_saved_drafts_but_retains_closed_dirty_drafts() {
    let (_directory, project) = fixture();
    let state_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(state_dir.path()).unwrap();
    let state = WorkspaceState::new(&project);
    let mut document = project.open_document("new.txt", true).unwrap();
    document.snapshot("draft".into());
    store.checkpoint(&state, &[document.clone()]).unwrap();
    store.checkpoint(&state, &[]).unwrap();
    assert_eq!(store.drafts(project.key()).unwrap().len(), 1);
    let saved = project.save(&document).unwrap();
    store.checkpoint(&state, &[saved]).unwrap();
    assert!(store.drafts(project.key()).unwrap().is_empty());
}

#[test]
fn diff_uses_current_disk_instead_of_an_old_baseline() {
    let (_directory, project) = fixture();
    fs::write(project.root.join("file"), "original\n").unwrap();
    let mut document = project.open_document("file", false).unwrap();
    document.snapshot("draft\n".into());
    fs::write(project.root.join("file"), "external\n").unwrap();
    let diff = project.diff(&document).unwrap();
    assert!(diff.contains("-external"));
    assert!(diff.contains("+draft"));
}

#[test]
fn late_background_checkpoint_cannot_replace_a_newer_flush() {
    let (_directory, project) = fixture();
    let state_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(state_dir.path()).unwrap();
    let mut old_state = WorkspaceState::new(&project);
    old_state.sequence = 1;
    let mut old = project.open_document("draft.txt", true).unwrap();
    old.snapshot("old".into());
    let mut newest_state = old_state.clone();
    newest_state.sequence = 2;
    let mut newest = old.clone();
    newest.snapshot("newest".into());
    store.checkpoint(&newest_state, &[newest]).unwrap();
    store.checkpoint(&old_state, &[old]).unwrap();
    assert_eq!(store.drafts(project.key()).unwrap()[0].text, "newest");
    assert_eq!(store.sequence().unwrap(), 2);
}

#[test]
fn search_prefers_unsaved_buffers_and_includes_new_files() {
    let (_directory, project) = fixture();
    fs::write(project.root.join("code.py"), "disk text").unwrap();
    let mut changed = project.open_document("code.py", false).unwrap();
    changed.snapshot("unsaved needle".into());
    let mut new = project.open_document("new.py", true).unwrap();
    new.snapshot("new needle".into());
    let hits = project.search_with_overlays(
        &project.index().files,
        "needle",
        &[changed, new],
        &AtomicBool::new(false),
    );
    assert_eq!(hits.len(), 2);
    assert_eq!(
        fs::read_to_string(project.root.join("code.py")).unwrap(),
        "disk text"
    );
    assert!(!project.root.join("new.py").exists());
}

#[test]
#[ignore = "Subprocess fixture exercised by the forced-termination test"]
fn crash_writer_helper() {
    use std::io::Write;
    let Some(root) = std::env::var_os("TANDEM_CRASH_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let project = Project::open(&root.join("project")).unwrap();
    let mut store = Store::open(&root.join("state")).unwrap();
    let mut document = project.open_document("main.rs", false).unwrap();
    document.snapshot("unsaved after crash 🦀".into());
    let mut state = WorkspaceState::new(&project);
    state.sequence = 1;
    state.tabs.push("main.rs".into());
    state.active = Some("main.rs".into());
    store.checkpoint(&state, &[document]).unwrap();
    println!("CHECKPOINT_READY");
    std::io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}

#[test]
fn committed_draft_survives_forced_process_termination() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("project")).unwrap();
    fs::write(root.path().join("project/main.rs"), "saved").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_writer_helper", "--ignored", "--nocapture"])
        .env("TANDEM_CRASH_FIXTURE", root.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line == "CHECKPOINT_READY" {
                let _ = tx.send(());
                break;
            }
        }
    });
    let ready = rx.recv_timeout(std::time::Duration::from_secs(10));
    child.kill().unwrap();
    child.wait().unwrap();
    ready.unwrap();
    let project = Project::open(&root.path().join("project")).unwrap();
    let store = Store::open(&root.path().join("state")).unwrap();
    assert_eq!(
        store.drafts(project.key()).unwrap()[0].text,
        "unsaved after crash 🦀"
    );
    assert_eq!(
        fs::read_to_string(project.root.join("main.rs")).unwrap(),
        "saved"
    );
}

#[test]
fn current_permissions_override_stale_readonly_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let (_directory, project) = fixture();
    let path = project.root.join("permissions.txt");
    fs::write(&path, "before").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let mut draft = project.open_document("permissions.txt", false).unwrap();
    assert!(draft.read_only);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        project.disk_change(&draft),
        DiskChange::MetadataChanged { read_only: false }
    ));
    draft.snapshot("after".into());
    let saved = project.save(&draft).unwrap();
    assert!(!saved.read_only);
    assert_eq!(fs::read_to_string(path).unwrap(), "after");
}

#[test]
fn newer_storage_schema_is_refused_without_rewriting_its_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE future_data(value TEXT); INSERT INTO future_data VALUES('preserve'); PRAGMA user_version=7;").unwrap();
    }
    let original = fs::read(&path).unwrap();
    assert!(Store::open(directory.path()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    let connection = rusqlite::Connection::open(path).unwrap();
    let value: String = connection
        .query_row("SELECT value FROM future_data", [], |row| row.get(0))
        .unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(value, "preserve");
    assert_eq!(version, 7);
}

#[test]
fn nested_new_draft_creates_directories_only_on_explicit_save_and_never_clobbers() {
    let (_directory, project) = fixture();
    let mut draft = project.open_document("new/sub/file.py", true).unwrap();
    draft.snapshot("print('nested draft')\n".into());
    assert!(!project.root.join("new").exists());
    assert!(matches!(project.disk_change(&draft), DiskChange::Unchanged));
    let saved = project.save(&draft).unwrap();
    assert!(!saved.dirty());
    assert_eq!(
        fs::read_to_string(project.root.join("new/sub/file.py")).unwrap(),
        draft.text
    );
    let mut second = project.open_document("other/sub/file.py", true).unwrap();
    second.snapshot("user draft".into());
    fs::create_dir_all(project.root.join("other/sub")).unwrap();
    fs::write(project.root.join("other/sub/file.py"), "external content").unwrap();
    assert!(project.save(&second).is_err());
    assert_eq!(
        fs::read_to_string(project.root.join("other/sub/file.py")).unwrap(),
        "external content"
    );
}

#[test]
fn nested_new_paths_refuse_escaping_links_and_non_directory_parents() {
    let (_directory, project) = fixture();
    let outside = tempfile::tempdir().unwrap();
    assert!(project.open_document("../outside/file.py", true).is_err());
    fs::write(project.root.join("file-parent"), "preserve").unwrap();
    assert!(project.open_document("file-parent/child.py", true).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), project.root.join("link")).unwrap();
        assert!(project.open_document("link/nested/file.py", true).is_err());
        let mut draft = project.open_document("later/nested/file.py", true).unwrap();
        draft.snapshot("draft".into());
        std::os::unix::fs::symlink(outside.path(), project.root.join("later")).unwrap();
        assert!(matches!(
            project.disk_change(&draft),
            DiskChange::Unavailable(_)
        ));
        assert!(project.save(&draft).is_err());
        assert!(!outside.path().join("nested").exists());
    }
}

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
fn captured_working_version_keeps_closed_drafts_and_prefers_live_buffers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let project = Project::open(&root).unwrap();
    let mut closed = project.open_document("closed.rs", true).unwrap();
    closed.snapshot("retained draft".into());
    let mut live = project.open_document("live.rs", true).unwrap();
    live.snapshot("checkpoint".into());
    let mut store = Store::open(&temp.path().join("state")).unwrap();
    store
        .checkpoint(&WorkspaceState::new(&project), &[closed, live.clone()])
        .unwrap();
    live.snapshot("latest live version".into());
    let docs = store.working_documents(project.key(), &[live]).unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(
        docs.iter().find(|d| d.path == "closed.rs").unwrap().text,
        "retained draft"
    );
    assert_eq!(
        docs.iter().find(|d| d.path == "live.rs").unwrap().text,
        "latest live version"
    );
}
