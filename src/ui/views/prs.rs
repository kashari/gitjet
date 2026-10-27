//! Pull / merge requests for any hosting provider, through
//! the provider's REST API and plain git — GitHub, GitLab, Bitbucket Cloud and Server, Azure
//! DevOps, Gitea/Forgejo, or a generic Git server with browser links only.
//!
//! Header: provider · host/repo and sign-in state. Left: the request list with a state filter (`s`)
//! and search (`/`). Right: details — branches, +/- stats, checks, reviews, description, files and
//! comments. All network work happens on background threads ([`state`]) or spawned tasks
//! ([`dialogs`]); drawing never waits for it.

mod dialogs;
mod render;
mod state;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::hosting::{
    self, refs, Detection, Hosting, PrState, ProviderKind, PullRequest, StateFilter,
};
use crate::ui::popups::{MenuItem, MenuPopup};
use crate::ui::views::Tab;
use crate::ui::widgets::{move_sel, panel, scroll_to_keep_visible, TextInput};
use crate::ui::{KeyHelp, PaletteItem, View};

use dialogs::Target;
use render::spinner;
use state::{details_of, lock, start_details_load, start_list_load, Reload, Setup, SharedRef};

/// Selection must rest this long before its details are fetched (avoids a request per keypress).
const DETAILS_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    List,
    Details,
}

pub struct PrsView {
    shared: SharedRef,
    setup: Setup,
    auth_source: Option<String>,
    prs: Vec<PullRequest>,
    /// Indices into `prs` matching the search.
    visible: Vec<usize>,
    list_error: Option<String>,
    loading: bool,
    seen_version: u64,
    default_branch: Option<String>,
    filter: StateFilter,
    search: TextInput,
    searching: bool,
    sel: usize,
    offset: usize,
    focus: Pane,
    detail_scroll: usize,
    detail_page: usize,
    detail_total: usize,
    setup_scroll: usize,
    sel_changed: Instant,
    started: Instant,
    list_area: Rect,
    list_rows_y: u16,
    detail_area: Rect,
}

impl PrsView {
    pub fn new(_ctx: &Ctx) -> Self {
        Self::with_shared(SharedRef::default())
    }

    fn with_shared(shared: SharedRef) -> Self {
        Self {
            shared,
            setup: Setup::Detecting,
            auth_source: None,
            prs: Vec::new(),
            visible: Vec::new(),
            list_error: None,
            loading: false,
            seen_version: 0,
            default_branch: None,
            filter: StateFilter::Open,
            search: TextInput::default(),
            searching: false,
            sel: 0,
            offset: 0,
            focus: Pane::List,
            detail_scroll: 0,
            detail_page: 1,
            detail_total: 0,
            setup_scroll: 0,
            sel_changed: Instant::now(),
            started: Instant::now(),
            list_area: Rect::default(),
            list_rows_y: 0,
            detail_area: Rect::default(),
        }
    }

    /// Copy results written by loader threads into the view.
    fn sync(&mut self) {
        let changed = {
            let g = lock(&self.shared);
            self.setup = g.setup.clone();
            self.auth_source = g.auth_source.clone();
            self.loading = g.list_loading;
            self.default_branch = g.default_branch.clone();
            if g.list_version == self.seen_version {
                false
            } else {
                self.seen_version = g.list_version;
                match &g.list {
                    Some(Ok(v)) => {
                        self.prs = v.clone();
                        self.list_error = None;
                    }
                    Some(Err(e)) => {
                        self.prs.clear();
                        self.list_error = Some(e.clone());
                    }
                    None => {
                        self.prs.clear();
                        self.list_error = None;
                    }
                }
                true
            }
        };
        if changed {
            self.apply_filter();
        }
    }

