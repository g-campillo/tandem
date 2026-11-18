//! Live check of the IDE features against the installed rust-analyzer and Pyright.
//! Run from the repository root:
//! `cargo run -p tandem-lsp --example features [rust|python]`.
use anyhow::{Context, Result, bail, ensure};
use lsp_types::{CodeAction, CompletionItem, InsertTextFormat};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, path::Path, sync::Arc, time::Duration};
use tandem_core::edit::indent::language_default;
use tandem_lsp::features::actions::{
    ORGANIZE_IMPORTS, code_action_params, code_actions, organize_imports, pyright_organize_imports,
};
use tandem_lsp::features::capabilities::ServerFeatures;
use tandem_lsp::features::completion::{
    accept_item, completion_items, completion_params, word_start,
};
use tandem_lsp::features::edits::{command_edits, relative_edits, workspace_edit};
use tandem_lsp::features::highlights::document_highlights;
use tandem_lsp::features::inlay::{LineHint, eol_hints, hint_text, inlay_params};
use tandem_lsp::features::signature::{signature_help, signature_params, signature_view};
use tandem_lsp::features::symbols::workspace_symbols;
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
                "workspaceSymbolProvider",
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
    checks.record(
        "rust highlights",
        highlights(&manager, &mut docs, &lib).await,
    );
    let probe = format!(
        "{RUST_LIB}\npub fn probe() -> String {{\n    let s = describe(1.0, \"x\");\n    s\n}}\n"
    );
    checks.record("rust inlay", inlay(&manager, &mut docs, &lib, &probe).await);
    checks.record(
        "rust rename",
        rename(
            &manager,
            &mut docs,
            project,
            Rename {
                file: "src/lib.rs",
                needle: "fn total_area",
                delta: "fn ".len(),
                prepare: true,
                files: &[("src/lib.rs", RUST_LIB)],
            },
        )
        .await,
    );
    checks.record(
        "rust actions",
        rust_actions(&manager, &mut docs, project).await,
    );
    checks.record(
        "rust workspace_symbol",
        workspace_symbol(&manager, &mut docs, project, ("src/lib.rs", RUST_LIB)).await,
    );
    manager.stop();
}

