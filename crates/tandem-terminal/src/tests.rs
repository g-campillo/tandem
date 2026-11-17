use super::{
    Events, Size, Terminal, TerminalFont, Writer, scroll_lines, view_height, visible_text,
    wait_for_line,
};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
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
