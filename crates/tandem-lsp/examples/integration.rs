use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tandem_lsp::*;

fn main() -> Result<()> {
    smol::block_on(run())
}
async fn request(
    manager: &LanguageManager,
    path: &Path,
    text: &str,
    method: &str,
    params: Value,
) -> Result<LanguageReply> {
    manager.request(method, path, "fixture", text, params).await
}
async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m2/lsp-integration");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"tandem-lsp-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n",
    )?;
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"tandem-lsp-fixture\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel=\"1.100.0\"\n",
    )?;
    let lib = root.join("src/lib.rs");
    let main = root.join("src/main.rs");
    let disk = "pub fn original_name() -> u32 { 1 }\n";
    std::fs::write(&lib, disk)?;
    std::fs::write(&main, "fn main() {}\n")?;
    let lib_text = "pub fn unsaved_name() -> u32 { 2 }\n";
    let main_text = "fn main() { let value = tandem_lsp_fixture::unsaved_name(); let text = String::new(); text.len(); }\n";
    let tools = tandem_core::RustTools::discover(&root)?;
    let manager = LanguageManager::start(rust_server(&tools, &root, &root.join("target")));
    let docs = vec![
        LanguageDocument {
            path: lib.clone(),
            instance: "fixture".into(),
            text: Arc::from(lib_text),
        },
        LanguageDocument {
            path: main.clone(),
            instance: "fixture".into(),
            text: Arc::from(main_text),
        },
    ];
    manager.set_documents(docs.clone());
    for _ in 0..100 {
        if manager.health().state == "ready" {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
    ensure!(manager.health().state == "ready", "{:?}", manager.health());
    let position = position_params(main_text, main_text.find("unsaved_name").unwrap() + 3)?;
    let mut definition = Value::Null;
    for _ in 0..40 {
        match request(
            &manager,
            &main,
            main_text,
            "textDocument/definition",
            position.clone(),
        )
        .await
        {
            Ok(reply) if !reply.value.is_null() && reply.value != json!([]) => {
                definition = reply.value;
                break;
            }
            _ => {}
        }
        smol::Timer::after(Duration::from_millis(250)).await;
    }
    ensure!(
        definition.to_string().contains("lib.rs"),
        "Unsaved cross-file definition missing: {definition}"
    );
    let hover = request(
        &manager,
        &main,
        main_text,
        "textDocument/hover",
        position.clone(),
    )
    .await?;
    ensure!(
        hover.value.to_string().contains("unsaved_name"),
        "Unsaved hover missing"
    );
    let refs = request(&manager, &main, main_text, "textDocument/references", {
        let mut p = position;
        p["context"] = json!({"includeDeclaration":true});
        p
    })
    .await?;
    ensure!(
        refs.value.as_array().is_some_and(|a| a.len() >= 2),
        "Cross-file references missing: {}",
        refs.value
    );
    let symbols = request(
        &manager,
        &lib,
        lib_text,
        "textDocument/documentSymbol",
        json!({}),
    )
    .await?;
    ensure!(
        symbols.value.to_string().contains("unsaved_name"),
        "Unsaved symbols missing"
    );
    let mut completion = Value::Null;
    for _ in 0..80 {
        completion = request(
            &manager,
            &main,
            main_text,
            "textDocument/completion",
            position_params(main_text, main_text.find("text.len").unwrap() + 7)?,
        )
        .await?
        .value;
        if completion.to_string().contains("len") {
            break;
        }
        smol::Timer::after(Duration::from_millis(250)).await;
    }
    ensure!(
        completion.to_string().contains("len"),
        "Ordinary symbol completion missing: {completion}"
    );
    let formatting = request(
        &manager,
        &main,
        main_text,
        "textDocument/formatting",
        json!({"options":{"tabSize":4,"insertSpaces":true}}),
    )
    .await?;
    let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(formatting.value.clone())?;
    ensure!(
        !edits.is_empty() && apply_text_edits(main_text, &edits)?.contains('\n'),
        "Formatting missing"
    );
    let mut changed = docs;
    changed[1].text = Arc::from("fn main() { let n: u32 = \"wrong\"; }\n");
    manager.set_documents(changed.clone());
    ensure!(
        !manager.is_current(&formatting.stamp),
        "Stale formatting remained current"
    );
    let diagnostic = request(
        &manager,
        &main,
        &changed[1].text,
        "textDocument/diagnostic",
        json!({}),
    )
    .await?;
    ensure!(
        !diagnostic_items(diagnostic.value.clone())?.is_empty(),
        "Incorrect unsaved code had no diagnostic"
    );
    manager.restart();
    smol::Timer::after(Duration::from_secs(2)).await;
    let recovered = request(
        &manager,
        &lib,
        lib_text,
        "textDocument/documentSymbol",
        json!({}),
    )
    .await?;
    ensure!(
        recovered.value.to_string().contains("unsaved_name"),
        "Restart lost unsaved documents"
    );
    manager.stop();
    ensure!(
        std::fs::read_to_string(&lib)? == disk
            && std::fs::read_to_string(&main)? == "fn main() {}\n",
        "Language features changed source files"
    );
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &json!({"definition":definition,"hover":hover.value,"references":refs.value,"symbols":symbols.value,"diagnostic":diagnostic.value,"completion":true,"formatting":true,"stale_rejected":true,"restart":true,"originals_unchanged":true}),
        )?,
    )?;
    println!(
        "Real Rust language integration passed: unsaved multi-file navigation, completion, diagnostics, formatting, stale stamps and restart."
    );
    Ok(())
}
