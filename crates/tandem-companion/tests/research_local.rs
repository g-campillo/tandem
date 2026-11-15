use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use tandem_checks::{InspectionExecutor, InspectionOutput, PreparedInspection};
use tandem_companion::*;
use tandem_core::*;

#[derive(Default)]
struct FakeExecutor {
    commands: Vec<Vec<String>>,
    output: InspectionOutput,
}
impl InspectionExecutor for FakeExecutor {
    fn execute(
        &mut self,
        command: &PreparedInspection,
        cancel: &AtomicBool,
        started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
    ) -> Result<InspectionOutput> {
        started(&command.provenance())?;
        self.commands.push(command.argv().to_vec());
        let mut output = self.output.clone();
        output.cancelled |= cancel.load(Ordering::Relaxed);
        Ok(output)
    }
}
fn request(kind: ResearchRequestKind) -> ResearchRequest {
    ResearchRequest::new(kind).unwrap()
}

#[test]
fn research_expands_an_immutable_unsaved_capture_and_keeps_exclusions() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    for (path, text) in [
        ("code.rs", "disk\n"),
        (".gitignore", "ignored*\n"),
        ("ignored.rs", "needle ignored"),
        ("secret.rs", "password = 'private-fixture-value'"),
        ("binary", "needle\0binary"),
        (".env", "needle private"),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    let outside = fixture.path().join("outside");
    std::fs::write(&outside, "needle private").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    std::fs::hard_link(&outside, root.join("hard")).unwrap();
    let project = Project::open(&root).unwrap();
    let mut code = project.open_document("code.rs", false).unwrap();
    code.snapshot("first\nneedle unsaved\nlast\n".into());
    let mut new = project.open_document("new.rs", true).unwrap();
    new.snapshot("needle new".into());
    let mut ignored = project.open_document("ignored-new.rs", true).unwrap();
    ignored.snapshot("needle ignored new".into());
    let cancel = AtomicBool::new(false);
    let capture = ResearchSnapshot::capture(
        &project,
        &[code, new, ignored],
        &fixture.path().join("cache"),
        &cancel,
    )
    .unwrap();
    let mut context = ResearchContext::new(&capture, "session", "operation", None, &[]).unwrap();
    let mut broker = ResearchBroker::with_executor(FakeExecutor::default());
    context.begin_round(4).unwrap();
    let list = broker
        .execute(
            request(ResearchRequestKind::ListFiles {
                prefix: None,
                limit: 200,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(list.content.contains("new.rs"));
    for excluded in ["ignored", "secret.rs", "binary", ".env", "link", "hard"] {
        assert!(!list.content.contains(excluded), "{excluded}");
    }
    let found = broker
        .execute(
            request(ResearchRequestKind::SearchText {
                query: "needle".into(),
                paths: vec![],
                limit: 200,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(found.content.contains("needle unsaved"));
    assert!(found.sources.iter().any(|s| matches!(&s.locator, SourceLocator::FileRange {path, start_line: 2, end_line: 2} if path == "code.rs")));
    std::fs::write(root.join("code.rs"), "changed source").unwrap();
    let read = broker
        .execute(
            request(ResearchRequestKind::ReadFileRange {
                path: "code.rs".into(),
                start_line: 2,
                end_line: 2,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(read.content, "needle unsaved\n");
    assert_eq!(read.snapshot_id.as_deref(), Some(capture.id()));
    let denied = broker
        .execute(
            request(ResearchRequestKind::ReadFileRange {
                path: "ignored.rs".into(),
                start_line: 1,
                end_line: 1,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(denied.status, ResearchReceiptStatus::Unavailable);
    assert!(request_path("../outside").is_err());
    let changed =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    assert_ne!(capture.id(), changed.id());
}
fn request_path(path: &str) -> Result<ResearchRequest> {
    ResearchRequest::new(ResearchRequestKind::ReadFileRange {
        path: path.into(),
        start_line: 1,
        end_line: 1,
    })
}

#[test]
fn transcript_research_is_current_session_only_and_budgeted() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let project = Project::open(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let capture =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    let events = [
        event("session", 1, "current"),
        event("other", 1, "private other"),
        event("session", 2, "password = 'private-fixture-value'"),
    ];
    let mut context =
        ResearchContext::new(&capture, "session", "operation", None, &events).unwrap();
    let mut broker = ResearchBroker::with_executor(FakeExecutor::default());
    context.begin_round(2).unwrap();
    let receipt = broker
        .execute(
            request(ResearchRequestKind::ConversationEvents {
                before_sequence: Some(3),
                limit: 10,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(receipt.content.contains("current"));
    assert!(!receipt.content.contains("private"));
    assert_eq!(receipt.status, ResearchReceiptStatus::Partial);
    let git = broker
        .execute(
            request(ResearchRequestKind::GitHistory {
                path: None,
                limit: 50,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(git.status, ResearchReceiptStatus::Unavailable);
    assert!(
        broker
            .execute(
                request(ResearchRequestKind::ListFiles {
                    prefix: None,
                    limit: 1
                }),
                &mut context,
                &cancel,
                |_| Ok(())
            )
            .is_err()
    );
    for _ in 1..8 {
        context.begin_round(1).unwrap();
    }
    assert!(
        context
            .begin_round(1)
            .unwrap_err()
            .to_string()
            .contains("Continue")
    );
    assert!(serde_json::from_value::<ResearchRequest>(serde_json::json!({"id":"r","kind":{"type":"Inspect","recipe":"Shell","argv":["touch","sentinel"]}})).is_err());
}
fn event(session: &str, sequence: u64, text: &str) -> ConversationEvent {
    ConversationEvent {
        id: new_id(),
        session_id: session.into(),
        sequence,
        created_at_ms: 0,
        run_id: None,
        payload: ConversationPayload::HumanMessage { text: text.into() },
    }
}

#[test]
fn dependency_metadata_is_offline_data_and_retains_declarations_and_sources() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    for (path, text) in [
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\n[dependencies]\nalias={package='real-package',version='1.2',git='https://example.invalid/repo'}\n",
        ),
        (
            "Cargo.lock",
            "version=4\n[[package]]\nname='real-package'\nversion='1.2.3'\nsource='registry+https://example.invalid/index'\n",
        ),
        (
            "pyproject.toml",
            "[project]\nname='fixture'\ndependencies=['httpx==0.28.0']\n[project.optional-dependencies]\ntest=['pytest>=8']\n",
        ),
        (
            "uv.lock",
            "version=1\n[[package]]\nname='httpx'\nversion='0.28.0'\nsource={registry='https://example.invalid/simple'}\n",
        ),
        ("requirements.txt", "requests==2.32.0\n-r private.txt\n"),
        (
            "setup.py",
            "raise RuntimeError('project must never be imported')\n",
        ),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    let project = Project::open(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let capture =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    let mut context = ResearchContext::new(&capture, "session", "op", None, &[]).unwrap();
    context.begin_round(2).unwrap();
    let mut broker = ResearchBroker::with_executor(FakeExecutor::default());
    let rust = broker
        .execute(
            request(ResearchRequestKind::DependencyMetadata {
                ecosystem: DependencyEcosystem::Rust,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(
        rust.content.contains("real-package")
            && rust.content.contains("1.2.3")
            && rust
                .content
                .contains("registry+https://example.invalid/index")
    );
    let python = broker
        .execute(
            request(ResearchRequestKind::DependencyMetadata {
                ecosystem: DependencyEcosystem::Python,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(
        python.content.contains("httpx")
            && python.content.contains("0.28.0")
            && python.content.contains("requests==2.32.0")
            && python.content.contains("pytest>=8")
    );
    assert!(!python.content.contains("project must never"));
    assert_eq!(python.status, ResearchReceiptStatus::Partial);
    assert!(broker.executor().commands.is_empty());
    assert!(capture.unchanged());
}

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("/usr/bin/git")
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn git_environment(fixture: &std::path::Path) -> tandem_checks::InspectionEnvironment {
    tandem_checks::InspectionEnvironment::configured(
        tandem_checks::CheckEnvironment {
            runtime: fixture.join("unused-runtime"),
            tools: None,
            python: None,
            runs: fixture.join("runs"),
            cargo_cache: fixture.join("cargo"),
            clang: fixture.join("clang"),
            sdk: None,
            denied_roots: vec![],
        },
        &ProjectTools::default(),
        Some(tandem_checks::InspectionTool {
            executable: "/usr/bin/git".into(),
            fingerprint: file_hash(std::path::Path::new("/usr/bin/git")).unwrap(),
            read_roots: vec![],
        }),
    )
    .unwrap()
}
/// Runs only the prepared fixed Git commands in disposable fixtures. This tests
/// real Git semantics/config defenses, not the production OS sandbox (Task 10).
struct FixtureGitExecutor;
impl InspectionExecutor for FixtureGitExecutor {
    fn execute(
        &mut self,
        command: &PreparedInspection,
        cancel: &AtomicBool,
        started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
    ) -> Result<InspectionOutput> {
        assert!(!cancel.load(Ordering::Relaxed));
        started(&command.provenance())?;
        let output = std::process::Command::new(&command.argv()[0])
            .args(&command.argv()[1..])
            .current_dir(command.cwd())
            .env_clear()
            .envs(command.environment())
            .output()?;
        let mut bytes = output.stdout;
        bytes.extend(output.stderr);
        Ok(InspectionOutput {
            output: bytes,
            exit_code: output.status.code().map(i64::from),
            ..Default::default()
        })
    }
}

fn historical_fixture() -> (tempfile::TempDir, Project, String, String) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir_all(root.join("src/space dir")).unwrap();
    git(&root, &["init", "-q"]);
    for (path, text) in [
        (".gitignore", "src/ignored.txt\n"),
        ("src/code.rs", "first symbol\n"),
        ("src/deleted.rs", "deleted symbol\n"),
        ("src/[code].rs", "first literal\n"),
        ("src/.env", "ordinary excluded environment record\n"),
        ("src/ignored.txt", "ordinary ignored record\n"),
        ("src/space dir/.env", "ordinary excluded nested record\n"),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    std::os::unix::fs::symlink(".env", root.join("src/link")).unwrap();
    git(&root, &["add", "-f", "."]);
    git(&root, &["commit", "-qm", "first src/.env src/ignored.txt"]);
    let first = git(&root, &["rev-parse", "HEAD"]);
    std::fs::remove_file(root.join("src/deleted.rs")).unwrap();
    for (path, text) in [
        ("src/code.rs", "second symbol\n"),
        ("src/[code].rs", "second literal\n"),
        ("src/.env", "changed excluded environment record\n"),
        ("src/ignored.txt", "changed ignored record\n"),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    git(&root, &["add", "-f", "."]);
    git(&root, &["commit", "-qm", "second src/space dir/.env"]);
    let second = git(&root, &["rev-parse", "HEAD"]);
    let project = Project::open(&root).unwrap();
    (fixture, project, first, second)
}

fn inspect_history(
    capture: &ResearchSnapshot,
    environment: &tandem_checks::InspectionEnvironment,
    kind: ResearchRequestKind,
) -> ResearchReceipt {
    let mut context =
        ResearchContext::new(capture, "session", "operation", Some(environment), &[]).unwrap();
    context.begin_round(1).unwrap();
    ResearchBroker::with_executor(FixtureGitExecutor)
        .execute(request(kind), &mut context, &AtomicBool::new(false), |_| {
            Ok(())
        })
        .unwrap()
}

#[test]
fn historical_object_ids_must_resolve_to_commits_before_returning_output() {
    let (fixture, project, first, second) = historical_fixture();
    let blob = git(&project.root, &["rev-parse", "HEAD:src/.env"]);
    let tree = git(&project.root, &["rev-parse", "HEAD:src"]);
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    for object in [blob, tree] {
        for kind in [
            ResearchRequestKind::GitShow {
                commit: object.clone(),
                path: None,
            },
            ResearchRequestKind::GitShow {
                commit: object.clone(),
                path: Some("src/code.rs".into()),
            },
            ResearchRequestKind::GitDiff {
                base: object.clone(),
                head: second.clone(),
                paths: vec!["src/code.rs".into()],
            },
            ResearchRequestKind::GitDiff {
                base: first.clone(),
                head: object.clone(),
                paths: vec!["src/code.rs".into()],
            },
        ] {
            let receipt = inspect_history(&capture, &environment, kind);
            assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
            assert!(receipt.content.is_empty(), "{}", receipt.content);
            assert!(receipt.sources.is_empty());
        }
    }
    assert!(capture.unchanged());
}

#[test]
fn historical_directories_cannot_expand_into_excluded_descendants_or_tree_listings() {
    let (fixture, project, first, second) = historical_fixture();
    // Current eligibility cannot authorize a symlink at a historical revision.
    std::fs::remove_file(project.root.join("src/link")).unwrap();
    std::fs::write(project.root.join("src/link"), "current regular file\n").unwrap();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    for path in ["src", "src/space dir", "src/link"] {
        for kind in [
            ResearchRequestKind::GitShow {
                commit: first.clone(),
                path: Some(path.into()),
            },
            ResearchRequestKind::GitDiff {
                base: first.clone(),
                head: second.clone(),
                paths: vec![path.into()],
            },
        ] {
            let receipt = inspect_history(&capture, &environment, kind);
            assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable, "{path}");
            assert!(receipt.content.is_empty(), "{}", receipt.content);
            assert!(receipt.sources.is_empty());
        }
    }
    assert!(capture.unchanged());
}

#[test]
fn historical_literal_leaves_and_deleted_files_remain_available_from_the_capture() {
    let (fixture, project, first, second) = historical_fixture();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    // Later live changes must not supply either revision or path-validation data.
    git(
        &project.root,
        &["commit", "--allow-empty", "-qm", "live only"],
    );
    std::fs::write(project.root.join(".gitignore"), "src/\n").unwrap();
    for (path, expected) in [
        ("src/[code].rs", "first literal\n"),
        ("src/deleted.rs", "deleted symbol\n"),
    ] {
        let show = inspect_history(
            &capture,
            &environment,
            ResearchRequestKind::GitShow {
                commit: first.clone(),
                path: Some(path.into()),
            },
        );
        assert_eq!(show.status, ResearchReceiptStatus::Complete);
        assert_eq!(show.content, expected);
        let diff = inspect_history(
            &capture,
            &environment,
            ResearchRequestKind::GitDiff {
                base: first.clone(),
                head: second.clone(),
                paths: vec![path.into()],
            },
        );
        assert_eq!(diff.status, ResearchReceiptStatus::Complete);
        assert!(diff.content.contains(expected.trim_end()));
        assert!(!diff.content.contains("excluded"));
    }
    for path in ["src/*", ":(glob)src/**", "src/[ci]*.rs"] {
        let receipt = inspect_history(
            &capture,
            &environment,
            ResearchRequestKind::GitDiff {
                base: first.clone(),
                head: second.clone(),
                paths: vec![path.into()],
            },
        );
        assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable, "{path}");
        assert!(receipt.content.is_empty());
    }
    assert!(capture.unchanged());
}

#[test]
fn historical_metadata_and_status_withhold_excluded_leaf_names() {
    let (fixture, project, first, _) = historical_fixture();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    for kind in [
        ResearchRequestKind::GitHistory {
            path: None,
            limit: 50,
        },
        ResearchRequestKind::GitHistory {
            path: Some("src".into()),
            limit: 50,
        },
        ResearchRequestKind::GitShow {
            commit: first.clone(),
            path: None,
        },
        ResearchRequestKind::Inspect {
            recipe: InspectionRecipe::GitStatus,
        },
    ] {
        let receipt = inspect_history(&capture, &environment, kind);
        assert!(matches!(
            receipt.status,
            ResearchReceiptStatus::Complete | ResearchReceiptStatus::Partial
        ));
        for excluded in [".env", "ignored.txt", "src/link"] {
            assert!(!receipt.content.contains(excluded), "{}", receipt.content);
        }
    }
    assert!(capture.unchanged());
}

#[test]
fn historical_directory_diffs_withhold_ordinary_excluded_values() {
    let (fixture, project, first, second) = historical_fixture();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    let receipt = inspect_history(
        &capture,
        &environment,
        ResearchRequestKind::GitDiff {
            base: first,
            head: second,
            paths: vec!["src".into()],
        },
    );
    assert!(receipt.content.is_empty(), "{}", receipt.content);
    assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
}

#[test]
fn historical_status_withholds_quoted_excluded_paths() {
    let (fixture, project, _, _) = historical_fixture();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let receipt = inspect_history(
        &capture,
        &git_environment(fixture.path()),
        ResearchRequestKind::Inspect {
            recipe: InspectionRecipe::GitStatus,
        },
    );
    assert!(!receipt.content.contains(".env"), "{}", receipt.content);
}

#[test]
fn historical_expansion_preserves_excluded_ancestors_and_deleted_ignore_policy() {
    let (fixture, project, first, second) = historical_fixture();
    let root = &project.root;
    std::fs::create_dir(root.join("src/was-directory")).unwrap();
    std::fs::write(
        root.join("src/was-directory/file.rs"),
        "historical linked descendant\n",
    )
    .unwrap();
    git(root, &["add", "src/was-directory"]);
    git(root, &["commit", "-qm", "directory before link"]);
    let directory_commit = git(root, &["rev-parse", "HEAD"]);
    std::fs::remove_file(root.join("src/was-directory/file.rs")).unwrap();
    std::fs::remove_dir(root.join("src/was-directory")).unwrap();
    std::os::unix::fs::symlink(fixture.path(), root.join("src/was-directory")).unwrap();
    std::fs::remove_file(root.join("src/ignored.txt")).unwrap();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    for kind in [
        ResearchRequestKind::GitShow {
            commit: directory_commit,
            path: Some("src/was-directory/file.rs".into()),
        },
        ResearchRequestKind::GitShow {
            commit: first.clone(),
            path: Some("src/ignored.txt".into()),
        },
        ResearchRequestKind::GitDiff {
            base: first,
            head: second,
            paths: vec!["src/ignored.txt".into()],
        },
    ] {
        let receipt = inspect_history(&capture, &environment, kind);
        assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
        assert!(receipt.content.is_empty());
        assert!(receipt.command.is_none());
    }
}

#[test]
fn historical_leaf_validation_must_finish_before_any_content_command() {
    let (fixture, project, first, second) = historical_fixture();
    let capture = ResearchSnapshot::capture(
        &project,
        &[],
        &fixture.path().join("cache"),
        &AtomicBool::new(false),
    )
    .unwrap();
    let environment = git_environment(fixture.path());
    for output in [
        InspectionOutput {
            output: b"unvalidated diagnostic src/.env".to_vec(),
            exit_code: Some(1),
            ..Default::default()
        },
        InspectionOutput {
            output: b"unvalidated truncated record".to_vec(),
            exit_code: Some(0),
            truncated: true,
            ..Default::default()
        },
        InspectionOutput {
            exit_code: Some(137),
            timed_out: true,
            ..Default::default()
        },
        InspectionOutput {
            exit_code: Some(137),
            cancelled: true,
            ..Default::default()
        },
        InspectionOutput {
            output: format!("100644 blob {}\tsrc/code.rs", "a".repeat(40)).into_bytes(),
            exit_code: Some(0),
            ..Default::default()
        },
    ] {
        let stopped = output.cancelled;
        let mut context =
            ResearchContext::new(&capture, "session", "operation", Some(&environment), &[])
                .unwrap();
        context.begin_round(1).unwrap();
        let mut broker = ResearchBroker::with_executor(FakeExecutor {
            output,
            ..Default::default()
        });
        let receipt = broker
            .execute(
                request(ResearchRequestKind::GitDiff {
                    base: first.clone(),
                    head: second.clone(),
                    paths: vec!["src/code.rs".into()],
                }),
                &mut context,
                &AtomicBool::new(false),
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            receipt.status,
            if stopped {
                ResearchReceiptStatus::Cancelled
            } else {
                ResearchReceiptStatus::Unavailable
            }
        );
        assert!(receipt.content.is_empty());
        assert!(receipt.sources.is_empty());
        assert!(receipt.command.is_some());
        assert!(
            broker
                .executor()
                .commands
                .iter()
                .all(|argv| argv.iter().any(|arg| arg == "ls-tree"))
        );
        assert!(capture.unchanged());
    }
}

#[test]
fn historical_expansion_cannot_lose_a_withheld_ignore_policy() {
    let (fixture, project, first, _) = historical_fixture();
    // An ignore file can exclude itself from the source capture while still
    // excluding historical leaves, including leaves deleted from the live tree.
    std::fs::remove_file(project.root.join("src/ignored.txt")).unwrap();
    for (policy, text) in [
        (".gitignore", ".gitignore\nsrc/ignored.txt\n"),
        (
            ".gitignore",
            "src/ignored.txt\npassword = 'private-fixture-value'\n",
        ),
        ("src/.ignore", ".ignore\nignored.txt\n"),
    ] {
        std::fs::write(project.root.join(".gitignore"), "").unwrap();
        std::fs::write(project.root.join(policy), text).unwrap();
        let capture = ResearchSnapshot::capture(
            &project,
            &[],
            &fixture.path().join("cache"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let receipt = inspect_history(
            &capture,
            &git_environment(fixture.path()),
            ResearchRequestKind::GitShow {
                commit: first.clone(),
                path: Some("src/ignored.txt".into()),
            },
        );
        assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
        assert!(receipt.content.is_empty());
        assert!(receipt.command.is_none());
    }
}

#[test]
fn git_research_uses_captured_history_and_ignores_executable_repository_configuration() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("code.rs"), "first symbol\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored*\n").unwrap();
    std::fs::write(root.join(".env"), "private history\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "first"]);
    let first = git(&root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("code.rs"), "second symbol\n").unwrap();
    git(&root, &["add", "code.rs"]);
    git(&root, &["commit", "-qm", "second"]);
    let second = git(&root, &["rev-parse", "HEAD"]);
    let sentinel = fixture.path().join("sentinel");
    let helper = fixture.path().join("helper");
    std::fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    for key in [
        "core.fsmonitor",
        "diff.external",
        "core.hooksPath",
        "core.pager",
        "credential.helper",
    ] {
        git(&root, &["config", key, helper.to_str().unwrap()]);
    }
    std::fs::write(root.join(".gitattributes"), "*.rs diff=fixture\n").unwrap();
    git(
        &root,
        &["config", "diff.fixture.textconv", helper.to_str().unwrap()],
    );
    let project = Project::open(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let mut doc = project.open_document("code.rs", false).unwrap();
    doc.snapshot("overlay symbol\n".into());
    let capture =
        ResearchSnapshot::capture(&project, &[doc], &fixture.path().join("cache"), &cancel)
            .unwrap();
    let environment = git_environment(fixture.path());
    let mut context =
        ResearchContext::new(&capture, "session", "operation", Some(&environment), &[]).unwrap();
    context.begin_round(4).unwrap();
    let mut broker = ResearchBroker::with_executor(FixtureGitExecutor);
    let mut activity = Vec::new();
    let history = broker
        .execute(
            request(ResearchRequestKind::GitHistory {
                path: Some("code.rs".into()),
                limit: 50,
            }),
            &mut context,
            &cancel,
            |e| {
                activity.push(e.clone());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(history.status, ResearchReceiptStatus::Complete);
    assert!(history.content.contains(&first) && history.content.contains(&second));
    let show = broker
        .execute(
            request(ResearchRequestKind::GitShow {
                commit: first.clone(),
                path: Some("code.rs".into()),
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(show.content, "first symbol\n");
    let diff = broker
        .execute(
            request(ResearchRequestKind::GitDiff {
                base: first,
                head: second,
                paths: vec!["code.rs".into()],
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(diff.content.contains("+second symbol"));
    let status = broker
        .execute(
            request(ResearchRequestKind::Inspect {
                recipe: InspectionRecipe::GitStatus,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert!(status.content.contains("code.rs"));
    assert!(!status.content.contains(".env"));
    assert!(!sentinel.exists());
    assert!(capture.unchanged());
    assert!(history.command.as_ref().is_some_and(
        |c| c.exit_code == Some(0) && c.argv.iter().any(|a| a == "core.fsmonitor=false")
    ));
    assert!(
        matches!(activity.first(), Some(ResearchProgress::Started { .. }))
            && matches!(activity.last(), Some(ResearchProgress::Completed { .. }))
    );
    assert_eq!(
        std::fs::read_to_string(root.join("code.rs")).unwrap(),
        "second symbol\n"
    );
    context.begin_round(1).unwrap();
    let excluded = broker
        .execute(
            request(ResearchRequestKind::GitShow {
                commit: "1234567".into(),
                path: Some(".env".into()),
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(excluded.status, ResearchReceiptStatus::Unavailable);
    assert!(excluded.command.is_none());
}

#[test]
fn bounded_inspection_outputs_report_filtering_timeout_cancel_and_actual_exit() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("code"), "text").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "first"]);
    let project = Project::open(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let capture =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    let environment = git_environment(fixture.path());
    let mut context =
        ResearchContext::new(&capture, "session", "op", Some(&environment), &[]).unwrap();
    context.begin_round(4).unwrap();
    for output in [
        InspectionOutput {
            output: b"password = 'private-fixture-value'".to_vec(),
            exit_code: Some(0),
            ..Default::default()
        },
        InspectionOutput {
            output: "🦀".repeat(30_000).into_bytes(),
            exit_code: Some(0),
            truncated: true,
            ..Default::default()
        },
        InspectionOutput {
            output: b"partial".to_vec(),
            exit_code: Some(137),
            timed_out: true,
            ..Default::default()
        },
        InspectionOutput {
            output: b"partial".to_vec(),
            exit_code: Some(137),
            cancelled: true,
            ..Default::default()
        },
    ] {
        let timed = output.timed_out;
        let stopped = output.cancelled;
        let mut broker = ResearchBroker::with_executor(FakeExecutor {
            output,
            ..Default::default()
        });
        let receipt = broker
            .execute(
                request(ResearchRequestKind::GitHistory {
                    path: None,
                    limit: 1,
                }),
                &mut context,
                &cancel,
                |_| Ok(()),
            )
            .unwrap();
        assert!(!receipt.content.contains("private-fixture"));
        assert!(receipt.content.len() <= MAX_RESEARCH_RECEIPT_BYTES);
        receipt.validate().unwrap();
        if timed {
            assert_eq!(receipt.status, ResearchReceiptStatus::Unavailable);
        } else if stopped {
            assert_eq!(receipt.status, ResearchReceiptStatus::Cancelled);
        } else {
            assert_eq!(receipt.status, ResearchReceiptStatus::Partial);
        }
        assert!(receipt.command.is_some());
        assert!(capture.unchanged());
    }
    context.begin_round(1).unwrap();
    cancel.store(true, Ordering::Relaxed);
    let mut broker = ResearchBroker::with_executor(FakeExecutor::default());
    let receipt = broker
        .execute(
            request(ResearchRequestKind::GitHistory {
                path: None,
                limit: 1,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(receipt.status, ResearchReceiptStatus::Cancelled);
    assert!(broker.executor().commands.is_empty());
}

#[test]
fn ranges_results_and_operation_text_are_bounded_with_explicit_partial_receipts() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("large"), "🦀".repeat(20_000)).unwrap();
    std::fs::write(root.join("other"), "needle\nneedle\n").unwrap();
    let project = Project::open(&root).unwrap();
    let cancel = AtomicBool::new(false);
    let capture =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    let mut context = ResearchContext::new(&capture, "session", "op", None, &[]).unwrap();
    let mut broker = ResearchBroker::with_executor(FakeExecutor::default());
    context.begin_round(4).unwrap();
    for _ in 0..4 {
        let receipt = broker
            .execute(
                request(ResearchRequestKind::ReadFileRange {
                    path: "large".into(),
                    start_line: 1,
                    end_line: 1,
                }),
                &mut context,
                &cancel,
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(receipt.status, ResearchReceiptStatus::Partial);
        assert!(receipt.content.len() <= MAX_RESEARCH_RECEIPT_BYTES);
        receipt.validate().unwrap();
    }
    assert!(context.budget().returned_bytes <= MAX_RESEARCH_RETURNED_BYTES);
    assert!(
        ResearchRequest::new(ResearchRequestKind::ReadFileRange {
            path: "large".into(),
            start_line: 1,
            end_line: 2001
        })
        .is_err()
    );
    assert!(
        ResearchRequest::new(ResearchRequestKind::GitHistory {
            path: None,
            limit: 51
        })
        .is_err()
    );
    assert!(
        ResearchRequest::new(ResearchRequestKind::GitShow {
            commit: "--output=sentinel".into(),
            path: None
        })
        .is_err()
    );
    let mut context = ResearchContext::new(&capture, "session", "results", None, &[]).unwrap();
    context.begin_round(2).unwrap();
    let list = broker
        .execute(
            request(ResearchRequestKind::ListFiles {
                prefix: None,
                limit: 1,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(list.status, ResearchReceiptStatus::Partial);
    assert_eq!(list.content.lines().count(), 1);
    let search = broker
        .execute(
            request(ResearchRequestKind::SearchText {
                query: "needle".into(),
                paths: vec![],
                limit: 1,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(search.status, ResearchReceiptStatus::Partial);
    assert_eq!(search.sources.len(), 1);
}

#[test]
fn linked_worktree_history_is_captured_without_granting_live_git_metadata_reads() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("main");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("code"), "first").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "fixture"]);
    let commit = git(&root, &["rev-parse", "HEAD"]);
    let worktree = fixture.path().join("worktree");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fixture-worktree",
            worktree.to_str().unwrap(),
        ],
    );
    let project = Project::open(&worktree).unwrap();
    let cancel = AtomicBool::new(false);
    let capture =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    let environment = git_environment(fixture.path());
    let mut context =
        ResearchContext::new(&capture, "session", "op", Some(&environment), &[]).unwrap();
    context.begin_round(1).unwrap();
    let mut broker = ResearchBroker::with_executor(FixtureGitExecutor);
    let receipt = broker
        .execute(
            request(ResearchRequestKind::GitHistory {
                path: None,
                limit: 1,
            }),
            &mut context,
            &cancel,
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(receipt.status, ResearchReceiptStatus::Complete);
    assert_eq!(receipt.content, format!("{commit}\n"));
    let command = receipt.command.unwrap();
    assert!(!command.argv.iter().any(|arg| arg.contains(".git")));
    assert!(capture.unchanged());
    // A new commit changes the next capture identity while this capture stays fixed.
    git(
        &worktree,
        &["commit", "--allow-empty", "-qm", "new history"],
    );
    let changed =
        ResearchSnapshot::capture(&project, &[], &fixture.path().join("cache"), &cancel).unwrap();
    assert_ne!(capture.id(), changed.id());
    assert!(capture.unchanged());
}
