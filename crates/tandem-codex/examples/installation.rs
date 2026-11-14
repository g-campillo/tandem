//! Runtime relocation diagnosis. Never print or persist raw response payloads.
use anyhow::{Context, Result};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
fn main() -> Result<()> {
    let binary = std::env::args_os()
        .nth(1)
        .context("Pass an explicitly selected runtime executable")?;
    let home = std::env::var_os("HOME").context("HOME missing")?;
    let codex_home =
        std::path::PathBuf::from(&home).join("Library/Application Support/Tandem/codex-m0");
    let mut child = Command::new(binary)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("CODEX_HOME", codex_home)
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "model_provider=\"openai\"",
            "-c",
            "forced_login_method=\"chatgpt\"",
            "-c",
            "cli_auth_credentials_store=\"keyring\"",
            "-c",
            "features.secret_auth_storage=false",
            "-c",
            "analytics.enabled=false",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for (id, method, params) in [
        (
            1,
            "initialize",
            json!({"clientInfo":{"name":"tandem","version":"0.1.0"},"capabilities":{"experimentalApi":true}}),
        ),
        (2, "account/read", json!({"refreshToken":false})),
    ] {
        writeln!(
            input,
            "{}",
            json!({"id":id,"method":method,"params":params})
        )?;
        input.flush()?;
        loop {
            let mut line = String::new();
            if output.read_line(&mut line)? == 0 {
                anyhow::bail!("Runtime closed before response")
            };
            let value: serde_json::Value = serde_json::from_str(&line)?;
            if value["id"] == id {
                if let Some(error) = value.get("error") {
                    let text = error.to_string().to_lowercase();
                    let classes = [
                        "keyring",
                        "auth",
                        "resource",
                        "code-mode",
                        "host",
                        "install",
                        "permission",
                        "not found",
                        "missing",
                        "workspace",
                        "routing",
                        "http",
                        "failed to",
                        "config",
                        "network",
                        "credential",
                        "disabled",
                    ]
                    .into_iter()
                    .filter(|s| text.contains(s))
                    .collect::<Vec<_>>();
                    println!(
                        "{method}: error code {}; classifications {:?}; raw payload withheld",
                        error["code"], classes
                    );
                } else {
                    println!("{method}: success");
                }
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}
