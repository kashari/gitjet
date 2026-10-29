//! An interactive program (the user's terminal editor) running in a pseudo-terminal and drawn in
//! the main area, so gitjet's tabs and status bar stay on screen while it runs. ConPTY on Windows.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Clear;
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::ui::{KeyHelp, Popup};

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct TerminalPane {
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
    /// (rows, cols) of the pty, kept in step with the pane's area.
    size: (u16, u16),
}

impl TerminalPane {
    /// Start `argv` in `cwd`, sized for the main area of a `screen`-sized terminal.
    pub fn spawn(argv: &[String], cwd: &Path, screen: Rect) -> Result<Self> {
        let (program, args) = argv.split_first().ok_or_else(|| anyhow!("no command"))?;
        let area = crate::app::layout(screen)[1];
        let size = (area.height.max(1), area.width.max(1));
        let pair = native_pty_system().openpty(pty_size(size))?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        if !cfg!(windows) {
            cmd.env("TERM", "xterm-256color");
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader()?;
        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.0, size.1, 0)));
        let (p, w) = (parser.clone(), writer.clone());
        std::thread::spawn(move || pump(reader, &p, &w));
        Ok(Self {
            parser,
            master: pair.master,
            writer,
            child,
            size,
        })
    }

    fn send(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes).and_then(|_| w.flush());
        }
    }
}

/// Feed the program's output to the screen model until it closes the pty.
fn pump(mut reader: Box<dyn Read + Send>, parser: &Mutex<vt100::Parser>, writer: &Writer) {
    let mut buf = [0u8; 8192];
    while let Ok(n @ 1..) = reader.read(&mut buf) {
        let Ok(mut p) = parser.lock() else { return };
        p.process(&buf[..n]);
        // ConPTY asks where the cursor is when it starts (and vim does to probe the terminal),
        // then waits for the answer.
        if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
            let (row, col) = p.screen().cursor_position();
            if let Ok(mut w) = writer.lock() {
                let _ = write!(w, "\x1b[{};{}R", row + 1, col + 1).and_then(|_| w.flush());
            }
        }
    }
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}

impl Popup for TerminalPane {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let r = crate::app::layout(area)[1];
        if r.width == 0 || r.height == 0 {
            return;
        }
        if (r.height, r.width) != self.size {
            self.size = (r.height, r.width);
            let _ = self.master.resize(pty_size(self.size));
            if let Ok(mut p) = self.parser.lock() {
                p.set_size(r.height, r.width);
            }
        }
        f.render_widget(Clear, r);
        let Ok(p) = self.parser.lock() else { return };
        let screen = p.screen();
        let buf = f.buffer_mut();
        for row in 0..r.height {
            for col in 0..r.width {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default()
                    .fg(color(cell.fgcolor()))
                    .bg(color(cell.bgcolor()));
                for (on, m) in [
                    (cell.bold(), Modifier::BOLD),
                    (cell.italic(), Modifier::ITALIC),
                    (cell.underline(), Modifier::UNDERLINED),
                    (cell.inverse(), Modifier::REVERSED),
                ] {
                    if on {
                        style = style.add_modifier(m);
                    }
                }
                let out = &mut buf[(r.x + col, r.y + row)];
                match cell.contents() {
                    s if s.is_empty() => out.set_symbol(" "),
                    s => out.set_symbol(&s),
                };
                out.set_style(style);
            }
        }
        if !screen.hide_cursor() {
            let (row, col) = screen.cursor_position();
            f.set_cursor_position((r.x + col.min(r.width - 1), r.y + row.min(r.height - 1)));
        }
    }

    fn handle_key(&mut self, key: KeyEvent, _ctx: &Ctx) -> Action {
        let app_cursor = self
            .parser
            .lock()
            .is_ok_and(|p| p.screen().application_cursor());
        self.send(&key_bytes(key, app_cursor));
        Action::None
    }

    fn poll(&mut self, _ctx: &Ctx) -> Action {
        match self.child.try_wait() {
            Ok(None) => Action::None,
            Ok(Some(s)) if s.success() => Action::Batch(vec![Action::ClosePopup, Action::Refresh]),
            Ok(Some(s)) => Action::Batch(vec![
                Action::ClosePopup,
                Action::Refresh,
                Action::Toast(format!("Editor exited with code {}", s.exit_code())),
            ]),
            Err(e) => Action::Batch(vec![
                Action::ClosePopup,
                Action::Error(format!("Editor: {e}")),
            ]),
        }
    }

    fn captures_everything(&self) -> bool {
        true
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![("editor", "every key goes to the editor; quit it to return")]
    }
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The bytes a terminal sends for `key` (xterm conventions).
fn key_bytes(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let csi_or_ss3 = |c: u8| {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let bytes: Vec<u8> = match key.code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            c @ 'a'..='z' => vec![c as u8 - b'a' + 1],
            ' ' | '@' | '2' => vec![0],
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '6' => vec![0x1e],
            '_' | '-' | '/' | '7' => vec![0x1f],
            c => c.to_string().into_bytes(),
        },
        KeyCode::Char('\n') | KeyCode::Enter => vec![b'\r'],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi_or_ss3(b'A'),
        KeyCode::Down => csi_or_ss3(b'B'),
        KeyCode::Right => csi_or_ss3(b'C'),
        KeyCode::Left => csi_or_ss3(b'D'),
        KeyCode::Home => csi_or_ss3(b'H'),
        KeyCode::End => csi_or_ss3(b'F'),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(n @ 1..=4) => vec![0x1b, b'O', b'P' + n - 1],
        KeyCode::F(n @ 5..=12) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5];
            format!("\x1b[{code}~").into_bytes()
        }
        _ => Vec::new(),
    };
    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        [&[0x1b][..], &bytes].concat()
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::temp_repo;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::{Duration, Instant};

    fn k(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        key_bytes(KeyEvent::new(code, m), false)
    }

    #[test]
    fn keys_encode_like_xterm() {
        let none = KeyModifiers::NONE;
        assert_eq!(k(KeyCode::Char('x'), none), b"x");
        assert_eq!(k(KeyCode::Char('é'), none), "é".as_bytes());
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), [3]);
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx");
        assert_eq!(k(KeyCode::Enter, none), b"\r");
        assert_eq!(k(KeyCode::Esc, none), [0x1b]);
        assert_eq!(k(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(key_bytes(KeyEvent::new(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(k(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(k(KeyCode::F(12), none), b"\x1b[24~");
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_program_in_the_main_area_and_closes_when_it_exits() {
        let (dir, repo) = temp_repo();
        let ctx = Ctx::new(repo, Config::default());
        let argv = ["sh", "-c", "printf 'hello pane'; read x"].map(String::from);
        let mut pane = TerminalPane::spawn(&argv, dir.path(), Rect::new(0, 0, 40, 10)).unwrap();
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            term.draw(|f| pane.draw(f, f.area(), &ctx)).unwrap();
            // Row 0 is gitjet's tab bar; the program starts on row 1.
            let row: String = (0..40)
                .map(|x| term.backend().buffer()[(x, 1)].symbol().to_string())
                .collect();
            if row.starts_with("hello pane") {
                break;
            }
            assert!(Instant::now() < deadline, "row 1: {row:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(pane.poll(&ctx), Action::None));
        pane.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Action::Batch(v) = pane.poll(&ctx) {
                assert!(matches!(v[0], Action::ClosePopup));
                break;
            }
            assert!(Instant::now() < deadline, "program did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
