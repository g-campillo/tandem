use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tandem_codex::{Client, EXPECTED_RUNTIME};

mod boundaries;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TURN_TIMEOUT: Duration = Duration::from_secs(180);

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn prepare(home: &Path, scratch: &Path) -> Result<()> {
    private_dir(home)?;
    private_dir(scratch)?;
    let config = home.join("config.toml");
    // A probe-owned home: never overwrite an existing differing configuration.
    let text = include_str!("../probe-config.toml");
    if config.exists() {
        ensure!(
            fs::read_to_string(config)? == text,
            "probe configuration differs; inspect it before continuing"
        );
    } else {
        fs::write(config, text)?;
    }
    Ok(())
}

fn auth_state(client: &mut Client) -> Result<&'static str> {
    let account = client.request(
        "account/read",
        json!({"refreshToken": false}),
        REQUEST_TIMEOUT,
    )?;
    match account.pointer("/account/type").and_then(Value::as_str) {
        Some("chatgpt") => Ok("chatgpt"),
        None if account["account"].is_null() => Ok("signed_out"),
        _ => bail!("subscription-only policy: unsupported authentication mode"),
    }
}

fn login(client: &mut Client) -> Result<()> {
    if auth_state(client)? == "chatgpt" {
        println!("Already signed in to Tandem's isolated Codex home using ChatGPT.");
        return Ok(());
    }
    let response = client.request(
        "account/login/start",
        json!({"type": "chatgpt"}),
        REQUEST_TIMEOUT,
    )?;
    let url = response["authUrl"]
        .as_str()
        .context("login response has no browser URL")?;
    // The URL is used only to open the managed flow, never logged or persisted.
    ensure!(
        url.starts_with("https://auth.openai.com/"),
        "unexpected managed login origin"
    );
    ensure!(
        Command::new("/usr/bin/open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success(),
        "could not open managed login browser"
    );
    println!("Managed ChatGPT sign-in opened. Waiting for completion; no credentials are logged.");
    let login_id = response["loginId"].clone();
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let event = client.next_event(deadline.saturating_duration_since(Instant::now()))?;
        if event["method"] == "account/login/completed" && event["params"]["loginId"] == login_id {
            ensure!(
                event["params"]["success"] == true,
                "managed login failed; details withheld"
            );
            ensure!(
                auth_state(client)? == "chatgpt",
                "login did not establish ChatGPT authentication"
            );
            println!("Managed ChatGPT sign-in completed.");
            return Ok(());
        }
    }
}

fn wait_turn(client: &mut Client, thread: &str, turn: &str) -> Result<Value> {
    let deadline = Instant::now() + TURN_TIMEOUT;
    let mut answer = None;
    let mut deltas = 0;
    let mut tool_items = 0;
    loop {
        let event = client.next_event(deadline.saturating_duration_since(Instant::now()))?;
        let params = &event["params"];
        if params["threadId"] != thread || params["turnId"].as_str().is_some_and(|id| id != turn) {
            continue;
        }
        match event["method"].as_str() {
            Some("item/agentMessage/delta") => deltas += 1,
            Some("item/completed") => {
                let item = &params["item"];
                match item["type"].as_str() {
                    Some("agentMessage") => {
                        if let Some(text) = item["text"].as_str()
                            && let Ok(value) = serde_json::from_str::<Value>(text)
                        {
                            answer = Some(value);
                        }
                    }
                    Some("userMessage" | "reasoning" | "plan") => {}
                    _ => tool_items += 1,
                }
            }
            Some("turn/completed") if params["turn"]["id"] == turn => {
                let status = params["turn"]["status"].as_str().unwrap_or("unknown");
                ensure!(
                    matches!(status, "completed" | "interrupted"),
                    "turn ended with {status}; private error payload withheld"
                );
                return Ok(
                    json!({"turn_id": turn, "status": status, "answer": answer, "stream_delta_count": deltas, "tool_item_count": tool_items}),
                );
            }
            _ => {}
        }
    }
}

