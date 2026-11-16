use rusqlite::Connection;
use tandem_core::appearance::{AppearancePrefs, SYSTEM_FONT};
use tandem_core::{Store, WorkspaceState};

fn write_setting(directory: &std::path::Path, value: &str) {
    let connection = Connection::open(directory.join("workspace.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('appearance',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [value],
        )
        .unwrap();
}

#[test]
fn a_new_profile_has_the_default_appearance() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store.appearance_prefs().unwrap(),
        AppearancePrefs::default()
    );
}

#[test]
fn the_appearance_survives_reopening_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let mut prefs = AppearancePrefs {
        opacity: 70,
        blur: 20,
        ..AppearancePrefs::default()
    };
    prefs.fonts.editor.family = "Menlo".into();
    prefs.fonts.editor.size = 16.;
    prefs.fonts.editor.ligatures = false;
    prefs.fonts.terminal.size = 13.;
    prefs.fonts.chat.family = "Helvetica Neue".into();
    prefs.fonts.chat.code_ligatures = false;
    prefs.fonts.explorer.family = SYSTEM_FONT.into();
    prefs.fonts.explorer.size = 12.;
    {
        let mut store = Store::open(directory.path()).unwrap();
        store.save_appearance_prefs(&prefs).unwrap();
        assert_eq!(store.appearance_prefs().unwrap(), prefs);
    }
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.appearance_prefs().unwrap(), prefs);
}

#[test]
fn saving_and_loading_keep_values_inside_their_limits() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let mut prefs = AppearancePrefs {
        opacity: 1,
        blur: 255,
        ..AppearancePrefs::default()
    };
    prefs.fonts.editor.size = 400.;
    store.save_appearance_prefs(&prefs).unwrap();
    let loaded = store.appearance_prefs().unwrap();
    assert_eq!((loaded.opacity, loaded.blur), (30, 100));
    assert_eq!(loaded.fonts.editor.size, 30.);
    drop(store);

    // A row written by hand, or by a later version, is brought inside the limits too.
    write_setting(
        directory.path(),
        r#"{"opacity":0,"blur":7,"fonts":{"terminal":{"size":3}}}"#,
    );
    let loaded = Store::open(directory.path())
        .unwrap()
        .appearance_prefs()
        .unwrap();
    assert_eq!((loaded.opacity, loaded.blur), (30, 7));
    assert_eq!(loaded.fonts.terminal.size, 10.);
    assert_eq!(loaded.fonts.terminal.family, "FiraCode Nerd Font Mono");
}

#[test]
fn a_damaged_row_is_an_error_and_other_settings_are_untouched() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut store = Store::open(directory.path()).unwrap();
        let layout = tandem_core::layout::PaneLayout {
            swapped: true,
            ..Default::default()
        };
        store.save_pane_layout(&layout).unwrap();
    }
    write_setting(directory.path(), "{not json");
    let mut store = Store::open(directory.path()).unwrap();
    let error = store.appearance_prefs().unwrap_err();
    assert!(format!("{error:#}").contains("The saved appearance settings are damaged"));
    assert!(store.pane_layout().unwrap().swapped);
    // Saving replaces the damaged row.
    store
        .save_appearance_prefs(&AppearancePrefs::default())
        .unwrap();
    assert_eq!(
        store.appearance_prefs().unwrap(),
        AppearancePrefs::default()
    );
}

#[test]
fn workspace_rows_load_with_and_without_a_font_size() {
    let old: WorkspaceState = serde_json::from_str(
        r#"{"root":"/p","tabs":["a.rs"],"active":"a.rs","font_size":18.0,"sequence":3}"#,
    )
    .unwrap();
    assert_eq!(old.font_size, 18.);
    let without: WorkspaceState =
        serde_json::from_str(r#"{"root":"/p","tabs":[],"active":null}"#).unwrap();
    assert_eq!(without.font_size, 14.);
}
