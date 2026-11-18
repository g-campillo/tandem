//! How Tandem launches each language server it runs: program, arguments, environment and settings.
use crate::{DiagnosticMode, ServerConfig};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;
use tandem_core::{Launch, NodeServer};

/// rust-analyzer from the project's toolchain, offline, with build scripts and proc macros on (as
/// cargo build runs them), so derived and generated code is known.
pub fn rust_server(tools: &tandem_core::RustTools, project: &Path, cache: &Path) -> ServerConfig {
    let bin = tools.toolchain.join("bin");
    let mut config = ServerConfig::new("rust-analyzer", &tools.rust_analyzer, project, cache);
    config.ready_detail = "rust-analyzer ready · build scripts and proc macros on".into();
    config.env = vec![
        ("RUSTC".into(), bin.join("rustc").into()),
        ("RUSTDOC".into(), bin.join("rustdoc").into()),
        ("CARGO_NET_OFFLINE".into(), "true".into()),
    ];
    config.path = vec![bin];
    config.initialization_options = json!({"checkOnSave":false,"cargo":{"extraArgs":["--offline"],"buildScripts":{"enable":true},"targetDir":cache},
        "procMacro":{"enable":true},"files":{"watcher":"server"},"completion":{"autoimport":{"enable":true}},"rustfmt":{"extraArgs":[]}});
    config
}

/// Pyright under Node, analysing open files with the selected interpreter.
/// `None` when Node or the Pyright script was not found.
pub fn python_server(
    tools: &tandem_core::PythonTools,
    project: &Path,
    cache: &Path,
) -> Option<ServerConfig> {
    let (node, script) = (tools.node.as_ref()?, tools.server.as_ref()?);
    let mut config = ServerConfig::new("Pyright", node, project, cache);
    config.ready_detail = "Pyright ready · selected local interpreter".into();
    config.args = vec![script.into(), "--stdio".into()];
    config.path = vec![tools.prefix.join("bin")];
    config.configuration = json!({"python":{"pythonPath":tools.interpreter,"analysis":{"diagnosticMode":"openFilesOnly","typeCheckingMode":"basic","autoImportCompletions":true,"autoSearchPaths":true}},"pyright":{"disableOrganizeImports":false}});
    config.push_configuration = true;
    config.resend_on_change = true;
    Some(config)
}

/// A server found by registry discovery (`tandem_core::discover_kind`), its builds and indexes
/// kept in `cache`.
pub fn server_config(
    server: &tandem_core::ServerTools,
    project: &Path,
    cache: &Path,
) -> ServerConfig {
    let mut config = launch_config(server, project, cache);
    config.prepare = server.prepare.clone();
    config
}

