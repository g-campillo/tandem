//! How Tandem launches each language server it runs: program, arguments, environment and settings.
use crate::{DiagnosticMode, ServerConfig};
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tandem_core::Launch;

/// rust-analyzer from the project's toolchain, offline, with build scripts and proc macros off.
pub fn rust_server(tools: &tandem_core::RustTools, project: &Path, cache: &Path) -> ServerConfig {
    let bin = tools.toolchain.join("bin");
    let mut config = ServerConfig::new("rust-analyzer", &tools.rust_analyzer, project, cache);
    config.ready_detail = "rust-analyzer ready · build scripts and proc macros off".into();
    config.env = vec![
        ("RUSTC".into(), bin.join("rustc").into()),
        ("RUSTDOC".into(), bin.join("rustdoc").into()),
        ("CARGO_NET_OFFLINE".into(), "true".into()),
    ];
    config.path = vec![bin];
    config.initialization_options = json!({"checkOnSave":false,"cargo":{"extraArgs":["--locked","--offline"],"buildScripts":{"enable":false},"targetDir":cache},
        "procMacro":{"enable":false},"files":{"watcher":"server"},"completion":{"autoimport":{"enable":false}},"rustfmt":{"extraArgs":[]}});
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
    config.configuration = json!({"python":{"pythonPath":tools.interpreter,"analysis":{"diagnosticMode":"openFilesOnly","typeCheckingMode":"basic","autoImportCompletions":false}},"pyright":{"disableOrganizeImports":true}});
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
    match &server.launch {
        Launch::SourceKit {
            server: program,
            developer_dir,
            extra_path,
        } => {
            let mut config = ServerConfig::new(&server.title, program, project, cache);
            config.args = vec!["--scratch-path".into(), cache.join("build").into()];
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
        } => {
            let mut config = ServerConfig::new(&server.title, launcher, project, cache);
            config.args = vec!["-data".into(), cache.join("workspace").into()];
            config.path = vec![java_home.join("bin")];
            config.env = vec![("JAVA_HOME".into(), java_home.into())];
            let settings = json!({"java":{"import":{"generatesMetadataFilesAtProjectRoot":false,"gradle":{"enabled":true,"offline":{"enabled":true}},"maven":{"enabled":true,"offline":{"enabled":true}}},"maven":{"downloadSources":false},"eclipse":{"downloadSources":false},"format":{"enabled":true},"autobuild":{"enabled":true}}});
            config.initialization_options = json!({ "settings": settings });
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiagnosticMode;
    use std::ffi::OsString;
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
            Some(&json!(false))
        );
        assert_eq!(
            config.initialization_options.pointer("/cargo/extraArgs"),
            Some(&json!(["--locked", "--offline"]))
        );
        assert_eq!(config.diagnostics, DiagnosticMode::Versioned);
        assert_eq!(
            config.ready_detail,
            "rust-analyzer ready · build scripts and proc macros off"
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
            name: "sourcekit-lsp".into(),
            title: "SourceKit-LSP (Xcode 26.1)".into(),
            languages: vec!["swift".into()],
            launch: tandem_core::Launch::SourceKit {
                server: program.clone(),
                developer_dir: developer.clone(),
                extra_path: vec!["/opt/homebrew/bin".into()],
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
    }

    #[test]
    fn jdtls_config_keeps_its_workspace_in_the_cache() {
        let launcher = PathBuf::from("/opt/homebrew/Cellar/jdtls/1.47.0/bin/jdtls");
        let java_home = PathBuf::from("/Users/me/.sdkman/candidates/java/current");
        let tools = tandem_core::ServerTools {
            name: "jdtls".into(),
            title: "jdtls 1.47.0".into(),
            languages: vec!["java".into()],
            launch: tandem_core::Launch::Jdtls {
                launcher: launcher.clone(),
                java_home: java_home.clone(),
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
            json!({ "settings": settings })
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
