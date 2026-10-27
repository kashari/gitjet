//! Pure diff layout: the rows shown in unified and side-by-side mode, pairing of removed/added
//! lines, change-block navigation and word-level segmentation. No terminal or git access here.

use std::ops::Range;
use std::time::Duration;

use similar::{ChangeTag, TextDiff};

use crate::git::diff::{ChangeKind, DiffLine, FileDiff, LineKind};

/// One rendered row of a diff. Line indices refer to `FileDiff::hunks[hunk].lines`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// File title (inline previews show several files in one stream)
    File(usize),
    /// Informational line about a file: rename, mode change, binary...
    Meta(usize, String),
    /// `@@ -a,b +c,d @@ section`
    Hunk(usize, usize),
    /// One line of a unified diff
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// One side-by-side row; `None` is a filler cell keeping both sides aligned
    Split {
        file: usize,
        hunk: usize,
        old: Option<usize>,
        new: Option<usize>,
    },
}

impl Row {
    /// File index this row belongs to.
    pub fn file(&self) -> usize {
        match self {
            Row::File(f) | Row::Meta(f, _) | Row::Hunk(f, _) => *f,
            Row::Line { file, .. } | Row::Split { file, .. } => *file,
        }
    }
}

/// A run of unchanged lines or one block of removals followed by additions within a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Same(usize),
    Change { dels: Vec<usize>, adds: Vec<usize> },
}

/// Split hunk lines into context lines and change blocks. `\ No newline` markers stay with the
/// line they annotate (inside the removed or added run, or as a context row).
pub fn segments(lines: &[DiffLine]) -> Vec<Segment> {
    let mut out = Vec::new();
    let n = lines.len();
    let mut i = 0;
    while i < n {
        match lines[i].kind {
            LineKind::Context | LineKind::NoNewline => {
                out.push(Segment::Same(i));
                i += 1;
            }
            LineKind::Removed | LineKind::Added => {
                let mut dels = Vec::new();
                let mut adds = Vec::new();
                take_run(lines, &mut i, LineKind::Removed, &mut dels);
                take_run(lines, &mut i, LineKind::Added, &mut adds);
                out.push(Segment::Change { dels, adds });
            }
        }
    }
    out
}

fn take_run(lines: &[DiffLine], i: &mut usize, kind: LineKind, into: &mut Vec<usize>) {
    while *i < lines.len() && lines[*i].kind == kind {
        into.push(*i);
        *i += 1;
        if *i < lines.len() && lines[*i].kind == LineKind::NoNewline {
            into.push(*i);
            *i += 1;
        }
    }
}

/// Side-by-side alignment of one hunk: `(old, new)` line indices per row. Context lines appear on
/// both sides; the n-th removed line of a block sits next to the n-th added line, and the shorter
/// side is padded with fillers (`None`).
pub fn align_hunk(lines: &[DiffLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    for seg in segments(lines) {
        match seg {
            Segment::Same(i) => rows.push((Some(i), Some(i))),
            Segment::Change { dels, adds } => {
                for k in 0..dels.len().max(adds.len()) {
                    rows.push((dels.get(k).copied(), adds.get(k).copied()));
                }
            }
        }
    }
    rows
}

/// For each hunk line, the index of its counterpart in the same change block (n-th removed ↔ n-th
/// added, markers excluded). Used for word-level highlighting.
pub fn pair_changes(lines: &[DiffLine]) -> Vec<Option<usize>> {
    let mut partner = vec![None; lines.len()];
    for seg in segments(lines) {
        if let Segment::Change { dels, adds } = seg {
            let real = |v: &[usize]| -> Vec<usize> {
                v.iter()
                    .copied()
                    .filter(|&i| lines[i].kind != LineKind::NoNewline)
                    .collect()
            };
            for (d, a) in real(&dels).into_iter().zip(real(&adds)) {
                partner[d] = Some(a);
                partner[a] = Some(d);
            }
        }
    }
    partner
}

/// Short label for a change kind (file lists, titles).
pub fn kind_label(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Modified => "modified",
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Copied => "copied",
        ChangeKind::ModeChanged => "mode changed",
    }
}

/// One-letter code for a change kind.
pub fn kind_letter(k: ChangeKind) -> char {
    match k {
        ChangeKind::Modified => 'M',
        ChangeKind::Added => 'A',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::ModeChanged => 'T',
    }
}

