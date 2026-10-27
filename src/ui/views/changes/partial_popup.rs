//! Fullscreen line-level staging for one file: pick lines (or whole hunks) and stage, unstage or
//! discard just those.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::diff::{FileDiff, LineKind};
use crate::git::partial::{self, Direction, Selection};
use crate::ui::popups::ConfirmPopup;
use crate::ui::widgets::{move_sel, panel, scroll_to_keep_visible};
use crate::ui::{KeyHelp, Popup};

/// One display row: a hunk header (`line == None`) or a diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowRef {
    hunk: usize,
    line: Option<usize>,
}

/// Flatten a diff into rows.
fn rows_of(fd: &FileDiff) -> Vec<RowRef> {
    let mut v = Vec::new();
    for (h, hk) in fd.hunks.iter().enumerate() {
        v.push(RowRef {
            hunk: h,
            line: None,
        });
        v.extend((0..hk.lines.len()).map(|l| RowRef {
            hunk: h,
            line: Some(l),
        }));
    }
    v
}

/// Toggle every changed line of hunk `h`: select all unless all are already selected.
fn toggle_hunk(fd: &FileDiff, sel: &mut Selection, h: usize) {
    let lines = partial::hunk_changes(fd, h);
    if lines.iter().all(|l| sel.contains(l)) {
        for l in lines {
            sel.remove(&l);
        }
    } else {
        sel.extend(lines);
    }
}

pub struct PartialPopup {
    path: String,
    staged: bool,
    untracked: bool,
    diff: Option<FileDiff>,
    rows: Vec<RowRef>,
    cursor: usize,
    offset: usize,
    height: usize,
    selected: Selection,
    error: Option<String>,
}

impl PartialPopup {
    /// Open for `path`, showing the staged diff when `staged`, else the unstaged one.
    pub fn new(ctx: &Ctx, path: &str, staged: bool) -> Self {
        let mut p = Self {
            path: path.to_string(),
            staged,
            untracked: false,
            diff: None,
            rows: Vec::new(),
            cursor: 0,
            offset: 0,
            height: 0,
            selected: Selection::new(),
            error: None,
        };
        p.load(ctx);
        if p.cursor_on_header() {
            p.next_change(1);
        }
        p
    }

