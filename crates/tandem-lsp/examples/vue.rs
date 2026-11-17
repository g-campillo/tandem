//! Live Vue check with @vue/language-server and its typescript-language-server companion, on a
//! one-component fixture. Vue's TypeScript plugin needs a project TypeScript 6 or older, so the
//! fixture borrows the node_modules of the Vue project `TANDEM_VUE_PROJECT` names; nothing is
//! written into that project. Prints `skipped: ...` and succeeds when a server is missing or the
//! variable is unset.
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

/// How long each check retries while both servers load the project.
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

const APP_TEXT: &str = "<script setup lang=\"ts\">\nimport { ref } from \"vue\";\nconst count = ref(1);\nconst label: number = \"x\";\n</script>\n<template>\n  <\n  <p>{{ count }} {{ label }}</p>\n</template>\n";

/// The install command for the project's own TypeScript, as discovery reports it.
const PROJECT_INSTALL: &str = "npm install";

/// Writes the fixture into a fresh `project`, with `modules` linked as its node_modules.
fn write_fixture(project: &Path, modules: Option<&Path>) -> Result<PathBuf> {
    let _ = std::fs::remove_dir_all(project);
    let src = project.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        project.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"noEmit":true,"target":"ESNext","module":"ESNext","moduleResolution":"Bundler","jsx":"preserve"},"include":["src/**/*.ts","src/**/*.vue"]}"#,
    )?;
    let app = src.join("App.vue");
    std::fs::write(&app, APP_TEXT)?;
    if let Some(modules) = modules {
        std::os::unix::fs::symlink(modules, project.join("node_modules"))?;
    }
    Ok(app)
}

/// A completion reply's item labels, from a list or an array.
fn completion_labels(value: &Value) -> Vec<&str> {
    value
        .get("items")
        .unwrap_or(value)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["label"].as_str())
        .collect()
}

async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/vue-language");
    let project = root.join("project");
    let vue_project = std::env::var_os("TANDEM_VUE_PROJECT").map(PathBuf::from);
    let modules = vue_project.as_ref().map(|vue| vue.join("node_modules"));
    if let Some(modules) = &modules {
        ensure!(
            modules.is_dir(),
            "TANDEM_VUE_PROJECT must name a Vue project with node_modules installed"
        );
    }
    let app = write_fixture(&project, modules.as_deref())?;

    let tools = match discover_kind(ToolKind::Vue, &project, &ToolRoots::system()?)
        .context("Vue is not served through the tool registry")?
    {
        Ok(tools) => tools,
        // Both servers are installed; only a project to borrow TypeScript from is missing.
        Err(missing)
            if vue_project.is_none()
                && missing
                    .install
                    .iter()
                    .all(|command| command == PROJECT_INSTALL) =>
        {
            println!("skipped: set TANDEM_VUE_PROJECT to a Vue project with node_modules");
            return Ok(());
        }
        Err(missing) => {
            println!("skipped: {}", missing.message);
            return Ok(());
        }
    };
    let server = tools.servers.first().context("No Vue server was found")?;
    println!("Using {}", tools.label);

    let manager = LanguageManager::start(server_config(
        server,
        &project,
        &root.join("cache").join(&server.name),
    ));
    manager.set_documents(vec![LanguageDocument {
        path: app.clone(),
        instance: "fixture".into(),
        text: Arc::from(APP_TEXT),
    }]);
    for _ in 0..600 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(manager.health().state == "ready", "{:?}", manager.health());

    // The type of a ref comes from TypeScript, through the relay to the companion.
    let at_count = position_params(APP_TEXT, APP_TEXT.find("count =").unwrap() + 2)?;
    eventually(&manager, &app, "textDocument/hover", at_count, |value| {
        value.to_string().contains("Ref<")
    })
    .await?;
    let after_angle = position_params(APP_TEXT, APP_TEXT.find("  <\n").unwrap() + 3)?;
    eventually(
        &manager,
        &app,
        "textDocument/completion",
        after_angle,
        |value| completion_labels(value).contains(&"div"),
    )
    .await?;
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
    manager.stop();

    println!("vue language check passed");
    Ok(())
}