fn launch_config(server: &tandem_core::ServerTools, project: &Path, cache: &Path) -> ServerConfig {
    match &server.launch {
        Launch::SourceKit {
            server: program,
            developer_dir,
            extra_path,
            xcode,
        } => {
            let mut config = ServerConfig::new(&server.title, program, project, cache);
            config.args = vec!["--scratch-path".into(), cache.join("build").into()];
            config.xcode_setup = xcode.clone();
            config.path = program
                .parent()
                .map(Path::to_path_buf)
                .into_iter()
                .chain(extra_path.iter().cloned())
                .collect();
            config.env = vec![("DEVELOPER_DIR".into(), developer_dir.into())];
            config.capabilities =
                json!({"textDocument":{"diagnostic":{"dynamicRegistration":true}}});
            config.diagnostics = DiagnosticMode::Settled;
            config.start_timeout = Duration::from_secs(30);
            config.request_timeout = Duration::from_secs(30);
            config.ready_detail = format!("{} ready · indexes in Tandem's cache", server.title);
            config
        }
        Launch::Jdtls {
            launcher,
            java_home,
            trigger_files,
        } => {
            let mut config = ServerConfig::new(&server.title, launcher, project, cache);
            config.args = vec!["-data".into(), cache.join("workspace").into()];
            config.path = vec![java_home.join("bin")];
            config.env = vec![("JAVA_HOME".into(), java_home.into())];
            let settings = json!({"java":{"import":{"generatesMetadataFilesAtProjectRoot":false,"gradle":{"enabled":true,"offline":{"enabled":true}},"maven":{"enabled":true,"offline":{"enabled":true}}},"maven":{"downloadSources":false},"eclipse":{"downloadSources":false},"format":{"enabled":true},"autobuild":{"enabled":true}}});
            config.initialization_options = json!({ "settings": settings });
            if !trigger_files.is_empty() {
                // jdtls infers a plain folder's source folders from these files' packages.
                config.initialization_options["triggerFiles"] = trigger_files
                    .iter()
                    .filter_map(|file| url::Url::from_file_path(file).ok())
                    .map(|uri| Value::String(uri.to_string()))
                    .collect();
            }
            config.configuration = settings;
            config.push_configuration = true;
            // jdtls pushes diagnostics without versions.
            config.diagnostics = DiagnosticMode::Settled;
            // initialize takes seconds; Maven and Gradle imports and first answers can take longer.
            config.start_timeout = Duration::from_secs(60);
            config.request_timeout = Duration::from_secs(60);
            config.ready_detail = "jdtls ready · Maven and Gradle import offline".into();
            config
        }
        Launch::Tsgo { server: program } => {
            let mut config = typescript_config(&server.title, program, project, cache);
            config.args = vec!["--lsp".into(), "--stdio".into()];
            config
        }
        Launch::TypeScriptLanguageServer {
            node,
            server: script,
            tsserver,
            vue_plugin,
        } => {
            let mut config = typescript_config(&server.title, node, project, cache);
            config.args = vec![script.into(), "--stdio".into()];
            config.path = node.parent().map(Path::to_path_buf).into_iter().collect();
            // Without automatic typing acquisition tsserver never runs `npm install @types/...`.
            config.initialization_options = json!({
                "tsserver": {"path": tsserver},
                "disableAutomaticTypingAcquisition": true,
                "preferences": inlay_preferences(),
            });
            if let Some(location) = vue_plugin {
                config.initialization_options["plugins"] = json!([{
                    "name": "@vue/typescript-plugin",
                    "location": location,
                    "languages": ["vue"],
                }]);
            }
            config
        }
        Launch::Node {
            node,
            script,
            server: kind,
        } => {
            let mut config = ServerConfig::new(&server.title, node, project, cache);
            config.args = vec![script.into(), "--stdio".into()];
            config.path = node.parent().map(Path::to_path_buf).into_iter().collect();
            (config.initialization_options, config.configuration) = node_settings(*kind);
            config.push_configuration = true;
            // The HTML, CSS and JSON servers pull diagnostics when the client declares
            // textDocument.diagnostic, as Tandem does; svelte-language-server pushes them without
            // versions. `Settled` takes either.
            config.diagnostics = DiagnosticMode::Settled;
            config.start_timeout = Duration::from_secs(30);
            config.request_timeout = Duration::from_secs(30);
            config
        }
        Launch::Vue {
            node,
            server: script,
            tsdk,
            ts_node,
            ts_server,
            plugin_location,
        } => {
            // Without --tsdk the Vue server loads its own peer TypeScript, which npm resolves to
            // TypeScript 7, and that has no JavaScript API.
            let mut tsdk_arg = OsString::from("--tsdk=");
            tsdk_arg.push(tsdk);
            let mut config = ServerConfig::new(&server.title, node, project, cache);
            config.args = vec![script.into(), "--stdio".into(), tsdk_arg];
            config.path = node.parent().map(Path::to_path_buf).into_iter().collect();
            config.initialization_options = json!({"typescript": {"tsdk": tsdk}});
            config.diagnostics = DiagnosticMode::Settled;
            config.start_timeout = Duration::from_secs(30);
            config.request_timeout = Duration::from_secs(30);
            config.ready_detail = format!("{} ready", server.title);
            // The Vue server asks this one for type information through tsserver/request, and
            // its answers for the script are merged with the Vue server's.
            let mut companion = ServerConfig::new(
                "typescript-language-server (Vue)",
                ts_node,
                project,
                cache.join("typescript"),
            );
            companion.args = vec![ts_server.into(), "--stdio".into()];
            companion.path = ts_node
                .parent()
                .map(Path::to_path_buf)
                .into_iter()
                .collect();
            companion.initialization_options = json!({
                "tsserver": {"path": tsdk.join("tsserver.js")},
                "disableAutomaticTypingAcquisition": true,
                "plugins": [{
                    "name": "@vue/typescript-plugin",
                    "location": plugin_location,
                    "languages": ["vue"],
                }],
            });
            companion.diagnostics = DiagnosticMode::Settled;
            companion.start_timeout = Duration::from_secs(30);
            companion.request_timeout = Duration::from_secs(30);
            config.companion = Some(Box::new(companion));
            config
        }
    }
}

