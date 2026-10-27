//! Commit graph lane layout for the log view.
//!
//! [`layout`] walks commits in topological order (newest first, as `git log --topo-order` prints
//! them) and assigns every commit a *lane* (column). Each lane remembers which commit it is
//! waiting for, so a line can be drawn from a child down to its parent. Every commit produces one
//! [`GraphRow`] containing its node position and the edges crossing that row:
//!
//! - [`EdgeKind::PassThrough`]: a lane waiting for a commit further down passes this row (`│`)
//! - [`EdgeKind::MergeIn`]: a second lane that was also waiting for this commit joins the node (`─╯`)
//! - [`EdgeKind::ForkOut`]: an extra parent of a merge leaves the node into its own lane (`─╮`)
//!
//! Rows render to box-drawing glyphs with [`GraphRow::render`], which also caps the width and marks
//! overflowing lanes. Colors are lane indices into `theme.graph` (taken modulo its length).

use super::log::Commit;

/// Line leaves the cell upwards.
pub const UP: u8 = 1;
/// Line leaves the cell downwards.
pub const DOWN: u8 = 2;
/// Line leaves the cell to the left.
pub const LEFT: u8 = 4;
/// Line leaves the cell to the right.
pub const RIGHT: u8 = 8;

/// Glyph for an ordinary commit.
pub const NODE: char = '●';
/// Glyph for a merge commit (two or more parents).
pub const MERGE_NODE: char = '○';
/// Shown in the last column when lanes beyond the width cap carry lines.
pub const OVERFLOW: char = '…';

/// How a line crosses a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A lane continuing straight through the row.
    PassThrough,
    /// A lane from above that ends in this row's node (another child of this commit).
    MergeIn,
    /// A line from this row's node to the lane of one of its non-first parents.
    ForkOut,
}

/// One line segment in a row, identified by the lane it lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub kind: EdgeKind,
    pub lane: usize,
    /// Color index (lane color; take it modulo the palette size)
    pub color: usize,
}

/// One rendered character with an optional color index (`None` = neutral/dim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub ch: char,
    pub color: Option<usize>,
}

/// Layout of one commit's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit's node
    pub node: usize,
    /// Color index of the node's lane
    pub color: usize,
    /// The commit has two or more parents
    pub merge: bool,
    pub edges: Vec<Edge>,
    /// Number of lanes this row spans
    pub width: usize,
}

#[derive(Debug, Clone)]
struct Lane {
    /// Commit this lane is waiting for
    target: String,
    color: usize,
}

/// Lay out `commits` (topological order, children before parents). Parents that are not part of
/// `commits` (beyond the loaded page, or filtered out) simply keep their lane open to the bottom.
pub fn layout(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        rows.push(layout_row(&mut lanes, &mut next_color, c));
    }
    rows
}

fn new_color(next: &mut usize) -> usize {
    let c = *next;
    *next += 1;
    c
}

/// First slot that is empty both before and after this row (so a freed lane is never reused in the
/// same row, which would make a merge-in and a fork-out look like one line).
fn free_slot(before: &[Option<Lane>], after: &[Option<Lane>], taken: Option<usize>) -> usize {
    (0..after.len().max(before.len()))
        .find(|&i| {
            Some(i) != taken
                && matches!(before.get(i), None | Some(None))
                && matches!(after.get(i), None | Some(None))
        })
        .unwrap_or_else(|| {
            after
                .len()
                .max(before.len())
                .max(taken.map_or(0, |t| t + 1))
        })
}

fn set_lane(lanes: &mut Vec<Option<Lane>>, i: usize, lane: Option<Lane>) {
    if lanes.len() <= i {
        lanes.resize(i + 1, None);
    }
    lanes[i] = lane;
}