    /// Recompute `visible` from the search terms, keeping the selected request when possible.
    fn apply_filter(&mut self) {
        let keep = self.selected().map(|p| p.number);
        let query = self.search.value().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.visible = self
            .prs
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                if terms.is_empty() {
                    return true;
                }
                let hay = format!(
                    "#{n} !{n} {} {} {} {} {}",
                    p.title,
                    p.author,
                    p.head,
                    p.base,
                    p.labels.join(" "),
                    n = p.number
                )
                .to_lowercase();
                terms.iter().all(|t| hay.contains(t))
            })
            .map(|(i, _)| i)
            .collect();
        let new_sel = keep
            .and_then(|n| self.visible.iter().position(|&i| self.prs[i].number == n))
            .unwrap_or(0);
        self.select(new_sel.min(self.visible.len().saturating_sub(1)));
    }

    fn selected(&self) -> Option<&PullRequest> {
        self.visible.get(self.sel).and_then(|&i| self.prs.get(i))
    }

    fn select(&mut self, sel: usize) {
        if sel != self.sel || self.detail_scroll != 0 {
            self.sel_changed = Instant::now();
        }
        if sel != self.sel {
            self.detail_scroll = 0;
        }
        self.sel = sel;
    }

    fn move_by(&mut self, delta: isize) {
        let s = move_sel(self.sel, delta, self.visible.len());
        self.select(s);
    }

    fn scroll_details(&mut self, delta: isize) {
        let max = self.detail_total.saturating_sub(self.detail_page);
        self.detail_scroll = (self.detail_scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    fn reload(&mut self, ctx: &Ctx, mode: Reload) {
        start_list_load(
            &self.shared,
            ctx.repo.clone(),
            ctx.config.hosting.clone(),
            self.filter,
            mode,
        );
        self.sync();
    }

    fn hosting(&self) -> Option<&Hosting> {
        self.setup.ready()
    }

    fn target(&self, ctx: &Ctx) -> Option<Target> {
        self.setup.hosting().map(|h| Target::new(h, ctx))
    }

    fn noun(&self) -> &'static str {
        self.setup
            .hosting()
            .map(|h| h.kind.noun())
            .unwrap_or("pull request")
    }

    /// Run `f` with the target and selected request, or explain why not.
    fn with_selected(&self, ctx: &Ctx, f: impl FnOnce(&Target, &PullRequest) -> Action) -> Action {
        match (self.target(ctx), self.selected()) {
            (Some(t), Some(pr)) => f(&t, pr),
            (Some(_), None) => Action::Toast(format!("No {} selected", self.noun())),
            (None, _) => Action::None,
        }
    }

    /// '.' / 'm': every action for the selection, with accelerators.
    fn context_menu(&self, ctx: &Ctx) -> Action {
        let Some(t) = self.target(ctx) else {
            return Action::None;
        };
        let h = t.h.clone();
        let mut items = Vec::new();
        if let Some(pr) = self.selected().cloned() {
            let open = pr.state == PrState::Open;
            let item = |label: String,
                        key: &str,
                        enabled: bool,
                        f: fn(&Target, &PullRequest, &Ctx) -> Action| {
                let (t, pr) = (t.clone(), pr.clone());
                MenuItem::new(label, key, move |ctx| f(&t, &pr, ctx)).enabled(enabled)
            };
            let r = h.reference(pr.number);
            items.push(item(format!("Show diff of {r}"), "d", true, dialogs::diff));
            items.push(item(
                format!("Checkout {r} as {}", refs::local_branch(pr.number)),
                "c",
                true,
                |t, pr, _| dialogs::checkout(t, pr),
            ));
            items.push(item("Add comment…".into(), "C", true, |t, pr, _| {
                dialogs::comment(t, pr)
            }));
            items.push(item("Review…".into(), "r", open, |t, pr, _| {
                dialogs::review(t, pr)
            }));
            items.push(item(
                "Merge…".into(),
                "G",
                open && !pr.draft,
                |t, pr, _| dialogs::merge(t, pr, false),
            ));
            let close_label = if pr.state == PrState::Closed {
                "Reopen"
            } else {
                "Close"
            };
            items.push(item(
                format!("{close_label} {r}"),
                "x",
                matches!(pr.state, PrState::Open | PrState::Closed),
                |t, pr, _| dialogs::close_or_reopen(t, pr),
            ));
            items.push(item("Open in browser".into(), "w", true, |t, pr, _| {
                dialogs::open_pr_web(&t.h, pr)
            }));
        }
        let default_base = self.default_branch.clone();
        let (tc, base) = (t.clone(), default_base.clone());
        items.push(MenuItem::new(
            format!("Create {}…", h.kind.noun()),
            "n",
            move |ctx| dialogs::create(&tc, base.as_deref(), ctx),
        ));
        let (tc, base) = (t.clone(), default_base);
        items.push(MenuItem::new(
            format!("Create {} in browser", h.kind.noun()),
            "N",
            move |ctx| dialogs::create_in_browser(&tc, base.as_deref(), ctx),
        ));
        let tc = t.clone();
        items.push(MenuItem::new(
            format!("Checkout {} by number…", h.kind.noun()),
            "#",
            move |_| dialogs::ask_number(&tc, "Checkout", |t, n, _| dialogs::checkout_number(t, n)),
        ));
        let hc = h.clone();
        items.push(MenuItem::new(
            "Open repository in browser",
            "W",
            move |_| dialogs::open_repo_web(&hc),
        ));
        if let Some(url) = hosting::web::pr_list_url(&h) {
            items.push(MenuItem::new(
                format!("Open {}s in browser", h.kind.noun()),
                "P",
                move |_| dialogs::open_in_browser(&url),
            ));
        }
        let hc = h.clone();
        items.push(MenuItem::new(
            "Open current branch in browser",
            "B",
            move |ctx| dialogs::open_branch_web(&hc, ctx),
        ));
        let tc = t.clone();
        items.push(
            MenuItem::new(
                format!("Sign in to {}…", h.url.credential_host()),
                "L",
                move |_| dialogs::sign_in(&tc),
            )
            .enabled(h.kind.has_api()),
        );
        let (shared, filter) = (self.shared.clone(), self.filter);
        items.push(MenuItem::new("Reload", "R", move |ctx| {
            // Same as the 'R' key: probe again, look up credentials again, drop cached details.
            start_list_load(
                &shared,
                ctx.repo.clone(),
                ctx.config.hosting.clone(),
                filter,
                Reload::Full,
            );
            Action::Toast("Reloading…".into())
        }));
        let title = match self.selected() {
            Some(pr) => h.reference(pr.number),
            None => format!("{}s", h.kind.short()),
        };
        Action::popup(MenuPopup::new(title, items))
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.search.set("");
                self.searching = false;
                self.apply_filter();
            }
            KeyCode::Enter => self.searching = false,
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            _ => {
                if self.search.handle_key(&key) {
                    self.apply_filter();
                }
            }
        }
        Action::None
    }

    /// Keys of the setup / limited screens (no request list).
    fn handle_setup_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if let KeyCode::Char('R') | KeyCode::Char('r') = key.code {
            self.reload(ctx, Reload::Full);
            return Action::Toast("Checking again…".into());
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.setup_scroll = self.setup_scroll.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.setup_scroll += 1;
                return Action::None;
            }
            _ => {}
        }
        let Some(t) = self.target(ctx) else {
            return Action::None;
        };
        let has_ref = refs::pr_ref(t.h.kind, 1).is_some();
        match key.code {
            KeyCode::Char('W') | KeyCode::Char('w') => dialogs::open_repo_web(&t.h),
            KeyCode::Char('B') | KeyCode::Char('b') => dialogs::open_branch_web(&t.h, ctx),
            KeyCode::Char('H') => dialogs::open_commit_web(&t.h, ctx),
            KeyCode::Char('N') | KeyCode::Char('n') => {
                dialogs::create_in_browser(&t, self.default_branch.as_deref(), ctx)
            }
            KeyCode::Char('L') | KeyCode::Char('l') => dialogs::sign_in(&t),
            KeyCode::Char('c') if has_ref => {
                dialogs::ask_number(&t, "Checkout", |t, n, _| dialogs::checkout_number(t, n))
            }
            KeyCode::Char('d') if has_ref => dialogs::ask_number(&t, "Diff", dialogs::diff_number),
            _ => Action::None,
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, h: &Hosting) {
        let t = &ctx.theme;
        let count = if self.visible.len() == self.prs.len() {
            self.prs.len().to_string()
        } else {
            format!("{}/{}", self.visible.len(), self.prs.len())
        };
        let busy = if self.loading {
            format!(" {}", spinner(self.started))
        } else {
            String::new()
        };
        let title = format!(
            " {}s · {} ({count}){busy} ",
            capitalize(h.kind.noun()),
            self.filter.label(),
        );
        let block = panel(title, self.focus == Pane::List, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_area = area;
        let show_search = self.searching || !self.search.is_empty();
        let [search_row, rows] = Layout::vertical([
            Constraint::Length(u16::from(show_search)),
            Constraint::Min(0),
        ])
        .areas(inner);
        self.list_rows_y = rows.y;
        if show_search {
            let [p, field] =
                Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(search_row);
            f.render_widget(Span::styled("/ ", t.key()), p);
            self.search.render(f, field, t.text(), self.searching);
        }
        if let Some(e) = &self.list_error {
            f.render_widget(
                Paragraph::new(e.as_str())
                    .style(Style::default().fg(t.error))
                    .wrap(Wrap { trim: false }),
                rows,
            );
            return;
        }
        if self.visible.is_empty() {
            let msg = if self.loading {
                format!("{} Loading {}s…", spinner(self.started), h.kind.noun())
            } else if !self.search.is_empty() {
                "Nothing matches the search. Esc clears it.".to_string()
            } else {
                format!(
                    "No {} {}s. Press n to create one, s to change the state filter.",
                    self.filter.label(),
                    h.kind.noun()
                )
            };
            f.render_widget(
                Paragraph::new(msg)
                    .style(t.dimmed())
                    .wrap(Wrap { trim: false }),
                rows,
            );
            return;
        }
        let height = rows.height as usize;
        self.offset = scroll_to_keep_visible(self.sel, self.offset, height);
        let max_number = self.prs.iter().map(|p| p.number).max().unwrap_or(0);
        let cols = render::columns(rows.width as usize, h.reference(max_number).chars().count());
        let lines: Vec<Line> = self
            .visible
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(vi, &i)| {
                let line = render::list_row(&self.prs[i], h, cols, t);
                if vi == self.sel {
                    line.style(t.selected())
                } else {
                    line
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), rows);
    }

    fn draw_details(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, h: &Hosting) {
        let t = &ctx.theme;
        let block = panel(" Details ", self.focus == Pane::Details, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.detail_area = area;
        self.detail_page = (inner.height as usize).max(1);
        let Some(pr) = self.selected().cloned() else {
            self.detail_total = 0;
            f.render_widget(
                Paragraph::new(format!("Select a {} to see its details.", h.kind.noun()))
                    .style(t.dimmed()),
                inner,
            );
            return;
        };
        let dv = details_of(&self.shared, pr.number);
        if dv.wanted && self.sel_changed.elapsed() >= DETAILS_DEBOUNCE {
            start_details_load(&self.shared, h.clone(), pr.number);
        }
        let width = inner.width as usize;
        let details = match &dv.data {
            Some(Ok(d)) => Some(Arc::clone(d)),
            _ => None,
        };
        let mut lines = render::detail_lines(&pr, details.as_deref(), h, width, t);
        match &dv.data {
            Some(Err(e)) => {
                lines.push(Line::raw(""));
                for l in render::wrap_text(e, width) {
                    lines.push(Line::styled(l, Style::default().fg(t.error)));
                }
            }
            None => {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    format!("{} Loading details…", spinner(self.started)),
                    t.dimmed(),
                ));
            }
            Some(Ok(_)) if dv.loading => {
                lines.insert(
                    0,
                    Line::styled(format!("{} refreshing…", spinner(self.started)), t.dimmed()),
                );
            }
            Some(Ok(_)) => {}
        }
        self.detail_total = lines.len();
        self.detail_scroll = self
            .detail_scroll
            .min(self.detail_total.saturating_sub(self.detail_page));
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(self.detail_scroll)
            .take(self.detail_page)
            .collect();
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn draw_setup(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let block = panel(format!(" {} ", self.title()), true, &ctx.theme);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let branch = ctx.repo.head_branch();
        let head = ctx.repo.head_commit();
        let lines = render::setup_lines(
            &self.setup,
            branch.as_deref(),
            head.as_deref(),
            self.started,
            &ctx.theme,
        );
        let max = lines.len().saturating_sub(1);
        self.setup_scroll = self.setup_scroll.min(max);
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.setup_scroll as u16, 0)),
            inner,
        );
    }
}