fn mode_suffix(mode: Option<&str>) -> String {
    match mode {
        Some("120000") => " (symbolic link)".into(),
        Some("160000") => " (submodule)".into(),
        Some("100755") => " (executable)".into(),
        _ => String::new(),
    }
}

/// Informational lines describing a file beyond its hunks: renames/copies, new/deleted files,
/// mode changes, binary content, or "no content changes".
pub fn file_meta(d: &FileDiff) -> Vec<String> {
    let mut v = Vec::new();
    let similarity = d
        .header
        .iter()
        .find_map(|h| h.strip_prefix("similarity index "))
        .map(|s| format!(" ({s} similar)"))
        .unwrap_or_default();
    let old = d.old_path.as_deref().unwrap_or("?");
    match d.kind {
        ChangeKind::Renamed => v.push(format!("renamed from {old}{similarity}")),
        ChangeKind::Copied => v.push(format!("copied from {old}{similarity}")),
        ChangeKind::Added => {
            if d.header.iter().any(|h| h.contains("untracked")) {
                v.push("new file (untracked)".into());
            } else {
                v.push(format!("new file{}", mode_suffix(d.new_mode.as_deref())));
            }
        }
        ChangeKind::Deleted => v.push(format!(
            "deleted file{}",
            mode_suffix(d.old_mode.as_deref())
        )),
        ChangeKind::Modified | ChangeKind::ModeChanged => {}
    }
    if let (Some(o), Some(n)) = (&d.old_mode, &d.new_mode) {
        if o != n {
            v.push(format!("mode changed {o} → {n}{}", mode_suffix(Some(n))));
        }
    }
    if d.binary {
        v.push("binary file — contents differ (not shown)".into());
    } else if d.hunks.is_empty() {
        match d.kind {
            ChangeKind::Added | ChangeKind::Deleted => v.push("empty file".into()),
            _ if v.is_empty() => v.push("no content changes".into()),
            _ => {}
        }
    }
    v
}

/// Unified rows for files in `files`, optionally preceded by a [`Row::File`] title per file.
pub fn unified_rows(diffs: &[FileDiff], files: Range<usize>, file_titles: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    for fi in files.filter(|&i| i < diffs.len()) {
        let d = &diffs[fi];
        if file_titles {
            rows.push(Row::File(fi));
        }
        rows.extend(file_meta(d).into_iter().map(|m| Row::Meta(fi, m)));
        for (hi, h) in d.hunks.iter().enumerate() {
            rows.push(Row::Hunk(fi, hi));
            rows.extend((0..h.lines.len()).map(|li| Row::Line {
                file: fi,
                hunk: hi,
                line: li,
            }));
        }
    }
    rows
}

/// Number of rows [`unified_rows`] produces for all files with titles (cheap; no allocation per line).
pub fn unified_row_count(diffs: &[FileDiff]) -> usize {
    diffs
        .iter()
        .map(|d| 1 + file_meta(d).len() + d.hunks.iter().map(|h| 1 + h.lines.len()).sum::<usize>())
        .sum()
}