/// Initialization options and settings of each Node.js server. The JSON server reads only `file:`
/// schemas: it asks the client for any other, and gets null, so nothing is downloaded.
fn node_settings(server: NodeServer) -> (Value, Value) {
    match server {
        NodeServer::Html => (
            json!({"provideFormatter":true,"embeddedLanguages":{"css":true,"javascript":true}}),
            json!({"html":{"format":{"enable":true}},"css":{"validate":true},"javascript":{"validate":{"enable":true}}}),
        ),
        NodeServer::Css => (
            json!({"provideFormatter":true}),
            json!({"css":{"validate":true},"scss":{"validate":true},"less":{"validate":true}}),
        ),
        NodeServer::Json => (
            json!({"provideFormatter":true,"handledSchemaProtocols":["file"]}),
            json!({"json":{"validate":{"enable":true},"format":{"enable":true},"schemas":[]}}),
        ),
        // Its defaults: checks with its own TypeScript, formats with its bundled
        // prettier-plugin-svelte, and pushes diagnostics without versions, which `Settled` takes.
        NodeServer::Svelte => (json!({}), json!({})),
    }
}

/// Settings both TypeScript servers share. tsgo pulls diagnostics; `Settled` is the fallback for
/// pushes, which typescript-language-server sends without versions.
fn typescript_config(title: &str, program: &Path, project: &Path, cache: &Path) -> ServerConfig {
    let mut config = ServerConfig::new(title, program, project, cache);
    config.configuration = typescript_settings();
    config.diagnostics = DiagnosticMode::Settled;
    config.start_timeout = Duration::from_secs(30);
    config.request_timeout = Duration::from_secs(30);
    config
}

/// tsserver's preferences for type hints and parameter-name hints.
fn inlay_preferences() -> Value {
    json!({
        "includeInlayParameterNameHints": "all",
        "includeInlayParameterNameHintsWhenArgumentMatchesName": false,
        "includeInlayFunctionParameterTypeHints": true,
        "includeInlayVariableTypeHints": true,
        "includeInlayVariableTypeHintsWhenTypeMatchesName": false,
        "includeInlayPropertyDeclarationTypeHints": true,
        "includeInlayFunctionLikeReturnTypeHints": true,
        "includeInlayEnumMemberValueHints": true,
    })
}

