use crate::{MaterializedSnapshot, materialize, snapshot_unchanged};
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tandem_codex::Client;
use tandem_core::*;

const RUNTIME_HASH: &str = "739cda86e15d30a6e2b40cf8dcbbfb988aaf48b305b27065573bb8e9fd41d002";
const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub struct CheckEnvironment {
    pub runtime: PathBuf,
    pub tools: Option<RustTools>,
    pub python: Option<PythonTools>,
    pub runs: PathBuf,
    pub cargo_cache: PathBuf,
    pub clang: PathBuf,
    pub sdk: Option<PathBuf>,
    pub denied_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub argv: Vec<String>,
    pub fingerprint: String,
    pub grant_fingerprint: String,
    pub language: String,
    pub version: String,
    pub read_roots: Vec<PathBuf>,
}

impl CheckEnvironment {
    pub fn discover(project: &Path, runs: PathBuf) -> Result<Self> {
        Self::discover_with_python(project, runs, None)
    }
    pub fn discover_with_python(
        project: &Path,
        runs: PathBuf,
        selected: Option<&Path>,
    ) -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
        let runtime = tandem_codex::discover_runtime()?.binary;
        ensure!(
            file_hash(&runtime)? == RUNTIME_HASH,
            "Validation requires the tested Codex 0.169.0 executable; no fallback was selected"
        );
        let clang = PathBuf::from(
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
        );
        let sdk=PathBuf::from("/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk").canonicalize().ok();
        Ok(Self {
            denied_roots: vec![],
            runtime,
            tools: RustTools::discover(project).ok(),
            python: PythonTools::discover(project, selected).ok(),
            runs,
            cargo_cache: home.join(".cargo"),
            clang,
            sdk,
        })
    }
    pub fn recipe(&self, id: &str) -> Result<Recipe> {
        if id.starts_with("python-") {
            let tools = self
                .python
                .as_ref()
                .context("Select an installed Python interpreter")?;
            let (title, args) = match id {
                "python-benchmark" => {
                    ("Python comparison benchmark", vec!["-I", "-B", "{harness}"])
                }
                "python-unittest" => (
                    "Python unittest",
                    vec![
                        "-I", "-B", "-m", "unittest", "discover", "-s", ".", "-t", ".", "-v",
                    ],
                ),
                "python-pytest" => (
                    "Python pytest",
                    vec![
                        "-I",
                        "-B",
                        "-m",
                        "pytest",
                        "-q",
                        "-o",
                        "cache_dir={artifacts}/pytest-cache",
                    ],
                ),
                _ => anyhow::bail!("Unknown Python recipe"),
            };
            let argv = std::iter::once(tools.interpreter.display().to_string())
                .chain(args.into_iter().map(str::to_string))
                .collect::<Vec<_>>();
            let read_roots = tools.read_roots();
            let identity = json!({"version":3,"argv":argv,"executable":file_hash(&tools.executable)?,"environment_metadata":tools.environment_fingerprint()?,"read_roots":read_roots,"runtime":file_hash(&self.runtime)?,"network":false,"inputs":"readonly snapshot","writes":"isolated artifacts","timeout_ms":120000,"plugin_autoload":false});
            let mut identity = identity;
            if id == "python-benchmark" {
                identity["harness_hash"] = json!(content_hash(include_bytes!("benchmark.py")));
                identity["module"] = json!("benchmarks/tandem_compare.py");
            }
            let mut permission_identity = identity.clone();
            permission_identity
                .as_object_mut()
                .unwrap()
                .remove("environment_metadata");
            return Ok(Recipe {
                id: id.into(),
                title: title.into(),
                argv,
                fingerprint: content_hash(identity.to_string().as_bytes()),
                grant_fingerprint: content_hash(permission_identity.to_string().as_bytes()),
                language: "python".into(),
                version: tools.label.clone(),
                read_roots,
            });
        }
        let tools = self
            .tools
            .as_ref()
            .context("Rust toolchain is unavailable")?;
        ensure!(
            self.clang.is_file() && self.sdk.is_some(),
            "Rust checks require the installed Apple linker/SDK"
        );
        let (title, args) = match id {
            "cargo-check" => (
                "Cargo check",
                vec![
                    "check",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--offline",
                    "--message-format=json",
                ],
            ),
            "cargo-test" => (
                "Cargo test",
                vec![
                    "test",
                    "--workspace",
                    "--locked",
                    "--offline",
                    "--message-format=json",
                ],
            ),
            _ => anyhow::bail!("Unknown validation recipe"),
        };
        let argv = std::iter::once(tools.cargo.display().to_string())
            .chain(args.into_iter().map(str::to_string))
            .collect::<Vec<_>>();
        let read_roots = vec![
            tools.toolchain.clone(),
            self.cargo_cache.join("registry"),
            self.cargo_cache.join("git"),
            PathBuf::from("/Applications/Xcode.app/Contents/Developer"),
            PathBuf::from("/Library/Developer"),
        ];
        let identity = json!({"version":2,"argv":argv,"cargo":file_hash(&tools.cargo)?,"rustc":file_hash(&tools.rustc)?,"runtime":file_hash(&self.runtime)?,
            "toolchain":tools.toolchain,"cache":self.cargo_cache,"clang":file_hash(&self.clang)?,"sdk":self.sdk,"network":false,"inputs":"readonly snapshot","writes":"isolated artifacts","timeout_ms":120000});
        Ok(Recipe {
            id: id.into(),
            title: title.into(),
            argv,
            fingerprint: content_hash(identity.to_string().as_bytes()),
            grant_fingerprint: content_hash(identity.to_string().as_bytes()),
            language: "rust".into(),
            version: tools.label.clone(),
            read_roots,
        })
    }
}

