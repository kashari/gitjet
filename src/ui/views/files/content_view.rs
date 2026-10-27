//! Read-only viewer for a file at a revision (`git show rev:path`) with line numbers, find and
//! go-to-line, plus shortcuts to annotate/compare that revision.

use std::cell::Cell;
use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::common::{
    commit_details, expand_tabs, find_line, parse_line_number, LineNav, Loader, Prompt,
    PromptOutcome,
};
use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::file_history::{is_binary, text_lines};
use crate::ui::diff_view::{self, DiffSource};
use crate::ui::popups::{MenuItem, MenuPopup};
use crate::ui::widgets::{panel, TextInput};
use crate::ui::{KeyHelp, Popup};
use crate::util::truncate;

/// Loaded file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(Vec<String>),
    /// Binary file of this many bytes
    Binary(usize),
}

impl Content {
    /// Classify raw bytes as text lines or binary.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if is_binary(bytes) {
            Content::Binary(bytes.len())
        } else {
            Content::Text(text_lines(bytes))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Search,
    Goto,
}

/// Fullscreen "file at revision" viewer.
pub struct ContentPopup {
    path: String,
    rev: String,
    loader: Loader<Content>,
    content: Option<Content>,
    error: Option<String>,
    nav: LineNav,
    prompt: Prompt,
    last_search: String,
    pending: Rc<Cell<Option<Cmd>>>,
}

impl ContentPopup {
    pub fn new(ctx: &Ctx, path: String, rev: String) -> Self {
        let (p, r) = (path.clone(), rev.clone());
        Self {
            loader: Loader::start(&ctx.repo, move |repo| {
                Ok(Content::from_bytes(&repo.show_file(&r, &p)?))
            }),
            path,
            rev,
            content: None,
            error: None,
            nav: LineNav::default(),
            prompt: Prompt::None,
            last_search: String::new(),
            pending: Rc::new(Cell::new(None)),
        }
    }

    fn lines(&self) -> &[String] {
        match &self.content {
            Some(Content::Text(l)) => l,
            _ => &[],
        }
    }

    fn find(&mut self, forward: bool) {
        let len = self.lines().len();
        if let Some(i) = find_line(self.lines(), &self.last_search, self.nav.cursor, forward) {
            self.nav.jump(i, len);
        }
    }

    fn run_cmd(&mut self, cmd: Cmd) {
        self.prompt = match cmd {
            Cmd::Search => Prompt::Search(TextInput::new(&self.last_search)),
            Cmd::Goto => Prompt::Goto(TextInput::default()),
        };
    }

    fn blame(&self, ctx: &Ctx) -> Action {
        let line = (self.nav.cursor + 1) as u32;
        Action::popup(super::blame_view::BlamePopup::new(
            ctx,
            self.path.clone(),
            Some(self.rev.clone()),
            Some(line),
        ))
    }

    fn compare_worktree(&self) -> Action {
        diff_view::open(
            format!("{} @ {} ↔ local", self.path, truncate(&self.rev, 12)),
            DiffSource::RevToWorktree {
                rev: self.rev.clone(),
                paths: vec![self.path.clone()],
            },
        )
    }

    fn menu(&self) -> Action {
        let pending = self.pending.clone();
        let set = move |c: Cmd| {
            let p = pending.clone();
            move |_: &Ctx| {
                p.set(Some(c));
                Action::None
            }
        };
        let blame = (self.path.clone(), self.rev.clone());
        let diff = self.compare_worktree();
        let path = self.path.clone();
        let rev = self.rev.clone();
        let items = vec![
            MenuItem::new("Annotate this revision", "b", move |ctx| {
                super::open_blame(ctx, &blame.0, Some(&blame.1))
            }),
            MenuItem::new("Compare with local (work tree)", "w", move |_| diff),
            MenuItem::new("Show file history", "h", move |ctx| {
                super::open_file_history(ctx, &path)
            }),
            MenuItem::new("Commit details", "c", move |ctx| commit_details(ctx, &rev)),
            MenuItem::new("Find in file", "/", set(Cmd::Search)),
            MenuItem::new("Go to line", "g", set(Cmd::Goto)),
        ];
        Action::popup(MenuPopup::new(truncate(&self.path, 40), items))
    }
}