/// Side-by-side rows for one file.
pub fn split_rows(diffs: &[FileDiff], file: usize) -> Vec<Row> {
    let Some(d) = diffs.get(file) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = file_meta(d)
        .into_iter()
        .map(|m| Row::Meta(file, m))
        .collect();
    for (hi, h) in d.hunks.iter().enumerate() {
        rows.push(Row::Hunk(file, hi));
        rows.extend(
            align_hunk(&h.lines)
                .into_iter()
                .map(|(old, new)| Row::Split {
                    file,
                    hunk: hi,
                    old,
                    new,
                }),
        );
    }
    rows
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowClass {
    Change,
    /// `\ No newline` markers and fillers: neither start nor end a change block
    Neutral,
    Other,
}

fn line_kind(diffs: &[FileDiff], file: usize, hunk: usize, line: usize) -> Option<LineKind> {
    diffs
        .get(file)?
        .hunks
        .get(hunk)?
        .lines
        .get(line)
        .map(|l| l.kind)
}

fn classify(diffs: &[FileDiff], row: &Row) -> RowClass {
    let of = |k: Option<LineKind>| match k {
        Some(LineKind::Added | LineKind::Removed) => RowClass::Change,
        Some(LineKind::Context) => RowClass::Other,
        Some(LineKind::NoNewline) | None => RowClass::Neutral,
    };
    match row {
        Row::Line { file, hunk, line } => of(line_kind(diffs, *file, *hunk, *line)),
        Row::Split {
            file,
            hunk,
            old,
            new,
        } => {
            let a = of(old.and_then(|l| line_kind(diffs, *file, *hunk, l)));
            let b = of(new.and_then(|l| line_kind(diffs, *file, *hunk, l)));
            if a == RowClass::Change || b == RowClass::Change {
                RowClass::Change
            } else if a == RowClass::Other || b == RowClass::Other {
                RowClass::Other
            } else {
                RowClass::Neutral
            }
        }
        _ => RowClass::Other,
    }
}

/// Row indices where a block of changed lines starts (targets for next/previous difference).
pub fn change_starts(diffs: &[FileDiff], rows: &[Row]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut in_change = false;
    for (i, r) in rows.iter().enumerate() {
        match classify(diffs, r) {
            RowClass::Change => {
                if !in_change {
                    out.push(i);
                }
                in_change = true;
            }
            RowClass::Neutral => {}
            RowClass::Other => in_change = false,
        }
    }
    out
}

/// Changed byte ranges of the old line and of the new line.
pub type WordRanges = (Vec<Range<usize>>, Vec<Range<usize>>);

/// Lines longer than this (combined) are not word-diffed, to keep rendering instant.
pub const MAX_WORD_DIFF_LEN: usize = 4000;

/// Byte ranges that differ between a removed line `old` and its added counterpart `new`, by
/// Unicode words. Ranges separated only by whitespace are merged. `None` when the lines share no
/// non-whitespace text (then highlighting words would just highlight everything) or are too long.
pub fn word_ranges(old: &str, new: &str) -> Option<WordRanges> {
    if old.len() + new.len() > MAX_WORD_DIFF_LEN {
        return None;
    }
    let diff = TextDiff::configure()
        .timeout(Duration::from_millis(20))
        .diff_unicode_words(old, new);
    let (mut o, mut n) = (0usize, 0usize);
    let (mut ro, mut rn) = (Vec::new(), Vec::new());
    let mut common = false;
    for ch in diff.iter_all_changes() {
        let len = ch.value().len();
        match ch.tag() {
            ChangeTag::Equal => {
                common |= !ch.value().trim().is_empty();
                o += len;
                n += len;
            }
            ChangeTag::Delete => {
                push_range(&mut ro, o..o + len);
                o += len;
            }
            ChangeTag::Insert => {
                push_range(&mut rn, n..n + len);
                n += len;
            }
        }
    }
    if !common {
        return None;
    }
    Some((
        merge_across_whitespace(old, ro),
        merge_across_whitespace(new, rn),
    ))
}

fn push_range(v: &mut Vec<Range<usize>>, r: Range<usize>) {
    if r.is_empty() {
        return;
    }
    match v.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => v.push(r),
    }
}

