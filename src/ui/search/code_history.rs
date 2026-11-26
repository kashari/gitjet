//! "Code history" (`;;`): type a piece of code and see which commits added, removed or changed
//! lines containing it (`git log -G`), with just those hunks, and where it is in the code now
//! (`git grep`). From there, `Ctrl+B` jumps to the declaration (class, method...) in the editor.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{frame, keys_line, GoTo, List, SETTLE};
use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::diff::FileDiff;
use crate::git::log::Commit;
use crate::git::search::{self, GrepOptions, Hit};
use crate::ui::bg::Preview;
use crate::ui::diff_view::{self, DiffSource};
use crate::ui::widgets::{centered_pct, TextInput};
use crate::ui::{KeyHelp, Popup};
use crate::util::{relative_time, truncate};

/// Commits listed at most.
const MAX_COMMITS: usize = 300;
/// Places in the current code listed at most.
const MAX_PLACES: usize = 500;

type Query = (String, GrepOptions);
type Commits = Result<Vec<Commit>, String>;
type Places = Result<(Vec<Hit>, bool), String>;
type Changes = Result<Vec<FileDiff>, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    /// Commits that touched the code
    History,
    /// Where the code is now
    Now,
}

pub struct CodeHistory {
    input: TextInput,
    opts: GrepOptions,
    pane: Pane,
    commits: Preview<Query, Commits>,
    places: Preview<Query, Places>,
    changes: Preview<(String, Query), Changes>,
    history_list: List,
    now_list: List,
    list_height: usize,
    diff_scroll: usize,
    diff_height: usize,
    /// Commit the diff scroll belongs to
    diff_for: String,
    /// Why the last `Ctrl+B` found nothing
    message: Option<String>,
}

impl Default for CodeHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// True for a name a declaration can have (`UserService`, `login_user`).
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

impl CodeHistory {
    pub fn new() -> Self {
        Self {
            input: TextInput::new(""),
            opts: GrepOptions::default(),
            pane: Pane::History,
            commits: Preview::new(SETTLE, 16),
            places: Preview::new(SETTLE, 16),
            changes: Preview::new(std::time::Duration::from_millis(60), 32),
            history_list: List::default(),
            now_list: List::default(),
            list_height: 10,
            diff_scroll: 0,
            diff_height: 10,
            diff_for: String::new(),
            message: None,
        }
    }

    fn query(&self) -> Option<Query> {
        let q = self.input.value();
        (q.trim().chars().count() >= 2).then_some((q, self.opts))
    }

    /// The commits for the current query, asking for them when they are not there yet.
    fn commits(&mut self, ctx: &Ctx) -> Option<Commits> {
        let q = self.query()?;
        let found = self.commits.get(&q);
        if found.is_none() {
            let (text, opts) = q.clone();
            self.commits.request(&ctx.repo, q, move |repo| {
                search::code_history(repo, &text, opts, MAX_COMMITS).map_err(|e| format!("{e:#}"))
            });
        }
        found
    }

    fn places(&mut self, ctx: &Ctx) -> Option<Places> {
        let q = self.query()?;
        let found = self.places.get(&q);
        if found.is_none() {
            let (text, opts) = q.clone();
            self.places.request(&ctx.repo, q, move |repo| {
                search::grep(repo, &text, opts, MAX_PLACES).map_err(|e| format!("{e:#}"))
            });
        }
        found
    }

    fn selected_commit(&mut self, ctx: &Ctx) -> Option<Commit> {
        match self.commits(ctx)? {
            Ok(c) => c.get(self.history_list.sel).cloned(),
            Err(_) => None,
        }
    }

    fn place_hits(&mut self, ctx: &Ctx) -> Vec<Hit> {
        match self.places(ctx) {
            Some(Ok((hits, _))) => hits,
            _ => Vec::new(),
        }
    }

    /// The hunks of the selected commit that touch the code.
    fn changes(&mut self, ctx: &Ctx, id: &str) -> Option<Changes> {
        let q = self.query()?;
        let key = (id.to_string(), q.clone());
        let found = self.changes.get(&key);
        if found.is_none() {
            let id = id.to_string();
            self.changes.request(&ctx.repo, key, move |repo| {
                let (text, opts) = q;
                let m = search::code_matcher(&text, opts).map_err(|e| format!("{e:#}"))?;
                search::code_changes(repo, &id, &m).map_err(|e| format!("{e:#}"))
            });
        }
        found
    }

