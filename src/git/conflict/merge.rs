//! Line-based 3-way merge model used by the merge resolver.
//!
//! The base text is diffed against "ours" and against "theirs"; overlapping hunks are grouped into
//! chunks. Each chunk is either unchanged, changed on one side only (auto-resolvable), changed
//! identically on both sides (auto-resolvable) or a real conflict that needs a decision.
//!
//! Line endings are normalized for comparison, so a file whose sides disagree only in CRLF vs LF
//! does not conflict; the merged text is written back with the dominant line ending of the inputs.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffTag};

/// How a chunk differs between the three versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// Same text in base, ours and theirs.
    Unchanged,
    /// Only our side changed this region (auto-resolvable).
    OursOnly,
    /// Only their side changed this region (auto-resolvable).
    TheirsOnly,
    /// Both sides made the identical change (auto-resolvable).
    BothSame,
    /// Both sides changed the region differently.
    Conflict,
}

/// The decision taken for a changed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Take our lines.
    Ours,
    /// Take their lines.
    Theirs,
    /// Our lines followed by theirs.
    OursThenTheirs,
    /// Their lines followed by ours.
    TheirsThenOurs,
    /// Discard both changes and keep the base lines.
    Base,
}

impl Resolution {
    /// Short description for status messages.
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Ours => "accepted yours",
            Resolution::Theirs => "accepted theirs",
            Resolution::OursThenTheirs => "accepted both (yours first)",
            Resolution::TheirsThenOurs => "accepted both (theirs first)",
            Resolution::Base => "discarded both changes",
        }
    }
}

/// A region of the file, with the lines each version has there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    /// None while unresolved. Always None for [`ChunkKind::Unchanged`].
    pub resolution: Option<Resolution>,
}

impl Chunk {
    /// True for every chunk except unchanged regions.
    pub fn is_change(&self) -> bool {
        self.kind != ChunkKind::Unchanged
    }

    pub fn is_conflict(&self) -> bool {
        self.kind == ChunkKind::Conflict
    }

    /// Unchanged regions count as resolved.
    pub fn is_resolved(&self) -> bool {
        !self.is_change() || self.resolution.is_some()
    }

    /// The resolution "Apply non-conflicting changes" would pick, None for conflicts.
    pub fn automatic_resolution(&self) -> Option<Resolution> {
        match self.kind {
            ChunkKind::OursOnly | ChunkKind::BothSame => Some(Resolution::Ours),
            ChunkKind::TheirsOnly => Some(Resolution::Theirs),
            ChunkKind::Unchanged | ChunkKind::Conflict => None,
        }
    }

    /// Lines for a given resolution.
    pub fn lines_for(&self, res: Resolution) -> Vec<&str> {
        fn s(v: &[String]) -> Vec<&str> {
            v.iter().map(String::as_str).collect()
        }
        match res {
            Resolution::Ours => s(&self.ours),
            Resolution::Theirs => s(&self.theirs),
            Resolution::Base => s(&self.base),
            Resolution::OursThenTheirs => self
                .ours
                .iter()
                .chain(&self.theirs)
                .map(String::as_str)
                .collect(),
            Resolution::TheirsThenOurs => self
                .theirs
                .iter()
                .chain(&self.ours)
                .map(String::as_str)
                .collect(),
        }
    }

    /// Lines shown in the result: the resolution's lines, or the base lines while unresolved.
    pub fn result_lines(&self) -> Vec<&str> {
        self.lines_for(self.resolution.unwrap_or(Resolution::Base))
    }
}

/// Labels and style for conflict markers.
#[derive(Debug, Clone)]
pub struct MarkerStyle {
    pub ours_label: String,
    pub theirs_label: String,
    /// Include the `|||||||` base section (`merge.conflictStyle=diff3`).
    pub diff3: bool,
}

/// Split text into lines without terminators, remembering line-ending style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitText {
    pub lines: Vec<String>,
    /// Number of `\r\n` and bare `\n` terminators.
    pub crlf: usize,
    pub lf: usize,
    /// True when the text ends with a line terminator (or is empty).
    pub trailing_newline: bool,
}

