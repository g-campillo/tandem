//! Language servers beyond Rust and Python: where Tandem looks, what it found and what is
//! missing. Discovery inspects files only; it never runs a tool or an installer.
pub use super::swift::XcodeSetup;
use super::{file_hash, java, svelte, swift, typescript, vue, web};
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
    /// find xcode-build-server. `xcode` is the buildServer.json Tandem writes before the server
    /// starts, for an Xcode project that has none.
    SourceKit {
        server: PathBuf,
        developer_dir: PathBuf,
        extra_path: Vec<PathBuf>,
        xcode: Option<XcodeSetup>,
    },
    /// `launcher` is jdtls's own start script, canonical; `java_home` is the Java 21 or newer
    /// runtime it runs on, given to it as JAVA_HOME.
    /// `trigger_files` are Java files of a project without Maven or Gradle, from which jdtls
    /// infers its source folders (`src/` for `src/demo/A.java` in `package demo`).
    Jdtls {
        launcher: PathBuf,
        java_home: PathBuf,
        trigger_files: Vec<PathBuf>,
    },
    /// TypeScript 7's native compiler, whose `--lsp --stdio` mode is a full language server.
    Tsgo { server: PathBuf },
    /// typescript-language-server under `node`, driving the tsserver of TypeScript 6 or older.
    /// `server` is the package's `lib/cli.mjs`; `tsserver` is TypeScript's `lib/tsserver.js`.
    /// `vue_plugin` is the folder `X` with `X/node_modules/@vue/typescript-plugin`, loaded in a Vue
    /// project so TypeScript files can import `.vue` files.
    TypeScriptLanguageServer {
        node: PathBuf,
        server: PathBuf,
        tsserver: PathBuf,
        vue_plugin: Option<PathBuf>,
    },
    /// A Node.js language server script, run as `node <script> --stdio`; `server` says which one,
    /// for its settings.
    Node {
        node: PathBuf,
        script: PathBuf,
        server: NodeServer,
    },
    /// @vue/language-server under `node`, with typescript-language-server under `ts_node` as its
    /// TypeScript companion. `server` is `bin/vue-language-server.js`; `tsdk` is the project's
    /// TypeScript `lib` folder, holding `typescript.js` and `tsserver.js`; `ts_server` is
    /// typescript-language-server's `lib/cli.mjs`; `plugin_location` is the folder `X` such that
    /// `X/node_modules/@vue/typescript-plugin` exists.
    Vue {
        node: PathBuf,
        server: PathBuf,
        tsdk: PathBuf,
        ts_node: PathBuf,
        ts_server: PathBuf,
        plugin_location: PathBuf,
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
    /// svelte-language-server, with the TypeScript and prettier-plugin-svelte it brings along.
    Svelte,
}

/// A command run in the project before its language server starts, so the generated types the
/// server reads are current: SvelteKit's `svelte-kit sync`, Nuxt's `nuxt prepare`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepare {
    /// Shown in health text, e.g. "svelte-kit sync".
    pub title: String,
    pub program: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    /// Project folders, relative with a trailing `/`, where an added or removed file calls for
    /// another run.
    pub watch: Vec<String>,
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
    pub prepare: Option<Prepare>,
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
    (ToolKind::Vue, vue::discover),
    (ToolKind::Svelte, svelte::discover),
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
    let hashes = files
        .iter()
        .map(|file| file_hash(file))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(fingerprint_hashes(tag, parts, &hashes))
}

/// `fingerprint` with the content hashes given: a file's hash equals content_hash of its bytes, so
/// a file Tandem is about to write fingerprints the same before and after.
pub(crate) fn fingerprint_hashes(tag: &str, parts: &[String], hashes: &[String]) -> String {
    let mut identity = format!("{tag}:{}", parts.join("\n"));
    for hash in hashes {
        identity.push('\n');
        identity.push_str(hash);
    }
    content_hash(identity.as_bytes())
}
