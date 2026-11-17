use super::{
    BAR, CursorCell, CursorLook, CursorRect, Events, Size, Terminal, TerminalFont, Writer,
    cursor_look, cursor_rect, scroll_lines, view_height, viewport_row, visible_cursor,
    visible_text, wait_for_line,
};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{CursorShape, Processor};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn scrollback_is_bounded_and_viewport_mapping_includes_history() {
    let writer: Writer = Arc::new(Mutex::new(Box::new(std::io::sink())));
    let mut term = Term::new(
        Config {
            scrolling_history: 2000,
            ..Default::default()
        },
        &Size { rows: 3, cols: 30 },
        Events(writer),
    );
    let mut parser: Processor = Processor::new();
    for line in 0..2500 {
        parser.advance(&mut term, format!("L{line:04}\r\n").as_bytes());
    }
    assert_eq!(term.grid().history_size(), 2000);
    assert!(visible_text(&term).contains("L2499"));
    term.scroll_display(Scroll::Top);
    assert!(visible_text(&term).contains("L0498"));
    assert!(!visible_text(&term).contains("L2499"));
}

#[test]
fn terminal_text_keeps_combining_characters_and_omits_wide_spacers() {
    let writer: Writer = Arc::new(Mutex::new(Box::new(std::io::sink())));
    let mut term = Term::new(
        Config::default(),
        &Size { rows: 3, cols: 30 },
        Events(writer),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, "café e\u{301} 🦀!".as_bytes());
    assert_eq!(
        visible_text(&term).lines().next().unwrap(),
        "café e\u{301} 🦀!"
    );
}

