//! Live SourceKit-LSP check on a small Swift package with unsaved edits. Prints `skipped: ...`
//! and succeeds when SourceKit-LSP is not installed.
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

/// Sends the request every 500 ms for up to 60 s, while SourceKit-LSP loads the package, until
/// `accept` holds for the answer.
async fn eventually(
    manager: &LanguageManager,
    path: &Path,
    text: &str,
    method: &str,
    params: Value,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(60);
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
            "{method} on {} gave no usable answer within 60 s; last: {last}",
            path.display()
        );
        smol::Timer::after(Duration::from_millis(500)).await;
    }
}

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/swift-language");
    let project = root.join("project");
    let cache = root.join("cache");
    let _ = std::fs::remove_dir_all(&project);
    let sources = project.join("Sources/Demo");
    std::fs::create_dir_all(&sources)?;
    std::fs::write(
        project.join("Package.swift"),
        "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Demo\",\n    targets: [\n        .executableTarget(name: \"Demo\")\n    ]\n)\n",
    )?;
    let greeter = sources.join("Greeter.swift");
    let main = sources.join("main.swift");
    std::fs::write(&greeter, "func oldName() -> Int { 1 }\n")?;
    std::fs::write(&main, "print(1)\n")?;
    let greeter_text = "func greet(_ name: String) -> String { \"hi \" + name }\n";
    let main_text = "let message = greet(\"x\")\nlet n: Int = greet(\"y\")\nprint(message, n)\n";

    let tools = match discover_kind(ToolKind::Swift, &project, &ToolRoots::system()?)
        .context("Swift is not served through the tool registry")?
    {
        Ok(tools) => tools,
        Err(missing) => {
            println!("skipped: {}", missing.message);
            return Ok(());
        }
    };
    let server = tools.servers.first().context("No Swift server was found")?;

    println!("Using {}", tools.label);
    let manager = LanguageManager::start(server_config(server, &project, &cache));
    let mut docs = vec![
        LanguageDocument {
            path: greeter.clone(),
            instance: "fixture".into(),
            text: Arc::from(greeter_text),
        },
        LanguageDocument {
            path: main.clone(),
            instance: "fixture".into(),
            text: Arc::from(main_text),
        },
    ];
    manager.set_documents(docs.clone());
    for _ in 0..600 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(manager.health().state == "ready", "{:?}", manager.health());

    let at_greet = position_params(main_text, main_text.find("greet(").unwrap() + 2)?;
    eventually(
        &manager,
        &main,
        main_text,
        "textDocument/definition",
        at_greet.clone(),
        |value| value.to_string().contains("Greeter.swift"),
    )
    .await?;
    eventually(
        &manager,
        &main,
        main_text,
        "textDocument/hover",
        at_greet,
        |value| value.to_string().contains("func greet"),
    )
    .await?;
    eventually(
        &manager,
        &main,
        main_text,
        "textDocument/diagnostic",
        json!({}),
        |value| {
            diagnostic_items(value.clone()).is_ok_and(|items| {
                items
                    .iter()
                    .any(|item| item.message.to_lowercase().contains("cannot convert"))
            })
        },
    )
    .await?;
    eventually(
        &manager,
        &greeter,
        greeter_text,
        "textDocument/documentSymbol",
        json!({}),
        |value| value.to_string().contains("greet("),
    )
    .await?;
    let unformatted = "let   x=1\n";
    docs[1].text = Arc::from(unformatted);
    manager.set_documents(docs);
    eventually(
        &manager,
        &main,
        unformatted,
        "textDocument/formatting",
        json!({"options":{"tabSize":4,"insertSpaces":true}}),
        |value| value.as_array().is_some_and(|edits| !edits.is_empty()),
    )
    .await?;
    manager.stop();

    ensure!(
        !project.join(".build").exists(),
        "SourceKit-LSP wrote .build into the project"
    );
    println!("swift language check passed");
    Ok(())
}
