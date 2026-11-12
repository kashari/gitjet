//! Two search dialogs that work from every tab, as in an IDE:
//!
//! - [`FindInFiles`]: text in all files of the work tree, with a preview of the selected match.
//! - [`GoTo`]: open a file by (part of) its name, `CamelHumps` included, or a class, struct or
//!   function by name.
//!
//! Both search while you type, off the UI thread, and open the selected result in the editor.

use std::collections::HashMap;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::search::{self, GrepOptions, Hit};
use crate::theme::Theme;
use crate::ui::bg::{Background, Preview};
use crate::ui::widgets::{centered_pct, move_sel, scroll_to_keep_visible, TextInput};
use crate::ui::{KeyHelp, PaletteItem, Popup};
use crate::util::truncate;

/// Matches shown at most; more than this and the query needs narrowing anyway.
const MAX_HITS: usize = 1000;
/// Files and symbols listed at most in "go to".
const MAX_ROWS: usize = 200;
const SETTLE: Duration = Duration::from_millis(120);

pub fn palette() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            name: "Find in files…",
            run: |_| Action::popup(FindInFiles::new()),
        },
        PaletteItem {
            name: "Go to file or class…",
            run: |ctx| Action::popup(GoTo::new(ctx)),
        },
    ]
}

fn frame<'a>(title: &'a str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.border(true))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), t.title()))
}

fn keys_line<'a>(keys: &[(&'a str, &'a str)], t: &Theme) -> Line<'a> {
    let mut spans = Vec::new();
    for (k, what) in keys {
        spans.push(Span::styled(*k, t.key()));
        spans.push(Span::styled(format!(" {what}   "), t.dimmed()));
    }
    Line::from(spans)
}

/// `text` with the byte range `at` in the match style. Ranges that do not fall on character
/// boundaries (or outside the text) are ignored.
fn highlighted(text: &str, at: Option<(usize, usize)>, base: Style, hit: Style) -> Line<'static> {
    match at {
        Some((a, b)) if a < b && text.is_char_boundary(a) && text.is_char_boundary(b) => {
            Line::from(vec![
                Span::styled(text[..a].to_string(), base),
                Span::styled(text[a..b].to_string(), hit),
                Span::styled(text[b..].to_string(), base),
            ])
        }
        _ => Line::from(Span::styled(text.to_string(), base)),
    }
}

/// Selection and scrolling of a result list.
#[derive(Default)]
struct List {
    sel: usize,
    offset: usize,
}

impl List {
    fn key(&mut self, key: &KeyEvent, len: usize, page: usize) -> bool {
        let delta = match key.code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -(page as isize),
            KeyCode::PageDown => page as isize,
            _ => return false,
        };
        self.sel = move_sel(self.sel, delta, len);
        true
    }

    fn fit(&mut self, len: usize, height: usize) {
        self.sel = self.sel.min(len.saturating_sub(1));
        self.offset = scroll_to_keep_visible(self.sel, self.offset, height);
    }
}

// ----- find in files -------------------------------------------------------------------------

type Found = Result<(Vec<Hit>, bool), String>;

pub struct FindInFiles {
    input: TextInput,
    opts: GrepOptions,
    loader: Preview<(String, GrepOptions), Found>,
    /// The result on screen and the query it belongs to
    shown: Option<((String, GrepOptions), Found)>,
    list: List,
    list_height: usize,
    /// Lines of the files previewed so far (None = unreadable or binary)
    files: HashMap<String, Option<Vec<String>>>,
}

impl FindInFiles {
    pub fn new() -> Self {
        Self {
            input: TextInput::new(""),
            opts: GrepOptions::default(),
            loader: Preview::new(SETTLE, 16),
            shown: None,
            list: List::default(),
            list_height: 10,
            files: HashMap::new(),
        }
    }

    fn key(&self) -> Option<(String, GrepOptions)> {
        let q = self.input.value();
        (q.chars().count() >= 2).then_some((q, self.opts))
    }