#[test]
fn closing_terminal_stops_its_foreground_job() {
    let directory = tempfile::tempdir().unwrap();
    let terminal = Terminal::new(directory.path()).unwrap();
    terminal.send(b"printf 'READY_FOR_JOB\\n'\r").unwrap();
    wait_for_line(&terminal, "READY_FOR_JOB").unwrap();
    terminal.send(b"sleep 30\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !terminal.is_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(terminal.is_busy());
    let foreground = terminal.master.process_group_leader().unwrap();
    let shell = terminal.child.process_id().unwrap();
    drop(terminal);
    let alive = |pid: u32| {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(foreground as u32) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(shell));
    assert!(!alive(foreground as u32));
}

#[test]
fn the_default_font_is_menlo_at_12_with_the_terminals_former_height() {
    let font = TerminalFont::default();
    assert_eq!(font.family.as_ref(), "Menlo");
    assert_eq!(font.size, 12.);
    assert_eq!(font.line_height(), 18.);
    assert_eq!(view_height(&font), 230.);
}

#[test]
fn a_larger_font_makes_taller_rows_and_a_taller_view() {
    let font = TerminalFont {
        size: 14.,
        ..TerminalFont::default()
    };
    assert_eq!(font.line_height(), 21.);
    assert_eq!(view_height(&font), 257.);
}

#[test]
fn scroll_lines_counts_whole_rows_of_the_row_height() {
    assert_eq!(scroll_lines(36., 18.), 2);
    assert_eq!(scroll_lines(-18., 18.), -1);
    assert_eq!(scroll_lines(8., 18.), 0);
    assert_eq!(scroll_lines(36., 0.), 0);
    assert_eq!(scroll_lines(f32::NAN, 18.), 0);
}

/// A terminal `rows` by 30 with 2,000 rows of history that has read `input`.
fn term_after(rows: u16, input: &str) -> Term<Events> {
    let writer: Writer = Arc::new(Mutex::new(Box::new(std::io::sink())));
    let mut term = Term::new(
        Config {
            scrolling_history: 2000,
            ..Default::default()
        },
        &Size { rows, cols: 30 },
        Events(writer),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, input.as_bytes());
    term
}

#[test]
fn viewport_row_adds_the_scrollback_and_drops_rows_below_the_screen() {
    assert_eq!(viewport_row(2, 0, 3), Some(2));
    assert_eq!(viewport_row(0, 2, 3), Some(2));
    assert_eq!(viewport_row(1, 1, 3), Some(2));
    assert_eq!(viewport_row(2, 1, 3), None);
    assert_eq!(viewport_row(0, 3, 3), None);
    assert_eq!(viewport_row(-1, 0, 3), None);
    assert_eq!(viewport_row(0, 0, 0), None);
}

#[test]
fn the_cursor_follows_the_output_as_a_block_over_a_blank_cell() {
    let cursor = visible_cursor(&term_after(3, "tandem$ ab")).unwrap();
    assert_eq!(
        cursor,
        CursorCell {
            row: 0,
            column: 10,
            cells: 1,
            shape: CursorShape::Block,
            text: " ".into(),
        }
    );
    let cursor = visible_cursor(&term_after(3, "one\r\ntwo\r\nx")).unwrap();
    assert_eq!((cursor.row, cursor.column), (2, 1));
}

#[test]
fn the_cursor_carries_the_character_under_it_with_its_combining_marks() {
    // ESC [ 1 G moves the cursor back to the first column.
    let cursor = visible_cursor(&term_after(3, "e\u{301}x\x1b[1G")).unwrap();
    assert_eq!((cursor.column, cursor.cells), (0, 1));
    assert_eq!(cursor.text, "e\u{301}");
}

#[test]
fn a_cursor_on_a_wide_character_covers_both_of_its_cells() {
    for input in ["\u{1F980}\x1b[1G", "\u{1F980}\x1b[2G"] {
        let cursor = visible_cursor(&term_after(3, input)).unwrap();
        assert_eq!((cursor.column, cursor.cells), (0, 2), "{input:?}");
        assert_eq!(cursor.text, "\u{1F980}");
    }
}

#[test]
fn the_program_sets_the_cursor_shape() {
    let shape = |input: &str| visible_cursor(&term_after(3, input)).map(|cursor| cursor.shape);
    assert_eq!(shape(""), Some(CursorShape::Block));
    assert_eq!(shape("\x1b[6 q"), Some(CursorShape::Beam));
    assert_eq!(shape("\x1b[5 q"), Some(CursorShape::Beam));
    assert_eq!(shape("\x1b[4 q"), Some(CursorShape::Underline));
    assert_eq!(shape("\x1b[6 q\x1b[2 q"), Some(CursorShape::Block));
    assert_eq!(shape("\x1b[6 q\x1b[0 q"), Some(CursorShape::Block));
}

#[test]
fn the_program_hides_and_shows_the_cursor() {
    assert_eq!(visible_cursor(&term_after(3, "\x1b[?25l")), None);
    assert!(visible_cursor(&term_after(3, "\x1b[?25l\x1b[?25h")).is_some());
    assert_eq!(visible_cursor(&term_after(3, "\x1b[6 q\x1b[?25l")), None);
}

#[test]
fn scrolling_back_moves_the_cursor_down_and_then_off_the_screen() {
    // Five lines in three rows leave two in the history and the cursor on the last row.
    let mut term = term_after(3, "L0\r\nL1\r\nL2\r\nL3\r\nL4");
    assert_eq!(visible_cursor(&term).map(|cursor| cursor.row), Some(2));
    term.scroll_display(Scroll::Delta(1));
    assert_eq!(visible_cursor(&term), None);
    term.scroll_display(Scroll::Bottom);
    assert_eq!(visible_cursor(&term).map(|cursor| cursor.row), Some(2));

    // ESC [ H puts the cursor on the top row, which stays on screen one row lower.
    let mut term = term_after(3, "L0\r\nL1\r\nL2\r\nL3\r\nL4\x1b[H");
    term.scroll_display(Scroll::Delta(1));
    assert_eq!(visible_cursor(&term).map(|cursor| cursor.row), Some(1));
    term.scroll_display(Scroll::Top);
    assert_eq!(visible_cursor(&term).map(|cursor| cursor.row), Some(2));
}

#[test]
fn focus_keeps_the_programs_shape_and_without_focus_the_cursor_is_hollow() {
    for (shape, focused) in [
        (CursorShape::Block, CursorLook::Block),
        (CursorShape::Underline, CursorLook::Underline),
        (CursorShape::Beam, CursorLook::Beam),
        (CursorShape::HollowBlock, CursorLook::Hollow),
    ] {
        assert_eq!(cursor_look(shape, true), Some(focused), "{shape:?}");
        assert_eq!(
            cursor_look(shape, false),
            Some(CursorLook::Hollow),
            "{shape:?}"
        );
    }
    assert_eq!(cursor_look(CursorShape::Hidden, true), None);
    assert_eq!(cursor_look(CursorShape::Hidden, false), None);
}

#[test]
fn the_cursor_rectangle_fills_its_cells_or_draws_a_bar() {
    let at = |cells: usize| CursorCell {
        row: 2,
        column: 3,
        cells,
        shape: CursorShape::Block,
        text: " ".into(),
    };
    let rect = |x: f32, y: f32, width: f32, height: f32| CursorRect {
        x,
        y,
        width,
        height,
    };
    assert_eq!(
        cursor_rect(CursorLook::Block, &at(1), 7.5, 18.),
        rect(22.5, 36., 7.5, 18.)
    );
    assert_eq!(
        cursor_rect(CursorLook::Hollow, &at(1), 7.5, 18.),
        rect(22.5, 36., 7.5, 18.)
    );
    assert_eq!(
        cursor_rect(CursorLook::Block, &at(2), 7.5, 18.),
        rect(22.5, 36., 15., 18.)
    );
    assert_eq!(
        cursor_rect(CursorLook::Beam, &at(2), 7.5, 18.),
        rect(22.5, 36., BAR, 18.)
    );
    assert_eq!(
        cursor_rect(CursorLook::Underline, &at(1), 7.5, 18.),
        rect(22.5, 52., 7.5, BAR)
    );
    assert_eq!(BAR, 2.);
}
