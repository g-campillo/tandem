//! Finds Node.js and the Claude helper, and prepares the helper's process environment.
//! A broken app bundle never falls back to a development helper.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MIN_NODE: (u64, u64, u64) = (20, 0, 0);

/// A Node.js executable that reported a supported version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRuntime {
    pub path: PathBuf,
    /// Without the leading `v`, for example `24.22.1`.
    pub version: String,
}

/// Where the helper's `main.mjs` and `node_modules` live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperInstall {
    pub dir: PathBuf,
    pub entry: PathBuf,
    pub bundled: bool,
    /// The app bundle's `Contents/Resources` folder when `bundled`.
    pub resources: Option<PathBuf>,
}

/// Everything needed to start the helper process.
#[derive(Clone, Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// The environment before scrubbing; `HelperClient::spawn` applies `child_env`.
    pub env: Vec<(OsString, OsString)>,
    pub node: Option<NodeRuntime>,
    pub helper: Option<HelperInstall>,
}

const DEVELOPMENT_HELPER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/helper");
const SDK_PACKAGE: &str = "node_modules/@anthropic-ai/claude-agent-sdk/package.json";
const CLAUDE_BINARY: &str = "node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude";
const NODE_MISSING: &str = "Node.js 20 or newer was not found. Install it (for example with nvm or Homebrew), then press Reconnect.";

/// Places to look for `node`, best first: `PATH`, then nvm installs from the newest version
/// down, then Homebrew's two prefixes.
pub fn node_candidates(path_var: Option<&OsStr>, home: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut add = |candidate: PathBuf| {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };
    for dir in path_var.into_iter().flat_map(std::env::split_paths) {
        if !dir.as_os_str().is_empty() {
            add(dir.join("node"));
        }
    }
    for dir in nvm_installs(home) {
        add(dir.join("bin/node"));
    }
    add(PathBuf::from("/opt/homebrew/bin/node"));
    add(PathBuf::from("/usr/local/bin/node"));
    candidates
}

/// nvm's version folders (`v24.22.1`), highest version first and unparsable names last.
fn nvm_installs(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) else {
        return Vec::new();
    };
    let mut installs: Vec<(Option<semver::Version>, PathBuf)> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name();
            let version = name
                .to_str()
                .and_then(|name| semver::Version::parse(name.strip_prefix('v')?).ok());
            (version, entry.path())
        })
        .collect();
    installs.sort_by(
        |(a_version, a_path), (b_version, b_path)| match (a_version, b_version) {
            (Some(a), Some(b)) => b.cmp(a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a_path.cmp(b_path),
        },
    );
    installs.into_iter().map(|(_, path)| path).collect()
}

/// The first candidate that runs and reports Node.js `MIN_NODE` or newer.
pub fn discover_node(path_var: Option<&OsStr>, home: &Path) -> Result<NodeRuntime> {
    select_node(&node_candidates(path_var, home))
}

fn select_node(candidates: &[PathBuf]) -> Result<NodeRuntime> {
    let minimum = semver::Version::new(MIN_NODE.0, MIN_NODE.1, MIN_NODE.2);
    candidates
        .iter()
        .filter(|candidate| candidate.is_file())
        .find_map(|candidate| {
            let output = Command::new(candidate).arg("--version").output().ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8(output.stdout).ok()?;
            let version = text.trim().strip_prefix('v')?;
            (semver::Version::parse(version).ok()? >= minimum).then(|| NodeRuntime {
                path: candidate.clone(),
                version: version.to_owned(),
            })
        })
        .ok_or_else(|| anyhow!(NODE_MISSING))
}

/// The app bundle's `Contents/Resources` folder when `executable` runs from a `.app`.
fn bundle_resources(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && contents
            .parent()?
            .extension()?
            .to_str()?
            .eq_ignore_ascii_case("app"))
    .then(|| contents.join("Resources"))
}

/// Where the helper should be. A bundled app always uses its own copy.
pub fn locate_helper(
    executable: &Path,
    override_dir: Option<&Path>,
    development: &Path,
) -> HelperInstall {
    let (dir, resources) = match bundle_resources(executable) {
        Some(resources) => (resources.join("claude-helper"), Some(resources)),
        None => (override_dir.unwrap_or(development).to_path_buf(), None),
    };
    HelperInstall {
        entry: dir.join("main.mjs"),
        bundled: resources.is_some(),
        resources,
        dir,
    }
}

