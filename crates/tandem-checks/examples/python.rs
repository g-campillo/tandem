use anyhow::{Result, ensure};
use std::{fs, sync::atomic::AtomicBool};
use tandem_checks::*;
use tandem_core::*;
fn main() -> Result<()> {
    let root = std::env::current_dir()?.join(".tandem-local/m3/python-checks");
    let source = root.join(new_id());
    fs::create_dir_all(&source)?;
    let venv = std::env::args().any(|a| a == "--venv");
    let copies = std::env::args().any(|a| a == "--copies");
    if venv {
        let base = PythonTools::discover(&source, None)?;
        let mut command = std::process::Command::new(&base.interpreter);
        command.args(["-I", "-m", "venv", "--without-pip"]);
        if copies {
            command.arg("--copies");
        }
        ensure!(
            command.arg(source.join(".venv")).status()?.success(),
            "Could not create the isolated virtualenv fixture"
        );
    }
    let disk = "def double(value: int) -> int:\n    return value * 2\n";
    fs::write(source.join("logic.py"), disk)?;
    let denied = serde_json::to_string(&source.join("logic.py").display().to_string())?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    fs::write(
        source.join("test_logic.py"),
        format!(
            r#"import unittest, pathlib, os, socket
from logic import double
class Behavior(unittest.TestCase):
    def test_value(self): self.assertEqual(double(21), 42)
    def test_zero(self): self.assertEqual(double(0), 0)
    def test_boundary(self):
        with self.assertRaises(PermissionError): pathlib.Path({denied}).read_text()
        with self.assertRaises(PermissionError): pathlib.Path({denied}).write_text('changed')
        with self.assertRaises(OSError): socket.create_connection(('127.0.0.1', {port}), timeout=1)
        pathlib.Path(os.environ['TANDEM_ARTIFACTS'], 'allowed.txt').write_text('artifact')
"#
        ),
    )?;
    let project = Project::open(&source)?;
    let environment = CheckEnvironment::discover(&source, root.join("runs"))?;
    let recipe = environment.recipe("python-unittest")?;
    let mut overlay = project.open_document("logic.py", false)?;
    overlay.snapshot("def double(value: int) -> int:\n    return value + value\n".into());
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
        "Python check failed: {}\n{}",
        passed.detail,
        passed.output
    );
    overlay.snapshot("def double(value: int) -> int:\n    return value + 1\n".into());
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
        "Faulty Python did not fail: {:?}\n{}",
        failed.status,
        failed.output
    );
    ensure!(
        fs::read_to_string(source.join("logic.py"))? == disk,
        "Original Python source changed"
    );
    fs::write(
        root.join(if copies {
            "venv-copies-receipt.json"
        } else if venv {
            "venv-receipt.json"
        } else {
            "receipt.json"
        }),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":passed,"failed":failed,"original_unchanged":true,"network_denied":true}),
        )?,
    )?;
    println!(
        "Real isolated Python checks passed (alternative implementation, faulty code, network/source boundary)."
    );
    Ok(())
}
