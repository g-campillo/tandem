//! Live Svelte check with svelte-language-server on a one-component project whose TypeScript
//! script has a type error. Prints `skipped: ...` and succeeds when the server is not installed.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tandem_core::{ToolKind, ToolRoots, discover_kind};
use tandem_lsp::*;

fn main() -> Result<()> {
    smol::block_on(run())
}

/// How long each check retries while the server loads the component and its TypeScript.
const PATIENCE: Duration = Duration::from_secs(60);

/// Sends the request every 500 ms for up to `PATIENCE`, until `accept` holds for the answer.
async fn eventually(
    manager: &LanguageManager,
    path: &Path,
    method: &str,
    params: Value,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let last = match manager
            .request(method, path, "fixture", APP_TEXT, params.clone())
            .await
        {
            Ok(reply) if accept(&reply.value) => return Ok(reply.value),
            Ok(reply) => reply.value.to_string(),
            Err(error) => format!("{error:#}"),
        };
        ensure!(
            Instant::now() < deadline,
            "{method} on {} gave no usable answer within {} s; last: {last}",
            path.display(),
            PATIENCE.as_secs()
        );
        smol::Timer::after(Duration::from_millis(500)).await;
    }
}

const APP_TEXT: &str = "<script lang=\"ts\">\n  let count: number = \"x\";\n</script>\n<button on:click={() => count++}>{count}</button>\n";

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/svelte-language");
    let project = root.join("project");
    let cache = root.join("cache");

    let tools = match discover_kind(ToolKind::Svelte, &project, &ToolRoots::system()?)
        .context("Svelte is not served through the tool registry")?
    {
        Ok(tools) => tools,
        Err(missing) => {
            println!("skipped: {}", missing.message);
            return Ok(());
        }
    };
    let server = tools
        .servers
        .first()
        .context("No Svelte server was found")?;
    println!("Using {}", tools.label);

    let _ = std::fs::remove_dir_all(&project);
    let src = project.join("src");
    std::fs::create_dir_all(&src)?;
    let app = src.join("App.svelte");
    std::fs::write(&app, APP_TEXT)?;

    let manager =
        LanguageManager::start(server_config(server, &project, &cache.join(&server.name)));
    manager.set_documents(vec![LanguageDocument {
        path: app.clone(),
        instance: "fixture".into(),
        text: Arc::from(APP_TEXT),
    }]);
    for _ in 0..300 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(manager.health().state == "ready", "{:?}", manager.health());

    // svelte-language-server pushes diagnostics without versions; Tandem answers with the latest
    // push once the server is quiet.
    eventually(
        &manager,
        &app,
        "textDocument/diagnostic",
        json!({}),
        |value| {
            diagnostic_items(value.clone()).is_ok_and(|items| {
                items
                    .iter()
                    .any(|item| item.message.contains("not assignable"))
            })
        },
    )
    .await?;
    let at_count = position_params(APP_TEXT, APP_TEXT.find("count").unwrap() + 2)?;
    eventually(&manager, &app, "textDocument/hover", at_count, |value| {
        !value.is_null()
    })
    .await?;
    eventually(
        &manager,
        &app,
        "textDocument/documentSymbol",
        json!({}),
        |value| value.as_array().is_some_and(|symbols| !symbols.is_empty()),
    )
    .await?;
    eventually(
        &manager,
        &app,
        "textDocument/formatting",
        json!({"options":{"tabSize":2,"insertSpaces":true}}),
        Value::is_array,
    )
    .await?;
    manager.stop();

    println!("svelte language check passed");
    Ok(())
}
