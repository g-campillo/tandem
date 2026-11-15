//! Read-only saved model assessments and definitions. These rows carry no authority.
use tandem_core::*;

pub struct HistoricalRow {
    pub text: String,
    pub source: Option<(String, u32)>,
    pub check_id: Option<String>,
}
fn row(text: impl Into<String>) -> HistoricalRow {
    HistoricalRow {
        text: text.into(),
        source: None,
        check_id: None,
    }
}
fn list(rows: &mut Vec<HistoricalRow>, label: &str, values: &[String]) {
    rows.extend(values.iter().map(|value| row(format!("{label}: {value}"))));
}
fn evidence(rows: &mut Vec<HistoricalRow>, ids: &[String], run: &ModelRun) {
    for id in ids {
        let mut item = row(format!(
            "Historical check evidence · {id} · unavailable in this saved capture"
        ));
        if let Some(check) = run.request.evidence.iter().find(|e| &e.id == id) {
            item.text = format!(
                "Historical check evidence · {} · {} · {}\n{}",
                check.id, check.kind, check.status, check.summary
            );
            item.check_id = Some(check.id.clone());
            if let Some(measurement) = &check.benchmark {
                item.text.push_str(&format!("\n{}", measurement.summary()));
            }
        }
        rows.push(item);
    }
}
pub fn historical_run(run: &ModelRun) -> Vec<HistoricalRow> {
    let mut rows = vec![];
    match &run.response {
        Some(CompanionResponse::Guidance(guidance)) => {
            rows.push(row(format!(
                "Historical model guidance\n{}",
                guidance.summary
            )));
            for section in &guidance.sections {
                rows.push(row(format!("{}\n{}", section.title, section.explanation)));
            }
            rows.push(row(format!(
                "Suggested next action: {}",
                guidance.next_action
            )));
            list(&mut rows, "Missing context", &guidance.requested_context);
        }
        Some(CompanionResponse::Plan(plan)) => {
            rows.push(row(format!(
                "Historical plan proposal\n{}\n{}",
                plan.goal, plan.architecture
            )));
            list(&mut rows, "Acceptance", &plan.acceptance);
            list(&mut rows, "Constraint", &plan.constraints);
            list(&mut rows, "Unknown", &plan.unknowns);
            for (i, step) in plan.steps.iter().enumerate() {
                rows.push(row(format!(
                    "Step {} · {}\nWhy: {}",
                    i + 1,
                    step.title,
                    step.why
                )));
                list(&mut rows, "Done when", &step.done_when);
                list(&mut rows, "File", &step.files);
                list(&mut rows, "Symbol", &step.symbols);
                list(&mut rows, "Language", &step.languages);
                if !step.depends_on.is_empty() {
                    rows.push(row(format!(
                        "Depends on steps: {}",
                        step.depends_on
                            .iter()
                            .map(|n| (n + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
            list(&mut rows, "Missing context", &plan.requested_context);
        }
        Some(CompanionResponse::Review(review)) => {
            rows.push(row(format!(
                "Historical model assessment · {:?}\n{}",
                review.conclusion, review.summary
            )));
            for finding in &review.findings {
                let mut item = row(format!(
                    "Historical model finding · {:?} · {}\n{}\nWhy: {}\nSuggested next action: {}\nCriteria: {}",
                    finding.category,
                    if finding.blocking {
                        "blocking assessment"
                    } else {
                        "nonblocking assessment"
                    },
                    finding.problem,
                    finding.why,
                    finding.next_action,
                    finding
                        .criterion_indices
                        .iter()
                        .map(|n| (n + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                if !finding.path.is_empty() {
                    item.source = Some((finding.path.clone(), finding.line));
                }
                rows.push(item);
                evidence(&mut rows, &finding.evidence_ids, run);
            }
            evidence(&mut rows, &review.evidence_used, run);
            list(&mut rows, "Validation needed", &review.validation_needed);
            list(&mut rows, "Missing context", &review.requested_context);
        }
        Some(CompanionResponse::Improvement(proposal)) => {
            rows.push(row(format!(
                "Historical read-only model improvement · {:?} · {:?}\n{}",
                proposal.objective, proposal.decision, proposal.summary
            )));
            list(&mut rows, "Behavioral invariant", &proposal.invariants);
            list(&mut rows, "Precondition", &proposal.preconditions);
            rows.push(row(format!(
                "Expected complexity: {}\nPractical costs: {}",
                proposal.expected_complexity, proposal.practical_costs
            )));
            list(&mut rows, "Tradeoff", &proposal.tradeoffs);
            list(&mut rows, "Suggested step", &proposal.suggested_steps);
            evidence(&mut rows, &proposal.evidence_ids, run);
            if proposal.evidence_ids.is_empty() {
                rows.push(row(
                    "No benchmark evidence cited; performance is unmeasured.",
                ));
            }
            list(&mut rows, "Validation needed", &proposal.validation_needed);
            rows.push(row(format!("Benchmark plan: {}", proposal.benchmark_plan)));
        }
        // Solution bodies remain exclusively in the separately gated reveal view.
        _ => {}
    }
    rows
}
pub fn historical_task(task: &TaskPlan) -> Vec<HistoricalRow> {
    let mut rows = vec![row(format!(
        "Historical task definition · revision {}\n{}\n{}",
        task.revision, task.goal, task.analysis
    ))];
    list(&mut rows, "Acceptance", &task.acceptance);
    list(&mut rows, "Constraint", &task.constraints);
    list(&mut rows, "Unknown", &task.unknowns);
    for (i, step) in task.steps.iter().enumerate() {
        rows.push(row(format!(
            "Historical step {} · {} · {:?}\nWhy: {}",
            i + 1,
            step.title,
            step.status,
            step.why
        )));
        list(&mut rows, "Done when", &step.done_when);
        list(&mut rows, "File", &step.files);
        list(&mut rows, "Symbol", &step.symbols);
        list(&mut rows, "Language", &step.languages);
        list(&mut rows, "Depends on", &step.depends_on);
        list(&mut rows, "Historical completion evidence", &step.evidence);
        if let Some(note) = &step.completion_note {
            rows.push(row(format!("Historical completion note: {note}")));
        }
    }
    rows
}
