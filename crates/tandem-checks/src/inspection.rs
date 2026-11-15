//! Fixed, host-selected research commands. No model turn or shell is involved.
use crate::{CheckEnvironment, MaterializedSnapshot, secure_read};
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tandem_codex::Client;
use tandem_core::*;

pub const INSPECTION_TIMEOUT_MS: u64 = 15_000;
pub const INSPECTION_OUTPUT_LIMIT: usize = MAX_RESEARCH_RECEIPT_BYTES;

/// Host-configured executable identity; constructing this never enables a tool.
#[derive(Clone, Debug)]
pub struct InspectionTool {
    pub executable: PathBuf,
    pub fingerprint: String,
    pub read_roots: Vec<PathBuf>,
}
impl InspectionTool {
    fn verify(&self, project: &Path) -> Result<()> {
        ensure!(
            self.executable.is_absolute() && file_hash(&self.executable)? == self.fingerprint,
            "Inspection executable changed or is untrusted"
        );
        for root in &self.read_roots {
            ensure!(
                !project.starts_with(root),
                "Tool read roots must not include the source project"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct InspectionEnvironment {
    checks: CheckEnvironment,
    git: Option<InspectionTool>,
    cargo: Option<InspectionTool>,
    python: Option<InspectionTool>,
}
impl InspectionEnvironment {
    /// Existing configured trust is required; no discovery or new grant occurs.
    /// Git is an optional, explicitly trusted host tool, identified by its hash.
    pub fn configured(
        checks: CheckEnvironment,
        trust: &ProjectTools,
        git: Option<InspectionTool>,
    ) -> Result<Self> {
        let cargo = checks
            .tools
            .as_ref()
            .filter(|t| {
                trust.trusted_fingerprint.as_ref() == Some(&t.fingerprint)
                    && rust_identity(t).is_ok_and(|id| id == t.fingerprint)
            })
            .map(|t| -> Result<_> {
                Ok(InspectionTool {
                    executable: t.cargo.clone(),
                    fingerprint: file_hash(&t.cargo)?,
                    read_roots: vec![t.toolchain.clone()],
                })
            })
            .transpose()?;
        let python = checks
            .python
            .as_ref()
            .filter(|t| {
                trust.python_trusted_fingerprint.as_ref() == Some(&t.fingerprint)
                    && python_identity(t).is_ok_and(|id| id == t.fingerprint)
            })
            .map(|t| -> Result<_> {
                Ok(InspectionTool {
                    executable: t.interpreter.clone(),
                    fingerprint: file_hash(&t.interpreter)?,
                    read_roots: t.read_roots(),
                })
            })
            .transpose()?;
        Ok(Self {
            checks,
            git,
            cargo,
            python,
        })
    }
}

fn rust_identity(tools: &RustTools) -> Result<String> {
    let mut identity = format!(
        "rust-tools-v1:{}:{}",
        tools.label,
        tools.rust_analyzer.display()
    );
    for path in [&tools.cargo, &tools.rustc, &tools.rust_analyzer] {
        identity.push_str(&file_hash(path)?);
    }
    Ok(content_hash(identity.as_bytes()))
}
fn python_identity(tools: &PythonTools) -> Result<String> {
    let version = tools
        .label
        .split(" · Pyright ")
        .nth(1)
        .context("Invalid configured Python identity")?;
    let mut identity = format!(
        "python-v1:{}:{}:{}",
        tools.interpreter.display(),
        file_hash(&tools.executable)?,
        version
    );
    if let Some(node) = &tools.node {
        identity.push_str(&file_hash(node)?);
    }
    Ok(content_hash(identity.as_bytes()))
}

pub struct GitSnapshot {
    directory: PathBuf,
    hashes: BTreeMap<String, String>,
}
impl GitSnapshot {
    pub fn identity(&self) -> String {
        content_hash(
            serde_json::to_vec(&self.hashes)
                .expect("string map serialization")
                .as_slice(),
        )
    }
    pub fn unchanged(&self) -> bool {
        let Ok(project) = Project::open(&self.directory) else {
            return false;
        };
        self.hashes.iter().all(|(path, hash)| {
            secure_read(&project, path).is_ok_and(|(bytes, _)| content_hash(&bytes) == *hash)
        })
    }
}

/// Copy only Git's data, never its configuration, hooks, alternates or helpers.
/// Linked-worktree metadata must have Git's conventional common-dir/backlink.
pub fn capture_git(
    project: &Project,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<Option<GitSnapshot>> {
    let entry = project.root.join(".git");
    let Ok(metadata) = fs::symlink_metadata(&entry) else {
        return Ok(None);
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "Linked Git directory is unavailable"
    );
    let (local, common) = if metadata.is_dir() {
        (entry.clone(), entry)
    } else {
        let (bytes, _) = secure_read(project, ".git")?;
        let text = std::str::from_utf8(&bytes)?.trim();
        let gitdir = text
            .strip_prefix("gitdir: ")
            .context("Invalid Git worktree metadata")?;
        ensure!(!gitdir.contains('\n'), "Invalid Git worktree metadata");
        let local = project.root.join(gitdir).canonicalize()?;
        let local_project = Project::open(&local)?;
        let (bytes, _) = secure_read(&local_project, "commondir")?;
        ensure!(
            std::str::from_utf8(&bytes)?.trim() == "../..",
            "Unsupported Git worktree common directory"
        );
        let common = local
            .parent()
            .and_then(Path::parent)
            .context("Invalid Git worktree common directory")?
            .to_path_buf();
        ensure!(
            local
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "worktrees"),
            "Invalid Git worktree directory"
        );
        let (backlink, _) = secure_read(&local_project, "gitdir")?;
        ensure!(
            Path::new(std::str::from_utf8(&backlink)?.trim()).canonicalize()?
                == entry.canonicalize()?,
            "Git worktree backlink differs"
        );
        (local, common)
    };
    ensure!(
        !destination.starts_with(&project.root),
        "Git capture must be private"
    );
    fs::create_dir(destination)?;
    let local_project = Project::open(&local)?;
    let common_project = Project::open(&common)?;
    let mut budget = (0usize, 0usize);
    for name in ["HEAD", "index"] {
        copy_git_file(
            &local_project,
            name,
            &destination.join(name),
            &mut budget,
            cancel,
        )?;
    }
    for name in ["packed-refs", "shallow"] {
        copy_git_file(
            &common_project,
            name,
            &destination.join(name),
            &mut budget,
            cancel,
        )?;
    }
    for name in ["objects", "refs"] {
        copy_git_tree(&common_project, name, destination, &mut budget, cancel, 0)?;
    }
    ensure!(
        destination.join("HEAD").is_file() && destination.join("objects").is_dir(),
        "Git data is unavailable"
    );
    // An explicit empty repository config defeats repository/global helpers.
    fs::write(
        destination.join("config"),
        "[core]\nrepositoryformatversion=0\nbare=false\n",
    )?;
    let captured = Project::open(destination)?;
    let mut hashes = BTreeMap::new();
    hash_git_tree(&captured, "", &mut hashes)?;
    Ok(Some(GitSnapshot {
        directory: destination.to_path_buf(),
        hashes,
    }))
}
fn hash_git_tree(
    project: &Project,
    path: &str,
    hashes: &mut BTreeMap<String, String>,
) -> Result<()> {
    for entry in fs::read_dir(project.root.join(path))? {
        let entry = entry?;
        let relative = if path.is_empty() {
            entry
                .file_name()
                .to_str()
                .context("Invalid Git path")?
                .to_string()
        } else {
            format!(
                "{path}/{}",
                entry.file_name().to_str().context("Invalid Git path")?
            )
        };
        if entry.file_type()?.is_dir() {
            hash_git_tree(project, &relative, hashes)?;
        } else {
            let (bytes, _) = secure_read(project, &relative)?;
            hashes.insert(relative, content_hash(&bytes));
        }
    }
    Ok(())
}
fn copy_git_file(
    project: &Project,
    path: &str,
    target: &Path,
    budget: &mut (usize, usize),
    cancel: &AtomicBool,
) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Git capture cancelled");
    if !project.root.join(path).try_exists()? {
        return Ok(());
    }
    let (bytes, _) = secure_read(project, path)?;
    budget.0 += 1;
    budget.1 += bytes.len();
    ensure!(
        budget.0 <= 20_000 && budget.1 <= 64 * 1024 * 1024,
        "Git metadata exceeds capture budget"
    );
    fs::create_dir_all(target.parent().context("Git output parent missing")?)?;
    fs::write(target, bytes)?;
    Ok(())
}
fn copy_git_tree(
    project: &Project,
    path: &str,
    target: &Path,
    budget: &mut (usize, usize),
    cancel: &AtomicBool,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= 16 && !cancel.load(Ordering::Relaxed),
        "Git traversal stopped"
    );
    let directory = project.root.join(path);
    if !directory.try_exists()? {
        return Ok(());
    }
    ensure!(
        fs::symlink_metadata(&directory)?.is_dir(),
        "Linked Git metadata is unavailable"
    );
    budget.0 += 1;
    ensure!(budget.0 <= 20_000, "Git metadata exceeds traversal budget");
    fs::create_dir_all(target.join(path))?;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let relative = format!(
            "{path}/{}",
            entry
                .file_name()
                .to_str()
                .context("Non-UTF-8 Git metadata")?
        );
        // Alternates can escape the capture; auxiliary metadata is unnecessary.
        if relative == "objects/info" || relative == "refs/replace" {
            continue;
        }
        ensure!(
            !entry.file_type()?.is_symlink(),
            "Linked Git metadata is unavailable"
        );
        if entry.file_type()?.is_dir() {
            copy_git_tree(project, &relative, target, budget, cancel, depth + 1)?;
        } else {
            copy_git_file(project, &relative, &target.join(&relative), budget, cancel)?;
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct PreparedInspection {
    runtime: PathBuf,
    project: PathBuf,
    input: PathBuf,
    artifacts: PathBuf,
    home: PathBuf,
    tool: InspectionTool,
    argv: Vec<String>,
    reads: Vec<PathBuf>,
    denies: Vec<PathBuf>,
    env: BTreeMap<String, String>,
    required_hashes: Vec<(PathBuf, String)>,
    git_leaf_checks: Vec<(Vec<String>, Vec<String>)>,
}
impl PreparedInspection {
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    pub fn cwd(&self) -> &Path {
        &self.input
    }
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.env
    }
    /// Run these fixed commands through the same restricted executor before the
    /// content command. Their output is validation data, never returned text.
    pub fn git_leaf_inspections(&self) -> Vec<Self> {
        self.git_leaf_checks
            .iter()
            .map(|(argv, _)| {
                let mut command = self.clone();
                command.argv = argv.clone();
                command.git_leaf_checks.clear();
                command
            })
            .collect()
    }
    pub fn validate_git_leaves(&self, outputs: &[InspectionOutput]) -> Result<()> {
        ensure!(
            outputs.len() == self.git_leaf_checks.len(),
            "Historical file validation is incomplete"
        );
        let mut present = BTreeSet::new();
        for ((_, paths), output) in self.git_leaf_checks.iter().zip(outputs) {
            ensure!(
                output.exit_code == Some(0)
                    && !output.truncated
                    && !output.cancelled
                    && !output.timed_out,
                "Historical file validation did not complete"
            );
            let text = std::str::from_utf8(&output.output)?;
            ensure!(
                text.is_empty() || text.ends_with('\0'),
                "Historical file validation is incomplete"
            );
            let mut seen = BTreeSet::new();
            for entry in text.split_terminator('\0') {
                let (metadata, path) = entry.split_once('\t').context("Invalid Git tree entry")?;
                let fields = metadata.split(' ').collect::<Vec<_>>();
                ensure!(
                    fields.len() == 3
                        && matches!(fields[0], "100644" | "100755")
                        && fields[1] == "blob"
                        && matches!(fields[2].len(), 40 | 64)
                        && fields[2].bytes().all(|c| c.is_ascii_hexdigit())
                        && paths.iter().any(|p| p == path)
                        && seen.insert(path),
                    "History requires literal regular file leaves; directories and links are unavailable"
                );
                present.insert(path);
            }
        }
        // A deleted/created file can be absent on one side of a diff, but every
        // requested path must be a regular file on at least one captured side.
        ensure!(
            self.git_leaf_checks
                .iter()
                .all(|(_, paths)| paths.iter().all(|p| present.contains(p.as_str()))),
            "One or more historical files are missing"
        );
        Ok(())
    }
    pub fn provenance(&self) -> ResearchCommand {
        ResearchCommand {
            argv: self.argv.clone(),
            cwd: self.input.display().to_string(),
            executable_hash: self.tool.fingerprint.clone(),
            exit_code: None,
            output_truncated: false,
            cancelled: false,
            timed_out: false,
        }
    }
    pub fn permission_config(&self) -> Result<String> {
        let mut reads = self.reads.clone();
        reads.extend(self.tool.read_roots.clone());
        reads.push(self.tool.executable.clone());
        for root in &reads {
            ensure!(
                !self.project.starts_with(root),
                "Inspection read root includes live source"
            );
        }
        let mut config = crate::runner::restricted_command_config(
            "inspection",
            &reads,
            &self.artifacts,
            &self.denies,
        )?;
        config.push_str("[shell_environment_policy]\ninherit=\"none\"\n");
        Ok(config)
    }
}

pub fn prepare_inspection(
    environment: &InspectionEnvironment,
    project: &Project,
    snapshot: &MaterializedSnapshot,
    git: Option<&GitSnapshot>,
    request: &ResearchRequest,
) -> Result<PreparedInspection> {
    request.validate()?;
    let directory = snapshot.artifacts.join(new_id());
    fs::create_dir(&directory)?;
    let home = directory.join("runtime-home");
    fs::create_dir(&home)?;
    let outputs = directory.join("output");
    fs::create_dir(&outputs)?;
    let mut reads = vec![snapshot.input.clone()];
    let mut env = BTreeMap::from([
        ("HOME".into(), outputs.display().to_string()),
        ("TMPDIR".into(), outputs.display().to_string()),
        ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ("LANG".into(), "C.UTF-8".into()),
    ]);
    let mut git_leaf_checks = Vec::new();
    let (tool, args) = match &request.kind {
        ResearchRequestKind::GitHistory { .. }
        | ResearchRequestKind::GitShow { .. }
        | ResearchRequestKind::GitDiff { .. }
        | ResearchRequestKind::Inspect {
            recipe: InspectionRecipe::GitStatus,
        } => {
            let git =
                git.context("Git history unavailable: folder has no captured Git metadata")?;
            let tool = environment
                .git
                .as_ref()
                .context("Git inspection tool is missing or untrusted")?;
            reads.push(git.directory.clone());
            for (key, value) in [
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_SYSTEM", "/dev/null"),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_OPTIONAL_LOCKS", "0"),
                ("GIT_NO_REPLACE_OBJECTS", "1"),
                ("GIT_ATTR_NOSYSTEM", "1"),
                ("GIT_CONFIG_COUNT", "0"),
            ] {
                env.insert(key.into(), value.into());
            }
            env.insert("GIT_DIR".into(), git.directory.display().to_string());
            env.insert("GIT_WORK_TREE".into(), snapshot.input.display().to_string());
            let mut args = vec![
                "--no-pager".into(),
                "--no-optional-locks".into(),
                "--no-replace-objects".into(),
                "--literal-pathspecs".into(),
            ];
            for config in [
                "core.fsmonitor=false",
                "core.hooksPath=/dev/null",
                "core.attributesFile=/dev/null",
                "diff.external=",
                "credential.helper=",
                "core.pager=cat",
                "interactive.diffFilter=",
            ] {
                args.extend(["-c".into(), config.into()]);
            }
            let revisions_and_paths = match &request.kind {
                ResearchRequestKind::GitShow {
                    commit,
                    path: Some(path),
                } => {
                    vec![(commit, vec![path.clone()])]
                }
                ResearchRequestKind::GitDiff { base, head, paths } => {
                    vec![(base, paths.clone()), (head, paths.clone())]
                }
                _ => vec![],
            };
            for (commit, paths) in revisions_and_paths {
                let mut check = vec![tool.executable.display().to_string()];
                check.extend(args.clone());
                check.extend([
                    "ls-tree".into(),
                    "--full-tree".into(),
                    "-z".into(),
                    format!("{commit}^{{commit}}"),
                    "--".into(),
                ]);
                check.extend(paths.iter().cloned());
                git_leaf_checks.push((check, paths));
            }
            match &request.kind {
                ResearchRequestKind::GitHistory { path, limit } => {
                    args.extend([
                        "log".into(),
                        format!("-{limit}"),
                        "--format=%H".into(),
                        "--no-show-signature".into(),
                        "HEAD^{commit}".into(),
                        "--".into(),
                    ]);
                    args.extend(path.clone());
                }
                ResearchRequestKind::GitShow { commit, path } => {
                    if let Some(path) = path {
                        args.extend([
                            "show".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            format!("{commit}^{{commit}}:{path}"),
                        ]);
                    } else {
                        args.extend([
                            "show".into(),
                            "--no-patch".into(),
                            "--format=%H".into(),
                            "--no-show-signature".into(),
                            format!("{commit}^{{commit}}"),
                            "--".into(),
                        ]);
                    }
                }
                ResearchRequestKind::GitDiff { base, head, paths } => {
                    args.extend([
                        "diff".into(),
                        "--no-ext-diff".into(),
                        "--no-textconv".into(),
                        "--no-renames".into(),
                        "--ignore-submodules=all".into(),
                        format!("{base}^{{commit}}"),
                        format!("{head}^{{commit}}"),
                        "--".into(),
                    ]);
                    args.extend(paths.iter().cloned());
                }
                _ => args.extend([
                    "status".into(),
                    "--porcelain=v1".into(),
                    "-z".into(),
                    "--untracked-files=all".into(),
                    "--no-renames".into(),
                    "--ignore-submodules=all".into(),
                ]),
            };
            (tool, args)
        }
        ResearchRequestKind::Inspect {
            recipe: InspectionRecipe::RustToolchain | InspectionRecipe::RustWorkspaceMetadata,
        } => {
            let tool = environment
                .cargo
                .as_ref()
                .context("Configured Rust toolchain is missing or untrusted")?;
            let metadata = matches!(
                request.kind,
                ResearchRequestKind::Inspect {
                    recipe: InspectionRecipe::RustWorkspaceMetadata
                }
            );
            let mut args = if metadata {
                vec![
                    "metadata",
                    "--offline",
                    "--locked",
                    "--no-deps",
                    "--format-version",
                    "1",
                ]
            } else {
                vec!["--version"]
            }
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
            let tools = environment
                .checks
                .tools
                .as_ref()
                .context("Rust tools unavailable")?;
            if metadata {
                ensure!(
                    snapshot.input.join("Cargo.toml").is_file()
                        && snapshot.input.join("Cargo.lock").is_file(),
                    "Offline locked metadata requires captured Cargo.toml and Cargo.lock"
                );
                // Deny project configuration rather than execute wrappers or helpers.
                ensure!(
                    !snapshot
                        .manifest
                        .files
                        .iter()
                        .any(|f| f.path.ends_with(".cargo/config")
                            || f.path.ends_with(".cargo/config.toml")),
                    "Cargo metadata unavailable with project Cargo configuration; manifests remain readable"
                );
                args.extend([
                    "--config".into(),
                    format!(
                        "build.rustc={}",
                        serde_json::to_string(&tools.rustc.display().to_string())?
                    ),
                    "--config".into(),
                    "build.rustc-wrapper=\"\"".into(),
                    "--config".into(),
                    "build.rustc-workspace-wrapper=\"\"".into(),
                ]);
            }
            env.insert("CARGO_HOME".into(), outputs.display().to_string());
            env.insert(
                "CARGO_TARGET_DIR".into(),
                outputs.join("target").display().to_string(),
            );
            env.insert("CARGO_NET_OFFLINE".into(), "true".into());
            env.insert("RUSTC".into(), tools.rustc.display().to_string());
            env.insert("RUSTC_WRAPPER".into(), String::new());
            env.insert("RUSTC_WORKSPACE_WRAPPER".into(), String::new());
            (tool, args)
        }
        ResearchRequestKind::Inspect {
            recipe: InspectionRecipe::PythonToolchain | InspectionRecipe::PythonProjectMetadata,
        } => {
            let tool = environment
                .python
                .as_ref()
                .context("Configured Python interpreter is missing or untrusted")?;
            // This fixed script uses only stdlib and never imports project code.
            let script = if matches!(
                request.kind,
                ResearchRequestKind::Inspect {
                    recipe: InspectionRecipe::PythonProjectMetadata
                }
            ) {
                "import sys,json,sysconfig,pathlib\nr={'version':sys.version,'implementation':sys.implementation.name,'platform':sysconfig.get_platform()}\np=pathlib.Path('pyproject.toml')\nif p.is_file():\n import tomllib\n with p.open('rb') as f: r['project']=tomllib.load(f).get('project',{})\nprint(json.dumps(r))"
            } else {
                "import sys,json,sysconfig; print(json.dumps({'version':sys.version,'implementation':sys.implementation.name,'platform':sysconfig.get_platform()}))"
            };
            (
                tool,
                vec![
                    "-I".into(),
                    "-S".into(),
                    "-B".into(),
                    "-c".into(),
                    script.into(),
                ],
            )
        }
        _ => anyhow::bail!("Request is not a fixed local inspection"),
    };
    let mut required_hashes = Vec::new();
    if environment
        .cargo
        .as_ref()
        .is_some_and(|cargo| cargo.executable == tool.executable)
    {
        let tools = environment
            .checks
            .tools
            .as_ref()
            .context("Rust tools unavailable")?;
        ensure!(
            rust_identity(tools)? == tools.fingerprint,
            "Configured Rust executable identities changed"
        );
        required_hashes.push((tools.rustc.clone(), file_hash(&tools.rustc)?));
    }
    if environment
        .python
        .as_ref()
        .is_some_and(|python| python.executable == tool.executable)
    {
        let tools = environment
            .checks
            .python
            .as_ref()
            .context("Python tools unavailable")?;
        ensure!(
            python_identity(tools)? == tools.fingerprint,
            "Configured Python executable identities changed"
        );
    }
    tool.verify(&project.root)?;
    let mut denies = vec![project.root.clone()];
    denies.extend(environment.checks.denied_roots.clone());
    // The source and Git data stay read-only; only this command's output is writable.
    let result = PreparedInspection {
        runtime: environment.checks.runtime.clone(),
        project: project.root.clone(),
        input: snapshot.input.clone(),
        artifacts: outputs,
        home,
        argv: std::iter::once(tool.executable.display().to_string())
            .chain(args)
            .collect(),
        tool: tool.clone(),
        reads,
        denies,
        env,
        required_hashes,
        git_leaf_checks,
    };
    result.permission_config()?;
    Ok(result)
}

#[derive(Clone, Debug, Default)]
pub struct InspectionOutput {
    pub output: Vec<u8>,
    pub exit_code: Option<i64>,
    pub truncated: bool,
    pub cancelled: bool,
    pub timed_out: bool,
}
pub trait InspectionExecutor {
    fn execute(
        &mut self,
        command: &PreparedInspection,
        cancel: &AtomicBool,
        started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
    ) -> Result<InspectionOutput>;
}
#[derive(Default)]
pub struct RuntimeInspectionExecutor;
impl InspectionExecutor for RuntimeInspectionExecutor {
    fn execute(
        &mut self,
        command: &PreparedInspection,
        cancel: &AtomicBool,
        started: &mut dyn FnMut(&ResearchCommand) -> Result<()>,
    ) -> Result<InspectionOutput> {
        command.tool.verify(&command.project)?;
        for (path, hash) in &command.required_hashes {
            ensure!(
                file_hash(path)? == *hash,
                "Configured inspection helper executable changed"
            );
        }
        tandem_codex::verify_runtime(&command.runtime)?;
        fs::write(
            command.home.join("config.toml"),
            command.permission_config()?,
        )?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(InspectionOutput {
                cancelled: true,
                ..Default::default()
            });
        }
        let mut client = Client::start(&command.runtime, &command.home, &command.input)?;
        let process = format!("inspection-{}", new_id());
        let id = crate::runner::begin_command(
            &mut client,
            &command.argv,
            &command.input,
            "inspection",
            &process,
            INSPECTION_TIMEOUT_MS,
            INSPECTION_OUTPUT_LIMIT,
            json!(command.env),
        )?;
        started(&command.provenance())?;
        let mut output = InspectionOutput::default();
        let deadline = Instant::now() + Duration::from_millis(INSPECTION_TIMEOUT_MS);
        let hard_deadline = deadline + Duration::from_secs(5);
        let mut stopping = false;
        let result = loop {
            ensure!(
                Instant::now() < hard_deadline,
                "Inspection runtime did not report a terminal result; outcome unknown"
            );
            if !stopping && (cancel.load(Ordering::Relaxed) || Instant::now() >= deadline) {
                output.cancelled = cancel.load(Ordering::Relaxed);
                output.timed_out = !output.cancelled;
                // Runtime terminates the command's process group, including children.
                client.begin_request("command/exec/terminate", json!({"processId":process}))?;
                stopping = true;
            }
            let Some(event) = client.poll_event(Duration::from_millis(50))? else {
                continue;
            };
            if event["id"].as_u64() == Some(id) {
                ensure!(
                    event.get("error").is_none(),
                    "Inspection command failed to execute"
                );
                output.exit_code = event["result"]["exitCode"].as_i64();
                break output;
            }
            if event["method"] == "command/exec/outputDelta"
                && event["params"]["processId"] == process
            {
                let bytes = base64::engine::general_purpose::STANDARD.decode(
                    event["params"]["deltaBase64"]
                        .as_str()
                        .context("Missing inspection output bytes")?,
                )?;
                let remaining = INSPECTION_OUTPUT_LIMIT.saturating_sub(output.output.len());
                output
                    .output
                    .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
                output.truncated |=
                    bytes.len() > remaining || event["params"]["capReached"] == true;
            }
        };
        client.shutdown()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Project, CheckEnvironment, ProjectTools) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        let toolchain = temp.path().join("toolchain");
        fs::create_dir(&toolchain).unwrap();
        for name in ["cargo", "rustc", "rust-analyzer"] {
            fs::write(toolchain.join(name), name).unwrap();
        }
        let mut tools = RustTools {
            cargo: toolchain.join("cargo"),
            rustc: toolchain.join("rustc"),
            rust_analyzer: toolchain.join("rust-analyzer"),
            rustfmt: toolchain.join("rustfmt"),
            label: "fixture".into(),
            fingerprint: String::new(),
            toolchain,
        };
        tools.fingerprint = rust_identity(&tools).unwrap();
        let trust = ProjectTools {
            trusted_fingerprint: Some(tools.fingerprint.clone()),
            ..Default::default()
        };
        let checks = CheckEnvironment {
            runtime: temp.path().join("runtime"),
            tools: Some(tools),
            python: None,
            runs: temp.path().join("runs"),
            cargo_cache: temp.path().join("cargo"),
            clang: temp.path().join("clang"),
            sdk: None,
            denied_roots: vec![],
        };
        let project = Project::open(&root).unwrap();
        (temp, project, checks, trust)
    }
    fn req(recipe: InspectionRecipe) -> ResearchRequest {
        ResearchRequest::new(ResearchRequestKind::Inspect { recipe }).unwrap()
    }
    #[test]
    fn metadata_is_fixed_offline_locked_and_requires_existing_trust() {
        let (temp, project, checks, trust) = setup();
        let cancel = AtomicBool::new(false);
        let snapshot =
            crate::materialize(&project, &[], &temp.path().join("first"), &cancel).unwrap();
        let env = InspectionEnvironment::configured(checks.clone(), &trust, None).unwrap();
        let error = prepare_inspection(
            &env,
            &project,
            &snapshot,
            None,
            &req(InspectionRecipe::RustWorkspaceMetadata),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("Cargo.lock"));
        fs::write(project.root.join("Cargo.lock"), "version=4\n").unwrap();
        let snapshot =
            crate::materialize(&project, &[], &temp.path().join("second"), &cancel).unwrap();
        let command = prepare_inspection(
            &env,
            &project,
            &snapshot,
            None,
            &req(InspectionRecipe::RustWorkspaceMetadata),
        )
        .unwrap();
        assert_eq!(
            &command.argv()[1..7],
            &[
                "metadata",
                "--offline",
                "--locked",
                "--no-deps",
                "--format-version",
                "1"
            ]
        );
        let config = command.permission_config().unwrap();
        let parsed: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(
            parsed["shell_environment_policy"]["inherit"].as_str(),
            Some("none")
        );
        assert_eq!(
            parsed["permissions"]["inspection"]["network"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(
            parsed["permissions"]["inspection"]["filesystem"][project.root.to_str().unwrap()]
                .as_str(),
            Some("deny")
        );
        assert_eq!(
            parsed["permissions"]["inspection"]["filesystem"][snapshot.input.to_str().unwrap()]
                .as_str(),
            Some("read")
        );
        assert_eq!(INSPECTION_TIMEOUT_MS, 15_000);
        assert_eq!(INSPECTION_OUTPUT_LIMIT, 64 * 1024);
        assert!(!command.environment().contains_key("RUSTUP_HOME"));
        let untrusted =
            InspectionEnvironment::configured(checks.clone(), &ProjectTools::default(), None)
                .unwrap();
        assert!(
            prepare_inspection(
                &untrusted,
                &project,
                &snapshot,
                None,
                &req(InspectionRecipe::RustToolchain)
            )
            .is_err()
        );
        fs::write(&checks.tools.as_ref().unwrap().rustc, "changed executable").unwrap();
        assert!(
            prepare_inspection(
                &env,
                &project,
                &snapshot,
                None,
                &req(InspectionRecipe::RustToolchain)
            )
            .is_err()
        );
        assert!(crate::snapshot_unchanged(&snapshot));
    }
    #[test]
    fn python_metadata_uses_only_a_fixed_isolated_stdlib_script() {
        let (temp, project, mut checks, mut trust) = setup();
        let interpreter = temp.path().join("python");
        fs::write(&interpreter, "python identity").unwrap();
        let mut python = PythonTools {
            interpreter: interpreter.clone(),
            executable: interpreter,
            environment: temp.path().join("environment"),
            prefix: temp.path().join("prefix"),
            node: None,
            server: None,
            label: "Python fixture · Pyright not installed".into(),
            fingerprint: String::new(),
        };
        python.fingerprint = python_identity(&python).unwrap();
        trust.python_trusted_fingerprint = Some(python.fingerprint.clone());
        checks.python = Some(python);
        let env = InspectionEnvironment::configured(checks.clone(), &trust, None).unwrap();
        let snapshot = crate::materialize(
            &project,
            &[],
            &temp.path().join("snapshot"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let command = prepare_inspection(
            &env,
            &project,
            &snapshot,
            None,
            &req(InspectionRecipe::PythonProjectMetadata),
        )
        .unwrap();
        assert_eq!(&command.argv()[1..5], &["-I", "-S", "-B", "-c"]);
        assert!(command.argv()[5].contains("tomllib") && !command.argv()[5].contains("importlib"));
        assert!(!command.argv()[5].contains("sys.path.insert"));
        assert!(serde_json::from_value::<ResearchRequest>(json!({"id":"r","kind":{"type":"Inspect","recipe":"PythonToolchain","argv":["custom"]}})).is_err());
        assert!(
            serde_json::from_value::<ResearchRequest>(
                json!({"id":"r","kind":{"type":"Inspect","recipe":"Unknown"}})
            )
            .is_err()
        );
        fs::write(&checks.python.as_ref().unwrap().interpreter, "changed").unwrap();
        assert!(
            prepare_inspection(
                &env,
                &project,
                &snapshot,
                None,
                &req(InspectionRecipe::PythonToolchain)
            )
            .is_err()
        );
    }
}
