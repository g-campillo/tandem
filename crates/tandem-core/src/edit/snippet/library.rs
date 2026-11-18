//! The snippets offered while typing: a built-in library per language, embedded in the
//! binary, and the user's own snippet files, both in the VS Code snippet file format.
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, bail};
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;

/// One snippet: the prefixes that offer it in completion, its body in the snippet syntax,
/// and whether it comes from the user's files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetDef {
    pub name: String,
    pub prefixes: Vec<String>,
    pub body: String,
    pub description: String,
    pub user: bool,
}

/// Every built-in snippet file by name, embedded in the binary.
const BUILTIN_FILES: &[(&str, &str)] = &[
    ("rust", include_str!("../../../snippets/rust.json")),
    ("python", include_str!("../../../snippets/python.json")),
    ("java", include_str!("../../../snippets/java.json")),
    ("swift", include_str!("../../../snippets/swift.json")),
    (
        "ecmascript",
        include_str!("../../../snippets/ecmascript.json"),
    ),
    (
        "javascript",
        include_str!("../../../snippets/javascript.json"),
    ),
    (
        "typescript",
        include_str!("../../../snippets/typescript.json"),
    ),
    ("react", include_str!("../../../snippets/react.json")),
    ("vue", include_str!("../../../snippets/vue.json")),
    ("svelte", include_str!("../../../snippets/svelte.json")),
    ("html", include_str!("../../../snippets/html.json")),
    ("css", include_str!("../../../snippets/css.json")),
    ("json", include_str!("../../../snippets/json.json")),
];

/// The built-in files a language's snippets come from, in order.
fn builtin_files(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["rust"],
        "python" => &["python"],
        "java" => &["java"],
        "swift" => &["swift"],
        "javascript" => &["ecmascript", "javascript"],
        "javascriptreact" => &["ecmascript", "javascript", "react"],
        "typescript" => &["ecmascript", "typescript"],
        "typescriptreact" => &["ecmascript", "typescript", "react"],
        "vue" => &["vue", "ecmascript"],
        "svelte" => &["svelte", "ecmascript"],
        "html" => &["html"],
        "css" | "scss" | "less" => &["css"],
        "json" => &["json"],
        _ => &[],
    }
}

/// The built-in snippets for a language id: rust [rust]; python [python]; java [java];
/// swift [swift]; javascript [ecmascript, javascript]; javascriptreact [ecmascript,
/// javascript, react]; typescript [ecmascript, typescript]; typescriptreact [ecmascript,
/// typescript, react]; vue [vue, ecmascript]; svelte [svelte, ecmascript]; html [html];
/// css, scss and less [css]; json [json]; any other language none.
pub fn builtin(language: &str) -> Vec<SnippetDef> {
    builtin_files(language)
        .iter()
        .flat_map(|name| {
            let (_, text) = BUILTIN_FILES
                .iter()
                .find(|(file, _)| file == name)
                .expect("every built-in snippet file is embedded");
            parse_file(text, false).expect("the built-in snippet files are valid")
        })
        .collect()
}

/// Removes `//` and `/* */` comments outside strings (escapes respected). A block comment
/// becomes one space plus the line breaks it held, so error positions keep their lines.
pub fn strip_json_comments(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            stripped.push(c);
            match c {
                '\\' => stripped.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('/')) => while chars.next_if(|&next| next != '\n').is_some() {},
            ('/', Some('*')) => {
                chars.next();
                stripped.push(' ');
                let mut previous = None;
                for next in chars.by_ref() {
                    if previous == Some('*') && next == '/' {
                        break;
                    }
                    if next == '\n' {
                        stripped.push('\n');
                    }
                    previous = Some(next);
                }
            }
            _ => {
                in_string = c == '"';
                stripped.push(c);
            }
        }
    }
    stripped
}

