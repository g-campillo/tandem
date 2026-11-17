mod servers;
mod swift;
pub use servers::*;

use crate::content_hash;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn read_tool_config(path: &Path) -> Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Tool configuration could not be opened without following links")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 65536,
        "Tool configuration exceeds its 64 KiB limit or is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "Hard-linked tool configuration is not supported"
        );
    }
    let mut text = String::new();
    file.take(65537).read_to_string(&mut text)?;
    ensure!(
        text.len() <= 65536,
        "Tool configuration grew beyond its limit"
    );
    Ok(text)
}
fn named_toolchain(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 100
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RustTools {
    pub toolchain: PathBuf,
    pub rust_analyzer: PathBuf,
    pub cargo: PathBuf,
    pub rustc: PathBuf,
    pub rustfmt: PathBuf,
    pub label: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectTools {
    pub trusted_fingerprint: Option<String>,
    pub recipe_grants: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub python_trusted_fingerprint: Option<String>,
    #[serde(default)]
    pub python_interpreter: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PythonTools {
    pub interpreter: PathBuf,
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub environment: PathBuf,
    pub node: Option<PathBuf>,
    pub server: Option<PathBuf>,
    pub label: String,
    pub fingerprint: String,
}
impl PythonTools {
    pub fn environment_fingerprint(&self) -> Result<String> {
        let mut metadata = std::collections::BTreeMap::new();
        let configuration = self.environment.join("pyvenv.cfg");
        if configuration.exists() {
            metadata.insert(
                configuration.display().to_string(),
                content_hash(read_tool_config(&configuration)?.as_bytes()),
            );
        }
        let mut total = 0u64;
        for base in [self.prefix.join("lib"), self.environment.join("lib")] {
            if !base.is_dir() {
                continue;
            }
            for version in std::fs::read_dir(&base)?.filter_map(Result::ok) {
                let site = version.path().join("site-packages");
                if !site.is_dir() {
                    continue;
                }
                for (index, entry) in std::fs::read_dir(site)?.enumerate() {
                    ensure!(
                        index < 2048,
                        "Python environment exceeds its metadata entry budget"
                    );
                    let entry = entry?;
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    let files = if name.ends_with(".dist-info") {
                        vec![path.join("METADATA"), path.join("RECORD")]
                    } else if name.ends_with(".pth") {
                        vec![path]
                    } else {
                        continue;
                    };
                    for file in files {
                        if !file.is_file() {
                            continue;
                        }
                        let size = file.metadata()?.len();
                        total += size;
                        ensure!(
                            size <= 2 * 1024 * 1024 && total <= 32 * 1024 * 1024,
                            "Python environment metadata exceeds the verification budget"
                        );
                        metadata.insert(file.display().to_string(), file_hash(&file)?);
                    }
                }
            }
        }
        Ok(content_hash(serde_json::to_string(&metadata)?.as_bytes()))
    }
    pub fn discover(project: &Path, selected: Option<&Path>) -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
        let interpreter = if let Some(selected) = selected {
            ensure!(
                selected.is_absolute(),
                "Choose an absolute Python executable path"
            );
            selected.to_path_buf()
        } else if project.join(".venv/bin/python").is_file() {
            project.join(".venv/bin/python")
        } else {
            let local = project.join(".python-version");
            let selection = if local.symlink_metadata().is_ok() {
                read_tool_config(&local)?
            } else {
                read_tool_config(&home.join(".pyenv/version")).unwrap_or_default()
            };
            let requested = selection.trim().to_string();
            ensure!(
                named_toolchain(&requested),
                "Select an installed Python interpreter in Tools"
            );
            home.join(".pyenv/versions")
                .join(requested)
                .join("bin/python3")
        };
        ensure!(
            interpreter.is_file(),
            "Selected Python interpreter is not installed; choose an existing executable"
        );
        let executable = interpreter.canonicalize()?;
        let mut prefix = executable
            .parent()
            .and_then(Path::parent)
            .context("Python installation path is unsupported")?
            .to_path_buf();
        let environment = interpreter
            .parent()
            .and_then(Path::parent)
            .context("Python environment has no prefix")?
            .to_path_buf();
        let configuration = environment.join("pyvenv.cfg");
        if configuration.exists() {
            let config = read_tool_config(&configuration)?;
            if let Some(base) = config
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(key, _)| key.trim() == "home")
                .map(|(_, value)| PathBuf::from(value.trim()))
            {
                ensure!(
                    base.is_absolute() && base.is_dir() && base.join("python3").is_file(),
                    "Virtualenv base Python installation is unavailable"
                );
                prefix = base
                    .canonicalize()?
                    .parent()
                    .context("Virtualenv base Python prefix is missing")?
                    .to_path_buf();
            }
        }
        ensure!(
            prefix != home && prefix != Path::new("/"),
            "Select a Python installation with a dedicated runtime directory"
        );
        let mut nodes = std::fs::read_dir(home.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect::<Vec<_>>();
        nodes.sort_by_key(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| semver::Version::parse(s.trim_start_matches('v')).ok())
        });
        let node_root = nodes.iter().rev().find(|p| {
            p.join("bin/node").is_file()
                && p.join("lib/node_modules/pyright/langserver.index.js")
                    .is_file()
        });
        let (node, server, version) = if let Some(root) = node_root {
            let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
                root.join("lib/node_modules/pyright/package.json"),
            )?)?;
            (
                Some(root.join("bin/node")),
                Some(root.join("lib/node_modules/pyright/langserver.index.js")),
                data["version"].as_str().unwrap_or("unknown").to_string(),
            )
        } else {
            (None, None, "not installed".into())
        };
        let label = format!(
            "Python {} · Pyright {version}",
            prefix.file_name().unwrap_or_default().to_string_lossy()
        );
        let mut identity = format!(
            "python-v1:{}:{}:{}",
            interpreter.display(),
            file_hash(&executable)?,
            version
        );
        if let Some(node) = &node {
            identity.push_str(&file_hash(node)?);
        }
        Ok(Self {
            interpreter,
            executable,
            prefix,
            environment,
            node,
            server,
            label,
            fingerprint: content_hash(identity.as_bytes()),
        })
    }
    pub fn read_roots(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.prefix.clone()];
        if self.environment != self.prefix {
            for relative in ["bin", "lib", "pyvenv.cfg"] {
                let path = self.environment.join(relative);
                if path.exists() {
                    paths.push(path);
                }
            }
        }
        paths
    }
}