impl SplitText {
    pub fn new(text: &str) -> Self {
        let mut lines = Vec::new();
        let (mut crlf, mut lf) = (0, 0);
        let mut trailing_newline = true;
        for piece in text.split_inclusive('\n') {
            if let Some(l) = piece.strip_suffix("\r\n") {
                crlf += 1;
                lines.push(l.to_string());
            } else if let Some(l) = piece.strip_suffix('\n') {
                lf += 1;
                lines.push(l.to_string());
            } else {
                trailing_newline = false;
                lines.push(piece.to_string());
            }
        }
        Self {
            lines,
            crlf,
            lf,
            trailing_newline,
        }
    }

    fn has_newlines(&self) -> bool {
        self.crlf + self.lf > 0
    }

    fn eol(&self) -> &'static str {
        if self.crlf > self.lf {
            "\r\n"
        } else {
            "\n"
        }
    }
}

/// Join lines with `eol`, adding a final terminator when `trailing` (and there are lines).
pub fn join_lines<S: AsRef<str>>(lines: &[S], eol: &str, trailing: bool) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(eol);
        }
        out.push_str(l.as_ref());
    }
    if trailing && !lines.is_empty() {
        out.push_str(eol);
    }
    out
}

/// Number of conflict-marker blocks (`<<<<<<<` lines) in `text`.
pub fn count_markers(text: &str) -> usize {
    marker_lines(text).len()
}

/// 0-based line numbers of `<<<<<<<` conflict-marker lines.
pub fn marker_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| is_marker_start(l))
        .map(|(i, _)| i)
        .collect()
}

fn is_marker_start(line: &str) -> bool {
    let l = line.trim_end_matches('\r');
    l == "<<<<<<<" || l.starts_with("<<<<<<< ")
}

/// The 3-way merge of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeModel {
    pub chunks: Vec<Chunk>,
    /// Line terminator used when writing the result.
    pub eol: &'static str,
    trailing: [bool; 3],
}

#[derive(Debug, Clone)]
struct Hunk {
    side: usize,
    base: Range<usize>,
    other: Range<usize>,
}

const OURS: usize = 0;
const THEIRS: usize = 1;

/// Changed regions between `base` and `side`, merging adjacent delete/insert operations.
fn hunks(base: &[String], side: &[String], which: usize) -> Vec<Hunk> {
    let deadline = Some(Instant::now() + Duration::from_secs(2));
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, base, side, deadline);
    // Positions are tracked from the op lengths: after compaction, similar may report the
    // other-side index of insert/delete ops inconsistently, but the op sequence itself is exact.
    let mut out: Vec<Hunk> = Vec::new();
    let (mut b, mut s) = (0usize, 0usize);
    for op in ops {
        let (tag, old, new) = op.as_tag_tuple();
        let (old_len, new_len) = match tag {
            DiffTag::Equal => {
                b += old.len();
                s += new.len();
                continue;
            }
            DiffTag::Delete => (old.len(), 0),
            DiffTag::Insert => (0, new.len()),
            DiffTag::Replace => (old.len(), new.len()),
        };
        let (base, other) = (b..b + old_len, s..s + new_len);
        b += old_len;
        s += new_len;
        match out.last_mut() {
            Some(last) if last.base.end == base.start && last.other.end == other.start => {
                last.base.end = base.end;
                last.other.end = other.end;
            }
            _ => out.push(Hunk {
                side: which,
                base,
                other,
            }),
        }
    }
    out
}

/// Whether two base ranges touch the same lines. Insertions only collide with insertions at the same
/// point or with a range they fall strictly inside; edits that are merely adjacent do not conflict.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => a.start == b.start,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start.max(b.start) < a.end.min(b.end),
    }
}

/// Group hunks of both sides into clusters of mutually overlapping changes (sorted by base position).
fn cluster(mut all: Vec<Hunk>) -> Vec<(Range<usize>, Vec<Hunk>)> {
    all.sort_by_key(|h| (h.base.start, h.base.end, h.side));
    let mut out: Vec<(Range<usize>, Vec<Hunk>)> = Vec::new();
    for h in all {
        if let Some((range, members)) = out.last_mut() {
            if overlaps(range, &h.base) {
                range.start = range.start.min(h.base.start);
                range.end = range.end.max(h.base.end);
                members.push(h);
                continue;
            }
        }
        out.push((h.base.clone(), vec![h]));
    }
    out
}