    fn load(&mut self, ctx: &Ctx) {
        let repo = &ctx.repo;
        self.untracked = repo
            .run(&["--literal-pathspecs", "ls-files", "--", &self.path])
            .map(|s| s.trim().is_empty())
            .unwrap_or(false);
        let res = if self.staged && self.untracked {
            Ok(None)
        } else {
            partial::load_diff(
                repo,
                &self.path,
                self.staged,
                self.untracked && repo.path(&self.path).exists(),
            )
        };
        match res {
            Ok(d) => {
                if d != self.diff {
                    self.selected.clear();
                }
                self.rows = d.as_ref().map(rows_of).unwrap_or_default();
                self.diff = d;
                self.error = None;
            }
            Err(e) => {
                self.diff = None;
                self.rows.clear();
                self.selected.clear();
                self.error = Some(format!("{e:#}"));
            }
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn cursor_on_header(&self) -> bool {
        self.rows.get(self.cursor).is_some_and(|r| r.line.is_none())
    }

    fn is_change_row(&self, i: usize) -> bool {
        match (self.rows.get(i), &self.diff) {
            (
                Some(RowRef {
                    hunk,
                    line: Some(l),
                }),
                Some(fd),
            ) => partial::is_change(fd, *hunk, *l),
            _ => false,
        }
    }

    /// Move to the next/previous changed line.
    fn next_change(&mut self, dir: isize) {
        let mut i = self.cursor as isize + dir;
        while i >= 0 && (i as usize) < self.rows.len() {
            if self.is_change_row(i as usize) {
                self.cursor = i as usize;
                return;
            }
            i += dir;
        }
    }

    fn jump_hunk(&mut self, dir: isize) {
        let cur = self.rows.get(self.cursor).map(|r| r.hunk).unwrap_or(0) as isize;
        let target = cur + dir;
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| r.hunk as isize == target && r.line.is_none())
        {
            self.cursor = i;
            self.next_change(1);
        }
    }

    fn toggle_current(&mut self) {
        let (Some(r), Some(fd)) = (self.rows.get(self.cursor).copied(), &self.diff) else {
            return;
        };
        match r.line {
            None => toggle_hunk(fd, &mut self.selected, r.hunk),
            Some(l) if partial::is_change(fd, r.hunk, l) => {
                if !self.selected.remove(&(r.hunk, l)) {
                    self.selected.insert((r.hunk, l));
                }
                self.next_change(1);
            }
            Some(_) => {}
        }
    }

    fn toggle_current_hunk(&mut self) {
        if let (Some(r), Some(fd)) = (self.rows.get(self.cursor), &self.diff) {
            toggle_hunk(fd, &mut self.selected, r.hunk);
        }
    }

    fn toggle_all(&mut self) {
        if let Some(fd) = &self.diff {
            let all = partial::all_changes(fd);
            if all.iter().all(|l| self.selected.contains(l)) {
                self.selected.clear();
            } else {
                self.selected = all;
            }
        }
    }

    /// The explicit selection, or the line / hunk under the cursor when nothing is selected.
    fn effective_selection(&self) -> Selection {
        if !self.selected.is_empty() {
            return self.selected.clone();
        }
        let (Some(r), Some(fd)) = (self.rows.get(self.cursor), &self.diff) else {
            return Selection::new();
        };
        match r.line {
            Some(l) if partial::is_change(fd, r.hunk, l) => [(r.hunk, l)].into_iter().collect(),
            Some(_) => Selection::new(),
            None => partial::hunk_changes(fd, r.hunk).into_iter().collect(),
        }
    }

    fn switch_side(&mut self, ctx: &Ctx) {
        self.staged = !self.staged;
        self.selected.clear();
        self.cursor = 0;
        self.offset = 0;
        self.load(ctx);
        self.next_change(1);
    }

    fn run(&mut self, ctx: &Ctx, dir: Direction) -> Action {
        if dir.staged() != self.staged {
            let side = if dir.staged() { "staged" } else { "unstaged" };
            return Action::Toast(format!("Switch to the {side} diff with 'd' first"));
        }
        let Some(fd) = self.diff.clone() else {
            return Action::Toast("Nothing to apply".into());
        };
        let sel = self.effective_selection();
        if sel.is_empty() {
            return Action::Toast("Select lines with Space (H selects a hunk)".into());
        }
        let n = sel.len();
        let untracked = self.untracked;
        if dir == Direction::Discard {
            let msg = format!(
                "Discard {n} selected line{} of {} from the work tree?\nThis cannot be undone.",
                if n == 1 { "" } else { "s" },
                self.path
            );
            return Action::popup(
                ConfirmPopup::new("Discard lines", msg, move |ctx| {
                    Action::from_result(
                        partial::apply_lines(&ctx.repo, &fd, &sel, Direction::Discard, untracked),
                        format!("Discarded {n} line(s)"),
                    )
                })
                .danger(),
            );
        }
        match partial::apply_lines(&ctx.repo, &fd, &sel, dir, untracked) {
            Ok(()) => {
                self.selected.clear();
                self.load(ctx);
                if !self.is_change_row(self.cursor) {
                    self.next_change(1);
                    self.next_change(-1);
                }
                Action::Batch(vec![
                    Action::Toast(format!("{} {n} line(s)", dir.verb())),
                    Action::Refresh,
                ])
            }
            Err(e) => Action::Error(format!("{e:#}")),
        }
    }

    fn line_for(&self, r: RowRef, fd: &FileDiff, cursor: bool, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let hk = &fd.hunks[r.hunk];
        let mut line = match r.line {
            None => {
                let changes = partial::hunk_changes(fd, r.hunk);
                let n = changes.iter().filter(|l| self.selected.contains(l)).count();
                let mark = match n {
                    0 => "[ ]",
                    _ if n == changes.len() => "[■]",
                    _ => "[▪]",
                };
                Line::from(vec![
                    Span::styled(format!("{mark} "), t.key()),
                    Span::styled(hk.header.clone(), Style::default().fg(t.hunk)),
                ])
            }
            Some(li) => {
                let l = &hk.lines[li];
                let selectable = matches!(l.kind, LineKind::Added | LineKind::Removed);
                let mark = if !selectable {
                    "    "
                } else if self.selected.contains(&(r.hunk, li)) {
                    "[x] "
                } else {
                    "[ ] "
                };
                let num = |n: Option<u32>| {
                    n.map(|n| format!("{n:>5}"))
                        .unwrap_or_else(|| "     ".into())
                };
                let (sign, style) = match l.kind {
                    LineKind::Added => ("+", Style::default().fg(t.added).bg(t.added_bg)),
                    LineKind::Removed => ("-", Style::default().fg(t.removed).bg(t.removed_bg)),
                    LineKind::Context => (" ", t.text()),
                    LineKind::NoNewline => ("", t.dimmed()),
                };
                let content = l.content.trim_end_matches('\r').replace('\t', "    ");
                Line::from(vec![
                    Span::styled(mark, t.key()),
                    Span::styled(format!("{} {} ", num(l.old_no), num(l.new_no)), t.dimmed()),
                    Span::styled(format!("{sign}{content}"), style),
                ])
            }
        };
        if cursor {
            line = line.patch_style(t.selected());
        }
        line
    }
}

impl Popup for PartialPopup {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        f.render_widget(Clear, area);
        let side = if self.staged {
            "staged changes (index → HEAD)"
        } else {
            "unstaged changes (work tree → index)"
        };
        let block = panel(format!(" Partial: {} — {side} ", self.path), true, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [body, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
        self.height = body.height as usize;
        let message = match (&self.error, &self.diff) {
            (Some(e), _) => Some((e.clone(), Style::default().fg(t.error))),
            (None, None) => Some((
                format!(
                    "No {} changes in this file. Press d to see the {} diff.",
                    if self.staged { "staged" } else { "unstaged" },
                    if self.staged { "unstaged" } else { "staged" }
                ),
                t.dimmed(),
            )),
            (None, Some(fd)) if fd.binary => Some((
                "Binary file — stage or unstage the whole file instead.".into(),
                t.dimmed(),
            )),
            _ => None,
        };
        if let Some((m, st)) = message {
            f.render_widget(Paragraph::new(m).style(st), body);
        } else if let Some(fd) = &self.diff {
            self.offset = scroll_to_keep_visible(self.cursor, self.offset, self.height)
                .min(self.rows.len().saturating_sub(self.height));
            let lines: Vec<Line> = self
                .rows
                .iter()
                .enumerate()
                .skip(self.offset)
                .take(self.height)
                .map(|(i, r)| self.line_for(*r, fd, i == self.cursor, ctx))
                .collect();
            f.render_widget(Paragraph::new(lines), body);
        }
        let hints: &[(&str, &str)] = if self.staged {
            &[
                ("Space", "select"),
                ("H", "hunk"),
                ("a", "all"),
                ("u", "unstage"),
                ("d", "unstaged diff"),
                ("Esc", "close"),
            ]
        } else {
            &[
                ("Space", "select"),
                ("H", "hunk"),
                ("a", "all"),
                ("s", "stage"),
                ("x", "discard"),
                ("d", "staged diff"),
                ("Esc", "close"),
            ]
        };
        let mut spans = vec![Span::styled(
            format!("{} selected  ", self.selected.len()),
            t.title(),
        )];
        for (k, d) in hints {
            spans.push(Span::styled(k.to_string(), t.key()));
            spans.push(Span::styled(format!(" {d}  "), t.dimmed()));
        }
        f.render_widget(Line::from(spans), status);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let page = self.height.max(1) as isize;
        let len = self.rows.len();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::ClosePopup,
            KeyCode::Up | KeyCode::Char('k') => self.cursor = move_sel(self.cursor, -1, len),
            KeyCode::Down | KeyCode::Char('j') => self.cursor = move_sel(self.cursor, 1, len),
            KeyCode::Char('u') if ctrl => self.cursor = move_sel(self.cursor, -page / 2, len),
            KeyCode::Char('d') if ctrl => self.cursor = move_sel(self.cursor, page / 2, len),
            KeyCode::PageUp => self.cursor = move_sel(self.cursor, -page, len),
            KeyCode::PageDown => self.cursor = move_sel(self.cursor, page, len),
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => self.cursor = len.saturating_sub(1),
            KeyCode::Char('n') | KeyCode::Char(']') => self.jump_hunk(1),
            KeyCode::Char('N') | KeyCode::Char('[') => self.jump_hunk(-1),
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_current(),
            KeyCode::Char('H') => self.toggle_current_hunk(),
            KeyCode::Char('a') => self.toggle_all(),
            KeyCode::Char('c') => self.selected.clear(),
            KeyCode::Char('d') | KeyCode::Tab => self.switch_side(ctx),
            KeyCode::Char('s') => return self.run(ctx, Direction::Stage),
            KeyCode::Char('u') => return self.run(ctx, Direction::Unstage),
            KeyCode::Char('x') => return self.run(ctx, Direction::Discard),
            KeyCode::Char('r') => self.load(ctx),
            _ => {}
        }
        Action::None
    }

