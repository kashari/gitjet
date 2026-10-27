//! Fullscreen Annotate (blame) popup: show the commit diff of a line,
//! annotate the previous revision, commit details, file history and history for a selection.

use std::cell::Cell;
use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::common::{
    age_color, commit_details, expand_tabs, find_line, open_in_editor, parse_line_number, LineNav,
    Loader, Prompt, PromptOutcome,
};
use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::blame::{BlameLine, NOT_COMMITTED_YET};
use crate::git::file_history::is_binary;
use crate::git::Repo;
use crate::ui::diff_view::{self, DiffSource};
use crate::ui::popups::{MenuItem, MenuPopup};
use crate::ui::widgets::panel;
use crate::ui::{KeyHelp, Popup};
use crate::util::{fit, format_date, format_time, truncate};

/// Commands from the context menu that change the popup's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    ToggleSelection,
    Search,
    FindNext,
    Goto,
    NextChange,
    PrevChange,
}

/// The annotate popup for one file at one revision (or the work tree).
pub struct BlamePopup {
    path: String,
    rev: Option<String>,
    loader: Loader<Vec<BlameLine>>,
    lines: Vec<BlameLine>,
    contents: Vec<String>,
    error: Option<String>,
    oldest: i64,
    newest: i64,
    nav: LineNav,
    anchor: Option<usize>,
    prompt: Prompt,
    last_search: String,
    /// 1-based line to put the cursor on once loaded
    pending_line: Option<u32>,
    pending: Rc<Cell<Option<Cmd>>>,
}

fn load(repo: &Repo, path: &str, rev: Option<&str>) -> anyhow::Result<Vec<BlameLine>> {
    let head = match rev {
        Some(r) => repo.show_file(r, path)?,
        None => {
            use std::io::Read;
            let mut buf = Vec::new();
            std::fs::File::open(repo.path(path))?
                .take(8000)
                .read_to_end(&mut buf)?;
            buf
        }
    };
    if is_binary(&head) {
        anyhow::bail!("{path} is a binary file and cannot be annotated");
    }
    repo.blame(path, rev)
}

impl BlamePopup {
    /// Start annotating `path` at `rev` (work tree when `None`), placing the cursor at `line` when given.
    pub fn new(ctx: &Ctx, path: String, rev: Option<String>, line: Option<u32>) -> Self {
        let mut p = Self {
            path,
            rev,
            loader: Loader::idle(),
            lines: Vec::new(),
            contents: Vec::new(),
            error: None,
            oldest: 0,
            newest: 0,
            nav: LineNav::default(),
            anchor: None,
            prompt: Prompt::None,
            last_search: String::new(),
            pending_line: line,
            pending: Rc::new(Cell::new(None)),
        };
        p.reload(ctx);
        p
    }

    fn reload(&mut self, ctx: &Ctx) {
        let path = self.path.clone();
        let rev = self.rev.clone();
        self.loader = Loader::start(&ctx.repo, move |repo| load(repo, &path, rev.as_deref()));
    }