/// Locates the helper for this executable (honouring `TANDEM_CLAUDE_HELPER_DIR` outside a
/// bundle) and checks that it is installed.
pub fn discover_helper() -> Result<HelperInstall> {
    let executable = std::env::current_exe().context("Cannot locate the Tandem application")?;
    let override_dir = std::env::var_os("TANDEM_CLAUDE_HELPER_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    verify_helper(locate_helper(
        &executable,
        override_dir.as_deref(),
        Path::new(DEVELOPMENT_HELPER),
    ))
}

fn verify_helper(helper: HelperInstall) -> Result<HelperInstall> {
    if helper.entry.is_file() && helper.dir.join(SDK_PACKAGE).is_file() {
        return Ok(helper);
    }
    if helper.bundled {
        bail!(
            "The bundled Claude helper is missing or damaged. Reinstall a verified Tandem release; editing still works."
        );
    }
    bail!(
        "The Claude helper is not installed. Run `npm ci` in {}, then press Reconnect.",
        helper.dir.display()
    )
}

/// The Claude Code binary the helper's SDK runs, when it is installed.
pub fn claude_binary(helper: &HelperInstall) -> Option<PathBuf> {
    let binary = helper.dir.join(CLAUDE_BINARY);
    binary.is_file().then_some(binary)
}

/// The command the user runs in a terminal to sign Claude Code in.
pub fn sign_in_hint(helper: Option<&HelperInstall>) -> String {
    match helper.and_then(claude_binary) {
        Some(binary) => format!("\"{}\" auth login", binary.display()),
        None => "claude auth login".to_owned(),
    }
}

/// The helper's environment: `vars` without API credentials or provider switches (so a turn
/// can only use the Claude subscription), with `extra_path` directories added to `PATH`.
pub fn child_env(
    vars: &[(OsString, OsString)],
    extra_path: &[PathBuf],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vars
        .iter()
        .filter(|(name, _)| !is_scrubbed(name))
        .cloned()
        .collect();
    let path_index = env.iter().position(|(name, _)| name == "PATH");
    let mut dirs: Vec<PathBuf> = path_index
        .map(|index| std::env::split_paths(&env[index].1).collect())
        .unwrap_or_default();
    let listed = dirs.len();
    for dir in extra_path {
        if dir.is_dir() && !dirs.contains(dir) && std::env::join_paths([dir]).is_ok() {
            dirs.push(dir.clone());
        }
    }
    if dirs.len() == listed {
        return env;
    }
    let value = std::env::join_paths(&dirs).expect("every directory was checked to be joinable");
    match path_index {
        Some(index) => env[index].1 = value,
        None => env.push(("PATH".into(), value)),
    }
    env
}

fn is_scrubbed(name: &OsStr) -> bool {
    name == "ANTHROPIC_API_KEY"
        || name == "ANTHROPIC_AUTH_TOKEN"
        || name.as_encoded_bytes().starts_with(b"CLAUDE_CODE_USE_")
}

impl Launch {
    /// Node.js and the installed helper, started as `node <helper>/main.mjs` in the helper
    /// folder with this process's environment.
    pub fn discover() -> Result<Launch> {
        let home = std::env::home_dir().unwrap_or_default();
        let node = discover_node(std::env::var_os("PATH").as_deref(), &home)?;
        let helper = discover_helper()?;
        Ok(Launch {
            program: node.path.clone(),
            args: vec![helper.entry.clone().into()],
            cwd: helper.dir.clone(),
            env: std::env::vars_os().collect(),
            node: Some(node),
            helper: Some(helper),
        })
    }

