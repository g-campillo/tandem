//! Swift: SourceKit-LSP from Xcode or the Command Line Tools, found by file path. Discovery never
//! runs `xcrun` or the `/usr/bin/sourcekit-lsp` shim, which can open the install dialog.
//!
//! SourceKit-LSP sees a whole Xcode module only through a buildServer.json. For an Xcode project
//! without one, discovery plans it from files alone: Homebrew's xcode-build-server, the build
//! folder whose `info.plist` names the project (Xcode's DerivedData or an `xcodebuild
//! -derivedDataPath` folder inside the project) and the scheme of its newest build log. The trust
//! fingerprint covers the planned text, and `XcodeSetup::apply` writes it before the server starts.
use super::servers::{
    KindTools, Launch, MissingTools, ServerTools, ToolNote, ToolRoots, fingerprint_hashes,
};
use super::{file_hash, read_tool_config};
use crate::{ToolKind, content_hash};
use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The newest build logs xcode-build-server reads when Tandem sets a project up.
const SEED_LOGS: usize = 20;

pub(crate) fn discover(project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let (server, developer_dir, title) = find_sourcekit(roots).ok_or_else(|| MissingTools {
        message: "SourceKit-LSP was not found. It comes with Xcode or the Command Line Tools."
            .into(),
        install: vec!["xcode-select --install".into()],
    })?;
    let extra_path = roots
        .homebrew
        .iter()
        .map(|prefix| prefix.join("bin"))
        .filter(|bin| bin.is_dir())
        .collect();
    let build_server = project.join("buildServer.json");
    // An xcode-build-server file whose build folder is gone (a cleared DerivedData) gives no
    // flags at all; it is replaced like a missing one.
    let stale = build_server
        .is_file()
        .then(|| read_small(&build_server))
        .flatten()
        .filter(|text| stale_build_root(text).is_some());
    let configured = build_server.is_file() && stale.is_none();
    let (notes, xcode) = if configured || project.join("Package.swift").is_file() {
        (vec![], None)
    } else {
        xcode_plan(project, roots, stale)
    };
    let mut parts = vec![server.display().to_string(), title.clone()];
    let mut hashes = vec![file_hash(&server)?];
    if let Some(setup) = &xcode {
        // The same as once the file is written, so writing it keeps the project trusted.
        parts.push(build_server.display().to_string());
        hashes.push(content_hash(setup.config.as_bytes()));
    } else if build_server.is_file() {
        parts.push(build_server.display().to_string());
        hashes.push(file_hash(&build_server)?);
    }
    let fingerprint = fingerprint_hashes("swift-v1", &parts, &hashes);
    Ok(KindTools {
        kind: ToolKind::Swift,
        label: title.clone(),
        servers: vec![ServerTools {
            prepare: None,
            name: "sourcekit-lsp".into(),
            title,
            languages: vec!["swift".into()],
            launch: Launch::SourceKit {
                server,
                developer_dir,
                extra_path,
                xcode,
            },
        }],
        notes,
        fingerprint,
    })
}

/// The first SourceKit-LSP file of Xcode.app, the other `Xcode*.app` by name, then the Command
/// Line Tools: (server, developer dir, title).
fn find_sourcekit(roots: &ToolRoots) -> Option<(PathBuf, PathBuf, String)> {
    let mut apps: Vec<(String, PathBuf)> = std::fs::read_dir(&roots.applications)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| name.starts_with("Xcode") && name.ends_with(".app"))
        .collect();
    apps.sort_by(|(a, _), (b, _)| (a != "Xcode.app", a).cmp(&(b != "Xcode.app", b)));
    for (name, app) in apps {
        let developer_dir = app.join("Contents/Developer");
        let server =
            developer_dir.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp");
        if server.is_file() {
            let xcode = match xcode_version(&app) {
                Some(version) => format!("Xcode {version}"),
                None => name.trim_end_matches(".app").to_string(),
            };
            return Some((server, developer_dir, format!("SourceKit-LSP ({xcode})")));
        }
    }
    let developer_dir = roots.system.join("Library/Developer/CommandLineTools");
    let server = developer_dir.join("usr/bin/sourcekit-lsp");
    server.is_file().then(|| {
        (
            server,
            developer_dir,
            "SourceKit-LSP (Command Line Tools)".to_string(),
        )
    })
}

/// `CFBundleShortVersionString` from the app's `Contents/version.plist`.
fn xcode_version(app: &Path) -> Option<String> {
    let plist = read_tool_config(&app.join("Contents/version.plist")).ok()?;
    let (_, rest) = plist.split_once("<key>CFBundleShortVersionString</key>")?;
    let (_, rest) = rest.split_once("<string>")?;
    let (version, _) = rest.split_once("</string>")?;
    let version = version.trim();
    (!version.is_empty() && !version.contains('<')).then(|| version.to_string())
}

/// The buildServer.json Tandem writes for an Xcode project that has none, before SourceKit-LSP
/// starts. Planned by discovery; the Swift trust fingerprint covers `config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XcodeSetup {
    /// `<project>/buildServer.json`.
    pub path: PathBuf,
    /// The file's exact text.
    pub config: String,
    /// Homebrew's xcode-build-server, which SourceKit-LSP runs through the file.
    pub tool: PathBuf,
    /// The Xcode build folder whose build logs hold the compiler flags.
    pub build_root: PathBuf,
    pub scheme: String,
    /// xcode-build-server's flags file for this project, build folder and scheme.
    pub flags: PathBuf,
    /// The text of a stale buildServer.json this one replaces.
    pub replaces: Option<String>,
}