    /// `Ctrl+B`: in the history, the declaration of the name typed (or, when there is none, of
    /// the method or class around its first place in the code); in "now", the method or class
    /// around the selected place.
    fn go_to_declaration(&mut self, ctx: &Ctx) -> Action {
        self.message = None;
        let text = self.input.value().trim().to_string();
        if self.pane == Pane::History && is_identifier(&text) {
            match search::declarations_of(&ctx.repo, &text) {
                Ok(found) if found.len() == 1 => {
                    return Action::edit(ctx.repo.path(&found[0].path), Some(found[0].line));
                }
                Ok(found) if found.len() > 1 => {
                    return Action::popup(GoTo::with_query(ctx, &text));
                }
                _ => {}
            }
        }
        let hits = self.place_hits(ctx);
        let hit = match self.pane {
            Pane::Now => hits.get(self.now_list.sel),
            Pane::History => hits.first(),
        };
        if let Some(hit) = hit {
            let lines: Vec<String> = std::fs::read(ctx.repo.path(&hit.path))
                .map(|b| {
                    String::from_utf8_lossy(&b)
                        .lines()
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(line) = search::enclosing_declaration(&lines, hit.line) {
                return Action::edit(ctx.repo.path(&hit.path), Some(line));
            }
        }
        self.message = Some(if text.is_empty() {
            "Type the code or a name first".into()
        } else {
            format!("No declaration found for \"{text}\"")
        });
        Action::None
    }

    fn status(&mut self, ctx: &Ctx) -> (String, bool) {
        if let Some(m) = &self.message {
            return (m.clone(), true);
        }
        if self.query().is_none() {
            return (
                "Type at least 2 characters of code: a name, a call, a string…".into(),
                false,
            );
        }
        let commits = match self.commits(ctx) {
            None => "searching the history…".to_string(),
            Some(Err(e)) => return (e.lines().next().unwrap_or("").to_string(), true),
            Some(Ok(c)) if c.is_empty() => "no commit changed it".into(),
            Some(Ok(c)) if c.len() >= MAX_COMMITS => {
                format!("the last {} commits that changed it", c.len())
            }
            Some(Ok(c)) if c.len() == 1 => "1 commit changed it".into(),
            Some(Ok(c)) => format!("{} commits changed it", c.len()),
        };
        let now = match self.places(ctx) {
            None => "searching the code…".to_string(),
            Some(Err(e)) => return (e.lines().next().unwrap_or("").to_string(), true),
            Some(Ok((h, _))) if h.is_empty() => "not in the code any more".into(),
            Some(Ok((h, more))) => {
                let files = h
                    .iter()
                    .map(|h| h.path.as_str())
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                format!(
                    "{}{} place{} in {files} file{} now",
                    h.len(),
                    if more { "+" } else { "" },
                    if h.len() == 1 { "" } else { "s" },
                    if files == 1 { "" } else { "s" }
                )
            }
        };
        (format!("{commits} · {now}"), false)
    }

    fn draw_tabs(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let tab = |on: bool, label: &'static str| {
            Span::styled(
                format!(" {label} "),
                if on {
                    Style::default()
                        .fg(t.accent)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    t.dimmed()
                },
            )
        };
        let flag = |on: bool, label: &'static str| tab(on, label);
        let line = Line::from(vec![
            tab(self.pane == Pane::History, "History"),
            Span::raw(" "),
            tab(self.pane == Pane::Now, "In the code now"),
            Span::styled("   Tab switches", t.dimmed()),
        ]);
        let flags = Line::from(vec![
            flag(self.opts.case_sensitive, "Aa"),
            Span::raw(" "),
            flag(self.opts.whole_word, "W"),
            Span::raw(" "),
            flag(self.opts.regex, ".*"),
        ]);
        let [left, right] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(13)]).areas(area);
        f.render_widget(line, left);
        f.render_widget(flags, right);
    }

    fn draw_history(&mut self, f: &mut Frame, list: Rect, preview: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let commits = match self.commits(ctx) {
            Some(Ok(c)) => c,
            _ => Vec::new(),
        };
        self.history_list.fit(commits.len(), self.list_height);
        let width = list.width as usize;
        let rows: Vec<Line> = commits
            .iter()
            .enumerate()
            .skip(self.history_list.offset)
            .take(self.list_height)
            .map(|(i, c)| {
                let mut line = Line::from(vec![
                    Span::styled(format!("{} ", c.short), Style::default().fg(t.hash)),
                    Span::styled(
                        format!("{:<14} ", truncate(&relative_time(c.time), 14)),
                        Style::default().fg(t.date),
                    ),
                    Span::styled(
                        format!("{:<16} ", truncate(&c.author, 16)),
                        Style::default().fg(t.author),
                    ),
                    Span::styled(
                        truncate(&c.subject, width.saturating_sub(c.short.len() + 33)),
                        t.text(),
                    ),
                ]);
                if i == self.history_list.sel {
                    line = line.style(t.selected());
                }
                line
            })
            .collect();
        f.render_widget(Paragraph::new(rows), list);

        let Some(c) = commits.get(self.history_list.sel).cloned() else {
            return;
        };
        if self.diff_for != c.id {
            self.diff_for = c.id.clone();
            self.diff_scroll = 0;
        }
        let changes = self.changes(ctx, &c.id);
        let counts = match (&changes, self.query()) {
            (Some(Ok(d)), Some((text, opts))) => search::code_matcher(&text, opts)
                .ok()
                .map(|m| search::count_changed(d, &m)),
            _ => None,
        };
        let what = match counts {
            Some((0, 0)) | None => String::new(),
            Some((a, 0)) => format!("added {a} line{}", if a == 1 { "" } else { "s" }),
            Some((0, r)) => format!("removed {r} line{}", if r == 1 { "" } else { "s" }),
            Some((a, r)) => format!("+{a} −{r} lines"),
        };
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(t.border(false))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", c.short), Style::default().fg(t.hash)),
                Span::styled(format!("{} ", truncate(&c.subject, 60)), t.text()),
                Span::styled(
                    format!(
                        "· {} · {} {what} ",
                        c.author,
                        crate::util::format_date(c.time)
                    ),
                    t.dimmed(),
                ),
            ]));
        let inner = block.inner(preview);
        f.render_widget(block, preview);
        self.diff_height = inner.height as usize;
        match changes {
            None => f.render_widget(Paragraph::new("Loading…").style(t.dimmed()), inner),
            Some(Err(e)) => {
                f.render_widget(Paragraph::new(e).style(Style::default().fg(t.error)), inner)
            }
            Some(Ok(d)) if d.is_empty() => f.render_widget(
                Paragraph::new("(only in a merge or a rename: Enter shows the whole commit)")
                    .style(t.dimmed()),
                inner,
            ),
            Some(Ok(d)) => {
                let max = diff_view::diff_height(&d).saturating_sub(self.diff_height);
                self.diff_scroll = self.diff_scroll.min(max);
                diff_view::draw_diff(f, inner, &d, self.diff_scroll, ctx);
            }
        }
    }

    fn draw_now(&mut self, f: &mut Frame, list: Rect, preview: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let hits = self.place_hits(ctx);
        self.now_list.fit(hits.len(), self.list_height);
        let width = list.width as usize;
        let rows: Vec<Line> = hits
            .iter()
            .enumerate()
            .skip(self.now_list.offset)
            .take(self.list_height)
            .map(|(i, h)| {
                let place = truncate(&format!("{}:{}", h.path, h.line), width / 2);
                let mut line = Line::from(vec![
                    Span::styled(format!("{place}  "), t.dimmed()),
                    Span::styled(h.text.trim().to_string(), t.text()),
                ]);
                if i == self.now_list.sel {
                    line = line.style(t.selected());
                }
                line
            })
            .collect();
        f.render_widget(Paragraph::new(rows), list);
        let Some(hit) = hits.get(self.now_list.sel) else {
            return;
        };
        let lines: Vec<String> = std::fs::read(ctx.repo.path(&hit.path))
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .lines()
                    .map(|l| l.replace('\t', "    "))
                    .collect()
            })
            .unwrap_or_default();
        let decl = search::enclosing_declaration(&lines, hit.line);
        let title = match decl.and_then(|d| lines.get(d - 1)) {
            Some(d) => format!(" {}:{}  in {} ", hit.path, hit.line, truncate(d.trim(), 60)),
            None => format!(" {}:{} ", hit.path, hit.line),
        };
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(t.border(false))
            .title(Span::styled(title, t.dimmed()));
        let inner = block.inner(preview);
        f.render_widget(block, preview);
        let h = inner.height as usize;
        let first = hit.line.saturating_sub(1).saturating_sub(h / 2);
        let w = lines.len().to_string().len();
        let text: Vec<Line> = lines
            .iter()
            .enumerate()
            .skip(first)
            .take(h)
            .map(|(i, l)| {
                let style = if i + 1 == hit.line {
                    t.text().bg(t.selection_bg)
                } else if Some(i + 1) == decl {
                    Style::default().fg(t.accent)
                } else {
                    t.text()
                };
                Line::from(vec![
                    Span::styled(format!("{:>w$} ", i + 1), t.dimmed()),
                    Span::styled(l.clone(), style),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(text), inner);
    }

    fn open(&mut self, ctx: &Ctx) -> Action {
        match self.pane {
            Pane::History => match self.selected_commit(ctx) {
                Some(c) => diff_view::open(
                    format!("{} {}", c.short, c.subject),
                    DiffSource::Commit {
                        rev: c.id,
                        paths: Vec::new(),
                    },
                ),
                None => Action::None,
            },
            Pane::Now => match self.place_hits(ctx).get(self.now_list.sel) {
                Some(h) => Action::edit(ctx.repo.path(&h.path), Some(h.line)),
                None => Action::None,
            },
        }
    }
}

impl Popup for CodeHistory {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let r = centered_pct(area, 94, 92);
        f.render_widget(Clear, r);
        let block = frame("Code history", t);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [field, status, tabs, list, preview, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Percentage(35),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(inner);
        self.input.render(f, field, t.text(), true);
        let (text, error) = self.status(ctx);
        let style = if error {
            Style::default().fg(t.error)
        } else {
            t.dimmed()
        };
        f.render_widget(Paragraph::new(text).style(style), status);
        self.draw_tabs(f, tabs, ctx);
        self.list_height = (list.height as usize).max(1);
        match self.pane {
            Pane::History => self.draw_history(f, list, preview, ctx),
            Pane::Now => self.draw_now(f, list, preview, ctx),
        }
        let enter = match self.pane {
            Pane::History => "whole commit",
            Pane::Now => "open",
        };
        f.render_widget(
            keys_line(
                &[
                    ("Enter", enter),
                    ("Ctrl+B", "declaration"),
                    ("Tab", "history/now"),
                    ("Shift+↑↓", "scroll"),
                    ("Alt+C/W/R", "options"),
                    ("Esc", "close"),
                ],
                t,
            ),
            keys,
        );
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if !matches!(key.code, KeyCode::Char('b') if ctrl) && key.code != KeyCode::F(12) {
            self.message = None;
        }
        match key.code {
            KeyCode::Esc => return Action::ClosePopup,
            KeyCode::Enter => return self.open(ctx),
            KeyCode::Char('b') if ctrl => return self.go_to_declaration(ctx),
            KeyCode::F(12) => return self.go_to_declaration(ctx),
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = match self.pane {
                    Pane::History => Pane::Now,
                    Pane::Now => Pane::History,
                }
            }
            KeyCode::Char('c') if alt => self.opts.case_sensitive = !self.opts.case_sensitive,
            KeyCode::Char('w') if alt => self.opts.whole_word = !self.opts.whole_word,
            KeyCode::Char('r') if alt => self.opts.regex = !self.opts.regex,
            KeyCode::Up if shift => self.diff_scroll = self.diff_scroll.saturating_sub(3),
            KeyCode::Down if shift => self.diff_scroll += 3,
            _ => {
                let (list, len) = match self.pane {
                    Pane::History => {
                        let n = match self.commits(ctx) {
                            Some(Ok(c)) => c.len(),
                            _ => 0,
                        };
                        (&mut self.history_list, n)
                    }
                    Pane::Now => {
                        let n = self.place_hits(ctx).len();
                        (&mut self.now_list, n)
                    }
                };
                if !list.key(&key, len, self.list_height) {
                    let before = self.input.value();
                    self.input.handle_key(&key);
                    if self.input.value() != before {
                        self.history_list = List::default();
                        self.now_list = List::default();
                    }
                }
            }
        }
        Action::None
    }

    fn poll(&mut self, ctx: &Ctx) -> Action {
        // Keep the loads going while the dialog is open.
        let _ = self.commits(ctx);
        let _ = self.places(ctx);
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            (
                "Tab",
                "switch between the history and the places in the code now",
            ),
            (
                "Enter",
                "show the whole commit / open the place in the editor",
            ),
            (
                "Ctrl+B or F12",
                "go to the declaration: of the name typed, or the method/class around the place",
            ),
            ("Shift+↑ Shift+↓", "scroll the changes"),
            ("Alt+C", "match case"),
            ("Alt+W", "whole words"),
            ("Alt+R", "regular expression"),
            ("Esc", "close"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(p: &mut dyn Popup, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        // Twice: the first frame asks for the loads, which finish at once in tests.
        for _ in 0..2 {
            term.draw(|f| p.draw(f, f.area(), ctx)).unwrap();
        }
        let buf = term.backend().buffer().clone();
        (0..40)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn type_text(p: &mut dyn Popup, ctx: &Ctx, text: &str) {
        for c in text.chars() {
            p.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE), ctx);
        }
    }

    fn edited(a: &Action) -> Option<(String, Option<usize>)> {
        match a {
            Action::Edit { path, line } => Some((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                *line,
            )),
            _ => None,
        }
    }

    fn setup() -> (TempDir, Ctx) {
        let (d, repo) = temp_repo();
        commit_files(
            &repo,
            &[(
                "src/Auth.java",
                "public class Auth {\n    void login() {\n        check(1);\n    }\n}\n",
            )],
            "add login",
        );
        commit_files(&repo, &[("README.md", "# demo\n")], "readme");
        commit_files(
            &repo,
            &[(
                "src/Auth.java",
                "public class Auth {\n    void login() {\n        check(2);\n    }\n}\n",
            )],
            "check twice",
        );
        (d, Ctx::new(repo, Config::default()))
    }

    #[test]
    fn lists_the_commits_that_changed_the_code_with_their_hunks() {
        let (_d, ctx) = setup();
        let mut p = CodeHistory::new();
        type_text(&mut p, &ctx, "check(");
        let s = screen(&mut p, &ctx);
        assert!(
            s.contains("2 commits changed it · 1 place in 1 file now"),
            "{s}"
        );
        assert!(s.contains("check twice"), "{s}");
        assert!(s.contains("add login"), "{s}");
        assert!(!s.contains("readme"), "{s}");
        // The newest commit's hunk, with what it did.
        assert!(s.contains("+1 −1 lines"), "{s}");
        assert!(s.contains("check(2);"), "{s}");
        p.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &ctx);
        let s = screen(&mut p, &ctx);
        assert!(s.contains("added 1 line"), "{s}");

        // Where it is now: the place, and the method around it.
        p.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &ctx);
        let s = screen(&mut p, &ctx);
        assert!(s.contains("src/Auth.java:3  check(2);"), "{s}");
        assert!(s.contains("in void login() {"), "{s}");
        let a = p.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &ctx);
        assert_eq!(edited(&a), Some(("Auth.java".into(), Some(3))));
        // Ctrl+B: the method around the place.
        let a = p.handle_key(key(KeyCode::Char('b'), KeyModifiers::CONTROL), &ctx);
        assert_eq!(edited(&a), Some(("Auth.java".into(), Some(2))));
    }

    #[test]
    fn ctrl_b_on_a_name_goes_to_its_declaration() {
        let (_d, ctx) = setup();
        let mut p = CodeHistory::new();
        type_text(&mut p, &ctx, "Auth");
        screen(&mut p, &ctx);
        let a = p.handle_key(key(KeyCode::F(12), KeyModifiers::NONE), &ctx);
        assert_eq!(edited(&a), Some(("Auth.java".into(), Some(1))));

        let mut p = CodeHistory::new();
        type_text(&mut p, &ctx, "nowhere_at_all");
        screen(&mut p, &ctx);
        let a = p.handle_key(key(KeyCode::Char('b'), KeyModifiers::CONTROL), &ctx);
        assert!(a.is_none());
        let s = screen(&mut p, &ctx);
        assert!(
            s.contains("No declaration found for \"nowhere_at_all\""),
            "{s}"
        );
        // The next key brings the counts back.
        p.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &ctx);
        let s = screen(&mut p, &ctx);
        assert!(
            s.contains("no commit changed it · not in the code any more"),
            "{s}"
        );
    }
}
