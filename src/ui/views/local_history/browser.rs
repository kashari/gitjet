//! Shared pieces of the Local History tab and the per-file popup: the snapshot list, the
//! "snapshot vs current file" preview, and the actions on a snapshot.

use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::diff::{DiffOptions, FileDiff};
use crate::git::local_history::{self, Snapshot};
use crate::git::Repo;
use crate::ui::bg::Preview;
use crate::ui::diff_view::{self, diff_height, draw_diff, DiffSource};
use crate::ui::popups::{ConfirmPopup, InputPopup};
use crate::ui::widgets::{move_sel, panel, scroll_to_keep_visible};
use crate::util::{format_time, relative_time, truncate};

/// What the preview pane shows for the selected snapshot.
#[derive(Debug, Clone)]
pub enum PreviewContent {
    /// Snapshot (left) vs the current file (right)
    Diff {
        diffs: Vec<FileDiff>,
        file_exists: bool,
    },
    /// Explanation instead of a diff (deletion entries, labels, errors)
    Message(String),
}

/// Identity of a snapshot, to know when the cached preview is stale.
type SnapKey = (String, Option<String>, i64);

fn key_of(s: &Snapshot) -> SnapKey {
    (s.path.clone(), s.hash.clone(), s.time)
}

/// How long the selection must rest on a snapshot before its diff is computed (holding an arrow
/// key skips the entries passed over).
const PREVIEW_DELAY: Duration = Duration::from_millis(60);

/// A selectable list of snapshots plus a diff preview of the selected one, computed in the
/// background (`git diff --no-index` is a process per snapshot).
pub struct SnapshotPane {
    pub snaps: Vec<Snapshot>,
    pub sel: usize,
    offset: usize,
    previews: Preview<SnapKey, PreviewContent>,
    /// Snapshot whose preview is shown (or requested).
    preview_key: Option<SnapKey>,
    preview: Option<PreviewContent>,
    pub diff_scroll: usize,
    diff_page: usize,
}

impl Default for SnapshotPane {
    fn default() -> Self {
        Self {
            snaps: Vec::new(),
            sel: 0,
            offset: 0,
            previews: Preview::new(PREVIEW_DELAY, 32),
            preview_key: None,
            preview: None,
            diff_scroll: 0,
            diff_page: 0,
        }
    }
}

