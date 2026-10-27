//! Rendering of commit rows: column layout, ref labels and the colored graph.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::git::graph::GraphRow;
use crate::git::log::Commit;
use crate::git::Repo;
use crate::theme::Theme;
use crate::util::{fit, format_time, relative_time, truncate};

/// Widest graph shown, in lanes.
pub const MAX_LANES: usize = 16;

/// Local and remote branch names (short form), to tell `feature/x` from `origin/x` in decorations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefIndex {
    pub local: HashSet<String>,
    pub remote: HashSet<String>,
}

impl RefIndex {
    /// Parse `git for-each-ref --format=%(refname)` output.
    pub fn parse(out: &str) -> Self {
        let mut idx = RefIndex::default();
        for l in out.lines().map(str::trim) {
            if let Some(b) = l.strip_prefix("refs/heads/") {
                idx.local.insert(b.to_string());
            } else if let Some(r) = l.strip_prefix("refs/remotes/") {
                idx.remote.insert(r.to_string());
            }
        }
        idx
    }

    pub fn load(repo: &Repo) -> Self {
        repo.try_run(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ])
        .map(|o| Self::parse(&o))
        .unwrap_or_default()
    }
}

/// Kind of a ref label, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub kind: RefKind,
    pub text: String,
}

/// Classify decorations (as produced by [`crate::git::log::short_ref`]) into labels, HEAD first,
/// then local branches, remote branches and tags. Symbolic `origin/HEAD` refs are dropped.
pub fn labels(refs: &[String], idx: &RefIndex) -> Vec<RefLabel> {
    let mut out: Vec<RefLabel> = refs
        .iter()
        .filter_map(|r| {
            let (kind, text) = if r == "HEAD" {
                (RefKind::Head, "HEAD".to_string())
            } else if let Some(b) = r.strip_prefix("HEAD -> ") {
                (RefKind::Head, format!("HEAD→{b}"))
            } else if let Some(t) = r.strip_prefix("tag: ") {
                (RefKind::Tag, t.to_string())
            } else if idx.local.contains(r) {
                (RefKind::Local, r.clone())
            } else if r.ends_with("/HEAD") {
                // Symbolic remote HEAD (origin/HEAD): it always sits next to the branch it points to.
                return None;
            } else if idx.remote.contains(r) {
                (RefKind::Remote, r.clone())
            } else {
                (RefKind::Local, r.clone())
            };
            Some(RefLabel { kind, text })
        })
        .collect();
    out.sort_by_key(|l| l.kind);
    out
}

pub fn label_style(kind: RefKind, t: &Theme) -> Style {
    let fg = match kind {
        RefKind::Head => t.head,
        RefKind::Local => t.local_branch,
        RefKind::Remote => t.remote_branch,
        RefKind::Tag => t.tag,
    };
    Style::default().fg(fg).add_modifier(Modifier::BOLD)
}

/// Column widths of a commit row. Zero width means the column is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    /// Graph cells (each lane takes two cells)
    pub graph: usize,
    /// Lanes the graph may show before overflowing
    pub lanes: usize,
    pub subject: usize,
    pub author: usize,
    pub date: usize,
    pub hash: usize,
}

/// One cell for the mark / HEAD indicator at the start of every row.
pub const MARK_WIDTH: usize = 1;

/// Lay out a row of `width` cells. `graph_lanes` is the widest graph among the visible rows,
/// `date` and `hash` the widths their texts need. Narrow terminals drop author, then date, then hash;
/// the graph never takes more than a third of the row.
pub fn columns(width: usize, graph_lanes: usize, date: usize, hash: usize) -> Columns {
    let author = if width >= 72 {
        (width / 8).clamp(10, 20)
    } else {
        0
    };
    let date = if width >= 56 { date } else { 0 };
    let hash = if width >= 40 { hash } else { 0 };
    let lanes = graph_lanes
        .clamp(1, MAX_LANES)
        .min((width / 3).div_ceil(2).max(1));
    let graph = lanes * 2 - 1;
    let fixed = MARK_WIDTH
        + graph
        + 1
        + [author, date, hash]
            .iter()
            .map(|w| if *w > 0 { w + 1 } else { 0 })
            .sum::<usize>();
    Columns {
        graph,
        lanes,
        subject: width.saturating_sub(fixed),
        author,
        date,
        hash,
    }
}

/// Date column text.
pub fn date_text(ts: i64, absolute: bool) -> String {
    if absolute {
        format_time(ts)
    } else {
        relative_time(ts)
    }
}

/// Everything a row needs besides the commit itself.
pub struct RowStyle<'a> {
    pub theme: &'a Theme,
    pub refs: &'a RefIndex,
    pub cols: Columns,
    pub absolute_dates: bool,
    /// `user.email`, lowercase
    pub me: Option<&'a str>,
    pub head: Option<&'a str>,
}

/// Colored graph cells for one row, padded to `cols.graph`.
fn graph_spans(g: &GraphRow, rs: &RowStyle) -> Vec<Span<'static>> {
    let t = rs.theme;
    let mut spans: Vec<Span<'static>> = g
        .render(rs.cols.lanes)
        .into_iter()
        .map(|gl| {
            let style = match gl.color {
                Some(c) => Style::default().fg(t.graph[c % t.graph.len()]),
                None => t.dimmed(),
            };
            Span::styled(gl.ch.to_string(), style)
        })
        .collect();
    let used = spans.len();
    spans.push(Span::raw(
        " ".repeat(rs.cols.graph.saturating_sub(used) + 1),
    ));
    spans
}

