//! HTML, CSS (with SCSS and Less) and JSON: the three language servers of
//! vscode-langservers-extracted, installed globally with npm and run under the Node.js that
//! installed them. Discovery never runs `node` or `npm`.
use super::node::global_package;
use super::servers::{
    KindTools, Launch, MissingTools, NodeServer, ServerTools, ToolRoots, fingerprint,
};
use crate::ToolKind;
use std::path::{Path, PathBuf};

const PACKAGE: &str = "vscode-langservers-extracted";
const INSTALL: &str = "npm install -g vscode-langservers-extracted";

/// The package's servers: (name, title, script in the package's bin folder, server, the
/// `Document::language()` ids it answers for).
const SERVERS: [(&str, &str, &str, NodeServer, &[&str]); 3] = [
    (
        "html",
        "HTML language server",
        "vscode-html-language-server",
        NodeServer::Html,
        &["html"],
    ),
    (
        "css",
        "CSS language server",
        "vscode-css-language-server",
        NodeServer::Css,
        &["css", "scss", "less"],
    ),
    (
        "json",
        "JSON language server",
        "vscode-json-language-server",
        NodeServer::Json,
        &["json", "jsonc"],
    ),
];

pub(crate) fn discover(_project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let Some(package) = global_package(roots, PACKAGE) else {
        return Err(MissingTools {
            message: "The HTML, CSS and JSON language servers were not found.".into(),
            install: vec![INSTALL.into()],
        });
    };
    let mut scripts: Vec<PathBuf> = Vec::with_capacity(SERVERS.len());
    for (_, _, script, _, _) in SERVERS {
        let path = package.dir.join("bin").join(script);
        if !path.is_file() {
            return Err(MissingTools {
                message: format!("{PACKAGE} {} has no bin/{script}.", package.version),
                install: vec![INSTALL.into()],
            });
        }
        scripts.push(path);
    }
    let package_json = package.dir.join("package.json");
    let mut files = vec![package.node.as_path(), package_json.as_path()];
    files.extend(scripts.iter().map(PathBuf::as_path));
    let fingerprint = fingerprint("web-v1", std::slice::from_ref(&package.version), &files)?;
    let servers = SERVERS
        .into_iter()
        .zip(scripts)
        .map(
            |((name, title, _, server, languages), script)| ServerTools {
                prepare: None,
                name: name.into(),
                title: title.into(),
                languages: languages.iter().map(|id| id.to_string()).collect(),
                launch: Launch::Node {
                    node: package.node.clone(),
                    script,
                    server,
                },
            },
        )
        .collect();
    Ok(KindTools {
        kind: ToolKind::Web,
        label: format!("{PACKAGE} {}", package.version),
        servers,
        notes: vec![],
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::servers::discover_kind;

    const SCRIPTS: [&str; 3] = [
        "vscode-html-language-server",
        "vscode-css-language-server",
        "vscode-json-language-server",
    ];

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
    /// vscode-langservers-extracted installed with `npm install -g` under `root`, with its three
    /// bin scripts. Returns (node, canonical package folder).
    fn install(root: &Path, version: &str) -> (PathBuf, PathBuf) {
        write(&root.join("bin/node"), "node");
        let dir = root.join("lib/node_modules/vscode-langservers-extracted");
        write(
            &dir.join("package.json"),
            &format!(r#"{{"name": "vscode-langservers-extracted", "version": "{version}"}}"#),
        );
        for script in SCRIPTS {
            write(
                &dir.join("bin").join(script),
                &format!("#!/usr/bin/env node\n// {script} {version}\n"),
            );
        }
        (root.join("bin/node"), dir.canonicalize().unwrap())
    }

    #[test]
    fn finds_the_three_servers() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let (node, dir) = install(&roots.homebrew[0], "4.10.0");

        let tools = discover_kind(ToolKind::Web, &project, &roots)
            .expect("HTML, CSS and JSON are served through the registry")
            .unwrap();
        assert_eq!(tools.kind, ToolKind::Web);
        assert_eq!(tools.label, "vscode-langservers-extracted 4.10.0");
        assert!(tools.notes.is_empty());
        let server =
            |name: &str, title: &str, script: &str, kind, languages: &[&str]| ServerTools {
                prepare: None,
                name: name.into(),
                title: title.into(),
                languages: languages.iter().map(|id| id.to_string()).collect(),
                launch: Launch::Node {
                    node: node.clone(),
                    script: dir.join("bin").join(script),
                    server: kind,
                },
            };
        assert_eq!(
            tools.servers,
            vec![
                server(
                    "html",
                    "HTML language server",
                    "vscode-html-language-server",
                    NodeServer::Html,
                    &["html"]
                ),
                server(
                    "css",
                    "CSS language server",
                    "vscode-css-language-server",
                    NodeServer::Css,
                    &["css", "scss", "less"]
                ),
                server(
                    "json",
                    "JSON language server",
                    "vscode-json-language-server",
                    NodeServer::Json,
                    &["json", "jsonc"]
                ),
            ]
        );

        // Trust is asked again whenever Node.js, the package or any server script changes.
        let first = tools.fingerprint;
        assert!(!first.is_empty());
        assert_eq!(discover(&project, &roots).unwrap().fingerprint, first);
        write(
            &dir.join("bin/vscode-css-language-server"),
            "#!/usr/bin/env node\n// patched\n",
        );
        let patched = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(patched, first);
        write(&node, "node, rebuilt");
        let rebuilt = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(rebuilt, patched);
        install(&roots.homebrew[0], "4.10.1");
        let updated = discover(&project, &roots).unwrap();
        assert_eq!(updated.label, "vscode-langservers-extracted 4.10.1");
        assert_ne!(updated.fingerprint, rebuilt);
    }

    #[test]
    fn reports_npm_install_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "The HTML, CSS and JSON language servers were not found.".into(),
                install: vec!["npm install -g vscode-langservers-extracted".into()],
            }
        );

        // A broken install, missing one of its scripts, is reported with the same command.
        let (_, dir) = install(&roots.home.join(".nvm/versions/node/v24.22.1"), "4.10.0");
        std::fs::remove_file(dir.join("bin/vscode-json-language-server")).unwrap();
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing.message,
            "vscode-langservers-extracted 4.10.0 has no bin/vscode-json-language-server."
        );
        assert_eq!(
            missing.install,
            vec!["npm install -g vscode-langservers-extracted".to_string()]
        );
    }
}
