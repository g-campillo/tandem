//! Small, single-document LSP client for the native editor feasibility gate.
//! This is not yet the product's multi-document language manager.
use anyhow::{Context, Result, anyhow, bail, ensure};
use lsp_types::{
    CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover, LocationLink, Position,
};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: usize = 4 * 1024 * 1024;

pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = Vec::new();
        let read = reader.take(4097).read_until(b'\n', &mut line)?;
        if read == 0 && header_bytes == 0 {
            return Ok(None);
        }
        ensure!(read > 0 && line.ends_with(b"\n"), "truncated LSP header");
        header_bytes += read;
        ensure!(header_bytes <= 4096, "LSP header exceeds limit");
        if line == b"\r\n" {
            break;
        }
        let text = std::str::from_utf8(&line).context("invalid LSP header encoding")?;
        if let Some((name, value)) = text.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            ensure!(length.is_none(), "duplicate LSP content length");
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid LSP content length")?,
            );
        }
    }
    let length = length.context("missing LSP content length")?;
    ensure!(length <= LIMIT, "LSP message exceeds limit");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).context("truncated LSP body")?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|_| anyhow!("invalid LSP JSON"))
}

pub fn utf16_position(text: &str, byte: usize) -> Result<Position> {
    ensure!(
        byte <= text.len() && text.is_char_boundary(byte),
        "invalid buffer byte offset"
    );
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let column = prefix
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .encode_utf16()
        .count();
    Ok(Position::new(line.try_into()?, column.try_into()?))
}

pub struct Session {
    child: Child,
    input: Option<ChildStdin>,
    incoming: Receiver<Result<Value>>,
    next_id: u64,
    uri: String,
    version: i32,
    current_text: String,
    diagnostics: Option<(Option<i32>, Vec<Diagnostic>)>,
    pull_diagnostics: bool,
}

impl Session {
    pub fn start(binary: &Path, project: &Path, file: &Path, text: &str) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .current_dir(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // The fixture uses an explicit installed toolchain, without changing PATH globally.
        let tool_bin = binary
            .parent()
            .context("language server path missing parent")?;
        command.env(
            "PATH",
            format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", tool_bin.display()),
        );
        let mut child = command.spawn().context("start rust-analyzer")?;
        let input = child.stdin.take().context("LSP stdin missing")?;
        let stdout = child.stdout.take().context("LSP stdout missing")?;
        let (tx, incoming) = mpsc::sync_channel(128);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(message)) => {
                        if tx.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        let uri = url::Url::from_file_path(file)
            .map_err(|_| anyhow!("invalid file URI"))?
            .to_string();
        let root_uri = url::Url::from_directory_path(project)
            .map_err(|_| anyhow!("invalid project URI"))?
            .to_string();
        let mut session = Self {
            child,
            input: Some(input),
            incoming,
            next_id: 1,
            uri,
            version: 1,
            current_text: text.into(),
            diagnostics: None,
            pull_diagnostics: false,
        };
        let initialized = session.request("initialize", json!({
            "processId": std::process::id(), "rootUri": root_uri,
            "capabilities": {"general": {"positionEncodings": ["utf-16"]}, "textDocument": {
                "completion": {"completionItem": {"snippetSupport": false}},
                "definition": {"linkSupport": true}, "diagnostic": {}}},
            "initializationOptions": {"checkOnSave": false, "cargo": {"buildScripts": {"enable": false}}, "procMacro": {"enable": false}}
        }))?;
        ensure!(
            initialized
                .pointer("/capabilities/positionEncoding")
                .is_none_or(|v| v == "utf-16"),
            "unsupported LSP position encoding"
        );
        session.pull_diagnostics = initialized
            .pointer("/capabilities/diagnosticProvider")
            .is_some();
        session.notify("initialized", json!({}))?;
        session.notify("textDocument/didOpen", json!({"textDocument": {"uri": session.uri, "languageId": "rust", "version": 1, "text": text}}))?;
        Ok(session)
    }