fn run_turn(client: &mut Client, thread: &str, prompt: &str, expected: i64) -> Result<Value> {
    let result = client.request("turn/start", json!({
        "threadId": thread,
        "input": [{"type": "text", "text": prompt}],
        "effort": "low",
        "outputSchema": {"type": "object", "properties": {"value": {"type": "integer"}}, "required": ["value"], "additionalProperties": false}
    }), REQUEST_TIMEOUT)?;
    let turn = result
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .context("turn/start missing ID")?;
    let receipt = wait_turn(client, thread, turn)?;
    ensure!(
        receipt["status"] == "completed",
        "fixture turn did not complete"
    );
    ensure!(
        receipt["answer"] == json!({"value": expected}),
        "fixture answer does not match the expected result"
    );
    ensure!(
        receipt["tool_item_count"] == 0,
        "unexpected tool activity in no-tools fixture"
    );
    Ok(receipt)
}

fn save_receipt(path: &Path, receipt: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pending = path.with_extension("pending");
    fs::write(&pending, serde_json::to_string_pretty(receipt)?)?;
    fs::rename(pending, path)?;
    Ok(())
}

fn interrupt_fixture(client: &mut Client, thread: &str) -> Result<Value> {
    let result = client.request("turn/start", json!({
        "threadId": thread,
        "input": [{"type": "text", "text": "Count from 1 to 1000 in plain text. Do not use tools."}],
        "effort": "low"
    }), REQUEST_TIMEOUT).context("start cancellation fixture")?;
    let turn = result
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .context("cancellation turn missing ID")?;
    // turn/start's response can precede the running-state transition. Wait for
    // turn/started, rather than racing interrupt against an idle thread.
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        let event = client.next_event(deadline.saturating_duration_since(Instant::now()))?;
        if event["params"]["threadId"] != thread || event["params"]["turn"]["id"] != turn {
            continue;
        }
        if event["method"] == "turn/completed" {
            return Ok(
                json!({"turn_id": turn, "status": event["params"]["turn"]["status"], "interrupt_sent": false, "race": "completed_before_start_observed"}),
            );
        }
        if event["method"] == "turn/started" {
            break;
        }
    }
    let interrupt = client.request(
        "turn/interrupt",
        json!({"threadId": thread, "turnId": turn}),
        REQUEST_TIMEOUT,
    );
    // Even a rejected interruption does not prove what happened to the turn.
    // Consume its terminal event and retain the actual result.
    let terminal =
        wait_turn(client, thread, turn).context("reconcile cancellation terminal state")?;
    Ok(json!({
        "turn_id": turn, "status": terminal["status"], "interrupt_sent": true,
        "interrupt_acknowledged": interrupt.is_ok()
    }))
}

fn recover_receipt(binary: &Path, home: &Path, scratch: &Path, path: &Path) -> Result<()> {
    let mut client = Client::start(binary, home, scratch)?;
    ensure!(
        auth_state(&mut client)? == "chatgpt",
        "subscription authentication missing"
    );
    let threads = client.request(
        "thread/list",
        json!({"limit": 10, "cwd": scratch}),
        REQUEST_TIMEOUT,
    )?;
    let entry = threads["data"]
        .as_array()
        .and_then(|rows| rows.first())
        .context("no isolated fixture thread to recover")?;
    let thread = entry["id"]
        .as_str()
        .context("recovered thread missing ID")?;
    client.request("thread/resume", json!({"threadId": thread, "excludeTurns": true, "sandbox": "read-only", "approvalPolicy": "never"}), REQUEST_TIMEOUT)?;
    let history = client.request(
        "thread/turns/list",
        json!({"threadId": thread, "limit": 20, "sortDirection": "asc", "itemsView": "full"}),
        REQUEST_TIMEOUT,
    )?;
    let turns = history["data"]
        .as_array()
        .context("turn history missing data")?;
    let mut receipts = Vec::new();
    for (turn, expected) in turns.iter().take(3).zip([42, 51, 102]) {
        ensure!(
            turn["status"] == "completed",
            "recovered fixture did not complete"
        );
        let items = turn["items"]
            .as_array()
            .context("recovered turn missing items")?;
        let answer = items
            .iter()
            .filter(|item| item["type"] == "agentMessage")
            .filter_map(|item| item["text"].as_str())
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .find(|answer| answer == &json!({"value": expected}))
            .context("recovered answer does not match fixture")?;
        receipts.push(json!({"turn_id": turn["id"], "status": "completed", "answer": answer}));
    }
    ensure!(receipts.len() == 3, "incomplete recovered fixture history");
    let mut receipt = json!({
        "gate": "M0-A", "runtime": EXPECTED_RUNTIME, "authentication": "chatgpt",
        "thread_id": thread, "turns": receipts, "recovered_from_runtime_history": true,
        "restart_resume_observed": true, "api_credentials_forwarded": false,
        "raw_protocol_logged": false, "cancellation": {"status": "pending"}
    });
    save_receipt(path, &receipt)?;
    println!(
        "Recovered and verified the three completed fixture answers without repeating model calls."
    );
    receipt["cancellation"] = interrupt_fixture(&mut client, thread)?;
    receipt["rejected_server_requests"] = json!(client.rejected_server_requests);
    save_receipt(path, &receipt)?;
    client.shutdown()?;
    println!(
        "Recovery and cancellation receipt saved to {}",
        path.display()
    );
    Ok(())
}

