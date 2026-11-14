use anyhow::{Result, ensure};
use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tandem_checks::*;
use tandem_core::*;

fn main() -> Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let _baseline = std::net::TcpStream::connect(address)?;
    let root = std::env::current_dir()?.join(".tandem-local/m2/check-integration");
    fs::create_dir_all(&root)?;
    let project_dir = root.join(new_id());
    fs::create_dir_all(project_dir.join("src"))?;
    fs::write(
        project_dir.join("Cargo.toml"),
        "[package]\nname='tandem-check-fixture'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    )?;
    fs::write(
        project_dir.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"tandem-check-fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        project_dir.join("rust-toolchain.toml"),
        "[toolchain]\nchannel='1.100.0'\n",
    )?;
    let original = "pub fn value() -> u32 { 1 }\n#[test] fn checks_unsaved_value() { assert_eq!(value(), 2); }\n";
    fs::write(project_dir.join("src/lib.rs"), original)?;
    fs::write(
        project_dir.join("original-marker.txt"),
        "original must stay unchanged",
    )?;
    let cache = root.join("read-only-cache");
    fs::create_dir_all(cache.join("registry"))?;
    let cache_marker = cache.join("registry/marker");
    fs::write(&cache_marker, "cache must stay unchanged")?;
    let marker = serde_json::to_string(
        &project_dir
            .join("original-marker.txt")
            .display()
            .to_string(),
    )?;
    let cache_literal = serde_json::to_string(&cache_marker.display().to_string())?;
    fs::write(
        project_dir.join("build.rs"),
        format!(
            r#"
fn main() {{
    let address:std::net::SocketAddr="{address}".parse().unwrap();
    assert!(std::net::TcpStream::connect_timeout(&address,std::time::Duration::from_secs(1)).is_err(), "tool network escaped");
    assert!(std::fs::read_to_string({marker}).is_err(), "original read escaped");
    assert!(std::fs::write({marker}, "changed").is_err(), "original write escaped");
    let out=std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("allowed-artifact"), "artifact").unwrap();
    for (source,name) in [("src/lib.rs","input-link"),({cache_literal},"cache-link")] {{
        let link=out.join(name);
        if std::fs::hard_link(source,&link).is_ok() {{ assert!(std::fs::write(&link,"escape").is_err(), "hardlink write escaped"); }}
    }}
}}
"#
        ),
    )?;
    let project = Project::open(&project_dir)?;
    let mut environment = CheckEnvironment::discover(&project.root, root.join("runs"))?;
    environment.cargo_cache = cache;
    let recipe = environment.recipe("cargo-test")?;
    let mut overlay = project.open_document("src/lib.rs", false)?;
    overlay.snapshot(original.replace("{ 1 }", "{ 2 }"));
    let mut receipts = Vec::new();
    let passed = run_check(
        &environment,
        &project,
        &[overlay.clone()],
        &recipe,
        CheckRecord::pending(
            project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        ),
        &AtomicBool::new(false),
        |record| {
            println!("{:?}: {}", record.status, record.detail);
            Ok(())
        },
    )?;
    ensure!(
        passed.status == CheckStatus::Passed,
        "captured-buffer test failed: {}\n{}",
        passed.detail,
        passed.output
    );
    ensure!(
        fs::read_to_string(project_dir.join("src/lib.rs"))? == original,
        "original source changed"
    );
    ensure!(
        fs::read_to_string(cache_marker)? == "cache must stay unchanged",
        "cache modified through hardlink"
    );
    receipts.push(passed);
    overlay.snapshot(
        "pub fn value() -> u32 { 0 }\n#[test] fn actual_failure() { assert_eq!(value(), 2); }\n"
            .into(),
    );
    let failed = run_check(
        &environment,
        &project,
        &[overlay],
        &recipe,
        CheckRecord::pending(
            project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        ),
        &AtomicBool::new(false),
        |_| Ok(()),
    )?;
    ensure!(
        failed.status == CheckStatus::Failed,
        "incorrect implementation was not reported as failed: {:?} {}",
        failed.status,
        failed.detail
    );
    receipts.push(failed);
    let mut slow = project.open_document("src/lib.rs", false)?;
    slow.snapshot(
        "#[test] fn slow() { std::thread::sleep(std::time::Duration::from_secs(60)); }\n".into(),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let mut scheduled = false;
    let cancelled = run_check(
        &environment,
        &project,
        &[slow],
        &recipe,
        CheckRecord::pending(
            project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        ),
        &cancel,
        |record| {
            if record.status == CheckStatus::Running && !scheduled {
                scheduled = true;
                let signal = signal.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    signal.store(true, Ordering::Relaxed);
                });
            }
            Ok(())
        },
    )?;
    ensure!(
        cancelled.status == CheckStatus::Cancelled,
        "check did not cancel: {:?}",
        cancelled.status
    );
    receipts.push(cancelled);
    // Offline dependencies are an environment limitation, never a false pass.
    let mut manifest = project.open_document("Cargo.toml", false)?;
    manifest.snapshot(format!(
        "{}\n[dependencies]\ntandem_missing_dependency_123456789 = \"1.0\"\n",
        manifest.text
    ));
    let unavailable = run_check(
        &environment,
        &project,
        &[manifest],
        &recipe,
        CheckRecord::pending(
            project.key().into(),
            recipe.id.clone(),
            recipe.fingerprint.clone(),
        ),
        &AtomicBool::new(false),
        |_| Ok(()),
    )?;
    ensure!(
        unavailable.status == CheckStatus::Unvalidated,
        "missing dependency should remain unvalidated: {:?}",
        unavailable.status
    );
    receipts.push(unavailable);
    fs::write(
        ".tandem-local/m2/check-integration.json",
        serde_json::to_vec_pretty(&receipts)?,
    )?;
    println!(
        "Real isolated checks passed: unsaved code, test failure, hardlink boundaries and cancellation."
    );
    Ok(())
}
