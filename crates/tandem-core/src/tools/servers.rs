//! Language servers beyond Rust and Python: where Tandem looks, what it found and what is
//! missing. Discovery inspects files only; it never runs a tool or an installer.
use super::{file_hash, java, swift, typescript, web};
use crate::{ToolKind, content_hash};
use anyhow::Context;
use std::path::{Path, PathBuf};

/// Where discovery looks. Tests point every root at a temp folder.
#[derive(Clone, Debug)]
pub struct ToolRoots {
    pub home: PathBuf,
    pub homebrew: Vec<PathBuf>,
    pub applications: PathBuf,
    pub system: PathBuf,
}

impl ToolRoots {
    /// HOME from the environment, homebrew ["/opt/homebrew", "/usr/local"], applications
    /// "/Applications", system "/".
    pub fn system() -> anyhow::Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
        Ok(Self {
            home,
            homebrew: vec!["/opt/homebrew".into(), "/usr/local".into()],
            applications: "/Applications".into(),
            system: "/".into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// `developer_dir` is the Xcode or Command Line Tools developer folder the server belongs to;
    /// `extra_path` holds the existing Homebrew bin folders, so a project's buildServer.json can
    /// find xcode-build-server.
    SourceKit {
        server: PathBuf,
        developer_dir: PathBuf,
        extra_path: Vec<PathBuf>,
    },
    /// `launcher` is jdtls's own start script, canonical; `java_home` is the Java 21 or newer
    /// runtime it runs on, given to it as JAVA_HOME.
    Jdtls {
        launcher: PathBuf,
        java_home: PathBuf,
    },
    /// TypeScript 7's native compiler, whose `--lsp --stdio` mode is a full language server.
    Tsgo { server: PathBuf },
    /// typescript-language-server under `node`, driving the tsserver of TypeScript 6 or older.
    /// `server` is the package's `lib/cli.mjs`; `tsserver` is TypeScript's `lib/tsserver.js`.
    TypeScriptLanguageServer {
        node: PathBuf,
        server: PathBuf,
        tsserver: PathBuf,
    },
    /// A Node.js language server script, run as `node <script> --stdio`; `server` says which one,
    /// for its settings.
    Node {
        node: PathBuf,
        script: PathBuf,
        server: NodeServer,
    },
}

/// The language servers Tandem runs as Node.js scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeServer {
    /// vscode-langservers-extracted's HTML server.
    Html,
    /// vscode-langservers-extracted's CSS server, which also serves SCSS and Less.
    Css,
    /// vscode-langservers-extracted's JSON server, which also serves JSON with Comments.
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerTools {
    /// Cache folder and health prefix: "sourcekit-lsp", "jdtls", "tsgo", "html", ...
    pub name: String,
    /// Display name, e.g. "SourceKit-LSP (Xcode 26.1)".
    pub title: String,
    /// `Document::language()` ids this server answers for.
    pub languages: Vec<String>,
    pub launch: Launch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNote {
    pub text: String,
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindTools {
    pub kind: ToolKind,
    /// One line for Settings.
    pub label: String,
    pub servers: Vec<ServerTools>,
    pub notes: Vec<ToolNote>,
    /// Trust is recorded against this; any change of an executable or launch input changes it.
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingTools {
    pub message: String,
    /// Exact commands that install what is missing. Tandem shows them and never runs them.
    pub install: Vec<String>,
}

impl From<anyhow::Error> for MissingTools {
    fn from(error: anyhow::Error) -> Self {
        Self {
            message: format!("{error:#}"),
            install: vec![],
        }
    }
}

pub type Discover = fn(&Path, &ToolRoots) -> Result<KindTools, MissingTools>;

/// Kinds served through this registry, in Settings order. Rust and Python keep their own discovery.
pub const SERVER_KINDS: &[(ToolKind, Discover)] = &[
    (ToolKind::Java, java::discover),
    (ToolKind::Swift, swift::discover),
    (ToolKind::TypeScript, typescript::discover),
    (ToolKind::Web, web::discover),
];

/// Runs the discovery of `kind`; `None` when the kind is not served through this registry.
pub fn discover_kind(
    kind: ToolKind,
    project: &Path,
    roots: &ToolRoots,
) -> Option<Result<KindTools, MissingTools>> {
    SERVER_KINDS
        .iter()
        .find(|(served, _)| *served == kind)
        .map(|(_, discover)| discover(project, roots))
}

/// content_hash of "{tag}:{parts joined by '\n'}" followed by file_hash of each file, in order.
pub(crate) fn fingerprint(tag: &str, parts: &[String], files: &[&Path]) -> anyhow::Result<String> {
    let mut identity = format!("{tag}:{}", parts.join("\n"));
    for file in files {
        identity.push('\n');
        identity.push_str(&file_hash(file)?);
    }
    Ok(content_hash(identity.as_bytes()))
}

/// The word as is when it is [A-Za-z0-9._/-]+, otherwise wrapped in single quotes with ' escaped
/// as '\''.
pub(crate) fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}
