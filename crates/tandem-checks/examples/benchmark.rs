use anyhow::{Result, ensure};
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
};
use tandem_checks::*;
use tandem_core::*;
fn main() -> Result<()> {
    let root = std::env::current_dir()?
        .join(".tandem-local/m4/benchmarks")
        .join(new_id());
    let source = root.join("project");
    fs::create_dir_all(source.join("benchmarks"))?;
    let fixture = r#"from bisect import bisect_left

def cases():
    yield ('empty', ([], [0]))
    yield ('duplicates-first-index', ([1,1,2,4,4], [0,1,2,3,4,5]))
    for size in (32, 1024, 16384):
        yield (f'sorted-n={size}; 128 queries; sorting excluded', (list(range(size)), [i * size // 127 for i in range(128)]))

def baseline(case):
    values, queries = case
    return [next((i for i, value in enumerate(values) if value == query), -1) for query in queries]

def candidate(case):
    values, queries = case
    result = []
    for query in queries:
        index = bisect_left(values, query)
        result.append(index if index < len(values) and values[index] == query else -1)
    return result
"#;
    fs::write(source.join("benchmarks/tandem_compare.py"), fixture)?;
    let project = Project::open(&source)?;
    let env = CheckEnvironment::discover(&source, root.join("runs"))?;
    let recipe = env.recipe("python-benchmark")?;
    let invoke = |overlays: &[Document], cancel: &AtomicBool, stop: bool| {
        run_check(
            &env,
            &project,
            overlays,
            &recipe,
            CheckRecord::pending(
                project.key().into(),
                recipe.id.clone(),
                recipe.fingerprint.clone(),
            ),
            cancel,
            |record| {
                if stop && record.status == CheckStatus::Running {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            },
        )
    };
    let passed = invoke(&[], &AtomicBool::new(false), false)?;
    ensure!(
        passed.status == CheckStatus::Passed && passed.benchmark.is_some(),
        "Benchmark failed: {:?} {} {}",
        passed.status,
        passed.detail,
        passed.output
    );
    let mut wrong = project.open_document("benchmarks/tandem_compare.py", false)?;
    wrong.snapshot(fixture.replace("    return result", "    return [999]"));
    let failed = invoke(&[wrong], &AtomicBool::new(false), false)?;
    ensure!(
        failed.status == CheckStatus::Failed && failed.benchmark.is_none(),
        "Wrong behavior was accepted"
    );
    let mut slow = project.open_document("benchmarks/tandem_compare.py", false)?;
    slow.snapshot(format!("import time\ntime.sleep(60)\n{fixture}"));
    let cancelled = invoke(&[slow], &AtomicBool::new(false), true)?;
    ensure!(
        matches!(
            cancelled.status,
            CheckStatus::Cancelled | CheckStatus::Interrupted
        ) && cancelled.benchmark.is_none(),
        "Cancelled benchmark became evidence: {:?}",
        cancelled.status
    );
    ensure!(
        fs::read_to_string(source.join("benchmarks/tandem_compare.py"))? == fixture,
        "Original source changed"
    );
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":passed,"failed":failed,"cancelled":cancelled,"source_unchanged":true}),
        )?,
    )?;
    fs::write(
        ".tandem-local/m4/benchmark-receipt.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"directory":root,"status":"passed","behavior_mismatch_rejected":true,"command_cancellation":true,"source_unchanged":true}),
        )?,
    )?;
    println!("{}", passed.benchmark.unwrap().summary());
    Ok(())
}
