//! Snapshotted production tool configuration for the host's fixed research broker.
use anyhow::Result;
use std::path::Path;
use tandem_checks::{CheckEnvironment, InspectionEnvironment, InspectionTool};
use tandem_core::{ProjectTools, file_hash};

pub struct ProjectInspection {
    checks: Option<CheckEnvironment>,
    trust: ProjectTools,
    git: Option<InspectionTool>,
}
impl ProjectInspection {
    pub fn snapshot(checks: Option<CheckEnvironment>, trust: ProjectTools) -> Self {
        // Git is a built-in host capability, selected only from this system path.
        // Language executables still require their independent saved project trust.
        let git = Path::new("/usr/bin/git")
            .canonicalize()
            .ok()
            .and_then(|executable| {
                let fingerprint = file_hash(&executable).ok()?;
                Some(InspectionTool {
                    executable,
                    fingerprint,
                    read_roots: vec![],
                })
            });
        Self { checks, trust, git }
    }
    pub fn environment(&self) -> Result<Option<InspectionEnvironment>> {
        self.checks
            .clone()
            .map(|checks| InspectionEnvironment::configured(checks, &self.trust, self.git.clone()))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
    use tandem_checks::{InspectionExecutor, InspectionOutput, PreparedInspection};
    use tandem_companion::{ResearchBroker, ResearchContext, ResearchProgress, ResearchSnapshot};
    use tandem_core::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        project: Project,
        checks: CheckEnvironment,
        trust: ProjectTools,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::create_dir(root.join("project")).unwrap();
            fs::write(root.join("project/lib.rs"), "pub fn greeting() {}\n").unwrap();
            fs::write(
                root.join("project/Cargo.toml"),
                "[package]\nname='fixture'\nversion='0.1.0'\n",
            )
            .unwrap();
            fs::write(root.join("project/Cargo.lock"), "version = 4\n").unwrap();
            fs::create_dir_all(root.join("project/.git/objects")).unwrap();
            fs::create_dir_all(root.join("project/.git/refs/heads")).unwrap();
            fs::write(root.join("project/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
            fs::write(
                root.join("project/.git/refs/heads/main"),
                format!("{}\n", "a".repeat(40)),
            )
            .unwrap();
            fs::create_dir(root.join("toolchain")).unwrap();
            for name in ["cargo", "rustc", "rust-analyzer", "python", "git"] {
                fs::write(root.join("toolchain").join(name), name).unwrap();
            }
            let mut rust = RustTools {
                toolchain: root.join("toolchain"),
                cargo: root.join("toolchain/cargo"),
                rustc: root.join("toolchain/rustc"),
                rust_analyzer: root.join("toolchain/rust-analyzer"),
                rustfmt: root.join("toolchain/rustfmt"),
                label: "Fixture".into(),
                fingerprint: String::new(),
            };
            let mut identity = format!(
                "rust-tools-v1:{}:{}",
                rust.label,
                rust.rust_analyzer.display()
            );
            for tool in [&rust.cargo, &rust.rustc, &rust.rust_analyzer] {
                identity.push_str(&file_hash(tool).unwrap());
            }
            rust.fingerprint = content_hash(identity.as_bytes());
            let interpreter = root.join("toolchain/python");
            let python = PythonTools {
                interpreter: interpreter.clone(),
                executable: interpreter.clone(),
                prefix: root.join("toolchain"),
                environment: root.join("environment"),
                node: None,
                server: None,
                label: "Fixture · Pyright not installed".into(),
                fingerprint: content_hash(
                    format!(
                        "python-v1:{}:{}:not installed",
                        interpreter.display(),
                        file_hash(&interpreter).unwrap()
                    )
                    .as_bytes(),
                ),
            };
            let trust = ProjectTools {
                trusted_fingerprint: Some(rust.fingerprint.clone()),
                python_trusted_fingerprint: Some(python.fingerprint.clone()),
                ..Default::default()
            };
            let checks = CheckEnvironment {
                runtime: root.join("unused-runtime"),
                tools: Some(rust),
                python: Some(python),
                runs: root.join("runs"),
                cargo_cache: root.join("cargo-cache"),
                clang: root.join("clang"),
                sdk: None,
                denied_roots: vec![root.join("private-sentinel")],
            };
            let project = Project::open(&root.join("project")).unwrap();
            Self {
                _temp: temp,
                root,
                project,
                checks,
                trust,
            }
        }
        fn configuration(&self) -> ProjectInspection {
            let mut configuration =
                ProjectInspection::snapshot(Some(self.checks.clone()), self.trust.clone());
            // Substitute only the host's executable identity; never a PATH/model choice.
            let executable = self.root.join("toolchain/git");
            configuration.git = Some(InspectionTool {
                fingerprint: file_hash(&executable).unwrap(),
                executable,
                read_roots: vec![],
            });
            configuration
        }
        fn capture(&self) -> ResearchSnapshot {
            ResearchSnapshot::capture(
                &self.project,
                &[],
                &self.root.join("captures"),
                &AtomicBool::new(false),
            )
            .unwrap()
        }
    }
    #[derive(Default)]
    struct Executor {
        commands: Vec<ResearchCommand>,
        project: PathBuf,
        denied: PathBuf,
    }
    impl InspectionExecutor for Executor {
        fn execute(
            &mut self,
            command: &PreparedInspection,
            cancel: &AtomicBool,
            started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
        ) -> Result<InspectionOutput> {
            assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
            assert_ne!(command.cwd(), self.project);
            let config = command.permission_config()?;
            assert!(config.contains("inherit=\"none\""));
            assert!(config.contains("[permissions.inspection.network]\nenabled=false"));
            assert!(config.contains("shell_tool=false"));
            assert!(config.contains("unified_exec=false"));
            assert!(config.contains(&format!(
                "{}=\"read\"",
                serde_json::to_string(&command.cwd().display().to_string()).unwrap()
            )));
            for path in [&self.project, &self.denied] {
                assert!(config.contains(&format!(
                    "{}=\"deny\"",
                    serde_json::to_string(&path.display().to_string()).unwrap()
                )));
            }
            assert!(!command.environment().contains_key("OPENAI_API_KEY"));
            assert!(!command.environment().contains_key("GIT_CONFIG"));
            let provenance = command.provenance();
            started(&provenance)?;
            self.commands.push(provenance);
            let argv = command.argv();
            let output = if argv.iter().any(|a| a == "ls-tree") {
                format!("100644 blob {}\tlib.rs\0", "b".repeat(40))
            } else if argv.iter().any(|a| a == "log") {
                format!("{}\n", "a".repeat(40))
            } else if argv.iter().any(|a| a == "status") {
                " M lib.rs\0".into()
            } else {
                "Fixture inspection\n".into()
            };
            Ok(InspectionOutput {
                output: output.into_bytes(),
                exit_code: Some(0),
                ..Default::default()
            })
        }
    }
    fn broker(f: &Fixture) -> ResearchBroker<Executor> {
        ResearchBroker::with_executor(Executor {
            project: f.project.root.clone(),
            denied: f.root.join("private-sentinel"),
            ..Default::default()
        })
    }
    fn inspect(recipe: InspectionRecipe) -> ResearchRequestKind {
        ResearchRequestKind::Inspect { recipe }
    }
    fn dispatch(
        f: &Fixture,
        configuration: &ProjectInspection,
        kinds: Vec<ResearchRequestKind>,
    ) -> (Vec<ResearchReceipt>, ResearchBroker<Executor>) {
        let environment = configuration.environment().unwrap();
        let capture = f.capture();
        let mut context =
            ResearchContext::new(&capture, "session", "operation", environment.as_ref(), &[])
                .unwrap();
        let mut broker = broker(f);
        let mut receipts = vec![];
        for chunk in kinds.chunks(4) {
            context.begin_round(chunk.len() as u8).unwrap();
            for kind in chunk {
                receipts.push(
                    broker
                        .execute(
                            ResearchRequest::new(kind.clone()).unwrap(),
                            &mut context,
                            &AtomicBool::new(false),
                            |event| {
                                match event {
                                    ResearchProgress::Started {
                                        session_id,
                                        operation_id,
                                        ..
                                    }
                                    | ResearchProgress::Completed {
                                        session_id,
                                        operation_id,
                                        ..
                                    } => {
                                        assert_eq!(session_id, "session");
                                        assert_eq!(operation_id, "operation");
                                    }
                                    ResearchProgress::CommandStarted { .. } => {}
                                }
                                Ok(())
                            },
                        )
                        .unwrap(),
                );
            }
        }
        assert!(capture.unchanged());
        for receipt in &receipts {
            assert_eq!(receipt.snapshot_id.as_deref(), Some(capture.id()));
        }
        (receipts, broker)
    }
    #[test]
    fn production_adapter_dispatches_trusted_language_and_fixed_host_git_inspections() {
        let f = Fixture::new();
        let configuration = f.configuration();
        let (receipts, broker) = dispatch(
            &f,
            &configuration,
            vec![
                inspect(InspectionRecipe::RustToolchain),
                inspect(InspectionRecipe::RustWorkspaceMetadata),
                inspect(InspectionRecipe::PythonToolchain),
                inspect(InspectionRecipe::PythonProjectMetadata),
                inspect(InspectionRecipe::GitStatus),
                ResearchRequestKind::GitHistory {
                    path: Some("lib.rs".into()),
                    limit: 3,
                },
                ResearchRequestKind::GitShow {
                    commit: "a".repeat(40),
                    path: Some("lib.rs".into()),
                },
                ResearchRequestKind::GitDiff {
                    base: "a".repeat(40),
                    head: "c".repeat(40),
                    paths: vec!["lib.rs".into()],
                },
            ],
        );
        for receipt in receipts {
            assert_eq!(
                receipt.status,
                ResearchReceiptStatus::Complete,
                "{receipt:?}"
            );
            assert!(receipt.command.is_some());
        }
        assert_eq!(broker.executor().commands.len(), 11); // Includes the fixed regular-leaf checks.
        let commands = &broker.executor().commands;
        assert!(commands[0].argv[0].ends_with("/toolchain/cargo"));
        assert!(commands[1].argv.iter().any(|a| a == "--offline"));
        assert_eq!(&commands[2].argv[1..5], &["-I", "-S", "-B", "-c"]);
        assert!(commands[4].argv[0].ends_with("/toolchain/git"));
    }
    #[test]
    fn production_adapter_refuses_untrusted_missing_and_changed_language_tools_independently() {
        for variant in 0..5 {
            let f = Fixture::new();
            let mut configuration = f.configuration();
            match variant {
                0 => {
                    configuration.trust.trusted_fingerprint = None;
                    configuration.trust.python_trusted_fingerprint = None;
                }
                1 => {
                    configuration.trust.trusted_fingerprint = Some("different".into());
                    configuration.trust.python_trusted_fingerprint = Some("different".into());
                }
                2 => {
                    let checks = configuration.checks.as_mut().unwrap();
                    checks.tools = None;
                    checks.python = None;
                }
                3 => {
                    fs::write(&f.checks.tools.as_ref().unwrap().cargo, "changed").unwrap();
                    fs::write(&f.checks.python.as_ref().unwrap().interpreter, "changed").unwrap();
                }
                _ => configuration.checks = None,
            }
            let (receipts, broker) = dispatch(
                &f,
                &configuration,
                vec![
                    inspect(InspectionRecipe::RustToolchain),
                    inspect(InspectionRecipe::PythonToolchain),
                ],
            );
            assert!(
                receipts
                    .iter()
                    .all(|r| r.status == ResearchReceiptStatus::Unavailable),
                "variant {variant}"
            );
            assert!(broker.executor().commands.is_empty());
        }
        for trusted in 0..2 {
            let f = Fixture::new();
            let mut configuration = f.configuration();
            if trusted == 0 {
                configuration.trust.python_trusted_fingerprint = None;
            } else {
                configuration.trust.trusted_fingerprint = None;
            }
            let (receipts, broker) = dispatch(
                &f,
                &configuration,
                vec![
                    inspect(InspectionRecipe::RustToolchain),
                    inspect(InspectionRecipe::PythonToolchain),
                ],
            );
            assert_eq!(receipts[trusted].status, ResearchReceiptStatus::Complete);
            assert_eq!(
                receipts[1 - trusted].status,
                ResearchReceiptStatus::Unavailable
            );
            assert_eq!(broker.executor().commands.len(), 1);
        }
    }
    #[test]
    fn production_adapter_refuses_missing_or_changed_host_git_without_using_custom_git() {
        for missing in [true, false] {
            let f = Fixture::new();
            let mut configuration = f.configuration();
            if missing {
                configuration.git = None;
            } else {
                fs::write(
                    &configuration.git.as_ref().unwrap().executable,
                    "replacement",
                )
                .unwrap();
            }
            let (receipts, broker) = dispatch(
                &f,
                &configuration,
                vec![ResearchRequestKind::GitHistory {
                    path: None,
                    limit: 1,
                }],
            );
            assert_eq!(receipts[0].status, ResearchReceiptStatus::Unavailable);
            assert!(broker.executor().commands.is_empty());
        }
    }
}
