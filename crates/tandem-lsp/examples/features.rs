//! Live check of the IDE features against the installed rust-analyzer and Pyright.
//! Run from the repository root: `cargo run -p tandem-lsp --example features [rust|python]`.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{future::Future, path::Path, sync::Arc, time::Duration};
use tandem_lsp::features::capabilities::ServerFeatures;
use tandem_lsp::*;

const RUST_LIB: &str = r#"pub trait Shape {
    fn area(&self) -> f64;
}

pub struct Square {
    pub side: f64,
}

impl Shape for Square {
    fn area(&self) -> f64 {
        self.side * self.side
    }
}

pub fn total_area(shapes: &[Square]) -> f64 {
    let mut total = 0.0;
    for shape in shapes {
        total += shape.area();
    }
    total
}

pub fn describe(side: f64, label: &str) -> String {
    format!("{label}: {}", total_area(&[Square { side }]))
}
"#;

const PYTHON_SHAPES: &str = r#"class Square:
    def __init__(self, side: float) -> None:
        self.side = side

    def area(self) -> float:
        return self.side * self.side


def total_area(shapes: list[Square]) -> float:
    total = 0.0
    for shape in shapes:
        total += shape.area()
    return total


def describe(side: float, label: str) -> str:
    return f"{label}: {total_area([Square(side)])}"
"#;

const PYTHON_MAIN: &str = "import sys\nimport os\n\nfrom shapes import Square, total_area\n\nprint(os.name, sys.argv, total_area([Square(2.0)]))\n";

fn main() -> Result<()> {
    smol::block_on(run())
}

async fn run() -> Result<()> {
    let (rust, python) = match std::env::args().nth(1).as_deref() {
        None => (true, true),
        Some("rust") => (true, false),
        Some("python") => (false, true),
        Some(other) => bail!("Unknown argument {other}; pass rust, python or nothing"),
    };
    let root = std::env::current_dir()?.join(".tandem-local/ide-features");
    let mut checks = Checks::default();
    if rust {
        rust_checks(&root.join("rust"), &mut checks).await;
    }
    if python {
        python_checks(&root.join("python"), &mut checks).await;
    }
    ensure!(
        checks.failed == 0,
        "{} feature check(s) failed",
        checks.failed
    );
    println!("All feature checks passed");
    Ok(())
}

/// Prints one line per check and counts the failures.
#[derive(Default)]
struct Checks {
    failed: usize,
}

impl Checks {
    fn record(&mut self, name: &str, result: Result<()>) {
        match result {
            Ok(()) => println!("ok {name}"),
            Err(error) => {
                self.failed += 1;
                println!("FAILED {name}: {error:#}");
            }
        }
    }
}

async fn rust_checks(project: &Path, checks: &mut Checks) {
    let lib = project.join("src/lib.rs");
    let started = async {
        write_rust_fixture(project)?;
        start(rust_config(project)?, &[(&lib, RUST_LIB)]).await
    }
    .await;
    let (manager, _docs) = match started {
        Ok(started) => started,
        Err(error) => return checks.record("rust start", Err(error)),
    };
    let features = manager.capabilities();
    checks.record(
        "rust capabilities",
        require_providers(
            "rust",
            &features,
            &[
                "completionProvider",
                "renameProvider",
                "codeActionProvider",
                "documentHighlightProvider",
                "signatureHelpProvider",
                "inlayHintProvider",
            ],
            &[
                (
                    "completionProvider.resolveProvider",
                    features.completion_resolve(),
                ),
                ("renameProvider.prepareProvider", features.rename_prepare()),
            ],
        ),
    );
    manager.stop();
}

async fn python_checks(project: &Path, checks: &mut Checks) {
    let shapes = project.join("shapes.py");
    let main = project.join("main.py");
    let started = async {
        write_python_fixture(project)?;
        start(
            python_config(project)?,
            &[(&shapes, PYTHON_SHAPES), (&main, PYTHON_MAIN)],
        )
        .await
    }
    .await;
    let (manager, _docs) = match started {
        Ok(started) => started,
        Err(error) => return checks.record("python start", Err(error)),
    };
    let features = manager.capabilities();
    checks.record(
        "python capabilities",
        require_providers(
            "python",
            &features,
            &[
                "completionProvider",
                "renameProvider",
                "signatureHelpProvider",
                "documentHighlightProvider",
                "workspaceSymbolProvider",
            ],
            &[],
        ),
    );
    manager.stop();
}

