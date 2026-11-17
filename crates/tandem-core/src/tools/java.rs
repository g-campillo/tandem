//! Java: jdtls from Homebrew or Mason, run on the first Java 21 or newer runtime found by file
//! path. Discovery never runs `java`, `/usr/bin/java` or `/usr/libexec/java_home`, which print
//! "Unable to locate a Java Runtime" or open an install dialog when no JDK is installed.
use super::read_tool_config;
use super::servers::{KindTools, Launch, MissingTools, ServerTools, ToolRoots, fingerprint};
use crate::ToolKind;
use std::path::{Component, Path, PathBuf};

/// The oldest Java release jdtls runs on.
const MINIMUM_MAJOR: u64 = 21;

pub(crate) fn discover(_project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let (launcher, runtime) = match (find_jdtls(roots), find_runtime(roots)) {
        (Some(launcher), Some(runtime)) => (launcher, runtime),
        (launcher, runtime) => {
            let mut messages = vec![];
            let mut install = vec![];
            if launcher.is_none() {
                messages.push("jdtls (Eclipse JDT Language Server) was not found.");
                install.push("brew install jdtls".to_string());
            }
            if runtime.is_none() {
                messages.push("jdtls needs Java 21 or newer.");
                install.push("brew install openjdk@21".to_string());
            }
            return Err(MissingTools {
                message: messages.join(" "),
                install,
            });
        }
    };
    let title = match cellar_version(&launcher) {
        Some(version) => format!("jdtls {version}"),
        None => "jdtls".to_string(),
    };
    let label = format!(
        "{title} · Java {} ({})",
        runtime.version,
        runtime.home.display()
    );
    let fingerprint = fingerprint(
        "java-v1",
        &[
            launcher.display().to_string(),
            runtime.home.display().to_string(),
            runtime.version.clone(),
        ],
        &[&launcher, &runtime.home.join("bin/java")],
    )?;
    Ok(KindTools {
        kind: ToolKind::Java,
        label,
        servers: vec![ServerTools {
            name: "jdtls".into(),
            title,
            languages: vec!["java".into()],
            launch: Launch::Jdtls {
                launcher,
                java_home: runtime.home,
            },
        }],
        notes: vec![],
        fingerprint,
    })
}

/// The first jdtls launcher of each Homebrew prefix's `opt/jdtls`, then Mason's, canonical.
/// Homebrew does not put jdtls on PATH.
fn find_jdtls(roots: &ToolRoots) -> Option<PathBuf> {
    roots
        .homebrew
        .iter()
        .map(|prefix| prefix.join("opt/jdtls/bin/jdtls"))
        .chain([roots
            .home
            .join(".local/share/nvim/mason/packages/jdtls/bin/jdtls")])
        .filter(|launcher| launcher.is_file())
        .find_map(|launcher| launcher.canonicalize().ok())
}

/// Homebrew's version of a canonical launcher: the path component two after `Cellar`.
fn cellar_version(launcher: &Path) -> Option<String> {
    let parts: Vec<&str> = launcher
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let cellar = parts.iter().position(|part| *part == "Cellar")?;
    // The version is a folder, never the launcher itself.
    (cellar + 2 < parts.len() - 1).then(|| parts[cellar + 2].to_string())
}

/// A Java home with its `bin/java` and the version its `release` file declares.
struct Runtime {
    home: PathBuf,
    /// `JAVA_VERSION` as written, e.g. "21.0.12" or "1.8.0_452".
    version: String,
    /// The numbers of `version`, used to order runtimes.
    numbers: Vec<u64>,
}

