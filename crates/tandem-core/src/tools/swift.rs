//! Swift: SourceKit-LSP from Xcode or the Command Line Tools, found by file path. Discovery never
//! runs `xcrun` or the `/usr/bin/sourcekit-lsp` shim, which can open the install dialog.
use super::read_tool_config;
use super::servers::{
    KindTools, Launch, MissingTools, ServerTools, ToolNote, ToolRoots, fingerprint, shell_quote,
};
use crate::ToolKind;
use std::path::{Path, PathBuf};

pub(crate) fn discover(project: &Path, roots: &ToolRoots) -> Result<KindTools, MissingTools> {
    let (server, developer_dir, title) = find_sourcekit(roots).ok_or_else(|| MissingTools {
        message: "SourceKit-LSP was not found. It comes with Xcode or the Command Line Tools."
            .into(),
        install: vec!["xcode-select --install".into()],
    })?;
    let extra_path = roots
        .homebrew
        .iter()
        .map(|prefix| prefix.join("bin"))
        .filter(|bin| bin.is_dir())
        .collect();
    let build_server = project.join("buildServer.json");
    let configured = build_server.is_file();
    let mut parts = vec![server.display().to_string(), title.clone()];
    let mut files = vec![server.as_path()];
    if configured {
        parts.push(build_server.display().to_string());
        files.push(&build_server);
    }
    let fingerprint = fingerprint("swift-v1", &parts, &files)?;
    let notes = if configured || project.join("Package.swift").is_file() {
        vec![]
    } else {
        xcode_project_notes(project, roots)
    };
    Ok(KindTools {
        kind: ToolKind::Swift,
        label: title.clone(),
        servers: vec![ServerTools {
            name: "sourcekit-lsp".into(),
            title,
            languages: vec!["swift".into()],
            launch: Launch::SourceKit {
                server,
                developer_dir,
                extra_path,
            },
        }],
        notes,
        fingerprint,
    })
}

/// The first SourceKit-LSP file of Xcode.app, the other `Xcode*.app` by name, then the Command
/// Line Tools: (server, developer dir, title).
fn find_sourcekit(roots: &ToolRoots) -> Option<(PathBuf, PathBuf, String)> {
    let mut apps: Vec<(String, PathBuf)> = std::fs::read_dir(&roots.applications)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| name.starts_with("Xcode") && name.ends_with(".app"))
        .collect();
    apps.sort_by(|(a, _), (b, _)| (a != "Xcode.app", a).cmp(&(b != "Xcode.app", b)));
    for (name, app) in apps {
        let developer_dir = app.join("Contents/Developer");
        let server =
            developer_dir.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp");
        if server.is_file() {
            let xcode = match xcode_version(&app) {
                Some(version) => format!("Xcode {version}"),
                None => name.trim_end_matches(".app").to_string(),
            };
            return Some((server, developer_dir, format!("SourceKit-LSP ({xcode})")));
        }
    }
    let developer_dir = roots.system.join("Library/Developer/CommandLineTools");
    let server = developer_dir.join("usr/bin/sourcekit-lsp");
    server.is_file().then(|| {
        (
            server,
            developer_dir,
            "SourceKit-LSP (Command Line Tools)".to_string(),
        )
    })
}

/// `CFBundleShortVersionString` from the app's `Contents/version.plist`.
fn xcode_version(app: &Path) -> Option<String> {
    let plist = read_tool_config(&app.join("Contents/version.plist")).ok()?;
    let (_, rest) = plist.split_once("<key>CFBundleShortVersionString</key>")?;
    let (_, rest) = rest.split_once("<string>")?;
    let (version, _) = rest.split_once("</string>")?;
    let version = version.trim();
    (!version.is_empty() && !version.contains('<')).then(|| version.to_string())
}

/// Notes for an Xcode project or workspace that has no buildServer.json yet.
fn xcode_project_notes(project: &Path, roots: &ToolRoots) -> Vec<ToolNote> {
    let mut entries = names_in(project);
    entries.sort();
    let container = ["xcworkspace", "xcodeproj"].into_iter().find_map(|kind| {
        let suffix = format!(".{kind}");
        let name = entries.iter().find(|name| name.ends_with(&suffix))?;
        Some((kind, name.clone()))
    });
    let Some((kind, container)) = container else {
        return vec![];
    };
    let flag = if kind == "xcworkspace" {
        "workspace"
    } else {
        "project"
    };
    let stem = container
        .rsplit_once('.')
        .map_or(container.as_str(), |(stem, _)| stem);
    let scheme = scheme_name(&project.join(&container)).unwrap_or_else(|| stem.to_string());
    let mut notes = vec![];
    if !roots
        .homebrew
        .iter()
        .any(|prefix| prefix.join("bin/xcode-build-server").is_file())
    {
        notes.push(ToolNote {
            text: "Xcode projects need xcode-build-server for full SourceKit-LSP support.".into(),
            command: Some("brew install xcode-build-server".into()),
        });
    }
    notes.push(ToolNote {
        text: "Build the project once in Xcode, then run this in the project folder to create buildServer.json. Until then SourceKit-LSP reads each file on its own.".into(),
        command: Some(format!(
            "xcode-build-server config -{flag} {} -scheme {}",
            shell_quote(&container),
            shell_quote(&scheme)
        )),
    });
    notes
}

