//! JavaScript and TypeScript: the project's own TypeScript when it has one, else the global one.
//! TypeScript 7 runs as its native compiler's language server (tsgo); TypeScript 6 and older run
//! through typescript-language-server. The server is chosen by which files exist; versions only
//! name it. Discovery never runs `node`, `npm` or `tsc`.
use super::node::{NodePackage, global_package, package_version};
use super::servers::{
    KindTools, Launch, MissingTools, ServerTools, ToolNote, ToolRoots, fingerprint,
};
use super::vue::typescript_plugin_location;
use crate::ToolKind;
use std::path::{Path, PathBuf};

/// The npm package that holds TypeScript 7's native compiler for this Mac.
#[cfg(target_arch = "x86_64")]
const NATIVE_PACKAGE: &str = "@typescript/typescript-darwin-x64";
#[cfg(not(target_arch = "x86_64"))]
const NATIVE_PACKAGE: &str = "@typescript/typescript-darwin-arm64";

/// The `Document::language()` ids one TypeScript server answers for.
const LANGUAGES: [&str; 4] = [
    "javascript",
    "javascriptreact",
    "typescript",
    "typescriptreact",
];

const INSTALL_TS_LS: &str = "npm install -g typescript-language-server";

/// An installed TypeScript package.
struct TypeScript {
    /// Canonical package folder.
    dir: PathBuf,
    version: String,
}

impl TypeScript {
    /// The project's `node_modules/typescript`, when it has a readable package.json.
    fn in_project(project: &Path) -> Option<Self> {
        let dir = project.join("node_modules/typescript");
        if !dir.join("package.json").is_file() {
            return None;
        }
        let dir = dir.canonicalize().ok()?;
        let version = package_version(&dir).ok()?;
        Some(Self { dir, version })
    }

    /// TypeScript 7's native compiler, whose undocumented `--lsp --stdio` mode is a full language
    /// server: beside the package (npm's hoisted layout, or pnpm's sibling link), else nested in
    /// it. Canonical.
    fn native(&self) -> Option<PathBuf> {
        let binary = Path::new(NATIVE_PACKAGE).join("lib/tsc");
        self.dir
            .parent()
            .map(|modules| modules.join(&binary))
            .into_iter()
            .chain([self.dir.join("node_modules").join(&binary)])
            .filter(|tsc| tsc.is_file())
            .find_map(|tsc| tsc.canonicalize().ok())
    }

    /// `lib/tsserver.js`, which TypeScript 6 and older have and TypeScript 7 does not.
    fn tsserver(&self) -> Option<PathBuf> {
        let tsserver = self.dir.join("lib/tsserver.js");
        tsserver.is_file().then_some(tsserver)
    }
}

impl From<NodePackage> for TypeScript {
    fn from(package: NodePackage) -> Self {
        Self {
            dir: package.dir,
            version: package.version,
        }
    }
}

pub(crate) fn discover(project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let project_ts = TypeScript::in_project(project);
    let global_ts = global_package(roots, "typescript").map(TypeScript::from);
    let ts_ls = global_package(roots, "typescript-language-server")
        .filter(|package| package.dir.join("lib/cli.mjs").is_file());
    let (title, mut launch, notes) = select(project_ts, global_ts, ts_ls)?;
    // A Vue project's TypeScript files import `.vue` files, which tsserver reads only through
    // Vue's plugin. TypeScript 7 (tsgo) cannot load plugins.
    if let Launch::TypeScriptLanguageServer { vue_plugin, .. } = &mut launch
        && project.join("node_modules/vue/package.json").is_file()
    {
        *vue_plugin = typescript_plugin_location(roots);
    }
    let (name, program, tsserver, files) = match &launch {
        Launch::Tsgo { server } => ("tsgo", server, None, vec![server.as_path()]),
        Launch::TypeScriptLanguageServer {
            node,
            server,
            tsserver,
            ..
        } => (
            "typescript-language-server",
            server,
            Some(tsserver),
            vec![node.as_path(), server, tsserver],
        ),
        other => unreachable!("TypeScript discovery chose {other:?}"),
    };
    let fingerprint = fingerprint(
        "typescript-v1",
        &[
            title.clone(),
            program.display().to_string(),
            tsserver
                .map(|tsserver| tsserver.display().to_string())
                .unwrap_or_default(),
            match &launch {
                Launch::TypeScriptLanguageServer {
                    vue_plugin: Some(plugin),
                    ..
                } => format!("vue plugin {}", plugin.display()),
                _ => String::new(),
            },
        ],
        &files,
    )?;
    Ok(KindTools {
        kind: ToolKind::TypeScript,
        label: title.clone(),
        servers: vec![ServerTools {
            prepare: None,
            name: name.into(),
            title,
            languages: LANGUAGES.map(String::from).to_vec(),
            launch,
        }],
        notes,
        fingerprint,
    })
}