/// Keys that work while requests cannot be listed.
fn setup_help(setup: &Setup) -> Vec<KeyHelp> {
    let mut v = Vec::new();
    if let Some(h) = setup.hosting() {
        v.push(("W", "repository in browser"));
        v.push(("B", "branch in browser"));
        v.push(("H", "HEAD commit in browser"));
        if hosting::web::create_url(h, "x", "y", None).is_some() {
            v.push(("N", "create in browser"));
        }
        if refs::pr_ref(h.kind, 1).is_some() {
            v.push(("c", "checkout by number"));
            v.push(("d", "diff by number"));
        }
        if h.kind.has_api() {
            v.push(("L", "sign in"));
        }
        v.push(("↑↓ / jk", "scroll"));
    }
    v.push(("R", "check again"));
    v
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    r.contains(Position::new(col, row))
}

/// A non-`None` no-op: tells the app a scroll event was handled.
fn handled() -> Action {
    Action::Batch(Vec::new())
}

impl View for PrsView {
    fn title(&self) -> String {
        match self.setup.hosting().map(|h| h.kind) {
            Some(ProviderKind::GitLab) => "Merge Requests".into(),
            _ => "Pull Requests".into(),
        }
    }

    fn refresh(&mut self, ctx: &Ctx) -> anyhow::Result<()> {
        self.reload(ctx, Reload::Soft);
        Ok(())
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.sync();
        let Some(h) = self.hosting().cloned() else {
            self.draw_setup(f, area, ctx);
            return;
        };
        let [header, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(render::header_line(
                &h,
                self.auth_source.as_deref(),
                &ctx.theme,
            )),
            header,
        );
        let [list, details] = if body.width >= 110 {
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body)
        } else {
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body)
        };
        self.draw_list(f, list, ctx, &h);
        self.draw_details(f, details, ctx, &h);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        self.sync();
        if self.searching {
            return self.handle_search_key(key);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Action::None;
        }
        if self.hosting().is_none() {
            return self.handle_setup_key(key, ctx);
        }
        let page = self.detail_page.max(1) as isize;
        let list_page = self.list_area.height.saturating_sub(3).max(1) as isize;
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Pane::List {
                    Pane::Details
                } else {
                    Pane::List
                };
            }
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Pane::List => self.move_by(-1),
                Pane::Details => self.scroll_details(-1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Pane::List => self.move_by(1),
                Pane::Details => self.scroll_details(1),
            },
            KeyCode::PageUp => match self.focus {
                Pane::List => self.move_by(-list_page),
                Pane::Details => self.scroll_details(-page),
            },
            KeyCode::PageDown | KeyCode::Char(' ') => match self.focus {
                Pane::List => self.move_by(list_page),
                Pane::Details => self.scroll_details(page),
            },
            KeyCode::Home | KeyCode::Char('g') => match self.focus {
                Pane::List => self.select(0),
                Pane::Details => self.detail_scroll = 0,
            },
            KeyCode::End => match self.focus {
                Pane::List => self.select(self.visible.len().saturating_sub(1)),
                Pane::Details => self.scroll_details(isize::MAX / 2),
            },
            KeyCode::Esc => {
                if !self.search.is_empty() {
                    self.search.set("");
                    self.apply_filter();
                } else {
                    self.focus = Pane::List;
                }
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.focus = Pane::List;
            }
            KeyCode::Char('s') => {
                self.filter = self.filter.next();
                self.reload(ctx, Reload::Soft);
                return Action::Toast(format!("Showing {} {}s", self.filter.label(), self.noun()));
            }
            KeyCode::Char('R') => {
                self.reload(ctx, Reload::Full);
                return Action::Toast("Reloading…".into());
            }
            KeyCode::Enter | KeyCode::Char('d') => {
                return self.with_selected(ctx, |t, pr| dialogs::diff(t, pr, ctx))
            }
            KeyCode::Char('c') => return self.with_selected(ctx, dialogs::checkout),
            KeyCode::Char('C') => return self.with_selected(ctx, dialogs::comment),
            KeyCode::Char('r') => return self.with_selected(ctx, dialogs::review),
            KeyCode::Char('G') => {
                return self.with_selected(ctx, |t, pr| dialogs::merge(t, pr, false))
            }
            KeyCode::Char('x') => return self.with_selected(ctx, dialogs::close_or_reopen),
            KeyCode::Char('w') => {
                return self.with_selected(ctx, |t, pr| dialogs::open_pr_web(&t.h, pr))
            }
            KeyCode::Char('W') => {
                return self
                    .hosting()
                    .map(dialogs::open_repo_web)
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('B') => {
                return self
                    .hosting()
                    .map(|h| dialogs::open_branch_web(h, ctx))
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('n') => {
                return self
                    .target(ctx)
                    .map(|t| dialogs::create(&t, self.default_branch.as_deref(), ctx))
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('N') => {
                return self
                    .target(ctx)
                    .map(|t| dialogs::create_in_browser(&t, self.default_branch.as_deref(), ctx))
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('L') => {
                return self
                    .target(ctx)
                    .map(|t| dialogs::sign_in(&t))
                    .unwrap_or(Action::None)
            }
            KeyCode::Char('.') | KeyCode::Char('m') => return self.context_menu(ctx),
            _ => {}
        }
        Action::None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _ctx: &Ctx) -> Action {
        if self.hosting().is_none() {
            return Action::None;
        }
        let (col, row) = (ev.column, ev.row);
        let in_list = contains(self.list_area, col, row);
        let in_details = contains(self.detail_area, col, row);
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if in_list => {
                self.focus = Pane::List;
                if row >= self.list_rows_y {
                    let idx = self.offset + (row - self.list_rows_y) as usize;
                    if idx < self.visible.len() {
                        self.select(idx);
                    }
                }
                handled()
            }
            MouseEventKind::Down(MouseButton::Left) if in_details => {
                self.focus = Pane::Details;
                handled()
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if ev.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                if in_details {
                    self.scroll_details(delta);
                } else if in_list {
                    self.move_by(delta.signum());
                }
                handled()
            }
            _ => Action::None,
        }
    }

    fn help(&self) -> Vec<KeyHelp> {
        if !matches!(self.setup, Setup::Ready(_)) {
            return setup_help(&self.setup);
        }
        vec![
            ("Enter / d", "diff"),
            ("c", "checkout as pr/N"),
            ("n", "create"),
            ("N", "create in browser"),
            ("C", "comment"),
            ("r", "review"),
            ("G", "merge"),
            ("s", "state filter"),
            (". / m", "all actions"),
            ("↑↓ / jk", "move / scroll"),
            ("Tab", "switch pane"),
            ("/", "search"),
            ("x", "close / reopen"),
            ("w", "open in browser"),
            ("W", "repository in browser"),
            ("B", "branch in browser"),
            ("L", "sign in"),
            ("R", "reload"),
            ("PgUp/PgDn / Space", "page"),
            ("g / Home", "first / top"),
            ("End", "last / bottom"),
            ("Esc", "clear search / back"),
        ]
    }

    fn captures_input(&self) -> bool {
        self.searching
    }

    fn badge(&self) -> Option<String> {
        (self.filter == StateFilter::Open && self.setup.ready().is_some() && !self.prs.is_empty())
            .then(|| self.prs.len().to_string())
    }
}