/// On the `for shape in shapes` line rust-analyzer offers code actions; resolving the first
/// one that has `data` gives an edit to `src/lib.rs` that applies to its text.
async fn rust_actions(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    project: &Path,
) -> Result<()> {
    let lib = project.join("src/lib.rs");
    set_text(manager, docs, &lib, RUST_LIB);
    let cursor = RUST_LIB.find("for shape in shapes").unwrap();
    let params = code_action_params(RUST_LIB, cursor..cursor, vec![], None)?;
    let reply = retry(|| {
        let params = params.clone();
        let lib = &lib;
        async move {
            manager
                .request("textDocument/codeAction", lib, "fixture", RUST_LIB, params)
                .await
        }
    })
    .await?;
    let actions = code_actions(&reply.value);
    ensure!(!actions.is_empty(), "no code actions on the for line");
    println!(
        "rust actions: {}",
        actions
            .iter()
            .map(|action| action.title.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let action = actions
        .iter()
        .find(|action| action.data.is_some())
        .context("no code action on the for line can be resolved")?;
    let resolved = manager
        .request(
            "codeAction/resolve",
            &lib,
            "fixture",
            RUST_LIB,
            serde_json::to_value(action)?,
        )
        .await?;
    let resolved: CodeAction = serde_json::from_value(resolved.value)?;
    let edit = resolved
        .edit
        .with_context(|| format!("resolving {:?} gave no edit", action.title))?;
    let root = std::fs::canonicalize(project)?;
    let edited = relative_edits(&root, workspace_edit(&serde_json::to_value(edit)?)?)?;
    let (_, edits) = edited
        .iter()
        .find(|(path, _)| path == "src/lib.rs")
        .with_context(|| format!("{:?} does not edit src/lib.rs", action.title))?;
    let changed = apply_text_edits(RUST_LIB, edits)?;
    ensure!(
        changed != RUST_LIB,
        "{:?} leaves src/lib.rs as it is",
        action.title
    );
    Ok(())
}

/// A rename of `total_area` to `sum_area` to check: where it is asked for (`delta` bytes
/// into the first `needle` of `file`), whether to ask `textDocument/prepareRename` first,
/// and every file the rename must edit (project-relative) with its text.
struct Rename<'a> {
    file: &'a str,
    needle: &'a str,
    delta: usize,
    prepare: bool,
    files: &'a [(&'a str, &'a str)],
}

/// Renaming `total_area` to `sum_area` gives a workspace edit that stays inside the
/// project and edits exactly `check.files`, leaving no `total_area` in any of them. With
/// `prepare`, the server first says the symbol can be renamed.
async fn rename(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    project: &Path,
    check: Rename<'_>,
) -> Result<()> {
    for (relative, text) in check.files {
        set_text(manager, docs, &project.join(relative), text);
    }
    let file = project.join(check.file);
    let text = check
        .files
        .iter()
        .find(|(relative, _)| *relative == check.file)
        .map(|(_, text)| *text)
        .context("the renamed file is not among the files to check")?;
    let mut params = at(text, check.needle, check.delta);
    if check.prepare {
        let prepared = retry(|| {
            let params = params.clone();
            let file = &file;
            async move {
                manager
                    .request("textDocument/prepareRename", file, "fixture", text, params)
                    .await
            }
        })
        .await?;
        ensure!(
            !prepared.value.is_null(),
            "the server says total_area can't be renamed"
        );
    }
    params["newName"] = json!("sum_area");
    let reply = retry(|| {
        let params = params.clone();
        let file = &file;
        async move {
            manager
                .request("textDocument/rename", file, "fixture", text, params)
                .await
        }
    })
    .await?;
    // Tandem's project root is canonical, as `Project::open` makes it.
    let root = std::fs::canonicalize(project)?;
    let edited = relative_edits(&root, workspace_edit(&reply.value)?)?;
    let mut paths: Vec<&str> = edited.iter().map(|(path, _)| path.as_str()).collect();
    let mut expected: Vec<&str> = check.files.iter().map(|(path, _)| *path).collect();
    paths.sort();
    expected.sort();
    ensure!(
        paths == expected,
        "the rename edits {paths:?}, not {expected:?}"
    );
    for (path, edits) in &edited {
        let original = check
            .files
            .iter()
            .find(|(relative, _)| relative == path)
            .map(|(_, text)| *text)
            .context("an edited file is not among the files to check")?;
        let renamed = apply_text_edits(original, edits)?;
        ensure!(
            !renamed.contains("total_area") && renamed.contains("sum_area"),
            "renaming left {path} as:\n{renamed}"
        );
    }
    Ok(())
}

/// Searching the workspace for `Squa` finds a symbol named `Square` in `file` (its
/// project-relative path and text), on the line that declares it.
async fn workspace_symbol(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    project: &Path,
    (file, text): (&str, &str),
) -> Result<()> {
    let path = project.join(file);
    set_text(manager, docs, &path, text);
    // Tandem's project root is canonical, as `Project::open` makes it.
    let root = std::fs::canonicalize(project)?;
    let square = |value: &Value| {
        workspace_symbols(value).into_iter().find(|hit| {
            hit.name == "Square" && hit.path.strip_prefix(&root).ok() == Some(Path::new(file))
        })
    };
    let reply = retry(|| async {
        let mut reply = manager
            .request(
                "workspace/symbol",
                &path,
                "fixture",
                text,
                json!({"query": "Squa"}),
            )
            .await?;
        // Symbols of a project still loading count as no answer, so they are asked again.
        if square(&reply.value).is_none() {
            reply.value = Value::Null;
        }
        Ok(reply)
    })
    .await?;
    let hit = square(&reply.value)
        .with_context(|| format!("the server found no symbol named Square in {file}"))?;
    let line = text
        .lines()
        .nth(hit.line as usize)
        .with_context(|| format!("{file} has no line {}", hit.line + 1))?;
    ensure!(
        line.contains("Square"),
        "line {} of {file} does not declare Square: {line:?}",
        hit.line + 1
    );
    println!("{file}: Square on line {}: {}", hit.line + 1, line.trim());
    Ok(())
}

/// With the cursor on `total` in `let mut total`, the server highlights at least three
/// uses of `total`, one of them a write.
async fn highlights(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    lib: &Path,
) -> Result<()> {
    set_text(manager, docs, lib, RUST_LIB);
    let params = at(RUST_LIB, "let mut total", "let mut ".len());
    let reply = retry(|| {
        let params = params.clone();
        async move {
            manager
                .request(
                    "textDocument/documentHighlight",
                    lib,
                    "fixture",
                    RUST_LIB,
                    params,
                )
                .await
        }
    })
    .await?;
    let found = document_highlights(RUST_LIB, &reply.value);
    ensure!(
        found.len() >= 3,
        "{} highlight(s) of total, not at least 3: {found:?}",
        found.len()
    );
    ensure!(
        found.iter().any(|(_, write)| *write),
        "no highlight of total is a write: {found:?}"
    );
    ensure!(
        found
            .iter()
            .all(|(range, _)| &RUST_LIB[range.clone()] == "total"),
        "a highlight is not total: {found:?}"
    );
    Ok(())
}

/// In `text`, rust-analyzer's hints put `total: f64` after `let mut total = 0.0;`, and the
/// `let s = describe(1.0, "x");` line shows the names of `describe`'s parameters.
async fn inlay(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    file: &Path,
    text: &str,
) -> Result<()> {
    set_text(manager, docs, file, text);
    let call = text
        .lines()
        .position(|line| line.contains("let s = describe"))
        .context("the probe has no let s line")? as u32;
    let total = |hints: &[LineHint]| {
        hints
            .iter()
            .any(|hint| hint.labels.iter().any(|label| label == "total: f64"))
    };
    let shown = |hints: &[LineHint]| {
        hints
            .iter()
            .find(|hint| hint.line == call)
            .and_then(|hint| hint_text(hint, true, true))
    };
    let complete = |hints: &[LineHint]| {
        total(hints) && shown(hints).is_some_and(|shown| shown.contains("(side:, label:)"))
    };
    let reply = retry(|| async move {
        let mut reply = manager
            .request(
                "textDocument/inlayHint",
                file,
                "fixture",
                text,
                inlay_params(text),
            )
            .await?;
        // Hints of a project still loading count as no answer, so they are asked again.
        if !complete(&eol_hints(text, &reply.value)) {
            reply.value = Value::Null;
        }
        Ok(reply)
    })
    .await?;
    let hints = eol_hints(text, &reply.value);
    ensure!(total(&hints), "no line has the hint total: f64: {hints:?}");
    let shown = shown(&hints).with_context(|| format!("the let s line has no hints: {hints:?}"))?;
    ensure!(
        shown.contains("(side:, label:)"),
        "the let s line shows {shown:?}"
    );
    println!("rust inlay: {shown}");
    Ok(())
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
    checks.record(
        "python rename",
        rename(
            &manager,
            &mut docs,
            project,
            Rename {
                file: "shapes.py",
                needle: "def total_area",
                delta: "def ".len(),
                prepare: false,
                files: &[("main.py", PYTHON_MAIN), ("shapes.py", PYTHON_SHAPES)],
            },
        )
        .await,
    );
    let probe = format!("{PYTHON_MAIN}\nfrom shapes import describe\n\nprint(describe(1.0, ))\n");
    checks.record(
        "python signature",
        signature(&manager, &mut docs, &main, &probe).await,
    );
    checks.record(
        "python organize_imports",
        python_organize_imports(&manager, &mut docs, project).await,
    );
    checks.record(
        "python workspace_symbol",
        workspace_symbol(&manager, &mut docs, project, ("shapes.py", PYTHON_SHAPES)).await,
    );
    manager.stop();
}

/// Organize Imports on `main.py` as Tandem runs it for Pyright: Pyright's
/// organize-imports action when it offers one (its command must be one Pyright lists),
/// else Pyright's own organize-imports command, which Tandem runs by name for Pyright
/// only (Pyright 1.1.414 offers no such action and lists no commands). The edits the
/// command asks for change only `main.py`, and sort its imports.
async fn python_organize_imports(
    manager: &LanguageManager,
    docs: &mut Vec<LanguageDocument>,
    project: &Path,
) -> Result<()> {
    let main = project.join("main.py");
    set_text(manager, docs, &main, PYTHON_MAIN);
    let params = code_action_params(
        PYTHON_MAIN,
        0..PYTHON_MAIN.len(),
        vec![],
        Some(&[ORGANIZE_IMPORTS]),
    )?;
    let reply = manager
        .request(
            "textDocument/codeAction",
            &main,
            "fixture",
            PYTHON_MAIN,
            params,
        )
        .await?;
    ensure!(
        reply.value.is_null() || reply.value.is_array(),
        "the code action reply is not a list: {}",
        reply.value
    );
    let command = match organize_imports(&code_actions(&reply.value)) {
        Some(action) => {
            let command = action
                .command
                .context("the organize-imports action runs no command")?;
            let commands = manager.capabilities().commands();
            ensure!(
                commands.contains(&command.command),
                "Pyright does not list {} among its commands {commands:?}",
                command.command
            );
            command
        }
        None => pyright_organize_imports(&file_uri(&main)?),
    };
    let reply = manager
        .request(
            "workspace/executeCommand",
            &main,
            "fixture",
            PYTHON_MAIN,
            json!({"command": command.command, "arguments": command.arguments}),
        )
        .await?;
    let files = command_edits(&reply.value, &main)?;
    let [file] = files.as_slice() else {
        bail!(
            "organize imports does not edit only main.py: {}",
            reply.value
        );
    };
    ensure!(
        file.path == main,
        "organize imports edits {} instead of main.py",
        file.path.display()
    );
    let organized = apply_text_edits(PYTHON_MAIN, &file.edits)?;
    ensure!(
        organized.starts_with("import os\nimport sys"),
        "organize imports gave:\n{organized}"
    );
    Ok(())
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
fn at(text: &str, needle: &str, delta: usize) -> Value {
    position_params(text, text.find(needle).unwrap() + delta).unwrap()
}