impl Runtime {
    fn read(home: PathBuf) -> Option<Self> {
        if !home.join("bin/java").is_file() {
            return None;
        }
        let release = read_tool_config(&home.join("release")).ok()?;
        let version = release.lines().find_map(|line| {
            let quoted = line.trim().strip_prefix("JAVA_VERSION=")?;
            quoted.strip_prefix('"')?.strip_suffix('"')
        })?;
        let numbers: Vec<u64> = version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|run| !run.is_empty())
            .map_while(|run| run.parse().ok())
            .collect();
        let plain = version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+' | b'-'));
        (plain && !numbers.is_empty()).then(|| Self {
            home,
            version: version.to_string(),
            numbers,
        })
    }

    /// The first number, or the second for the old `1.x` versions.
    fn major(&self) -> u64 {
        match self.numbers.as_slice() {
            [1, minor, ..] => *minor,
            numbers => numbers[0],
        }
    }
}

/// The first Java 21 or newer of SDKMAN's `current`, the other SDKMAN candidates, Homebrew's
/// openjdk formulae, then the installed JavaVirtualMachines; within each group the highest
/// major first. Never `/usr/bin/java` or `/usr/libexec/java_home`.
fn find_runtime(roots: &ToolRoots) -> Option<Runtime> {
    let sdkman = roots.home.join(".sdkman/candidates/java");
    let candidates = sorted_names(&sdkman)
        .into_iter()
        .filter(|name| name != "current")
        .map(|name| sdkman.join(name))
        .collect();
    let homebrew = roots
        .homebrew
        .iter()
        .flat_map(|prefix| {
            let opt = prefix.join("opt");
            sorted_names(&opt)
                .into_iter()
                .filter(|name| name == "openjdk" || name.starts_with("openjdk@"))
                .map(move |name| opt.join(name).join("libexec/openjdk.jdk/Contents/Home"))
        })
        .collect();
    let machines = [&roots.system, &roots.home]
        .into_iter()
        .flat_map(|root| {
            let folder = root.join("Library/Java/JavaVirtualMachines");
            sorted_names(&folder)
                .into_iter()
                .map(move |name| folder.join(name).join("Contents/Home"))
        })
        .collect();
    [vec![sdkman.join("current")], candidates, homebrew, machines]
        .into_iter()
        .find_map(|group| {
            let mut usable: Vec<Runtime> = group
                .into_iter()
                .filter_map(Runtime::read)
                .filter(|runtime| runtime.major() >= MINIMUM_MAJOR)
                .collect();
            // Stable, so equal versions keep their listing order.
            usable.sort_by(|a, b| (b.major(), &b.numbers).cmp(&(a.major(), &a.numbers)));
            usable.into_iter().next()
        })
}