fn annotate_receipts(binary: &Path, home: &Path, scratch: &Path) -> Result<()> {
    let path = Path::new(".tandem-local/m0/subscription-receipt.json");
    let mut receipt: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let thread = receipt["thread_id"]
        .as_str()
        .context("receipt thread missing")?;
    let mut client = Client::start(binary, home, scratch)?;
    ensure!(
        auth_state(&mut client)? == "chatgpt",
        "ChatGPT authentication missing"
    );
    let resumed = client.request(
        "thread/resume",
        json!({"threadId": thread, "excludeTurns": true}),
        REQUEST_TIMEOUT,
    )?;
    receipt["model"] = resumed["model"].clone();
    save_receipt(path, &receipt)?;
    let policy_path = Path::new(".tandem-local/m0/model-boundary-receipt.json");
    let mut policy: Value = serde_json::from_str(&fs::read_to_string(policy_path)?)?;
    let history = client.request(
        "thread/turns/list",
        json!({"threadId": policy["thread_id"], "limit": 10, "itemsView": "full"}),
        REQUEST_TIMEOUT,
    )?;
    let mut commands = Vec::new();
    for turn in history["data"]
        .as_array()
        .context("fixture history missing")?
    {
        for item in turn["items"].as_array().context("fixture items missing")? {
            if item["type"] == "commandExecution" {
                let command = item["command"].as_str().unwrap_or("");
                let output = item["aggregatedOutput"].as_str().unwrap_or("");
                commands.push(json!({
                    "mentions_source_fixture": command.contains("source.txt"),
                    "mentions_outside_fixture": command.contains("outside.txt"),
                    "mentions_instruction_fixture": command.contains("AGENTS.md"),
                    "permission_denial_reported": output.contains("Permission denied") || output.contains("Operation not permitted"),
                    "exit_code": item["exitCode"]
                }));
            }
        }
    }
    policy["persisted_command_evidence"] = json!(commands);
    save_receipt(policy_path, &policy)?;
    println!("Recorded available model and persisted command evidence without new model calls.");
    Ok(())
}