    fn poll(&mut self) {
        let Some(res) = self.loader.poll() else {
            return;
        };
        match res {
            Ok(lines) => {
                let committed = lines
                    .iter()
                    .filter(|l| !l.is_uncommitted())
                    .map(|l| l.author_time);
                self.oldest = committed.clone().min().unwrap_or(0);
                self.newest = committed.max().unwrap_or(0);
                self.contents = lines.iter().map(|l| l.content.clone()).collect();
                self.lines = lines;
                self.error = None;
                if let Some(n) = self.pending_line.take() {
                    self.nav
                        .jump(n.saturating_sub(1) as usize, self.lines.len());
                }
                self.nav.cursor = self.nav.cursor.min(self.lines.len().saturating_sub(1));
                if self.anchor.is_some_and(|a| a >= self.lines.len()) {
                    self.anchor = None;
                }
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn current(&self) -> Option<&BlameLine> {
        self.lines.get(self.nav.cursor)
    }

    fn rev_label(&self) -> String {
        match &self.rev {
            Some(r) => truncate(r, 12),
            None => "work tree".into(),
        }
    }

    /// Selected line range (0-based, inclusive).
    fn selection(&self) -> (usize, usize) {
        let c = self.nav.cursor;
        match self.anchor {
            Some(a) => (a.min(c), a.max(c)),
            None => (c, c),
        }
    }

    fn run_cmd(&mut self, cmd: Cmd) {
        let len = self.lines.len();
        match cmd {
            Cmd::ToggleSelection => {
                self.anchor = if self.anchor.is_some() {
                    None
                } else {
                    Some(self.nav.cursor)
                };
            }
            Cmd::Search => {
                self.prompt = Prompt::Search(crate::ui::widgets::TextInput::new(&self.last_search))
            }
            Cmd::Goto => self.prompt = Prompt::Goto(crate::ui::widgets::TextInput::default()),
            Cmd::FindNext => self.find(true),
            Cmd::NextChange | Cmd::PrevChange => {
                if let Some(i) = self.same_commit_run(cmd == Cmd::NextChange) {
                    self.nav.jump(i, len);
                }
            }
        }
    }

    fn find(&mut self, forward: bool) {
        if let Some(i) = find_line(&self.contents, &self.last_search, self.nav.cursor, forward) {
            self.nav.jump(i, self.lines.len());
        }
    }

    /// Start of the next/previous run of lines belonging to the cursor line's commit.
    fn same_commit_run(&self, forward: bool) -> Option<usize> {
        let commit = &self.current()?.commit;
        next_run_of(&self.lines, commit, self.nav.cursor, forward)
    }

    fn diff_action(&self) -> Action {
        let Some(l) = self.current() else {
            return Action::None;
        };
        if l.is_uncommitted() {
            return diff_view::open(
                format!("Local changes: {}", self.path),
                DiffSource::Local {
                    paths: vec![self.path.clone()],
                },
            );
        }
        let mut paths = vec![l.orig_path.clone()];
        if l.orig_path != self.path {
            paths.push(self.path.clone());
        }
        diff_view::open(
            format!("{} {} — {}", l.short(), l.orig_path, l.summary),
            DiffSource::Commit {
                rev: l.commit.clone(),
                paths,
            },
        )
    }

    /// Data needed by actions that open other popups, detached from `self`.
    fn snapshot(&self) -> BlameSnapshot {
        BlameSnapshot {
            path: self.path.clone(),
            rev: self.rev.clone(),
            line: self.current().cloned(),
            range: (!self.lines.is_empty()).then(|| {
                let (a, b) = self.selection();
                (self.lines[a].final_line, self.lines[b].final_line)
            }),
        }
    }

    fn menu(&self) -> Action {
        let cur = self.current().cloned();
        let path = self.path.clone();
        let pending = self.pending.clone();
        let set = move |c: Cmd| {
            let p = pending.clone();
            move |_: &Ctx| {
                p.set(Some(c));
                Action::None
            }
        };
        let has_line = cur.is_some();
        let committed = cur.as_ref().is_some_and(|l| !l.is_uncommitted());
        let diff = self.diff_action();
        let me = self.snapshot();
        let me2 = me.clone();
        let me3 = me.clone();
        let commit = cur.as_ref().map(|l| l.commit.clone()).unwrap_or_default();
        let history_path = path.clone();
        let items = vec![
            MenuItem::new("Show diff of this commit", "d", move |_| diff).enabled(has_line),
            MenuItem::new("Annotate previous revision", "p", move |ctx| {
                me.previous(ctx)
            })
            .enabled(has_line),
            MenuItem::new("Commit details", "c", move |ctx| {
                commit_details(ctx, &commit)
            })
            .enabled(committed),
            MenuItem::new("Show file history", "h", move |ctx| {
                super::open_file_history(ctx, &history_path)
            }),
            MenuItem::new(
                "Start / clear line selection",
                "v",
                set(Cmd::ToggleSelection),
            )
            .enabled(has_line),
            MenuItem::new("Show history for selection", "L", move |ctx| {
                me2.line_history(ctx)
            })
            .enabled(has_line),
            MenuItem::new("Find in file", "/", set(Cmd::Search)),
            MenuItem::new("Find next", "F3", set(Cmd::FindNext)),
            MenuItem::new("Go to line", "g", set(Cmd::Goto)),
            MenuItem::new("Next change of this commit", "n", set(Cmd::NextChange))
                .enabled(has_line),
            MenuItem::new("Previous change of this commit", "N", set(Cmd::PrevChange))
                .enabled(has_line),
            MenuItem::new(
                if self.rev.is_some() {
                    "Show file at this revision"
                } else {
                    "Open in editor"
                },
                "o",
                move |ctx| me3.open(ctx),
            ),
        ];
        Action::popup(MenuPopup::new(
            format!("Annotate {}", truncate(&path, 40)),
            items,
        ))
    }

    fn draw_body(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        self.nav.fit(area.height as usize, self.lines.len());
        let digits = self.lines.len().max(1).to_string().len();
        let wide = area.width >= 70;
        let (author_w, gutter_w) = if wide {
            (14usize, 8 + 1 + 14 + 1 + 10)
        } else {
            (0, 8 + 1 + 10)
        };
        let (sel_a, sel_b) = self.selection();
        let cursor_commit = self.current().map(|l| l.commit.clone());
        let mut out: Vec<Line> = Vec::with_capacity(area.height as usize);
        for i in self.nav.offset..(self.nav.offset + area.height as usize).min(self.lines.len()) {
            let l = &self.lines[i];
            let first_of_run = i == self.nav.offset || self.lines[i - 1].commit != l.commit;
            let same = cursor_commit.as_deref() == Some(l.commit.as_str());
            let color = if l.is_uncommitted() {
                t.modified
            } else {
                age_color(t, l.author_time, self.oldest, self.newest)
            };
            let mut gstyle = Style::default().fg(color);
            if same {
                gstyle = gstyle.add_modifier(Modifier::BOLD);
            }
            let gutter = if !first_of_run {
                " ".repeat(gutter_w)
            } else if l.is_uncommitted() {
                fit(NOT_COMMITTED_YET, gutter_w)
            } else if wide {
                format!(
                    "{} {} {}",
                    fit(l.short(), 8),
                    fit(&l.author, author_w),
                    fit(&format_date(l.author_time), 10)
                )
            } else {
                format!(
                    "{} {}",
                    fit(l.short(), 8),
                    fit(&format_date(l.author_time), 10)
                )
            };
            let mark = if same { "▌" } else { "│" };
            let mut spans = vec![
                Span::styled(gutter, gstyle),
                Span::styled(
                    mark,
                    Style::default().fg(if same { color } else { t.border }),
                ),
                Span::styled(format!("{:>digits$} ", l.final_line), t.dimmed()),
                Span::styled(expand_tabs(&l.content), t.text()),
            ];
            let in_sel = self.anchor.is_some() && i >= sel_a && i <= sel_b;
            if i == self.nav.cursor || in_sel {
                let st = if i == self.nav.cursor {
                    t.selected()
                } else {
                    Style::default().bg(t.selection_bg)
                };
                spans = spans.into_iter().map(|s| s.patch_style(st)).collect();
            }
            out.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(out), area);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        if self.prompt.is_active() {
            self.prompt.draw(f, area, t);
            return;
        }
        let Some(l) = self.current() else {
            return;
        };
        let pos = match self.anchor {
            Some(_) => {
                let (a, b) = self.selection();
                format!(
                    " lines {}-{} selected  {}/{} ",
                    a + 1,
                    b + 1,
                    self.nav.cursor + 1,
                    self.lines.len()
                )
            }
            None => format!(" {}/{} ", self.nav.cursor + 1, self.lines.len()),
        };
        let pw = pos.chars().count() as u16;
        let [left, right] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(pw)]).areas(area);
        let info = if l.is_uncommitted() {
            vec![Span::styled(
                NOT_COMMITTED_YET,
                Style::default().fg(t.modified),
            )]
        } else {
            vec![
                Span::styled(format!("{} ", l.short()), Style::default().fg(t.hash)),
                Span::styled(
                    format!("{} <{}> ", l.author, l.author_mail),
                    Style::default().fg(t.author),
                ),
                Span::styled(
                    format!("{} ", format_time(l.author_time)),
                    Style::default().fg(t.date),
                ),
                Span::styled(l.summary.clone(), t.text()),
            ]
        };
        f.render_widget(Line::from(info), left);
        f.render_widget(Span::styled(pos, t.dimmed()), right);
    }
}

/// Data captured for menu actions that open other popups.
#[derive(Clone)]
struct BlameSnapshot {
    path: String,
    rev: Option<String>,
    line: Option<BlameLine>,
    range: Option<(u32, u32)>,
}

impl BlameSnapshot {
    fn previous(&self, ctx: &Ctx) -> Action {
        let Some(l) = &self.line else {
            return Action::None;
        };
        let target = match (&l.previous, l.is_uncommitted()) {
            (Some((h, p)), _) => Some((h.clone(), p.clone())),
            (None, true) => ctx.repo.head_commit().map(|h| (h, self.path.clone())),
            (None, false) => None,
        };
        match target {
            Some((rev, path)) => {
                Action::popup(BlamePopup::new(ctx, path, Some(rev), Some(l.orig_line)))
            }
            None => {
                Action::Toast("No previous revision: the line was added in the first commit".into())
            }
        }
    }