/// The snippets of a file in the VS Code format, in the order they are written: an object
/// of named entries with a `prefix` (a string or a list; none offers the snippet only
/// through Insert Snippet), a `body` (a string or a list of lines joined with "\n") and an
/// optional `description`. Comments are allowed and a file with nothing but comments is
/// empty. Any entry that is not of this shape fails the whole file, naming the entry.
pub fn parse_file(text: &str, user: bool) -> anyhow::Result<Vec<SnippetDef>> {
    let text = strip_json_comments(text);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let Entries(entries) =
        serde_json::from_str(&text).context("The file is not a JSON object of snippets")?;
    entries
        .into_iter()
        .map(|(name, entry)| snippet(name, &entry, user))
        .collect()
}

/// One entry of a snippet file.
fn snippet(name: String, entry: &Value, user: bool) -> anyhow::Result<SnippetDef> {
    let Some(entry) = entry.as_object() else {
        bail!("The snippet \"{name}\" is not an object");
    };
    let body = match entry.get("body") {
        None | Some(Value::Null) => bail!("The snippet \"{name}\" has no body"),
        Some(body) => strings(body)
            .with_context(|| {
                format!("The body of snippet \"{name}\" is not a string or a list of strings")
            })?
            .join("\n"),
    };
    let prefixes = match entry.get("prefix") {
        None | Some(Value::Null) => Vec::new(),
        Some(prefix) => strings(prefix).with_context(|| {
            format!("The prefix of snippet \"{name}\" is not a string or a list of strings")
        })?,
    };
    let description = match entry.get("description") {
        None | Some(Value::Null) => String::new(),
        Some(description) => strings(description)
            .with_context(|| {
                format!(
                    "The description of snippet \"{name}\" is not a string or a list of strings"
                )
            })?
            .join("\n"),
    };
    Ok(SnippetDef {
        name,
        prefixes: prefixes
            .into_iter()
            .filter(|prefix| !prefix.is_empty())
            .collect(),
        body,
        description,
        user,
    })
}