/// The lines a side has for base range `lo..hi`, given its hunks inside the cluster and its offset
/// relative to the base before the cluster.
fn side_range(members: &[&Hunk], lo: usize, hi: usize, delta: isize) -> Range<usize> {
    match (members.first(), members.last()) {
        (Some(first), Some(last)) => {
            let start = first.other.start.saturating_sub(first.base.start - lo);
            let end = last.other.end + (hi - last.base.end);
            start..end
        }
        _ => {
            let start = (lo as isize + delta).max(0) as usize;
            start..start + (hi - lo)
        }
    }
}

impl MergeModel {
    /// Compute the chunk sequence for a 3-way merge.
    pub fn new(base: &str, ours: &str, theirs: &str) -> Self {
        let texts = [
            SplitText::new(base),
            SplitText::new(ours),
            SplitText::new(theirs),
        ];
        let [b, o, t] = &texts;
        let mut all = hunks(&b.lines, &o.lines, OURS);
        all.extend(hunks(&b.lines, &t.lines, THEIRS));
        let mut chunks = Vec::new();
        let mut delta = [0isize; 2];
        let mut pos = 0;
        for (range, members) in cluster(all) {
            let (lo, hi) = (range.start, range.end);
            if lo > pos {
                chunks.push(unchanged(&b.lines[pos..lo]));
            }
            let mut ranges = [0..0, 0..0];
            for side in [OURS, THEIRS] {
                let mine: Vec<&Hunk> = members.iter().filter(|h| h.side == side).collect();
                ranges[side] = side_range(&mine, lo, hi, delta[side]);
                for h in &mine {
                    delta[side] += h.other.len() as isize - h.base.len() as isize;
                }
            }
            let has = |side: usize| members.iter().any(|h| h.side == side);
            let ours_lines = slice(&o.lines, &ranges[OURS]);
            let theirs_lines = slice(&t.lines, &ranges[THEIRS]);
            let kind = if !has(THEIRS) {
                ChunkKind::OursOnly
            } else if !has(OURS) {
                ChunkKind::TheirsOnly
            } else if ours_lines == theirs_lines {
                ChunkKind::BothSame
            } else {
                ChunkKind::Conflict
            };
            chunks.push(Chunk {
                kind,
                base: b.lines[lo..hi].to_vec(),
                ours: ours_lines,
                theirs: theirs_lines,
                resolution: None,
            });
            pos = hi;
        }
        if pos < b.lines.len() {
            chunks.push(unchanged(&b.lines[pos..]));
        }
        let eol = [o, t, b]
            .into_iter()
            .find(|s| s.has_newlines())
            .map(SplitText::eol)
            .unwrap_or("\n");
        Self {
            chunks,
            eol,
            trailing: [b.trailing_newline, o.trailing_newline, t.trailing_newline],
        }
    }

    /// Whether the result ends with a newline: a 3-way merge of the three flags.
    fn trailing_newline(&self) -> bool {
        let [base, ours, theirs] = self.trailing;
        if ours == theirs || ours != base {
            ours
        } else {
            theirs
        }
    }