fn quote(path: &Path) -> Result<String> {
    Ok(serde_json::to_string(
        path.to_str().context("Non-UTF-8 validation path")?,
    )?)
}
fn configure(
    environment: &CheckEnvironment,
    project: &Project,
    snapshot: &MaterializedSnapshot,
    home: &Path,
    recipe: &Recipe,
) -> Result<()> {
    for trusted in [&environment.runtime, &environment.runs] {
        ensure!(
            !trusted.starts_with(&project.root) && !project.root.starts_with(trusted),
            "Choose a repository folder that does not overlap toolchain, cache or runtime directories"
        );
    }
    fs::create_dir_all(home)?;
    let mut config = String::from(
        "model_provider=\"openai\"\nforced_login_method=\"chatgpt\"\ncli_auth_credentials_store=\"ephemeral\"\napproval_policy=\"never\"\ndefault_permissions=\"validation\"\nproject_doc_max_bytes=0\nweb_search=\"disabled\"\n[memories]\ngenerate_memories=false\nuse_memories=false\n[analytics]\nenabled=false\n[features]\nshell_tool=false\nunified_exec=false\napply_patch_freeform=false\napps=false\nplugins=false\nbrowser_use=false\ncomputer_use=false\nremote_control=false\nmemories=false\nmemory_tool=false\nskip_host_skill_discovery=true\n[permissions.validation.filesystem]\n\":minimal\"=\"read\"\n",
    );
    config.push_str(&format!("{}=\"read\"\n", quote(&snapshot.input)?));
    if recipe.id == "python-benchmark" {
        config.push_str(&format!(
            "{}=\"read\"\n",
            quote(&home.join("benchmark.py"))?
        ));
    }
    for path in &recipe.read_roots {
        ensure!(
            !project.root.starts_with(path),
            "The selected tool environment must not include the source project"
        );
        ensure!(
            recipe.language == "python" || !path.starts_with(&project.root),
            "Rust validation inputs must not overlap the source project's tool/cache directories"
        );
        if path.exists() {
            config.push_str(&format!("{}=\"read\"\n", quote(path)?));
        }
    }
    config.push_str(&format!("{}=\"write\"\n", quote(&snapshot.artifacts)?));
    let denied = std::iter::once(project.root.clone())
        .chain(environment.denied_roots.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for root in denied {
        config.push_str(&format!("{}=\"deny\"\n", quote(&root)?));
    }
    config.push_str("[permissions.validation.network]\nenabled=false\n");
    fs::write(home.join("config.toml"), config)?;
    let cargo_home = snapshot.artifacts.join("cargo-home");
    fs::create_dir_all(&cargo_home)?;
    fs::create_dir_all(snapshot.artifacts.join("tmp"))?;
    #[cfg(unix)]
    for name in ["registry", "git"] {
        let source = environment.cargo_cache.join(name);
        if source.exists() {
            std::os::unix::fs::symlink(source, cargo_home.join(name))?;
        }
    }
    Ok(())
}

pub fn run_check(
    environment: &CheckEnvironment,
    project: &Project,
    overlays: &[Document],
    recipe: &Recipe,
    mut record: CheckRecord,
    cancel: &AtomicBool,
    mut observe: impl FnMut(&CheckRecord) -> Result<()>,
) -> Result<CheckRecord> {
    let result = (|| -> Result<()> {
        let prepared_recipe = environment.recipe(&recipe.id)?;
        ensure!(
            prepared_recipe.grant_fingerprint == recipe.grant_fingerprint,
            "The executable or execution scope changed; refresh tools and authorize its new envelope"
        );
        let recipe = &prepared_recipe;
        record.recipe_fingerprint = recipe.fingerprint.clone();
        record.sequence += 1;
        ensure!(
            file_hash(&environment.runtime)? == RUNTIME_HASH,
            "Validation runtime changed"
        );
        tandem_codex::verify_runtime(&environment.runtime)?;
        observe(&record)?;
        let directory = environment.runs.join(&record.id);
        let snapshot = materialize(project, overlays, &directory, cancel)?;
        record.snapshot = Some(snapshot.manifest.clone());
        record.command = recipe
            .argv
            .iter()
            .map(|s| {
                s.replace("{artifacts}", &snapshot.artifacts.display().to_string())
                    .replace(
                        "{harness}",
                        &directory
                            .join("codex-home/benchmark.py")
                            .display()
                            .to_string(),
                    )
            })
            .collect();
        record.executable_version = recipe.version.clone();
        record.sequence += 1;
        observe(&record)?;
        if recipe.language == "rust" {
            let tools = environment
                .tools
                .as_ref()
                .context("Rust toolchain unavailable")?;
            ensure!(
                snapshot.input.join("Cargo.toml").is_file(),
                "No Cargo.toml was captured; this recipe is not applicable"
            );
            ensure!(
                snapshot.input.join("Cargo.lock").is_file(),
                "No Cargo.lock was captured. Prepare dependencies explicitly before a locked/offline check"
            );
            if let Ok(text) = fs::read_to_string(snapshot.input.join("rust-toolchain.toml")) {
                let data: toml::Value = toml::from_str(&text).map_err(|_| {
                    anyhow::anyhow!("Captured rust-toolchain.toml is invalid; inspect it locally")
                })?;
                if let Some(channel) = data
                    .get("toolchain")
                    .and_then(|v| v.get("channel"))
                    .and_then(toml::Value::as_str)
                {
                    ensure!(
                        tools.label == channel || tools.label.starts_with(&format!("{channel}-")),
                        "The captured toolchain selection differs from the authorized executable"
                    );
                }
            }
        }
        let home = directory.join("codex-home");
        configure(environment, project, &snapshot, &home, recipe)?;
        if recipe.id == "python-benchmark" {
            ensure!(
                snapshot
                    .input
                    .join("benchmarks/tandem_compare.py")
                    .is_file(),
                "Create benchmarks/tandem_compare.py with cases(), baseline(case), candidate(case) first"
            );
            fs::write(home.join("benchmark.py"), include_bytes!("benchmark.py"))?;
        }
        if cancel.load(Ordering::Relaxed) {
            record.finish(
                CheckStatus::Cancelled,
                "Cancelled before starting a command",
            );
            return Ok(());
        }
        let mut client = Client::start(&environment.runtime, &home, &snapshot.input)?;
        let process = format!("check-{}", record.id);
        let env = if recipe.language == "rust" {
            let tools = environment.tools.as_ref().unwrap();
            json!({"CARGO_HOME":snapshot.artifacts.join("cargo-home"),"CARGO_TARGET_DIR":snapshot.artifacts.join("target"),"TMPDIR":snapshot.artifacts.join("tmp"),
                "RUSTC":tools.rustc,"RUSTDOC":tools.toolchain.join("bin/rustdoc"),"CARGO_NET_OFFLINE":"true","CARGO_TERM_COLOR":"never",
                "PATH":format!("{}:/usr/bin:/bin:/usr/sbin:/sbin",tools.toolchain.join("bin").display()),"SDKROOT":environment.sdk,"CC":environment.clang,"CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER":environment.clang})
        } else {
            json!({"TMPDIR":snapshot.artifacts.join("tmp"),"HOME":snapshot.artifacts,"PYTHONDONTWRITEBYTECODE":"1","PYTEST_DISABLE_PLUGIN_AUTOLOAD":"1","PATH":"/usr/bin:/bin:/usr/sbin:/sbin","TANDEM_ARTIFACTS":snapshot.artifacts})
        };
        let request=client.begin_request("command/exec",json!({"command":record.command,"cwd":snapshot.input,"permissionProfile":"validation","processId":process,
            "streamStdoutStderr":true,"timeoutMs":120000,"outputBytesCap":OUTPUT_LIMIT,
            "env":env}))?;
        record.status = CheckStatus::Running;
        record.detail = "Running captured inputs; original source is denied and selected tool environments are read-only".into();
        record.sequence += 1;
        observe(&record)?;
        let mut output = Vec::new();
        let mut stdout = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(140);
        let mut terminated = false;
        let mut published = Instant::now();
        let mut published_bytes = 0;
        let result = loop {
            ensure!(
                Instant::now() < deadline,
                "Validation runtime did not report a terminal result; outcome unknown"
            );
            if cancel.load(Ordering::Relaxed) && !record.cancel_requested {
                record.cancel_requested = true;
                record.detail =
                    "Stopping the command; completed work and artifacts are retained".into();
                record.sequence += 1;
                observe(&record)?;
                terminated = client
                    .request(
                        "command/exec/terminate",
                        json!({"processId":process}),
                        Duration::from_secs(10),
                    )
                    .is_ok();
            }
            if output.len() != published_bytes && published.elapsed() > Duration::from_millis(500) {
                record.output = display_output(&output, &snapshot.input);
                record.sequence += 1;
                observe(&record)?;
                published_bytes = output.len();
                published = Instant::now();
            }
            let Some(event) = client.poll_event(Duration::from_millis(100))? else {
                continue;
            };
            if event["id"].as_u64() == Some(request) {
                ensure!(
                    event.get("error").is_none(),
                    "Validation request failed; no successful result was recorded"
                );
                break event["result"].clone();
            }
            if event["method"] == "command/exec/outputDelta"
                && event["params"]["processId"] == process
            {
                let bytes = base64::engine::general_purpose::STANDARD.decode(
                    event["params"]["deltaBase64"]
                        .as_str()
                        .context("Missing output bytes")?,
                )?;
                let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
                output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
                if event["params"]["stream"] == "stdout" {
                    let remaining = OUTPUT_LIMIT.saturating_sub(stdout.len());
                    stdout.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
                }
                record.output_truncated |=
                    bytes.len() > remaining || event["params"]["capReached"] == true;
                if published.elapsed() > Duration::from_millis(500) {
                    record.output = display_output(&output, &snapshot.input);
                    record.sequence += 1;
                    observe(&record)?;
                    published_bytes = output.len();
                    published = Instant::now();
                }
            }
        };
        record.output = display_output(&output, &snapshot.input);
        record.exit_code = result["exitCode"].as_i64();
        record.diagnostics = cargo_diagnostics(&stdout, &snapshot.input);
        record.snapshot_unchanged = Some(snapshot_unchanged(&snapshot));
        let log = directory.join("output.txt");
        fs::write(&log, &record.output)?;
        record.log_path = Some(log.display().to_string());
        let empty_tests = recipe.language == "python"
            && (record.output.contains("Ran 0 tests") || record.output.contains("no tests ran"));
        let status = if record.snapshot_unchanged != Some(true) || empty_tests {
            CheckStatus::Unvalidated
        } else if record.exit_code == Some(0) {
            CheckStatus::Passed
        } else if terminated {
            CheckStatus::Cancelled
        } else if record.output.contains("test result: FAILED.")
            || record.diagnostics.iter().any(|d| d.level == "error")
        {
            CheckStatus::Failed
        } else if infrastructure_failure(&record.output) {
            CheckStatus::Unvalidated
        } else {
            CheckStatus::Failed
        };
        let detail = match status {
            CheckStatus::Passed => {
                if record.cancel_requested {
                    "Command completed before cancellation took effect"
                } else {
                    "Command exited successfully on the captured snapshot; acceptance criteria remain a separate decision"
                }
            }
            CheckStatus::Cancelled => "Command was interrupted; existing artifacts are retained",
            CheckStatus::Unvalidated => {
                "Inputs changed or required tooling/dependencies were unavailable; this is not a successful validation"
            }
            _ => "Command failed on the captured snapshot; inspect diagnostics and output",
        };
        if recipe.id == "python-benchmark" && status == CheckStatus::Passed {
            let text = std::str::from_utf8(&stdout).context("Benchmark output was not UTF-8")?;
            let rows = text
                .lines()
                .filter_map(|l| l.strip_prefix("TANDEM_BENCHMARK_V1="))
                .collect::<Vec<_>>();
            ensure!(
                rows.len() == 1 && !record.output_truncated,
                "Benchmark did not return one complete receipt"
            );
            let measurement: tandem_core::BenchmarkMeasurement = serde_json::from_str(rows[0])?;
            measurement.validate()?;
            record.benchmark = Some(measurement);
        }
        record.finish(status, detail);
        client.shutdown()?;
        Ok(())
    })();
    if let Err(error) = result {
        record.finish(
            if record.cancel_requested {
                CheckStatus::Interrupted
            } else if cancel.load(Ordering::Relaxed) {
                CheckStatus::Cancelled
            } else {
                CheckStatus::Unvalidated
            },
            error.to_string(),
        );
    }
    observe(&record)?;
    Ok(record)
}

fn infrastructure_failure(output: &str) -> bool {
    [
        "no matching package named",
        "failed to download",
        "lock file",
        "lockfile",
        "failed to get",
        "could not execute process",
        "command not found",
        "No module named",
        "No such file or directory",
        "Operation not permitted",
        "Permission denied",
    ]
    .iter()
    .any(|needle| output.contains(needle))
}
fn display_output(output: &[u8], input: &Path) -> String {
    let mut display = String::new();
    for line in String::from_utf8_lossy(output).lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            match value["reason"].as_str() {
                Some("compiler-message") => {
                    if let Some(rendered) = value["message"]["rendered"].as_str() {
                        display.push_str(rendered);
                        continue;
                    }
                }
                Some("compiler-artifact" | "build-script-executed") => continue,
                Some("build-finished") => {
                    display.push_str(if value["success"] == true {
                        "Build completed.\n"
                    } else {
                        "Build failed.\n"
                    });
                    continue;
                }
                _ => {}
            }
        }
        display.push_str(line);
        display.push('\n');
    }
    display.replace(&format!("{}/", input.display()), "")
}
fn cargo_diagnostics(stdout: &[u8], input: &Path) -> Vec<CheckDiagnostic> {
    let mut result = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-message" {
            continue;
        }
        let message = &value["message"];
        let Some(spans) = message["spans"].as_array() else {
            continue;
        };
        for span in spans.iter().filter(|span| span["is_primary"] == true) {
            let Some(file) = span["file_name"].as_str() else {
                continue;
            };
            let path = Path::new(file);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else if input.join(path).is_file() {
                input.join(path)
            } else {
                value["manifest_path"]
                    .as_str()
                    .and_then(|p| Path::new(p).parent())
                    .unwrap_or(input)
                    .join(path)
            };
            if let Some(path) = absolute
                .strip_prefix(input)
                .ok()
                .and_then(|path| normalized_relative(path).ok())
            {
                result.push(CheckDiagnostic {
                    path,
                    line: span["line_start"].as_u64().unwrap_or(1) as u32,
                    column: span["column_start"].as_u64().unwrap_or(1) as u32,
                    message: message["message"]
                        .as_str()
                        .unwrap_or("Compiler diagnostic")
                        .into(),
                    level: message["level"].as_str().unwrap_or("info").into(),
                });
            }
        }
    }
    result.truncate(200);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependency_metadata_changes_evidence_identity_without_expanding_the_grant() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("python");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        let executable = prefix.join("bin/python3");
        fs::write(&executable, "fixture executable identity").unwrap();
        let runtime = temp.path().join("codex");
        fs::write(&runtime, "fixture runtime identity").unwrap();
        let metadata = prefix.join("lib/python3.14/site-packages/example-1.dist-info/METADATA");
        fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        fs::write(&metadata, "Version: 1").unwrap();
        let python = PythonTools {
            interpreter: executable.clone(),
            executable,
            prefix: prefix.clone(),
            environment: prefix,
            node: None,
            server: None,
            label: "fixture Python".into(),
            fingerprint: "fixture".into(),
        };
        let env = CheckEnvironment {
            denied_roots: vec![],
            runtime,
            tools: None,
            python: Some(python),
            runs: temp.path().join("runs"),
            cargo_cache: temp.path().join("cargo"),
            clang: temp.path().join("clang"),
            sdk: None,
        };
        let before = env.recipe("python-unittest").unwrap();
        fs::write(metadata, "Version: 2").unwrap();
        let after = env.recipe("python-unittest").unwrap();
        assert_eq!(before.grant_fingerprint, after.grant_fingerprint);
        assert_ne!(before.fingerprint, after.fingerprint);
        assert_eq!(before.argv, after.argv);
    }
}
