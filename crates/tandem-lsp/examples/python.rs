use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tandem_lsp::*;
fn main() -> Result<()> {
    smol::block_on(run())
}
async fn run() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m3/python-language");
    std::fs::create_dir_all(&root)?;
    let library = root.join("helper.py");
    let main = root.join("main.py");
    std::fs::write(
        &library,
        "def disk_name(value: int) -> int:\n    return value\n",
    )?;
    std::fs::write(&main, "# disk\n")?;
    let library_text = "def unsaved_double(value: int) -> int:\n    return value + value\n";
    let main_text = "from helper import unsaved_double\nvalue: int = unsaved_double(21)\nmessage = 'hello'\nmessage.upper()\n";
    let tools = tandem_core::PythonTools::discover(&root, None)?;
    let manager = LanguageManager::start(
        python_server(&tools, &root, &root.join("cache")).context("Pyright is not installed")?,
    );
    let mut docs = vec![
        LanguageDocument {
            path: library.clone(),
            instance: "py".into(),
            text: Arc::from(library_text),
        },
        LanguageDocument {
            path: main.clone(),
            instance: "py".into(),
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
    let mut definition = json!(null);
    for _ in 0..40 {
        if let Ok(reply) = manager
            .request(
                "textDocument/definition",
                &main,
                "py",
                main_text,
                position_params(main_text, main_text.find("unsaved_double(21)").unwrap() + 3)?,
            )
            .await
            && !reply.value.is_null()
            && reply.value != json!([])
        {
            definition = reply.value;
            break;
        }
        smol::Timer::after(Duration::from_millis(200)).await;
    }
    ensure!(
        definition.to_string().contains("helper.py"),
        "Unsaved Python definition missing: {definition}"
    );
    let completion = manager
        .request(
            "textDocument/completion",
            &main,
            "py",
            main_text,
            position_params(main_text, main_text.find("message.upper").unwrap() + 10)?,
        )
        .await?;
    ensure!(
        completion.value.to_string().contains("upper"),
        "Python ordinary completion missing"
    );
    let hover = manager
        .request(
            "textDocument/hover",
            &main,
            "py",
            main_text,
            position_params(main_text, main_text.find("unsaved_double(21)").unwrap() + 3)?,
        )
        .await?;
    ensure!(
        hover.value.to_string().contains("unsaved_double"),
        "Python hover missing"
    );
    docs[1].text =
        Arc::from("from helper import unsaved_double\nvalue: int = unsaved_double('wrong')\n");
    manager.set_documents(docs.clone());
    let diagnostic = manager
        .request(
            "textDocument/diagnostic",
            &main,
            "py",
            &docs[1].text,
            json!({}),
        )
        .await?;
    ensure!(
        !diagnostic_items(diagnostic.value.clone())?.is_empty(),
        "Versioned Python diagnostic missing"
    );
    docs[1].text = Arc::from(main_text);
    manager.set_documents(docs.clone());
    ensure!(
        !manager.is_current(&diagnostic.stamp),
        "Old Python diagnostic stayed current"
    );
    let clean = manager
        .request("textDocument/diagnostic", &main, "py", main_text, json!({}))
        .await?;
    ensure!(
        diagnostic_items(clean.value)?.is_empty(),
        "Corrected Python still has a diagnostic"
    );
    manager.stop();
    ensure!(
        std::fs::read_to_string(&main)? == "# disk\n",
        "Python tooling changed source"
    );
    ensure!(
        std::fs::read_to_string(&library)?
            == "def disk_name(value: int) -> int:\n    return value\n",
        "Python tooling changed the library source"
    );
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &json!({"version":"Pyright 1.1.414","unsaved_cross_file_definition":definition,"completion":true,"hover":true,"versioned_diagnostics":diagnostic.value,"old_diagnostics_rejected":true,"source_unchanged":true}),
        )?,
    )?;
    println!("Real Python language integration passed.");
    Ok(())
}
