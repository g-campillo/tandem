//! Visible-window measurements of editor mutation to the next frame callback.
//! These are not physical keyboard-to-photon or compositor latency measurements.
use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::input::EditorState;
use gpui_kit::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static FAILED: AtomicBool = AtomicBool::new(false);

pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

async fn next_frame(cx: &mut AsyncWindowContext) -> Result<Instant> {
    let (sender, receiver) = smol::channel::bounded(1);
    cx.update(|window, _| {
        window.on_next_frame(move |_, _| {
            let _ = sender.try_send(Instant::now());
        });
    })?;
    smol::future::race(
        async { receiver.recv().await.context("frame callback closed") },
        async {
            smol::Timer::after(Duration::from_secs(3)).await;
            anyhow::bail!("native frame callback timeout")
        },
    )
    .await
}

pub fn run(editor: Entity<EditorState>, window: &Window, cx: &App) {
    window
        .spawn(cx, async move |cx| {
            let result = measure(editor, cx).await;
            match result {
                Ok(()) => println!(
                    "Native large-buffer frame and Unicode/CRLF round-trip checks completed."
                ),
                Err(error) => {
                    FAILED.store(true, Ordering::Relaxed);
                    eprintln!("Native editor check failed: {error:#}");
                }
            }
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
}

async fn measure(editor: Entity<EditorState>, cx: &mut AsyncWindowContext) -> Result<()> {
    next_frame(cx).await?;
    let roundtrip = "// café e\u{301} 🦀\r\nfn main() {}\r\n";
    editor.update_in(cx, |editor, window, cx| {
        editor.set_value(roundtrip, window, cx)
    })?;
    let contents = editor.read_with(cx, |editor, _| editor.text().to_string());
    ensure!(
        contents == roundtrip,
        "editor normalized the buffer's bytes unexpectedly"
    );
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tandem-local/m0");
    std::fs::create_dir_all(&output)?;
    let file = output.join("editor-roundtrip.txt");
    std::fs::write(&file, &contents)?;
    ensure!(
        std::fs::read_to_string(file)? == roundtrip,
        "buffer snapshot round trip changed bytes"
    );
    let large: String = (0..20_000)
        .map(|line| format!("// representative line {line}: ownership and borrowing\n"))
        .collect();
    let bytes = large.len();
    editor.update_in(cx, |editor, window, cx| editor.set_value(large, window, cx))?;
    next_frame(cx).await?;
    next_frame(cx).await?;
    let mut times = Vec::new();
    for _ in 0..40 {
        let start = Instant::now();
        editor.update_in(cx, |editor, window, cx| editor.insert("x", window, cx))?;
        let frame = next_frame(cx).await?;
        times.push(frame.duration_since(start).as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let p95 = times[37];
    let maximum = times[39];
    let receipt = json!({
        "gate": "M0-B editor measurement", "lines": 20000, "bytes": bytes, "samples": times.len(),
        "measurement": "programmatic EditorState insertion through next native frame callback",
        "p95_ms": p95, "maximum_ms": maximum, "unicode_crlf_snapshot_roundtrip": true,
        "target_p95_ms": 50, "target_maximum_ms": 250,
        "target_met_for_this_measurement": p95 < 50.0 && maximum < 250.0,
        "excludes": ["physical keyboard event delivery", "compositor/pixel presentation", "IME sessions", "concurrent real model streaming"],
        "build": "debug", "rust": "1.100.0", "gpui_kit": "0.9.1"
    });
    std::fs::write(
        output.join("editor-measurement.json"),
        serde_json::to_string_pretty(&receipt)?,
    )?;
    ensure!(
        p95 < 50.0 && maximum < 250.0,
        "native editor measurement exceeded its target (p95 {p95:.1} ms, max {maximum:.1} ms)"
    );
    Ok(())
}
