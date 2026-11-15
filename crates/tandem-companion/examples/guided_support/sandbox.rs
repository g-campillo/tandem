//! Zero-model-turn OS proof through the actual production inspection executor.
use super::*;
use std::{
    net::{TcpListener, TcpStream},
    sync::atomic::Ordering,
    time::Duration,
};
use tandem_checks::*;

pub fn run(root: &Path) -> Result<()> {
    let directory = root.join("sandbox-proof");
    ensure!(
        !directory.exists(),
        "Sandbox proof already has artifacts; inspect them, never overwrite"
    );
    fs::create_dir_all(&directory)?;
    let fixture = FixtureTask::create(&directory.join("project"), "rust")?;
    let tools = directory.join("owned-tool-environment");
    fs::create_dir(&tools)?;
    let tool_marker = tools.join("tool-root-marker");
    fs::write(&tool_marker, "tool root unchanged")?;
    let env_marker = tools.join("environment-marker");
    fs::write(&env_marker, "environment unchanged")?;
    let denied = directory.join("denied");
    fs::create_dir(&denied)?;
    let sentinel = denied.join("sentinel");
    fs::write(&sentinel, "owned denied sentinel")?;
    let live = fixture.project.root.join("README.md");
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let _baseline = TcpStream::connect(address)?;
    let script = tools.join("inspection-probe");
    let code = format!(
        r#"#!/usr/bin/python3
import json, os, pathlib, socket, subprocess, sys
live={live}
sentinel={sentinel}
tool={tool}
environment={environment}
def denied_read(path):
    try: pathlib.Path(path).read_bytes(); return False
    except OSError: return True
def denied_write(path):
    try: pathlib.Path(path).write_text('ESCAPED'); return False
    except OSError: return True
results={{'live_read_denied':denied_read(live),'sentinel_read_denied':denied_read(sentinel),'live_write_denied':denied_write(live),'tool_root_write_denied':denied_write(tool),'environment_write_denied':denied_write(environment)}}
results['captured_input_readable']=bool(pathlib.Path('README.md').read_text())
results['captured_input_readonly']=denied_write('README.md')
out=pathlib.Path(os.environ['HOME']); (out/'allowed-artifact').write_text('allowed')
results['declared_artifact_writable']=True
link=out/'live-link'; link.symlink_to(live);results['symlink_read_denied']=denied_read(link)
try:
    os.link('README.md',out/'input-hardlink'); results['hardlink_write_denied']=denied_write(out/'input-hardlink')
except OSError: results['hardlink_write_denied']=True
child=subprocess.run([sys.executable,'-I','-c','import pathlib,sys\ntry: pathlib.Path(sys.argv[1]).read_bytes();sys.exit(1)\nexcept OSError:sys.exit(0)',sentinel])
results['subprocess_read_denied']=child.returncode==0
try:
    socket.create_connection(('127.0.0.1',{port}),1).close();results['loopback_network_denied']=False
except OSError:results['loopback_network_denied']=True
print(json.dumps(results),flush=True)
sys.exit(0 if all(results.values()) else 1)
"#,
        live = serde_json::to_string(&live.display().to_string())?,
        sentinel = serde_json::to_string(&sentinel.display().to_string())?,
        tool = serde_json::to_string(&tool_marker.display().to_string())?,
        environment = serde_json::to_string(&env_marker.display().to_string())?,
        port = address.port()
    );
    fs::write(&script, code)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    }
    let snapshot = materialize(
        &fixture.project,
        &[],
        &directory.join("capture"),
        &AtomicBool::new(false),
    )?;
    let git = capture_git(
        &fixture.project,
        &directory.join("git"),
        &AtomicBool::new(false),
    )?
    .context("Fixture Git snapshot absent")?;
    let mut checks = CheckEnvironment::discover(&fixture.project.root, directory.join("checks"))?;
    checks.denied_roots.push(denied);
    let environment = InspectionEnvironment::configured(
        checks.clone(),
        &ProjectTools::default(),
        Some(InspectionTool {
            executable: script.clone(),
            fingerprint: file_hash(&script)?,
            read_roots: vec![tools.clone()],
        }),
    )?;
    let request = ResearchRequest::new(ResearchRequestKind::Inspect {
        recipe: InspectionRecipe::GitStatus,
    })?;
    let command = prepare_inspection(
        &environment,
        &fixture.project,
        &snapshot,
        Some(&git),
        &request,
    )?;
    fs::write(
        directory.join("inspection-profile.toml"),
        command.permission_config()?,
    )?;
    let mut executor = RuntimeInspectionExecutor;
    let result = executor.execute(&command, &AtomicBool::new(false), &mut |_| Ok(()))?;
    let output = String::from_utf8_lossy(&result.output);
    write_json(
        &directory.join("inspection-result.json"),
        &json!({"exit_code":result.exit_code,"output":output,"truncated":result.truncated,"command":command.provenance(),"model_turns":0,"identity":identity()?}),
    )?;
    ensure!(
        result.exit_code == Some(0) && !result.truncated,
        "Real inspection boundary probe failed: {output}"
    );
    let assertions: BTreeMap<String, bool> = output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .next_back()
        .context("No boundary assertion object was returned; full output retained")?;
    ensure!(
        assertions.values().all(|v| *v),
        "A real OS boundary assertion failed"
    );
    ensure!(
        fs::read_to_string(&tool_marker)? == "tool root unchanged"
            && fs::read_to_string(&env_marker)? == "environment unchanged"
            && fs::read_to_string(&sentinel)? == "owned denied sentinel",
        "Owned sentinel changed"
    );
    ensure!(
        snapshot_unchanged(&snapshot),
        "Inspection changed captured source"
    );
    fixture.verify_originals()?;

    cancellation(&directory)
}

