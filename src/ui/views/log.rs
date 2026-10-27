//! Commit log: commit graph, ref labels, filters, details pane and history
//! actions (checkout, cherry-pick, revert, reset, undo, reword, branch/tag here, compare).
//!
//! Commits load page by page on a background thread so huge repositories and expensive filters
//! (`-S`, rare authors) never freeze the UI; more pages load automatically while scrolling.

mod clipboard;
mod details;
mod filters;
mod mailbox;
mod menu;
mod rows;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::graph::{layout, GraphRow};
use crate::git::history;
use crate::git::log::{Commit, LogQuery};
use crate::git::Repo;
use crate::ui::popups::InputPopup;
use crate::ui::views::Tab;
use crate::ui::widgets::{move_sel, panel, scroll_to_keep_visible};
use crate::ui::{KeyHelp, PaletteItem, View};

use details::{Details, DetailsLoader};
use filters::Filters;
use mailbox::{post, Msg};
use rows::{columns, commit_line, date_text, RefIndex, RowStyle};

/// Start loading the next page when the selection gets this close to the end.
const LOOKAHEAD: usize = 50;
/// A refresh reloads everything loaded so far, but never more than this many commits.
const MAX_RELOAD: usize = 20_000;
/// Two clicks on the same row within this time open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Details,
}

/// Result of one background load.
struct Loaded {
    commits: Vec<Commit>,
    /// Parents rewritten by a path filter, used for the graph only (`commits` hold the real parents)
    graph_parents: HashMap<String, Vec<String>>,
    /// Only for full reloads: refs, HEAD and the user identity are re-read with them
    refs: Option<RefIndex>,
    head: Option<String>,
    me: Option<(String, String)>,
}

struct LoadDone {
    generation: u64,
    append: bool,
    requested: usize,
    result: anyhow::Result<Loaded>,
}

/// Remote branches containing HEAD, computed after a full reload has been delivered.
struct PushedDone {
    generation: u64,
    head: String,
    remotes: Option<Vec<String>>,
}

/// Background loading of log pages. Only the result of the latest request is applied.
struct Loader {
    tx: Sender<LoadDone>,
    rx: Receiver<LoadDone>,
    pushed_tx: Sender<PushedDone>,
    pushed_rx: Receiver<PushedDone>,
    generation: u64,
    /// `Some(append)` while a load is running
    running: Option<bool>,
}

impl Loader {
    fn new() -> Self {
        let (tx, rx) = channel();
        let (pushed_tx, pushed_rx) = channel();
        Self {
            tx,
            rx,
            pushed_tx,
            pushed_rx,
            generation: 0,
            running: None,
        }
    }

    fn start(&mut self, repo: Repo, q: LogQuery, append: bool) {
        self.generation += 1;
        self.running = Some(append);
        let generation = self.generation;
        let tx = self.tx.clone();
        let pushed_tx = self.pushed_tx.clone();
        let requested = q.max;
        std::thread::spawn(move || {
            let result = load(&repo, &q, append);
            let head = result.as_ref().ok().and_then(|l| l.head.clone());
            let _ = tx.send(LoadDone {
                generation,
                append,
                requested,
                result,
            });
            // `branch -r --contains` can be slow with many remote branches: it runs after the commits
            // are delivered, still off the UI thread, and only feeds the "already pushed" warnings.
            if let Some(head) = head {
                let remotes = history::pushed_to(&repo, &head).ok();
                let _ = pushed_tx.send(PushedDone {
                    generation,
                    head,
                    remotes,
                });
            }
        });
    }
}

fn load(repo: &Repo, q: &LogQuery, append: bool) -> anyhow::Result<Loaded> {
    // An unborn branch has no history to walk (but other refs may exist with `--all`).
    let unborn_head =
        !q.all && repo.head_commit().is_none() && q.revs.iter().all(|r| r.starts_with('-'));
    let mut commits = if unborn_head {
        Vec::new()
    } else {
        repo.log(q)?
    };
    let graph_parents = if q.paths.is_empty() {
        HashMap::new()
    } else {
        filters::restore_real_parents(repo, &mut commits)?
    };
    if append {
        return Ok(Loaded {
            commits,
            graph_parents,
            refs: None,
            head: None,
            me: None,
        });
    }
    let me = repo.config_get("user.email").map(|email| {
        let name = repo
            .config_get("user.name")
            .unwrap_or_else(|| email.clone());
        (name, email)
    });
    let head = repo.head_commit();
    Ok(Loaded {
        commits,
        graph_parents,
        refs: Some(RefIndex::load(repo)),
        head,
        me,
    })
}