fn layout_row(lanes: &mut Vec<Option<Lane>>, next_color: &mut usize, c: &Commit) -> GraphRow {
    let before = lanes.clone();
    let matches: Vec<usize> = before
        .iter()
        .enumerate()
        .filter(|(_, l)| l.as_ref().is_some_and(|l| l.target == c.id))
        .map(|(i, _)| i)
        .collect();
    let (node, color) = match matches.first() {
        Some(&i) => (i, before[i].as_ref().map(|l| l.color).unwrap_or(0)),
        None => {
            let i = free_slot(&before, &before, None);
            (i, new_color(next_color))
        }
    };

    let mut after = before.clone();
    for &m in &matches {
        after[m] = None;
    }
    let first_parent = c.parents.first().map(|p| Lane {
        target: p.clone(),
        color,
    });
    set_lane(&mut after, node, first_parent);

    let mut edges: Vec<Edge> = before
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let l = l.as_ref()?;
            (!matches.contains(&i)).then_some(Edge {
                kind: EdgeKind::PassThrough,
                lane: i,
                color: l.color,
            })
        })
        .collect();
    edges.extend(matches.iter().skip(1).map(|&i| Edge {
        kind: EdgeKind::MergeIn,
        lane: i,
        color: before[i].as_ref().map(|l| l.color).unwrap_or(color),
    }));

    for p in c.parents.iter().skip(1) {
        let existing = after
            .iter()
            .enumerate()
            .find(|(i, l)| *i != node && l.as_ref().is_some_and(|l| l.target == *p))
            .map(|(i, l)| (i, l.as_ref().map(|l| l.color).unwrap_or(0)));
        let (lane, lane_color) = match existing {
            Some(found) => found,
            None => {
                let i = free_slot(&before, &after, Some(node));
                let col = new_color(next_color);
                set_lane(
                    &mut after,
                    i,
                    Some(Lane {
                        target: p.clone(),
                        color: col,
                    }),
                );
                (i, col)
            }
        };
        edges.push(Edge {
            kind: EdgeKind::ForkOut,
            lane,
            color: lane_color,
        });
    }

    while after.last().is_some_and(Option::is_none) {
        after.pop();
    }
    let used = |v: &[Option<Lane>]| v.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
    let width = used(&before).max(used(&after)).max(node + 1);
    *lanes = after;
    GraphRow {
        node,
        color,
        merge: c.parents.len() > 1,
        edges,
        width,
    }
}

