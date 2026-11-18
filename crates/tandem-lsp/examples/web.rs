//! Live HTML, CSS and JSON check with vscode-langservers-extracted's three servers on a small
//! project: HTML with an embedded style, SCSS, JSON with a trailing comma and a tsconfig.json with a
//! comment, which opens as JSON with Comments. Prints `skipped: ...` and succeeds when the package
//! is not installed.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tandem_core::{ToolKind, ToolRoots, discover_kind};
use tandem_lsp::*;

fn main() -> Result<()> {
    smol::block_on(run())
}

/// Sends the request every 250 ms for up to 30 s, while the server starts, until `accept` holds
/// for the answer.
async fn eventually(
    manager: &LanguageManager,
    path: &Path,
    text: &str,
    method: &str,
    params: Value,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let last = match manager
            .request(method, path, "fixture", text, params.clone())
            .await
        {
            Ok(reply) if accept(&reply.value) => return Ok(reply.value),
            Ok(reply) => reply.value.to_string(),
            Err(error) => format!("{error:#}"),
        };
        ensure!(
            Instant::now() < deadline,
            "{method} on {} gave no usable answer within 30 s; last: {last}",
            path.display()
        );
        smol::Timer::after(Duration::from_millis(250)).await;
    }
}

const HTML_TEXT: &str = "<p class=\"a\">Hi</p><style>p { color: red }</style>";
const SCSS_TEXT: &str = "$c: red;\n.a { color: $c; }\n";
const PACKAGE_TEXT: &str = "{\"name\": \"x\",}";
const TSCONFIG_TEXT: &str = "{ // comment\n \"compilerOptions\": {} }";

/// Starts the server that answers for `language`, with `documents` open, and waits until it is
/// ready.
async fn start(
    tools: &tandem_core::KindTools,
    language: &str,
    project: &Path,
    cache: &Path,
    documents: &[(&Path, &str)],
) -> Result<LanguageManager> {
    let server = tools
        .servers
        .iter()
        .find(|server| server.languages.iter().any(|id| id == language))
        .with_context(|| format!("No server answers for {language}"))?;
    let manager = LanguageManager::start(server_config(server, project, &cache.join(&server.name)));
    manager.set_documents(
        documents
            .iter()
            .map(|(path, text)| LanguageDocument {
                path: path.to_path_buf(),
                instance: "fixture".into(),
                text: Arc::from(*text),
            })
            .collect(),
    );
    for _ in 0..300 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(
        manager.health().state == "ready",
        "{}: {:?}",
        server.name,
        manager.health()
    );
    Ok(manager)
}

/// The labels of a completion reply: a list of items, or a CompletionList.
fn completion_labels(value: &Value) -> Vec<String> {
    value
        .as_array()
        .or_else(|| value["items"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| item["label"].as_str().map(String::from))
        .collect()
}

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/web-language");
    let project = root.join("project");
    let cache = root.join("cache");

    let tools = match discover_kind(ToolKind::Web, &project, &ToolRoots::system()?)
        .context("HTML, CSS and JSON are not served through the tool registry")?
    {
        Ok(tools) => tools,
        Err(missing) => {
            println!("skipped: {}", missing.message);
            return Ok(());
        }
    };

    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project)?;
    let file = |name: &str, text: &str| -> Result<PathBuf> {
        let path = project.join(name);
        std::fs::write(&path, text)?;
        Ok(path)
    };
    let html = file("index.html", HTML_TEXT)?;
    let scss = file("style.scss", SCSS_TEXT)?;
    let package = file("package.json", PACKAGE_TEXT)?;
    let tsconfig = file("tsconfig.json", TSCONFIG_TEXT)?;
    ensure!(
        tandem_core::languages::language_id_for_path(&tsconfig.to_string_lossy()) == "jsonc",
        "tsconfig.json should open as JSON with Comments"
    );
    println!("Using {}", tools.label);

    let css = start(&tools, "scss", &project, &cache, &[(&scss, SCSS_TEXT)]).await?;
    let typed = ".a { col";
    let after_col = position_params(SCSS_TEXT, SCSS_TEXT.find(typed).unwrap() + typed.len())?;
    eventually(
        &css,
        &scss,
        SCSS_TEXT,
        "textDocument/completion",
        after_col,
        |value| {
            completion_labels(value)
                .iter()
                .any(|label| label == "color")
        },
    )
    .await?;
    eventually(
        &css,
        &scss,
        SCSS_TEXT,
        "textDocument/formatting",
        json!({"options":{"tabSize":2,"insertSpaces":true}}),
        Value::is_array,
    )
    .await?;
    css.stop();

    let json_server = start(
        &tools,
        "json",
        &project,
        &cache,
        &[(&package, PACKAGE_TEXT), (&tsconfig, TSCONFIG_TEXT)],
    )
    .await?;
    eventually(
        &json_server,
        &package,
        PACKAGE_TEXT,
        "textDocument/diagnostic",
        json!({}),
        |value| diagnostic_items(value.clone()).is_ok_and(|items| !items.is_empty()),
    )
    .await?;
    let tsconfig_diagnostics = eventually(
        &json_server,
        &tsconfig,
        TSCONFIG_TEXT,
        "textDocument/diagnostic",
        json!({}),
        |value| diagnostic_items(value.clone()).is_ok(),
    )
    .await?;
    let comments = diagnostic_items(tsconfig_diagnostics)?
        .into_iter()
        .map(|item| item.message)
        .filter(|message| message.to_lowercase().contains("comment"))
        .collect::<Vec<_>>();
    ensure!(
        comments.is_empty(),
        "tsconfig.json, opened as jsonc, still reports comments: {comments:?}"
    );
    json_server.stop();

    let html_server = start(&tools, "html", &project, &cache, &[(&html, HTML_TEXT)]).await?;
    eventually(
        &html_server,
        &html,
        HTML_TEXT,
        "textDocument/documentSymbol",
        json!({}),
        |value| value.as_array().is_some_and(|symbols| !symbols.is_empty()),
    )
    .await?;
    html_server.stop();

    println!("web language check passed");
    Ok(())
}
