//! Runs `HelperClient` against the scripted fake helper (`src/bin/fake_helper.rs`).
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use tandem_claude::{HelperClient, Launch, Received};

pub fn fake_helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tandem-fake-helper"))
}

/// Writes one directive per line to a new script file in `dir`.
pub fn write_script(dir: &Path, lines: &[Value]) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let path = dir.join(format!(
        "script-{}.jsonl",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).expect("write the fake helper script");
    path
}

pub fn fake_launch(script: &Path) -> Launch {
    Launch::custom(fake_helper_path(), vec![script.into()])
}

/// Spawns the helper with a sink that forwards everything it receives to a channel.
pub fn collect(launch: &Launch) -> (HelperClient, Receiver<Received>) {
    let (sender, receiver) = mpsc::channel();
    let client = HelperClient::spawn(
        launch,
        Box::new(move |received| {
            let _ = sender.send(received);
        }),
    )
    .expect("spawn the helper");
    (client, receiver)
}