fn write_rust_fixture(project: &Path) -> Result<()> {
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"ide-features-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )?;
    std::fs::write(
        project.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"ide-features-fixture\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        project.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.100.0\"\n",
    )?;
    std::fs::write(project.join("src/lib.rs"), RUST_LIB)?;
    Ok(())
}

fn write_python_fixture(project: &Path) -> Result<()> {
    std::fs::create_dir_all(project)?;
    std::fs::write(project.join("shapes.py"), PYTHON_SHAPES)?;
    std::fs::write(project.join("main.py"), PYTHON_MAIN)?;
    Ok(())
}

/// rust-analyzer for `project`, found the way Tandem finds it.
fn rust_config(project: &Path) -> Result<ServerConfig> {
    let tools = tandem_core::RustTools::discover(project)?;
    Ok(ServerConfig {
        python: None,
        binary: tools.rust_analyzer,
        toolchain: tools.toolchain,
        project: project.into(),
        cache: project.join("target"),
    })
}

/// Pyright for `project`, found the way Tandem finds it.
fn python_config(project: &Path) -> Result<ServerConfig> {
    let tools = tandem_core::PythonTools::discover(project, None)?;
    Ok(ServerConfig {
        binary: tools.node.context("Node.js for Pyright was not found")?,
        toolchain: tools.prefix,
        project: project.into(),
        cache: project.join("cache"),
        python: Some(PythonServer {
            script: tools.server.context("Pyright was not found")?,
            interpreter: tools.interpreter,
        }),
    })
}

/// Starts the server, opens `files` as documents and waits until it is ready.
async fn start(
    config: ServerConfig,
    files: &[(&Path, &str)],
) -> Result<(LanguageManager, Vec<LanguageDocument>)> {
    let manager = LanguageManager::start(config);
    let mut docs = Vec::new();
    for (path, text) in files {
        set_text(&manager, &mut docs, path, text);
    }
    for _ in 0..100 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    let health = manager.health();
    if health.state != "ready" {
        manager.stop();
        bail!(
            "The server is not ready ({}: {})",
            health.state,
            health.detail
        );
    }
    Ok((manager, docs))
}

/// Prints the server's providers and requires every provider in `providers` and every
/// flag in `flags` (a name and whether the server sets it).
fn require_providers(
    language: &str,
    features: &ServerFeatures,
    providers: &[&str],
    flags: &[(&str, bool)],
) -> Result<()> {
    let offered = features
        .0
        .as_object()
        .map(|capabilities| {
            capabilities
                .keys()
                .filter(|key| key.ends_with("Provider"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    println!("{language} providers: {}", offered.join(", "));
    let missing = providers
        .iter()
        .filter(|provider| !features.has(provider))
        .chain(flags.iter().filter(|(_, set)| !set).map(|(name, _)| name))
        .copied()
        .collect::<Vec<_>>();
    ensure!(missing.is_empty(), "missing {}", missing.join(", "));
    Ok(())
}

/// Replaces the text of `path` among `docs` (adding it when missing) and sends the
/// documents to the server.
fn set_text(manager: &LanguageManager, docs: &mut Vec<LanguageDocument>, path: &Path, text: &str) {
    match docs.iter_mut().find(|doc| doc.path == path) {
        Some(doc) => doc.text = Arc::from(text),
        None => docs.push(LanguageDocument {
            path: path.into(),
            instance: "fixture".into(),
            text: Arc::from(text),
        }),
    }
    manager.set_documents(docs.clone());
}

/// Repeats `request` until the server answers with something other than `null` or `[]`
/// (up to 40 tries, 250 ms apart), and returns the last attempt.
#[allow(dead_code)] // The feature checks of later tasks use it.
async fn retry<F, R>(mut request: F) -> Result<LanguageReply>
where
    F: FnMut() -> R,
    R: Future<Output = Result<LanguageReply>>,
{
    let mut last = request().await;
    for _ in 1..40 {
        if last
            .as_ref()
            .is_ok_and(|reply| !reply.value.is_null() && reply.value != json!([]))
        {
            break;
        }
        smol::Timer::after(Duration::from_millis(250)).await;
        last = request().await;
    }
    last
}

/// Position parameters for `delta` bytes after the first `needle` in `text`.
#[allow(dead_code)] // The feature checks of later tasks use it.
fn at(text: &str, needle: &str, delta: usize) -> Value {
    position_params(text, text.find(needle).unwrap() + delta).unwrap()
}
