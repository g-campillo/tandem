//! One opt-in hosted-search capability probe, never a model-quality retry loop.
mod guided_support;
use anyhow::{Result, ensure};
use guided_support::*;
use serde_json::json;
use std::sync::atomic::AtomicBool;
use tandem_companion::*;
use tandem_core::*;

fn main() -> Result<()> {
    require_subscription_usage(std::env::args())?;
    let directory = run_directory()?;
    if std::env::args().any(|a| a == "--source-section-only") {
        return verify_source_section(&directory);
    }
    if std::env::args().any(|a| a == "--source-only") {
        return continue_source(&directory);
    }
    if std::env::args().any(|a| a == "--sandbox-only") {
        return guided_support::sandbox::run(&directory);
    }
    if std::env::args().any(|a| a == "--sandbox-resume-cancel") {
        return guided_support::sandbox::resume_cancellation(&directory);
    }
    let (_, model) = health(&directory)?; // Read-only official runtime, zero turns.
    if std::env::args().any(|a| a == "--prerequisites-only") {
        return Ok(());
    }
    let budget = budget(&directory)?;
    budget.lock().unwrap().claim("probe")?;
    let started = now_ms();
    let mut observations = Vec::new();
    let mut source_receipts = Vec::new();
    let result = (|| -> Result<()> {
        let screened = screen_public_brief(
            PublicResearchBrief {
                id: new_id(),
                topics: vec!["Rust standard library integer arithmetic".into()],
                queries: vec!["Rust u32 checked arithmetic official documentation".into()],
                urls: vec![],
            },
            &[],
        )?;
        let brief = screened.brief.as_ref().expect("generic public brief");
        let mut search = PublicResearchSession::connect()?;
        budget.lock().unwrap().reserve("probe", "public")?;
        println!(
            "Dispatching the single approved public research probe; model={}, effort={:?}; cap=4 (shared total cap=52)",
            model.model, model.effort
        );
        let outcome = search.search(brief, &model, &AtomicBool::new(false), |progress| {
            match progress {
                PublicResearchProgress::BriefPrepared { brief, model } => write_json(
                    &directory.join("probe-brief.json"),
                    &json!({"brief":brief,"model":model}),
                )?,
                PublicResearchProgress::SearchObserved { observation } => {
                    observations.push(observation.clone());
                    write_json(&directory.join("probe-search-events.json"), &observations)?;
                }
                PublicResearchProgress::Completed { receipt } => {
                    write_json(&directory.join("probe-search.json"), receipt)?
                }
            }
            Ok(())
        })?;
        ensure!(
            outcome.receipt.status == ResearchReceiptStatus::Complete
                && !outcome.observations.is_empty(),
            "Selected model did not establish hosted search capability: {:?}; {:?}",
            outcome.receipt.status,
            outcome.receipt.omissions
        );
        let mut fetcher = PublicSourceFetcher::new()?;
        // Independent host retrieval establishes the citation, not search text.
        for url in outcome.candidates.iter().take(3) {
            let receipt = fetcher.fetch(
                ResearchRequest::new(ResearchRequestKind::PublicPage { url: url.clone() })?,
                &AtomicBool::new(false),
            )?;
            let valid = receipt.status == ResearchReceiptStatus::Complete
                && receipt
                    .public_web
                    .as_ref()
                    .is_some_and(|w| w.http_status == Some(200) && w.body_hash.is_some());
            source_receipts.push(receipt);
            if valid {
                break;
            }
        }
        ensure!(
            source_receipts
                .iter()
                .any(|r| r.status == ResearchReceiptStatus::Complete),
            "Search candidates produced no usable citation; unsupported/query/PDF/network limitations are retained"
        );
        Ok(())
    })();
    let status = if result.is_ok() { "passed" } else { "failed" };
    budget.lock().unwrap().finish("probe", status)?;
    write_json(
        &directory.join("probe-receipt.json"),
        &json!({"class":"R","identity":identity()?,"selected":model,
        "started_ms":started,"completed_ms":now_ms(),"status":status,"error":result.as_ref().err().map(|e|format!("{e:#}")),
        "hosted_search_observations":observations,"source_receipts":source_receipts,"charged_turns":budget.lock().unwrap().counts(),
        "limits":{"probe":4,"per_language":24,"aggregate":52},"limitations":["Model local tools are disabled by the production profile. No tool events is protocol evidence, not OS sandbox proof.","A charged reservation is never refunded even if preflight failed; it is a conservative dispatch upper bound.","This probe establishes recorded capability only, not broad research/model quality or native usability."]}),
    )?;
    println!(
        "Probe {status}; durable charges={:?}; receipts={}",
        budget.lock().unwrap().counts(),
        directory.display()
    );
    result
}

