//! The tests play Claude by changing files with `std::fs`, which is exactly
//! what a Bash command run during a takeover would do.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tandem_core::chat::ChangeKind;
use tandem_core::takeover::{
    SNAPSHOT_MAX_FILE_BYTES, SNAPSHOT_MAX_TRACKED_BYTES, Snapshot, SnapshotChange,
};
use tandem_core::{Project, content_hash};

struct Fixture {
    _project_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
    project: Project,
    snapshot_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let project_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let project = Project::open(project_dir.path()).unwrap();
        let snapshot_dir = state_dir.path().join("takeover").join("set-1");
        Self {
            _project_dir: project_dir,
            _state_dir: state_dir,
            project,
            snapshot_dir,
        }
    }

    /// The project from the plan: two sources, a README and an ignored build folder.
    fn standard() -> Self {
        let fixture = Self::new();
        fixture.write("src/a.rs", "fn a() {}\n");
        fixture.write("src/b.rs", "fn b() {}\n");
        fixture.write("README.md", "# Demo\n");
        fixture.write(".gitignore", "build/\n");
        fixture.write("build/out.txt", "built\n");
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.project.root.join(relative)
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.path(relative)).unwrap()
    }

    fn mode(&self, relative: &str) -> u32 {
        fs::metadata(self.path(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    fn set_mode(&self, relative: &str, mode: u32) {
        fs::set_permissions(self.path(relative), fs::Permissions::from_mode(mode)).unwrap();
    }

    fn capture(&self) -> Snapshot {
        Snapshot::capture(&self.project, &self.snapshot_dir).unwrap()
    }
}

fn change(path: &str, kind: ChangeKind, after: Option<&[u8]>, undoable: bool) -> SnapshotChange {
    SnapshotChange {
        path: path.into(),
        kind,
        after_hash: after.map(content_hash),
        undoable,
    }
}

fn find<'a>(changes: &'a [SnapshotChange], path: &str) -> &'a SnapshotChange {
    changes
        .iter()
        .find(|change| change.path == path)
        .unwrap_or_else(|| panic!("{path} is not in {changes:?}"))
}

fn undo(fixture: &Fixture, snapshot: &Snapshot, change: &SnapshotChange) -> anyhow::Result<()> {
    snapshot.undo(&fixture.project, &change.path, change.after_hash.as_deref())
}

/// What a takeover typically does: edit, delete, create in a new folder and
/// at the root, and write build output that the project ignores.
fn play_claude(fixture: &Fixture) {
    fixture.write("src/a.rs", "fn a() { println!(\"json\"); }\n");
    fs::remove_file(fixture.path("src/b.rs")).unwrap();
    fixture.write("src/new/c.rs", "fn c() {}\n");
    fixture.write("notes.txt", "notes\n");
    fixture.write("build/more.txt", "more\n");
    fixture.write("target/x.o", [0u8, 1, 2, 3]);
}

#[test]
fn diff_reports_created_modified_and_deleted_files_but_nothing_ignored() {
    let fixture = Fixture::standard();
    let snapshot = fixture.capture();
    assert!(snapshot.diff(&fixture.project).unwrap().is_empty());

    play_claude(&fixture);

    assert_eq!(
        snapshot.diff(&fixture.project).unwrap(),
        vec![
            change(
                "notes.txt",
                ChangeKind::Created,
                Some(b"notes\n".as_slice()),
                true
            ),
            change(
                "src/a.rs",
                ChangeKind::Modified,
                Some(b"fn a() { println!(\"json\"); }\n".as_slice()),
                true
            ),
            change("src/b.rs", ChangeKind::Deleted, None, true),
            change(
                "src/new/c.rs",
                ChangeKind::Created,
                Some(b"fn c() {}\n".as_slice()),
                true
            ),
        ]
    );
}