    /// Any program in the current directory, for tests.
    pub fn custom(program: PathBuf, args: Vec<OsString>) -> Launch {
        Launch {
            program,
            args,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            env: std::env::vars_os().collect(),
            node: None,
            helper: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn pair(name: &str, value: impl Into<OsString>) -> (OsString, OsString) {
        (name.into(), value.into())
    }

    fn lookup<'a>(vars: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsString> {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// A shell script standing in for `node`, reporting `version` for `--version`.
    fn fake_node(dir: &Path, version: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("node");
        fs::write(&path, format!("#!/bin/sh\necho {version}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn nvm_bin(home: &Path, name: &str) -> PathBuf {
        home.join(".nvm/versions/node").join(name).join("bin")
    }

    fn joined(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn child_env_removes_credentials_and_provider_switches() {
        let vars = vec![
            pair("ANTHROPIC_API_KEY", "x"),
            pair("ANTHROPIC_AUTH_TOKEN", "y"),
            pair("CLAUDE_CODE_USE_BEDROCK", "1"),
            pair("CLAUDE_CODE_USE_VERTEX", "1"),
            pair("ANTHROPIC_MODEL", "haiku"),
            pair("CLAUDE_CODE_ENTRYPOINT", "tandem"),
            pair("HOME", "/Users/someone"),
        ];
        let env = child_env(&vars, &[]);
        let names: Vec<_> = env.iter().map(|(name, _)| name.to_str().unwrap()).collect();
        assert_eq!(names, ["ANTHROPIC_MODEL", "CLAUDE_CODE_ENTRYPOINT", "HOME"]);
    }

    #[test]
    fn child_env_appends_only_existing_unlisted_directories_to_path() {
        let temp = tempfile::tempdir().unwrap();
        let listed = temp.path().join("listed");
        let added = temp.path().join("added");
        let missing = temp.path().join("missing");
        fs::create_dir(&listed).unwrap();
        fs::create_dir(&added).unwrap();
        let vars = vec![
            pair("PATH", joined(&[Path::new("/usr/bin"), &listed])),
            pair("KEEP", "z"),
        ];
        let env = child_env(
            &vars,
            &[listed.clone(), added.clone(), missing, added.clone()],
        );
        assert_eq!(
            lookup(&env, "PATH"),
            Some(&joined(&[Path::new("/usr/bin"), &listed, &added]))
        );
        assert_eq!(lookup(&env, "KEEP"), Some(&OsString::from("z")));
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn child_env_creates_path_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let env = child_env(&[pair("KEEP", "z")], &[temp.path().to_path_buf()]);
        assert_eq!(lookup(&env, "PATH"), Some(&joined(&[temp.path()])));
    }

    #[test]
    fn node_candidates_try_path_then_nvm_by_version_then_homebrew() {
        let home = tempfile::tempdir().unwrap();
        for name in ["v18.20.0", "v24.1.0", "system", "v22.3.0"] {
            fs::create_dir_all(nvm_bin(home.path(), name)).unwrap();
        }
        let path_var = joined(&[Path::new("/first"), Path::new("/second")]);
        let candidates = node_candidates(Some(&path_var), home.path());
        assert_eq!(
            candidates,
            [
                PathBuf::from("/first/node"),
                PathBuf::from("/second/node"),
                nvm_bin(home.path(), "v24.1.0").join("node"),
                nvm_bin(home.path(), "v22.3.0").join("node"),
                nvm_bin(home.path(), "v18.20.0").join("node"),
                nvm_bin(home.path(), "system").join("node"),
                PathBuf::from("/opt/homebrew/bin/node"),
                PathBuf::from("/usr/local/bin/node"),
            ]
        );
    }

    #[test]
    fn node_candidates_have_no_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let path_var = joined(&[
            Path::new("/opt/homebrew/bin"),
            Path::new("/first"),
            Path::new("/first"),
        ]);
        assert_eq!(
            node_candidates(Some(&path_var), home.path()),
            [
                PathBuf::from("/opt/homebrew/bin/node"),
                PathBuf::from("/first/node"),
                PathBuf::from("/usr/local/bin/node"),
            ]
        );
        assert_eq!(
            node_candidates(None, home.path()),
            [
                PathBuf::from("/opt/homebrew/bin/node"),
                PathBuf::from("/usr/local/bin/node"),
            ]
        );
    }

    #[test]
    fn discover_node_prefers_a_path_candidate() {
        let home = tempfile::tempdir().unwrap();
        let on_path = home.path().join("bin");
        let path_node = fake_node(&on_path, "v24.22.1");
        fake_node(&nvm_bin(home.path(), "v22.3.0"), "v22.3.0");
        let node = discover_node(Some(&joined(&[&on_path])), home.path()).unwrap();
        assert_eq!(
            node,
            NodeRuntime {
                path: path_node,
                version: "24.22.1".into(),
            }
        );
    }

    #[test]
    fn discover_node_skips_an_old_node_for_a_newer_nvm_one() {
        let home = tempfile::tempdir().unwrap();
        let on_path = home.path().join("bin");
        fake_node(&on_path, "v18.19.0");
        let nvm_node = fake_node(&nvm_bin(home.path(), "v22.3.0"), "v22.3.0");
        let node = discover_node(Some(&joined(&[&on_path])), home.path()).unwrap();
        assert_eq!(node.path, nvm_node);
        assert_eq!(node.version, "22.3.0");
    }

    #[test]
    fn no_usable_node_gives_the_install_hint() {
        // The system locations are left out so the result does not depend on this Mac.
        let temp = tempfile::tempdir().unwrap();
        let old = fake_node(&temp.path().join("old"), "v18.19.0");
        let broken = fake_node(&temp.path().join("broken"), "not-a-version");
        let error = select_node(&[temp.path().join("missing/node"), old, broken]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Node.js 20 or newer was not found. Install it (for example with nvm or Homebrew), then press Reconnect."
        );
    }

    #[test]
    fn a_bundled_app_uses_its_resources_helper() {
        let helper = locate_helper(
            Path::new("/Applications/Tandem.app/Contents/MacOS/tandem"),
            Some(Path::new("/elsewhere")),
            Path::new("/code/crates/tandem-claude/helper"),
        );
        assert_eq!(
            helper,
            HelperInstall {
                dir: "/Applications/Tandem.app/Contents/Resources/claude-helper".into(),
                entry: "/Applications/Tandem.app/Contents/Resources/claude-helper/main.mjs".into(),
                bundled: true,
                resources: Some("/Applications/Tandem.app/Contents/Resources".into()),
            }
        );
        let lowercase = locate_helper(
            Path::new("/Applications/Tandem.APP/Contents/MacOS/tandem"),
            None,
            Path::new("/dev"),
        );
        assert!(lowercase.bundled);
    }

    #[test]
    fn a_development_build_uses_the_override_or_the_source_helper() {
        let development = Path::new("/code/crates/tandem-claude/helper");
        let plain = locate_helper(Path::new("/code/target/debug/tandem"), None, development);
        assert_eq!(
            plain,
            HelperInstall {
                dir: development.into(),
                entry: development.join("main.mjs"),
                bundled: false,
                resources: None,
            }
        );
        let overridden = locate_helper(
            Path::new("/code/target/debug/tandem"),
            Some(Path::new("/tmp/helper")),
            development,
        );
        assert_eq!(overridden.dir, PathBuf::from("/tmp/helper"));
        assert_eq!(overridden.entry, PathBuf::from("/tmp/helper/main.mjs"));
        assert!(!overridden.bundled);
    }

    fn install_in(dir: &Path, bundled: bool) -> HelperInstall {
        HelperInstall {
            dir: dir.into(),
            entry: dir.join("main.mjs"),
            bundled,
            resources: None,
        }
    }

    #[test]
    fn verify_helper_requires_the_entry_and_the_sdk() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let dev_error = verify_helper(install_in(dir, false)).unwrap_err();
        assert_eq!(
            dev_error.to_string(),
            format!(
                "The Claude helper is not installed. Run `npm ci` in {}, then press Reconnect.",
                dir.display()
            )
        );
        let bundled_error = verify_helper(install_in(dir, true)).unwrap_err();
        assert_eq!(
            bundled_error.to_string(),
            "The bundled Claude helper is missing or damaged. Reinstall a verified Tandem release; editing still works."
        );
        fs::write(dir.join("main.mjs"), "").unwrap();
        assert!(verify_helper(install_in(dir, false)).is_err());
        let sdk = dir.join("node_modules/@anthropic-ai/claude-agent-sdk");
        fs::create_dir_all(&sdk).unwrap();
        fs::write(sdk.join("package.json"), "{}").unwrap();
        assert_eq!(
            verify_helper(install_in(dir, false)).unwrap(),
            install_in(dir, false)
        );
    }

    #[test]
    fn sign_in_hint_names_the_bundled_claude_binary_when_present() {
        let temp = tempfile::tempdir().unwrap();
        let helper = install_in(temp.path(), false);
        assert_eq!(claude_binary(&helper), None);
        assert_eq!(sign_in_hint(Some(&helper)), "claude auth login");
        assert_eq!(sign_in_hint(None), "claude auth login");

        let binary_dir = temp
            .path()
            .join("node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64");
        fs::create_dir_all(&binary_dir).unwrap();
        fs::write(binary_dir.join("claude"), "").unwrap();
        let binary = binary_dir.join("claude");
        assert_eq!(claude_binary(&helper), Some(binary.clone()));
        assert_eq!(
            sign_in_hint(Some(&helper)),
            format!("\"{}\" auth login", binary.display())
        );
    }

    #[test]
    fn a_custom_launch_has_no_node_or_helper() {
        let launch = Launch::custom("/bin/echo".into(), vec!["hi".into()]);
        assert_eq!(launch.program, PathBuf::from("/bin/echo"));
        assert_eq!(launch.args, [OsString::from("hi")]);
        assert_eq!(launch.cwd, std::env::current_dir().unwrap());
        assert!(launch.node.is_none() && launch.helper.is_none());
        assert!(lookup(&launch.env, "PATH").is_some());
    }
}
