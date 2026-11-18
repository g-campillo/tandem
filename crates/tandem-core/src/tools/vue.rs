//! Vue: @vue/language-server for the component, with typescript-language-server as its TypeScript
//! companion. The companion runs the project's own TypeScript, 6 or older, with Vue's TypeScript
//! plugin; TypeScript 7 has no JavaScript API for the plugin. Both servers are global npm packages
//! run under the Node.js that installed them. Discovery never runs `node` or `npm`.
use super::node::{global_package, package_version};
use super::servers::{KindTools, Launch, MissingTools, ServerTools, ToolRoots, fingerprint};
use crate::ToolKind;
use std::path::{Path, PathBuf};

const INSTALL_VUE: &str = "npm install -g @vue/language-server";
/// The Vue server's command, relative to its package folder.
const VUE_SCRIPT: &str = "bin/vue-language-server.js";
/// typescript-language-server's command, relative to its package folder.
const TS_LS_SCRIPT: &str = "lib/cli.mjs";

/// The project's own TypeScript, which the companion runs with Vue's TypeScript plugin.
enum ProjectTypeScript {
    /// `lib` holds both `typescript.js` and `tsserver.js`: TypeScript 6 or older.
    Usable {
        lib: PathBuf,
        version: String,
    },
    /// TypeScript 7 or newer, which has no JavaScript API for the plugin.
    TooNew(String),
    Missing,
}

impl ProjectTypeScript {
    fn find(project: &Path) -> Self {
        let dir = project.join("node_modules/typescript");
        if !dir.join("package.json").is_file() {
            return Self::Missing;
        }
        let Ok(dir) = dir.canonicalize() else {
            return Self::Missing;
        };
        let Ok(version) = package_version(&dir) else {
            return Self::Missing;
        };
        let lib = dir.join("lib");
        if lib.join("typescript.js").is_file() && lib.join("tsserver.js").is_file() {
            return Self::Usable { lib, version };
        }
        let major = version
            .split(['.', '-'])
            .next()
            .and_then(|major| major.parse::<u64>().ok());
        if major.is_some_and(|major| major >= 7) {
            Self::TooNew(version)
        } else {
            Self::Missing
        }
    }
}

/// Vue's TypeScript plugin: in the server's own node_modules, else hoisted beside the server in the
/// global node_modules.
fn plugin_dir(vue: &Path) -> Option<PathBuf> {
    let plugin = Path::new("@vue/typescript-plugin");
    [
        Some(vue.join("node_modules").join(plugin)),
        vue.parent()
            .and_then(Path::parent)
            .map(|modules| modules.join(plugin)),
    ]
    .into_iter()
    .flatten()
    .find(|dir| dir.is_dir())
}

