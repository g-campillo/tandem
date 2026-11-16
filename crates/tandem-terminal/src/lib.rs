use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt};
use gpui_kit::*;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// The rows of the terminal.
const ROWS: u16 = 9;

/// The font the terminal's output is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalFont {
    pub family: SharedString,
    /// The size in pixels.
    pub size: f32,
    pub features: FontFeatures,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self {
            family: "Menlo".into(),
            size: 12.,
            features: FontFeatures::default(),
        }
    }
}

impl TerminalFont {
    /// The height of a row: one and a half times the size, in whole pixels.
    pub fn line_height(&self) -> f32 {
        (self.size * 1.5).round()
    }
}

/// The height of the terminal view in `font`: its rows, and 68 px for the buttons
/// above them and the padding.
pub fn view_height(font: &TerminalFont) -> f32 {
    f32::from(ROWS) * font.line_height() + 68.
}

/// How many rows a scroll of `pixels` moves when a row is `line_height` high.
pub fn scroll_lines(pixels: f32, line_height: f32) -> i32 {
    if line_height > 0. && pixels.is_finite() {
        (pixels / line_height).round() as i32
    } else {
        0
    }
}

#[derive(Clone, Copy)]
struct Size {
    rows: u16,
    cols: u16,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows.into()
    }
    fn screen_lines(&self) -> usize {
        self.rows.into()
    }
    fn columns(&self) -> usize {
        self.cols.into()
    }
}
impl Size {
    fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;
struct Events(Writer);
impl EventListener for Events {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event
            && let Ok(mut writer) = self.0.lock()
        {
            let _ = writer.write_all(text.as_bytes());
        }
        // No OSC clipboard writes, external links, or host actions in this probe.
    }
}

pub struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
    grid: Arc<Mutex<Term<Events>>>,
    revision: Arc<AtomicU64>,
}