/// Box-drawing character for a set of line directions.
pub fn glyph_for(mask: u8) -> char {
    const UD: u8 = UP | DOWN;
    const LR: u8 = LEFT | RIGHT;
    match mask {
        0 => ' ',
        m if m == UD || m == UP || m == DOWN => '│',
        m if m == LR || m == LEFT || m == RIGHT => '─',
        m if m == UD | LR => '┼',
        m if m == DOWN | LEFT => '╮',
        m if m == DOWN | RIGHT => '╭',
        m if m == UP | LEFT => '╯',
        m if m == UP | RIGHT => '╰',
        m if m == UD | RIGHT => '├',
        m if m == UD | LEFT => '┤',
        m if m == LR | DOWN => '┬',
        m if m == LR | UP => '┴',
        _ => '┼',
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    mask: u8,
    color: Option<usize>,
}

impl Cell {
    fn add(&mut self, mask: u8, color: usize) {
        self.mask |= mask;
        if self.color.is_none() {
            self.color = Some(color);
        }
    }
}

impl GraphRow {
    /// Number of character cells the full row occupies (lanes plus connectors between them).
    pub fn cell_width(&self) -> usize {
        (self.width * 2).saturating_sub(1)
    }

    fn cells(&self) -> Vec<Cell> {
        let mut cells = vec![Cell::default(); self.cell_width().max(1)];
        let node_cell = self.node * 2;
        // Vertical lines first so crossings keep the color of the lane they belong to.
        let order = [EdgeKind::PassThrough, EdgeKind::MergeIn, EdgeKind::ForkOut];
        for kind in order {
            for e in self.edges.iter().filter(|e| e.kind == kind) {
                let at = e.lane * 2;
                if at >= cells.len() {
                    continue;
                }
                let vertical = match kind {
                    EdgeKind::PassThrough => {
                        cells[at].add(UP | DOWN, e.color);
                        continue;
                    }
                    EdgeKind::MergeIn => UP,
                    EdgeKind::ForkOut => DOWN,
                };
                let toward_node = if at > node_cell { LEFT } else { RIGHT };
                cells[at].add(vertical | toward_node, e.color);
                let (lo, hi) = (at.min(node_cell), at.max(node_cell));
                for cell in cells.iter_mut().take(hi).skip(lo + 1) {
                    cell.add(LEFT | RIGHT, e.color);
                }
                cells[node_cell].add(if at > node_cell { RIGHT } else { LEFT }, e.color);
            }
        }
        cells[node_cell].color = Some(self.color);
        cells
    }

    /// Render the row as glyphs, at most `max_lanes` lanes wide (always `2 * width - 1` cells,
    /// capped). When lanes are cut off, the last column shows the node if it is hidden, or
    /// [`OVERFLOW`] when hidden lanes carry lines.
    pub fn render(&self, max_lanes: usize) -> Vec<Glyph> {
        let max_lanes = max_lanes.max(1);
        let node_ch = if self.merge { MERGE_NODE } else { NODE };
        let cells = self.cells();
        let to_glyph = |i: usize, c: &Cell| Glyph {
            ch: if i == self.node * 2 {
                node_ch
            } else {
                glyph_for(c.mask)
            },
            color: if c.mask == 0 && i != self.node * 2 {
                None
            } else {
                c.color
            },
        };
        if self.width <= max_lanes {
            return cells
                .iter()
                .enumerate()
                .map(|(i, c)| to_glyph(i, c))
                .collect();
        }
        // Visible lanes 0..max_lanes-1, the connector after the last of them, then the overflow column.
        let keep = (2 * (max_lanes - 1)).saturating_sub(1);
        let mut out: Vec<Glyph> = cells
            .iter()
            .take(keep)
            .enumerate()
            .map(|(i, c)| to_glyph(i, c))
            .collect();
        if max_lanes >= 2 {
            let conn = cells[keep];
            out.push(Glyph {
                ch: glyph_for(conn.mask),
                color: if conn.mask == 0 { None } else { conn.color },
            });
        }
        let overflow = if self.node >= max_lanes - 1 {
            Glyph {
                ch: node_ch,
                color: Some(self.color),
            }
        } else if cells.iter().skip(keep + 1).any(|c| c.mask != 0) {
            Glyph {
                ch: OVERFLOW,
                color: None,
            }
        } else {
            Glyph {
                ch: ' ',
                color: None,
            }
        };
        out.push(overflow);
        out
    }

    /// The rendered row as a plain string, trailing blanks removed.
    #[cfg(test)]
    pub fn to_text(&self, max_lanes: usize) -> String {
        self.render(max_lanes)
            .iter()
            .map(|g| g.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.to_string(),
            short: id.to_string(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            subject: id.to_string(),
            refs: Vec::new(),
        }
    }

    fn texts(commits: &[Commit], max: usize) -> Vec<String> {
        layout(commits).iter().map(|r| r.to_text(max)).collect()
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let commits = [c("c", &["b"]), c("b", &["a"]), c("a", &[])];
        let rows = layout(&commits);
        assert!(rows
            .iter()
            .all(|r| r.node == 0 && r.width == 1 && r.edges.is_empty()));
        assert!(rows.iter().all(|r| r.color == rows[0].color));
        assert_eq!(texts(&commits, 10), vec!["●", "●", "●"]);
    }

    #[test]
    fn branch_and_merge() {
        // m merges f into x; both come from base.
        let commits = [
            c("m", &["x", "f"]),
            c("x", &["base"]),
            c("f", &["base"]),
            c("base", &[]),
        ];
        let rows = layout(&commits);
        assert_eq!(texts(&commits, 10), vec!["○─╮", "● │", "│ ●", "●─╯"]);
        assert!(rows[0].merge);
        assert_eq!(
            rows[0].edges,
            vec![Edge {
                kind: EdgeKind::ForkOut,
                lane: 1,
                color: 1
            }]
        );
        assert_eq!(
            rows[1].edges,
            vec![Edge {
                kind: EdgeKind::PassThrough,
                lane: 1,
                color: 1
            }]
        );
        assert_eq!(rows[2].node, 1);
        assert_eq!(rows[2].color, 1);
        assert_eq!(
            rows[3].edges,
            vec![Edge {
                kind: EdgeKind::MergeIn,
                lane: 1,
                color: 1
            }]
        );
        assert_eq!(rows[3].width, 2);
    }

    #[test]
    fn octopus_merge_forks_into_three_lanes() {
        let commits = [
            c("o", &["a", "b", "d"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("d", &["r"]),
            c("r", &[]),
        ];
        let t = texts(&commits, 10);
        assert_eq!(t[0], "○─┬─╮");
        assert_eq!(t[1], "● │ │");
        assert_eq!(t[2], "│ ● │");
        assert_eq!(t[3], "│ │ ●");
        assert_eq!(t[4], "●─┴─╯");
        let rows = layout(&commits);
        let colors: Vec<usize> = rows[0].edges.iter().map(|e| e.color).collect();
        assert_eq!(colors, vec![1, 2]);
    }

    #[test]
    fn multiple_roots_and_unrelated_tips() {
        // Two independent histories interleaved (like `--all` with an orphan branch).
        let commits = [
            c("x2", &["x1"]),
            c("y2", &["y1"]),
            c("x1", &[]),
            c("y1", &[]),
        ];
        let rows = layout(&commits);
        assert_eq!(
            rows.iter().map(|r| r.node).collect::<Vec<_>>(),
            vec![0, 1, 0, 1]
        );
        assert_ne!(rows[0].color, rows[1].color);
        assert_eq!(texts(&commits, 10), vec!["●", "│ ●", "● │", "  ●"]);
        // After both roots, all lanes are free again: a new tip starts at lane 0.
        let more = [c("x1", &[]), c("z", &[])];
        assert_eq!(layout(&more)[1].node, 0);
    }

    #[test]
    fn parents_outside_the_page_keep_their_lane_open() {
        let commits = [c("m", &["a", "zzz"]), c("a", &["outside"])];
        let rows = layout(&commits);
        assert_eq!(
            rows[1].edges,
            vec![Edge {
                kind: EdgeKind::PassThrough,
                lane: 1,
                color: 1
            }]
        );
        assert_eq!(texts(&commits, 10), vec!["○─╮", "● │"]);
    }

    #[test]
    fn siblings_sharing_a_parent_converge_at_it() {
        let commits = [c("a", &["p"]), c("b", &["p"]), c("p", &[])];
        assert_eq!(texts(&commits, 10), vec!["●", "│ ●", "●─╯"]);
    }

    #[test]
    fn merge_into_existing_lane_uses_tee() {
        // `m`'s second parent `b` is already awaited by lane 1 (from tip `t`).
        let commits = [
            c("t", &["b"]),
            c("m", &["a", "b"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("r", &[]),
        ];
        let t = texts(&commits, 10);
        assert_eq!(t, vec!["●", "├─○", "│ ●", "● │", "●─╯"]);
        let rows = layout(&commits);
        assert_eq!(rows[1].node, 1);
        assert_eq!(
            rows[1]
                .edges
                .iter()
                .filter(|e| e.kind == EdgeKind::ForkOut)
                .count(),
            1
        );
        // The fork-out reuses lane 0's color, the lane it joins.
        assert_eq!(rows[1].edges.last().map(|e| e.color), Some(rows[0].color));
    }

    #[test]
    fn fork_out_to_the_left_and_crossings() {
        // Lane 0 is free when `m` (lane 1) forks its second parent, so the fork goes left.
        let commits = [
            c("a", &["r"]),
            c("m", &["p1", "p2"]),
            c("r", &[]),
            c("p1", &[]),
            c("p2", &[]),
        ];
        let t = texts(&commits, 10);
        assert_eq!(t[0], "●");
        assert_eq!(t[1], "│ ○─╮");
        assert_eq!(t[2], "● │ │");
        assert_eq!(t[3], "  ● │");
        assert_eq!(t[4], "    ●");
        // A free lane left of a merge node: the fork turns down on the left side.
        let commits = [
            c("a", &["r"]),
            c("b", &["m"]),
            c("r", &[]),
            c("m", &["p", "w"]),
        ];
        let t = texts(&commits, 10);
        assert_eq!(t, vec!["●", "│ ●", "● │", "╭─○"]);
        // Pass-through lanes left of the node, fork to a new lane on the right.
        let commits = [c("y", &["q"]), c("z", &["q2"]), c("m", &["p", "w"])];
        assert_eq!(texts(&commits, 10)[2], "│ │ ○─╮");
    }

    #[test]
    fn horizontal_line_crosses_pass_through_lanes() {
        // Lane 1 waits for `q`; merge `m` at lane 0 forks into new lane 2, crossing lane 1.
        let commits = [
            c("m0", &["m"]),
            c("y", &["q"]),
            c("m", &["a", "b"]),
            c("a", &[]),
            c("b", &[]),
            c("q", &[]),
        ];
        let rows = layout(&commits);
        assert_eq!(rows[2].node, 0);
        assert_eq!(rows[2].to_text(10), "○─┼─╮");
        let g = rows[2].render(10);
        // The crossing keeps the color of the vertical lane it belongs to.
        assert_eq!(g[2].color, Some(rows[1].color));
        assert_eq!(g[4].color, g[3].color);
    }

    #[test]
    fn width_cap_shows_overflow() {
        // Five independent tips, then their roots: five lanes at the widest point.
        let mut commits: Vec<Commit> = (0..5)
            .map(|i| c(&format!("t{i}"), &[&format!("r{i}")]))
            .collect();
        commits.extend((0..5).map(|i| c(&format!("r{i}"), &[])));
        let rows = layout(&commits);
        assert_eq!(rows[4].width, 5);
        assert_eq!(rows[4].to_text(10), "│ │ │ │ ●");
        // Capped to 3 lanes: two real lanes, then the overflow column holds the hidden node.
        assert_eq!(rows[4].to_text(3), "│ │ ●");
        assert_eq!(rows[4].render(3).len(), 5);
        // Node visible, hidden lanes still carry lines → ellipsis.
        assert_eq!(rows[5].to_text(3), "● │ …");
        // Everything always fits the cap.
        assert!(rows.iter().all(|r| r.render(3).len() <= 5));
        assert_eq!(rows[9].to_text(1), "●");
    }

    #[test]
    fn overflow_continues_horizontal_lines() {
        let commits = [c("o", &["a", "b", "d", "e"])];
        let row = &layout(&commits)[0];
        assert_eq!(row.to_text(10), "○─┬─┬─╮");
        assert_eq!(row.to_text(2), "○─…");
    }

    #[test]
    fn glyphs_cover_all_masks() {
        assert_eq!(glyph_for(UP | DOWN), '│');
        assert_eq!(glyph_for(LEFT | RIGHT), '─');
        assert_eq!(glyph_for(UP | DOWN | LEFT | RIGHT), '┼');
        assert_eq!(glyph_for(DOWN | LEFT), '╮');
        assert_eq!(glyph_for(DOWN | RIGHT), '╭');
        assert_eq!(glyph_for(UP | LEFT), '╯');
        assert_eq!(glyph_for(UP | RIGHT), '╰');
        assert_eq!(glyph_for(UP | DOWN | RIGHT), '├');
        assert_eq!(glyph_for(UP | DOWN | LEFT), '┤');
        assert_eq!(glyph_for(LEFT | RIGHT | DOWN), '┬');
        assert_eq!(glyph_for(LEFT | RIGHT | UP), '┴');
        assert_eq!(glyph_for(0), ' ');
    }

    #[test]
    fn empty_input() {
        assert!(layout(&[]).is_empty());
    }
}