impl SnapshotPane {
    /// Replace the list; keeps the selection on the same snapshot when it is still present.
    pub fn set(&mut self, snaps: Vec<Snapshot>) {
        let keep = self.selected().map(key_of);
        self.snaps = snaps;
        self.sel = keep
            .and_then(|k| self.snaps.iter().position(|s| key_of(s) == k))
            .unwrap_or(0);
        self.sel = self.sel.min(self.snaps.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&Snapshot> {
        self.snaps.get(self.sel)
    }

    /// Forget the cached previews (after a refresh the work-tree file may have changed).
    pub fn invalidate(&mut self) {
        self.previews.invalidate();
        self.preview_key = None;
        self.preview = None;
    }

    pub fn move_by(&mut self, delta: isize) {
        let before = self.sel;
        self.sel = move_sel(self.sel, delta, self.snaps.len());
        if self.sel != before {
            self.diff_scroll = 0;
        }
    }

    pub fn home(&mut self) {
        self.sel = 0;
        self.diff_scroll = 0;
    }

    pub fn end(&mut self) {
        self.sel = self.snaps.len().saturating_sub(1);
        self.diff_scroll = 0;
    }

    pub fn scroll_diff(&mut self, delta: isize) {
        let total = match &self.preview {
            Some(PreviewContent::Diff { diffs, .. }) => diff_height(diffs),
            _ => 0,
        };
        let next = (self.diff_scroll as isize + delta).max(0) as usize;
        self.diff_scroll = next.min(total.saturating_sub(1));
    }

    /// Lines in one page of the diff preview (as last drawn).
    pub fn diff_page(&self) -> isize {
        self.diff_page.max(1) as isize
    }

    /// Request the selected snapshot's preview if the selection changed, and pick it up once it
    /// has been computed. Markers and deletions need no git and show at once.
    fn ensure_preview(&mut self, ctx: &Ctx) {
        let key = self.selected().map(key_of);
        if key != self.preview_key {
            self.preview_key = key.clone();
            self.preview = None;
            if let Some(s) = self.selected().cloned() {
                if let Some(instant) = instant_preview(&s) {
                    self.preview = Some(instant);
                    return;
                }
                let opts = DiffOptions::from_config(&ctx.config.diff);
                self.previews
                    .request(&ctx.repo, key.expect("selected"), move |repo| {
                        snapshot_preview(repo, &s, &opts)
                    });
            }
        }
        if self.preview.is_none() {
            if let Some(k) = &self.preview_key {
                self.preview = self.previews.get(k);
            }
        }
    }

    /// True while the selected snapshot's diff is still being computed.
    pub fn preview_loading(&self) -> bool {
        self.preview_key.is_some() && self.preview.is_none()
    }

    /// The loaded diffs of the selection (for "show full diff"), if they have arrived.
    pub fn preview_diffs(&mut self, ctx: &Ctx) -> Option<Vec<FileDiff>> {
        self.ensure_preview(ctx);
        match &self.preview {
            Some(PreviewContent::Diff { diffs, .. }) => Some(diffs.clone()),
            _ => None,
        }
    }

    /// Draw the list into `area` (with border). `show_path` adds the file path (timeline mode).
    pub fn draw_list(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        focused: bool,
        show_path: bool,
        ctx: &Ctx,
    ) {
        let t = &ctx.theme;
        let block = panel(format!(" {title} ({}) ", self.snaps.len()), focused, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if self.snaps.is_empty() {
            f.render_widget(Paragraph::new("No snapshots yet").style(t.dimmed()), inner);
            return;
        }
        let h = inner.height as usize;
        self.offset = scroll_to_keep_visible(self.sel, self.offset, h);
        let w = inner.width as usize;
        let lines: Vec<Line> = self
            .snaps
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(h)
            .map(|(i, s)| {
                let line = snapshot_line(s, show_path, w, ctx);
                if i == self.sel {
                    line.style(t.selected())
                } else {
                    line
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// Draw the preview of the selected snapshot into `area` (with border).
    pub fn draw_preview(&mut self, f: &mut Frame, area: Rect, focused: bool, ctx: &Ctx) {
        self.ensure_preview(ctx);
        let t = &ctx.theme;
        let title = match self.selected() {
            Some(s) if !s.is_marker() => format!(" {} — snapshot vs current ", s.path),
            _ => " Preview ".to_string(),
        };
        let block = panel(title, focused, t);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.diff_page = inner.height.saturating_sub(1) as usize;
        match &self.preview {
            None if self.preview_loading() => {
                f.render_widget(Paragraph::new("Loading…").style(t.dimmed()), inner)
            }
            None => f.render_widget(Paragraph::new("Select a snapshot").style(t.dimmed()), inner),
            Some(PreviewContent::Message(m)) => {
                f.render_widget(Paragraph::new(m.as_str()).style(t.dimmed()), inner)
            }
            Some(PreviewContent::Diff { diffs, file_exists }) => {
                let [note, body] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
                let text = if !file_exists {
                    Span::styled(
                        "The file no longer exists; R restores this version",
                        Style::default().fg(t.warning),
                    )
                } else if diffs.is_empty() {
                    Span::styled("Identical to the current file", t.dimmed())
                } else {
                    Span::styled("- snapshot   + current file", t.dimmed())
                };
                f.render_widget(Line::from(text), note);
                if !diffs.is_empty() {
                    self.diff_scroll = self.diff_scroll.min(diff_height(diffs).saturating_sub(1));
                    draw_diff(f, body, diffs, self.diff_scroll, ctx);
                }
            }
        }
    }
}

/// One list row: absolute time, relative time, label / deletion marker, optionally the path.
fn snapshot_line(s: &Snapshot, show_path: bool, width: usize, ctx: &Ctx) -> Line<'static> {
    let t = &ctx.theme;
    let mut spans = vec![
        Span::styled(
            format!(" {} ", format_time(s.time)),
            Style::default().fg(t.date),
        ),
        Span::styled(format!("{:<15}", relative_time(s.time)), t.dimmed()),
    ];
    if s.is_marker() {
        spans.push(Span::styled(
            format!(" ◆ {}", s.label.as_deref().unwrap_or("")),
            Style::default().fg(t.tag),
        ));
        return Line::from(spans);
    }
    if s.is_deletion() {
        spans.push(Span::styled(" deleted", Style::default().fg(t.removed)));
    }
    if let Some(l) = &s.label {
        spans.push(Span::styled(format!(" ◆ {l}"), Style::default().fg(t.tag)));
    }
    if show_path {
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let room = width.saturating_sub(used + 2);
        spans.push(Span::styled(
            format!("  {}", truncate(&s.path, room)),
            t.text(),
        ));
    }
    Line::from(spans)
}

/// The preview of a snapshot that needs no git (labels and deletions), or None for a diff.
fn instant_preview(s: &Snapshot) -> Option<PreviewContent> {
    if s.is_marker() {
        return Some(PreviewContent::Message(format!(
            "Label \"{}\" put {} ({}).\nFiles changed at that moment carry the same label.",
            s.label.as_deref().unwrap_or(""),
            relative_time(s.time),
            format_time(s.time)
        )));
    }
    if s.hash.is_none() {
        return Some(PreviewContent::Message(
            "The file was deleted at this point.\nReverting to this entry deletes the current file (its content is saved first).".into(),
        ));
    }
    None
}

/// Compute the preview for one snapshot (background thread).
fn snapshot_preview(repo: &Repo, s: &Snapshot, opts: &DiffOptions) -> PreviewContent {
    if let Some(p) = instant_preview(s) {
        return p;
    }
    let hash = s.hash.as_deref().unwrap_or_default();
    match snapshot_diff(repo, &s.path, hash, opts) {
        Ok(p) => p,
        Err(e) => PreviewContent::Message(format!("Could not load the snapshot: {e:#}")),
    }
}

fn snapshot_diff(
    repo: &Repo,
    path: &str,
    hash: &str,
    opts: &DiffOptions,
) -> anyhow::Result<PreviewContent> {
    let left = local_history::object_path(repo, hash)?;
    if !left.exists() {
        anyhow::bail!("the stored content was pruned");
    }
    let current = repo.path(path);
    let file_exists = current.is_file();
    let right = if file_exists {
        current
    } else {
        local_history::empty_file(repo)?
    };
    let mut diffs = repo.diff_files(&left.to_string_lossy(), &right.to_string_lossy(), opts)?;
    for d in diffs.iter_mut() {
        d.old_path = Some(path.to_string());
        d.new_path = Some(path.to_string());
    }
    Ok(PreviewContent::Diff { diffs, file_exists })
}

/// "Revert to this snapshot" with a danger confirmation.
pub fn revert_action(snap: &Snapshot) -> Action {
    if snap.is_marker() {
        return Action::Toast("Select a file snapshot to revert".into());
    }
    let path = snap.path.clone();
    let hash = snap.hash.clone();
    let what = match &hash {
        Some(_) => format!(
            "Replace the current content of {path} with the snapshot from {}?",
            format_time(snap.time)
        ),
        None => format!(
            "Delete {path} (it was deleted at {})?",
            format_time(snap.time)
        ),
    };
    let msg = format!(
        "{what}\n\nThe current content is saved to local history first, so this can be undone."
    );
    Action::popup(
        ConfirmPopup::new("Revert to snapshot", msg, move |ctx| {
            let r = local_history::restore(
                &ctx.repo,
                &ctx.config.local_history,
                &path,
                hash.as_deref(),
            );
            Action::from_result(r, format!("Reverted {path}"))
        })
        .danger(),
    )
}

/// "Put Label…": prompt, then snapshot all changed files under that label in the background.
pub fn label_action() -> Action {
    Action::popup(
        InputPopup::new("Put label", "", |label, ctx| {
            let cfg = ctx.config.local_history.clone();
            Action::spawn("Put local history label", move |repo| {
                let n = local_history::snapshot_with_label(repo, &cfg, &label)?;
                Ok(format!(
                    "Label \"{}\" put ({n} file{})",
                    label.trim(),
                    if n == 1 { "" } else { "s" }
                ))
            })
        })
        .prompt("Marks the current state of all changed files in local history"),
    )
}

/// "Snapshot now" for every changed file, in the background.
pub fn snapshot_now_action(ctx: &Ctx) -> Action {
    let cfg = ctx.config.local_history.clone();
    Action::spawn("Local history snapshot", move |repo| {
        let n = local_history::snapshot_changed(repo, &cfg)?;
        Ok(match n {
            0 => "No changes since the last snapshot".to_string(),
            1 => "1 file snapshotted".to_string(),
            n => format!("{n} files snapshotted"),
        })
    })
}

/// Full-screen diff of the selected snapshot vs the current file. Uses the computed preview when
/// it has arrived; otherwise the diff viewer loads the two files itself (in the background).
pub fn full_diff_action(pane: &mut SnapshotPane, ctx: &Ctx) -> Action {
    let Some(s) = pane.selected().cloned() else {
        return Action::None;
    };
    let title = format!("{} — snapshot {} vs current", s.path, format_time(s.time));
    if instant_preview(&s).is_some() {
        return Action::Toast("Nothing to compare for this entry".into());
    }
    match pane.preview_diffs(ctx) {
        Some(d) if !d.is_empty() => diff_view::open(title, DiffSource::Raw(d)),
        Some(_) => Action::Toast("The snapshot is identical to the current file".into()),
        None => {
            let repo = &ctx.repo;
            let hash = s.hash.as_deref().unwrap_or_default();
            let left = match local_history::object_path(repo, hash) {
                Ok(p) if p.exists() => p,
                _ => return Action::Error("The stored content was pruned".into()),
            };
            let current = repo.path(&s.path);
            let right = if current.is_file() {
                current
            } else {
                match local_history::empty_file(repo) {
                    Ok(p) => p,
                    Err(e) => return Action::Error(format!("{e:#}")),
                }
            };
            diff_view::open(title, DiffSource::Files { left, right })
        }
    }
}