/// The Log tab.
pub struct LogView {
    commits: Vec<Commit>,
    graph: Vec<GraphRow>,
    /// Graph-only parents of path-filtered commits (see [`filters::restore_real_parents`])
    graph_parents: HashMap<String, Vec<String>>,
    /// No more commits beyond the loaded ones
    exhausted: bool,
    filters: Filters,
    all_by_default: bool,
    page_size: usize,
    sel: usize,
    offset: usize,
    marked: HashSet<String>,
    focus: Focus,
    show_details: bool,
    absolute_dates: bool,
    details: Option<Details>,
    details_loader: DetailsLoader,
    file_sel: usize,
    file_offset: usize,
    info_scroll: u16,
    refs: RefIndex,
    head: Option<String>,
    /// Remote branches containing HEAD (`None` until the background check finishes)
    head_pushed: Option<Vec<String>>,
    /// Generation of the last applied full reload
    reload_generation: u64,
    /// (user.name, user.email)
    me: Option<(String, String)>,
    error: Option<String>,
    /// One-line message under the list (text, is_error); cleared by the next key press
    notice: Option<(String, bool)>,
    loader: Loader,
    /// Select this commit when the running reload finishes (go to)
    pending_select: Option<String>,
    /// Commit selected when the running reload started
    keep_selection: Option<String>,
    list_area: Rect,
    files_area: Rect,
    last_click: Option<(Instant, Focus, usize)>,
}

impl LogView {
    pub fn new(ctx: &Ctx) -> Self {
        let all = ctx.config.log.all_branches;
        Self {
            commits: Vec::new(),
            graph: Vec::new(),
            graph_parents: HashMap::new(),
            exhausted: false,
            filters: Filters::new(all),
            all_by_default: all,
            page_size: ctx.config.log.page_size.clamp(50, 10_000),
            sel: 0,
            offset: 0,
            marked: HashSet::new(),
            focus: Focus::List,
            show_details: true,
            absolute_dates: false,
            details: None,
            details_loader: DetailsLoader::new(),
            file_sel: 0,
            file_offset: 0,
            info_scroll: 0,
            refs: RefIndex::default(),
            head: None,
            head_pushed: None,
            reload_generation: 0,
            me: None,
            error: None,
            notice: None,
            loader: Loader::new(),
            pending_select: None,
            keep_selection: None,
            list_area: Rect::default(),
            files_area: Rect::default(),
            last_click: None,
        }
    }

    fn selected(&self) -> Option<&Commit> {
        self.commits.get(self.sel)
    }

    // ----- loading ---------------------------------------------------------------------------

    /// Reload from the top (keeping the selection), loading as many commits as are loaded now.
    fn reload(&mut self, ctx: &Ctx) {
        let max = self.page_size.max(self.commits.len()).min(MAX_RELOAD);
        self.keep_selection = self.selected().map(|c| c.id.clone());
        self.loader
            .start(ctx.repo.clone(), self.filters.to_query(0, max), false);
    }

    fn load_more(&mut self, ctx: &Ctx) {
        if self.exhausted || self.loader.running.is_some() || self.commits.is_empty() {
            return;
        }
        let q = self.filters.to_query(self.commits.len(), self.page_size);
        self.loader.start(ctx.repo.clone(), q, true);
    }

    fn maybe_load_more(&mut self, ctx: &Ctx, visible_end: usize) {
        if self.sel + LOOKAHEAD >= self.commits.len() || visible_end >= self.commits.len() {
            self.load_more(ctx);
        }
    }

