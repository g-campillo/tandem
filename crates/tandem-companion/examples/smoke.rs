use anyhow::{Context, Result, ensure};
use std::sync::atomic::AtomicBool;
use tandem_companion::*;
use tandem_core::*;

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--status") {
        let args = std::env::args().collect::<Vec<_>>();
        let session = if let Some(index) = args.iter().position(|a| a == "--runtime") {
            let binary = args.get(index + 1).context("--runtime requires a path")?;
            let home =
                std::path::PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
            CompanionSession::connect_at(
                std::path::Path::new(binary),
                &home.join("Library/Application Support/Tandem/codex-m0"),
                &home
                    .join("Library/Caches/Tandem/relocation-check")
                    .join(new_id()),
            )?
        } else {
            CompanionSession::connect()?
        };
        println!(
            "{:?}: {}",
            session.connection.kind,
            session
                .connection
                .limits
                .iter()
                .find(|s| s.starts_with("Included"))
                .map_or("usage status unavailable", String::as_str)
        );
        return Ok(());
    }
    if std::env::args().any(|a| a == "--resume") {
        let root = std::env::current_dir()?.join(".tandem-local/m3/companion-smoke");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("receipt.json"))?)?;
        let previous: ModelRun = serde_json::from_value(saved["run"].clone())?;
        let binding: ThreadBinding = serde_json::from_value(saved["binding"].clone())?;
        let mut request = previous.request;
        request.action = CompanionAction::Ask;
        request.question="What inclusive input range did my original task specify? State both endpoints as decimal digits, without code.".into();
        let mut session = CompanionSession::connect()?;
        let (run, _) = session.run(
            ModelRun::pending(previous.project, request),
            Some(binding.clone()),
            &AtomicBool::new(false),
            |_| Ok(()),
        )?;
        ensure!(
            run.status == ModelStatus::Completed
                && run.thread_id.as_deref() == Some(&binding.thread_id),
            "Conversation did not resume"
        );
        let text = serde_json::to_string(&run.response)?.replace([',', '_'], "");
        ensure!(
            text.contains("1000000"),
            "Resumed response did not retain the original range"
        );
        std::fs::write(
            root.join("resume-receipt.json"),
            serde_json::to_vec_pretty(&run)?,
        )?;
        println!(
            "Real subscription conversation resumed in a new runtime and retained its earlier task context."
        );
        return Ok(());
    }
    let root = std::env::current_dir()?.join(".tandem-local/m3/companion-smoke");
    let source = root.join("project");
    std::fs::create_dir_all(source.join("src"))?;
    std::fs::write(
        source.join("src/lib.rs"),
        "pub fn double(value: u32) -> u32 { todo!() }\n",
    )?;
    let project = Project::open(&source)?;
    let context = capture_context(
        &project,
        &[],
        &["src/lib.rs".into()],
        &root.join("captures"),
        &AtomicBool::new(false),
    )?;
    let mut session = CompanionSession::connect()?;
    println!("Connection: {:?}", session.connection.kind);
    if std::env::args().any(|a| a == "--login") {
        session.login(&AtomicBool::new(false))?;
    }
    ensure!(
        session.connection.kind == ConnectionKind::Ready,
        "{}",
        session.connection.detail
    );
    let request=ModelRequest{ model_preference: ModelPreference::Automatic, resolved_model: None, conversation_id: Some(new_id()),
        delegation:None,action:CompanionAction::Analyze,question:"Implement double for values between 0 and 1,000,000. It must return twice the input, including zero. Keep the plan to one meaningful implementation step and keep the solution hidden.".into(),language:"rust".into(),guidance:GuidanceSettings::default().effective(project.key(),"rust",None),task:None,context,evidence:vec![],prior_decisions:vec![]};
    let (run, binding) = session.run(
        ModelRun::pending(project.key().into(), request),
        None,
        &AtomicBool::new(false),
        |run| {
            println!("{:?}: {} bytes", run.status, run.received_bytes);
            Ok(())
        },
    )?;
    ensure!(
        run.status == ModelStatus::Completed,
        "Companion smoke failed: {}",
        run.detail
    );
    ensure!(
        matches!(run.response, Some(CompanionResponse::Plan(_))),
        "Plan missing"
    );
    ensure!(run.tool_items == 0, "Unexpected tool activity");
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"run":run,"binding":binding,"subscription":true,"source_unchanged":std::fs::read_to_string(source.join("src/lib.rs"))?=="pub fn double(value: u32) -> u32 { todo!() }\n"}),
        )?,
    )?;
    println!("Real subscription-backed structured analysis completed.");
    Ok(())
}
