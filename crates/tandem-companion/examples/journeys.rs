use anyhow::{Result, ensure};
use std::{fs, sync::atomic::AtomicBool};
use tandem_checks::*;
use tandem_companion::*;
use tandem_core::*;

fn main() -> Result<()> {
    let base = std::env::current_dir()?
        .join(".tandem-local/m3/journeys")
        .join(new_id());
    fs::create_dir_all(&base)?;
    let mut session = CompanionSession::connect()?;
    ensure!(
        session.connection.kind == ConnectionKind::Ready,
        "Subscription is not ready"
    );
    let mut all = vec![];
    for language in ["rust", "python"] {
        let root = base.join(language);
        fs::create_dir_all(&root)?;
        let (path, disk, alternative, wrong, recipe_id) = if language == "rust" {
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname='tandem-journey'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
            )?;
            fs::write(
                root.join("Cargo.lock"),
                "version = 4\n[[package]]\nname = \"tandem-journey\"\nversion = \"0.1.0\"\n",
            )?;
            fs::write(
                root.join("rust-toolchain.toml"),
                "[toolchain]\nchannel='1.100.0'\n",
            )?;
            fs::create_dir(root.join("tests"))?;
            fs::write(
                root.join("tests/behavior.rs"),
                "#[test] fn behavior() { for value in [0, 1, 21, 1_000_000] { assert_eq!(tandem_journey::double(value), value * 2); } }\n",
            )?;
            (
                "src/lib.rs",
                "pub fn double(value: u32) -> u32 { todo!() }\n",
                "pub fn double(value: u32) -> u32 { value + value }\n",
                "pub fn double(value: u32) -> u32 { value + 1 }\n",
                "cargo-test",
            )
        } else {
            fs::write(
                root.join("test_logic.py"),
                "import unittest\nfrom logic import double\nclass Behavior(unittest.TestCase):\n    def test_values(self):\n        for value in [0, 1, 21, 1_000_000]: self.assertEqual(double(value), value * 2)\n",
            )?;
            (
                "logic.py",
                "def double(value: int) -> int:\n    raise NotImplementedError\n",
                "def double(value: int) -> int:\n    return sum((value, value))\n",
                "def double(value: int) -> int:\n    return value + 1\n",
                "python-unittest",
            )
        };
        fs::write(root.join(path), disk)?;
        let project = Project::open(&root)?;
        let selected = if language == "rust" {
            vec![path.into(), "tests/behavior.rs".into()]
        } else {
            vec![path.into(), "test_logic.py".into()]
        };
        let capture = |overlays: &[Document]| {
            capture_context(
                &project,
                overlays,
                &selected,
                &base.join("captures"),
                &AtomicBool::new(false),
            )
        };
        let mut settings = GuidanceSettings::default();
        settings.defaults.instructions = Some(if language == "python" {
            InstructionDetail::HighLevel
        } else {
            InstructionDetail::Detailed
        });
        settings.defaults.explanation = Some(if language == "rust" {
            ExplanationDepth::Detailed
        } else {
            ExplanationDepth::Concise
        });
        let guidance = settings.effective(project.key(), language, None);
        let mut request=ModelRequest{ model_preference: ModelPreference::Automatic, resolved_model: None, conversation_id: Some(new_id()),
            delegation:None,action:CompanionAction::Analyze,question:"Implement double for integer values from 0 through 1,000,000 inclusive. Return twice the input. Existing tests cover the contract; do not add requirements outside this input domain. Keep the plan to one implementation step. Keep all solution code hidden.".into(),language:language.into(),guidance,task:None,context:capture(&[])?,evidence:vec![],prior_decisions:vec![]};
        let (run, mut binding) = session.run(
            ModelRun::pending(project.key().into(), request.clone()),
            None,
            &AtomicBool::new(false),
            |r| {
                if r.status != ModelStatus::Running {
                    println!("{language} Analyze: {:?}", r.status);
                }
                Ok(())
            },
        )?;
        ensure!(
            run.status == ModelStatus::Completed,
            "Analysis failed: {}",
            run.detail
        );
        let Some(CompanionResponse::Plan(plan)) = &run.response else {
            anyhow::bail!("No plan")
        };
        let mut task = plan.into_plan(project.key(), None)?;
        task.start_step(&task.steps[0].id.clone())?;
        all.push(run);
        request.task = Some(task.clone());
        request.action = CompanionAction::Guide;
        request.question="Give useful instructions and explain the relevant language concepts without showing implementation code.".into();
        let (guide, next) = session.run(
            ModelRun::pending(project.key().into(), request.clone()),
            binding,
            &AtomicBool::new(false),
            |_| Ok(()),
        )?;
        binding = next;
        ensure!(
            guide.status == ModelStatus::Completed,
            "Hidden guidance failed: {}",
            guide.detail
        );
        all.push(guide);
        let mut overlay = project.open_document(path, false)?;
        overlay.snapshot(alternative.into());
        let environment = CheckEnvironment::discover(&root, base.join("checks"))?;
        let recipe = environment.recipe(recipe_id)?;
        let passed = run_check(
            &environment,
            &project,
            std::slice::from_ref(&overlay),
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
            passed.status == CheckStatus::Passed,
            "Valid {language} implementation did not pass tests: {}",
            passed.output
        );
        request.context = capture(std::slice::from_ref(&overlay))?;
        request.evidence = matching_check_evidence(
            &request.context,
            std::slice::from_ref(&passed),
            &[(recipe.id.clone(), recipe.fingerprint.clone())],
        );
        ensure!(
            !request.evidence.is_empty(),
            "Evidence did not match context"
        );
        request.action = CompanionAction::Review;
        request.question="Review the alternative implementation against the accepted behavior and real test evidence. A different valid implementation must be accepted; do not require a particular textual solution. Keep suggested implementation code hidden.".into();
        let (review, next) = session.run(
            ModelRun::pending(project.key().into(), request.clone()),
            binding,
            &AtomicBool::new(false),
            |_| Ok(()),
        )?;
        binding = next;
        ensure!(
            review.status == ModelStatus::Completed,
            "Valid review failed: {}",
            review.detail
        );
        let Some(CompanionResponse::Review(result)) = &review.response else {
            anyhow::bail!("No review")
        };
        ensure!(
            result.conclusion == ReviewConclusion::RequirementsSatisfied,
            "Valid alternative was not accepted: {result:?}"
        );
        println!("{language}: valid alternative accepted with real test evidence");
        all.push(review);
        request.action = CompanionAction::Reveal;
        request.question="Show one complete suggested function and explain its meaningful sections. This is display-only; do not change files.".into();
        let (solution, _) = session.run(
            ModelRun::pending(project.key().into(), request.clone()),
            None,
            &AtomicBool::new(false),
            |_| Ok(()),
        )?;
        ensure!(
            solution.status == ModelStatus::Completed
                && matches!(solution.response, Some(CompanionResponse::Solution(_))),
            "Explicit solution missing"
        );
        all.push(solution);
        overlay.snapshot(wrong.into());
        let failed = run_check(
            &environment,
            &project,
            std::slice::from_ref(&overlay),
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
            "Faulty fixture did not fail"
        );
        request.context = capture(std::slice::from_ref(&overlay))?;
        request.evidence = matching_check_evidence(
            &request.context,
            std::slice::from_ref(&failed),
            &[(recipe.id.clone(), recipe.fingerprint.clone())],
        );
        request.action = CompanionAction::Review;
        request.question="Review this changed implementation against the accepted requirements and actual failing tests. Explain the problem, why it matters, and the smallest next action. Keep replacement implementation code hidden.".into();
        let (bad_review, _) = session.run(
            ModelRun::pending(project.key().into(), request),
            binding,
            &AtomicBool::new(false),
            |_| Ok(()),
        )?;
        ensure!(
            bad_review.status == ModelStatus::Completed,
            "Incorrect-code review failed: {}",
            bad_review.detail
        );
        let Some(CompanionResponse::Review(result)) = &bad_review.response else {
            anyhow::bail!("Missing fault review")
        };
        ensure!(
            result.conclusion == ReviewConclusion::NeedsChanges
                && result
                    .findings
                    .iter()
                    .any(|f| f.blocking && !f.next_action.is_empty()),
            "Incorrect code had no justified blocking finding: {result:?}"
        );
        all.push(bad_review);
        ensure!(
            fs::read_to_string(root.join(path))? == disk,
            "Companion/validation changed original source"
        );
        println!("{language}: faulty implementation explained; original source unchanged");
        fs::write(
            base.join(format!("{language}-checks.json")),
            serde_json::to_vec_pretty(&vec![passed, failed])?,
        )?;
        fs::write(base.join("runs.json"), serde_json::to_vec_pretty(&all)?)?;
    }
    ensure!(
        all.iter().all(|r| r.tool_items == 0),
        "Unexpected model tool activity"
    );
    fs::write(
        ".tandem-local/m3/journey-receipt.json",
        serde_json::to_vec_pretty(
            &serde_json::json!({"directory":base,"turns":all.len(),"languages":["rust","python"],"alternative_accepted":true,"faulty_implementation_explained":true,"hidden_guidance":true,"explicit_reveal":true,"source_unchanged":true,"tool_calls":0}),
        )?,
    )?;
    println!("Both subscription-backed companion journeys passed.");
    Ok(())
}
