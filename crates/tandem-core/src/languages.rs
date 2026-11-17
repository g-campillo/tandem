//! The languages Tandem knows: file detection, display names, help-level keys and the
//! language tools that serve them. Ids are VS Code / LSP language identifiers.
use serde::{Deserialize, Serialize};

/// A group of language tools a project trusts as one unit: its own discovery, notice and servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Rust,
    Python,
    Java,
    Swift,
    TypeScript,
    Vue,
    Svelte,
    Web,
}

impl ToolKind {
    pub const ALL: [ToolKind; 8] = [
        Self::Rust,
        Self::Python,
        Self::Java,
        Self::Swift,
        Self::TypeScript,
        Self::Vue,
        Self::Svelte,
        Self::Web,
    ];

    /// Stable key for saved trust and cache folders; equals the serde name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Java => "java",
            Self::Swift => "swift",
            Self::TypeScript => "typescript",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Web => "web",
        }
    }

    /// Heading in Settings → Language tools.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Java => "Java",
            Self::Swift => "Swift",
            Self::TypeScript => "JavaScript and TypeScript",
            Self::Vue => "Vue",
            Self::Svelte => "Svelte",
            Self::Web => "HTML, CSS and JSON",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Lower-case extensions without the dot.
    pub extensions: &'static [&'static str],
    /// Help-level key (`chat::HELP_LANGUAGES`), `None` for data formats.
    pub help: Option<&'static str>,
    pub tools: Option<ToolKind>,
}

pub const PLAIN_TEXT: &str = "plain_text";

const fn row(
    id: &'static str,
    label: &'static str,
    extensions: &'static [&'static str],
    help: Option<&'static str>,
    tools: Option<ToolKind>,
) -> LanguageInfo {
    LanguageInfo {
        id,
        label,
        extensions,
        help,
        tools,
    }
}

pub const LANGUAGES: &[LanguageInfo] = {
    use ToolKind::{Java, Python, Rust, Svelte, Swift, TypeScript, Vue, Web};
    &[
        row("rust", "Rust", &["rs"], Some("rust"), Some(Rust)),
        row(
            "python",
            "Python",
            &["py", "pyi"],
            Some("python"),
            Some(Python),
        ),
        row("java", "Java", &["java"], Some("java"), Some(Java)),
        row("swift", "Swift", &["swift"], Some("swift"), Some(Swift)),
        row(
            "javascript",
            "JavaScript",
            &["js", "mjs", "cjs"],
            Some("javascript"),
            Some(TypeScript),
        ),
        row(
            "javascriptreact",
            "JavaScript JSX",
            &["jsx"],
            Some("javascript"),
            Some(TypeScript),
        ),
        row(
            "typescript",
            "TypeScript",
            &["ts", "mts", "cts"],
            Some("typescript"),
            Some(TypeScript),
        ),
        row(
            "typescriptreact",
            "TypeScript JSX",
            &["tsx"],
            Some("typescript"),
            Some(TypeScript),
        ),
        row("vue", "Vue", &["vue"], Some("vue"), Some(Vue)),
        row(
            "svelte",
            "Svelte",
            &["svelte"],
            Some("svelte"),
            Some(Svelte),
        ),
        row("html", "HTML", &["html", "htm"], Some("html"), Some(Web)),
        row("css", "CSS", &["css"], Some("css"), Some(Web)),
        row("scss", "SCSS", &["scss"], Some("css"), Some(Web)),
        row("less", "Less", &["less"], Some("css"), Some(Web)),
        row("json", "JSON", &["json"], None, Some(Web)),
        row("jsonc", "JSON with Comments", &["jsonc"], None, Some(Web)),
        row("toml", "TOML", &["toml"], None, None),
        row("markdown", "Markdown", &["md"], None, None),
    ]
};

/// The language id of the file at `path`, relative or absolute: by its name for the JSON
/// files that allow comments, otherwise by its extension; `PLAIN_TEXT` when unknown.
pub fn language_id_for_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if allows_comments(path, &name) {
        return "jsonc";
    }
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .and_then(|(_, extension)| {
            LANGUAGES
                .iter()
                .find(|row| row.extensions.contains(&extension))
        })
        .map_or(PLAIN_TEXT, |row| row.id)
}

/// JSON files that tools read as JSON with Comments; `name` is the lower-cased file name.
fn allows_comments(path: &str, name: &str) -> bool {
    let named = matches!(
        name,
        "tsconfig.json" | "jsconfig.json" | "devcontainer.json"
    );
    let config =
        (name.starts_with("tsconfig.") || name.starts_with("jsconfig.")) && name.ends_with(".json");
    let editor_settings = name.ends_with(".json") && path.split('/').any(|part| part == ".vscode");
    named || config || editor_settings
}

pub fn language_info(id: &str) -> Option<&'static LanguageInfo> {
    LANGUAGES.iter().find(|row| row.id == id)
}

/// "Plain Text" for `plain_text` and unknown ids.
pub fn language_label(id: &str) -> &'static str {
    language_info(id).map_or("Plain Text", |row| row.label)
}

pub fn tool_kind(id: &str) -> Option<ToolKind> {
    language_info(id).and_then(|row| row.tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn language_ids_follow_names_and_extensions() {
        let cases = [
            ("src/main.rs", "rust"),
            ("a.PY", "python"),
            ("A.java", "java"),
            ("App.swift", "swift"),
            ("x.js", "javascript"),
            ("x.mjs", "javascript"),
            ("x.cjs", "javascript"),
            ("App.jsx", "javascriptreact"),
            ("x.ts", "typescript"),
            ("x.mts", "typescript"),
            ("x.cts", "typescript"),
            ("types.d.ts", "typescript"),
            ("App.tsx", "typescriptreact"),
            ("App.vue", "vue"),
            ("App.svelte", "svelte"),
            ("index.html", "html"),
            ("old.htm", "html"),
            ("a.css", "css"),
            ("a.scss", "scss"),
            ("a.less", "less"),
            ("package.json", "json"),
            ("tsconfig.json", "jsonc"),
            ("tsconfig.app.json", "jsonc"),
            ("jsconfig.json", "jsonc"),
            (".vscode/settings.json", "jsonc"),
            ("/abs/web/.vscode/launch.json", "jsonc"),
            ("a.jsonc", "jsonc"),
            ("Cargo.toml", "toml"),
            ("README.md", "markdown"),
            ("Makefile", "plain_text"),
            (".bashrc", "plain_text"),
            ("notes", "plain_text"),
            ("", "plain_text"),
        ];
        for (path, expected) in cases {
            assert_eq!(language_id_for_path(path), expected, "{path:?}");
        }
    }

    #[test]
    fn every_row_is_reachable_and_unique() {
        let mut ids = BTreeSet::new();
        let mut extensions = BTreeSet::new();
        for row in LANGUAGES {
            assert!(ids.insert(row.id), "duplicate id {}", row.id);
            for extension in row.extensions {
                assert!(
                    extensions.insert(*extension),
                    "extension {extension} is in two rows"
                );
            }
            assert_eq!(language_label(row.id), row.label, "{}", row.id);
        }
        assert_eq!(language_label("plain_text"), "Plain Text");
        assert_eq!(language_label("nope"), "Plain Text");
    }

    #[test]
    fn tool_kinds_serialize_as_their_keys() {
        for kind in ToolKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.key()));
        }
    }
}