    fn line_history(&self, ctx: &Ctx) -> Action {
        match self.range {
            Some((a, b)) => super::open_line_history(ctx, &self.path, a, b, self.rev.as_deref()),
            None => Action::None,
        }
    }

    fn open(&self, ctx: &Ctx) -> Action {
        match &self.rev {
            None => open_in_editor(
                ctx,
                &self.path,
                self.line.as_ref().map(|l| l.final_line as usize),
            ),
            Some(r) => super::open_file_at_revision(ctx, &self.path, r),
        }
    }
}

/// Index of the first line of the next (or previous) run of `commit` relative to `from`, wrapping.
pub fn next_run_of(lines: &[BlameLine], commit: &str, from: usize, forward: bool) -> Option<usize> {
    let n = lines.len();
    if n == 0 {
        return None;
    }
    let starts: Vec<usize> = (0..n)
        .filter(|&i| lines[i].commit == commit && (i == 0 || lines[i - 1].commit != commit))
        .collect();
    // The run containing `from` (its start) should not count as "next/previous".
    let own = starts.iter().rev().find(|&&s| s <= from).copied();
    let pick = if forward {
        starts.iter().find(|&&s| s > from).or(starts.first())
    } else {
        starts
            .iter()
            .rev()
            .find(|&&s| Some(s) != own && s < from)
            .or(starts.iter().rev().find(|&&s| Some(s) != own))
    };
    pick.copied()
        .filter(|&s| Some(s) != own || starts.len() == 1)
}

impl Popup for BlamePopup {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.poll();
        if let Some(cmd) = self.pending.take() {
            self.run_cmd(cmd);
        }
        let t = &ctx.theme;
        f.render_widget(Clear, area);
        let mut title = vec![
            Span::styled(" Annotate ", t.title()),
            Span::styled(format!("{} ", self.path), t.text()),
            Span::styled(
                format!("@ {} ", self.rev_label()),
                Style::default().fg(t.hash),
            ),
        ];
        if self.loader.is_loading() && !self.lines.is_empty() {
            title.push(Span::styled("(reloading…) ", t.dimmed()));
        }
        let block = panel(Line::from(title), true, t);
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
        if self.lines.is_empty() {
            let msg = if self.loader.is_loading() {
                "Annotating…"
            } else {
                "The file is empty"
            };
            f.render_widget(Paragraph::new(msg).style(t.dimmed()), body);
            return;
        }
        self.draw_body(f, body, ctx);
        self.draw_footer(f, footer, ctx);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let len = self.lines.len();
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
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc if self.anchor.is_some() => self.anchor = None,
            KeyCode::Esc | KeyCode::Char('q') => return Action::ClosePopup,
            KeyCode::Enter | KeyCode::Char('d') => return self.diff_action(),
            KeyCode::Char('p') => return self.snapshot().previous(ctx),
            KeyCode::Char('c') => {
                return self
                    .current()
                    .map(|l| commit_details(ctx, &l.commit))
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('h') => return super::open_file_history(ctx, &self.path),
            KeyCode::Char('L') => return self.snapshot().line_history(ctx),
            KeyCode::Char('o') => return self.snapshot().open(ctx),
            KeyCode::Char('v') => self.run_cmd(Cmd::ToggleSelection),
            KeyCode::Char('/') => self.run_cmd(Cmd::Search),
            KeyCode::Char('g') => self.run_cmd(Cmd::Goto),
            KeyCode::F(3) => self.find(!shift),
            KeyCode::Char('n') => self.run_cmd(Cmd::NextChange),
            KeyCode::Char('N') => self.run_cmd(Cmd::PrevChange),
            KeyCode::Char('r') => self.reload(ctx),
            KeyCode::Char('.') | KeyCode::Char('m') => return self.menu(),
            _ => {}
        }
        Action::None
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if self.rev.is_none() {
            self.reload(ctx);
        }
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            ("↑↓/jk", "line"),
            ("Enter", "commit diff"),
            ("p", "annotate previous"),
            ("c", "commit details"),
            ("h", "file history"),
            ("v", "select lines"),
            ("L", "history for selection"),
            ("/", "find"),
            ("F3", "find next"),
            ("g", "go to line"),
            ("n/N", "next/prev same commit"),
            ("o", "open"),
            ("r", "reload"),
            (".", "menu"),
            ("Esc", "close"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(commit: &str) -> BlameLine {
        BlameLine {
            commit: commit.to_string(),
            orig_line: 1,
            final_line: 1,
            author: String::new(),
            author_mail: String::new(),
            author_time: 0,
            summary: String::new(),
            orig_path: String::new(),
            previous: None,
            content: String::new(),
        }
    }

    #[test]
    fn runs_of_same_commit() {
        let lines: Vec<BlameLine> = ["a", "a", "b", "a", "c", "a", "a"]
            .iter()
            .map(|c| line(c))
            .collect();
        assert_eq!(next_run_of(&lines, "a", 0, true), Some(3));
        assert_eq!(next_run_of(&lines, "a", 1, true), Some(3));
        assert_eq!(next_run_of(&lines, "a", 3, true), Some(5));
        assert_eq!(next_run_of(&lines, "a", 6, true), Some(0));
        assert_eq!(next_run_of(&lines, "a", 3, false), Some(0));
        assert_eq!(next_run_of(&lines, "a", 0, false), Some(5));
        assert_eq!(next_run_of(&lines, "b", 2, true), Some(2));
        assert_eq!(next_run_of(&lines, "zz", 2, true), None);
        assert_eq!(next_run_of(&[], "a", 0, true), None);
    }
}