    fn refresh(&mut self, ctx: &Ctx) {
        self.load(ctx);
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            ("j/k", "move"),
            ("Space", "select line"),
            ("H", "select hunk"),
            ("a", "select all"),
            ("s", "stage"),
            ("u", "unstage"),
            ("x", "discard"),
            ("d", "staged/unstaged"),
            ("n/N", "next/prev hunk"),
            ("Esc", "close"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;

    fn press(p: &mut PartialPopup, ctx: &Ctx, c: char) -> Action {
        p.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), ctx)
    }

    #[test]
    fn select_and_stage_lines_through_keys() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "init");
        write(&repo, "f.txt", "a\nB\nc\nd\n");
        let ctx = Ctx::new(repo, Config::default());
        let mut p = PartialPopup::new(&ctx, "f.txt", false);
        // Cursor starts on the first change ("-b"); select it and the "+B" after it.
        press(&mut p, &ctx, ' ');
        press(&mut p, &ctx, ' ');
        assert_eq!(p.selected.len(), 2);
        press(&mut p, &ctx, 's');
        assert_eq!(ctx.repo.run(&["show", ":f.txt"]).unwrap(), "a\nB\nc\n");
        // Remaining "+d" is still unstaged; whole hunk via H then discard is confirmed separately.
        press(&mut p, &ctx, 'H');
        assert_eq!(p.selected.len(), 1);
        assert!(matches!(press(&mut p, &ctx, 'x'), Action::Popup(_)));
        // Switch to the staged side and unstage everything.
        press(&mut p, &ctx, 'd');
        assert!(p.staged);
        press(&mut p, &ctx, 'a');
        press(&mut p, &ctx, 'u');
        assert_eq!(ctx.repo.run(&["show", ":f.txt"]).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn hunk_toggle_is_symmetric() {
        let fd = crate::git::diff::parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n",
        )
        .pop()
        .unwrap();
        let mut sel = Selection::new();
        toggle_hunk(&fd, &mut sel, 0);
        assert_eq!(sel.len(), 2);
        toggle_hunk(&fd, &mut sel, 0);
        assert!(sel.is_empty());
        assert_eq!(rows_of(&fd).len(), 4);
    }
}