/// The first shared scheme of the container, else the first scheme of any of its users.
fn scheme_name(container: &Path) -> Option<String> {
    let first = |folders: Vec<PathBuf>| {
        let mut schemes: Vec<PathBuf> = folders
            .iter()
            .flat_map(|folder| {
                names_in(folder)
                    .into_iter()
                    .filter(|name| name.ends_with(".xcscheme"))
                    .map(move |name| folder.join(name))
            })
            .collect();
        schemes.sort();
        let scheme = schemes.first()?;
        Some(scheme.file_stem()?.to_string_lossy().to_string())
    };
    first(vec![container.join("xcshareddata/xcschemes")]).or_else(|| {
        let mut users = names_in(&container.join("xcuserdata"));
        users.sort();
        first(
            users
                .into_iter()
                .map(|user| container.join("xcuserdata").join(user).join("xcschemes"))
                .collect(),
        )
    })
}

/// UTF-8 entry names of a folder; empty when it cannot be read.
fn names_in(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolKind;
    use crate::tools::servers::{Launch, ToolNote, discover_kind, shell_quote};
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
    fn plist(version: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>BuildVersion</key>\n\t<string>7</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n</dict>\n</plist>\n"
        )
    }
    /// An Xcode app with its version and SourceKit-LSP; returns the server path.
    fn xcode(roots: &ToolRoots, app: &str, version: &str) -> PathBuf {
        let contents = roots.applications.join(app).join("Contents");
        write(&contents.join("version.plist"), &plist(version));
        let server =
            contents.join("Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/sourcekit-lsp");
        write(&server, &format!("sourcekit-lsp from {app}"));
        server
    }
    fn command_line_tools(roots: &ToolRoots) -> PathBuf {
        let server = roots
            .system
            .join("Library/Developer/CommandLineTools/usr/bin/sourcekit-lsp");
        write(&server, "sourcekit-lsp from the Command Line Tools");
        server
    }
    fn project(temp: &Path) -> PathBuf {
        let project = temp.join("project");
        std::fs::create_dir_all(&project).unwrap();
        project
    }

    #[test]
    fn finds_sourcekit_in_xcode_first() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        xcode(&roots, "Xcode-beta.app", "27.0");
        let server = xcode(&roots, "Xcode.app", "26.1");
        std::fs::create_dir_all(roots.homebrew[0].join("bin")).unwrap();
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.kind, ToolKind::Swift);
        assert_eq!(tools.label, "SourceKit-LSP (Xcode 26.1)");
        assert_eq!(tools.servers.len(), 1);
        let found = &tools.servers[0];
        assert_eq!(found.name, "sourcekit-lsp");
        assert_eq!(found.title, "SourceKit-LSP (Xcode 26.1)");
        assert_eq!(found.languages, vec!["swift".to_string()]);
        assert_eq!(
            found.launch,
            Launch::SourceKit {
                server,
                developer_dir: roots.applications.join("Xcode.app/Contents/Developer"),
                extra_path: vec![roots.homebrew[0].join("bin")],
            }
        );
        assert!(tools.notes.is_empty());
        assert!(!tools.fingerprint.is_empty());
    }

    #[test]
    fn other_xcode_apps_follow_in_name_order() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write(
            &roots.applications.join("Xcode.app/Contents/version.plist"),
            &plist("26.1"),
        );
        xcode(&roots, "Xcode-beta.app", "27.0");
        xcode(&roots, "Xcode-26.0.app", "26.0");
        let server = xcode(&roots, "Xcode-16.4.app", "16.4");
        std::fs::create_dir_all(roots.applications.join("Xcodes.txt")).unwrap();
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.label, "SourceKit-LSP (Xcode 16.4)");
        let Launch::SourceKit { server: found, .. } = &tools.servers[0].launch;
        assert_eq!(found, &server);
    }

    #[test]
    fn falls_back_to_the_command_line_tools() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        write(
            &roots.applications.join("Xcode.app/Contents/version.plist"),
            &plist("26.1"),
        );
        let server = command_line_tools(&roots);
        let tools = discover(&project(temp.path()), &roots).unwrap();
        assert_eq!(tools.label, "SourceKit-LSP (Command Line Tools)");
        assert_eq!(tools.servers[0].title, "SourceKit-LSP (Command Line Tools)");
        assert_eq!(
            tools.servers[0].launch,
            Launch::SourceKit {
                server,
                developer_dir: roots.system.join("Library/Developer/CommandLineTools"),
                extra_path: vec![],
            }
        );
    }

    #[test]
    fn reports_the_install_command_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let missing = discover_kind(ToolKind::Swift, &project(temp.path()), &roots)
            .expect("Swift is served through the registry")
            .unwrap_err();
        assert_eq!(
            missing.message,
            "SourceKit-LSP was not found. It comes with Xcode or the Command Line Tools."
        );
        assert_eq!(missing.install, vec!["xcode-select --install".to_string()]);
        assert!(discover_kind(ToolKind::Rust, temp.path(), &roots).is_none());
    }

    #[test]
    fn xcode_projects_get_build_server_notes() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let project = project(temp.path());
        write(
            &project.join("My App.xcodeproj/xcshareddata/xcschemes/My App.xcscheme"),
            "<Scheme/>",
        );
        write(
            &project.join("My App.xcodeproj/xcuserdata/me.xcuserdatad/xcschemes/Alpha.xcscheme"),
            "<Scheme/>",
        );
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(
            tools.notes,
            vec![
                ToolNote {
                    text: "Xcode projects need xcode-build-server for full SourceKit-LSP support."
                        .into(),
                    command: Some("brew install xcode-build-server".into()),
                },
                ToolNote {
                    text: "Build the project once in Xcode, then run this in the project folder to create buildServer.json. Until then SourceKit-LSP reads each file on its own.".into(),
                    command: Some(
                        "xcode-build-server config -project 'My App.xcodeproj' -scheme 'My App'"
                            .into()
                    ),
                },
            ]
        );

        write(
            &roots.homebrew[1].join("bin/xcode-build-server"),
            "#!/bin/sh\n",
        );
        let tools = discover(&project, &roots).unwrap();
        assert_eq!(tools.notes.len(), 1);
        assert_eq!(
            tools.notes[0].command.as_deref(),
            Some("xcode-build-server config -project 'My App.xcodeproj' -scheme 'My App'")
        );
    }

    #[test]
    fn workspaces_and_user_schemes_name_the_build_server_command() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let project = project(temp.path());
        write(
            &project.join("Shop.xcodeproj/xcshareddata/xcschemes/Shop.xcscheme"),
            "<Scheme/>",
        );
        write(
            &project.join("Shop.xcworkspace/xcuserdata/me.xcuserdatad/xcschemes/Zeta.xcscheme"),
            "<Scheme/>",
        );
        write(
            &project.join("Shop.xcworkspace/xcuserdata/me.xcuserdatad/xcschemes/Beta.xcscheme"),
            "<Scheme/>",
        );
        let command = |tools: &KindTools| tools.notes.last().unwrap().command.clone().unwrap();
        assert_eq!(
            command(&discover(&project, &roots).unwrap()),
            "xcode-build-server config -workspace Shop.xcworkspace -scheme Beta"
        );

        std::fs::remove_dir_all(project.join("Shop.xcworkspace/xcuserdata")).unwrap();
        assert_eq!(
            command(&discover(&project, &roots).unwrap()),
            "xcode-build-server config -workspace Shop.xcworkspace -scheme Shop"
        );
    }

    #[test]
    fn no_notes_for_packages_or_configured_projects() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        command_line_tools(&roots);
        let project = project(temp.path());
        write(
            &project.join("App.xcodeproj/xcshareddata/xcschemes/App.xcscheme"),
            "<Scheme/>",
        );
        assert_eq!(discover(&project, &roots).unwrap().notes.len(), 2);

        write(
            &project.join("Package.swift"),
            "// swift-tools-version:5.9\n",
        );
        assert!(discover(&project, &roots).unwrap().notes.is_empty());

        std::fs::remove_file(project.join("Package.swift")).unwrap();
        write(&project.join("buildServer.json"), "{}");
        assert!(discover(&project, &roots).unwrap().notes.is_empty());
    }

    #[test]
    fn build_server_json_changes_the_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let server = command_line_tools(&roots);
        let project = project(temp.path());
        let fingerprint = || discover(&project, &roots).unwrap().fingerprint;
        let plain = fingerprint();
        assert_eq!(fingerprint(), plain);

        write(
            &project.join("buildServer.json"),
            "{\"argv\":[\"/opt/homebrew/bin/xcode-build-server\"]}",
        );
        let configured = fingerprint();
        assert_ne!(configured, plain);

        write(
            &project.join("buildServer.json"),
            "{\"argv\":[\"/tmp/other-build-server\"]}",
        );
        assert_ne!(fingerprint(), configured);

        std::fs::remove_file(project.join("buildServer.json")).unwrap();
        assert_eq!(fingerprint(), plain);
        write(&server, "a different sourcekit-lsp");
        assert_ne!(fingerprint(), plain);
    }

    #[test]
    fn shell_quote_keeps_plain_words_and_quotes_the_rest() {
        assert_eq!(shell_quote("abc"), "abc");
        assert_eq!(shell_quote("My.App_2/x-y"), "My.App_2/x-y");
        assert_eq!(shell_quote("My App"), "'My App'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