impl XcodeSetup {
    fn new(
        project: &Path,
        tool: &Path,
        build_root: &Path,
        scheme: &str,
        home: &Path,
        replaces: Option<String>,
    ) -> Self {
        let mut config = serde_json::to_string_pretty(&json!({
            "name": "xcode build server",
            "version": tool_version(tool),
            "bspVersion": "2.2.0",
            "languages": ["c", "cpp", "objective-c", "objective-cpp", "swift"],
            "argv": [tool],
            "build_root": build_root,
            "scheme": scheme,
            "kind": "xcode",
        }))
        .expect("plain JSON");
        config.push('\n');
        // xcode-build-server's own naming: the project path with every / as -, then
        // compile_file-<scheme>-<md5 of build_root>.
        let digest = Md5::digest(build_root.to_string_lossy().as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let flags = home
            .join("Library/Caches/xcode-build-server")
            .join(project.to_string_lossy().replace('/', "-"))
            .join(format!("compile_file-{scheme}-{hex}"));
        Self {
            path: project.join("buildServer.json"),
            config,
            tool: tool.to_path_buf(),
            build_root: build_root.to_path_buf(),
            scheme: scheme.to_string(),
            flags,
            replaces,
        }
    }

    /// Writes buildServer.json and hides it from Git, unless it is already there with this text,
    /// then lets xcode-build-server read the project's recent build logs when it has no flags
    /// yet. Overwrites only the stale file it `replaces`; fails when the file holds anything else,
    /// as the trust only covers this text.
    pub fn apply(&self) -> Result<()> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) if text == self.config => {}
            Ok(text) if self.replaces.as_ref() == Some(&text) => {
                std::fs::write(&self.path, &self.config)
                    .with_context(|| format!("Could not replace {}", self.path.display()))?;
                if let Err(error) = exclude_from_git(&self.path) {
                    eprintln!("tandem: could not hide buildServer.json from Git: {error:#}");
                }
            }
            Ok(_) => bail!(
                "{} changed after Tandem checked the project. Choose Recheck Installed Tools.",
                self.path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.path)
                    .and_then(|mut file| {
                        std::io::Write::write_all(&mut file, self.config.as_bytes())
                    })
                    .with_context(|| format!("Could not write {}", self.path.display()))?;
                if let Err(error) = exclude_from_git(&self.path) {
                    eprintln!("tandem: could not hide buildServer.json from Git: {error:#}");
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not read {}", self.path.display()));
            }
        }
        if !self.flags.exists() {
            self.read_build_logs();
        }
        Ok(())
    }

    /// Whether xcode-build-server holds compiler flags for the project. Builds that compiled
    /// nothing (the app was up to date) leave none in their logs.
    pub fn flags_found(&self) -> bool {
        // An empty flags file is `[]`; one command alone is hundreds of bytes.
        std::fs::metadata(&self.flags).is_ok_and(|meta| meta.len() > 16)
    }

    /// Runs `xcode-build-server parse --append` on the scheme's newest logs, oldest first. On its
    /// own xcode-build-server starts from the newest log alone, which lacks every module that
    /// build left untouched, and SourceKit-LSP then reports their types as missing. Best effort:
    /// a log that fails to parse is skipped.
    fn read_build_logs(&self) {
        let folder = self.build_root.join("Logs/Build");
        let Some(logs) =
            read_small(&folder.join("LogStoreManifest.plist")).and_then(|text| build_logs(&text))
        else {
            return;
        };
        let mut logs: Vec<(f64, PathBuf)> = logs
            .into_iter()
            .filter(|log| log.scheme == self.scheme)
            .map(|log| (log.stopped, folder.join(log.file)))
            .filter(|(_, path)| path.is_file())
            .collect();
        logs.sort_by(|(a, _), (b, _)| a.total_cmp(b));
        let skip = logs.len().saturating_sub(SEED_LOGS);
        let Some(parent) = self.flags.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let mut path = self
            .tool
            .parent()
            .map(Path::to_path_buf)
            .into_iter()
            .collect::<Vec<_>>();
        path.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
        let path = std::env::join_paths(path).unwrap_or_default();
        for (_, log) in logs.into_iter().skip(skip) {
            let mut command = Command::new(&self.tool);
            command
                .args(["parse", "--append", "--xcactivitylog"])
                .arg(&log)
                .arg("--output")
                .arg(&self.flags)
                .env_clear()
                .env("PATH", &path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for name in ["HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            let _ = command.status();
        }
    }
}

/// Notes and setup for a project without a usable buildServer.json (`stale` holds the text of one
/// whose build folder is gone): none unless it has an Xcode workspace or project at its root or
/// one folder down.
fn xcode_plan(
    project: &Path,
    roots: &ToolRoots,
    stale: Option<String>,
) -> (Vec<ToolNote>, Option<XcodeSetup>) {
    let containers = xcode_containers(project);
    let Some(container) = containers.first() else {
        return (vec![], None);
    };
    let gone = stale
        .as_deref()
        .and_then(stale_build_root)
        .map(|root| {
            format!("buildServer.json points at a build folder that no longer exists ({root}). ")
        })
        .unwrap_or_default();
    let Some(tool) = roots
        .homebrew
        .iter()
        .map(|prefix| prefix.join("bin/xcode-build-server"))
        .find(|tool| tool.is_file())
    else {
        return (
            vec![ToolNote {
                text: format!(
                    "{gone}Install xcode-build-server, then choose Recheck Installed Tools. Until then SourceKit-LSP reads each Swift file on its own and reports types from other files as missing."
                ),
                command: Some("brew install xcode-build-server".into()),
            }],
            None,
        );
    };
    let paths: Vec<PathBuf> = containers.iter().map(|name| project.join(name)).collect();
    let Some(build_root) = build_root(project, &paths, &roots.home) else {
        return (
            vec![ToolNote {
                text: format!(
                    "{gone}Build {container} once in Xcode or with xcodebuild, then choose Recheck Installed Tools. Until then SourceKit-LSP reads each Swift file on its own and reports types from other files as missing."
                ),
                command: None,
            }],
            None,
        );
    };
    let name = Path::new(container.as_str())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(container.as_str());
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let scheme = read_small(&build_root.join("Logs/Build/LogStoreManifest.plist"))
        .and_then(|text| build_logs(&text))
        .and_then(|logs| {
            logs.into_iter()
                .max_by(|a, b| a.stopped.total_cmp(&b.stopped))
                .map(|log| log.scheme)
        })
        .filter(|scheme| !scheme.is_empty())
        .or_else(|| scheme_name(&project.join(container)))
        .unwrap_or_else(|| stem.to_string());
    let shown = build_root
        .strip_prefix(project)
        .unwrap_or(&build_root)
        .display()
        .to_string();
    let verb = if stale.is_some() {
        "replaces"
    } else {
        "writes"
    };
    let setup = XcodeSetup::new(project, &tool, &build_root, &scheme, &roots.home, stale);
    let note = ToolNote {
        text: format!(
            "{gone}Enabling {verb} buildServer.json for scheme {scheme}, with the flags of the builds in {shown}, and hides it from Git. SourceKit-LSP then reads the project as Xcode builds it."
        ),
        command: None,
    };
    (vec![note], Some(setup))
}

/// The Xcode build folder of one of `containers` with the newest build log manifest, from Xcode's
/// `~/Library/Developer/Xcode/DerivedData`, XcodeBuildMCP's
/// `~/Library/Developer/XcodeBuildMCP/workspaces/*/DerivedData`, and the project's own folders
/// three levels deep (`xcodebuild -derivedDataPath`).
fn build_root(project: &Path, containers: &[PathBuf], home: &Path) -> Option<PathBuf> {
    let containers: Vec<PathBuf> = containers
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect();
    let mcp = home.join("Library/Developer/XcodeBuildMCP/workspaces");
    let derived = std::iter::once(home.join("Library/Developer/Xcode/DerivedData")).chain(
        names_in(&mcp)
            .into_iter()
            .map(|name| mcp.join(name).join("DerivedData")),
    );
    let mut candidates: Vec<PathBuf> = derived
        .flat_map(|folder| {
            names_in(&folder)
                .into_iter()
                .map(move |name| folder.join(name))
        })
        .filter(|path| is_build_folder(path))
        .collect();
    candidates.extend(project_build_folders(project));
    candidates
        .into_iter()
        .filter(|folder| {
            read_tool_config(&folder.join("info.plist"))
                .ok()
                .and_then(|text| parse_plist(&text))
                .and_then(|info| Some(PathBuf::from(info.get("WorkspacePath")?.as_str()?)))
                .map(|path| path.canonicalize().unwrap_or(path))
                .is_some_and(|path| containers.contains(&path))
        })
        .filter_map(|folder| {
            let manifest = folder.join("Logs/Build/LogStoreManifest.plist");
            Some((std::fs::metadata(manifest).ok()?.modified().ok()?, folder))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, folder)| folder)
}

/// Build folders inside the project, three levels deep, skipping Git, dependency, package and
/// bundle folders and never descending into a build folder.
fn project_build_folders(project: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        ".git",
        "node_modules",
        "Pods",
        "Carthage",
        ".build",
        ".swiftpm",
    ];
    const BUNDLES: &[&str] = &[
        ".xcodeproj",
        ".xcworkspace",
        ".xcassets",
        ".app",
        ".framework",
        ".bundle",
        ".lproj",
    ];
    let mut found = vec![];
    let mut queue = vec![(project.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((folder, depth)) = queue.pop() {
        for name in names_in(&folder) {
            seen += 1;
            if seen > 5000 {
                return found;
            }
            if SKIP.contains(&name.as_str()) || BUNDLES.iter().any(|end| name.ends_with(end)) {
                continue;
            }
            let path = folder.join(&name);
            if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                continue;
            }
            if is_build_folder(&path) {
                found.push(path);
            } else if depth < 2 {
                queue.push((path, depth + 1));
            }
        }
    }
    found
}

/// The project's Xcode workspaces, then projects, as paths relative to it: at its root, else one
/// folder down (`App/App.xcodeproj`), never inside a bundle or a dependency folder.
fn xcode_containers(project: &Path) -> Vec<String> {
    let in_folder = |folder: &str| -> Vec<String> {
        let mut names = names_in(&project.join(folder));
        names.sort();
        [".xcworkspace", ".xcodeproj"]
            .iter()
            .flat_map(|suffix| names.iter().filter(move |name| name.ends_with(suffix)))
            .map(|name| {
                if folder.is_empty() {
                    name.clone()
                } else {
                    format!("{folder}/{name}")
                }
            })
            .collect()
    };
    let root = in_folder("");
    if !root.is_empty() {
        return root;
    }
    let mut folders: Vec<String> = names_in(project)
        .into_iter()
        .filter(|name| {
            !name.starts_with('.')
                && !["Pods", "Carthage", "node_modules", "build", "DerivedData"]
                    .contains(&name.as_str())
                && !name.contains(".xc")
                && std::fs::symlink_metadata(project.join(name)).is_ok_and(|meta| meta.is_dir())
        })
        .collect();
    folders.sort();
    folders
        .iter()
        .flat_map(|folder| in_folder(folder))
        .collect()
}

/// The missing build folder an xcode-build-server buildServer.json names, if it is missing.
fn stale_build_root(text: &str) -> Option<String> {
    let config: Value = serde_json::from_str(text).ok()?;
    (config.get("kind")?.as_str()? == "xcode").then_some(())?;
    let root = config.get("build_root")?.as_str()?;
    (!Path::new(root).exists()).then(|| root.to_string())
}

/// An Xcode build folder: `info.plist` naming the workspace, and a build log manifest.
fn is_build_folder(path: &Path) -> bool {
    path.join("info.plist").is_file() && path.join("Logs/Build/LogStoreManifest.plist").is_file()
}

/// One entry of a build folder's `Logs/Build/LogStoreManifest.plist`.
#[derive(Debug, PartialEq)]
struct BuildLog {
    file: String,
    scheme: String,
    /// Seconds since 2001, when the build finished.
    stopped: f64,
}

/// The logs a `LogStoreManifest.plist` lists; `None` when it cannot be read.
fn build_logs(manifest: &str) -> Option<Vec<BuildLog>> {
    let plist = parse_plist(manifest)?;
    let logs = plist.get("logs")?.as_object()?;
    Some(
        logs.values()
            .filter_map(|log| {
                let file = log.get("fileName")?.as_str()?;
                // A bare file name, never a path out of the logs folder.
                (!file.contains('/') && file.ends_with(".xcactivitylog")).then_some(())?;
                Some(BuildLog {
                    file: file.to_string(),
                    scheme: log
                        .get("schemeIdentifier-schemeName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    stopped: log
                        .get("timeStoppedRecording")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                })
            })
            .collect(),
    )
}

/// The installed version from Homebrew's `Cellar/xcode-build-server/<version>/bin` behind the
/// link, else "unknown".
fn tool_version(tool: &Path) -> String {
    tool.canonicalize()
        .ok()
        .and_then(|real| {
            let version = real.parent()?.parent()?;
            (version.parent()?.file_name()? == "xcode-build-server")
                .then(|| version.file_name()?.to_str().map(str::to_string))?
        })
        .unwrap_or_else(|| "unknown".into())
}

/// A text file of at most 4 MiB, not followed through a link.
fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    (meta.is_file() && meta.len() <= 4 * 1024 * 1024).then_some(())?;
    std::fs::read_to_string(path).ok()
}

/// Adds `/<path from the repository root>` to the repository's `info/exclude`, which only this
/// clone reads. Nothing to do outside a Git repository.
fn exclude_from_git(file: &Path) -> Result<()> {
    let Some((root, git)) = file
        .ancestors()
        .skip(1)
        .find_map(|dir| Some((dir, dir.join(".git"))).filter(|(_, git)| git.exists()))
    else {
        return Ok(());
    };
    let common = if git.is_dir() {
        git
    } else {
        // A linked worktree: `.git` names its git dir, whose `commondir` names the shared one.
        let text = std::fs::read_to_string(&git)?;
        let Some(dir) = text.trim().strip_prefix("gitdir:") else {
            bail!("{} names no git dir", git.display());
        };
        let dir = root.join(dir.trim());
        match std::fs::read_to_string(dir.join("commondir")) {
            Ok(common) => dir.join(common.trim()),
            Err(_) => dir,
        }
    };
    let relative = file.strip_prefix(root)?;
    let line = format!("/{}", relative.to_string_lossy());
    let exclude = common.join("info/exclude");
    let text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(());
    }
    std::fs::create_dir_all(common.join("info"))?;
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&exclude, format!("{text}{separator}{line}\n"))?;
    Ok(())
}