pub(crate) fn discover(project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let vue = global_package(roots, "@vue/language-server")
        .filter(|package| package.dir.join(VUE_SCRIPT).is_file());
    let plugin = vue.as_ref().and_then(|vue| plugin_dir(&vue.dir));
    let ts_ls = global_package(roots, "typescript-language-server")
        .filter(|package| package.dir.join(TS_LS_SCRIPT).is_file());
    let typescript = ProjectTypeScript::find(project);

    let mut needs = vec![];
    let mut install = vec![];
    if vue.is_none() {
        needs.push("@vue/language-server");
        install.push(INSTALL_VUE.to_string());
    } else if plugin.is_none() {
        // @vue/language-server brings the plugin along; reinstalling restores it.
        needs.push("@vue/typescript-plugin");
        install.push(INSTALL_VUE.to_string());
    }
    if ts_ls.is_none() {
        needs.push("typescript-language-server");
        install.push("npm install -g typescript-language-server".into());
    }
    if matches!(typescript, ProjectTypeScript::Missing) {
        needs.push("the project's own TypeScript (run npm install in the project folder)");
        install.push("npm install".into());
    }
    let mut problems = vec![];
    if !needs.is_empty() {
        problems.push(format!("Vue tools need: {}.", needs.join(", ")));
    }
    if let ProjectTypeScript::TooNew(version) = &typescript {
        problems.push(format!(
            "Vue's TypeScript plugin needs TypeScript 6 or older; this project uses TypeScript {version}."
        ));
    }
    let (Some(vue), Some(plugin), Some(ts_ls), ProjectTypeScript::Usable { lib, version }) =
        (vue, plugin, ts_ls, typescript)
    else {
        return Err(MissingTools {
            message: problems.join(" "),
            install,
        });
    };

    let server = vue.dir.join(VUE_SCRIPT);
    let ts_server = ts_ls.dir.join(TS_LS_SCRIPT);
    let title = format!("Vue {} with TypeScript {version} (project)", vue.version);
    let fingerprint = fingerprint(
        "vue-v1",
        &[title.clone(), plugin.display().to_string()],
        &[
            &vue.node,
            &server,
            &ts_ls.node,
            &ts_server,
            &lib.join("tsserver.js"),
            &lib.join("typescript.js"),
        ],
    )?;
    // The plugin folder is <location>/node_modules/@vue/typescript-plugin.
    let plugin_location = plugin
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Ok(KindTools {
        kind: ToolKind::Vue,
        label: title.clone(),
        servers: vec![ServerTools {
            name: "vue".into(),
            title,
            languages: vec!["vue".into()],
            launch: Launch::Vue {
                node: vue.node,
                server,
                tsdk: lib,
                ts_node: ts_ls.node,
                ts_server,
                plugin_location,
            },
        }],
        notes: vec![],
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolKind;
    use crate::tools::servers::{Launch, ServerTools, discover_kind};
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
    fn package_json(dir: &Path, name: &str, version: &str) {
        write(
            &dir.join("package.json"),
            &format!(r#"{{"name": "{name}", "version": "{version}"}}"#),
        );
    }
    /// A global npm root, as nvm lays one out, with its Node.js. Returns the root.
    fn nvm(roots: &ToolRoots) -> PathBuf {
        let root = roots.home.join(".nvm/versions/node/v24.22.1");
        write(&root.join("bin/node"), "node");
        root
    }
    /// Where `npm install -g @vue/language-server` puts Vue's TypeScript plugin.
    enum Plugin {
        /// In the server's own node_modules.
        Nested,
        /// Beside the server in the global node_modules.
        Hoisted,
        Absent,
    }
    /// @vue/language-server under `root`. Returns the canonical package folder.
    fn global_vue(root: &Path, version: &str, plugin: Plugin) -> PathBuf {
        let modules = root.join("lib/node_modules");
        let dir = modules.join("@vue/language-server");
        package_json(&dir, "@vue/language-server", version);
        write(
            &dir.join("bin/vue-language-server.js"),
            &format!("#!/usr/bin/env node\n// vue {version}\n"),
        );
        let plugin_dir = match plugin {
            Plugin::Nested => Some(dir.join("node_modules/@vue/typescript-plugin")),
            Plugin::Hoisted => Some(modules.join("@vue/typescript-plugin")),
            Plugin::Absent => None,
        };
        if let Some(plugin_dir) = plugin_dir {
            package_json(&plugin_dir, "@vue/typescript-plugin", version);
        }
        dir.canonicalize().unwrap()
    }
    /// typescript-language-server under `root`. Returns its canonical `lib/cli.mjs`.
    fn global_ts_ls(root: &Path, version: &str) -> PathBuf {
        let dir = root.join("lib/node_modules/typescript-language-server");
        package_json(&dir, "typescript-language-server", version);
        write(&dir.join("lib/cli.mjs"), &format!("// cli {version}"));
        dir.join("lib/cli.mjs").canonicalize().unwrap()
    }
    /// The project's TypeScript 6 or older in node_modules. Returns its canonical `lib` folder.
    fn project_typescript(project: &Path, version: &str) -> PathBuf {
        let dir = project.join("node_modules/typescript");
        package_json(&dir, "typescript", version);
        write(
            &dir.join("lib/tsserver.js"),
            &format!("// tsserver {version}"),
        );
        write(
            &dir.join("lib/typescript.js"),
            &format!("// typescript {version}"),
        );
        dir.join("lib").canonicalize().unwrap()
    }

    #[test]
    fn needs_all_parts_and_lists_every_missing_install() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let missing = discover_kind(ToolKind::Vue, &project, &roots)
            .expect("Vue is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "Vue tools need: @vue/language-server, typescript-language-server, the project's own TypeScript (run npm install in the project folder).".into(),
                install: vec![
                    "npm install -g @vue/language-server".into(),
                    "npm install -g typescript-language-server".into(),
                    "npm install".into(),
                ],
            }
        );

        // Each part found drops out of the list.
        let global = nvm(&roots);
        global_ts_ls(&global, "6.0.2");
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing.install,
            ["npm install -g @vue/language-server", "npm install"]
        );
        project_typescript(&project, "5.9.3");
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "Vue tools need: @vue/language-server.".into(),
                install: vec!["npm install -g @vue/language-server".into()],
            }
        );

        // A Vue server without its TypeScript plugin is reinstalled, which brings the plugin.
        global_vue(&global, "3.3.14", Plugin::Absent);
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "Vue tools need: @vue/typescript-plugin.".into(),
                install: vec!["npm install -g @vue/language-server".into()],
            }
        );

        global_vue(&global, "3.3.14", Plugin::Nested);
        assert!(discover(&project, &roots).is_ok());

        // A project TypeScript without its JavaScript API counts as not installed.
        std::fs::remove_file(project.join("node_modules/typescript/lib/typescript.js")).unwrap();
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(missing.install, ["npm install"]);
    }

    #[test]
    fn finds_the_nested_plugin_location() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots);
        let vue = global_vue(&global, "3.3.14", Plugin::Nested);
        let cli = global_ts_ls(&global, "6.0.2");
        let project = temp.path().join("project");
        let tsdk = project_typescript(&project, "5.9.3");

        let tools = discover_kind(ToolKind::Vue, &project, &roots)
            .expect("Vue is served through the registry")
            .unwrap();
        assert_eq!(tools.kind, ToolKind::Vue);
        assert_eq!(tools.label, "Vue 3.3.14 with TypeScript 5.9.3 (project)");
        assert!(tools.notes.is_empty());
        assert_eq!(
            tools.servers,
            vec![ServerTools {
                name: "vue".into(),
                title: "Vue 3.3.14 with TypeScript 5.9.3 (project)".into(),
                languages: vec!["vue".into()],
                launch: Launch::Vue {
                    node: global.join("bin/node"),
                    server: vue.join("bin/vue-language-server.js"),
                    tsdk: tsdk.clone(),
                    ts_node: global.join("bin/node"),
                    ts_server: cli,
                    plugin_location: vue.clone(),
                },
            }]
        );
        assert!(
            vue.join("node_modules/@vue/typescript-plugin").is_dir(),
            "plugin_location/node_modules/@vue/typescript-plugin is the plugin"
        );

        // Trust is asked again when the project's TypeScript or a server changes.
        let first = tools.fingerprint;
        assert!(!first.is_empty());
        assert_eq!(discover(&project, &roots).unwrap().fingerprint, first);
        write(&tsdk.join("typescript.js"), "// typescript, patched");
        let patched = discover(&project, &roots).unwrap().fingerprint;
        assert_ne!(patched, first);
        write(&vue.join("bin/vue-language-server.js"), "// vue, patched");
        assert_ne!(discover(&project, &roots).unwrap().fingerprint, patched);

        // npm can also hoist the plugin beside the server in the global node_modules.
        let homebrew = &roots.homebrew[0];
        std::fs::remove_dir_all(global.join("lib/node_modules/@vue")).unwrap();
        write(&homebrew.join("bin/node"), "node");
        global_vue(homebrew, "3.3.14", Plugin::Hoisted);
        let tools = discover(&project, &roots).unwrap();
        let Launch::Vue {
            node,
            plugin_location,
            ..
        } = &tools.servers[0].launch
        else {
            panic!("{:?}", tools.servers[0].launch);
        };
        assert_eq!(node, &homebrew.join("bin/node"));
        let lib = homebrew.join("lib").canonicalize().unwrap();
        assert_eq!(plugin_location, &lib);
        assert!(lib.join("node_modules/@vue/typescript-plugin").is_dir());
    }

    #[test]
    fn refuses_project_typescript_7() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let global = nvm(&roots);
        global_ts_ls(&global, "6.0.2");
        let project = temp.path().join("project");
        // TypeScript 7 ships its native compiler and no tsserver.js or typescript.js.
        let typescript = project.join("node_modules/typescript");
        package_json(&typescript, "typescript", "7.0.2");
        write(&typescript.join("lib/tsc.js"), "// launcher");

        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "Vue tools need: @vue/language-server. Vue's TypeScript plugin needs TypeScript 6 or older; this project uses TypeScript 7.0.2.".into(),
                install: vec!["npm install -g @vue/language-server".into()],
            }
        );

        global_vue(&global, "3.3.14", Plugin::Nested);
        let missing = discover(&project, &roots).unwrap_err();
        assert_eq!(
            missing,
            MissingTools {
                message: "Vue's TypeScript plugin needs TypeScript 6 or older; this project uses TypeScript 7.0.2.".into(),
                install: vec![],
            }
        );
    }
}