fn exercise(binary: &Path, home: &Path, scratch: &Path, receipt_path: &Path) -> Result<()> {
    let mut client = Client::start(binary, home, scratch)?;
    ensure!(
        auth_state(&mut client)? == "chatgpt",
        "ChatGPT sign-in required; run the login command"
    );
    let models = client.request(
        "model/list",
        json!({"limit": 100, "includeHidden": false}),
        REQUEST_TIMEOUT,
    )?;
    let choices = models["data"]
        .as_array()
        .context("model/list missing data")?;
    let choice = choices
        .iter()
        .find(|m| m["isDefault"] == true)
        .or_else(|| choices.first())
        .context("no available subscription models")?;
    let model = choice["model"]
        .as_str()
        .context("model entry missing model ID")?;
    println!("Starting short subscription fixtures using the account's available default model.");
    let started = client.request("thread/start", json!({
        "model": model, "modelProvider": "openai", "cwd": scratch,
        "sandbox": "read-only", "approvalPolicy": "never",
        "developerInstructions": "This is a harmless arithmetic integration fixture. Answer the user's calculation directly using the required JSON schema. Do not use tools, read files, or execute commands."
    }), REQUEST_TIMEOUT)?;
    let thread = started
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .context("thread/start missing ID")?
        .to_string();
    let mut receipt = json!({
        "gate": "M0-A", "runtime": EXPECTED_RUNTIME, "authentication": "chatgpt",
        "model": model, "thread_id": thread, "turns": [], "cancellation": {"status": "pending"},
        "api_credentials_forwarded": false, "raw_protocol_logged": false
    });
    save_receipt(receipt_path, &receipt)?;
    let first = run_turn(
        &mut client,
        &thread,
        "Set a running value to 17 plus 25. Return that value.",
        42,
    )?;
    receipt["turns"].as_array_mut().unwrap().push(first);
    save_receipt(receipt_path, &receipt)?;
    println!("First real turn completed and its answer was verified.");
    let second = run_turn(
        &mut client,
        &thread,
        "Add 9 to the previous running value and return the new value.",
        51,
    )?;
    receipt["turns"].as_array_mut().unwrap().push(second);
    save_receipt(receipt_path, &receipt)?;
    println!("Context-dependent follow-up completed and its answer was verified.");
    client.shutdown()?;
    let mut resumed = Client::start(binary, home, scratch)?;
    ensure!(
        auth_state(&mut resumed)? == "chatgpt",
        "authentication did not survive child restart"
    );
    resumed.request("thread/resume", json!({"threadId": thread, "excludeTurns": true, "sandbox": "read-only", "approvalPolicy": "never"}), REQUEST_TIMEOUT)?;
    let third = run_turn(
        &mut resumed,
        &thread,
        "Double the last running value and return the new value.",
        102,
    )?;
    receipt["turns"].as_array_mut().unwrap().push(third);
    save_receipt(receipt_path, &receipt)?;
    println!("Child restart and thread resume completed with the expected answer.");
    receipt["cancellation"] = interrupt_fixture(&mut resumed, &thread)?;
    receipt["rejected_server_requests"] =
        json!(client.rejected_server_requests + resumed.rejected_server_requests);
    receipt["notification_counts_before_restart"] = json!(client.notifications);
    receipt["notification_counts_after_restart"] = json!(resumed.notifications);
    save_receipt(receipt_path, &receipt)?;
    resumed.shutdown()?;
    println!(
        "Subscription fixture receipt saved to {}",
        receipt_path.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args
        .next()
        .context("usage: tandem-probe status|login|turns|recover|boundaries [receipt-path]")?;
    let user_home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
    let home = user_home.join("Library/Application Support/Tandem/codex-m0");
    let scratch = user_home.join("Library/Caches/Tandem/m0-auth-fixture");
    let binary = Path::new("/opt/homebrew/Caskroom/codex/0.169.0/bin/codex");
    prepare(&home, &scratch)?;
    match action.as_str() {
        "status" => {
            let mut client = Client::start(binary, &home, &scratch)?;
            println!(
                "{}",
                json!({"runtime": EXPECTED_RUNTIME, "auth": auth_state(&mut client)?, "home": "Tandem/codex-m0", "transport": "stdio"})
            );
        }
        "login" => login(&mut Client::start(binary, &home, &scratch)?)?,
        "turns" => {
            let receipt = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".tandem-local/m0/subscription-receipt.json"));
            exercise(binary, &home, &scratch, &receipt)?;
        }
        "boundaries" => {
            boundaries::run(
                binary,
                &user_home,
                Path::new(".tandem-local/m0/boundaries-receipt.json"),
            )?;
        }
        "recover" => recover_receipt(
            binary,
            &home,
            &scratch,
            Path::new(".tandem-local/m0/subscription-receipt.json"),
        )?,
        "model-boundaries" => boundaries::model_check(binary, &home, &user_home)?,
        "annotate-receipts" => annotate_receipts(binary, &home, &scratch)?,
        _ => bail!("unknown probe action"),
    }
    Ok(())
}