impl Popup for ContentPopup {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        if let Some(res) = self.loader.poll() {
            match res {
                Ok(c) => self.content = Some(c),
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
        if let Some(cmd) = self.pending.take() {
            self.run_cmd(cmd);
        }
        let t = &ctx.theme;
        f.render_widget(Clear, area);
        let title = Line::from(vec![
            Span::styled(" File ", t.title()),
            Span::styled(format!("{} ", self.path), t.text()),
            Span::styled(
                format!("@ {} ", truncate(&self.rev, 14)),
                Style::default().fg(t.hash),
            ),
        ]);
        let block = panel(title, true, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
        if let Some(e) = &self.error {
            f.render_widget(
                Paragraph::new(e.as_str()).style(Style::default().fg(t.error)),
                body,
            );
            return;
        }
        let lines = match &self.content {
            None => {
                f.render_widget(Paragraph::new("Loading…").style(t.dimmed()), body);
                return;
            }
            Some(Content::Binary(n)) => {
                let msg = format!("Binary file ({n} bytes) — content not shown");
                f.render_widget(Paragraph::new(msg).style(t.dimmed()), body);
                return;
            }
            Some(Content::Text(l)) if l.is_empty() => {
                f.render_widget(Paragraph::new("The file is empty").style(t.dimmed()), body);
                return;
            }
            Some(Content::Text(l)) => l,
        };
        self.nav.fit(body.height as usize, lines.len());
        let digits = lines.len().to_string().len();
        let out: Vec<Line> = lines
            .iter()
            .enumerate()
            .skip(self.nav.offset)
            .take(body.height as usize)
            .map(|(i, l)| {
                let line = Line::from(vec![
                    Span::styled(format!("{:>digits$} │ ", i + 1), t.dimmed()),
                    Span::styled(expand_tabs(l), t.text()),
                ]);
                if i == self.nav.cursor {
                    line.style(t.selected())
                } else {
                    line
                }
            })
            .collect();
        f.render_widget(Paragraph::new(out), body);
        if self.prompt.is_active() {
            self.prompt.draw(f, footer, t);
        } else {
            let pos = format!(" {}/{} ", self.nav.cursor + 1, lines.len());
            f.render_widget(
                Paragraph::new(Span::styled(pos, t.dimmed())).right_aligned(),
                footer,
            );
        }
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let len = self.lines().len();
        if self.prompt.is_active() {
            match self.prompt.handle_key(&key) {
                PromptOutcome::Search(q) => {
                    self.last_search = q;
                    self.find(true);
                }
                PromptOutcome::Goto(s) => match parse_line_number(&s, len) {
                    Some(i) => self.nav.jump(i, len),
                    None => return Action::Toast(format!("Not a line number: {s}")),
                },
                PromptOutcome::Editing | PromptOutcome::Cancelled => {}
            }
            return Action::None;
        }
        if self.nav.handle_key(&key, len) {
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::ClosePopup,
            KeyCode::Char('b') => return self.blame(ctx),
            KeyCode::Char('w') => return self.compare_worktree(),
            KeyCode::Char('h') => return super::open_file_history(ctx, &self.path),
            KeyCode::Char('c') => return commit_details(ctx, &self.rev),
            KeyCode::Char('/') => self.run_cmd(Cmd::Search),
            KeyCode::Char('g') => self.run_cmd(Cmd::Goto),
            KeyCode::F(3) => self.find(!key.modifiers.contains(KeyModifiers::SHIFT)),
            KeyCode::Char('.') | KeyCode::Char('m') => return self.menu(),
            _ => {}
        }
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            ("↑↓/jk", "line"),
            ("PgUp/PgDn", "page"),
            ("/", "find"),
            ("F3", "find next"),
            ("g", "go to line"),
            ("b", "annotate"),
            ("w", "vs local"),
            ("h", "history"),
            ("c", "commit"),
            (".", "menu"),
            ("Esc", "close"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_content() {
        assert_eq!(
            Content::from_bytes(b"a\r\nb\n"),
            Content::Text(vec!["a".into(), "b".into()])
        );
        assert_eq!(Content::from_bytes(b"\x89PNG\0\0"), Content::Binary(6));
        assert_eq!(Content::from_bytes(b""), Content::Text(vec![]));
    }
}
