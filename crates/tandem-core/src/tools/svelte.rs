//! Svelte: svelte-language-server, installed globally with npm and run under the Node.js that
//! installed it. The package brings its own TypeScript (peer dependency `^5.9.2 || ^6.0.2`,
//! installed by npm). Discovery never runs `node` or `npm`.
use super::node::global_package;
use super::servers::{
    KindTools, Launch, MissingTools, NodeServer, ServerTools, ToolRoots, fingerprint,
};
use crate::ToolKind;
use std::path::Path;

const PACKAGE: &str = "svelte-language-server";
const INSTALL: &str = "npm install -g svelte-language-server";
/// The package's `svelteserver` command, relative to the package folder.
const SCRIPT: &str = "bin/server.js";

pub(crate) fn discover(_project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let Some(package) = global_package(roots, PACKAGE) else {
        return Err(MissingTools {
            message: format!("{PACKAGE} was not found."),
            install: vec![INSTALL.into()],
        });
    };
    let script = package.dir.join(SCRIPT);
    if !script.is_file() {
        return Err(MissingTools {
            message: format!("{PACKAGE} {} has no {SCRIPT}.", package.version),
            install: vec![INSTALL.into()],
        });
    }
    let package_json = package.dir.join("package.json");
    let fingerprint = fingerprint(
        "svelte-v1",
        std::slice::from_ref(&package.version),
        &[&package.node, &package_json, &script],
    )?;
    let label = format!("{PACKAGE} {}", package.version);
    Ok(KindTools {
        kind: ToolKind::Svelte,
        label: label.clone(),
        servers: vec![ServerTools {
            name: "svelte".into(),
            title: label,
            languages: vec!["svelte".into()],
            launch: Launch::Node {
                node: package.node,
                script,
                server: NodeServer::Svelte,
            },
        }],
        notes: vec![],
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::servers::discover_kind;
    use std::path::PathBuf;

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
    /// svelte-language-server installed with `npm install -g` under `root`, with its bin script.
    /// Returns (node, canonical package folder).
    fn install(root: &Path, version: &str) -> (PathBuf, PathBuf) {
        write(&root.join("bin/node"), "node");
        let dir = root.join("lib/node_modules/svelte-language-server");
        write(
            &dir.join("package.json"),
            &format!(r#"{{"name": "svelte-language-server", "version": "{version}"}}"#),
        );
        write(
            &dir.join("bin/server.js"),
            &format!("#! /usr/bin/env node\n// svelteserver {version}\n"),
        );
        (root.join("bin/node"), dir.canonicalize().unwrap())
    }

    #[test]
    fn finds_svelteserver() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let (node, dir) = install(&roots.home.join(".nvm/versions/node/v24.22.1"), "0.17.21");

        let tools = discover_kind(ToolKind::Svelte, &project, &roots)
            .expect("Svelte is served through the registry")
            .unwrap();
        assert_eq!(tools.kind, ToolKind::Svelte);
        assert_eq!(tools.label, "svelte-language-server 0.17.21");
        assert!(tools.notes.is_empty());
        assert_eq!(
            tools.servers,
            vec![ServerTools {
                name: "svelte".into(),
                title: "svelte-language-server 0.17.21".into(),
                languages: vec!["svelte".into()],
                launch: Launch::Node {
                    node: node.clone(),
                    script: dir.join("bin/server.js"),
                    server: NodeServer::Svelte,
                },
            }]
        );

        // Trust is asked again whenever Node.js, the package or the server script changes.
        let first = tools.fingerprint;
        assert!(!first.is_empty());
        assert_eq!(discover(&project, &roots).unwrap().fingerprint, first);
        write(
            &dir.join("bin/server.js"),
            "#! /usr/bin/env node\n// patched\n",
        );
        let patched = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(patched, first);
        write(&node, "node, rebuilt");
        let rebuilt = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(rebuilt, patched);
        install(&roots.home.join(".nvm/versions/node/v24.22.1"), "0.17.22");
        let updated = discover(&project, &roots).unwrap();
        assert_eq!(updated.label, "svelte-language-server 0.17.22");
        assert_ne!(updated.fingerprint, rebuilt);
    }

    #[test]
    fn reports_npm_install_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let missing = discover_kind(ToolKind::Svelte, &project, &roots)
            .expect("Svelte is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "svelte-language-server was not found.".into(),
                install: vec!["npm install -g svelte-language-server".into()],
            }
        );

        // A broken install, missing its script, is reported with the same command.
        let (_, dir) = install(&roots.homebrew[0], "0.17.21");
        std::fs::remove_file(dir.join("bin/server.js")).unwrap();
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing.message,
            "svelte-language-server 0.17.21 has no bin/server.js."
        );
        assert_eq!(
            missing.install,
            vec!["npm install -g svelte-language-server".to_string()]
        );
    }
}