/// VS Code's `typescript` and `javascript` settings, as workspace/configuration answers them: the
/// same hints, and no automatic type acquisition. tsgo 7.0.2 otherwise runs
/// `npm install @types/...` in ~/Library/Caches/typescript when it finds npm; either setting stops it.
fn typescript_settings() -> Value {
    let settings = json!({
        "inlayHints": {
            "parameterNames": {"enabled": "all", "suppressWhenArgumentMatchesName": true},
            "parameterTypes": {"enabled": true},
            "variableTypes": {"enabled": true, "suppressWhenTypeMatchesName": true},
            "propertyDeclarationTypes": {"enabled": true},
            "functionLikeReturnTypes": {"enabled": true},
            "enumMemberValues": {"enabled": true},
        },
        "tsserver": {"automaticTypeAcquisition": {"enabled": false}},
        "disableAutomaticTypeAcquisition": true,
    });
    json!({"typescript": settings, "javascript": settings})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiagnosticMode;
    use std::path::PathBuf;

    #[test]
    fn rust_server_keeps_today_s_settings() {
        let toolchain = PathBuf::from("/home/me/.rustup/toolchains/1.100.0-aarch64-apple-darwin");
        let tools = tandem_core::RustTools {
            toolchain: toolchain.clone(),
            rust_analyzer: toolchain.join("bin/rust-analyzer"),
            cargo: toolchain.join("bin/cargo"),
            rustc: toolchain.join("bin/rustc"),
            rustfmt: toolchain.join("bin/rustfmt"),
            label: "Rust 1.100.0".into(),
            fingerprint: "f".into(),
        };
        let config = rust_server(
            &tools,
            Path::new("/work/app"),
            Path::new("/state/cache/rust"),
        );
        assert_eq!(config.name, "rust-analyzer");
        assert_eq!(config.program, toolchain.join("bin/rust-analyzer"));
        assert!(config.args.is_empty());
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, PathBuf::from("/state/cache/rust"));
        assert_eq!(config.path, vec![toolchain.join("bin")]);
        assert_eq!(
            config.env,
            vec![
                ("RUSTC".into(), OsString::from(toolchain.join("bin/rustc"))),
                (
                    "RUSTDOC".into(),
                    OsString::from(toolchain.join("bin/rustdoc"))
                ),
                ("CARGO_NET_OFFLINE".into(), OsString::from("true")),
            ]
        );
        assert_eq!(
            config.initialization_options.pointer("/cargo/targetDir"),
            Some(&json!("/state/cache/rust"))
        );
        assert_eq!(
            config.initialization_options.pointer("/procMacro/enable"),
            Some(&json!(true))
        );
        assert_eq!(
            config
                .initialization_options
                .pointer("/cargo/buildScripts/enable"),
            Some(&json!(true))
        );
        assert_eq!(
            config.initialization_options.pointer("/cargo/extraArgs"),
            Some(&json!(["--offline"]))
        );
        assert_eq!(config.diagnostics, DiagnosticMode::Versioned);
        assert_eq!(
            config.ready_detail,
            "rust-analyzer ready · build scripts and proc macros on"
        );
        assert_eq!(config.configuration, json!({}));
        assert!(!config.push_configuration);
        assert!(!config.resend_on_change);
    }

    #[test]
    fn python_server_runs_pyright_with_the_interpreter() {
        let mut tools = tandem_core::PythonTools {
            interpreter: "/work/app/.venv/bin/python".into(),
            executable: "/opt/python/bin/python3.13".into(),
            prefix: "/opt/node".into(),
            environment: "/work/app/.venv".into(),
            node: Some("/opt/node/bin/node".into()),
            server: Some("/opt/node/lib/pyright/langserver.index.js".into()),
            label: "Python 3.13".into(),
            fingerprint: "f".into(),
        };
        let config = python_server(&tools, Path::new("/work/app"), Path::new("/state/py"))
            .expect("Node and Pyright are present");
        assert_eq!(config.name, "Pyright");
        assert_eq!(config.program, PathBuf::from("/opt/node/bin/node"));
        assert_eq!(
            config.args,
            vec![
                OsString::from("/opt/node/lib/pyright/langserver.index.js"),
                OsString::from("--stdio")
            ]
        );
        assert_eq!(config.path, vec![PathBuf::from("/opt/node/bin")]);
        assert!(config.env.is_empty());
        assert_eq!(
            config.configuration.pointer("/python/pythonPath"),
            Some(&json!("/work/app/.venv/bin/python"))
        );
        assert_eq!(
            config
                .configuration
                .pointer("/python/analysis/diagnosticMode"),
            Some(&json!("openFilesOnly"))
        );
        // Finds packages under src/ that are not installed, as the pyright command does.
        assert_eq!(
            config
                .configuration
                .pointer("/python/analysis/autoSearchPaths"),
            Some(&json!(true))
        );
        assert!(config.push_configuration);
        assert!(config.resend_on_change);
        assert!(config.initialization_options.is_null());
        assert_eq!(config.diagnostics, DiagnosticMode::Versioned);
        assert_eq!(
            config.ready_detail,
            "Pyright ready · selected local interpreter"
        );

        tools.node = None;
        assert!(python_server(&tools, Path::new("/work/app"), Path::new("/state/py")).is_none());
        tools.node = Some("/opt/node/bin/node".into());
        tools.server = None;
        assert!(python_server(&tools, Path::new("/work/app"), Path::new("/state/py")).is_none());
    }

    #[test]
    fn sourcekit_config_keeps_builds_out_of_the_project() {
        let developer = PathBuf::from("/Applications/Xcode.app/Contents/Developer");
        let program = developer.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp");
        let tools = tandem_core::ServerTools {
            prepare: None,
            name: "sourcekit-lsp".into(),
            title: "SourceKit-LSP (Xcode 26.1)".into(),
            languages: vec!["swift".into()],
            launch: tandem_core::Launch::SourceKit {
                server: program.clone(),
                developer_dir: developer.clone(),
                extra_path: vec!["/opt/homebrew/bin".into()],
                xcode: None,
            },
        };
        let cache = Path::new("/state/language-cache/p/swift/sourcekit-lsp");
        let config = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(config.name, "SourceKit-LSP (Xcode 26.1)");
        assert_eq!(config.program, program);
        assert_eq!(
            config.args,
            vec![
                OsString::from("--scratch-path"),
                OsString::from(cache.join("build"))
            ]
        );
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, cache);
        assert_eq!(
            config.path,
            vec![
                developer.join("Toolchains/XcodeDefault.xctoolchain/usr/bin"),
                PathBuf::from("/opt/homebrew/bin")
            ]
        );
        assert_eq!(
            config.env,
            vec![("DEVELOPER_DIR".into(), OsString::from(&developer))]
        );
        assert_eq!(
            config.capabilities,
            json!({"textDocument":{"diagnostic":{"dynamicRegistration":true}}})
        );
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(30));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(
            config.ready_detail,
            "SourceKit-LSP (Xcode 26.1) ready · indexes in Tandem's cache"
        );
        assert!(config.initialization_options.is_null());
        assert!(!config.push_configuration);
        assert!(!config.resend_on_change);
        assert_eq!(config.xcode_setup, None);
    }

    #[test]
    fn sourcekit_config_carries_the_xcode_setup() {
        let setup = tandem_core::XcodeSetup {
            path: "/work/app/buildServer.json".into(),
            config: "{}\n".into(),
            tool: "/opt/homebrew/bin/xcode-build-server".into(),
            build_root: "/work/app/build".into(),
            scheme: "App".into(),
            flags: "/Users/me/Library/Caches/xcode-build-server/-work-app/compile_file-App-0"
                .into(),
            replaces: None,
        };
        let tools = tandem_core::ServerTools {
            prepare: None,
            name: "sourcekit-lsp".into(),
            title: "SourceKit-LSP (Xcode 26.1)".into(),
            languages: vec!["swift".into()],
            launch: tandem_core::Launch::SourceKit {
                server: "/x/sourcekit-lsp".into(),
                developer_dir: "/x".into(),
                extra_path: vec![],
                xcode: Some(setup.clone()),
            },
        };
        let config = server_config(&tools, Path::new("/work/app"), Path::new("/state/swift"));
        assert_eq!(config.xcode_setup, Some(setup));
    }

    fn tsgo_tools() -> tandem_core::ServerTools {
        tandem_core::ServerTools {
            prepare: None,
            name: "tsgo".into(),
            title: "TypeScript 7.0.2 (global)".into(),
            languages: vec!["typescript".into(), "javascript".into()],
            launch: tandem_core::Launch::Tsgo {
                server: "/n/lib/node_modules/typescript/node_modules/@typescript/typescript-darwin-arm64/lib/tsc".into(),
            },
        }
    }

    fn ts_ls_tools() -> tandem_core::ServerTools {
        tandem_core::ServerTools {
            prepare: None,
            name: "typescript-language-server".into(),
            title: "TypeScript 5.9.3 (project) via typescript-language-server 6.0.2".into(),
            languages: vec!["typescript".into(), "javascript".into()],
            launch: tandem_core::Launch::TypeScriptLanguageServer {
                node: "/n/bin/node".into(),
                server: "/n/lib/node_modules/typescript-language-server/lib/cli.mjs".into(),
                tsserver: "/work/app/node_modules/typescript/lib/tsserver.js".into(),
                vue_plugin: None,
            },
        }
    }

    #[test]
    fn tsgo_config_runs_the_native_lsp_mode() {
        let cache = Path::new("/state/language-cache/p/typescript/tsgo");
        let config = server_config(&tsgo_tools(), Path::new("/work/app"), cache);
        assert_eq!(config.name, "TypeScript 7.0.2 (global)");
        assert_eq!(
            config.program,
            PathBuf::from(
                "/n/lib/node_modules/typescript/node_modules/@typescript/typescript-darwin-arm64/lib/tsc"
            )
        );
        assert_eq!(
            config.args,
            vec![OsString::from("--lsp"), OsString::from("--stdio")]
        );
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, cache);
        assert!(config.path.is_empty());
        assert!(config.env.is_empty());
        assert!(config.initialization_options.is_null());
        assert_eq!(config.capabilities, json!({}));
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(30));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(config.ready_detail, "TypeScript 7.0.2 (global) ready");
        assert!(!config.resend_on_change);
    }

    #[test]
    fn ts_ls_config_loads_vues_plugin_and_carries_the_prepare_step() {
        let mut tools = ts_ls_tools();
        let cache = Path::new("/state/ts");
        let plain = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(plain.initialization_options.pointer("/plugins"), None);
        assert_eq!(plain.prepare, None);

        let tandem_core::Launch::TypeScriptLanguageServer { vue_plugin, .. } = &mut tools.launch
        else {
            unreachable!()
        };
        *vue_plugin = Some("/n/lib/node_modules/@vue/language-server".into());
        let prepare = tandem_core::Prepare {
            title: "nuxt prepare".into(),
            program: "/n/bin/node".into(),
            args: vec!["nuxt.mjs".into(), "prepare".into()],
            watch: vec!["app/composables/".into()],
        };
        tools.prepare = Some(prepare.clone());
        let config = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(
            config.initialization_options.pointer("/plugins"),
            Some(&json!([{
                "name": "@vue/typescript-plugin",
                "location": "/n/lib/node_modules/@vue/language-server",
                "languages": ["vue"],
            }]))
        );
        assert_eq!(config.prepare, Some(prepare));
    }

    #[test]
    fn ts_ls_config_disables_type_acquisition() {
        let cache = Path::new("/state/language-cache/p/typescript/typescript-language-server");
        let config = server_config(&ts_ls_tools(), Path::new("/work/app"), cache);
        assert_eq!(
            config.name,
            "TypeScript 5.9.3 (project) via typescript-language-server 6.0.2"
        );
        assert_eq!(config.program, PathBuf::from("/n/bin/node"));
        assert_eq!(
            config.args,
            vec![
                OsString::from("/n/lib/node_modules/typescript-language-server/lib/cli.mjs"),
                OsString::from("--stdio")
            ]
        );
        assert_eq!(config.path, vec![PathBuf::from("/n/bin")]);
        assert!(config.env.is_empty());
        assert_eq!(config.cache, cache);
        assert_eq!(
            config.initialization_options.pointer("/tsserver/path"),
            Some(&json!("/work/app/node_modules/typescript/lib/tsserver.js"))
        );
        assert_eq!(
            config
                .initialization_options
                .pointer("/disableAutomaticTypingAcquisition"),
            Some(&json!(true))
        );
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(30));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(
            config.ready_detail,
            "TypeScript 5.9.3 (project) via typescript-language-server 6.0.2 ready"
        );
    }

    #[test]
    fn ts_ls_config_sets_inlay_hint_preferences() {
        let config = server_config(
            &ts_ls_tools(),
            Path::new("/work/app"),
            Path::new("/state/ts"),
        );
        assert_eq!(
            config.initialization_options["preferences"],
            json!({"includeInlayParameterNameHints":"all","includeInlayParameterNameHintsWhenArgumentMatchesName":false,"includeInlayFunctionParameterTypeHints":true,"includeInlayVariableTypeHints":true,"includeInlayVariableTypeHintsWhenTypeMatchesName":false,"includeInlayPropertyDeclarationTypeHints":true,"includeInlayFunctionLikeReturnTypeHints":true,"includeInlayEnumMemberValueHints":true})
        );
    }

    #[test]
    fn typescript_configuration_answers_inlay_hint_settings() {
        let hints = json!({"parameterNames":{"enabled":"all","suppressWhenArgumentMatchesName":true},"parameterTypes":{"enabled":true},"variableTypes":{"enabled":true,"suppressWhenTypeMatchesName":true},"propertyDeclarationTypes":{"enabled":true},"functionLikeReturnTypes":{"enabled":true},"enumMemberValues":{"enabled":true}});
        for tools in [tsgo_tools(), ts_ls_tools()] {
            let config = server_config(&tools, Path::new("/work/app"), Path::new("/state/ts"));
            // workspace/configuration answers each item's section from this value.
            for section in ["typescript", "javascript"] {
                assert_eq!(
                    config.configuration[section]["inlayHints"], hints,
                    "{} {section}",
                    tools.name
                );
            }
        }
    }

    #[test]
    fn typescript_configuration_turns_off_type_acquisition() {
        // tsgo otherwise runs `npm install @types/...` in ~/Library/Caches/typescript when npm
        // is on its PATH; each of these settings stops it.
        for tools in [tsgo_tools(), ts_ls_tools()] {
            let config = server_config(&tools, Path::new("/work/app"), Path::new("/state/ts"));
            for section in ["typescript", "javascript"] {
                let settings = &config.configuration[section];
                assert_eq!(
                    settings.pointer("/tsserver/automaticTypeAcquisition/enabled"),
                    Some(&json!(false)),
                    "{} {section}",
                    tools.name
                );
                assert_eq!(
                    settings["disableAutomaticTypeAcquisition"],
                    json!(true),
                    "{} {section}",
                    tools.name
                );
            }
        }
    }

    #[test]
    fn web_configs_enable_formatting_and_no_schema_downloads() {
        let package = PathBuf::from("/n/lib/node_modules/vscode-langservers-extracted");
        let web = |name: &str, title: &str, kind| tandem_core::ServerTools {
            prepare: None,
            name: name.into(),
            title: title.into(),
            languages: vec![name.into()],
            launch: tandem_core::Launch::Node {
                node: "/n/bin/node".into(),
                script: package.join(format!("bin/vscode-{name}-language-server")),
                server: kind,
            },
        };
        let configs = [
            ("html", "HTML language server", NodeServer::Html),
            ("css", "CSS language server", NodeServer::Css),
            ("json", "JSON language server", NodeServer::Json),
        ]
        .map(|(name, title, kind)| {
            let cache = PathBuf::from("/state/language-cache/p/web").join(name);
            let config = server_config(&web(name, title, kind), Path::new("/work/app"), &cache);
            assert_eq!(config.name, title);
            assert_eq!(config.program, PathBuf::from("/n/bin/node"));
            assert_eq!(
                config.args,
                vec![
                    OsString::from(package.join(format!("bin/vscode-{name}-language-server"))),
                    OsString::from("--stdio")
                ]
            );
            assert_eq!(config.project, PathBuf::from("/work/app"));
            assert_eq!(config.cache, cache);
            assert_eq!(config.path, vec![PathBuf::from("/n/bin")]);
            assert!(config.env.is_empty());
            assert_eq!(
                config.initialization_options["provideFormatter"],
                json!(true),
                "{name}"
            );
            assert!(config.push_configuration, "{name}");
            assert!(!config.resend_on_change, "{name}");
            assert_eq!(config.capabilities, json!({}));
            assert_eq!(config.diagnostics, DiagnosticMode::Settled);
            assert_eq!(config.start_timeout, Duration::from_secs(30));
            assert_eq!(config.request_timeout, Duration::from_secs(30));
            assert_eq!(config.ready_detail, format!("{title} ready"));
            config
        });
        let [html, css, json] = configs;
        assert_eq!(
            html.initialization_options,
            json!({"provideFormatter":true,"embeddedLanguages":{"css":true,"javascript":true}})
        );
        assert_eq!(
            html.configuration,
            json!({"html":{"format":{"enable":true}},"css":{"validate":true},"javascript":{"validate":{"enable":true}}})
        );
        assert_eq!(css.initialization_options, json!({"provideFormatter":true}));
        assert_eq!(
            css.configuration,
            json!({"css":{"validate":true},"scss":{"validate":true},"less":{"validate":true}})
        );
        // Only file: schemas are read; the server asks the client for any other and gets null.
        assert_eq!(
            json.initialization_options["handledSchemaProtocols"],
            json!(["file"])
        );
        assert_eq!(
            json.configuration,
            json!({"json":{"validate":{"enable":true},"format":{"enable":true},"schemas":[]}})
        );
    }

    #[test]
    fn svelte_config_uses_settled_diagnostics() {
        let script = PathBuf::from("/n/lib/node_modules/svelte-language-server/bin/server.js");
        let tools = tandem_core::ServerTools {
            prepare: None,
            name: "svelte".into(),
            title: "svelte-language-server 0.17.21".into(),
            languages: vec!["svelte".into()],
            launch: tandem_core::Launch::Node {
                node: "/n/bin/node".into(),
                script: script.clone(),
                server: NodeServer::Svelte,
            },
        };
        let cache = Path::new("/state/language-cache/p/svelte/svelte");
        let config = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(config.name, "svelte-language-server 0.17.21");
        assert_eq!(config.program, PathBuf::from("/n/bin/node"));
        assert_eq!(
            config.args,
            vec![OsString::from(script), OsString::from("--stdio")]
        );
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, cache);
        assert_eq!(config.path, vec![PathBuf::from("/n/bin")]);
        assert!(config.env.is_empty());
        assert_eq!(config.initialization_options, json!({}));
        assert_eq!(config.configuration, json!({}));
        assert!(config.push_configuration);
        assert!(!config.resend_on_change);
        assert_eq!(config.capabilities, json!({}));
        // svelte-language-server pushes diagnostics without versions.
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(30));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(config.ready_detail, "svelte-language-server 0.17.21 ready");
    }

    #[test]
    fn vue_config_has_a_typescript_companion_with_the_plugin() {
        let tsdk = PathBuf::from("/work/app/node_modules/typescript/lib");
        let vue = PathBuf::from("/n/lib/node_modules/@vue/language-server");
        let tools = tandem_core::ServerTools {
            prepare: None,
            name: "vue".into(),
            title: "Vue 3.3.14 with TypeScript 5.9.3 (project)".into(),
            languages: vec!["vue".into()],
            launch: tandem_core::Launch::Vue {
                node: "/n/bin/node".into(),
                server: vue.join("bin/vue-language-server.js"),
                tsdk: tsdk.clone(),
                ts_node: "/t/bin/node".into(),
                ts_server: "/t/lib/node_modules/typescript-language-server/lib/cli.mjs".into(),
                plugin_location: vue.clone(),
            },
        };
        let cache = Path::new("/state/language-cache/p/vue/vue");
        let config = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(config.name, "Vue 3.3.14 with TypeScript 5.9.3 (project)");
        assert_eq!(config.program, PathBuf::from("/n/bin/node"));
        assert_eq!(
            config.args,
            vec![
                OsString::from(vue.join("bin/vue-language-server.js")),
                OsString::from("--stdio"),
                OsString::from("--tsdk=/work/app/node_modules/typescript/lib"),
            ]
        );
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, cache);
        assert_eq!(config.path, vec![PathBuf::from("/n/bin")]);
        assert!(config.env.is_empty());
        assert_eq!(
            config.initialization_options,
            json!({"typescript": {"tsdk": "/work/app/node_modules/typescript/lib"}})
        );
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(30));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(
            config.ready_detail,
            "Vue 3.3.14 with TypeScript 5.9.3 (project) ready"
        );

        let companion = config
            .companion
            .expect("Vue runs with a TypeScript companion");
        assert_eq!(companion.name, "typescript-language-server (Vue)");
        assert_eq!(companion.program, PathBuf::from("/t/bin/node"));
        assert_eq!(
            companion.args,
            vec![
                OsString::from("/t/lib/node_modules/typescript-language-server/lib/cli.mjs"),
                OsString::from("--stdio"),
            ]
        );
        assert_eq!(companion.project, PathBuf::from("/work/app"));
        // Inside the Vue server's cache, never in the project.
        assert_eq!(companion.cache, cache.join("typescript"));
        assert_eq!(companion.path, vec![PathBuf::from("/t/bin")]);
        assert!(companion.env.is_empty());
        assert_eq!(
            companion.initialization_options,
            json!({
                "tsserver": {"path": "/work/app/node_modules/typescript/lib/tsserver.js"},
                "disableAutomaticTypingAcquisition": true,
                "plugins": [{
                    "name": "@vue/typescript-plugin",
                    "location": "/n/lib/node_modules/@vue/language-server",
                    "languages": ["vue"],
                }],
            })
        );
        assert_eq!(companion.diagnostics, DiagnosticMode::Settled);
        assert_eq!(companion.start_timeout, Duration::from_secs(30));
        assert_eq!(companion.request_timeout, Duration::from_secs(30));
        assert!(companion.companion.is_none());
    }

    #[test]
    fn jdtls_config_keeps_its_workspace_in_the_cache() {
        let launcher = PathBuf::from("/opt/homebrew/Cellar/jdtls/1.47.0/bin/jdtls");
        let java_home = PathBuf::from("/Users/me/.sdkman/candidates/java/current");
        let tools = tandem_core::ServerTools {
            prepare: None,
            name: "jdtls".into(),
            title: "jdtls 1.47.0".into(),
            languages: vec!["java".into()],
            launch: tandem_core::Launch::Jdtls {
                launcher: launcher.clone(),
                java_home: java_home.clone(),
                trigger_files: vec!["/work/app/src/demo/A.java".into()],
            },
        };
        let cache = Path::new("/state/language-cache/p/java/jdtls");
        let config = server_config(&tools, Path::new("/work/app"), cache);
        assert_eq!(config.name, "jdtls 1.47.0");
        assert_eq!(config.program, launcher);
        assert_eq!(
            config.args,
            vec![
                OsString::from("-data"),
                OsString::from(cache.join("workspace"))
            ]
        );
        assert_eq!(config.project, PathBuf::from("/work/app"));
        assert_eq!(config.cache, cache);
        assert_eq!(config.path, vec![java_home.join("bin")]);
        assert_eq!(
            config.env,
            vec![("JAVA_HOME".into(), OsString::from(&java_home))]
        );
        let settings = &config.configuration;
        assert_eq!(
            config.initialization_options,
            json!({
                "settings": settings,
                "triggerFiles": ["file:///work/app/src/demo/A.java"],
            })
        );
        assert_eq!(
            settings.pointer("/java/import/generatesMetadataFilesAtProjectRoot"),
            Some(&json!(false))
        );
        for build in ["gradle", "maven"] {
            assert_eq!(
                settings.pointer(&format!("/java/import/{build}/enabled")),
                Some(&json!(true))
            );
            assert_eq!(
                settings.pointer(&format!("/java/import/{build}/offline/enabled")),
                Some(&json!(true))
            );
        }
        assert_eq!(
            settings.pointer("/java/maven/downloadSources"),
            Some(&json!(false))
        );
        assert_eq!(
            settings.pointer("/java/eclipse/downloadSources"),
            Some(&json!(false))
        );
        assert_eq!(settings.pointer("/java/format/enabled"), Some(&json!(true)));
        assert!(config.push_configuration);
        assert!(!config.resend_on_change);
        assert_eq!(config.diagnostics, DiagnosticMode::Settled);
        assert_eq!(config.start_timeout, Duration::from_secs(60));
        assert_eq!(config.request_timeout, Duration::from_secs(60));
        assert_eq!(
            config.ready_detail,
            "jdtls ready · Maven and Gradle import offline"
        );
    }
}