fn continue_source(directory: &std::path::Path) -> Result<()> {
    let budget = budget(directory)?;
    ensure!(
        budget.lock().unwrap().counts().get("probe") == Some(&1),
        "Source-only continuation requires the original single-search receipt"
    );
    let path = directory.join("probe-receipt.json");
    let mut receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    ensure!(
        receipt.get("source_only_continuation").is_none(),
        "Source-only continuation was already attempted"
    );
    let source = PublicSourceFetcher::new()?.fetch(
        ResearchRequest::new(ResearchRequestKind::PublicPage {
            url: "https://doc.rust-lang.org/std/cmp/fn.min.html".into(),
        })?,
        &AtomicBool::new(false),
    )?;
    let passed = source.status == ResearchReceiptStatus::Complete
        && source
            .public_web
            .as_ref()
            .is_some_and(|w| w.body_hash.is_some());
    receipt["source_only_continuation"] = json!({"selection":"Directly requested smaller official integer-minimum documentation, not returned by search","source":source,"model_turns":0,"at_ms":now_ms()});
    if passed {
        receipt["initial_attempt_status"] = receipt["status"].clone();
        receipt["initial_attempt_error"] = receipt["error"].clone();
        receipt["status"] = json!("passed_after_source_selection");
        receipt["error"] = serde_json::Value::Null;
    }
    write_json(&path, &receipt)?;
    println!(
        "Same-probe source selection: {:?}; no model turns; charges={:?}",
        source.status,
        budget.lock().unwrap().counts()
    );
    ensure!(
        passed,
        "Smaller source was unavailable; prior limitations retained"
    );
    Ok(())
}
fn verify_source_section(directory: &std::path::Path) -> Result<()> {
    let budget = budget(directory)?;
    ensure!(
        budget.lock().unwrap().counts().get("python") == Some(&13),
        "Section proof belongs to the already attempted Python journey"
    );
    let path = directory.join("python/source-section-receipt.json");
    ensure!(
        !path.exists(),
        "Section proof was already attempted; retain its actual receipt"
    );
    let receipt = PublicSourceFetcher::new()?.fetch(
        ResearchRequest::new(ResearchRequestKind::PublicPage {
            url: "https://docs.python.org/3/library/functions.html#min".into(),
        })?,
        &AtomicBool::new(false),
    )?;
    write_json(
        &path,
        &json!({"class":"R","model_turns":0,"selection":"Same directly requested official Python HTML section as the earlier Partial receipts; no cap raised","receipt":receipt,"identity":identity()?}),
    )?;
    println!(
        "Python section-only GET: {:?}; returned_bytes={}; no model turns",
        receipt.status,
        receipt.content.len()
    );
    ensure!(
        receipt.status == ResearchReceiptStatus::Complete
            && receipt.content.contains("smallest")
            && receipt.content.len() < MAX_RESEARCH_RECEIPT_BYTES
            && receipt
                .public_web
                .as_ref()
                .is_some_and(|web| web.final_url.ends_with("#min") && web.body_hash.is_some()),
        "Requested actual section was not fully available within existing limits"
    );
    Ok(())
}