impl RustTools {
    /// Inspect installed files only. Discovery never invokes rustup or installs tools.
    pub fn discover(project: &Path) -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
        Self::discover_in(&home, project)
    }
    pub fn discover_in(home: &Path, project: &Path) -> Result<Self> {
        let rustup = home.join(".rustup");
        let directory = rustup.join("toolchains");
        let mut installed: Vec<PathBuf> = std::fs::read_dir(&directory)
            .context("No installed Rust toolchains were found")?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join("bin/rustc").is_file() && path.join("bin/cargo").is_file())
            .collect();
        installed.sort_by_key(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| semver::Version::parse(s.split('-').next().unwrap_or("")).ok())
        });
        let requested = if project.join("rust-toolchain.toml").exists() {
            let data: toml::Value = toml::from_str(&read_tool_config(
                &project.join("rust-toolchain.toml"),
            )?)
            .map_err(|_| anyhow::anyhow!("rust-toolchain.toml is invalid; inspect it locally"))?;
            data.get("toolchain")
                .and_then(|v| v.get("channel"))
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        } else if project.join("rust-toolchain").exists() {
            Some(
                read_tool_config(&project.join("rust-toolchain"))?
                    .trim()
                    .to_string(),
            )
        } else {
            std::fs::read_to_string(rustup.join("settings.toml"))
                .ok()
                .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
                .and_then(|v| {
                    v.get("default_toolchain")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                })
        };
        let toolchain = if let Some(requested) = requested {
            ensure!(
                named_toolchain(&requested),
                "Unsupported Rust toolchain name; select an installed named toolchain"
            );
            installed.iter().find(|path| path.file_name().and_then(|s| s.to_str()).is_some_and(|name| name == requested || name.starts_with(&format!("{requested}-"))))
                .cloned().ok_or_else(|| anyhow::anyhow!("The requested Rust toolchain is not installed. Check the project toolchain configuration and install it explicitly; Tandem will not auto-install it."))?
        } else {
            installed
                .last()
                .cloned()
                .context("No installed Rust toolchain")?
        };
        let rust_analyzer = if toolchain.join("bin/rust-analyzer").is_file() {
            toolchain.join("bin/rust-analyzer")
        } else {
            installed
                .iter()
                .rev()
                .map(|path| path.join("bin/rust-analyzer"))
                .find(|path| path.is_file())
                .context("rust-analyzer is not installed")?
        };
        let cargo = toolchain.join("bin/cargo");
        let rustc = toolchain.join("bin/rustc");
        let rustfmt = toolchain.join("bin/rustfmt");
        let label = toolchain
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut identity = format!("rust-tools-v1:{label}:{}", rust_analyzer.display());
        for path in [&cargo, &rustc, &rust_analyzer] {
            identity.push_str(&file_hash(path)?);
        }
        Ok(Self {
            toolchain,
            rust_analyzer,
            cargo,
            rustc,
            rustfmt,
            label,
            fingerprint: content_hash(identity.as_bytes()),
        })
    }
}

pub fn file_hash(path: &Path) -> Result<String> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Tool not found: {}", path.display()))?;
    ensure!(
        metadata.is_file() && metadata.len() <= 2 * 1024 * 1024 * 1024,
        "Unsupported tool executable size"
    );
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn normalized_relative(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty()
        || !path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        bail!("Path must stay inside the project");
    }
    Ok(path.to_str().context("Paths must be UTF-8")?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_configuration_is_bounded_and_cannot_follow_links_to_private_data() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private");
        std::fs::write(&private, "private fixture value").unwrap();
        let link = temp.path().join("rust-toolchain");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        assert!(read_tool_config(&link).is_err());
        let hard = temp.path().join("hard-config");
        std::fs::hard_link(&private, &hard).unwrap();
        assert!(read_tool_config(&hard).is_err());
        let large = temp.path().join("large");
        std::fs::write(&large, vec![b'a'; 65537]).unwrap();
        assert!(read_tool_config(&large).is_err());
        assert!(!named_toolchain("private=value"));
        assert!(!named_toolchain("../../outside"));
        assert!(!named_toolchain(".."));
        assert!(named_toolchain("1.100.0-aarch64-apple-darwin"));
    }
}