/// A string as a list of one, or a list of strings.
fn strings(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::String(text) => Some(vec![text.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// The entries of a snippet file in the order they are written, whichever order
/// `serde_json::Map` keeps in this build.
struct Entries(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(EntriesVisitor)
    }
}

struct EntriesVisitor;

impl<'de> Visitor<'de> for EntriesVisitor {
    type Value = Entries;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object of snippets")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(Entries(entries))
    }
}

/// Built-ins with the user's entries added; a user entry replaces every built-in that
/// shares one of its prefixes. User entries come first.
pub fn merged(builtin: Vec<SnippetDef>, user: Vec<SnippetDef>) -> Vec<SnippetDef> {
    let kept: Vec<SnippetDef> = {
        let taken: HashSet<&str> = user
            .iter()
            .flat_map(|snippet| snippet.prefixes.iter().map(String::as_str))
            .collect();
        builtin
            .into_iter()
            .filter(|snippet| {
                !snippet
                    .prefixes
                    .iter()
                    .any(|prefix| taken.contains(prefix.as_str()))
            })
            .collect()
    };
    let mut snippets = user;
    snippets.extend(kept);
    snippets
}

/// A file's modification time and length.
type Stamp = (SystemTime, u64);

/// The snippets of one user file, or why they could not be read.
type FileSnippets = Result<Vec<SnippetDef>, String>;

/// The user's snippet folder (`<state dir>/snippets`): `<language id>.json` and
/// `all.json`, re-read when a file's modification time or length changes.
pub struct UserSnippets {
    dir: PathBuf,
    cache: HashMap<PathBuf, (Option<Stamp>, FileSnippets)>,
}

impl UserSnippets {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cache: HashMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The user's snippets for `language` (those of `<language>.json`, then those of
    /// `all.json`) and the first file error, as "snippets/<file>: <error>". Missing files
    /// are no error; a broken file adds no snippets.
    pub fn for_language(&mut self, language: &str) -> (Vec<SnippetDef>, Option<String>) {
        let mut snippets = Vec::new();
        let mut first_error = None;
        for file in [format!("{language}.json"), "all.json".to_string()] {
            match self.file(&file) {
                Ok(found) => snippets.extend(found),
                Err(error) => {
                    first_error.get_or_insert_with(|| format!("snippets/{file}: {error}"));
                }
            }
        }
        (snippets, first_error)
    }

    /// The snippets of one file in the folder, read again only when its modification time
    /// or length changed since the last read.
    fn file(&mut self, file: &str) -> FileSnippets {
        let path = self.dir.join(file);
        let stamp = match fs::metadata(&path) {
            Ok(metadata) => Some((
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                metadata.len(),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        if let Some((cached, snippets)) = self.cache.get(&path)
            && *cached == stamp
        {
            return snippets.clone();
        }
        let snippets = match stamp {
            None => Ok(Vec::new()),
            Some(_) => fs::read_to_string(&path)
                .context("The file could not be read")
                .and_then(|text| parse_file(&text, true))
                .map_err(|error| format!("{error:#}")),
        };
        self.cache.insert(path, (stamp, snippets.clone()));
        snippets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::snippet::{SnippetIndent, expand};

    /// Every language id the editor gives a document.
    const LANGUAGES: &[&str] = &[
        "rust",
        "python",
        "java",
        "swift",
        "javascript",
        "javascriptreact",
        "typescript",
        "typescriptreact",
        "vue",
        "svelte",
        "html",
        "css",
        "scss",
        "less",
        "json",
    ];

    fn prefixes(language: &str) -> Vec<String> {
        builtin(language)
            .into_iter()
            .flat_map(|snippet| snippet.prefixes)
            .collect()
    }

    fn def(name: &str, prefixes: &[&str], body: &str, user: bool) -> SnippetDef {
        SnippetDef {
            name: name.to_string(),
            prefixes: prefixes.iter().map(|prefix| prefix.to_string()).collect(),
            body: body.to_string(),
            description: String::new(),
            user,
        }
    }

    #[test]
    fn every_language_has_builtin_snippets() {
        for language in LANGUAGES {
            assert!(!builtin(language).is_empty(), "{language} has no snippets");
        }
    }

    #[test]
    fn every_builtin_file_parses_and_every_body_expands() {
        assert_eq!(BUILTIN_FILES.len(), 13);
        let indent = SnippetIndent {
            line: String::new(),
            unit: "\t".to_string(),
        };
        for (file, text) in BUILTIN_FILES {
            let snippets = parse_file(text, false)
                .unwrap_or_else(|error| panic!("{file}.json does not parse: {error:#}"));
            assert!(!snippets.is_empty(), "{file}.json has no snippets");
            for snippet in snippets {
                let name = format!("{file}.json \"{}\"", snippet.name);
                assert!(!snippet.prefixes.is_empty(), "{name} has no prefix");
                assert!(!snippet.description.is_empty(), "{name} has no description");
                assert!(!snippet.user, "{name} is marked as a user snippet");
                let expansion = expand(&snippet.body, &HashMap::new(), &indent);
                assert!(
                    !expansion.text.contains("${"),
                    "{name} leaves snippet syntax in {:?}",
                    expansion.text
                );
            }
        }
    }

    #[test]
    fn literal_dollars_and_braces_in_builtin_bodies_stay_text() {
        let indent = SnippetIndent {
            line: String::new(),
            unit: "  ".to_string(),
        };
        let body = |language: &str, prefix: &str| {
            builtin(language)
                .into_iter()
                .find(|snippet| snippet.prefixes.iter().any(|p| p == prefix))
                .unwrap()
                .body
        };
        let reactive = expand(&body("svelte", "reactive"), &HashMap::new(), &indent);
        assert_eq!(reactive.text, "$: name = value;");
        let awaiting = expand(&body("svelte", "await"), &HashMap::new(), &indent);
        assert!(awaiting.text.contains("  <p>{error.message}</p>\n"));
    }

    #[test]
    fn languages_combine_their_files() {
        let tsx = prefixes("typescriptreact");
        for prefix in ["useState", "interface", "log"] {
            assert!(
                tsx.iter().any(|p| p == prefix),
                "typescriptreact lacks {prefix}"
            );
        }
        assert!(!tsx.iter().any(|p| p == "require"));
        let jsx = prefixes("javascriptreact");
        for prefix in ["useState", "require", "log"] {
            assert!(
                jsx.iter().any(|p| p == prefix),
                "javascriptreact lacks {prefix}"
            );
        }
        let vue = prefixes("vue");
        for prefix in ["vfor", "log"] {
            assert!(vue.iter().any(|p| p == prefix), "vue lacks {prefix}");
        }
        assert!(!prefixes("typescript").iter().any(|p| p == "useState"));
        assert!(!prefixes("javascript").iter().any(|p| p == "interface"));
        assert_eq!(prefixes("scss"), prefixes("css"));
        assert!(prefixes("html").iter().any(|p| p == "!"));
    }

    #[test]
    fn unknown_languages_have_no_snippets() {
        assert!(builtin("toml").is_empty());
        assert!(builtin("markdown").is_empty());
        assert!(builtin("").is_empty());
    }

    #[test]
    fn parse_file_reads_strings_and_arrays_in_file_order() {
        let text = r#"{
            "Zeta": {"prefix": "z", "body": "zeta($1)", "description": "Last by name."},
            "Alpha": {"prefix": ["a", "al"], "body": ["one", "\ttwo"]},
            "Hidden": {"body": "only through Insert Snippet", "description": ["Two", "lines"]}
        }"#;
        let snippets = parse_file(text, true).unwrap();
        assert_eq!(
            snippets,
            vec![
                SnippetDef {
                    description: "Last by name.".to_string(),
                    ..def("Zeta", &["z"], "zeta($1)", true)
                },
                def("Alpha", &["a", "al"], "one\n\ttwo", true),
                SnippetDef {
                    description: "Two\nlines".to_string(),
                    ..def("Hidden", &[], "only through Insert Snippet", true)
                },
            ]
        );
        assert!(!parse_file(text, false).unwrap()[0].user);
    }

    #[test]
    fn parse_file_accepts_comments_and_empty_files() {
        let text = "// My snippets\n{\n  /* a block\n     comment */\n  \"Url\": {\"prefix\": \"url\", // trailing\n    \"body\": \"https://example.com/* not a comment */\"}\n}\n";
        let snippets = parse_file(text, true).unwrap();
        assert_eq!(
            snippets,
            vec![def(
                "Url",
                &["url"],
                "https://example.com/* not a comment */",
                true
            )]
        );
        assert_eq!(parse_file("", true).unwrap(), vec![]);
        assert_eq!(parse_file("  // nothing yet\n", true).unwrap(), vec![]);
        assert_eq!(parse_file("{}", true).unwrap(), vec![]);
    }

    #[test]
    fn strip_json_comments_keeps_strings_and_line_breaks() {
        assert_eq!(
            strip_json_comments("{\"a\": 1, // one\n\"b\": /* two\nlines */ 2}"),
            "{\"a\": 1, \n\"b\":  \n 2}"
        );
        assert_eq!(
            strip_json_comments(r#"["a \" // b", "c \\", /* d */ "e"]"#),
            r#"["a \" // b", "c \\",   "e"]"#
        );
        assert_eq!(strip_json_comments("1 /* open"), "1  ");
        assert_eq!(strip_json_comments("\"// é\" // é"), "\"// é\" ");
    }

    #[test]
    fn parse_file_reports_problems() {
        let error = format!(
            "{:#}",
            parse_file("{\n  \"A\": {\"body\": \"a\"\n", true).unwrap_err()
        );
        assert!(error.contains("line 3"), "{error}");
        let error = format!("{:#}", parse_file("[]", true).unwrap_err());
        assert!(error.contains("object of snippets"), "{error}");
        let error = format!(
            "{:#}",
            parse_file(r#"{"A": {"prefix": "a"}}"#, true).unwrap_err()
        );
        assert!(error.contains("\"A\" has no body"), "{error}");
        let error = format!(
            "{:#}",
            parse_file(r#"{"B": {"body": 1}}"#, true).unwrap_err()
        );
        assert!(error.contains("\"B\""), "{error}");
        let error = format!(
            "{:#}",
            parse_file(r#"{"C": {"body": "c", "prefix": [1]}}"#, true).unwrap_err()
        );
        assert!(error.contains("\"C\""), "{error}");
        let error = format!("{:#}", parse_file(r#"{"D": "d"}"#, true).unwrap_err());
        assert!(error.contains("\"D\""), "{error}");
    }

    #[test]
    fn merged_puts_user_snippets_first_and_replaces_builtins_by_prefix() {
        let builtin = vec![
            def("Function", &["fn"], "fn $1() {}", false),
            def("Document", &["html5", "!"], "<!DOCTYPE html>", false),
            def("For", &["for"], "for $1 in $2 {}", false),
        ];
        let user = vec![
            def("My function", &["fn", "f"], "fn $1() -> u8 {}", true),
            def("Bang", &["!"], "!", true),
        ];
        assert_eq!(
            merged(builtin.clone(), user.clone()),
            vec![user[0].clone(), user[1].clone(), builtin[2].clone()]
        );
        assert_eq!(merged(builtin.clone(), vec![]), builtin);
    }

    #[test]
    fn user_snippets_in_a_missing_folder_are_empty() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("snippets");
        let mut snippets = UserSnippets::new(dir.clone());
        assert_eq!(snippets.dir(), dir);
        assert_eq!(snippets.for_language("rust"), (vec![], None));
    }

    #[test]
    fn user_snippets_combine_the_language_file_and_all_json() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("rust.json"),
            r#"{"Ok": {"prefix": "ok", "body": "Ok(())"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("all.json"),
            r#"{"Todo": {"prefix": "todo", "body": "TODO: $1"}}"#,
        )
        .unwrap();
        let mut snippets = UserSnippets::new(temp.path().to_path_buf());
        assert_eq!(
            snippets.for_language("rust"),
            (
                vec![
                    def("Ok", &["ok"], "Ok(())", true),
                    def("Todo", &["todo"], "TODO: $1", true),
                ],
                None
            )
        );
        assert_eq!(
            snippets.for_language("python"),
            (vec![def("Todo", &["todo"], "TODO: $1", true)], None)
        );
    }

    #[test]
    fn user_snippets_are_read_again_when_a_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("rust.json");
        let mut snippets = UserSnippets::new(temp.path().to_path_buf());
        std::fs::write(&file, r#"{"A": {"prefix": "a", "body": "a"}}"#).unwrap();
        assert_eq!(
            snippets.for_language("rust").0,
            vec![def("A", &["a"], "a", true)]
        );
        std::fs::write(&file, r#"{"Bee": {"prefix": "bee", "body": "bee"}}"#).unwrap();
        assert_eq!(
            snippets.for_language("rust").0,
            vec![def("Bee", &["bee"], "bee", true)]
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(snippets.for_language("rust"), (vec![], None));
    }

    #[test]
    fn a_broken_user_file_is_reported_and_the_others_still_load() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("rust.json"), "{ \"A\": ").unwrap();
        std::fs::write(
            temp.path().join("all.json"),
            r#"{"Todo": {"prefix": "todo", "body": "TODO"}}"#,
        )
        .unwrap();
        let mut snippets = UserSnippets::new(temp.path().to_path_buf());
        let (found, error) = snippets.for_language("rust");
        assert_eq!(found, vec![def("Todo", &["todo"], "TODO", true)]);
        let error = error.unwrap();
        assert!(error.starts_with("snippets/rust.json: "), "{error}");
        assert!(error.contains("line 1"), "{error}");
    }
}