    /// Ask for the current query and pick up its result when it is there.
    fn update(&mut self, ctx: &Ctx) {
        let Some(key) = self.key() else {
            self.shown = None;
            return;
        };
        if self.shown.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        match self.loader.get(&key) {
            Some(found) => {
                self.shown = Some((key, found));
                self.list = List::default();
            }
            None => {
                let (q, opts) = key.clone();
                self.loader.request(&ctx.repo, key, move |repo| {
                    search::grep(repo, &q, opts, MAX_HITS).map_err(|e| format!("{e:#}"))
                });
            }
        }
    }

    fn hits(&self) -> &[Hit] {
        match &self.shown {
            Some((_, Ok((hits, _)))) => hits,
            _ => &[],
        }
    }

    fn status(&self) -> (String, bool) {
        let waiting =
            self.key().is_some() && self.key().as_ref() != self.shown.as_ref().map(|(k, _)| k);
        match &self.shown {
            _ if self.key().is_none() => ("Type at least 2 characters".into(), false),
            Some((_, Err(e))) if !waiting => (e.lines().next().unwrap_or("").to_string(), true),
            Some((_, Ok((hits, more)))) if !waiting => {
                let files = hits
                    .iter()
                    .map(|h| h.path.as_str())
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                let n = hits.len();
                (
                    match (n, more) {
                        (0, _) => "No matches".into(),
                        (_, true) => format!("First {n} matches in {files} files (there are more: narrow the search)"),
                        (1, _) => "1 match".into(),
                        _ => format!("{n} matches in {files} file{}", if files == 1 { "" } else { "s" }),
                    },
                    false,
                )
            }
            _ => ("Searching…".into(), false),
        }
    }

    /// Byte range of the match in a hit's text, for highlighting (plain-text searches only).
    fn match_range(&self, hit: &Hit) -> Option<(usize, usize)> {
        if self.opts.regex {
            return None;
        }
        let start = hit.column.checked_sub(1)?;
        let end = start + self.input.value().len();
        (end <= hit.text.len()).then_some((start, end))
    }