impl Terminal {
    pub fn new(cwd: &Path) -> Result<Self> {
        let size = Size {
            rows: ROWS,
            cols: 100,
        };
        let pair = native_pty_system().openpty(size.pty())?;
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-i");
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("PS1", "tandem$ ");
        command.env("ENV", "/dev/null");
        // Finder launches have a sparse PATH. Extend only this child process,
        // preserving inherited precedence and leaving shell/global files alone.
        let mut paths: Vec<PathBuf> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        if let Some(home) = std::env::var_os("HOME") {
            for candidate in [
                PathBuf::from(home).join(".cargo/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ] {
                if candidate.is_dir() && !paths.contains(&candidate) {
                    paths.push(candidate);
                }
            }
        }
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
        let child = pair.slave.spawn_command(command)?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let grid = Arc::new(Mutex::new(Term::new(
            Config {
                scrolling_history: 2000,
                ..Default::default()
            },
            &size,
            Events(writer.clone()),
        )));
        let revision = Arc::new(AtomicU64::new(0));
        let updated = revision.clone();
        let output = grid.clone();
        std::thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut bytes = [0u8; 4096];
            while let Ok(length) = reader.read(&mut bytes) {
                if length == 0 {
                    break;
                }
                if let Ok(mut term) = output.lock() {
                    parser.advance(&mut *term, &bytes[..length]);
                } else {
                    break;
                }
                updated.fetch_add(1, Ordering::Relaxed);
            }
        });
        Ok(Self {
            master: pair.master,
            writer,
            child,
            grid,
            revision,
        })
    }

    fn send(&self, bytes: &[u8]) -> Result<()> {
        self.scroll(Scroll::Bottom);
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal writer unavailable"))?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    fn text(&self) -> String {
        let Ok(term) = self.grid.lock() else {
            return String::new();
        };
        visible_text(&term)
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let size = Size { rows, cols };
        self.master.resize(size.pty())?;
        self.grid
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal grid unavailable"))?
            .resize(size);
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn visible_text(term: &Term<Events>) -> String {
    let mut lines = vec![String::new(); term.screen_lines()];
    for cell in term.grid().display_iter() {
        if cell
            .cell
            .flags
            .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
        {
            continue;
        }
        let row = cell.point.line.0 + term.grid().display_offset() as i32;
        if let Some(line) = lines.get_mut(row as usize) {
            line.push(cell.cell.c);
            if let Some(extra) = cell.cell.zerowidth() {
                line.extend(extra);
            }
        }
    }
    lines
        .into_iter()
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Terminal {
    fn scroll(&self, scroll: Scroll) {
        if let Ok(mut term) = self.grid.lock() {
            term.scroll_display(scroll);
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
    fn is_busy(&self) -> bool {
        #[cfg(unix)]
        {
            self.master
                .process_group_leader()
                .zip(self.child.process_id())
                .is_some_and(|(foreground, shell)| foreground != shell as i32)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(foreground) = self.master.process_group_leader()
            && foreground > 1
        {
            // This group belongs to our own PTY, never to the caller's shell.
            unsafe {
                libc::kill(-foreground, libc::SIGHUP);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct TerminalView {
    terminal: Terminal,
    focus: FocusHandle,
    text: String,
    revision: u64,
    columns: u16,
    font: TerminalFont,
}

pub fn start() -> Result<Terminal> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
    let cwd = home.join("Library/Caches/Tandem/m0-terminal");
    std::fs::create_dir_all(&cwd)?;
    Terminal::new(&cwd)
}

impl TerminalView {
    pub fn resize_columns(&mut self, columns: u16) {
        let columns = columns.clamp(40, 300);
        if columns != self.columns && self.terminal.resize(ROWS, columns).is_ok() {
            self.columns = columns;
        }
    }
    pub fn is_busy(&self) -> bool {
        self.terminal.is_busy()
    }
    pub fn new(terminal: Terminal, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |view, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(33)).await;
                if view
                    .update(cx, |view, cx| {
                        let revision = view.terminal.revision.load(Ordering::Relaxed);
                        if revision != view.revision {
                            view.revision = revision;
                            view.text = view.terminal.text();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            terminal,
            focus: cx.focus_handle(),
            text: String::new(),
            revision: 0,
            columns: 100,
            font: TerminalFont::default(),
        }
    }

    /// Draws the output in `font` from now on.
    pub fn set_font(&mut self, font: TerminalFont, cx: &mut Context<Self>) {
        if self.font != font {
            self.font = font;
            cx.notify();
        }
    }

    /// The width of one character of the terminal's font, in pixels.
    pub fn cell_width(&self, window: &Window) -> f32 {
        let font = Font {
            family: self.font.family.clone(),
            features: self.font.features.clone(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let id = window.text_system().resolve_font(&font);
        window
            .text_system()
            .ch_advance(id, px(self.font.size))
            .map_or(self.font.size * 0.6, f32::from)
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let bytes = if key.modifiers.platform && key.key == "v" {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default()
                .into_bytes()
        } else if key.modifiers.control && key.key.len() == 1 {
            vec![key.key.as_bytes()[0] & 0x1f]
        } else if key.modifiers.platform {
            return;
        } else {
            match key.key.as_str() {
                "enter" => b"\r".to_vec(),
                "backspace" => vec![0x7f],
                "tab" => b"\t".to_vec(),
                "escape" => vec![0x1b],
                "up" => b"\x1b[A".to_vec(),
                "down" => b"\x1b[B".to_vec(),
                "right" => b"\x1b[C".to_vec(),
                "left" => b"\x1b[D".to_vec(),
                _ => key.key_char.as_deref().unwrap_or("").as_bytes().to_vec(),
            }
        };
        if !bytes.is_empty() {
            let _ = self.terminal.send(&bytes);
            cx.stop_propagation();
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line = self.font.line_height();
        div()
            .id("human-terminal")
            .role(Role::Group)
            .aria_label(
                "Human terminal. Click to type. Control C interrupts the foreground command.",
            )
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_key_down(cx.listener(|this, event, _, cx| this.key(event, cx)))
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                let lines = scroll_lines(f32::from(event.delta.pixel_delta(px(line)).y), line);
                this.terminal.scroll(Scroll::Delta(lines));
                cx.stop_propagation();
            }))
            .v_flex()
            .w_full()
            .min_w_0()
            .h(px(view_height(&self.font)))
            .flex_shrink_0()
            .overflow_hidden()
            .p_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .min_w_0()
                    .flex_shrink_0()
                    .gap_1()
                    .mb_1()
                    .font_family(".SystemUIFont")
                    .child(
                        Button::new("terminal-interrupt")
                            .small()
                            .compact()
                            .h(px(28.))
                            .ghost()
                            .accessibility_label("Interrupt")
                            .child(div().text_size(px(13.)).child("Interrupt"))
                            .on_click(cx.listener(|this, _, _, _| {
                                let _ = this.terminal.send(&[3]);
                            })),
                    )
                    .child(
                        Button::new("terminal-copy")
                            .small()
                            .compact()
                            .h(px(28.))
                            .ghost()
                            .accessibility_label("Copy Visible Output")
                            .child(div().text_size(px(13.)).child("Copy Visible Output"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(this.text.clone()))
                            })),
                    )
                    .child(
                        Button::new("terminal-bottom")
                            .small()
                            .compact()
                            .h(px(28.))
                            .ghost()
                            .accessibility_label("Latest Output")
                            .child(div().text_size(px(13.)).child("Latest Output"))
                            .on_click(
                                cx.listener(|this, _, _, _| this.terminal.scroll(Scroll::Bottom)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("terminal-output")
                    .role(Role::Label)
                    .aria_label(self.text.clone())
                    .font_family(self.font.family.clone())
                    .font_features(self.font.features.clone())
                    .text_size(px(self.font.size))
                    .line_height(px(line))
                    .children(
                        self.text
                            .lines()
                            .map(|text| div().h(px(line)).child(text.to_string())),
                    ),
            )
    }
}

fn wait_for_line(terminal: &Terminal, expected: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if terminal.text().lines().any(|line| line.trim() == expected) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!(
        "terminal fixture did not produce {expected:?}; fixture grid: {:?}",
        terminal.text()
    )
}

pub fn check() -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME unavailable")?);
    let cwd = home.join("Library/Caches/Tandem/m0-terminal");
    std::fs::create_dir_all(&cwd)?;
    let terminal = Terminal::new(&cwd)?;
    let pid = terminal
        .child
        .process_id()
        .context("terminal child PID unavailable")?;
    terminal.send("printf '\\033[31mTANDEM_UNICODE_café\\033[0m\\n'\r".as_bytes())?;
    wait_for_line(&terminal, "TANDEM_UNICODE_café")?;
    terminal.resize(10, 100)?;
    terminal.send(b"stty size\r")?;
    wait_for_line(&terminal, "10 100")?;
    terminal.send(b"sleep 30\r")?;
    std::thread::sleep(Duration::from_millis(150));
    terminal.send(&[3])?;
    terminal.send(b"printf 'TANDEM_AFTER_INTERRUPT\\n'\r")?;
    wait_for_line(&terminal, "TANDEM_AFTER_INTERRUPT")?;
    drop(terminal);
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success();
    ensure!(!alive, "terminal child survived cleanup");
    let receipt = serde_json::json!({"gate": "M0-B terminal slice", "pty": "portable-pty 0.9.0", "engine": "alacritty_terminal 0.26.0", "unicode_and_ansi": true, "resize": true, "ctrl_c": true, "child_reaped": true,
        "limitations": ["probe renderer uses plain monochrome cells", "selection/scrollback UI and terminal IME are not implemented"]});
    std::fs::write(
        ".tandem-local/m0/terminal-receipt.json",
        serde_json::to_string_pretty(&receipt)?,
    )?;
    println!("Native PTY, terminal parsing, resize, Ctrl-C and child cleanup passed.");
    Ok(())
}
