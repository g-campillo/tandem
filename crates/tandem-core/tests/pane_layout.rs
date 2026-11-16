use rusqlite::Connection;
use tandem_core::Store;
use tandem_core::layout::{Pane, PaneLayout};

fn swapped_layout() -> PaneLayout {
    let mut layout = PaneLayout {
        swapped: true,
        ..PaneLayout::default()
    };
    layout.set_width(Pane::Files, Some(300.));
    layout.set_width(Pane::Chat, Some(480.));
    layout
}

#[test]
fn the_layout_defaults_when_absent_and_survives_a_reopen() {
    let directory = tempfile::tempdir().unwrap();
    {
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.pane_layout().unwrap(), PaneLayout::default());
    }
    let layout = swapped_layout();
    Store::open(directory.path())
        .unwrap()
        .save_pane_layout(&layout)
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.pane_layout().unwrap(), layout);
}

#[test]
fn saving_again_replaces_the_saved_layout() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    store.save_pane_layout(&swapped_layout()).unwrap();
    let second = PaneLayout {
        swapped: false,
        files_width: None,
        chat_width: Some(512.),
    };
    store.save_pane_layout(&second).unwrap();
    assert_eq!(store.pane_layout().unwrap(), second);
    let connection = Connection::open(directory.path().join("workspace.sqlite")).unwrap();
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key='pane_layout'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn a_damaged_layout_is_an_error_and_is_left_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let connection = Connection::open(directory.path().join("workspace.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('pane_layout','not json')",
            [],
        )
        .unwrap();
    let error = store.pane_layout().unwrap_err();
    assert_eq!(error.to_string(), "The saved pane layout is damaged");
    let value: String = connection
        .query_row(
            "SELECT value FROM settings WHERE key='pane_layout'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "not json");
}