/// UTF-8 entry names of a folder, sorted; empty when it cannot be read.
fn sorted_names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::servers::discover_kind;

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
    /// A JDK home with `bin/java` and a `release` file declaring `version`.
    fn jdk(home: &Path, version: &str) -> PathBuf {
        write(&home.join("bin/java"), &format!("java {version}"));
        write(
            &home.join("release"),
            &format!(
                "IMPLEMENTOR=\"Fake\"\nJAVA_RUNTIME_VERSION=\"{version}+7\"\nJAVA_VERSION=\"{version}\"\nJAVA_VERSION_DATE=\"2026-07-21\"\n"
            ),
        );
        home.to_path_buf()
    }
    fn sdkman(roots: &ToolRoots, name: &str, version: &str) -> PathBuf {
        jdk(
            &roots.home.join(".sdkman/candidates/java").join(name),
            version,
        )
    }
    /// SDKMAN's `current`, a link to an installed candidate as `sdk default` makes it.
    fn sdkman_current(roots: &ToolRoots, name: &str) -> PathBuf {
        let current = roots.home.join(".sdkman/candidates/java/current");
        std::os::unix::fs::symlink(
            roots.home.join(".sdkman/candidates/java").join(name),
            &current,
        )
        .unwrap();
        current
    }
    fn homebrew_openjdk(prefix: &Path, formula: &str, version: &str) -> PathBuf {
        jdk(
            &prefix
                .join("opt")
                .join(formula)
                .join("libexec/openjdk.jdk/Contents/Home"),
            version,
        )
    }
    /// jdtls installed by Homebrew: the Cellar folder and its `opt` link. Returns the canonical
    /// launcher.
    fn homebrew_jdtls(prefix: &Path, version: &str) -> PathBuf {
        let keg = prefix.join("Cellar/jdtls").join(version);
        write(
            &keg.join("bin/jdtls"),
            &format!("#!/bin/bash\n# jdtls {version}\n"),
        );
        std::fs::create_dir_all(prefix.join("opt")).unwrap();
        std::os::unix::fs::symlink(&keg, prefix.join("opt/jdtls")).unwrap();
        keg.join("bin/jdtls").canonicalize().unwrap()
    }
    fn mason_jdtls(roots: &ToolRoots) -> PathBuf {
        let launcher = roots
            .home
            .join(".local/share/nvim/mason/packages/jdtls/bin/jdtls");
        write(&launcher, "#!/usr/bin/env python3\n");
        launcher.canonicalize().unwrap()
    }
    fn java_home(tools: &KindTools) -> PathBuf {
        let Launch::Jdtls { java_home, .. } = &tools.servers[0].launch else {
            panic!("expected a jdtls launch, got {:?}", tools.servers[0].launch);
        };
        java_home.clone()
    }

    #[test]
    fn prefers_sdkman_current_when_it_is_21_or_newer() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let launcher = homebrew_jdtls(&roots.homebrew[0], "1.47.0");
        sdkman(&roots, "21.0.12-amzn", "21.0.12");
        sdkman(&roots, "27.0.0.0+35-tem", "27");
        homebrew_openjdk(&roots.homebrew[0], "openjdk", "25.0.1");
        let current = sdkman_current(&roots, "21.0.12-amzn");
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(tools.kind, ToolKind::Java);
        assert_eq!(
            tools.label,
            format!("jdtls 1.47.0 · Java 21.0.12 ({})", current.display())
        );
        assert_eq!(tools.servers.len(), 1);
        let server = &tools.servers[0];
        assert_eq!(server.name, "jdtls");
        assert_eq!(server.title, "jdtls 1.47.0");
        assert_eq!(server.languages, vec!["java".to_string()]);
        assert_eq!(
            server.launch,
            Launch::Jdtls {
                launcher,
                java_home: current,
            }
        );
        assert!(tools.notes.is_empty());
        assert!(!tools.fingerprint.is_empty());
    }

    #[test]
    fn skips_runtimes_older_than_21() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        mason_jdtls(&roots);
        sdkman(&roots, "17.0.18-amzn", "17.0.18");
        sdkman(&roots, "8.0.452-zulu", "1.8.0_452");
        sdkman(&roots, "21.0.12-amzn", "21.0.12");
        let newest = sdkman(&roots, "27.0.0.0+35-tem", "27");
        sdkman_current(&roots, "17.0.18-amzn");
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(java_home(&tools), newest);
        assert!(tools.label.contains("Java 27 ("), "{}", tools.label);

        let fingerprint = tools.fingerprint;
        std::fs::remove_dir_all(&newest).unwrap();
        let tools = discover(temp.path(), &roots).unwrap();
        assert!(tools.label.contains("Java 21.0.12 ("), "{}", tools.label);
        assert_ne!(tools.fingerprint, fingerprint);

        std::fs::remove_dir_all(roots.home.join(".sdkman/candidates/java/21.0.12-amzn")).unwrap();
        let missing = discover(temp.path(), &roots).unwrap_err();
        assert_eq!(missing.message, "jdtls needs Java 21 or newer.");
        assert_eq!(missing.install, vec!["brew install openjdk@21".to_string()]);
    }

    #[test]
    fn uses_homebrew_openjdk_when_sdkman_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        homebrew_jdtls(&roots.homebrew[0], "1.47.0");
        jdk(
            &roots
                .system
                .join("Library/Java/JavaVirtualMachines/temurin-27.jdk/Contents/Home"),
            "27",
        );
        homebrew_openjdk(&roots.homebrew[0], "openjdk@21", "21.0.9");
        homebrew_openjdk(&roots.homebrew[0], "openjdk@17", "17.0.17");
        let newest = homebrew_openjdk(&roots.homebrew[1], "openjdk@25", "25.0.5.0.3");
        write(&roots.homebrew[0].join("opt/openjdk-tools/readme"), "");
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(java_home(&tools), newest);
        assert!(tools.label.contains("Java 25.0.5.0.3 ("), "{}", tools.label);
    }

    #[test]
    fn falls_back_to_installed_java_virtual_machines() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        homebrew_jdtls(&roots.homebrew[0], "1.47.0");
        jdk(
            &roots
                .system
                .join("Library/Java/JavaVirtualMachines/zulu-17.jdk/Contents/Home"),
            "17.0.17",
        );
        let user = jdk(
            &roots
                .home
                .join("Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"),
            "21.0.9",
        );
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(java_home(&tools), user);
    }

    #[test]
    fn a_runtime_needs_its_java_and_release_files() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        mason_jdtls(&roots);
        let no_java = sdkman(&roots, "25-tem", "25");
        std::fs::remove_file(no_java.join("bin/java")).unwrap();
        let no_release = sdkman(&roots, "24-tem", "24");
        std::fs::remove_file(no_release.join("release")).unwrap();
        let no_version = sdkman(&roots, "23-tem", "23");
        write(&no_version.join("release"), "IMPLEMENTOR=\"Fake\"\n");
        let usable = sdkman(&roots, "22-tem", "22.0.2");
        assert_eq!(java_home(&discover(temp.path(), &roots).unwrap()), usable);
    }

    #[test]
    fn finds_jdtls_in_homebrew_opt_or_mason() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        sdkman(&roots, "21.0.12-amzn", "21.0.12");
        let mason = mason_jdtls(&roots);
        let homebrew = homebrew_jdtls(&roots.homebrew[1], "1.47.0");
        assert!(homebrew.ends_with("Cellar/jdtls/1.47.0/bin/jdtls"));
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(tools.servers[0].title, "jdtls 1.47.0");
        assert!(tools.label.starts_with("jdtls 1.47.0 · Java 21.0.12 ("));
        let Launch::Jdtls { launcher, .. } = &tools.servers[0].launch else {
            panic!("expected a jdtls launch");
        };
        assert_eq!(launcher, &homebrew);

        let fingerprint = tools.fingerprint;
        std::fs::remove_file(roots.homebrew[1].join("opt/jdtls")).unwrap();
        let tools = discover(temp.path(), &roots).unwrap();
        assert_eq!(tools.servers[0].title, "jdtls");
        assert!(tools.label.starts_with("jdtls · Java 21.0.12 ("));
        let Launch::Jdtls { launcher, .. } = &tools.servers[0].launch else {
            panic!("expected a jdtls launch");
        };
        assert_eq!(launcher, &mason);
        assert_ne!(tools.fingerprint, fingerprint);

        let fingerprint = tools.fingerprint;
        write(&mason, "#!/usr/bin/env python3\n# updated\n");
        assert_ne!(
            discover(temp.path(), &roots).unwrap().fingerprint,
            fingerprint
        );
    }

    #[test]
    fn reports_both_install_commands_when_nothing_is_installed() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let missing = discover_kind(ToolKind::Java, temp.path(), &roots)
            .expect("Java is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing.message,
            "jdtls (Eclipse JDT Language Server) was not found. jdtls needs Java 21 or newer."
        );
        assert_eq!(
            missing.install,
            vec![
                "brew install jdtls".to_string(),
                "brew install openjdk@21".to_string()
            ]
        );

        sdkman(&roots, "21.0.12-amzn", "21.0.12");
        let missing = discover(temp.path(), &roots).unwrap_err();
        assert_eq!(
            missing.message,
            "jdtls (Eclipse JDT Language Server) was not found."
        );
        assert_eq!(missing.install, vec!["brew install jdtls".to_string()]);
    }
}