/// A small reader for XML property lists (dict, array, key, string, integer, real, true, false,
/// date, data) as JSON; `None` for anything else, binary property lists included.
fn parse_plist(text: &str) -> Option<Value> {
    let mut reader = PlistReader { rest: text };
    match reader.tag()? {
        Tag::Open("plist") => {}
        _ => return None,
    }
    let first = reader.tag()?;
    plist_value(&mut reader, first)
}

enum Tag<'a> {
    Open(&'a str),
    Close(&'a str),
    Empty(&'a str),
}

struct PlistReader<'a> {
    rest: &'a str,
}

impl<'a> PlistReader<'a> {
    /// The next tag, skipping text, `<?…?>` and `<!…>`.
    fn tag(&mut self) -> Option<Tag<'a>> {
        loop {
            let start = self.rest.find('<')?;
            let after = &self.rest[start + 1..];
            let end = after.find('>')?;
            let inner = &after[..end];
            self.rest = &after[end + 1..];
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            let name = |body: &'a str| body.split_whitespace().next().unwrap_or_default();
            return Some(match (inner.strip_prefix('/'), inner.strip_suffix('/')) {
                (Some(close), _) => Tag::Close(name(close)),
                (None, Some(empty)) => Tag::Empty(name(empty)),
                (None, None) => Tag::Open(name(inner)),
            });
        }
    }

    /// The unescaped text up to `</name>`.
    fn text(&mut self, name: &str) -> Option<String> {
        let close = format!("</{name}>");
        let end = self.rest.find(&close)?;
        let raw = &self.rest[..end];
        self.rest = &self.rest[end + close.len()..];
        Some(
            raw.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        )
    }
}