#[test]
fn capture_writes_a_private_manifest_and_content_addressed_blobs() {
    let fixture = Fixture::standard();
    fixture.set_mode("src/a.rs", 0o755);
    fixture.write("src/copy.rs", "fn a() {}\n");
    fixture.capture();

    let dir_mode = fs::metadata(&fixture.snapshot_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.snapshot_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["root"], fixture.project.key());
    let entries = manifest["entries"].as_object().unwrap();
    let paths: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            ".gitignore",
            "README.md",
            "src/a.rs",
            "src/b.rs",
            "src/copy.rs"
        ]
    );
    let hash = content_hash(b"fn a() {}\n");
    assert_eq!(
        entries["src/a.rs"],
        serde_json::json!({ "hash": hash, "stored": true, "mode": 0o755 })
    );
    assert_eq!(entries["src/copy.rs"]["hash"], hash);
    assert_eq!(
        fs::read(fixture.snapshot_dir.join("blobs").join(&hash)).unwrap(),
        b"fn a() {}\n"
    );
    let blobs = fs::read_dir(fixture.snapshot_dir.join("blobs"))
        .unwrap()
        .count();
    assert_eq!(blobs, 4, "identical contents share one blob");
}

#[test]
fn undo_restores_bytes_and_mode_and_removes_created_files() {
    let fixture = Fixture::standard();
    fixture.set_mode("src/a.rs", 0o755);
    let snapshot = fixture.capture();
    play_claude(&fixture);
    fixture.set_mode("src/a.rs", 0o644);
    let changes = snapshot.diff(&fixture.project).unwrap();

    undo(&fixture, &snapshot, find(&changes, "src/a.rs")).unwrap();
    assert_eq!(fixture.read("src/a.rs"), b"fn a() {}\n");
    assert_eq!(fixture.mode("src/a.rs"), 0o755);

    undo(&fixture, &snapshot, find(&changes, "src/b.rs")).unwrap();
    assert_eq!(fixture.read("src/b.rs"), b"fn b() {}\n");

    undo(&fixture, &snapshot, find(&changes, "src/new/c.rs")).unwrap();
    undo(&fixture, &snapshot, find(&changes, "notes.txt")).unwrap();
    assert!(!fixture.path("src/new/c.rs").exists());
    assert!(!fixture.path("notes.txt").exists());

    assert!(snapshot.diff(&fixture.project).unwrap().is_empty());
    assert!(
        fixture.path("build/more.txt").exists(),
        "ignored files are left alone"
    );
}

#[test]
fn undo_recreates_missing_parent_folders_of_a_deleted_file() {
    let fixture = Fixture::standard();
    fixture.write("src/deep/nested/d.rs", "fn d() {}\n");
    let snapshot = fixture.capture();
    fs::remove_dir_all(fixture.path("src/deep")).unwrap();
    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        changes,
        vec![change(
            "src/deep/nested/d.rs",
            ChangeKind::Deleted,
            None,
            true
        )]
    );

    undo(&fixture, &snapshot, &changes[0]).unwrap();
    assert_eq!(fixture.read("src/deep/nested/d.rs"), b"fn d() {}\n");
    assert!(snapshot.diff(&fixture.project).unwrap().is_empty());
}

#[test]
fn keep_moves_the_baseline_to_the_current_content() {
    let fixture = Fixture::standard();
    let mut snapshot = fixture.capture();
    play_claude(&fixture);

    snapshot.keep(&fixture.project, "src/a.rs").unwrap();
    let changes = snapshot.diff(&fixture.project).unwrap();
    assert!(changes.iter().all(|change| change.path != "src/a.rs"));
    assert_eq!(changes.len(), 3);

    fixture.write("src/a.rs", "fn a() { /* second edit */ }\n");
    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        find(&changes, "src/a.rs"),
        &change(
            "src/a.rs",
            ChangeKind::Modified,
            Some(b"fn a() { /* second edit */ }\n".as_slice()),
            true
        )
    );

    undo(&fixture, &snapshot, find(&changes, "src/a.rs")).unwrap();
    assert_eq!(
        fixture.read("src/a.rs"),
        b"fn a() { println!(\"json\"); }\n"
    );
}

