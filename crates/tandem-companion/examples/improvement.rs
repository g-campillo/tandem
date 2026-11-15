use anyhow::{Context, Result, ensure};
use std::{fs, sync::atomic::AtomicBool};
use tandem_companion::*;
use tandem_core::*;
fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--keep") {
        return keep_current();
    }
    let root = std::env::current_dir()?.join(".tandem-local/m4");
    let pointer: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("benchmark-receipt.json"))?)?;
    let directory = std::path::PathBuf::from(
        pointer["directory"]
            .as_str()
            .context("No benchmark directory")?,
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("receipt.json"))?)?;
    let check: CheckRecord = serde_json::from_value(receipt["passed"].clone())?;
    let project = Project::open(&directory.join("project"))?;
    let cancel = AtomicBool::new(false);
    let mut context = capture_context(
        &project,
        &[],
        &["benchmarks/tandem_compare.py".into()],
        &root.join("improvement-context"),
        &cancel,
    )?;
    let env = tandem_checks::CheckEnvironment::discover(&project.root, directory.join("runs"))?;
    let identities =
        bind_validation_environment(&mut context, Some(&env), &["python-benchmark".into()]);
    let evidence = matching_check_evidence(&context, &[check], &identities);
    ensure!(
        evidence.len() == 1,
        "Benchmark evidence did not match captured source and interpreter"
    );
    let request=ModelRequest{ model_preference: ModelPreference::Automatic, resolved_model: None, conversation_id: None, guided: None,action:CompanionAction::ImproveBalanced,question:"Evaluate baseline versus candidate for repeated lookup in already sorted random-access lists. Preserve first-index and missing-value semantics. Use the provided benchmark receipt only for measured statements, state sorting/setup exclusions and avoid generalizing tiny timings. Discuss an unsorted one-shot workload separately; no automatic optimization there. Proposals only, no full implementation.".into(),language:"python".into(),guidance:GuidanceSettings::default().effective(project.key(),"python",None),task:None,context,evidence,prior_decisions:vec![],delegation:None};
    let before = fs::read(project.root.join("benchmarks/tandem_compare.py"))?;
    let mut session = CompanionSession::connect()?;
    let (run, _) = session.run(
        ModelRun::pending(project.key().into(), request),
        None,
        &cancel,
        |_| Ok(()),
    )?;
    ensure!(
        run.status == ModelStatus::Completed,
        "Improvement request failed: {}",
        run.detail
    );
    let Some(CompanionResponse::Improvement(proposal)) = &run.response else {
        anyhow::bail!("Wrong response type")
    };
    ensure!(
        proposal.evidence_ids.contains(&run.request.evidence[0].id),
        "Proposal did not cite measured evidence"
    );
    ensure!(
        run.tool_items == 0
            && fs::read(project.root.join("benchmarks/tandem_compare.py"))? == before,
        "Proposal modified source or used tools"
    );
    fs::write(
        root.join("improvement-receipt.json"),
        serde_json::to_vec_pretty(&run)?,
    )?;
    println!(
        "Subscription-backed improvement proposal completed and cited the actual benchmark receipt; no edits or model tools."
    );
    Ok(())
}

fn keep_current() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m4/keep-current");
    let source = root.join("project");
    fs::create_dir_all(&source)?;
    let code = "def first_index(values, target):\n    for index, value in enumerate(values):\n        if value == target:\n            return index\n    return -1\n";
    fs::write(source.join("lookup.py"), code)?;
    let project = Project::open(&source)?;
    let cancel = AtomicBool::new(false);
    let context = capture_context(
        &project,
        &[],
        &["lookup.py".into()],
        &root.join("captures"),
        &cancel,
    )?;
    let request=ModelRequest{ model_preference: ModelPreference::Automatic, resolved_model: None, conversation_id: None, guided: None,action:CompanionAction::ImproveSpeed,question:"Evaluate lookup.py for one lookup per unsorted list of at most 20 heterogeneous equality-comparable objects. Return the first original index, with no input mutation. Objects need not be orderable or hashable. No measured bottleneck exists. Consider sorting, indexing, and allocation costs. Recommend keeping the implementation when that is the useful engineering choice. Do not produce code or claim measured speedups.".into(),language:"python".into(),guidance:GuidanceSettings::default().effective(project.key(),"python",None),task:None,context,evidence:vec![],prior_decisions:vec![],delegation:None};
    let mut session = CompanionSession::connect()?;
    let (run, _) = session.run(
        ModelRun::pending(project.key().into(), request),
        None,
        &cancel,
        |_| Ok(()),
    )?;
    ensure!(
        run.status == ModelStatus::Completed,
        "Keep-current request failed: {}",
        run.detail
    );
    let Some(CompanionResponse::Improvement(proposal)) = &run.response else {
        anyhow::bail!("Wrong proposal type")
    };
    ensure!(
        proposal.decision == ImprovementDecision::KeepCurrent && proposal.evidence_ids.is_empty(),
        "Expected an unmeasured keep-current recommendation"
    );
    ensure!(
        run.tool_items == 0 && fs::read_to_string(project.root.join("lookup.py"))? == code,
        "Unexpected mutation"
    );
    fs::write(root.join("receipt.json"), serde_json::to_vec_pretty(&run)?)?;
    println!(
        "Real speed review kept the tiny unsorted lookup, with no benchmark claim or source changes."
    );
    Ok(())
}