/// Targeted continuation after the first inspection returned valid assertions
/// alongside xcrun diagnostics. It does not repeat that inspection or use a model.
pub fn resume_cancellation(root: &Path) -> Result<()> {
    let directory = root.join("sandbox-proof");
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("inspection-result.json"))?)?;
    ensure!(value["exit_code"] == 0, "Prior inspection did not pass");
    let output = value["output"].as_str().context("Missing output")?;
    let assertions: BTreeMap<String, bool> = output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .next_back()
        .context("Missing real OS assertions")?;
    ensure!(
        assertions.len() == 12 && assertions.values().all(|v| *v),
        "Prior OS boundary assertions did not all pass"
    );
    cancellation(&directory)
}
fn cancellation(directory: &Path) -> Result<()> {
    ensure!(
        !directory.join("cancellation-result.json").exists(),
        "Cancellation proof already completed; do not rerun"
    );
    let project = Project::open(&directory.join("project"))?;
    let tools = directory.join("owned-tool-environment");
    let snapshot = materialize(
        &project,
        &[],
        &directory.join("cancel-capture"),
        &AtomicBool::new(false),
    )?;
    let git = capture_git(
        &project,
        &directory.join("cancel-git"),
        &AtomicBool::new(false),
    )?
    .context("Missing Git capture")?;
    let checks = CheckEnvironment::discover(&project.root, directory.join("checks"))?;
    let request = ResearchRequest::new(ResearchRequestKind::Inspect {
        recipe: InspectionRecipe::GitStatus,
    })?;
    let slow = tools.join("inspection-child-tree");
    fs::write(
        &slow,
        r#"#!/Applications/Xcode.app/Contents/Developer/usr/bin/python3
import subprocess,sys,time
child_code="import subprocess,sys,time,os,json; grand=subprocess.Popen(['/bin/sleep','60']); print(json.dumps({'child':os.getpid(),'grandchild':grand.pid}),flush=True);time.sleep(60)"
subprocess.Popen([sys.executable,'-I','-c',child_code])
time.sleep(60)
"#,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&slow, fs::Permissions::from_mode(0o755))?;
    }
    let environment = InspectionEnvironment::configured(
        checks,
        &ProjectTools::default(),
        Some(InspectionTool {
            executable: slow.clone(),
            fingerprint: file_hash(&slow)?,
            read_roots: vec![tools],
        }),
    )?;
    let command = prepare_inspection(&environment, &project, &snapshot, Some(&git), &request)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let result = RuntimeInspectionExecutor.execute(&command, &cancel, &mut |_| {
        let signal = signal.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            signal.store(true, Ordering::Release);
        });
        Ok(())
    })?;
    let output = String::from_utf8_lossy(&result.output);
    let pids: BTreeMap<String, u32> = output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .next_back()
        .context("Child PIDs not observed; full output retained")?;
    let mut survivors = vec![];
    for (role, pid) in &pids {
        let status = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()?;
        let state = String::from_utf8(status.stdout)?;
        if !state.trim().is_empty() && !state.trim().starts_with('Z') {
            survivors.push(role.clone());
        }
    }
    write_json(
        &directory.join("cancellation-result.json"),
        &json!({"cancelled":result.cancelled,"timed_out":result.timed_out,"exit_code":result.exit_code,"pids":pids,"survivors":survivors,"output":output,"model_turns":0}),
    )?;
    ensure!(
        result.cancelled && !result.timed_out && pids.len() == 2 && survivors.is_empty(),
        "Actual cancellation did not terminate the child/grandchild tree"
    );
    println!(
        "Real inspection OS boundaries and child-tree cancellation passed; zero model turns; {}",
        directory.display()
    );
    Ok(())
}