fn plist_value(reader: &mut PlistReader, tag: Tag) -> Option<Value> {
    Some(match tag {
        Tag::Empty("dict") => Value::Object(Map::new()),
        Tag::Empty("array") => Value::Array(vec![]),
        Tag::Empty("string") => Value::String(String::new()),
        Tag::Empty("true") => Value::Bool(true),
        Tag::Empty("false") => Value::Bool(false),
        Tag::Open("dict") => {
            let mut map = Map::new();
            loop {
                match reader.tag()? {
                    Tag::Close("dict") => break Value::Object(map),
                    Tag::Open("key") => {
                        let key = reader.text("key")?;
                        let next = reader.tag()?;
                        map.insert(key, plist_value(reader, next)?);
                    }
                    _ => return None,
                }
            }
        }
        Tag::Open("array") => {
            let mut items = vec![];
            loop {
                match reader.tag()? {
                    Tag::Close("array") => break Value::Array(items),
                    next => items.push(plist_value(reader, next)?),
                }
            }
        }
        Tag::Open(name @ ("string" | "date" | "data")) => Value::String(reader.text(name)?),
        Tag::Open("integer") => Value::from(reader.text("integer")?.trim().parse::<i64>().ok()?),
        Tag::Open("real") => Value::from(reader.text("real")?.trim().parse::<f64>().ok()?),
        _ => return None,
    })
}