    fn draw_preview(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let Some(hit) = self.hits().get(self.list.sel).cloned() else {
            return;
        };
        let lines = self.files.entry(hit.path.clone()).or_insert_with(|| {
            let bytes = std::fs::read(ctx.repo.path(&hit.path)).ok()?;
            (bytes.len() < 4_000_000).then(|| {
                String::from_utf8_lossy(&bytes)
                    .lines()
                    .map(|l| l.replace('\t', "    "))
                    .collect()
            })
        });
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(t.border(false))
            .title(Span::styled(
                format!(" {}:{} ", hit.path, hit.line),
                t.dimmed(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(lines) = lines else {
            f.render_widget(
                Paragraph::new("(cannot show this file)").style(t.dimmed()),
                inner,
            );
            return;
        };
        let h = inner.height as usize;
        let first = hit.line.saturating_sub(1).saturating_sub(h / 2);
        let width = lines.len().to_string().len();
        let text: Vec<Line> = lines
            .iter()
            .enumerate()
            .skip(first)
            .take(h)
            .map(|(i, l)| {
                let current = i + 1 == hit.line;
                let style = if current {
                    t.text().bg(t.selection_bg)
                } else {
                    t.text()
                };
                Line::from(vec![
                    Span::styled(format!("{:>width$} ", i + 1), t.dimmed()),
                    Span::styled(l.clone(), style),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(text), inner);
    }
}

impl Popup for FindInFiles {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.update(ctx);
        let t = &ctx.theme;
        let r = centered_pct(area, 92, 90);
        f.render_widget(Clear, r);
        let block = frame("Find in files", t);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [field, status, list, preview, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Percentage(50),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(inner);

        let flag = |on: bool, label: &'static str| {
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
        let flags = Line::from(vec![
            flag(self.opts.case_sensitive, "Aa"),
            Span::raw(" "),
            flag(self.opts.whole_word, "W"),
            Span::raw(" "),
            flag(self.opts.regex, ".*"),
        ]);
        let [input, flags_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(13)]).areas(field);
        self.input.render(f, input, t.text(), true);
        f.render_widget(flags, flags_area);

        let (text, error) = self.status();
        let style = if error {
            Style::default().fg(t.error)
        } else {
            t.dimmed()
        };
        f.render_widget(Paragraph::new(text).style(style), status);

        self.list_height = list.height as usize;
        let n = self.hits().len();
        self.list.fit(n, self.list_height);
        let width = list.width as usize;
        let rows: Vec<Line> = self
            .hits()
            .iter()
            .enumerate()
            .skip(self.list.offset)
            .take(self.list_height)
            .map(|(i, h)| {
                let place = format!("{}:{}", h.path, h.line);
                let place = truncate(&place, width / 2);
                // Leading indentation says nothing in a result list.
                let indent = h.text.len() - h.text.trim_start().len();
                let at = self
                    .match_range(h)
                    .and_then(|(a, b)| Some((a.checked_sub(indent)?, b - indent)));
                let mut line = highlighted(
                    h.text.trim_start(),
                    at,
                    t.text(),
                    Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
                );
                line.spans
                    .insert(0, Span::styled(format!("{place}  "), t.dimmed()));
                if i == self.list.sel {
                    line = line.style(t.selected());
                }
                line
            })
            .collect();
        f.render_widget(Paragraph::new(rows), list);
        self.draw_preview(f, preview, ctx);
        f.render_widget(
            keys_line(
                &[
                    ("Enter", "open"),
                    ("↑↓ PgUp PgDn", "move"),
                    ("Alt+C", "case"),
                    ("Alt+W", "words"),
                    ("Alt+R", "regex"),
                    ("Esc", "close"),
                ],
                t,
            ),
            keys,
        );
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Action::ClosePopup,
            KeyCode::Enter => {
                return match self.hits().get(self.list.sel) {
                    Some(h) => Action::edit(ctx.repo.path(&h.path), Some(h.line)),
                    None => Action::None,
                };
            }
            KeyCode::Char('c') if alt => self.opts.case_sensitive = !self.opts.case_sensitive,
            KeyCode::Char('w') if alt => self.opts.whole_word = !self.opts.whole_word,
            KeyCode::Char('r') if alt => self.opts.regex = !self.opts.regex,
            _ => {
                let n = self.hits().len();
                if !self.list.key(&key, n, self.list_height.max(1)) {
                    self.input.handle_key(&key);
                }
            }
        }
        self.update(ctx);
        Action::None
    }

    fn poll(&mut self, ctx: &Ctx) -> Action {
        self.update(ctx);
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![
            ("Enter", "open the match in the editor"),
            ("Alt+C", "match case"),
            ("Alt+W", "whole words"),
            ("Alt+R", "regular expression"),
            ("Esc", "close"),
        ]
    }
}

// ----- go to file or class -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    File(String),
    Symbol(Hit),
}

pub struct GoTo {
    input: TextInput,
    files_loader: Background<Result<Vec<String>, String>>,
    files: Vec<String>,
    error: Option<String>,
    symbols: Preview<String, Vec<Hit>>,
    /// Query the rows were computed for (with the number of symbols it had then)
    ranked_for: Option<(String, usize)>,
    rows: Vec<Target>,
    list: List,
    list_height: usize,
}

/// `name:42` → (`name`, line 42).
fn split_line(query: &str) -> (&str, Option<usize>) {
    match query.rsplit_once(':') {
        Some((name, n)) if !name.is_empty() => match n.trim().parse() {
            Ok(n) => (name, Some(n)),
            Err(_) => (query, None),
        },
        _ => (query, None),
    }
}

impl GoTo {
    pub fn new(ctx: &Ctx) -> Self {
        let mut files_loader = Background::new();
        files_loader.start(&ctx.repo, |repo| {
            search::files(repo).map_err(|e| format!("{e:#}"))
        });
        Self {
            input: TextInput::new(""),
            files_loader,
            files: Vec::new(),
            error: None,
            symbols: Preview::new(Duration::from_millis(200), 16),
            ranked_for: None,
            rows: Vec::new(),
            list: List::default(),
            list_height: 10,
        }
    }

    fn update(&mut self, ctx: &Ctx) {
        if let Some(loaded) = self.files_loader.poll() {
            match loaded {
                Ok(files) => self.files = files,
                Err(e) => self.error = Some(e),
            }
            self.ranked_for = None;
        }
        let value = self.input.value();
        let (name, _) = split_line(&value);
        let name = name.trim().to_string();
        // Declarations are searched in file contents: only worth it for a few characters or more.
        let symbols = if name.chars().count() >= 3 && !name.contains('/') {
            let found = self.symbols.get(&name);
            if found.is_none() {
                let q = name.clone();
                self.symbols.request(&ctx.repo, name.clone(), move |repo| {
                    search::symbols(repo, &q, 50).unwrap_or_default()
                });
            }
            found.unwrap_or_default()
        } else {
            Vec::new()
        };
        let key = (name.clone(), symbols.len());
        if self.ranked_for.as_ref() == Some(&key) {
            return;
        }
        let same_query = self.ranked_for.as_ref().is_some_and(|(q, _)| *q == name);
        let mut ranked: Vec<(i64, &String)> = self
            .files
            .iter()
            .filter_map(|p| search::file_score(p, &name).map(|s| (s, p)))
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        self.rows = ranked
            .into_iter()
            .take(MAX_ROWS)
            .map(|(_, p)| Target::File(p.clone()))
            .chain(symbols.into_iter().map(Target::Symbol))
            .collect();
        self.ranked_for = Some(key);
        if !same_query {
            self.list = List::default();
        }
    }

    fn open(&self, ctx: &Ctx) -> Action {
        let value = self.input.value();
        let (_, line) = split_line(&value);
        match self.rows.get(self.list.sel) {
            Some(Target::File(p)) => Action::edit(ctx.repo.path(p), line),
            Some(Target::Symbol(h)) => Action::edit(ctx.repo.path(&h.path), Some(h.line)),
            None => Action::None,
        }
    }
}

impl Popup for GoTo {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.update(ctx);
        let t = &ctx.theme;
        let r = centered_pct(area, 70, 70);
        f.render_widget(Clear, r);
        let block = frame("Go to file or class", t);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [field, status, list, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        self.input.render(f, field, t.text(), true);
        let (text, style) = match &self.error {
            Some(e) => (e.clone(), Style::default().fg(t.error)),
            None if self.files_loader.busy() => ("Listing files…".to_string(), t.dimmed()),
            None if self.rows.is_empty() => ("Nothing found".to_string(), t.dimmed()),
            None => (
                format!(
                    "{} files — name, CamelHumps (UsSe), dir/name, or name:line",
                    self.files.len()
                ),
                t.dimmed(),
            ),
        };
        f.render_widget(Paragraph::new(text).style(style), status);

        self.list_height = list.height as usize;
        self.list.fit(self.rows.len(), self.list_height);
        let width = list.width as usize;
        let rows: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.list.offset)
            .take(self.list_height)
            .map(|(i, row)| {
                let (main, place, style) = match row {
                    Target::File(p) => {
                        let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
                        (name.to_string(), dir.to_string(), t.text())
                    }
                    Target::Symbol(h) => (
                        h.text.trim().to_string(),
                        format!("{}:{}", h.path, h.line),
                        Style::default().fg(t.accent),
                    ),
                };
                let main = truncate(&main, width * 3 / 5);
                let mut line = Line::from(vec![
                    Span::styled(main, style),
                    Span::styled(format!("  {}", truncate(&place, width * 2 / 5)), t.dimmed()),
                ]);
                if i == self.list.sel {
                    line = line.style(t.selected());
                }
                line
            })
            .collect();
        f.render_widget(Paragraph::new(rows), list);
        f.render_widget(
            keys_line(
                &[
                    ("Enter", "open"),
                    ("↑↓ PgUp PgDn", "move"),
                    ("Esc", "close"),
                ],
                t,
            ),
            keys,
        );
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => return Action::ClosePopup,
            // The file opens in place of this dialog.
            KeyCode::Enter => {
                let open = self.open(ctx);
                return if open.is_none() {
                    Action::None
                } else {
                    Action::ClosePopup.then(open)
                };
            }
            _ => {
                if !self
                    .list
                    .key(&key, self.rows.len(), self.list_height.max(1))
                {
                    self.input.handle_key(&key);
                }
            }
        }
        self.update(ctx);
        Action::None
    }

    fn poll(&mut self, ctx: &Ctx) -> Action {
        self.update(ctx);
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        vec![("Enter", "open in the editor"), ("Esc", "close")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn setup() -> (TempDir, Ctx) {
        let (d, repo) = temp_repo();
        commit_files(
            &repo,
            &[
                (
                    "src/UserService.java",
                    "public class UserService {\n  void login() {}\n}\n",
                ),
                (
                    "src/util/strings.rs",
                    "pub struct Joiner;\n// login helper\n",
                ),
                ("README.md", "# demo\n"),
            ],
            "files",
        );
        (d, Ctx::new(repo, Config::default()))
    }

    fn screen(p: &mut dyn Popup, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| p.draw(f, f.area(), ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..30)
            .map(|y| (0..110).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn type_text(p: &mut dyn Popup, ctx: &Ctx, text: &str) {
        for c in text.chars() {
            p.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), ctx);
        }
    }

    fn opened(a: Action) -> Option<(String, Option<usize>)> {
        match a {
            Action::Edit { path, line } => Some((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                line,
            )),
            Action::Batch(v) => v.into_iter().find_map(opened),
            _ => None,
        }
    }

    #[test]
    fn find_in_files_lists_matches_previews_and_opens_at_the_line() {
        let (_d, ctx) = setup();
        let mut p = FindInFiles::new();
        assert!(screen(&mut p, &ctx).contains("Type at least 2 characters"));
        type_text(&mut p, &ctx, "login");
        let s = screen(&mut p, &ctx);
        assert!(s.contains("2 matches in 2 files"), "{s}");
        assert!(s.contains("src/UserService.java:2  void login() {}"), "{s}");
        // Preview of the selected match, with line numbers.
        assert!(s.contains("1 public class UserService {"), "{s}");
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            opened(p.handle_key(enter, &ctx)),
            Some(("UserService.java".into(), Some(2)))
        );
        p.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &ctx);
        assert_eq!(
            opened(p.handle_key(enter, &ctx)),
            Some(("strings.rs".into(), Some(2)))
        );

        // Whole words only: "logi" matches nothing any more.
        let mut p = FindInFiles::new();
        type_text(&mut p, &ctx, "logi");
        assert!(screen(&mut p, &ctx).contains("2 matches"));
        p.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT), &ctx);
        assert!(screen(&mut p, &ctx).contains("No matches"));
    }

    #[test]
    fn go_to_finds_files_by_humps_and_classes_by_name() {
        let (_d, ctx) = setup();
        let mut p = GoTo::new(&ctx);
        assert!(screen(&mut p, &ctx).contains("3 files"));
        type_text(&mut p, &ctx, "UsSe");
        let s = screen(&mut p, &ctx);
        assert!(s.contains("UserService.java  src"), "{s}");
        assert!(!s.contains("README.md"), "{s}");
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            opened(p.handle_key(enter, &ctx)),
            Some(("UserService.java".into(), None))
        );

        // A line number after the name.
        let mut p = GoTo::new(&ctx);
        type_text(&mut p, &ctx, "strings:2");
        screen(&mut p, &ctx);
        assert_eq!(
            opened(p.handle_key(enter, &ctx)),
            Some(("strings.rs".into(), Some(2)))
        );

        // A class that is not a file name: found as a declaration inside a file.
        let mut p = GoTo::new(&ctx);
        type_text(&mut p, &ctx, "Joiner");
        let s = screen(&mut p, &ctx);
        assert!(
            s.contains("pub struct Joiner;  src/util/strings.rs:1"),
            "{s}"
        );
        assert_eq!(
            opened(p.handle_key(enter, &ctx)),
            Some(("strings.rs".into(), Some(1)))
        );
    }
}