// ----- palette ---------------------------------------------------------------------------------

/// The repository's hosting from local detection only (config, known hosts, cached probes).
fn palette_target(ctx: &Ctx) -> Result<Target, String> {
    match hosting::detect_local(&ctx.repo, &ctx.config.hosting) {
        Detection::Found(h) => Ok(Target::new(&h, ctx)),
        Detection::NoRemotes => {
            Err("This repository has no remotes — add one on a hosting server first".into())
        }
        Detection::NotHosted { remote, url } => Err(format!(
            "Remote '{remote}' ({url}) is a local path, not a hosted repository"
        )),
    }
}

fn palette_create(ctx: &Ctx) -> Action {
    match palette_target(ctx) {
        Ok(t) => dialogs::create(&t, None, ctx),
        Err(e) => Action::Error(e),
    }
}

fn palette_checkout(ctx: &Ctx) -> Action {
    match palette_target(ctx) {
        Ok(t) if refs::pr_ref(t.h.kind, 1).is_some() || t.h.kind.has_api() => {
            dialogs::ask_number(&t, "Checkout", |t, n, _| dialogs::checkout_number(t, n))
        }
        Ok(t) => Action::Error(format!(
            "{} has no known pull request refs or API; configure its kind under [[hosting.hosts]] in {}",
            t.h.url.host,
            crate::config::Config::path().display()
        )),
        Err(e) => Action::Error(e),
    }
}