/// The first shared scheme of the container, else the first scheme of any of its users.
fn scheme_name(container: &Path) -> Option<String> {
    let first = |folders: Vec<PathBuf>| {
        let mut schemes: Vec<PathBuf> = folders
            .iter()
            .flat_map(|folder| {
                names_in(folder)
                    .into_iter()
                    .filter(|name| name.ends_with(".xcscheme"))
                    .map(move |name| folder.join(name))
            })
            .collect();
        schemes.sort();
        let scheme = schemes.first()?;
        Some(scheme.file_stem()?.to_string_lossy().to_string())
    };
    first(vec![container.join("xcshareddata/xcschemes")]).or_else(|| {
        let mut users = names_in(&container.join("xcuserdata"));
        users.sort();
        first(
            users
                .into_iter()
                .map(|user| container.join("xcuserdata").join(user).join("xcschemes"))
                .collect(),
        )
    })
}

/// UTF-8 entry names of a folder; empty when it cannot be read.
fn names_in(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolKind;
    use crate::tools::servers::{Launch, ToolNote, discover_kind};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn roots(temp: &Path) -> ToolRoots {
        ToolRoots {
            home: temp.join("home"),
            homebrew: vec![temp.join("opt-homebrew"), temp.join("usr-local")],
            applications: temp.join("Applications"),
            system: temp.join("system"),
        }
    }
    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn plist(version: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>BuildVersion</key>\n\t<string>7</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n</dict>\n</plist>\n"
        )
    }
    /// An Xcode app with its version and SourceKit-LSP; returns the server path.
    fn xcode(roots: &ToolRoots, app: &str, version: &str) -> PathBuf {
        let contents = roots.applications.join(app).join("Contents");
        write(&contents.join("version.plist"), &plist(version));
        let server =
            contents.join("Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp");
        write(&server, &format!("sourcekit-lsp from {app}"));
        server
    }
    fn command_line_tools(roots: &ToolRoots) -> PathBuf {
        let server = roots
            .system
            .join("Library/Developer/CommandLineTools/usr/bin/sourcekit-lsp");
        write(&server, "sourcekit-lsp from the Command Line Tools");
        server
    }
    fn project(temp: &Path) -> PathBuf {
        let project = temp.join("project");
        std::fs::create_dir_all(&project).unwrap();
        project
    }

    #[test]
    fn finds_sourcekit_in_xcode_first() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        xcode(&roots, "Xcode-beta.app", "27.0");
        let server = xcode(&roots, "Xcode.app", "26.1");
        std::fs::create_dir_all(roots.homebrew[0].join("bin")).unwrap();
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.kind, ToolKind::Swift);
        assert_eq!(tools.label, "SourceKit-LSP (Xcode 26.1)");
        assert_eq!(tools.servers.len(), 1);
        let found = &tools.servers[0];
        assert_eq!(found.name, "sourcekit-lsp");
        assert_eq!(found.title, "SourceKit-LSP (Xcode 26.1)");
        assert_eq!(found.languages, vec!["swift".to_string()]);
        assert_eq!(
            found.launch,
            Launch::SourceKit {
                server,
                developer_dir: roots.applications.join("Xcode.app/Contents/Developer"),
                extra_path: vec![roots.homebrew[0].join("bin")],
                xcode: None,
            }
        );
        assert!(tools.notes.is_empty());
        assert!(!tools.fingerprint.is_empty());
    }

    #[test]
    fn other_xcode_apps_follow_in_name_order() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write(
            &roots.applications.join("Xcode.app/Contents/version.plist"),
            &plist("26.1"),
        );
        xcode(&roots, "Xcode-beta.app", "27.0");
        xcode(&roots, "Xcode-26.0.app", "26.0");
        let server = xcode(&roots, "Xcode-16.4.app", "16.4");
        std::fs::create_dir_all(roots.applications.join("Xcodes.txt")).unwrap();
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.label, "SourceKit-LSP (Xcode 16.4)");
        let Launch::SourceKit { server: found, .. } = &tools.servers[0].launch else {
            panic!("expected a SourceKit launch");
        };
        assert_eq!(found, &server);
    }

    #[test]
    fn falls_back_to_the_command_line_tools() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write(
            &roots.applications.join("Xcode.app/Contents/version.plist"),
            &plist("26.1"),
        );
        let server = command_line_tools(&roots);
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.label, "SourceKit-LSP (Command Line Tools)");
        assert_eq!(tools.servers[0].title, "SourceKit-LSP (Command Line Tools)");
        assert_eq!(
            tools.servers[0].launch,
            Launch::SourceKit {
                server,
                developer_dir: roots.system.join("Library/Developer/CommandLineTools"),
                extra_path: vec![],
                xcode: None,
            }
        );
    }

    #[test]
    fn reports_the_install_command_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let missing = discover_kind(ToolKind::Swift, &project(temp.path()), &roots)
            .expect("Swift is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing.message,
            "SourceKit-LSP was not found. It comes with Xcode or the Command Line Tools."
        );
        assert_eq!(missing.install, vec!["xcode-select --install".to_string()]);
        assert!(discover_kind(ToolKind::Rust, temp.path(), &roots).is_none());
    }

    #[test]
    fn no_notes_for_packages_or_configured_projects() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let project = project(temp.path());
        write(
            &project.join("App.xcodeproj/xcshareddata/xcschemes/App.xcscheme"),
            "<Scheme/>",
        );
        assert_eq!(discover(&project, &roots).unwrap().notes.len(), 1);

        write(
            &project.join("Package.swift"),
            "// swift-tools-version:5.9\n",
        );
        assert!(discover(&project, &roots).unwrap().notes.is_empty());

        std::fs::remove_file(project.join("Package.swift")).unwrap();
        write(&project.join("buildServer.json"), "{}");
        assert!(discover(&project, &roots).unwrap().notes.is_empty());
    }

    #[test]
    fn build_server_json_changes_the_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let server = command_line_tools(&roots);
        let project = project(temp.path());
        let fingerprint = || discover(&project, &roots).unwrap().fingerprint;
        let plain = fingerprint();
        assert_eq!(fingerprint(), plain);

        write(
            &project.join("buildServer.json"),
            "{\"argv\":[\"/opt/homebrew/bin/xcode-build-server\"]}",
        );
        let configured = fingerprint();
        assert_ne!(configured, plain);

        write(
            &project.join("buildServer.json"),
            "{\"argv\":[\"/tmp/other-build-server\"]}",
        );
        assert_ne!(fingerprint(), configured);

        std::fs::remove_file(project.join("buildServer.json")).unwrap();
        assert_eq!(fingerprint(), plain);
        write(&server, "a different sourcekit-lsp");
        assert_ne!(fingerprint(), plain);
    }

    /// An Xcode build folder for `workspace` whose manifest lists `logs` as
    /// (file, scheme, finished), each log file written beside it.
    fn build_folder(folder: &Path, workspace: &Path, logs: &[(&str, &str, f64)]) {
        write(
            &folder.join("info.plist"),
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>LastAccessedDate</key>\n\t<date>2026-11-18T06:24:35Z</date>\n\t<key>WorkspacePath</key>\n\t<string>{}</string>\n</dict>\n</plist>\n",
                workspace.display().to_string().replace('&', "&amp;")
            ),
        );
        let entries: String = logs
            .iter()
            .map(|(file, scheme, stopped)| {
                write(&folder.join("Logs/Build").join(file), "log");
                let id = file.trim_end_matches(".xcactivitylog");
                format!(
                    "\t\t<key>{id}</key>\n\t\t<dict>\n\t\t\t<key>fileName</key>\n\t\t\t<string>{file}</string>\n\t\t\t<key>hasPrimaryLog</key>\n\t\t\t<true/>\n\t\t\t<key>primaryObservable</key>\n\t\t\t<dict>\n\t\t\t\t<key>totalNumberOfErrors</key>\n\t\t\t\t<integer>0</integer>\n\t\t\t</dict>\n\t\t\t<key>schemeIdentifier-schemeName</key>\n\t\t\t<string>{scheme}</string>\n\t\t\t<key>timeStoppedRecording</key>\n\t\t\t<real>{stopped}</real>\n\t\t</dict>\n"
                )
            })
            .collect();
        write(
            &folder.join("Logs/Build/LogStoreManifest.plist"),
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>logFormatVersion</key>\n\t<integer>13</integer>\n\t<key>logs</key>\n\t<dict>\n{entries}\t</dict>\n</dict>\n</plist>\n"
            ),
        );
    }
    fn age(path: &Path, seconds: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }
    /// A fake xcode-build-server that records each run and creates its `--output` file.
    fn fake_tool(path: &Path, record: &Path) {
        write(
            path,
            &format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\nfor a; do last=$a; done\ntouch \"$last\"\n",
                record.display()
            ),
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn setup(tools: &KindTools) -> Option<XcodeSetup> {
        let Launch::SourceKit { xcode, .. } = &tools.servers[0].launch else {
            panic!("expected a SourceKit launch");
        };
        xcode.clone()
    }

    #[test]
    fn xcode_projects_say_what_their_setup_still_needs() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let project = project(temp.path());
        let plain = discover(&project, &roots).unwrap().fingerprint;
        write(
            &project.join("My App.xcodeproj/xcshareddata/xcschemes/My App.xcscheme"),
            "<Scheme/>",
        );
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(
            tools.notes,
            vec![ToolNote {
                text: "Install xcode-build-server, then choose Recheck Installed Tools. Until then SourceKit-LSP reads each Swift file on its own and reports types from other files as missing.".into(),
                command: Some("brew install xcode-build-server".into()),
            }]
        );
        assert_eq!(setup(&tools), None);
        assert_eq!(tools.fingerprint, plain);

        write(
            &roots.homebrew[1].join("bin/xcode-build-server"),
            "#!/bin/sh\n",
        );
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(
            tools.notes,
            vec![ToolNote {
                text: "Build My App.xcodeproj once in Xcode or with xcodebuild, then choose Recheck Installed Tools. Until then SourceKit-LSP reads each Swift file on its own and reports types from other files as missing.".into(),
                command: None,
            }]
        );
        assert_eq!(setup(&tools), None);
        assert_eq!(tools.fingerprint, plain);

        // Another project's build folder does not count.
        build_folder(
            &roots
                .home
                .join("Library/Developer/Xcode/DerivedData/Other-abc"),
            &temp.path().join("other/Other.xcodeproj"),
            &[("A.xcactivitylog", "Other", 1.0)],
        );
        assert_eq!(setup(&discover(&project, &roots).unwrap()), None);
    }

    #[test]
    fn xcode_projects_plan_a_build_server_from_their_newest_build_folder() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let tool = roots.homebrew[1].join("bin/xcode-build-server");
        write(&tool, "#!/bin/sh\n");
        let project = project(temp.path());
        let container = project.join("My App.xcodeproj");
        write(
            &container.join("xcshareddata/xcschemes/My App.xcscheme"),
            "<Scheme/>",
        );
        let derived = roots
            .home
            .join("Library/Developer/Xcode/DerivedData/My_App-abc");
        build_folder(
            &derived,
            &container,
            &[("Old.xcactivitylog", "My App", 5.0)],
        );
        let inside = project.join("build/dd");
        build_folder(
            &inside,
            &container,
            &[
                ("B.xcactivitylog", "Alpha", 20.0),
                ("A.xcactivitylog", "My App", 10.0),
            ],
        );
        age(&derived.join("Logs/Build/LogStoreManifest.plist"), 60);

        let tools = discover(&project, &roots).unwrap();
        let planned = setup(&tools).expect("a planned buildServer.json");
        assert_eq!(planned.path, project.join("buildServer.json"));
        assert_eq!(planned.tool, tool);
        assert_eq!(planned.build_root, inside);
        assert_eq!(planned.scheme, "Alpha");
        let hex: String = Md5::digest(inside.to_string_lossy().as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            planned.flags,
            roots
                .home
                .join("Library/Caches/xcode-build-server")
                .join(project.to_string_lossy().replace('/', "-"))
                .join(format!("compile_file-Alpha-{hex}"))
        );
        let config: Value = serde_json::from_str(&planned.config).unwrap();
        assert_eq!(
            config,
            json!({
                "name": "xcode build server",
                "version": "unknown",
                "bspVersion": "2.2.0",
                "languages": ["c", "cpp", "objective-c", "objective-cpp", "swift"],
                "argv": [tool],
                "build_root": inside,
                "scheme": "Alpha",
                "kind": "xcode",
            })
        );
        assert_eq!(
            tools.notes,
            vec![ToolNote {
                text: "Enabling writes buildServer.json for scheme Alpha, with the flags of the builds in build/dd, and hides it from Git. SourceKit-LSP then reads the project as Xcode builds it.".into(),
                command: None,
            }]
        );

        // Xcode's own DerivedData once it holds the newest build.
        age(&inside.join("Logs/Build/LogStoreManifest.plist"), 120);
        let planned = setup(&discover(&project, &roots).unwrap()).unwrap();
        assert_eq!(planned.build_root, derived);
        assert_eq!(planned.scheme, "My App");
    }

    #[test]
    fn workspaces_come_first_and_scheme_files_name_the_scheme_without_logs() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        write(
            &roots.homebrew[0].join("bin/xcode-build-server"),
            "#!/bin/sh\n",
        );
        let project = project(temp.path());
        write(
            &project.join("Shop.xcodeproj/xcshareddata/xcschemes/Shop.xcscheme"),
            "<Scheme/>",
        );
        write(
            &project.join("Shop.xcworkspace/xcuserdata/me.xcuserdatad/xcschemes/Zeta.xcscheme"),
            "<Scheme/>",
        );
        write(
            &project.join("Shop.xcworkspace/xcuserdata/me.xcuserdatad/xcschemes/Beta.xcscheme"),
            "<Scheme/>",
        );
        build_folder(
            &project.join("DerivedData"),
            &project.join("Shop.xcworkspace"),
            &[],
        );
        assert_eq!(
            setup(&discover(&project, &roots).unwrap()).unwrap().scheme,
            "Beta"
        );

        std::fs::remove_dir_all(project.join("Shop.xcworkspace/xcuserdata")).unwrap();
        assert_eq!(
            setup(&discover(&project, &roots).unwrap()).unwrap().scheme,
            "Shop"
        );
    }

    #[test]
    fn writing_the_planned_build_server_keeps_the_project_trusted() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let tool = roots.homebrew[0].join("bin/xcode-build-server");
        let record = temp.path().join("runs.txt");
        fake_tool(&tool, &record);
        let project = project(temp.path());
        let container = project.join("App.xcodeproj");
        write(&container.join("project.pbxproj"), "");
        write(&project.join(".git/info/exclude"), "# local");
        let build = project.join("build");
        build_folder(
            &build,
            &container,
            &[
                ("New.xcactivitylog", "App", 30.0),
                ("Tests.xcactivitylog", "AppTests", 20.0),
                ("Old.xcactivitylog", "App", 10.0),
                ("Gone.xcactivitylog", "App", 5.0),
            ],
        );
        std::fs::remove_file(build.join("Logs/Build/Gone.xcactivitylog")).unwrap();

        let before = discover(&project, &roots).unwrap();
        let planned = setup(&before).unwrap();
        planned.apply().unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("buildServer.json")).unwrap(),
            planned.config
        );
        assert_eq!(
            std::fs::read_to_string(project.join(".git/info/exclude")).unwrap(),
            "# local\n/buildServer.json\n"
        );
        let logs = build.join("Logs/Build");
        let run = |log: &str| {
            format!(
                "parse --append --xcactivitylog {} --output {}",
                logs.join(log).display(),
                planned.flags.display()
            )
        };
        assert_eq!(
            std::fs::read_to_string(&record).unwrap(),
            format!(
                "{}\n{}\n",
                run("Old.xcactivitylog"),
                run("New.xcactivitylog")
            )
        );

        let after = discover(&project, &roots).unwrap();
        assert_eq!(after.fingerprint, before.fingerprint);
        assert!(after.notes.is_empty());
        assert_eq!(setup(&after), None);

        // Again: nothing to write, add or read.
        planned.apply().unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join(".git/info/exclude")).unwrap(),
            "# local\n/buildServer.json\n"
        );
        assert_eq!(std::fs::read_to_string(&record).unwrap().lines().count(), 2);

        // A buildServer.json the trust does not cover is never used.
        write(
            &project.join("buildServer.json"),
            "{\"argv\":[\"/tmp/other\"]}",
        );
        let error = planned.apply().unwrap_err().to_string();
        assert!(
            error.contains("changed after Tandem checked the project"),
            "{error}"
        );
    }

    #[test]
    fn nested_projects_and_xcodebuildmcp_builds_are_found() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        write(
            &roots.homebrew[0].join("bin/xcode-build-server"),
            "#!/bin/sh\n",
        );
        let project = project(temp.path());
        let container = project.join("App/Finance.xcodeproj");
        write(&container.join("project.pbxproj"), "");
        // Dependency and bundle folders are not searched.
        write(&project.join("Pods/Pods.xcodeproj/project.pbxproj"), "");
        let tools = discover(&project, &roots).unwrap();
        assert!(
            tools.notes[0]
                .text
                .starts_with("Build App/Finance.xcodeproj once"),
            "{}",
            tools.notes[0].text
        );

        let mcp = roots.home.join(
            "Library/Developer/XcodeBuildMCP/workspaces/finance-ab12/DerivedData/Finance-cd34",
        );
        build_folder(&mcp, &container, &[("A.xcactivitylog", "Finance", 3.0)]);
        let planned = setup(&discover(&project, &roots).unwrap()).unwrap();
        assert_eq!(planned.build_root, mcp);
        assert_eq!(planned.scheme, "Finance");
        assert_eq!(planned.replaces, None);
    }

    #[test]
    fn a_build_server_whose_build_folder_is_gone_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let tool = roots.homebrew[0].join("bin/xcode-build-server");
        fake_tool(&tool, &temp.path().join("runs.txt"));
        let project = project(temp.path());
        let container = project.join("App.xcodeproj");
        write(&container.join("project.pbxproj"), "");
        let gone = temp.path().join("DerivedData/App-old");
        let stale = format!(
            "{{\"name\": \"xcode build server\", \"argv\": [\"/opt/homebrew/bin/xcode-build-server\"], \"build_root\": \"{}\", \"scheme\": \"App\", \"kind\": \"xcode\"}}",
            gone.display()
        );
        write(&project.join("buildServer.json"), &stale);
        let configured = discover(&project, &roots).unwrap();

        // Nothing built since: a note, and the stale file is what runs and what trust covers.
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(setup(&tools), None);
        assert!(tools.notes[0].text.starts_with(&format!(
            "buildServer.json points at a build folder that no longer exists ({}). Build App.xcodeproj once",
            gone.display()
        )));
        assert_eq!(tools.fingerprint, configured.fingerprint);

        let build = project.join("build");
        build_folder(&build, &container, &[("A.xcactivitylog", "App", 1.0)]);
        let tools = discover(&project, &roots).unwrap();
        let planned = setup(&tools).unwrap();
        assert_eq!(planned.replaces.as_deref(), Some(stale.as_str()));
        assert!(
            tools.notes[0]
                .text
                .contains("Enabling replaces buildServer.json for scheme App")
        );
        planned.apply().unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("buildServer.json")).unwrap(),
            planned.config
        );
        let after = discover(&project, &roots).unwrap();
        assert_eq!(after.fingerprint, tools.fingerprint);
        assert_eq!(setup(&after), None);

        // A file that is neither the stale one nor the planned one is left alone.
        write(&project.join("buildServer.json"), "{\"kind\": \"manual\"}");
        assert!(planned.apply().is_err());
        // A build server that is not xcode-build-server's is never stale.
        assert!(stale_build_root("{\"kind\": \"manual\", \"build_root\": \"/gone\"}").is_none());
    }

    #[test]
    fn empty_flags_mean_the_logs_compiled_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let flags = temp.path().join("compile_file-App-0");
        let setup = XcodeSetup::new(
            temp.path(),
            Path::new("/opt/homebrew/bin/xcode-build-server"),
            &temp.path().join("build"),
            "App",
            temp.path(),
            None,
        );
        let setup = XcodeSetup { flags, ..setup };
        assert!(!setup.flags_found());
        write(&setup.flags, "[]");
        assert!(!setup.flags_found());
        write(
            &setup.flags,
            "[{\"command\": \"swiftc -module-name App a.swift\", \"file\": \"a.swift\"}]",
        );
        assert!(setup.flags_found());
    }

    #[test]
    fn linked_worktrees_hide_the_file_in_the_shared_git_folder() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let tree = temp.path().join("tree");
        std::fs::create_dir_all(main.join(".git/worktrees/tree")).unwrap();
        write(&main.join(".git/worktrees/tree/commondir"), "../..\n");
        write(
            &tree.join(".git"),
            &format!("gitdir: {}\n", main.join(".git/worktrees/tree").display()),
        );
        let file = tree.join("ios/buildServer.json");
        write(&file, "{}");
        exclude_from_git(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(main.join(".git/info/exclude")).unwrap(),
            "/ios/buildServer.json\n"
        );
        let outside = temp.path().join("plain/buildServer.json");
        write(&outside, "{}");
        exclude_from_git(&outside).unwrap();
    }

    #[test]
    fn reads_xml_property_lists() {
        let text = "<?xml version=\"1.0\"?>\n<!DOCTYPE plist>\n<plist version=\"1.0\">\n<dict>\n\t<key>a &amp; b</key>\n\t<string>&lt;nil&gt;</string>\n\t<key>n</key><integer>-3</integer>\n\t<key>r</key><real>816675875.5</real>\n\t<key>t</key><true/>\n\t<key>e</key><string/>\n\t<key>list</key>\n\t<array>\n\t\t<string>x</string>\n\t\t<dict/>\n\t\t<false/>\n\t</array>\n</dict>\n</plist>\n";
        assert_eq!(
            parse_plist(text),
            Some(json!({
                "a & b": "<nil>",
                "n": -3,
                "r": 816675875.5,
                "t": true,
                "e": "",
                "list": ["x", {}, false],
            }))
        );
        assert_eq!(parse_plist("bplist00\u{1}"), None);
        assert_eq!(
            parse_plist("<plist><dict><key>a</key></dict></plist>"),
            None
        );
    }

    #[test]
    fn the_tool_version_comes_from_its_cellar_folder() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp
            .path()
            .join("Cellar/xcode-build-server/1.3.0/bin/xcode-build-server");
        write(&real, "#!/bin/sh\n");
        let link = temp.path().join("bin/xcode-build-server");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(tool_version(&link), "1.3.0");
        let plain = temp.path().join("other/xcode-build-server");
        write(&plain, "#!/bin/sh\n");
        assert_eq!(tool_version(&plain), "unknown");
    }
}