#[test]
fn keep_of_a_deleted_or_created_file_accepts_its_current_state() {
    let fixture = Fixture::standard();
    let mut snapshot = fixture.capture();
    play_claude(&fixture);

    snapshot.keep(&fixture.project, "src/b.rs").unwrap();
    snapshot.keep(&fixture.project, "notes.txt").unwrap();
    let paths: Vec<String> = snapshot
        .diff(&fixture.project)
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(paths, ["src/a.rs", "src/new/c.rs"]);

    // The kept state survives a restart because keep rewrites the manifest.
    let reopened = Snapshot::open(&fixture.snapshot_dir).unwrap();
    assert_eq!(
        reopened.diff(&fixture.project).unwrap(),
        snapshot.diff(&fixture.project).unwrap()
    );

    // Undoing a kept creation now restores the kept bytes.
    fixture.write("notes.txt", "rewritten\n");
    let changes = reopened.diff(&fixture.project).unwrap();
    undo(&fixture, &reopened, find(&changes, "notes.txt")).unwrap();
    assert_eq!(fixture.read("notes.txt"), b"notes\n");
}

#[test]
fn undo_refuses_when_the_file_changed_after_the_diff() {
    let fixture = Fixture::standard();
    let snapshot = fixture.capture();
    play_claude(&fixture);
    let changes = snapshot.diff(&fixture.project).unwrap();

    fixture.write("src/a.rs", "fn a() { /* the user edited it */ }\n");
    let error = undo(&fixture, &snapshot, find(&changes, "src/a.rs")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "src/a.rs changed after Claude's edit, so it was not restored."
    );
    assert_eq!(
        fixture.read("src/a.rs"),
        b"fn a() { /* the user edited it */ }\n"
    );

    let error = snapshot
        .undo(&fixture.project, "notes.txt", Some(&content_hash(b"other")))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "notes.txt changed after Claude's edit, so it was not restored."
    );
    assert_eq!(fixture.read("notes.txt"), b"notes\n");

    // A deleted file that has come back is not overwritten either.
    fixture.write("src/b.rs", "fn b2() {}\n");
    let error = undo(&fixture, &snapshot, find(&changes, "src/b.rs")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "src/b.rs changed after Claude's edit, so it was not restored."
    );
    assert_eq!(fixture.read("src/b.rs"), b"fn b2() {}\n");
}

#[test]
fn large_files_are_tracked_without_undo_and_huge_files_are_ignored() {
    let fixture = Fixture::standard();
    let three_mib = 3 * 1024 * 1024;
    assert!(three_mib as u64 > SNAPSHOT_MAX_FILE_BYTES);
    fixture.write("assets/big.bin", vec![b'a'; three_mib]);
    let huge = fs::File::create(fixture.path("assets/huge.bin")).unwrap();
    huge.set_len(SNAPSHOT_MAX_TRACKED_BYTES + 1).unwrap();
    let snapshot = fixture.capture();
    assert!(
        !fixture
            .snapshot_dir
            .join("blobs")
            .join(content_hash(&vec![b'a'; three_mib]))
            .exists()
    );

    let changed = vec![b'b'; three_mib];
    fixture.write("assets/big.bin", &changed);
    huge.set_len(SNAPSHOT_MAX_TRACKED_BYTES + 2).unwrap();
    let created = fs::File::create(fixture.path("assets/huge2.bin")).unwrap();
    created.set_len(SNAPSHOT_MAX_TRACKED_BYTES + 1).unwrap();

    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        changes,
        vec![change(
            "assets/big.bin",
            ChangeKind::Modified,
            Some(changed.as_slice()),
            false
        )]
    );
    let error = undo(&fixture, &snapshot, &changes[0]).unwrap_err();
    assert_eq!(error.to_string(), "assets/big.bin is too large to restore.");
    assert_eq!(fixture.read("assets/big.bin"), changed);

    // Never tracked, so its deletion is not reported either.
    fs::remove_file(fixture.path("assets/huge.bin")).unwrap();
    assert_eq!(snapshot.diff(&fixture.project).unwrap(), changes);
}

#[test]
fn open_on_the_same_folder_yields_the_same_diff_and_discard_removes_it() {
    let fixture = Fixture::standard();
    let snapshot = fixture.capture();
    play_claude(&fixture);
    let before_restart = snapshot.diff(&fixture.project).unwrap();
    drop(snapshot);

    let reopened = Snapshot::open(&fixture.snapshot_dir).unwrap();
    assert_eq!(reopened.diff(&fixture.project).unwrap(), before_restart);
    undo(&fixture, &reopened, find(&before_restart, "src/b.rs")).unwrap();
    assert_eq!(fixture.read("src/b.rs"), b"fn b() {}\n");

    reopened.discard().unwrap();
    assert!(!fixture.snapshot_dir.exists());
    assert!(Snapshot::open(&fixture.snapshot_dir).is_err());
}