/// Ref labels followed by the subject, fitted into `width` cells.
fn subject_spans(c: &Commit, rs: &RowStyle, width: usize, mine: bool) -> Vec<Span<'static>> {
    let t = rs.theme;
    let mut spans = Vec::new();
    let mut used = 0;
    let labels = labels(&c.refs, rs.refs);
    for (i, l) in labels.iter().enumerate() {
        let text = format!("[{}] ", l.text);
        let w = text.width();
        // Leave room for the subject: labels after the first take at most half of the column.
        if i > 0 && used + w > width / 2 {
            let more = format!("+{} ", labels.len() - i);
            used += more.width();
            spans.push(Span::styled(more, t.dimmed()));
            break;
        }
        used += w;
        spans.push(Span::styled(text, label_style(l.kind, t)));
    }
    let mut style = t.text();
    if mine {
        style = style.add_modifier(Modifier::BOLD);
    }
    if rs.head == Some(c.id.as_str()) {
        style = style.fg(t.head);
    }
    let rest = width.saturating_sub(used);
    spans.push(Span::styled(fit(&c.subject, rest), style));
    if used > width {
        // Labels alone overflow: cut them back to the column width.
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        return vec![Span::styled(fit(&truncate(&text, width), width), t.text())];
    }
    spans
}

/// One commit row as a full-width line.
pub fn commit_line(c: &Commit, g: &GraphRow, marked: bool, rs: &RowStyle) -> Line<'static> {
    let t = rs.theme;
    let mine = rs.me.is_some_and(|me| c.email.eq_ignore_ascii_case(me));
    let is_head = rs.head == Some(c.id.as_str());
    let mark = if marked {
        Span::styled(
            "▌",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )
    } else if is_head {
        Span::styled(
            "›",
            Style::default().fg(t.head).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(" ")
    };
    let mut spans = vec![mark];
    spans.extend(graph_spans(g, rs));
    spans.extend(subject_spans(c, rs, rs.cols.subject, mine));
    if rs.cols.author > 0 {
        let mut style = Style::default().fg(t.author);
        if mine {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(
            format!(" {}", fit(&c.author, rs.cols.author)),
            style,
        ));
    }
    if rs.cols.date > 0 {
        spans.push(Span::styled(
            format!(
                " {}",
                fit(&date_text(c.time, rs.absolute_dates), rs.cols.date)
            ),
            Style::default().fg(t.date),
        ));
    }
    if rs.cols.hash > 0 {
        spans.push(Span::styled(
            format!(" {}", fit(&c.short, rs.cols.hash)),
            Style::default().fg(t.hash),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::graph::layout;

    fn idx() -> RefIndex {
        RefIndex::parse("refs/heads/main\nrefs/heads/feature/x\nrefs/remotes/origin/main\nrefs/remotes/origin/HEAD\n")
    }

    #[test]
    fn ref_index_parsing() {
        let i = idx();
        assert!(i.local.contains("feature/x"));
        assert!(i.remote.contains("origin/main"));
        assert_eq!(i.local.len(), 2);
    }

    #[test]
    fn labels_are_classified_and_ordered() {
        let refs: Vec<String> = [
            "tag: v1",
            "origin/main",
            "HEAD -> main",
            "feature/x",
            "origin/HEAD",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let l = labels(&refs, &idx());
        let got: Vec<(RefKind, &str)> = l.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (RefKind::Head, "HEAD→main"),
                (RefKind::Local, "feature/x"),
                (RefKind::Remote, "origin/main"),
                (RefKind::Tag, "v1"),
            ]
        );
        let detached = labels(&["HEAD".to_string()], &idx());
        assert_eq!(detached[0].kind, RefKind::Head);
    }

    #[test]
    fn column_layout_adapts_to_width() {
        let c = columns(120, 3, 14, 7);
        assert_eq!(c.graph, 5);
        assert_eq!(c.author, 15);
        assert_eq!(c.date, 14);
        assert_eq!(c.hash, 7);
        assert_eq!(
            MARK_WIDTH + c.graph + 1 + c.subject + 1 + c.author + 1 + c.date + 1 + c.hash,
            120
        );

        let c = columns(70, 2, 14, 7);
        assert_eq!(c.author, 0);
        assert!(c.date > 0);
        assert_eq!(
            MARK_WIDTH + c.graph + 1 + c.subject + 1 + c.date + 1 + c.hash,
            70
        );

        let c = columns(30, 40, 14, 7);
        assert_eq!((c.author, c.date, c.hash), (0, 0, 0));
        assert!(c.lanes <= 5, "graph capped to a third of the width");
        assert_eq!(c.graph, c.lanes * 2 - 1);

        let c = columns(400, 100, 14, 7);
        assert_eq!(c.lanes, MAX_LANES);
        assert_eq!(columns(0, 0, 14, 7).subject, 0);
    }

    #[test]
    fn row_fills_the_width() {
        let t = Theme::dark();
        let refs = idx();
        let commit = Commit {
            id: "abc".into(),
            short: "abc".into(),
            parents: vec!["p".into()],
            author: "Ünïcödé Author".into(),
            email: "Me@Example.com".into(),
            time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            subject: "fix: 日本語 subject that is rather long".into(),
            refs: vec![
                "HEAD -> main".into(),
                "origin/main".into(),
                "tag: v1.0".into(),
            ],
        };
        let rows = layout(std::slice::from_ref(&commit));
        for width in [20usize, 50, 80, 140] {
            let rs = RowStyle {
                theme: &t,
                refs: &refs,
                cols: columns(width, 1, 16, 7),
                absolute_dates: true,
                me: Some("me@example.com"),
                head: Some("abc"),
            };
            let line = commit_line(&commit, &rows[0], true, &rs);
            assert_eq!(line.width(), width, "width {width}");
        }
    }
}