fn palette_repo_web(ctx: &Ctx) -> Action {
    match palette_target(ctx) {
        Ok(t) => dialogs::open_repo_web(&t.h),
        Err(e) => Action::Error(e),
    }
}

fn palette_sign_in(ctx: &Ctx) -> Action {
    match palette_target(ctx) {
        Ok(t) => dialogs::sign_in(&t),
        Err(e) => Action::Error(e),
    }
}

/// Palette entries for this feature.
pub fn palette() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            name: "Create pull request…",
            run: palette_create,
        },
        PaletteItem {
            name: "View pull requests",
            run: |_| Action::SwitchTab(Tab::PullRequests),
        },
        PaletteItem {
            name: "Checkout pull request…",
            run: palette_checkout,
        },
        PaletteItem {
            name: "Open repository in browser",
            run: palette_repo_web,
        },
        PaletteItem {
            name: "Sign in to hosting provider…",
            run: palette_sign_in,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{parse_remote_url, DetectedBy};
    use crate::git::testutil::temp_repo;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn pr(number: u64, title: &str, author: &str) -> PullRequest {
        PullRequest {
            number,
            title: title.into(),
            author: author.into(),
            head: format!("feat-{number}"),
            base: "main".into(),
            state: PrState::Open,
            draft: number % 2 == 0,
            updated: Some(chrono::Utc::now().timestamp() - 3600),
            labels: vec!["ui".into()],
            ..Default::default()
        }
    }

    fn hosting_of(kind: ProviderKind, url: &str) -> Hosting {
        Hosting::new(
            kind,
            "origin",
            url,
            parse_remote_url(url).expect("url"),
            None,
            DetectedBy::KnownHost,
        )
    }

    fn ready_view(kind: ProviderKind, url: &str) -> PrsView {
        let mut v = PrsView::with_shared(state::tests::shared_with(FakeTransport::new()));
        {
            let mut g = lock(&v.shared);
            g.setup = Setup::Ready(hosting_of(kind, url));
            g.list = Some(Ok(vec![
                pr(3, "Fix login", "alice"),
                pr(2, "Add dark mode", "bob"),
                pr(1, "Docs", "alice"),
            ]));
            g.list_version = 1;
            // Pretend details are already cached so drawing does not start a request.
            for n in 1..=3 {
                g.details.insert(
                    n,
                    state::DetailEntry {
                        data: Some(Err("offline".into())),
                        loading: false,
                        fresh: true,
                    },
                );
            }
        }
        v.sync();
        v
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn screen(v: &mut PrsView, ctx: &Ctx, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
        term.draw(|f| v.draw(f, f.area(), ctx)).expect("draw");
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

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let (d, repo) = temp_repo();
        (d, Ctx::new(repo, Config::default()))
    }

    #[test]
    fn search_filters_and_keeps_selection() {
        let (_d, ctx) = ctx();
        let mut v = ready_view(ProviderKind::GitHub, "https://github.com/o/r");
        assert_eq!(v.visible.len(), 3);
        v.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(v.selected().map(|p| p.number), Some(2));
        v.handle_key(key(KeyCode::Char('/')), &ctx);
        assert!(v.captures_input());
        for c in "alice".chars() {
            v.handle_key(key(KeyCode::Char(c)), &ctx);
        }
        assert_eq!(v.visible.len(), 2);
        v.handle_key(key(KeyCode::Enter), &ctx);
        assert!(!v.captures_input());
        v.handle_key(key(KeyCode::Esc), &ctx);
        assert_eq!(v.visible.len(), 3);
        v.search.set("#1");
        v.apply_filter();
        assert_eq!(v.selected().map(|p| p.number), Some(1));
    }

    #[test]
    fn draws_header_list_and_details() {
        let (_d, ctx) = ctx();
        let mut v = ready_view(
            ProviderKind::Azure,
            "https://dev.azure.com/org/proj/_git/repo",
        );
        let wide = screen(&mut v, &ctx, 140, 20);
        assert!(
            wide.contains("Azure DevOps · dev.azure.com/org/proj/repo"),
            "{wide}"
        );
        assert!(wide.contains("not signed in"));
        assert!(wide.contains("Pull requests · open (3)"), "{wide}");
        assert!(wide.contains("!3"));
        assert!(wide.contains("Fix login"));
        assert!(wide.contains("[draft] Add dark mode"));
        assert!(wide.contains("feat-3 → main"));
        assert!(wide.contains("offline"));
        let narrow = screen(&mut v, &ctx, 60, 30);
        assert!(narrow.contains("Details"));
        assert_eq!(v.title(), "Pull Requests");
        let gl = ready_view(ProviderKind::GitLab, "https://gitlab.com/g/p");
        assert_eq!(gl.title(), "Merge Requests");
    }

    #[test]
    fn keys_switch_panes_and_open_dialogs() {
        let (_d, ctx) = ctx();
        let mut v = ready_view(ProviderKind::GitHub, "https://github.com/o/r");
        v.handle_key(key(KeyCode::Tab), &ctx);
        assert_eq!(v.focus, Pane::Details);
        v.handle_key(key(KeyCode::Tab), &ctx);
        assert_eq!(v.focus, Pane::List);
        for c in ['.', 'C', 'r', 'x', 'd', 'L'] {
            assert!(
                matches!(v.handle_key(key(KeyCode::Char(c)), &ctx), Action::Popup(_)),
                "{c}"
            );
        }
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('c')), &ctx),
            Action::Spawn(_)
        ));
        // Selected #3 is not a draft, so merge opens the menu.
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('G')), &ctx),
            Action::Popup(_)
        ));
        // Empty repository: create explains why it cannot.
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('n')), &ctx),
            Action::Error(_)
        ));
        assert_eq!(v.badge().as_deref(), Some("3"));
        let help: Vec<&str> = v.help().iter().map(|(k, _)| *k).collect();
        for reserved in ["1", "[", "]", "?", ":", "q", "F5"] {
            assert!(!help.contains(&reserved));
        }
    }

    #[test]
    fn setup_and_limited_screens() {
        let (_d, ctx) = ctx();
        let mut v = PrsView::with_shared(state::tests::shared_with(FakeTransport::new()));
        lock(&v.shared).setup = Setup::NoRemotes;
        let s = screen(&mut v, &ctx, 100, 15);
        assert!(s.contains("This repository has no remotes"), "{s}");
        assert!(v.handle_key(key(KeyCode::Char('c')), &ctx).is_none());
        assert!(v.badge().is_none());
        assert!(matches!(palette_create(&ctx), Action::Error(_)));
        assert!(matches!(palette_repo_web(&ctx), Action::Error(_)));
        assert!(matches!(palette_sign_in(&ctx), Action::Error(_)));

        lock(&v.shared).setup = Setup::Limited {
            hosting: hosting_of(ProviderKind::Generic, "https://git.example.org/a/b"),
            reason: "No pull request API is known for git.example.org.".into(),
        };
        let s = screen(&mut v, &ctx, 120, 40);
        assert!(s.contains("Git server · git.example.org/a/b"), "{s}");
        assert!(s.contains("[[hosting.hosts]]"), "{s}");
        // Generic hosts have no refs or API: these keys do nothing / explain.
        assert!(v.handle_key(key(KeyCode::Char('c')), &ctx).is_none());
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('L')), &ctx),
            Action::Error(_)
        ));
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('N')), &ctx),
            Action::Error(_)
        ));
        assert_eq!(v.help().last().map(|(k, _)| *k), Some("R"));

        lock(&v.shared).setup = Setup::Limited {
            hosting: hosting_of(ProviderKind::Gitea, "https://codeberg.org/a/b"),
            reason: "Not found (HTTP 404)".into(),
        };
        v.sync();
        for c in ['c', 'd', 'L'] {
            assert!(
                matches!(v.handle_key(key(KeyCode::Char(c)), &ctx), Action::Popup(_)),
                "{c}"
            );
        }
    }

    #[test]
    fn palette_has_entries() {
        let names: Vec<&str> = palette().iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "Create pull request…",
                "View pull requests",
                "Checkout pull request…",
                "Open repository in browser",
                "Sign in to hosting provider…"
            ]
        );
        let (_d, ctx) = ctx();
        assert!(matches!(
            (palette()[1].run)(&ctx),
            Action::SwitchTab(Tab::PullRequests)
        ));
        ctx.repo
            .run(&["remote", "add", "origin", "git@gitlab.com:g/p.git"])
            .unwrap();
        assert!(matches!(palette_checkout(&ctx), Action::Popup(_)));
        assert!(matches!(palette_sign_in(&ctx), Action::Popup(_)));
    }
}
