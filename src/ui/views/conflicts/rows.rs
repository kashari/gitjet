//! Aligned three-pane layout for the merge resolver: every chunk occupies the same rows in the
//! "Yours", "Result" and "Theirs" panes, so one scroll offset keeps all panes in sync.

use unicode_width::UnicodeWidthChar;

use crate::git::conflict::MergeModel;

/// A line shown in one pane: index into the chunk's lines for that pane, and its 1-based file line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub idx: usize,
    pub line_no: usize,
}

/// One screen row across the three panes. `None` cells are padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub chunk: usize,
    /// First row of its chunk (where markers are drawn).
    pub first: bool,
    pub ours: Option<Cell>,
    pub result: Option<Cell>,
    pub theirs: Option<Cell>,
}

/// The rows of a model plus the first row of every chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<Row>,
    /// `chunk_start[i]` is the first row of chunk `i`; `chunk_start[len]` is the total row count.
    pub chunk_start: Vec<usize>,
}

impl Rows {
    /// Rows `[start, end)` of chunk `i`.
    pub fn chunk_rows(&self, i: usize) -> (usize, usize) {
        let start = self.chunk_start.get(i).copied().unwrap_or(0);
        let end = self.chunk_start.get(i + 1).copied().unwrap_or(start);
        (start, end)
    }

    /// Result-pane line number (1-based) where chunk `i` starts.
    pub fn result_line_of(&self, i: usize) -> usize {
        let (start, end) = self.chunk_rows(i);
        let mut last = 0;
        for r in &self.rows[..start.min(self.rows.len())] {
            if let Some(c) = r.result {
                last = c.line_no;
            }
        }
        self.rows[start.min(self.rows.len())..end.min(self.rows.len())]
            .iter()
            .find_map(|r| r.result.map(|c| c.line_no))
            .unwrap_or(last + 1)
    }
}

fn cell(i: usize, len: usize, counter: &mut usize) -> Option<Cell> {
    (i < len).then(|| {
        *counter += 1;
        Cell {
            idx: i,
            line_no: *counter,
        }
    })
}

/// Lay out `model`: each chunk is as tall as its longest pane (at least one row for changes).
pub fn build(model: &MergeModel) -> Rows {
    let mut out = Rows::default();
    let (mut no_o, mut no_r, mut no_t) = (0, 0, 0);
    for (ci, c) in model.chunks.iter().enumerate() {
        out.chunk_start.push(out.rows.len());
        let (o, r, t) = (c.ours.len(), c.result_lines().len(), c.theirs.len());
        let mut h = o.max(r).max(t);
        if c.is_change() {
            h = h.max(1);
        }
        for i in 0..h {
            out.rows.push(Row {
                chunk: ci,
                first: i == 0,
                ours: cell(i, o, &mut no_o),
                result: cell(i, r, &mut no_r),
                theirs: cell(i, t, &mut no_t),
            });
        }
    }
    out.chunk_start.push(out.rows.len());
    out
}

/// Display `s` from column `skip`, at most `width` columns wide (tabs expand to 4 spaces).
pub fn clip(s: &str, skip: usize, width: usize) -> String {
    let mut out = String::new();
    let (mut col, mut used) = (0usize, 0usize);
    let expanded = s.replace('\t', "    ");
    for ch in expanded.chars() {
        let w = ch.width().unwrap_or(0);
        if col < skip {
            col += w;
            continue;
        }
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
        col += w;
    }
    out
}

/// Row offset that shows `target` about a third of the way down a viewport of `height` rows.
pub fn scroll_for(target: usize, scroll: usize, height: usize) -> usize {
    if height == 0 || (target >= scroll && target < scroll + height) {
        return scroll;
    }
    target.saturating_sub(height / 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::conflict::Resolution;

    #[test]
    fn chunks_align_across_panes() {
        // ours replaces one line by three, theirs deletes it.
        let m = MergeModel::new("a\nb\nc\n", "a\nx\ny\nz\nc\n", "a\nc\n");
        let rows = build(&m);
        assert_eq!(rows.rows.len(), 5);
        let conflict = m.chunks.iter().position(|c| c.is_conflict()).unwrap();
        let (s, e) = rows.chunk_rows(conflict);
        assert_eq!((s, e), (1, 4));
        assert!(rows.rows[s].first && !rows.rows[s + 1].first);
        // ours has 3 lines, theirs none, result (base while unresolved) one.
        assert!(rows.rows[s..e]
            .iter()
            .all(|r| r.ours.is_some() && r.theirs.is_none()));
        assert_eq!(
            rows.rows[s..e]
                .iter()
                .filter(|r| r.result.is_some())
                .count(),
            1
        );
        // line numbers continue per pane
        assert_eq!(rows.rows[4].ours.unwrap().line_no, 5);
        assert_eq!(rows.rows[4].theirs.unwrap().line_no, 2);
        assert_eq!(rows.rows[4].result.unwrap().line_no, 3);
        assert_eq!(*rows.chunk_start.last().unwrap(), rows.rows.len());
    }

    #[test]
    fn resolution_changes_result_height() {
        let mut m = MergeModel::new("a\n", "a\no1\no2\n", "a\nt\n");
        let ci = m.chunks.iter().position(|c| c.is_conflict()).unwrap();
        assert_eq!(build(&m).rows.len(), 3);
        m.resolve(ci, Resolution::OursThenTheirs);
        let rows = build(&m);
        assert_eq!(rows.rows.len(), 4);
        assert_eq!(rows.result_line_of(ci), 2);
        // A resolved-to-empty chunk still keeps a row so it stays addressable.
        let mut m = MergeModel::new("a\nb\n", "a\n", "a\n");
        m.resolve(1, Resolution::Ours);
        let rows = build(&m);
        assert_eq!(rows.chunk_rows(1), (1, 2));
        assert_eq!(rows.result_line_of(1), 2);
    }

    #[test]
    fn clipping_and_scrolling() {
        assert_eq!(clip("hello world", 6, 3), "wor");
        assert_eq!(clip("\tx", 0, 10), "    x");
        assert_eq!(clip("日本語", 2, 2), "本");
        assert_eq!(clip("abc", 10, 5), "");
        assert_eq!(scroll_for(5, 0, 10), 0);
        assert_eq!(scroll_for(50, 0, 30), 40);
        assert_eq!(scroll_for(2, 10, 9), 0);
    }
}