#[test]
fn undo_refuses_symlink_components_and_paths_outside_the_project() {
    let fixture = Fixture::standard();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "outside\n").unwrap();
    let snapshot = fixture.capture();

    std::os::unix::fs::symlink(outside.path(), fixture.path("linked")).unwrap();
    assert!(snapshot.diff(&fixture.project).unwrap().is_empty());
    assert!(
        snapshot
            .undo(
                &fixture.project,
                "linked/secret.txt",
                Some(&content_hash(b"outside\n"))
            )
            .is_err()
    );
    assert_eq!(
        fs::read(outside.path().join("secret.txt")).unwrap(),
        b"outside\n"
    );

    let escape = format!(
        "../{}/secret.txt",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    assert!(
        snapshot
            .undo(&fixture.project, &escape, Some(&content_hash(b"outside\n")))
            .is_err()
    );
    assert!(snapshot.undo(&fixture.project, "../outside", None).is_err());
    assert_eq!(
        fs::read(outside.path().join("secret.txt")).unwrap(),
        b"outside\n"
    );
}

#[test]
fn a_folder_replaced_by_a_symlink_does_not_count_as_its_files() {
    let fixture = Fixture::standard();
    let snapshot = fixture.capture();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(elsewhere.path().join("a.rs"), "fn a() {}\n").unwrap();
    fs::write(elsewhere.path().join("b.rs"), "fn b() {}\n").unwrap();
    fs::remove_dir_all(fixture.path("src")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), fixture.path("src")).unwrap();

    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        changes,
        vec![
            change("src/a.rs", ChangeKind::Deleted, None, true),
            change("src/b.rs", ChangeKind::Deleted, None, true),
        ]
    );
    assert!(undo(&fixture, &snapshot, &changes[0]).is_err());
    assert_eq!(
        fs::read(elsewhere.path().join("a.rs")).unwrap(),
        b"fn a() {}\n"
    );
}

#[test]
fn a_file_that_becomes_ignored_is_not_reported_as_deleted() {
    let fixture = Fixture::standard();
    let snapshot = fixture.capture();
    fixture.write(".gitignore", "build/\nREADME.md\n");

    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        changes,
        vec![change(
            ".gitignore",
            ChangeKind::Modified,
            Some(b"build/\nREADME.md\n".as_slice()),
            true
        )]
    );

    // It is still compared on disk, so a change to it is reported.
    fixture.write("README.md", "# Changed\n");
    let changes = snapshot.diff(&fixture.project).unwrap();
    assert_eq!(
        find(&changes, "README.md"),
        &change(
            "README.md",
            ChangeKind::Modified,
            Some(b"# Changed\n".as_slice()),
            true
        )
    );
}

#[test]
fn a_snapshot_only_applies_to_the_project_it_was_taken_of() {
    let fixture = Fixture::standard();
    let mut snapshot = fixture.capture();
    let other = Fixture::standard();

    assert!(snapshot.diff(&other.project).is_err());
    assert!(snapshot.keep(&other.project, "src/a.rs").is_err());
    assert!(
        snapshot
            .undo(
                &other.project,
                "src/a.rs",
                Some(&content_hash(b"fn a() {}\n"))
            )
            .is_err()
    );
}

#[test]
fn capture_refuses_a_project_whose_index_is_truncated() {
    let fixture = Fixture::new();
    let many = fixture.path("many");
    fs::create_dir(&many).unwrap();
    for number in 0..=tandem_core::MAX_INDEX_FILES {
        fs::write(many.join(format!("{number}.txt")), "").unwrap();
    }

    let error = Snapshot::capture(&fixture.project, &fixture.snapshot_dir).unwrap_err();
    assert_eq!(
        error.to_string(),
        "This project has more than 20,000 files that are not ignored; a takeover cannot be undone safely here."
    );
    assert!(!fixture.snapshot_dir.exists());
}
