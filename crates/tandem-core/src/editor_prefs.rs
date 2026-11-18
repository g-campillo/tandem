//! How the editor behaves beyond typing: whether a save formats the document first and
//! whether the language server's type and parameter-name hints are shown. One preference
//! for every project, saved in `settings` under `editor`.
use crate::Store;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPrefs {
    /// Format the document with its language server before saving it.
    pub format_on_save: bool,
    /// Show the inferred types of variables and expressions at the end of the line.
    pub type_hints: bool,
    /// Show the parameter names of a call's arguments at the end of the line.
    pub parameter_hints: bool,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self {
            format_on_save: false,
            type_hints: true,
            parameter_hints: true,
        }
    }
}

impl Store {
    /// The saved editor settings, or the defaults when none were saved.
    pub fn editor_prefs(&self) -> Result<EditorPrefs> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key='editor'", [], |r| {
                r.get(0)
            })
            .optional()?;
        value
            .map(|value| {
                serde_json::from_str(&value).context("The saved editor settings are damaged")
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn save_editor_prefs(&mut self, prefs: &EditorPrefs) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key,value) VALUES('editor',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(prefs)?],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_format_nothing_on_save_and_show_both_hints() {
        let prefs = EditorPrefs::default();
        assert!(!prefs.format_on_save);
        assert!(prefs.type_hints);
        assert!(prefs.parameter_hints);
    }

    #[test]
    fn the_prefs_default_when_absent_and_survive_a_reopen() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = Store::open(directory.path()).unwrap();
            assert_eq!(store.editor_prefs().unwrap(), EditorPrefs::default());
        }
        let prefs = EditorPrefs {
            format_on_save: true,
            type_hints: false,
            parameter_hints: true,
        };
        Store::open(directory.path())
            .unwrap()
            .save_editor_prefs(&prefs)
            .unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.editor_prefs().unwrap(), prefs);
    }

    #[test]
    fn saving_again_replaces_the_saved_prefs() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store
            .save_editor_prefs(&EditorPrefs {
                format_on_save: true,
                ..EditorPrefs::default()
            })
            .unwrap();
        let second = EditorPrefs {
            format_on_save: false,
            type_hints: false,
            parameter_hints: false,
        };
        store.save_editor_prefs(&second).unwrap();
        assert_eq!(store.editor_prefs().unwrap(), second);
        let rows: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key='editor'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn missing_fields_take_their_defaults_and_unknown_fields_are_ignored() {
        let prefs: EditorPrefs =
            serde_json::from_str(r#"{"format_on_save":true,"later":1}"#).unwrap();
        assert_eq!(
            prefs,
            EditorPrefs {
                format_on_save: true,
                type_hints: true,
                parameter_hints: true,
            }
        );
    }

    #[test]
    fn a_saved_value_with_missing_fields_loads_with_their_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .connection
            .execute(
                r#"INSERT INTO settings(key,value) VALUES('editor','{"type_hints":false,"later":1}')"#,
                [],
            )
            .unwrap();
        assert_eq!(
            store.editor_prefs().unwrap(),
            EditorPrefs {
                format_on_save: false,
                type_hints: false,
                parameter_hints: true,
            }
        );
    }

    #[test]
    fn damaged_prefs_are_an_error_and_are_left_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO settings(key,value) VALUES('editor','not json')",
                [],
            )
            .unwrap();
        let error = store.editor_prefs().unwrap_err();
        assert_eq!(error.to_string(), "The saved editor settings are damaged");
        let value: String = store
            .connection
            .query_row("SELECT value FROM settings WHERE key='editor'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "not json");
    }
}
