use anyhow::{Context, Result, ensure};
use gpui_kit::component::input::{CompletionProvider, DefinitionProvider, HoverProvider, Rope};
use gpui_kit::*;
use lsp_types::{CompletionContext, CompletionResponse, Hover, LocationLink};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tandem_lsp::Session;

pub const RUST_TEXT: &str = "fn double(value: u32) -> u32 {\n    value * 2\n}\n\nfn main() {\n    let mut message = String::from(\"Hello, Tandem 🦀\");\n    message.push_str(\"!\");\n    println!(\"{message}: {}\", double(21));\n}\n";

pub fn start() -> Result<Session> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
    let project = home.join("Library/Caches/Tandem/m0-editor-language");
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"tandem-editor-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )?;
    let file = project.join("src/main.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    let binary = home.join(".rustup/toolchains/1.100.0-aarch64-apple-darwin/bin/rust-analyzer");
    Session::start(&binary, &project, &file, RUST_TEXT)
}

#[derive(Clone)]
pub struct Provider(pub Arc<Mutex<Session>>);

impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let session = self.0.clone();
        let text = text.to_string();
        cx.background_spawn(async move {
            session
                .lock()
                .map_err(|_| anyhow::anyhow!("language session failed"))?
                .completions(&text, offset)
        })
    }
    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text == "."
            || new_text == "::"
            || new_text
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    }
}

impl DefinitionProvider for Provider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let session = self.0.clone();
        let text = text.to_string();
        cx.background_spawn(async move {
            session
                .lock()
                .map_err(|_| anyhow::anyhow!("language session failed"))?
                .definitions(&text, offset)
        })
    }
}

impl HoverProvider for Provider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let session = self.0.clone();
        let text = text.to_string();
        cx.background_spawn(async move {
            session
                .lock()
                .map_err(|_| anyhow::anyhow!("language session failed"))?
                .hover(&text, offset)
        })
    }
}

pub fn check() -> Result<()> {
    let mut session = start()?;
    let completion_text = RUST_TEXT.replace("message.push_str", "message.pu");
    let completion_offset = completion_text.find("message.pu").unwrap() + "message.pu".len();
    let mut labels = Vec::new();
    // Initial workspace loading is asynchronous; a bounded retry checks real
    // results rather than assuming initialize means indexing has finished.
    for _ in 0..20 {
        let response = session.completions(&completion_text, completion_offset)?;
        let items = match response {
            CompletionResponse::Array(v) => v,
            CompletionResponse::List(v) => v.items,
        };
        labels = items.into_iter().map(|item| item.label).collect();
        if labels.iter().any(|label| label.starts_with("push_str")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    ensure!(
        labels.iter().any(|label| label.starts_with("push_str")),
        "real LSP completion did not include push_str"
    );
    ensure!(
        labels.iter().all(|label| label.starts_with("pu")),
        "unrelated completion labels leaked through prefix filtering"
    );
    let offset = RUST_TEXT.rfind("double(21)").unwrap() + 2;
    let definitions = session.definitions(RUST_TEXT, offset)?;
    ensure!(
        definitions
            .iter()
            .any(|link| link.target_selection_range.start.line == 0),
        "definition did not map to the fixture function"
    );
    let broken = format!("{RUST_TEXT}\nfn incomplete(");
    let diagnostics = session.diagnostics(&broken)?;
    ensure!(
        !diagnostics.is_empty(),
        "unsaved syntax error did not produce diagnostics"
    );
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    ensure!(
        std::fs::read_to_string(home.join("Library/Caches/Tandem/m0-editor-language/src/main.rs"))?
            == "fn main() {}\n",
        "language fixture wrote its unsaved overlay to disk"
    );
    let receipt = json!({"gate": "M0-B language slice", "completion_count": labels.len(), "expected_symbol_found": true,
        "definition_target_line": 0, "unsaved_diagnostic_count": diagnostics.len(), "disk_unchanged": true,
        "position_encoding": "utf-16", "toolchain": "1.100.0"});
    let path = PathBuf::from(".tandem-local/m0/language-receipt.json");
    std::fs::write(&path, serde_json::to_string_pretty(&receipt)?)?;
    println!(
        "Real rust-analyzer completion, definition and unsaved diagnostics passed; {}",
        path.display()
    );
    Ok(())
}
