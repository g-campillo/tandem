//! Java: jdtls from Homebrew or Mason, run on the first Java 21 or newer runtime found by file
//! path. Discovery never runs `java`, `/usr/bin/java` or `/usr/libexec/java_home`, which print
//! "Unable to locate a Java Runtime" or open an install dialog when no JDK is installed.
use super::read_tool_config;
use super::servers::{
    KindTools, Launch, MissingTools, ServerTools, ToolNote, ToolRoots, fingerprint,
};
use crate::ToolKind;
use std::path::{Component, Path, PathBuf};

/// The oldest Java release jdtls runs on.
const MINIMUM_MAJOR: u64 = 21;

pub(crate) fn discover(project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
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
            prepare: None,
            name: "jdtls".into(),
            title,
            languages: vec!["java".into()],
            launch: Launch::Jdtls {
                launcher,
                java_home: runtime.home,
                trigger_files: plain_java_files(project),
            },
        }],
        notes: build_notes(project, &roots.home),
        fingerprint,
    })
}

/// The first jdtls launcher of each Homebrew prefix's `opt/jdtls`, then Mason's, canonical.
/// Homebrew does not put jdtls on PATH.
/// What a project needs before the offline import can resolve it: Maven dependencies missing from
/// `~/.m2/repository` (those of the root pom.xml with plain versions), or a Gradle build that never
/// ran (no `.gradle` folder).
fn build_notes(project: &Path, home: &Path) -> Vec<ToolNote> {
    let offline = "Tandem imports Java projects offline, so their libraries resolve only once a build has downloaded them.";
    if let Ok(pom) = read_tool_config(&project.join("pom.xml")) {
        let repository = home.join(".m2/repository");
        let missing: Vec<String> = maven_dependencies(&pom)
            .into_iter()
            .filter(|(group, artifact, version)| {
                !repository
                    .join(group.replace('.', "/"))
                    .join(artifact)
                    .join(version)
                    .is_dir()
            })
            .map(|(group, artifact, version)| format!("{group}:{artifact}:{version}"))
            .collect();
        if missing.is_empty() {
            return vec![];
        }
        let maven = if project.join("mvnw").is_file() {
            "./mvnw"
        } else {
            "mvn"
        };
        return vec![ToolNote {
            text: format!(
                "{offline} Not downloaded yet: {}. Run this in the project folder, then choose Restart Java Tools.",
                missing.join(", ")
            ),
            command: Some(format!("{maven} dependency:resolve")),
        }];
    }
    let gradle = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .any(|file| project.join(file).is_file());
    if gradle && !project.join(".gradle").is_dir() {
        let command = if project.join("gradlew").is_file() {
            "./gradlew build"
        } else {
            "gradle build"
        };
        return vec![ToolNote {
            text: format!(
                "{offline} This Gradle project has not been built here yet. Run this in the project folder, then choose Restart Java Tools."
            ),
            command: Some(command.into()),
        }];
    }
    vec![]
}

/// Up to 50 Java files of a project with no Maven, Gradle or Eclipse project files, from folders
/// eight levels deep (skipping hidden, dependency and build output folders). Without them jdtls
/// takes the project folder for the source folder, and every `package` line looks wrong.
fn plain_java_files(project: &Path) -> Vec<PathBuf> {
    const BUILD_FILES: [&str; 6] = [
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        ".project",
    ];
    const SKIP: [&str; 5] = ["node_modules", "target", "build", "out", "bin"];
    if BUILD_FILES.iter().any(|file| project.join(file).exists()) {
        return vec![];
    }
    let mut found = vec![];
    let mut queue = vec![(project.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((folder, depth)) = queue.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            seen += 1;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if seen > 5000 || found.len() >= 50 {
                return found;
            }
            if kind.is_file() && name.ends_with(".java") {
                found.push(entry.path());
            } else if kind.is_dir() && depth < 8 && !name.starts_with('.') && !SKIP.contains(&name)
            {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    found
}

/// (group, artifact, version) of each `<dependency>` in a pom.xml whose three values are written
/// out, not taken from properties or a parent.
fn maven_dependencies(pom: &str) -> Vec<(String, String, String)> {
    let field = |block: &str, name: &str| -> Option<String> {
        let (_, rest) = block.split_once(&format!("<{name}>"))?;
        let (value, _) = rest.split_once(&format!("</{name}>"))?;
        let value = value.trim();
        (!value.is_empty() && !value.contains("${") && !value.contains('<'))
            .then(|| value.to_string())
    };
    pom.split("<dependency>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</dependency>").next()?;
            Some((
                field(block, "groupId")?,
                field(block, "artifactId")?,
                field(block, "version")?,
            ))
        })
        .collect()
}

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
                trigger_files: vec![],
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
    fn notes_name_what_a_build_must_download_first() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (home, project) = (home.path(), project.path());
        write(
            &project.join("pom.xml"),
            "<project><dependencies>\n<dependency><groupId>org.jsoup</groupId><artifactId>jsoup</artifactId><version>1.18.3</version></dependency>\n<dependency><groupId>com.google.guava</groupId><artifactId>guava</artifactId><version>${guava.version}</version></dependency>\n<dependency><groupId>org.slf4j</groupId><artifactId>slf4j-api</artifactId><version>2.0.16</version><scope>compile</scope></dependency>\n</dependencies></project>",
        );
        std::fs::create_dir_all(home.join(".m2/repository/org/slf4j/slf4j-api/2.0.16")).unwrap();
        assert_eq!(
            build_notes(project, home),
            vec![ToolNote {
                text: "Tandem imports Java projects offline, so their libraries resolve only once a build has downloaded them. Not downloaded yet: org.jsoup:jsoup:1.18.3. Run this in the project folder, then choose Restart Java Tools.".into(),
                command: Some("mvn dependency:resolve".into()),
            }]
        );
        write(&project.join("mvnw"), "#!/bin/sh\n");
        assert_eq!(
            build_notes(project, home)[0].command.as_deref(),
            Some("./mvnw dependency:resolve")
        );
        std::fs::create_dir_all(home.join(".m2/repository/org/jsoup/jsoup/1.18.3")).unwrap();
        assert!(build_notes(project, home).is_empty());

        // Gradle: a project never built here has no .gradle folder.
        let gradle = tempfile::tempdir().unwrap();
        let gradle = gradle.path();
        write(&gradle.join("build.gradle.kts"), "plugins { java }");
        write(&gradle.join("gradlew"), "#!/bin/sh\n");
        assert_eq!(
            build_notes(gradle, home)[0].command.as_deref(),
            Some("./gradlew build")
        );
        std::fs::create_dir_all(gradle.join(".gradle")).unwrap();
        assert!(build_notes(gradle, home).is_empty());
    }

    #[test]
    fn plain_folders_name_their_java_files_for_jdtls() {
        let project = tempfile::tempdir().unwrap();
        let project = project.path();
        write(&project.join("src/demo/A.java"), "package demo;");
        write(&project.join("src/demo/B.java"), "package demo;");
        write(&project.join("src/demo/notes.txt"), "");
        write(&project.join("target/classes/demo/Gen.java"), "");
        write(&project.join(".hidden/C.java"), "");
        assert_eq!(
            plain_java_files(project),
            vec![
                project.join("src/demo/A.java"),
                project.join("src/demo/B.java")
            ]
        );
        write(&project.join("pom.xml"), "<project/>");
        assert!(plain_java_files(project).is_empty());
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
