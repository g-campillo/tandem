//! Live jdtls check on a small Java project without build files. Prints `skipped: ...` and
//! succeeds when jdtls or a Java 21 runtime is not installed.
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

/// Sends the request every 500 ms for up to 90 s, while jdtls imports the project, until
/// `accept` holds for the answer.
async fn eventually(
    manager: &LanguageManager,
    path: &Path,
    text: &str,
    method: &str,
    params: Value,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(90);
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
            "{method} on {} gave no usable answer within 90 s; last: {last}",
            path.display()
        );
        smol::Timer::after(Duration::from_millis(500)).await;
    }
}

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/java-language");
    let project = root.join("project");
    let cache = root.join("cache");

    let tools = match discover_kind(ToolKind::Java, &project, &ToolRoots::system()?)
        .context("Java is not served through the tool registry")?
    {
        Ok(tools) => tools,
        Err(missing) => {
            println!("skipped: {}", missing.message);
            return Ok(());
        }
    };
    let server = tools.servers.first().context("No Java server was found")?;

    let _ = std::fs::remove_dir_all(&project);
    let sources = project.join("src/demo");
    std::fs::create_dir_all(&sources)?;
    let greeter = sources.join("Greeter.java");
    let main = sources.join("Main.java");
    let greeter_text = "package demo;\npublic class Greeter {\n    public static String greet(String name) { return \"hi \" + name; }\n}\n";
    let main_text = "package demo;\npublic class Main {\n    public static void main(String[] args) {\n        String s = Greeter.greet(\"x\");\n        int n = Greeter.greet(\"y\");\n    }\n}\n";
    std::fs::write(&greeter, greeter_text)?;
    std::fs::write(&main, main_text)?;

    println!("Using {}", tools.label);
    let manager = LanguageManager::start(server_config(server, &project, &cache));
    manager.set_documents(vec![
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
    ]);
    for _ in 0..900 {
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
        |value| value.to_string().contains("Greeter.java"),
    )
    .await?;
    eventually(
        &manager,
        &main,
        main_text,
        "textDocument/hover",
        at_greet,
        // jdtls 1.60 shows `String Greeter.greet(String name)`; others qualify it with `demo.`.
        |value| value.to_string().contains("Greeter.greet(String name)"),
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
                    .any(|item| item.message.contains("Type mismatch"))
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
        |value| value.to_string().contains("greet"),
    )
    .await?;
    eventually(
        &manager,
        &main,
        main_text,
        "textDocument/formatting",
        json!({"options":{"tabSize":4,"insertSpaces":true}}),
        Value::is_array,
    )
    .await?;
    manager.stop();

    for metadata in [".project", ".classpath", ".settings"] {
        ensure!(
            !project.join(metadata).exists(),
            "jdtls wrote {metadata} into the project"
        );
    }
    println!("java language check passed");
    Ok(())
}
