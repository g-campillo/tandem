use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImprovementObjective {
    Clarity,
    Speed,
    Balanced,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImprovementDecision {
    KeepCurrent,
    ProposeChange,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImprovementProposal {
    pub objective: ImprovementObjective,
    pub decision: ImprovementDecision,
    pub summary: String,
    pub invariants: Vec<String>,
    pub preconditions: Vec<String>,
    pub expected_complexity: String,
    pub practical_costs: String,
    pub tradeoffs: Vec<String>,
    pub suggested_steps: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub validation_needed: Vec<String>,
    pub benchmark_plan: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkCase {
    pub label: String,
    pub baseline_ns: Vec<u64>,
    pub candidate_ns: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkMeasurement {
    pub method: String,
    pub warmups: usize,
    pub samples: usize,
    pub cases: Vec<BenchmarkCase>,
}
impl BenchmarkMeasurement {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.method == "perf_counter_ns; alternating order; deepcopy/setup excluded"
                && self.warmups == 2
                && self.samples == 7,
            "Unrecognized benchmark method"
        );
        anyhow::ensure!(
            !self.cases.is_empty() && self.cases.len() <= 12,
            "Invalid benchmark case count"
        );
        let mut labels = std::collections::BTreeSet::new();
        for case in &self.cases {
            anyhow::ensure!(
                !case.label.is_empty() && case.label.len() <= 640 && labels.insert(&case.label),
                "Invalid case label"
            );
            for times in [&case.baseline_ns, &case.candidate_ns] {
                anyhow::ensure!(
                    times.len() == self.samples
                        && times.iter().all(|n| *n > 0 && *n <= 120_000_000_000),
                    "Invalid timing samples"
                );
            }
        }
        Ok(())
    }
    pub fn summary(&self) -> String {
        if self.validate().is_err() {
            return "Invalid benchmark receipt; no measurement is available.".into();
        }
        fn stats(v: &[u64]) -> String {
            let mut sorted = v.to_vec();
            sorted.sort_unstable();
            format!(
                "min {} / median {} / max {} ns",
                sorted[0],
                sorted[sorted.len() / 2],
                sorted[sorted.len() - 1]
            )
        }
        format!(
            "Measured on this snapshot only. {}; {} warmups, {} samples. Output equality checked on every iteration; this does not prove all inputs.\n{}",
            self.method,
            self.warmups,
            self.samples,
            self.cases
                .iter()
                .map(|c| format!(
                    "{}: baseline {}; candidate {}",
                    c.label,
                    stats(&c.baseline_ns),
                    stats(&c.candidate_ns)
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}