    /// Indices of all changed chunks.
    pub fn changes(&self) -> Vec<usize> {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].is_change())
            .collect()
    }

    /// Unresolved real conflicts.
    pub fn conflicts_remaining(&self) -> usize {
        self.chunks
            .iter()
            .filter(|c| c.is_conflict() && !c.is_resolved())
            .count()
    }

    /// Unresolved auto-resolvable changes (one-sided or identical).
    pub fn pending_changes(&self) -> usize {
        self.chunks
            .iter()
            .filter(|c| !c.is_conflict() && !c.is_resolved())
            .count()
    }

    /// True when every chunk has a decision.
    pub fn is_fully_resolved(&self) -> bool {
        self.chunks.iter().all(Chunk::is_resolved)
    }

    /// Set the resolution of chunk `i` (ignored for unchanged chunks). Returns true if it changed.
    pub fn resolve(&mut self, i: usize, res: Resolution) -> bool {
        match self.chunks.get_mut(i) {
            Some(c) if c.is_change() && c.resolution != Some(res) => {
                c.resolution = Some(res);
                true
            }
            _ => false,
        }
    }

    /// Clear the resolution of chunk `i`. Returns true if it was resolved.
    pub fn undo(&mut self, i: usize) -> bool {
        match self.chunks.get_mut(i) {
            Some(c) if c.resolution.is_some() => {
                c.resolution = None;
                true
            }
            _ => false,
        }
    }

    /// Clear every resolution.
    pub fn reset(&mut self) {
        self.chunks.iter_mut().for_each(|c| c.resolution = None);
    }

    /// Resolve every unresolved non-conflicting change. Returns how many.
    pub fn apply_non_conflicting(&mut self) -> usize {
        let mut n = 0;
        for c in self.chunks.iter_mut().filter(|c| c.resolution.is_none()) {
            if let Some(r) = c.automatic_resolution() {
                c.resolution = Some(r);
                n += 1;
            }
        }
        n
    }

    /// Next (or previous) chunk after `from` matching `pred`, wrapping around.
    pub fn find(
        &self,
        from: Option<usize>,
        forward: bool,
        pred: impl Fn(&Chunk) -> bool,
    ) -> Option<usize> {
        let n = self.chunks.len();
        if n == 0 {
            return None;
        }
        let start = from.unwrap_or(if forward { n - 1 } else { 0 });
        (1..=n)
            .map(|step| {
                if forward {
                    (start + step) % n
                } else {
                    (start + n * 2 - step) % n
                }
            })
            .find(|&i| pred(&self.chunks[i]))
    }

    /// The result as shown: resolutions applied, base text for unresolved chunks.
    pub fn result_text(&self) -> String {
        let lines: Vec<&str> = self.chunks.iter().flat_map(Chunk::result_lines).collect();
        join_lines(&lines, self.eol, self.trailing_newline())
    }

    /// The result with non-conflicting changes applied and conflict markers for unresolved
    /// conflicts — what `git merge` would leave in the work tree, plus the decisions taken so far.
    pub fn text_with_markers(&self, style: &MarkerStyle) -> String {
        let mut lines: Vec<String> = Vec::new();
        for c in &self.chunks {
            match c.resolution.or_else(|| c.automatic_resolution()) {
                Some(r) => lines.extend(c.lines_for(r).into_iter().map(str::to_string)),
                None if !c.is_change() => lines.extend(c.base.iter().cloned()),
                None => {
                    lines.push(format!("<<<<<<< {}", style.ours_label));
                    lines.extend(c.ours.iter().cloned());
                    if style.diff3 {
                        lines.push("||||||| base".to_string());
                        lines.extend(c.base.iter().cloned());
                    }
                    lines.push("=======".to_string());
                    lines.extend(c.theirs.iter().cloned());
                    lines.push(format!(">>>>>>> {}", style.theirs_label));
                }
            }
        }
        // Marker lines always need a terminator after them.
        let trailing =
            self.trailing_newline() || lines.last().is_some_and(|l| l.starts_with(">>>>>>> "));
        join_lines(&lines, self.eol, trailing)
    }
}

fn unchanged(lines: &[String]) -> Chunk {
    Chunk {
        kind: ChunkKind::Unchanged,
        base: lines.to_vec(),
        ours: lines.to_vec(),
        theirs: lines.to_vec(),
        resolution: None,
    }
}

