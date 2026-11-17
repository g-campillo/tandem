//! Live check of the IDE features against the installed rust-analyzer and Pyright.
//! Run from the repository root:
//! `cargo run -p tandem-lsp --example features [rust|python]`.
use anyhow::{Context, Result, bail, ensure};
use lsp_types::{CompletionItem, InsertTextFormat};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, path::Path, sync::Arc, time::Duration};
use tandem_core::edit::indent::language_default;
use tandem_lsp::features::capabilities::ServerFeatures;
use tandem_lsp::features::completion::{
    accept_item, completion_items, completion_params, word_start,
};
use tandem_lsp::features::signature::{signature_help, signature_params, signature_view};
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
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (rust, python) = match arguments.as_slice() {
        [] => (true, true),
        [only] if only == "rust" => (true, false),
        [only] if only == "python" => (false, true),
        other => bail!(
            "Unknown arguments {}; pass rust, python or nothing",
            other.join(" ")
        ),
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
    let (manager, mut docs) = match started {
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
    checks.record(
        "rust completion_snippet",
        completion_snippet(&manager, &mut docs, &lib).await,
    );
    checks.record(
        "rust completion_autoimport",
        completion_autoimport(&manager, &mut docs, &lib).await,
    );
    let probe = format!("{RUST_LIB}\npub fn probe() -> String {{\n    describe(1.0, )\n}}\n");
    checks.record(
        "rust signature",
        signature(&manager, &mut docs, &lib, &probe).await,
    );
    manager.stop();
}

/// In `text`, the parameter hints right after `describe(1.0, ` describe `describe` with
/// its `label` parameter active.
async fn signature(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    file: &Path,
    text: &str,
) -> Result<()> {
    set_text(manager, docs, file, text);
    let cursor = text.rfind("describe(1.0, ").unwrap() + "describe(1.0, ".len();
    let params = signature_params(text, cursor, None, false, None)?;
    let reply = retry(|| {
        let params = params.clone();
        async move {
            manager
                .request("textDocument/signatureHelp", file, "fixture", text, params)
                .await
        }
    })
    .await?;
    let help = signature_help(reply.value)?.context("the server described no call")?;
    let view = signature_view(&help, None).context("the reply has no signature")?;
    ensure!(
        view.label.contains("label"),
        "the signature {:?} has no label parameter",
        view.label
    );
    let active = view
        .active
        .clone()
        .with_context(|| format!("no parameter of {:?} is active", view.label))?;
    ensure!(
        view.label[active.clone()].starts_with("label"),
        "the active parameter of {:?} is {:?}, not label",
        view.label,
        &view.label[active]
    );
    Ok(())
}

/// Completing `total_ar` offers `total_area` as a snippet, and accepting it inserts the
/// call with the cursor inside its parentheses.
async fn completion_snippet(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    lib: &Path,
) -> Result<()> {
    let text = format!("{RUST_LIB}\npub fn probe() -> f64 {{\n    total_ar\n}}\n");
    set_text(manager, docs, lib, &text);
    let cursor = text.rfind("total_ar").unwrap() + "total_ar".len();
    let items = completion_until(manager, lib, &text, cursor, |item| {
        item.label.starts_with("total_area")
    })
    .await?;
    let item = items
        .iter()
        .find(|item| item.label.starts_with("total_area"))
        .context("rust-analyzer offered no total_area completion")?;
    ensure!(
        item.insert_text_format == Some(InsertTextFormat::SNIPPET),
        "the total_area completion is not a snippet: {:?}",
        item.insert_text_format
    );
    let plan = accept_item(
        &text,
        cursor,
        cursor,
        word_start(&text, cursor, "rust"),
        item,
        &HashMap::new(),
        language_default("rust"),
    )?;
    let new = plan.edit.apply(&text);
    let open = new
        .rfind("total_area(")
        .context("accepting total_area inserted no call")?
        + "total_area".len();
    let close = open + new[open..].find(')').context("the call is not closed")?;
    ensure!(
        open < plan.selection.start && plan.selection.end <= close,
        "the selection {:?} is not inside the parentheses at {open}..{close} of {:?}",
        plan.selection,
        &new[open..=close]
    );
    Ok(())
}

/// Completing `HashMa` offers `HashMap` from std; once resolved it carries the import,
/// and accepting it adds the `use` line along with the name.
async fn completion_autoimport(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    lib: &Path,
) -> Result<()> {
    let text = format!("{RUST_LIB}\npub fn probe() {{\n    let map = HashMa\n}}\n");
    set_text(manager, docs, lib, &text);
    let cursor = text.rfind("HashMa").unwrap() + "HashMa".len();
    let is_import =
        |item: &CompletionItem| item.label.starts_with("HashMap") && item.data.is_some();
    let items = completion_until(manager, lib, &text, cursor, is_import).await?;
    let item = items
        .into_iter()
        .find(is_import)
        .context("rust-analyzer offered no HashMap import completion")?;
    let resolved = manager
        .request(
            "completionItem/resolve",
            lib,
            "fixture",
            &text,
            serde_json::to_value(&item)?,
        )
        .await?;
    let resolved: CompletionItem = serde_json::from_value(resolved.value)?;
    ensure!(
        resolved
            .additional_text_edits
            .as_ref()
            .is_some_and(|edits| !edits.is_empty()),
        "the resolved HashMap completion has no additional edits"
    );
    let plan = accept_item(
        &text,
        cursor,
        cursor,
        word_start(&text, cursor, "rust"),
        &resolved,
        &HashMap::new(),
        language_default("rust"),
    )?;
    let new = plan.edit.apply(&text);
    ensure!(
        new.contains("use std::collections::HashMap;") && new.contains("let map = HashMap"),
        "accepting HashMap gave:\n{new}"
    );
    Ok(())
}

/// The completion items at `cursor`, asked for until one of them satisfies `wanted`.
async fn completion_until(
    manager: &LanguageManager,
    file: &Path,
    text: &str,
    cursor: usize,
    wanted: impl Fn(&CompletionItem) -> bool,
) -> Result<Vec<CompletionItem>> {
    let params = completion_params(text, cursor, None, &[], false)?;
    let wanted = &wanted;
    let reply = retry(|| {
        let params = params.clone();
        async move {
            let mut reply = manager
                .request("textDocument/completion", file, "fixture", text, params)
                .await?;
            // A reply without the wanted item counts as no answer, so it is asked again.
            if !completion_items(reply.value.clone())?.0.iter().any(wanted) {
                reply.value = Value::Null;
            }
            Ok(reply)
        }
    })
    .await?;
    Ok(completion_items(reply.value)?.0)
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
    let (manager, mut docs) = match started {
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
    let probe = format!("{PYTHON_MAIN}\nfrom shapes import describe\n\nprint(describe(1.0, ))\n");
    checks.record(
        "python signature",
        signature(&manager, &mut docs, &main, &probe).await,
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