    fn send(&mut self, message: Value) -> Result<()> {
        let body = serde_json::to_vec(&message)?;
        let input = self.input.as_mut().context("LSP input closed")?;
        write!(input, "Content-Length: {}\r\n\r\n", body.len())?;
        input.write_all(&body)?;
        input.flush()?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn process_event(&mut self, message: &Value) -> Result<()> {
        if message.get("method").is_some() && message.get("id").is_some() {
            let method = message["method"].as_str().unwrap_or("");
            let result = match method {
                "workspace/configuration" => json!(vec![
                    Value::Null;
                    message
                        .pointer("/params/items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                ]),
                "client/registerCapability" | "window/workDoneProgress/create" => Value::Null,
                "workspace/applyEdit" => {
                    json!({"applied": false, "failureReason": "probe does not authorize workspace edits"})
                }
                _ => {
                    self.send(json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unsupported client method"}}))?;
                    return Ok(());
                }
            };
            self.send(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))?;
        }
        if message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == self.uri
        {
            let version = message["params"]["version"].as_i64().map(|v| v as i32);
            if version.is_none_or(|v| v == self.version) {
                self.diagnostics = Some((
                    version,
                    serde_json::from_value(message["params"]["diagnostics"].clone())?,
                ));
            }
        }
        Ok(())
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let message = self
                .incoming
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("LSP response unavailable")??;
            if message["id"] == id && message.get("method").is_none() {
                if let Some(error) = message.get("error") {
                    bail!("LSP request {method} failed (code {})", error["code"]);
                }
                return message.get("result").cloned().context("missing LSP result");
            }
            self.process_event(&message)?;
        }
    }

    pub fn sync(&mut self, text: &str) -> Result<i32> {
        if self.current_text != text {
            self.version = self
                .version
                .checked_add(1)
                .context("document version exhausted")?;
            self.current_text = text.into();
            self.diagnostics = None;
            self.notify("textDocument/didChange", json!({"textDocument": {"uri": self.uri, "version": self.version}, "contentChanges": [{"text": text}]}))?;
        }
        Ok(self.version)
    }

    fn position_params(&self, text: &str, offset: usize) -> Result<Value> {
        Ok(json!({"textDocument": {"uri": self.uri}, "position": utf16_position(text, offset)?}))
    }

    pub fn completions(&mut self, text: &str, offset: usize) -> Result<CompletionResponse> {
        self.sync(text)?;
        let result = self.request(
            "textDocument/completion",
            self.position_params(text, offset)?,
        )?;
        if result.is_null() {
            return Ok(CompletionResponse::Array(Vec::new()));
        }
        let mut response: CompletionResponse = serde_json::from_value(result)?;
        let prefix = text[..offset]
            .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
            .next()
            .unwrap_or("")
            .to_lowercase();
        let items = match &mut response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => &mut list.items,
        };
        items.retain(|item| {
            item.filter_text
                .as_deref()
                .unwrap_or(&item.label)
                .to_lowercase()
                .starts_with(&prefix)
        });
        Ok(response)
    }

    pub fn definitions(&mut self, text: &str, offset: usize) -> Result<Vec<LocationLink>> {
        self.sync(text)?;
        let result = self.request(
            "textDocument/definition",
            self.position_params(text, offset)?,
        )?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let definition: GotoDefinitionResponse = serde_json::from_value(result)?;
        let locations = match definition {
            GotoDefinitionResponse::Link(links) => return Ok(links),
            GotoDefinitionResponse::Scalar(location) => vec![location],
            GotoDefinitionResponse::Array(locations) => locations,
        };
        Ok(locations
            .into_iter()
            .map(|l| LocationLink {
                origin_selection_range: None,
                target_uri: l.uri,
                target_range: l.range,
                target_selection_range: l.range,
            })
            .collect())
    }

    pub fn hover(&mut self, text: &str, offset: usize) -> Result<Option<Hover>> {
        self.sync(text)?;
        let result = self.request("textDocument/hover", self.position_params(text, offset)?)?;
        Ok(serde_json::from_value(result)?)
    }

    pub fn diagnostics(&mut self, text: &str) -> Result<Vec<Diagnostic>> {
        self.sync(text)?;
        if self.pull_diagnostics {
            let result = self.request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": self.uri}}),
            )?;
            return Ok(serde_json::from_value(result["items"].clone())?);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((_, items)) = &self.diagnostics {
                return Ok(items.clone());
            }
            let event = self
                .incoming
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("diagnostics unavailable")??;
            self.process_event(&event)?;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Bounded cleanup: the feasibility client must not leave a language server behind.
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn positions_distinguish_bytes_and_utf16() {
        assert_eq!(utf16_position("a🦀é\nx", 7).unwrap(), Position::new(0, 4));
        assert_eq!(utf16_position("a🦀é\nx", 9).unwrap(), Position::new(1, 1));
        assert!(utf16_position("🦀", 1).is_err());
    }

    #[test]
    fn headers_are_bounded_and_body_is_not_line_based() {
        let text = "{\"result\":\"a\\nb\"}";
        let input = format!("Content-Length: {}\r\n\r\n{text}", text.len());
        assert_eq!(
            read_frame(&mut Cursor::new(input)).unwrap().unwrap()["result"],
            "a\nb"
        );
        for bad in [
            "Content-Length: 9000000\r\n\r\n",
            "Content-Length: 4\r\nContent-Length: 4\r\n\r\nnull",
            "Content-Length: 4\r\n\r\nnu",
        ] {
            assert!(read_frame(&mut Cursor::new(bad)).is_err());
        }
    }
}