/// The first usable server: the project's TypeScript (native, else through
/// typescript-language-server), then the global one the same way.
fn select(
    project_ts: Option<TypeScript>,
    global_ts: Option<TypeScript>,
    ts_ls: Option<NodePackage>,
) -> Result<(String, Launch, Vec<ToolNote>), MissingTools> {
    let global_native = global_ts
        .as_ref()
        .and_then(|global| Some((global, global.native()?)));
    let through_ts_ls = |ts_ls: &NodePackage, tsserver: PathBuf| Launch::TypeScriptLanguageServer {
        node: ts_ls.node.clone(),
        server: ts_ls.dir.join("lib/cli.mjs"),
        tsserver,
        vue_plugin: None,
    };
    if let Some(project_ts) = &project_ts {
        if let Some(native) = project_ts.native() {
            return Ok((
                format!("TypeScript {} (project)", project_ts.version),
                Launch::Tsgo { server: native },
                vec![],
            ));
        }
        if let Some(tsserver) = project_ts.tsserver() {
            if let Some(ts_ls) = &ts_ls {
                return Ok((
                    format!(
                        "TypeScript {} (project) via typescript-language-server {}",
                        project_ts.version, ts_ls.version
                    ),
                    through_ts_ls(ts_ls, tsserver),
                    vec![],
                ));
            }
            if let Some((global, native)) = global_native {
                return Ok((
                    format!("TypeScript {} (global)", global.version),
                    Launch::Tsgo { server: native },
                    vec![ToolNote {
                        text: format!(
                            "Answers come from TypeScript {}, not the project's {}. Install typescript-language-server to use the project's TypeScript.",
                            global.version, project_ts.version
                        ),
                        command: Some(INSTALL_TS_LS.into()),
                    }],
                ));
            }
            return Err(MissingTools {
                message: format!(
                    "This project's TypeScript {} needs typescript-language-server.",
                    project_ts.version
                ),
                install: vec![INSTALL_TS_LS.into()],
            });
        }
    }
    if let Some((global, native)) = global_native {
        return Ok((
            format!("TypeScript {} (global)", global.version),
            Launch::Tsgo { server: native },
            vec![],
        ));
    }
    if let Some(global) = &global_ts
        && let Some(tsserver) = global.tsserver()
    {
        let Some(ts_ls) = &ts_ls else {
            return Err(MissingTools {
                message: format!(
                    "The global TypeScript {} needs typescript-language-server.",
                    global.version
                ),
                install: vec![INSTALL_TS_LS.into()],
            });
        };
        return Ok((
            format!(
                "TypeScript {} (global) via typescript-language-server {}",
                global.version, ts_ls.version
            ),
            through_ts_ls(ts_ls, tsserver),
            vec![],
        ));
    }
    Err(MissingTools {
        message: "JavaScript and TypeScript tools need TypeScript.".into(),
        install: vec!["npm install -g typescript".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolKind;
    use crate::tools::servers::{Launch, ToolNote, discover_kind};
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
    fn link(target: &Path, link: &Path) {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    fn package_json(dir: &Path, name: &str, version: &str) {
        write(
            &dir.join("package.json"),
            &format!(r#"{{"name": "{name}", "version": "{version}"}}"#),
        );
    }
    /// The native compiler package beside or inside a TypeScript 7 package. Returns the binary.
    fn native(package: &Path) -> PathBuf {
        let tsc = package.join(NATIVE_PACKAGE).join("lib/tsc");
        write(&tsc, "native tsc");
        tsc
    }
    /// A TypeScript 6 or older package with its tsserver.js. Returns the canonical tsserver.js.
    fn typescript_with_tsserver(dir: &Path, version: &str) -> PathBuf {
        package_json(dir, "typescript", version);
        write(
            &dir.join("lib/tsserver.js"),
            &format!("// tsserver {version}"),
        );
        write(&dir.join("lib/typescript.js"), "// typescript");
        dir.join("lib/tsserver.js").canonicalize().unwrap()
    }
    /// npm's hoisted layout: node_modules/typescript beside node_modules/@typescript/....
    /// Returns the canonical native binary.
    fn project_typescript_7(project: &Path, version: &str) -> PathBuf {
        let modules = project.join("node_modules");
        package_json(&modules.join("typescript"), "typescript", version);
        write(&modules.join("typescript/lib/tsc.js"), "// launcher");
        native(&modules).canonicalize().unwrap()
    }
    fn nvm(roots: &ToolRoots, version: &str) -> PathBuf {
        let root = roots.home.join(".nvm/versions/node").join(version);
        write(&root.join("bin/node"), "node");
        root
    }
    /// A global TypeScript 7 as `npm install -g typescript` lays it out: the native package nested
    /// in the typescript package. Returns the canonical native binary.
    fn global_typescript_7(root: &Path, version: &str) -> PathBuf {
        let dir = root.join("lib/node_modules/typescript");
        package_json(&dir, "typescript", version);
        native(&dir.join("node_modules")).canonicalize().unwrap()
    }
    /// typescript-language-server installed globally. Returns (node, canonical cli.mjs).
    fn global_ts_ls(root: &Path, version: &str) -> (PathBuf, PathBuf) {
        let dir = root.join("lib/node_modules/typescript-language-server");
        package_json(&dir, "typescript-language-server", version);
        write(&dir.join("lib/cli.mjs"), &format!("// cli {version}"));
        (
            root.join("bin/node"),
            dir.join("lib/cli.mjs").canonicalize().unwrap(),
        )
    }

    fn assert_server(tools: &KindTools, name: &str, title: &str, launch: Launch) {
        assert_eq!(tools.kind, ToolKind::TypeScript);
        assert_eq!(tools.label, title);
        assert_eq!(tools.servers.len(), 1);
        let server = &tools.servers[0];
        assert_eq!(server.name, name);
        assert_eq!(server.title, title);
        assert_eq!(
            server.languages,
            [
                "javascript",
                "javascriptreact",
                "typescript",
                "typescriptreact"
            ]
        );
        assert_eq!(server.launch, launch);
        assert!(!tools.fingerprint.is_empty());
    }

    #[test]
    fn project_typescript_7_uses_its_native_server() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots, "v24.22.1");
        global_typescript_7(&global, "7.0.1");
        global_ts_ls(&global, "6.0.2");

        let project = temp.path().join("hoisted");
        let tsc = project_typescript_7(&project, "7.0.2");
        let tools = discover(&project, &roots).unwrap();
        assert_server(
            &tools,
            "tsgo",
            "TypeScript 7.0.2 (project)",
            Launch::Tsgo { server: tsc },
        );
        assert!(tools.notes.is_empty());

        // pnpm links node_modules/typescript into its store, with the native package beside it.
        let project = temp.path().join("pnpm");
        let store = project.join("node_modules/.pnpm");
        let typescript = store.join("typescript@7.0.2/node_modules/typescript");
        package_json(&typescript, "typescript", "7.0.2");
        let native_store = store.join("@typescript+typescript-darwin@7.0.2/node_modules");
        let tsc = native(&native_store);
        link(
            &native_store.join(NATIVE_PACKAGE),
            &store
                .join("typescript@7.0.2/node_modules")
                .join(NATIVE_PACKAGE),
        );
        link(&typescript, &project.join("node_modules/typescript"));
        let tools = discover(&project, &roots).unwrap();
        assert_server(
            &tools,
            "tsgo",
            "TypeScript 7.0.2 (project)",
            Launch::Tsgo {
                server: tsc.canonicalize().unwrap(),
            },
        );
    }

    #[test]
    fn project_typescript_5_uses_typescript_language_server() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots, "v24.22.1");
        global_typescript_7(&global, "7.0.2");
        let (node, cli) = global_ts_ls(&global, "6.0.2");
        let project = temp.path().join("project");
        let tsserver = typescript_with_tsserver(&project.join("node_modules/typescript"), "5.9.3");
        let tools = discover(&project, &roots).unwrap();
        assert_server(
            &tools,
            "typescript-language-server",
            "TypeScript 5.9.3 (project) via typescript-language-server 6.0.2",
            Launch::TypeScriptLanguageServer {
                node,
                server: cli,
                tsserver,
                vue_plugin: None,
            },
        );
        assert!(tools.notes.is_empty());
    }

    #[test]
    fn vue_projects_load_vues_typescript_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots, "v24.22.1");
        global_ts_ls(&global, "6.0.2");
        let project = temp.path().join("project");
        typescript_with_tsserver(&project.join("node_modules/typescript"), "5.9.3");
        let vue = global.join("lib/node_modules/@vue/language-server");
        package_json(&vue, "@vue/language-server", "3.3.14");
        write(&vue.join("bin/vue-language-server.js"), "// vue");
        package_json(
            &vue.join("node_modules/@vue/typescript-plugin"),
            "@vue/typescript-plugin",
            "3.3.14",
        );
        let plugin = |tools: &KindTools| match &tools.servers[0].launch {
            Launch::TypeScriptLanguageServer { vue_plugin, .. } => vue_plugin.clone(),
            other => panic!("unexpected {other:?}"),
        };
        // Not a Vue project: no plugin.
        let plain = discover(&project, &roots).unwrap();
        assert_eq!(plugin(&plain), None);

        package_json(&project.join("node_modules/vue"), "vue", "3.5.0");
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(plugin(&tools), Some(vue.canonicalize().unwrap()));
        assert_ne!(tools.fingerprint, plain.fingerprint);
    }

    #[test]
    fn project_typescript_5_without_ts_ls_falls_back_to_global_tsgo_with_a_note() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots, "v24.22.1");
        let tsc = global_typescript_7(&global, "7.0.2");
        let project = temp.path().join("project");
        typescript_with_tsserver(&project.join("node_modules/typescript"), "5.9.3");
        let tools = discover(&project, &roots).unwrap();
        assert_server(
            &tools,
            "tsgo",
            "TypeScript 7.0.2 (global)",
            Launch::Tsgo { server: tsc },
        );
        assert_eq!(
            tools.notes,
            vec![ToolNote {
                text: "Answers come from TypeScript 7.0.2, not the project's 5.9.3. Install typescript-language-server to use the project's TypeScript.".into(),
                command: Some("npm install -g typescript-language-server".into()),
            }]
        );

        std::fs::remove_dir_all(global.join("lib/node_modules/typescript")).unwrap();
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing.message,
            "This project's TypeScript 5.9.3 needs typescript-language-server."
        );
        assert_eq!(
            missing.install,
            vec!["npm install -g typescript-language-server".to_string()]
        );
    }

    #[test]
    fn without_project_typescript_the_global_one_serves() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let homebrew = &roots.homebrew[0];
        write(&homebrew.join("bin/node"), "node");
        let tsserver =
            typescript_with_tsserver(&homebrew.join("lib/node_modules/typescript"), "5.9.3");
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing.message,
            "The global TypeScript 5.9.3 needs typescript-language-server."
        );
        assert_eq!(
            missing.install,
            vec!["npm install -g typescript-language-server".to_string()]
        );

        let (node, cli) = global_ts_ls(homebrew, "6.0.2");
        assert_server(
            &discover(&project, &roots).unwrap(),
            "typescript-language-server",
            "TypeScript 5.9.3 (global) via typescript-language-server 6.0.2",
            Launch::TypeScriptLanguageServer {
                node,
                server: cli,
                tsserver,
                vue_plugin: None,
            },
        );

        let tsc = global_typescript_7(&nvm(&roots, "v24.22.1"), "7.0.2");
        assert_server(
            &discover(&project, &roots).unwrap(),
            "tsgo",
            "TypeScript 7.0.2 (global)",
            Launch::Tsgo { server: tsc },
        );
    }

    #[test]
    fn no_typescript_anywhere_reports_npm_install_g_typescript() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots, "v24.22.1");
        global_ts_ls(&global, "6.0.2");
        let project = temp.path().join("project");
        // A TypeScript without tsserver.js or a native compiler serves nothing.
        package_json(
            &project.join("node_modules/typescript"),
            "typescript",
            "7.0.2",
        );
        let missing = discover_kind(ToolKind::TypeScript, &project, &roots)
            .expect("TypeScript is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing.message,
            "JavaScript and TypeScript tools need TypeScript."
        );
        assert_eq!(
            missing.install,
            vec!["npm install -g typescript".to_string()]
        );
    }

    #[test]
    fn updating_the_project_typescript_changes_the_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        let tsc = project_typescript_7(&project, "7.0.2");
        let first = discover(&project, &roots).unwrap().fingerprint;
        assert_eq!(discover(&project, &roots).unwrap().fingerprint, first);

        write(&tsc, "native tsc, rebuilt");
        let rebuilt = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(rebuilt, first);

        package_json(
            &project.join("node_modules/typescript"),
            "typescript",
            "7.0.3",
        );
        let updated = discover(&project, &roots).unwrap();
        assert_eq!(updated.label, "TypeScript 7.0.3 (project)");
        assert_ne!(updated.fingerprint, rebuilt);

        let other = temp.path().join("other");
        let tsserver = typescript_with_tsserver(&other.join("node_modules/typescript"), "5.9.3");
        global_ts_ls(&nvm(&roots, "v24.22.1"), "6.0.2");
        let before = discover(&other, &roots).unwrap().fingerprint;
        write(&tsserver, "// tsserver 5.9.3, patched");
        assert_ne!(discover(&other, &roots).unwrap().fingerprint, before);
    }
}
