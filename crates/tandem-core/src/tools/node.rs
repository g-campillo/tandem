//! Global npm packages: language servers installed with `npm install -g` under nvm or Homebrew's
//! Node.js, found by file path. Discovery never runs `node` or `npm`.
use super::servers::ToolRoots;
use anyhow::{Context, ensure};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest package.json Tandem reads.
const PACKAGE_JSON_LIMIT: u64 = 1024 * 1024;

/// A global npm package and the Node.js that installed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodePackage {
    pub node: PathBuf,
    pub dir: PathBuf,
    pub version: String,
}

/// `package` (e.g. "@vue/language-server") under nvm's installs, newest Node version first
/// (<home>/.nvm/versions/node/v*/lib/node_modules/<package>, sorted with semver like
/// PythonTools::discover does for Pyright), then each Homebrew prefix's lib/node_modules/<package>.
/// Requires the sibling bin/node file and a package.json. `dir` is canonicalised.
pub fn global_package(roots: &ToolRoots, package: &str) -> Option<NodePackage> {
    let mut installs: Vec<PathBuf> = std::fs::read_dir(roots.home.join(".nvm/versions/node"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    // Newest first; folders that are not versions go last.
    installs.sort_by_key(|install| {
        std::cmp::Reverse(
            install
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| semver::Version::parse(name.trim_start_matches('v')).ok()),
        )
    });
    installs
        .into_iter()
        .chain(roots.homebrew.iter().cloned())
        .find_map(|root| {
            let node = root.join("bin/node");
            let dir = root.join("lib/node_modules").join(package);
            if !node.is_file() || !dir.join("package.json").is_file() {
                return None;
            }
            let dir = dir.canonicalize().ok()?;
            let version = package_version(&dir).ok()?;
            Some(NodePackage { node, dir, version })
        })
}

/// "version" from <dir>/package.json: opened with O_NOFOLLOW on the last component, a regular file of
/// at most 1 MiB. No hard-link check, because pnpm hard-links package files from its store.
pub(crate) fn package_version(dir: &Path) -> anyhow::Result<String> {
    let path = dir.join("package.json");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).with_context(|| {
        format!(
            "{} could not be opened without following links",
            path.display()
        )
    })?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= PACKAGE_JSON_LIMIT,
        "{} exceeds its 1 MiB limit or is not a regular file",
        path.display()
    );
    let mut text = String::new();
    file.take(PACKAGE_JSON_LIMIT + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= PACKAGE_JSON_LIMIT,
        "{} grew beyond its limit",
        path.display()
    );
    let data: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let version = data["version"]
        .as_str()
        .with_context(|| format!("{} has no version", path.display()))?;
    // Shown in Settings and part of trust fingerprints.
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+' | b'_')),
        "{} has an unsupported version",
        path.display()
    );
    Ok(version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// `<root>/bin/node`, as nvm and Homebrew install it.
    fn node(root: &Path) -> PathBuf {
        write(&root.join("bin/node"), "node");
        root.join("bin/node")
    }
    /// `<root>/lib/node_modules/<package>` with a package.json declaring `version`. Returns the
    /// canonical package folder.
    fn package(root: &Path, package: &str, version: &str) -> PathBuf {
        let dir = root.join("lib/node_modules").join(package);
        write(
            &dir.join("package.json"),
            &format!(r#"{{"name": "{package}", "version": "{version}"}}"#),
        );
        dir.canonicalize().unwrap()
    }
    fn nvm(roots: &ToolRoots, version: &str) -> PathBuf {
        roots.home.join(".nvm/versions/node").join(version)
    }

    #[test]
    fn finds_the_newest_nvm_install_first() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        for version in ["v9.11.2", "v20.19.0", "v24.18.0"] {
            node(&nvm(&roots, version));
            package(&nvm(&roots, version), "typescript", "5.9.3");
        }
        let newest = nvm(&roots, "v24.22.1");
        let newest_node = node(&newest);
        let dir = package(&newest, "typescript", "7.0.2");
        node(&roots.homebrew[0]);
        package(&roots.homebrew[0], "typescript", "6.0.3");
        assert_eq!(
            global_package(&roots, "typescript"),
            Some(NodePackage {
                node: newest_node,
                dir,
                version: "7.0.2".into(),
            })
        );

        node(&roots.homebrew[0]);
        let scoped = package(&roots.homebrew[0], "@vue/language-server", "3.3.14");
        let found = global_package(&roots, "@vue/language-server").unwrap();
        assert_eq!(found.dir, scoped);
        assert_eq!(found.version, "3.3.14");
    }

    #[test]
    fn falls_back_to_homebrew() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        node(&nvm(&roots, "v24.22.1"));
        package(&nvm(&roots, "v24.22.1"), "pyright", "1.1.400");
        node(&roots.homebrew[0]);
        let homebrew_node = node(&roots.homebrew[1]);
        let dir = package(&roots.homebrew[1], "typescript-language-server", "6.0.2");
        assert_eq!(
            global_package(&roots, "typescript-language-server"),
            Some(NodePackage {
                node: homebrew_node,
                dir,
                version: "6.0.2".into(),
            })
        );
        assert_eq!(global_package(&roots, "svelte-language-server"), None);
    }

    #[test]
    fn ignores_a_package_without_node() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        package(&nvm(&roots, "v24.22.1"), "typescript", "7.0.2");
        package(&roots.homebrew[0], "typescript", "7.0.1");
        assert_eq!(global_package(&roots, "typescript"), None);

        let older = nvm(&roots, "v20.19.0");
        let older_node = node(&older);
        let dir = package(&older, "typescript", "5.9.3");
        assert_eq!(
            global_package(&roots, "typescript"),
            Some(NodePackage {
                node: older_node,
                dir,
                version: "5.9.3".into(),
            })
        );
    }

    #[test]
    fn a_package_needs_its_package_json() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let newest = nvm(&roots, "v24.22.1");
        node(&newest);
        write(
            &newest.join("lib/node_modules/typescript/bin/tsc"),
            "#!/usr/bin/env node\n",
        );
        let older = nvm(&roots, "v20.19.0");
        node(&older);
        let dir = package(&older, "typescript", "5.9.3");
        assert_eq!(global_package(&roots, "typescript").unwrap().dir, dir);
    }

    #[test]
    fn package_version_reads_only_a_small_regular_file() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("pkg");
        write(
            &dir.join("package.json"),
            r#"{"name":"x","version":"6.0.2"}"#,
        );
        assert_eq!(package_version(&dir).unwrap(), "6.0.2");

        // pnpm hard-links package files from its store.
        let linked = temp.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::hard_link(dir.join("package.json"), linked.join("package.json")).unwrap();
        assert_eq!(package_version(&linked).unwrap(), "6.0.2");

        let symlinked = temp.path().join("symlinked");
        std::fs::create_dir_all(&symlinked).unwrap();
        std::os::unix::fs::symlink(dir.join("package.json"), symlinked.join("package.json"))
            .unwrap();
        assert!(package_version(&symlinked).is_err());

        let large = temp.path().join("large");
        let padding = " ".repeat(1024 * 1024);
        write(
            &large.join("package.json"),
            &format!(r#"{{"version":"1.0.0"}}{padding}"#),
        );
        assert!(package_version(&large).is_err());

        let unversioned = temp.path().join("unversioned");
        write(&unversioned.join("package.json"), r#"{"name":"x"}"#);
        assert!(package_version(&unversioned).is_err());

        let odd = temp.path().join("odd");
        write(
            &odd.join("package.json"),
            r#"{"version":"1.0.0\nInjected"}"#,
        );
        assert!(package_version(&odd).is_err());
    }
}