fn slice(lines: &[String], r: &Range<usize>) -> Vec<String> {
    let end = r.end.min(lines.len());
    let start = r.start.min(end);
    lines[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(m: &MergeModel) -> Vec<ChunkKind> {
        m.chunks.iter().map(|c| c.kind).collect()
    }

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    /// Concatenating each side's chunk lines must reproduce that side exactly.
    fn assert_reconstructs(base: &str, ours: &str, theirs: &str) {
        let m = MergeModel::new(base, ours, theirs);
        let join = |f: &dyn Fn(&Chunk) -> &Vec<String>| -> Vec<String> {
            m.chunks.iter().flat_map(|c| f(c).clone()).collect()
        };
        assert_eq!(
            join(&|c| &c.base),
            SplitText::new(base).lines,
            "base of {base:?}/{ours:?}/{theirs:?}"
        );
        assert_eq!(
            join(&|c| &c.ours),
            SplitText::new(ours).lines,
            "ours of {base:?}/{ours:?}/{theirs:?}"
        );
        assert_eq!(
            join(&|c| &c.theirs),
            SplitText::new(theirs).lines,
            "theirs of {base:?}/{ours:?}/{theirs:?}"
        );
        for c in &m.chunks {
            match c.kind {
                ChunkKind::Unchanged => assert!(c.base == c.ours && c.base == c.theirs),
                ChunkKind::OursOnly => assert_eq!(c.base, c.theirs),
                ChunkKind::TheirsOnly => assert_eq!(c.base, c.ours),
                ChunkKind::BothSame => assert_eq!(c.ours, c.theirs),
                ChunkKind::Conflict => assert_ne!(c.ours, c.theirs),
            }
        }
        for w in m.chunks.windows(2) {
            assert!(
                !(w[0].kind == ChunkKind::Unchanged && w[1].kind == ChunkKind::Unchanged),
                "adjacent unchanged chunks"
            );
        }
    }

    #[test]
    fn identical_inputs_are_one_unchanged_chunk() {
        let m = MergeModel::new("a\nb\n", "a\nb\n", "a\nb\n");
        assert_eq!(kinds(&m), vec![ChunkKind::Unchanged]);
        assert!(m.is_fully_resolved());
        assert_eq!(m.result_text(), "a\nb\n");
    }

    #[test]
    fn one_sided_changes_are_auto_resolvable() {
        let base = "1\n2\n3\n4\n5\n";
        let ours = "1\nTWO\n3\n4\n5\n";
        let theirs = "1\n2\n3\n4\nFIVE\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(
            kinds(&m),
            vec![
                ChunkKind::Unchanged,
                ChunkKind::OursOnly,
                ChunkKind::Unchanged,
                ChunkKind::TheirsOnly
            ]
        );
        assert_eq!(m.conflicts_remaining(), 0);
        assert_eq!(m.pending_changes(), 2);
        // Unresolved chunks show the base text.
        assert_eq!(m.result_text(), base);
        assert_eq!(m.apply_non_conflicting(), 2);
        assert_eq!(m.result_text(), "1\nTWO\n3\n4\nFIVE\n");
        assert!(m.is_fully_resolved());
    }

    #[test]
    fn overlapping_edits_conflict() {
        let base = "a\nb\nc\nd\n";
        let ours = "a\nB1\nC1\nd\n";
        let theirs = "a\nb\nC2\nD2\n";
        let m = MergeModel::new(base, ours, theirs);
        assert_eq!(kinds(&m), vec![ChunkKind::Unchanged, ChunkKind::Conflict]);
        let c = &m.chunks[1];
        assert_eq!(c.base, v(&["b", "c", "d"]));
        assert_eq!(c.ours, v(&["B1", "C1", "d"]));
        assert_eq!(c.theirs, v(&["b", "C2", "D2"]));
        assert_eq!(m.conflicts_remaining(), 1);
    }

    #[test]
    fn chained_overlaps_form_one_conflict() {
        // ours edits 2-3, theirs edits 3-4, ours edits 4-5 again: one cluster.
        let base = "1\n2\n3\n4\n5\n6\n";
        let ours = "1\nx\nx\n4\ny\n6\n";
        let theirs = "1\n2\nz\nz\n5\n6\n";
        let m = MergeModel::new(base, ours, theirs);
        let conflicts: Vec<&Chunk> = m.chunks.iter().filter(|c| c.is_conflict()).collect();
        assert_eq!(conflicts.len(), 1);
        assert_reconstructs(base, ours, theirs);
    }

    #[test]
    fn adjacent_edits_do_not_conflict() {
        let base = "a\nb\nc\nd\n";
        let ours = "a\nB\nc\nd\n";
        let theirs = "a\nb\nC\nd\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(
            kinds(&m),
            vec![
                ChunkKind::Unchanged,
                ChunkKind::OursOnly,
                ChunkKind::TheirsOnly,
                ChunkKind::Unchanged
            ]
        );
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "a\nB\nC\nd\n");
    }

    #[test]
    fn insert_at_end_by_both_conflicts_and_both_orders_work() {
        let base = "a\n";
        let ours = "a\nours\n";
        let theirs = "a\ntheirs\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(kinds(&m), vec![ChunkKind::Unchanged, ChunkKind::Conflict]);
        assert!(m.chunks[1].base.is_empty());
        m.resolve(1, Resolution::OursThenTheirs);
        assert_eq!(m.result_text(), "a\nours\ntheirs\n");
        m.resolve(1, Resolution::TheirsThenOurs);
        assert_eq!(m.result_text(), "a\ntheirs\nours\n");
        m.resolve(1, Resolution::Base);
        assert_eq!(m.result_text(), "a\n");
    }

    #[test]
    fn insert_at_end_and_edit_of_last_line_are_separate() {
        let base = "a\nb\n";
        let ours = "a\nb\nc\n";
        let theirs = "a\nB\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(m.conflicts_remaining(), 0);
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "a\nB\nc\n");
    }

    #[test]
    fn insertion_at_same_point_with_same_text_is_both_same() {
        let m = MergeModel::new("a\nc\n", "a\nb\nc\n", "a\nb\nc\n");
        assert_eq!(
            kinds(&m),
            vec![
                ChunkKind::Unchanged,
                ChunkKind::BothSame,
                ChunkKind::Unchanged
            ]
        );
        assert_eq!(m.conflicts_remaining(), 0);
    }

    #[test]
    fn identical_edits_are_both_same() {
        let mut m = MergeModel::new("x\nold\ny\n", "x\nnew\ny\n", "x\nnew\ny\n");
        assert_eq!(kinds(&m)[1], ChunkKind::BothSame);
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "x\nnew\ny\n");
    }

    #[test]
    fn deletions() {
        // ours deletes a line, theirs edits another one far away
        let base = "1\n2\n3\n4\n5\n";
        let ours = "1\n3\n4\n5\n";
        let theirs = "1\n2\n3\n4\nfive\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(m.conflicts_remaining(), 0);
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "1\n3\n4\nfive\n");
        // delete vs edit of the same line conflicts
        let m = MergeModel::new("a\nb\nc\n", "a\nc\n", "a\nB\nc\n");
        let c = m.chunks.iter().find(|c| c.is_conflict()).expect("conflict");
        assert!(c.ours.is_empty());
        assert_eq!(c.theirs, v(&["B"]));
        // both delete the same line
        let m = MergeModel::new("a\nb\nc\n", "a\nc\n", "a\nc\n");
        assert_eq!(kinds(&m)[1], ChunkKind::BothSame);
        // everything deleted on one side
        let mut m = MergeModel::new("a\nb\n", "", "a\nb\n");
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "");
    }

    #[test]
    fn missing_base_is_add_add() {
        let m = MergeModel::new("", "one\n", "two\n");
        assert_eq!(kinds(&m), vec![ChunkKind::Conflict]);
        let m = MergeModel::new("", "same\n", "same\n");
        assert_eq!(kinds(&m), vec![ChunkKind::BothSame]);
    }

    #[test]
    fn crlf_is_preserved_and_eol_only_differences_do_not_conflict() {
        let base = "a\r\nb\r\nc\r\n";
        let ours = "a\r\nB\r\nc\r\n";
        let theirs = "a\r\nb\r\nC\r\n";
        let mut m = MergeModel::new(base, ours, theirs);
        assert_eq!(m.eol, "\r\n");
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "a\r\nB\r\nC\r\n");
        // theirs converted to LF entirely: no conflicts, ours' CRLF wins
        let mut m = MergeModel::new(base, ours, "a\nb\nc\n");
        assert_eq!(m.conflicts_remaining(), 0);
        m.apply_non_conflicting();
        assert_eq!(m.result_text(), "a\r\nB\r\nc\r\n");
        let style = MarkerStyle {
            ours_label: "HEAD".into(),
            theirs_label: "x".into(),
            diff3: false,
        };
        let m = MergeModel::new("a\r\n", "b\r\n", "c\r\n");
        assert_eq!(
            m.text_with_markers(&style),
            "<<<<<<< HEAD\r\nb\r\n=======\r\nc\r\n>>>>>>> x\r\n"
        );
    }

    #[test]
    fn trailing_newline_merge() {
        let mut m = MergeModel::new("a\nb\n", "a\nb", "a\nB\n");
        m.apply_non_conflicting();
        // ours removed the final newline, theirs edited b: the newline removal survives
        assert_eq!(m.result_text(), "a\nB");
        let m = MergeModel::new("a", "a", "a");
        assert_eq!(m.result_text(), "a");
        let m = MergeModel::new("", "", "");
        assert!(m.chunks.is_empty());
        assert_eq!(m.result_text(), "");
    }

    #[test]
    fn markers_and_diff3() {
        let base = "1\nx\n3\n4\n";
        let ours = "1\nours\n3\nfour\n";
        let theirs = "1\ntheirs\n3\n4\n";
        let mut m = MergeModel::new(base, ours, theirs);
        let style = MarkerStyle {
            ours_label: "HEAD".into(),
            theirs_label: "feature".into(),
            diff3: false,
        };
        assert_eq!(
            m.text_with_markers(&style),
            "1\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\n3\nfour\n"
        );
        let diff3 = MarkerStyle {
            diff3: true,
            ..style.clone()
        };
        assert!(m
            .text_with_markers(&diff3)
            .contains("||||||| base\nx\n=======\n"));
        assert_eq!(count_markers(&m.text_with_markers(&style)), 1);
        let ci = m.find(None, true, Chunk::is_conflict).unwrap();
        m.resolve(ci, Resolution::Theirs);
        assert_eq!(m.text_with_markers(&style), "1\ntheirs\n3\nfour\n");
        assert_eq!(count_markers("a\n<<<<<<< HEAD\nb\n"), 1);
        assert_eq!(marker_lines("x\r\n<<<<<<<\r\n"), vec![1]);
    }

    #[test]
    fn resolve_undo_and_navigation() {
        let base = "a\nb\nc\nd\ne\n";
        let ours = "A\nb\nC1\nd\ne\n";
        let theirs = "a\nb\nC2\nd\nE\n";
        let mut m = MergeModel::new(base, ours, theirs);
        let changes = m.changes();
        assert_eq!(changes.len(), 3);
        let unresolved = |c: &Chunk| c.is_change() && !c.is_resolved();
        assert_eq!(m.find(None, true, unresolved), Some(changes[0]));
        assert_eq!(m.find(None, false, unresolved), Some(changes[2]));
        assert_eq!(m.find(Some(changes[2]), true, unresolved), Some(changes[0]));
        assert!(m.resolve(changes[1], Resolution::Ours));
        assert!(!m.resolve(changes[1], Resolution::Ours));
        assert_eq!(m.find(Some(changes[0]), true, unresolved), Some(changes[2]));
        assert_eq!(m.conflicts_remaining(), 0);
        assert!(m.undo(changes[1]));
        assert!(!m.undo(changes[1]));
        assert_eq!(m.conflicts_remaining(), 1);
        assert_eq!(m.chunks[1].kind, ChunkKind::Unchanged);
        assert!(
            !m.resolve(1, Resolution::Ours),
            "unchanged chunks cannot be resolved"
        );
        m.apply_non_conflicting();
        m.reset();
        assert_eq!(m.pending_changes(), 2);
    }

    #[test]
    fn randomized_reconstruction() {
        // Deterministic pseudo-random edits over a small alphabet exercise many overlap shapes.
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rnd = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let alphabet = ["a", "b", "c", "d", "e"];
        for _ in 0..400 {
            let len = rnd(8) as usize;
            let base: Vec<&str> = (0..len).map(|_| alphabet[rnd(5) as usize]).collect();
            let mut mutate = |src: &Vec<&'static str>| -> Vec<&'static str> {
                let mut out = src.clone();
                for _ in 0..rnd(4) {
                    let at = rnd(out.len() as u64 + 1) as usize;
                    match rnd(3) {
                        0 => out.insert(at, alphabet[rnd(5) as usize]),
                        1 if at < out.len() => {
                            out.remove(at);
                        }
                        _ if at < out.len() => out[at] = alphabet[rnd(5) as usize],
                        _ => {}
                    }
                }
                out
            };
            let ours = mutate(&base);
            let theirs = mutate(&base);
            let text = |v: &Vec<&str>| v.iter().map(|l| format!("{l}\n")).collect::<String>();
            assert_reconstructs(&text(&base), &text(&ours), &text(&theirs));
            // Taking every side wholesale reproduces that side.
            let mut m = MergeModel::new(&text(&base), &text(&ours), &text(&theirs));
            for i in m.changes() {
                m.resolve(i, Resolution::Ours);
            }
            assert_eq!(m.result_text(), text(&ours));
            for i in m.changes() {
                m.resolve(i, Resolution::Theirs);
            }
            assert_eq!(m.result_text(), text(&theirs));
        }
    }

    #[test]
    fn hunks_follow_op_sequence() {
        // similar reports odd cross-side indices for this input; positions must still be exact.
        let b = v(&["e", "a", "d", "e"]);
        let t = v(&["e", "d", "d", "d", "b", "e"]);
        let h = hunks(&b, &t, THEIRS);
        let mut rebuilt: Vec<String> = Vec::new();
        let mut pos = 0;
        for x in &h {
            rebuilt.extend(b[pos..x.base.start].iter().cloned());
            rebuilt.extend(t[x.other.clone()].iter().cloned());
            pos = x.base.end;
        }
        rebuilt.extend(b[pos..].iter().cloned());
        assert_eq!(rebuilt, t);
    }
}
