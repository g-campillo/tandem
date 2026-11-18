//! Live JavaScript and TypeScript check on a small project with JSX and an unsaved edit, served by
//! whichever TypeScript discovery picks: the global one, since the fixture has no node_modules.
//! When `TANDEM_TS6_PROJECT` names a project with TypeScript 6 or older in its node_modules, the
//! same checks then run through typescript-language-server with that TypeScript. Prints
//! `skipped: ...` and succeeds when no TypeScript is installed.
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

/// Sends the request every 250 ms for up to 30 s, while the server loads the project, until
/// `accept` holds for the answer.
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

/// The fixture's source files and the texts Tandem holds for them.
struct Fixture {
    greet: PathBuf,
    app: PathBuf,
    plain: PathBuf,
}

const GREET_TEXT: &str = "export function greet(name: string): string { return \"hi \" + name; }\n";
const APP_TEXT: &str = "import { greet } from \"./greet\";\nexport const view = <p>{greet(\"x\")}</p>;\nconst n: number = greet(\"y\");\n";
const PLAIN_TEXT: &str = "export const answer=42;\nfunction f(){\nreturn 1}\n";

/// Writes the fixture into a fresh `project`. `greet.ts` on disk is older than its unsaved text.
fn write_fixture(project: &Path) -> Result<Fixture> {
    let _ = std::fs::remove_dir_all(project);
    let src = project.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        project.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"jsx":"preserve","noEmit":true},"include":["src"]}"#,
    )?;
    std::fs::write(
        src.join("jsx.d.ts"),
        "declare namespace JSX { interface IntrinsicElements { [name: string]: unknown } }",
    )?;
    let fixture = Fixture {
        greet: src.join("greet.ts"),
        app: src.join("app.tsx"),
        plain: src.join("plain.js"),
    };
    std::fs::write(&fixture.greet, "export const old = 1;\n")?;
    std::fs::write(&fixture.app, APP_TEXT)?;
    std::fs::write(&fixture.plain, PLAIN_TEXT)?;
    Ok(fixture)
}

/// Starts the server discovery picked for `project` and runs every check against the fixture.
async fn check(
    server: &tandem_core::ServerTools,
    project: &Path,
    cache: &Path,
    fixture: &Fixture,
) -> Result<()> {
    let manager = LanguageManager::start(server_config(server, project, cache));
    let document = |path: &Path, text: &str| LanguageDocument {
        path: path.to_path_buf(),
        instance: "fixture".into(),
        text: Arc::from(text),
    };
    manager.set_documents(vec![
        document(&fixture.greet, GREET_TEXT),
        document(&fixture.app, APP_TEXT),
        document(&fixture.plain, PLAIN_TEXT),
    ]);
    for _ in 0..300 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(manager.health().state == "ready", "{:?}", manager.health());

    let at_greet = position_params(APP_TEXT, APP_TEXT.find("greet(").unwrap() + 2)?;
    eventually(
        &manager,
        &fixture.app,
        APP_TEXT,
        "textDocument/definition",
        at_greet.clone(),
        |value| value.to_string().contains("greet.ts"),
    )
    .await?;
    eventually(
        &manager,
        &fixture.app,
        APP_TEXT,
        "textDocument/hover",
        at_greet,
        // tsserver 6 and older show an imported function as its alias.
        |value| {
            let hover = value.to_string();
            hover.contains("function greet") || hover.contains("(alias) greet(name: string)")
        },
    )
    .await?;
    let diagnostics = eventually(
        &manager,
        &fixture.app,
        APP_TEXT,
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
    let jsx = diagnostic_items(diagnostics)?
        .into_iter()
        .filter(|item| item.message.contains("JSX element") || item.message.contains("--jsx"))
        .map(|item| item.message)
        .collect::<Vec<_>>();
    ensure!(jsx.is_empty(), "JSX in app.tsx was rejected: {jsx:?}");
    eventually(
        &manager,
        &fixture.greet,
        GREET_TEXT,
        "textDocument/documentSymbol",
        json!({}),
        |value| value.to_string().contains("greet"),
    )
    .await?;
    eventually(
        &manager,
        &fixture.plain,
        PLAIN_TEXT,
        "textDocument/formatting",
        json!({"options":{"tabSize":2,"insertSpaces":true}}),
        |value| value.as_array().is_some_and(|edits| !edits.is_empty()),
    )
    .await?;

    let lines = APP_TEXT.lines().count();
    let hints = manager
        .request(
            "textDocument/inlayHint",
            &fixture.app,
            "fixture",
            APP_TEXT,
            json!({"range":{"start":{"line":0,"character":0},"end":{"line":lines,"character":0}}}),
        )
        .await
        .map(|reply| reply.value);
    match hints {
        Ok(Value::Array(hints)) if !hints.is_empty() => {
            let labels = hints.iter().map(hint_label).collect::<Vec<_>>();
            println!("inlay hints: {}", labels.join(" | "));
        }
        Ok(_) => println!("note: {} returned no inlay hints", server.name),
        Err(error) => println!("note: {} refused inlay hints: {error:#}", server.name),
    }
    manager.stop();
    Ok(())
}

/// An inlay hint's text: its label string, or its label parts joined.
fn hint_label(hint: &Value) -> String {
    match &hint["label"] {
        Value::String(label) => label.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["value"].as_str())
            .collect(),
        other => other.to_string(),
    }
}

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/typescript-language");
    let roots = ToolRoots::system()?;
    let project = root.join("project");
    let fixture = write_fixture(&project)?;
    let tools = match discover_kind(ToolKind::TypeScript, &project, &roots)
        .context("TypeScript is not served through the tool registry")?
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
        .context("No TypeScript server was found")?;
    println!("Using {} ({})", tools.label, server.name);
    ensure!(
        server.title.ends_with("(global)") || server.title.contains("(global) via"),
        "The fixture has no node_modules, yet discovery chose {}",
        server.title
    );
    check(
        server,
        &project,
        &root.join("cache").join(&server.name),
        &fixture,
    )
    .await?;
    println!("typescript language check passed");

    let Some(ts6) = std::env::var_os("TANDEM_TS6_PROJECT").map(PathBuf::from) else {
        println!(
            "skipped: typescript-language-server path needs a project with TypeScript 6 or older"
        );
        return Ok(());
    };
    let modules = ts6.join("node_modules");
    ensure!(
        modules.join("typescript/lib/tsserver.js").is_file(),
        "TANDEM_TS6_PROJECT must name a project with TypeScript 6 or older in its node_modules"
    );
    // The same fixture, borrowing that project's node_modules; nothing is written into it.
    let project = root.join("ts6-project");
    let fixture = write_fixture(&project)?;
    std::os::unix::fs::symlink(&modules, project.join("node_modules"))?;
    let tools = match discover_kind(ToolKind::TypeScript, &project, &roots)
        .context("TypeScript is not served through the tool registry")?
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
        .context("No TypeScript server was found")?;
    println!("Using {} ({})", tools.label, server.name);
    ensure!(
        server.name == "typescript-language-server",
        "TypeScript 6 or older should run through typescript-language-server, not {}",
        server.name
    );
    check(
        server,
        &project,
        &root.join("cache").join(&server.name),
        &fixture,
    )
    .await?;
    println!("typescript-language-server check passed");
    Ok(())
}