    fn poll_loader(&mut self) {
        while let Ok(done) = self.loader.rx.try_recv() {
            if done.generation != self.loader.generation {
                continue;
            }
            self.loader.running = None;
            match done.result {
                Ok(l) if done.append => self.apply_page(l, done.requested),
                Ok(l) => {
                    self.reload_generation = done.generation;
                    self.apply_reload(l, done.requested)
                }
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    if done.append {
                        self.exhausted = true;
                    } else {
                        self.commits.clear();
                        self.graph.clear();
                        self.graph_parents.clear();
                        self.sel = 0;
                    }
                }
            }
        }
        // Applied after the reload it belongs to (it is sent later by the same thread).
        while let Ok(p) = self.loader.pushed_rx.try_recv() {
            if p.generation == self.reload_generation
                && self.head.as_deref() == Some(p.head.as_str())
            {
                self.head_pushed = p.remotes;
            }
        }
    }

    fn apply_reload(&mut self, l: Loaded, requested: usize) {
        self.exhausted = l.commits.len() < requested;
        self.commits = l.commits;
        self.graph_parents = l.graph_parents;
        self.refs = l.refs.unwrap_or_default();
        self.head = l.head;
        self.head_pushed = None;
        self.me = l.me;
        self.error = None;
        let ids: HashSet<&str> = self.commits.iter().map(|c| c.id.as_str()).collect();
        self.marked.retain(|m| ids.contains(m.as_str()));
        let goto = self.pending_select.take();
        let keep = self.keep_selection.take();
        let found = |id: &String| self.commits.iter().position(|c| &c.id == id);
        match (goto.as_ref().and_then(found), keep.as_ref().and_then(found)) {
            (Some(i), _) => self.sel = i,
            (None, Some(i)) if goto.is_none() => self.sel = i,
            _ => self.sel = self.sel.min(self.commits.len().saturating_sub(1)),
        }
        if let (Some(id), None) = (&goto, goto.as_ref().and_then(found)) {
            let short = &id[..id.len().min(10)];
            self.notice = Some((
                format!("Commit {short} is not in the log with the current filters"),
                true,
            ));
        }
        self.rebuild_graph();
    }

    fn apply_page(&mut self, page: Loaded, requested: usize) {
        self.exhausted = page.commits.len() < requested;
        let known: HashSet<String> = self.commits.iter().map(|c| c.id.clone()).collect();
        self.commits
            .extend(page.commits.into_iter().filter(|c| !known.contains(&c.id)));
        self.graph_parents.extend(page.graph_parents);
        self.rebuild_graph();
    }

    /// Lay out the graph. Path filters connect commits through their rewritten parents. When filters
    /// hide commits from the middle of the history, parents that are not listed are dropped so lanes do
    /// not pile up; first-parent mode follows first parents only.
    fn rebuild_graph(&mut self) {
        let hides = self.filters.hides_commits();
        let first_parent = self.filters.first_parent;
        if !hides && !first_parent && self.graph_parents.is_empty() {
            self.graph = layout(&self.commits);
            return;
        }
        let ids: HashSet<&str> = self.commits.iter().map(|c| c.id.as_str()).collect();
        let simplified: Vec<Commit> = self
            .commits
            .iter()
            .map(|c| {
                let mut c = c.clone();
                if let Some(p) = self.graph_parents.get(&c.id) {
                    c.parents = p.clone();
                }
                if first_parent {
                    c.parents.truncate(1);
                }
                if hides {
                    c.parents.retain(|p| ids.contains(p.as_str()));
                }
                c
            })
            .collect();
        self.graph = layout(&simplified);
    }

    // ----- messages --------------------------------------------------------------------------

    fn process_mailbox(&mut self, ctx: &Ctx) {
        let mut reload = false;
        for msg in mailbox::take() {
            match msg {
                Msg::Edit(f) => {
                    let before = self.filters.clone();
                    f(&mut self.filters);
                    reload |= self.filters != before;
                }
                Msg::ClearFilters => {
                    self.filters.clear(self.all_by_default);
                    reload = true;
                }
                Msg::GoTo(id) => {
                    match self.commits.iter().position(|c| c.id == id) {
                        Some(i) => {
                            self.sel = i;
                            self.focus = Focus::List;
                        }
                        None => {
                            // Anchor the log at the commit; filters that could hide it are cleared so it
                            // is guaranteed to be listed (apply_reload reports it if it still is not).
                            let cleared = self.filters.clear_hiding();
                            self.filters.branch = filters::BranchFilter::Rev(id.clone());
                            let short = &id[..id.len().min(10)];
                            let what = if cleared {
                                "; filters that hid it were cleared"
                            } else {
                                ""
                            };
                            self.notice = Some((
                                format!("Showing history from {short}{what} (a: all branches, F: reset)"),
                                false,
                            ));
                            self.pending_select = Some(id);
                            reload = true;
                        }
                    }
                }
                Msg::ToggleMark(id) => self.toggle_mark(id),
            }
        }
        if reload {
            self.sel = 0;
            self.offset = 0;
            self.reload(ctx);
        }
    }

    fn toggle_mark(&mut self, id: String) {
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
    }

    /// Marked commits in log order (newest first).
    fn marked_commits(&self) -> Vec<Commit> {
        self.commits
            .iter()
            .filter(|c| self.marked.contains(&c.id))
            .cloned()
            .collect()
    }

    // ----- drawing ---------------------------------------------------------------------------

    fn list_title(&self) -> String {
        let mut parts = vec!["Log".to_string()];
        parts.extend(self.filters.describe());
        let count = if self.exhausted {
            format!("{}", self.commits.len())
        } else {
            format!("{}+", self.commits.len())
        };
        let mut title = format!(" {}  ({count})", parts.join(" · "));
        if !self.marked.is_empty() {
            title.push_str(&format!(" · {} marked", self.marked.len()));
        }
        if self.loader.running.is_some() {
            title.push_str(" · loading…");
        }
        title.push(' ');
        title
    }

    /// Text shown instead of the list when nothing is loaded, and whether it is an error.
    fn empty_message(&self) -> (String, bool) {
        if let Some(e) = &self.error {
            (e.clone(), true)
        } else if self.loader.running.is_some() {
            ("Loading history…".into(), false)
        } else if self.filters.is_filtered() {
            ("No commits match the filters (F clears them)".into(), false)
        } else {
            ("No commits yet".into(), false)
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let block = panel(self.list_title(), self.focus == Focus::List, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_area = inner;
        if self.commits.is_empty() {
            let (msg, is_error) = self.empty_message();
            let style = if is_error {
                Style::default().fg(t.error)
            } else {
                t.dimmed()
            };
            f.render_widget(
                Paragraph::new(msg).style(style).wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        let mut h = inner.height as usize;
        let error_line = self
            .error
            .clone()
            .map(|e| (e, true))
            .or_else(|| self.notice.clone());
        if error_line.is_some() {
            h = h.saturating_sub(1);
        }
        self.sel = self.sel.min(self.commits.len() - 1);
        self.offset = scroll_to_keep_visible(self.sel, self.offset, h);
        let end = (self.offset + h).min(self.commits.len());
        let visible = self.offset..end;
        let lanes = self.graph[visible.clone()]
            .iter()
            .map(|g| g.width)
            .max()
            .unwrap_or(1);
        let date_w = self.commits[visible.clone()]
            .iter()
            .map(|c| date_text(c.time, self.absolute_dates).width())
            .max()
            .unwrap_or(0);
        let hash_w = self.commits[visible.clone()]
            .iter()
            .map(|c| c.short.len())
            .max()
            .unwrap_or(7);
        let me = self.me.as_ref().map(|(_, e)| e.to_lowercase());
        let rs = RowStyle {
            theme: t,
            refs: &self.refs,
            cols: columns(inner.width as usize, lanes, date_w, hash_w),
            absolute_dates: self.absolute_dates,
            me: me.as_deref(),
            head: self.head.as_deref(),
        };
        let lines: Vec<Line> = visible
            .clone()
            .map(|i| {
                let c = &self.commits[i];
                let line = commit_line(c, &self.graph[i], self.marked.contains(&c.id), &rs);
                if i == self.sel {
                    line.style(t.selected())
                } else {
                    line
                }
            })
            .collect();
        let [rows_area, err_area] =
            Layout::vertical([Constraint::Length(h as u16), Constraint::Min(0)]).areas(inner);
        f.render_widget(Paragraph::new(lines), rows_area);
        if let Some((text, is_error)) = error_line {
            let style = if is_error {
                Style::default().fg(t.error)
            } else {
                t.dimmed()
            };
            f.render_widget(Paragraph::new(text).style(style), err_area);
        }
        self.maybe_load_more(ctx, end);
    }

    /// Show the selected commit's details: from the cache, or a "loading" placeholder while a
    /// background thread runs `git show` (huge commits must not freeze the UI).
    fn ensure_details(&mut self, ctx: &Ctx) {
        self.details_loader.poll();
        let Some(c) = self.commits.get(self.sel) else {
            self.details = None;
            return;
        };
        let id = c.id.clone();
        if self
            .details
            .as_ref()
            .is_some_and(|d| d.id == id && !d.loading)
        {
            return;
        }
        let shown_before = self.details.as_ref().is_some_and(|d| d.id == id);
        match self.details_loader.get(&id) {
            Some(d) => self.details = Some(d),
            None => {
                let placeholder = Details::placeholder(c);
                self.details_loader.request(&ctx.repo, &id);
                self.details = Some(placeholder);
            }
        }
        if !shown_before {
            self.file_sel = 0;
            self.file_offset = 0;
            self.info_scroll = 0;
        }
    }

    fn draw_details(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let focused = self.focus == Focus::Details;
        let block = panel(" Details ", focused, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.ensure_details(ctx);
        let (Some(c), Some(d)) = (self.commits.get(self.sel), self.details.as_ref()) else {
            self.files_area = Rect::default();
            f.render_widget(
                Paragraph::new("No commit selected").style(t.dimmed()),
                inner,
            );
            return;
        };
        let info = details::info_lines(c, d, &self.refs, t);
        let w = inner.width.max(1) as usize;
        let needed: usize = info.iter().map(|l| l.width().max(1).div_ceil(w)).sum();
        let max_info = (inner.height as usize * 3 / 5).max(3);
        let info_h = needed.min(max_info) as u16;
        let [info_area, header_area, files_area] = Layout::vertical([
            Constraint::Length(info_h),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
        let max_scroll = needed.saturating_sub(info_h as usize) as u16;
        self.info_scroll = self.info_scroll.min(max_scroll);
        f.render_widget(
            Paragraph::new(info)
                .wrap(Wrap { trim: false })
                .scroll((self.info_scroll, 0)),
            info_area,
        );
        let more = if max_scroll > 0 {
            "  (PgUp/PgDn: scroll message)"
        } else {
            ""
        };
        let capped = if d.files.len() >= details::MAX_FILES {
            "+"
        } else {
            ""
        };
        let count = if d.loading {
            "loading…".to_string()
        } else {
            format!("{}{capped}", d.files.len())
        };
        f.render_widget(
            Line::styled(format!("Changed files ({count}){more}"), t.title()),
            header_area,
        );
        self.files_area = files_area;
        let fh = files_area.height as usize;
        self.file_sel = self.file_sel.min(d.files.len().saturating_sub(1));
        self.file_offset = scroll_to_keep_visible(self.file_sel, self.file_offset, fh);
        let lines: Vec<Line> = d
            .files
            .iter()
            .enumerate()
            .skip(self.file_offset)
            .take(fh)
            .map(|(i, file)| {
                let line = details::file_line(file, t);
                if focused && i == self.file_sel {
                    line.style(t.selected())
                } else {
                    line
                }
            })
            .collect();
        if lines.is_empty() && !d.loading {
            f.render_widget(
                Paragraph::new("  (no file changes)").style(t.dimmed()),
                files_area,
            );
        } else {
            f.render_widget(Paragraph::new(lines), files_area);
        }
    }

    // ----- keys ------------------------------------------------------------------------------

    fn page(&self) -> isize {
        (self.list_area.height as isize).max(1)
    }

    fn commit_menu(&self) -> Action {
        match self.selected() {
            Some(c) => menu::commit_menu(menu::Selection {
                commit: c.clone(),
                marked: self.marked_commits(),
                head: self.head.clone(),
                head_pushed: self.head_pushed.clone(),
                on_branch: self.on_branch(self.sel),
            }),
            None => Action::None,
        }
    }

    /// Whether the commit at `idx` is HEAD or an ancestor of it, decided from the loaded commits when
    /// the filters keep every commit (`None` when the loaded log cannot tell).
    fn on_branch(&self, idx: usize) -> Option<bool> {
        let head = self.head.as_deref()?;
        if self.commits.get(idx)?.id == head {
            return Some(true);
        }
        if !self.filters.keeps_ancestry() {
            return None;
        }
        let from = self.commits.iter().position(|c| c.id == head)?;
        Some(menu::reaches(&self.commits, from, idx))
    }

    fn selected_file(&self) -> Option<(&Commit, &details::ChangedFile)> {
        let c = self.selected()?;
        let d = self.details.as_ref().filter(|d| d.id == c.id)?;
        Some((c, d.files.get(self.file_sel)?))
    }

    /// Keys shared by both panes. `None` means "not handled".
    fn common_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Action> {
        let a = match key.code {
            KeyCode::Char('/') => filters::text_input(self.filters.text.as_deref().unwrap_or("")),
            KeyCode::Char('f') => {
                let me = self
                    .me
                    .clone()
                    .or_else(|| ctx.repo.config_get("user.email").map(|e| (e.clone(), e)));
                filters::filter_menu(&self.filters, &self.commits, me)
            }
            KeyCode::Char('F') => {
                post(Msg::ClearFilters);
                Action::Toast("Log filters cleared".into())
            }
            KeyCode::Char('g') => goto_popup(),
            KeyCode::Char('t') => {
                self.absolute_dates = !self.absolute_dates;
                Action::None
            }
            KeyCode::Char('a') => {
                post(Msg::Edit(Box::new(Filters::toggle_all)));
                Action::None
            }
            KeyCode::Char('D') => {
                self.show_details = !self.show_details;
                if !self.show_details {
                    self.focus = Focus::List;
                }
                Action::None
            }
            _ => return None,
        };
        Some(a)
    }

    fn list_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let len = self.commits.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.sel = move_sel(self.sel, -1, len),
            KeyCode::Down | KeyCode::Char('j') => self.sel = move_sel(self.sel, 1, len),
            KeyCode::PageUp => self.sel = move_sel(self.sel, -self.page(), len),
            KeyCode::PageDown => self.sel = move_sel(self.sel, self.page(), len),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = len.saturating_sub(1),
            KeyCode::Enter => return self.selected().map(menu::show_diff).unwrap_or(Action::None),
            KeyCode::Char('v') | KeyCode::Char(' ') => {
                if let Some(id) = self.selected().map(|c| c.id.clone()) {
                    self.toggle_mark(id);
                    self.sel = move_sel(self.sel, 1, len);
                }
            }
            KeyCode::Esc if !self.marked.is_empty() => {
                self.marked.clear();
                return Action::Toast("Marks cleared".into());
            }
            KeyCode::Char('y') => {
                return match self.selected() {
                    Some(c) => clipboard::copy(&c.id),
                    None => Action::None,
                }
            }
            KeyCode::Char('.') | KeyCode::Char('m') => return self.commit_menu(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.show_details = true;
                self.focus = Focus::Details;
            }
            _ => return Action::None,
        }
        let visible_end = self.offset + self.list_area.height as usize;
        self.maybe_load_more(ctx, visible_end);
        Action::None
    }

    fn details_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let n = self.details.as_ref().map_or(0, |d| d.files.len());
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.file_sel = move_sel(self.file_sel, -1, n),
            KeyCode::Down | KeyCode::Char('j') => self.file_sel = move_sel(self.file_sel, 1, n),
            KeyCode::Home => self.file_sel = 0,
            KeyCode::End => self.file_sel = n.saturating_sub(1),
            KeyCode::PageUp => self.info_scroll = self.info_scroll.saturating_sub(5),
            KeyCode::PageDown => self.info_scroll = self.info_scroll.saturating_add(5),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('d') => {
                return self.selected().map(menu::show_diff).unwrap_or(Action::None)
            }
            KeyCode::Enter
            | KeyCode::Char('b')
            | KeyCode::Char('h')
            | KeyCode::Char('l')
            | KeyCode::Char('.')
            | KeyCode::Char('m')
            | KeyCode::Char('y') => {
                let Some((c, file)) = self.selected_file() else {
                    return Action::None;
                };
                return match key.code {
                    KeyCode::Enter => menu::file_diff(c, file),
                    KeyCode::Char('b') => menu::blame(ctx, c, file),
                    KeyCode::Char('h') => {
                        crate::ui::views::files::open_file_history(ctx, &file.path)
                    }
                    KeyCode::Char('l') => menu::compare_local(c, file.diff_paths()),
                    KeyCode::Char('y') => clipboard::copy(&file.path),
                    _ => menu::file_menu(c, file),
                };
            }
            _ => {}
        }
        Action::None
    }

    fn click(&mut self, focus: Focus, index: usize) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(at, f, i)| {
            f == focus && i == index && now.duration_since(at) < DOUBLE_CLICK
        });
        self.last_click = if double {
            None
        } else {
            Some((now, focus, index))
        };
        double
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// `g`: jump to a commit by hash, branch, tag or any revision expression.
fn goto_popup() -> Action {
    Action::popup(
        InputPopup::new("Go to hash / branch / tag", "", |text, ctx| {
            let rev = text.trim();
            if rev.starts_with('-') {
                return Action::Error(format!("Unknown revision: {rev}"));
            }
            match ctx.repo.rev_parse(rev) {
                Ok(id) => {
                    post(Msg::GoTo(id));
                    Action::None
                }
                Err(_) => Action::Error(format!("Unknown revision: {rev}")),
            }
        })
        .prompt("Commit hash (or prefix), branch, tag or revision such as HEAD~3"),
    )
}

impl View for LogView {
    fn title(&self) -> String {
        "Log".into()
    }

    fn refresh(&mut self, ctx: &Ctx) -> anyhow::Result<()> {
        self.details_loader.forget_errors();
        self.process_mailbox(ctx);
        self.reload(ctx);
        Ok(())
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.poll_loader();
        self.process_mailbox(ctx);
        if !self.show_details {
            self.draw_list(f, area, ctx);
            return;
        }
        let wide = area.width >= 100;
        let (list, details) = if wide {
            let [l, d] =
                Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                    .areas(area);
            (l, d)
        } else {
            let [l, d] = Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);
            (l, d)
        };
        self.draw_list(f, list, ctx);
        self.draw_details(f, details, ctx);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        self.poll_loader();
        self.notice = None;
        self.process_mailbox(ctx);
        if let Some(a) = self.common_key(&key, ctx) {
            return a;
        }
        match self.focus {
            Focus::List => self.list_key(key, ctx),
            Focus::Details => self.details_key(key, ctx),
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, ctx: &Ctx) -> Action {
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return Action::None;
        }
        if contains(self.list_area, ev.column, ev.row) {
            let i = self.offset + (ev.row - self.list_area.y) as usize;
            if i < self.commits.len() {
                self.focus = Focus::List;
                self.sel = i;
                if self.click(Focus::List, i) {
                    return self.selected().map(menu::show_diff).unwrap_or(Action::None);
                }
                self.maybe_load_more(ctx, i);
            }
        } else if contains(self.files_area, ev.column, ev.row) {
            let i = self.file_offset + (ev.row - self.files_area.y) as usize;
            let n = self.details.as_ref().map_or(0, |d| d.files.len());
            if i < n {
                self.focus = Focus::Details;
                self.file_sel = i;
                if self.click(Focus::Details, i) {
                    if let Some((c, file)) = self.selected_file() {
                        return menu::file_diff(c, file);
                    }
                }
            }
        }
        Action::None
    }

    fn help(&self) -> Vec<KeyHelp> {
        let common: Vec<KeyHelp> = vec![
            ("/", "filter by text"),
            ("f", "filter menu"),
            ("F", "clear filters"),
            ("a", "all branches / HEAD"),
            ("g", "go to hash/branch/tag"),
            ("t", "relative/absolute dates"),
            ("D", "toggle details"),
        ];
        let mut v: Vec<KeyHelp> = match self.focus {
            Focus::List => vec![
                ("Enter", "show diff"),
                (". / m", "commit actions"),
                ("v / Space", "mark commit"),
                ("Tab", "details pane"),
                ("↑↓/jk", "move"),
                ("PgUp/PgDn", "page"),
                ("Home/End", "first/last"),
                ("y", "copy hash"),
                ("Esc", "clear marks"),
            ],
            Focus::Details => vec![
                ("Enter", "file diff"),
                (". / m", "file actions"),
                ("b", "blame at revision"),
                ("h", "file history"),
                ("l", "compare with local"),
                ("d", "commit diff"),
                ("↑↓/jk", "move"),
                ("PgUp/PgDn", "scroll message"),
                ("y", "copy path"),
                ("Tab/Esc", "back to log"),
            ],
        };
        v.extend(common);
        v
    }
}

/// Palette entries for this feature.
pub fn palette() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            name: "Go to commit…",
            run: |_| Action::SwitchTab(Tab::Log).then(goto_popup()),
        },
        PaletteItem {
            name: "Undo last commit",
            run: menu::undo_head,
        },
        PaletteItem {
            name: "Reset HEAD…",
            run: menu::reset_prompt,
        },
        PaletteItem {
            name: "Cherry-pick…",
            run: menu::cherry_pick_prompt,
        },
        PaletteItem {
            name: "Revert…",
            run: menu::revert_prompt,
        },
        PaletteItem {
            name: "Filter log by user…",
            run: |_| Action::SwitchTab(Tab::Log).then(filters::user_input()),
        },
        PaletteItem {
            name: "Filter log by branch…",
            run: |ctx| Action::SwitchTab(Tab::Log).then(filters::branch_picker(ctx)),
        },
        PaletteItem {
            name: "Filter log by path…",
            run: |_| Action::SwitchTab(Tab::Log).then(filters::path_input("")),
        },
        PaletteItem {
            name: "Toggle all branches in log",
            run: |_| {
                post(Msg::Edit(Box::new(Filters::toggle_all)));
                Action::SwitchTab(Tab::Log)
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn wait_loaded(v: &mut LogView) {
        for _ in 0..500 {
            v.poll_loader();
            if v.loader.running.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("log did not load");
    }

    fn render(v: &mut LogView, ctx: &Ctx, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| v.draw(f, f.area(), ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render until the details pane has finished loading in the background.
    fn render_loaded(v: &mut LogView, ctx: &Ctx, w: u16, h: u16) -> String {
        for _ in 0..500 {
            let screen = render(v, ctx, w, h);
            if v.details.as_ref().is_some_and(|d| !d.loading) {
                return screen;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("details did not load");
    }

    fn setup() -> (TempDir, Ctx) {
        let (d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("f.txt", "f\n")], "feature work");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("a.txt", "2\n")], "main work");
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge feature", "feature"])
            .unwrap();
        repo.run(&["tag", "v1"]).unwrap();
        let mut config = Config::default();
        config.log.page_size = 50;
        (d, Ctx::new(repo, config))
    }

    #[test]
    fn loads_draws_and_filters() {
        let (_d, ctx) = setup();
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 4);
        assert!(v.exhausted);
        assert_eq!(v.commits[0].subject, "merge feature");
        let screen = render(&mut v, &ctx, 140, 20);
        assert!(screen.contains("Changed files (loading…)"), "{screen}");
        let screen = render_loaded(&mut v, &ctx, 140, 20);
        assert!(screen.contains("merge feature"), "{screen}");
        assert!(screen.contains("[HEAD→main]"), "{screen}");
        assert!(screen.contains("[v1]"), "{screen}");
        assert!(screen.contains("○─╮"), "{screen}");
        assert!(screen.contains("Changed files (1)"), "{screen}");

        // Quick text filter through the mailbox, as the '/' popup does.
        post(Msg::Edit(Box::new(|f| {
            f.text = Some("feature work".into())
        })));
        render(&mut v, &ctx, 140, 20);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 1);
        assert!(v.list_title().contains("text: \"feature work\""));
        v.handle_key(key(KeyCode::Char('F')), &ctx);
        render(&mut v, &ctx, 140, 20);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 4);

        // HEAD only still includes the merged feature commit; first-parent drops it.
        post(Msg::Edit(Box::new(|f| {
            f.branch = filters::BranchFilter::Head;
            f.first_parent = true;
        })));
        render(&mut v, &ctx, 140, 20);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 3);
        assert!(v.graph.iter().all(|g| g.width == 1));
    }

    #[test]
    fn goto_marks_and_navigation() {
        let (_d, ctx) = setup();
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        render(&mut v, &ctx, 100, 20);
        let first = v.commits[3].id.clone();
        post(Msg::GoTo(first.clone()));
        v.handle_key(key(KeyCode::Null), &ctx);
        assert_eq!(v.sel, 3);
        v.handle_key(key(KeyCode::Home), &ctx);
        v.handle_key(key(KeyCode::Char('v')), &ctx);
        v.handle_key(key(KeyCode::Char('v')), &ctx);
        assert_eq!(v.marked.len(), 2);
        assert_eq!(v.sel, 2);
        let marked = v.marked_commits();
        assert_eq!(marked[0].id, v.commits[0].id, "log order, newest first");
        assert!(matches!(
            v.handle_key(key(KeyCode::Esc), &ctx),
            Action::Toast(_)
        ));
        assert!(v.marked.is_empty());
        v.handle_key(key(KeyCode::Tab), &ctx);
        assert_eq!(v.focus, Focus::Details);
        assert!(v.help().iter().any(|(k, _)| *k == "b"));
        v.handle_key(key(KeyCode::Esc), &ctx);
        assert_eq!(v.focus, Focus::List);
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('.')), &ctx),
            Action::Popup(_)
        ));
        assert!(matches!(
            v.handle_key(key(KeyCode::Enter), &ctx),
            Action::Popup(_)
        ));

        // A commit that is not loaded: reload anchored at it.
        post(Msg::Edit(Box::new(|f| {
            f.branch = filters::BranchFilter::Rev("feature".into())
        })));
        render(&mut v, &ctx, 100, 20);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 2);
        let main_tip = ctx.repo.rev_parse("main~1").unwrap();
        post(Msg::GoTo(main_tip.clone()));
        render(&mut v, &ctx, 100, 20);
        wait_loaded(&mut v);
        assert_eq!(v.selected().map(|c| c.id.clone()), Some(main_tip));
    }

    #[test]
    fn pagination_loads_more_while_scrolling() {
        let (_d, repo) = temp_repo();
        for i in 0..130 {
            commit_files(&repo, &[("n.txt", &format!("{i}\n"))], &format!("c{i}"));
        }
        let mut config = Config::default();
        config.log.page_size = 50;
        let ctx = Ctx::new(repo, config);
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 50);
        assert!(!v.exhausted);
        v.handle_key(key(KeyCode::Down), &ctx);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 100);
        v.handle_key(key(KeyCode::End), &ctx);
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 130);
        assert!(v.exhausted);
        assert_eq!(v.graph.len(), 130);
        // Refresh keeps everything loaded and the selection.
        v.sel = 120;
        let id = v.commits[120].id.clone();
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 130);
        assert_eq!(v.commits[v.sel].id, id);
    }

    #[test]
    fn empty_repository_and_detached_head() {
        let (_d, repo) = temp_repo();
        let ctx = Ctx::new(repo, Config::default());
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert!(v.commits.is_empty());
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(render(&mut v, &ctx, 80, 10).contains("No commits yet"));
        assert!(matches!(
            v.handle_key(key(KeyCode::Enter), &ctx),
            Action::None
        ));
        let mut f2 = Filters::new(false);
        f2.text = Some("x".into());
        v.filters = f2;
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert!(v.error.is_none(), "{:?}", v.error);

        let c1 = commit_files(&ctx.repo, &[("a", "1\n")], "one");
        commit_files(&ctx.repo, &[("a", "2\n")], "two");
        ctx.repo.run(&["checkout", "-q", "--detach", &c1]).unwrap();
        v.filters = Filters::new(false);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 1);
        let screen = render(&mut v, &ctx, 120, 10);
        assert!(screen.contains("[HEAD]"), "{screen}");
    }

    #[test]
    fn path_filter_graph_uses_rewritten_parents_but_actions_see_real_ones() {
        let (_d, repo) = temp_repo();
        let one = commit_files(&repo, &[("p.txt", "1\n")], "one");
        let mid = commit_files(&repo, &[("other.txt", "x\n")], "unrelated");
        commit_files(&repo, &[("p.txt", "3\n")], "three");
        let ctx = Ctx::new(repo, Config::default());
        let mut v = LogView::new(&ctx);
        v.filters.paths = vec!["p.txt".into()];
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 2);
        assert_eq!(
            v.commits[0].parents,
            vec![mid.clone()],
            "real parent for actions"
        );
        assert_eq!(v.graph_parents[&v.commits[0].id], vec![one]);
        // The graph connects "three" straight to "one": a single lane, no dangling edge.
        assert!(v.graph.iter().all(|g| g.width == 1));
        let screen = render_loaded(&mut v, &ctx, 140, 20);
        assert!(
            screen.contains(&mid[..7]),
            "details show the real parent: {screen}"
        );
    }

    #[test]
    fn goto_hidden_commit_clears_hiding_filters() {
        let (_d, ctx) = setup();
        let mut v = LogView::new(&ctx);
        v.filters.text = Some("merge".into());
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.commits.len(), 1);
        let target = ctx.repo.rev_parse("main~1").unwrap();
        post(Msg::GoTo(target.clone()));
        render(&mut v, &ctx, 120, 20);
        wait_loaded(&mut v);
        assert_eq!(v.selected().map(|c| c.id.clone()), Some(target.clone()));
        assert!(v.filters.text.is_none());
        let screen = render(&mut v, &ctx, 120, 20);
        assert!(
            screen.contains("filters that hid it were cleared"),
            "{screen}"
        );
        // The notice goes away with the next key.
        v.handle_key(key(KeyCode::Null), &ctx);
        assert!(v.notice.is_none());
    }

    #[test]
    fn ancestry_from_loaded_commits() {
        fn pos(v: &LogView, ctx: &Ctx, rev: &str) -> usize {
            let id = ctx.repo.rev_parse(rev).unwrap();
            v.commits.iter().position(|c| c.id == id).unwrap()
        }
        let (_d, ctx) = setup();
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.on_branch(pos(&v, &ctx, "HEAD")), Some(true));
        assert_eq!(
            v.on_branch(pos(&v, &ctx, "feature")),
            Some(true),
            "merged side branch"
        );
        assert_eq!(v.on_branch(pos(&v, &ctx, "main~2")), Some(true));
        let main_work = ctx.repo.rev_parse("main~1").unwrap();
        ctx.repo.run(&["checkout", "-q", "feature"]).unwrap();
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(
            v.on_branch(pos(&v, &ctx, &main_work)),
            Some(false),
            "main work is not on feature"
        );
        // Filters that hide commits: the loaded log cannot tell, so the action checks with git later.
        v.filters.text = Some("work".into());
        v.refresh(&ctx).unwrap();
        wait_loaded(&mut v);
        assert_eq!(v.on_branch(pos(&v, &ctx, "HEAD")), Some(true));
        assert_eq!(v.on_branch(pos(&v, &ctx, &main_work)), None);
    }

    #[test]
    fn pushed_state_of_head_arrives_in_background() {
        let (_d, ctx) = setup();
        let head = ctx.repo.head_commit().unwrap();
        ctx.repo
            .run(&["update-ref", "refs/remotes/origin/main", &head])
            .unwrap();
        let mut v = LogView::new(&ctx);
        v.refresh(&ctx).unwrap();
        for _ in 0..500 {
            v.poll_loader();
            if v.head_pushed.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(v.head_pushed, Some(vec!["origin/main".to_string()]));
        assert!(matches!(v.commit_menu(), Action::Popup(_)));
    }
}
