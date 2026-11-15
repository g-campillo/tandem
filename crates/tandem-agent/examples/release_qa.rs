mod support;
use anyhow::{Result, ensure};
use std::fs;
use tandem_core::*;
fn main() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m5");
    let source = root.join("qa-project");
    ensure!(!source.exists(), "QA project exists; do not overwrite it");
    fs::create_dir_all(source.join("src"))?;
    fs::create_dir(source.join("tests"))?;
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname='tandem-release-qa'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    )?;
    fs::write(
        source.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"tandem-release-qa\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        source.join("rust-toolchain.toml"),
        "[toolchain]\nchannel='1.100.0'\n",
    )?;
    fs::write(
        source.join("src/lib.rs"),
        "/// Double inputs in 0..=1_000_000.\npub fn double(value: u32) -> u32 { value * 2 }\n",
    )?;
    fs::write(
        source.join("tests/behavior.rs"),
        "#[test]\nfn representative_inputs() { for value in [0, 1, 21, 1_000_000] { assert_eq!(tandem_release_qa::double(value), value + value); } }\n",
    )?;
    fs::write(
        source.join("logic.py"),
        "def first_index(values, target):\n    for index, value in enumerate(values):\n        if value == target:\n            return index\n    return -1\n",
    )?;
    fs::write(
        source.join("test_logic.py"),
        "import unittest\nfrom logic import first_index\nclass LookupTests(unittest.TestCase):\n    def test_cases(self):\n        for values, target, expected in [([], 2, -1), ([4, 2, 4], 4, 0), ([3], 1, -1)]:\n            self.assertEqual(first_index(values, target), expected)\n",
    )?;
    let project = Project::open(&source)?;
    let mut store = Store::open(&root.join("qa-state"))?;
    let mut task = TaskPlan::new(
        project.key().into(),
        "Provide a safe small-input doubling function".into(),
        vec!["For integers from zero to one million, return twice the input".into()],
        vec!["Implement and validate the doubling function".into()],
    )?;
    task.steps[0].files = vec!["src/lib.rs".into()];
    task.steps[0].why =
        "Keep the arithmetic contract explicit and validate boundary inputs.".into();
    task.steps[0].done_when = task.acceptance.clone();
    support::approved_session(&mut store, task)?;
    let docs = vec![
        project.open_document("src/lib.rs", false)?,
        project.open_document("logic.py", false)?,
    ];
    let mut state = WorkspaceState::new(&project);
    state.tabs = docs.iter().map(|d| d.path.clone()).collect();
    state.active = Some("src/lib.rs".into());
    state.sequence = 1;
    store.checkpoint(&state, &docs)?;
    println!("Prepared isolated M5 Rust/Python coding workspace.");
    Ok(())
}