fn merge_across_whitespace(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last)
                if text
                    .get(last.end..r.start)
                    .is_some_and(|gap| gap.trim().is_empty()) =>
            {
                last.end = r.end;
            }
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::parse_unified;

    fn lines(spec: &str) -> Vec<DiffLine> {
        spec.split('|')
            .map(|s| {
                let (k, c) = s.split_at(1);
                let kind = match k {
                    "+" => LineKind::Added,
                    "-" => LineKind::Removed,
                    "\\" => LineKind::NoNewline,
                    _ => LineKind::Context,
                };
                DiffLine {
                    kind,
                    content: c.to_string(),
                    old_no: None,
                    new_no: None,
                }
            })
            .collect()
    }

    #[test]
    fn aligns_blocks_with_fillers() {
        // ctx, 3 removed, 1 added, ctx, 1 added
        let l = lines(" a|-b|-c|-d|+B| e|+f");
        let rows = align_hunk(&l);
        assert_eq!(
            rows,
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(4)),
                (Some(2), None),
                (Some(3), None),
                (Some(5), Some(5)),
                (None, Some(6)),
            ]
        );
    }

    #[test]
    fn aligns_more_added_than_removed() {
        let l = lines("-x|+y|+z|+w");
        assert_eq!(
            align_hunk(&l),
            vec![(Some(0), Some(1)), (None, Some(2)), (None, Some(3))]
        );
    }

    #[test]
    fn no_newline_markers_stay_with_their_side() {
        let l = lines("-old|\\ No newline at end of file|+new|\\ No newline at end of file");
        let rows = align_hunk(&l);
        assert_eq!(rows, vec![(Some(0), Some(2)), (Some(1), Some(3))]);
        let p = pair_changes(&l);
        assert_eq!(p, vec![Some(2), None, Some(0), None]);
        // Context followed by a marker is a shared row.
        let l = lines(" a|\\ No newline at end of file");
        assert_eq!(align_hunk(&l), vec![(Some(0), Some(0)), (Some(1), Some(1))]);
    }

    #[test]
    fn pairs_nth_removed_with_nth_added() {
        let l = lines("-a|-b|+A| c|+d");
        assert_eq!(pair_changes(&l), vec![Some(2), None, Some(0), None, None]);
    }

    #[test]
    fn change_starts_in_both_modes() {
        let text = "diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,6 +1,6 @@
 a
-b
+B
 c
 d
-e
+E
 f
";
        let diffs = parse_unified(text);
        let rows = unified_rows(&diffs, 0..1, false);
        // Hunk, a, -b, +B, c, d, -e, +E, f
        assert_eq!(change_starts(&diffs, &rows), vec![2, 6]);
        let split = split_rows(&diffs, 0);
        // Hunk, a, b|B, c, d, e|E, f
        assert_eq!(split.len(), 7);
        assert_eq!(change_starts(&diffs, &split), vec![2, 5]);
        let titled = unified_rows(&diffs, 0..1, true);
        assert_eq!(titled.len(), unified_row_count(&diffs));
    }

    #[test]
    fn meta_lines_describe_files() {
        let text = "diff --git a/old.txt b/new.txt
similarity index 90%
rename from old.txt
rename to new.txt
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/img.png b/img.png
index 1..2 100644
Binary files a/img.png and b/img.png differ
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 1..0
diff --git a/link b/link
new file mode 120000
index 0..1
--- /dev/null
+++ b/link
@@ -0,0 +1 @@
+target
\\ No newline at end of file
";
        let d = parse_unified(text);
        assert_eq!(file_meta(&d[0]), vec!["renamed from old.txt (90% similar)"]);
        assert_eq!(
            file_meta(&d[1]),
            vec!["mode changed 100644 → 100755 (executable)"]
        );
        assert_eq!(
            file_meta(&d[2]),
            vec!["binary file — contents differ (not shown)"]
        );
        assert_eq!(file_meta(&d[3]), vec!["deleted file", "empty file"]);
        assert_eq!(file_meta(&d[4]), vec!["new file (symbolic link)"]);
        assert_eq!(
            unified_row_count(&d),
            unified_rows(&d, 0..d.len(), true).len()
        );
    }

    #[test]
    fn word_ranges_find_changed_words() {
        let (o, n) = word_ranges("let x = 1;", "let y = 1;").unwrap();
        assert_eq!(o, vec![4..5]);
        assert_eq!(n, vec![4..5]);
        let (o, n) = word_ranges("foo(a, b)", "foo(a, b, c)").unwrap();
        assert!(o.is_empty());
        assert_eq!(&"foo(a, b, c)"[n[0].clone()], ", c");
    }

    #[test]
    fn word_ranges_merge_across_spaces_and_handle_unicode() {
        let old = "the quick brown fox";
        let new = "the slow red fox";
        let (o, n) = word_ranges(old, new).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(&old[o[0].clone()], "quick brown");
        assert_eq!(&new[n[0].clone()], "slow red");
        let (o, n) = word_ranges("naïve café", "naïve thé").unwrap();
        assert_eq!(&"naïve café"[o[0].clone()], "café");
        assert_eq!(&"naïve thé"[n[0].clone()], "thé");
    }

    #[test]
    fn word_ranges_skip_unrelated_or_huge_lines() {
        assert!(word_ranges("alpha", "beta").is_none());
        let big = "x ".repeat(MAX_WORD_DIFF_LEN);
        assert!(word_ranges(&big, "x").is_none());
        // Whitespace-only change is still highlighted.
        let (o, n) = word_ranges("a b", "a  b").unwrap();
        assert!(o.len() + n.len() > 0);
    }
}
