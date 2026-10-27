//! Partial staging/unstaging/reverting of hunks and single lines by building patches for `git apply`.
//!
//! The flow is always the same: load a [`FileDiff`] with [`load_diff`] (which keeps `\r` of CRLF
//! lines so patches apply byte-exactly), let the user pick changed lines as `(hunk, line)` indices,
//! then [`apply_lines`] in one [`Direction`]:
//!
//! - [`Direction::Stage`]: work tree → index (`git apply --cached`)
//! - [`Direction::Unstage`]: index → HEAD (`git apply --cached --reverse` of the staged diff)
//! - [`Direction::Discard`]: drop work-tree changes (`git apply --reverse` of the unstaged diff)
//!
//! Patch building ([`build_patch`]) keeps one side of the original diff fixed (the side `git apply`
//! will match against) and constructs the other: in a forward patch unselected `+` lines are dropped
//! and unselected `-` lines become context; in a reverse patch unselected `+` lines become context and
//! unselected `-` lines are dropped. Hunk headers are recomputed and `\ No newline at end of file`
//! markers are re-placed for the constructed side.

use std::collections::BTreeSet;

use anyhow::{anyhow, bail, Result};

use super::diff::{parse_unified, ChangeKind, DiffLine, FileDiff, Hunk, LineKind};
use super::Repo;

/// Selected changed lines as `(hunk index, line index within the hunk)`.
pub type Selection = BTreeSet<(usize, usize)>;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// What to do with the selected lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Add selected work-tree changes to the index.
    Stage,
    /// Remove selected staged changes from the index (work tree untouched).
    Unstage,
    /// Throw away selected unstaged changes from the work tree.
    Discard,
}

impl Direction {
    /// Whether the patch is applied with `--reverse`.
    pub fn reverse(self) -> bool {
        !matches!(self, Direction::Stage)
    }

    /// Whether this direction works on the staged diff (index vs HEAD).
    pub fn staged(self) -> bool {
        matches!(self, Direction::Unstage)
    }

    pub fn verb(self) -> &'static str {
        match self {
            Direction::Stage => "Staged",
            Direction::Unstage => "Unstaged",
            Direction::Discard => "Discarded",
        }
    }
}

/// Load the diff of one file for partial operations: `staged` = index vs HEAD, otherwise work tree
/// vs index. Untracked files get a synthetic "new file" diff. Returns `None` when there is no change.
///
/// Always uses 3 lines of context, no whitespace ignoring and no textconv, so that the result can be
/// turned back into an applicable patch. Line contents keep a trailing `\r` for CRLF lines.
pub fn load_diff(
    repo: &Repo,
    path: &str,
    staged: bool,
    untracked: bool,
) -> Result<Option<FileDiff>> {
    if untracked && !staged {
        return untracked_file_diff(repo, path).map(Some);
    }
    let mut args: Vec<&str> = vec!["--literal-pathspecs", "diff"];
    if staged {
        args.push("--cached");
        if repo.head_commit().is_none() {
            args.push(EMPTY_TREE);
        }
    }
    args.extend([
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U3",
        "--",
        path,
    ]);
    let bytes = repo.run_bytes(&args)?;
    let raw = String::from_utf8(bytes)
        .map_err(|_| anyhow!("{path} is not UTF-8 text; partial staging is not possible"))?;
    let mut files = parse_unified(&raw);
    let Some(mut fd) = files.pop() else {
        return Ok(None);
    };
    restore_cr(&mut fd, &raw);
    Ok(Some(fd))
}

/// An "all lines added" diff for an untracked file, byte-exact (keeps `\r`, marks a missing final newline).
pub fn untracked_file_diff(repo: &Repo, path: &str) -> Result<FileDiff> {
    let bytes = std::fs::read(repo.path(path)).map_err(|e| anyhow!("reading {path}: {e}"))?;
    let binary = bytes.iter().take(8000).any(|b| *b == 0);
    let mut fd = FileDiff {
        old_path: None,
        new_path: Some(path.to_string()),
        kind: ChangeKind::Added,
        binary,
        old_mode: None,
        new_mode: Some("100644".into()),
        header: vec!["new file (untracked)".into()],
        hunks: Vec::new(),
    };
    if binary {
        return Ok(fd);
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow!("{path} is not UTF-8 text; partial staging is not possible"))?;
    if text.is_empty() {
        return Ok(fd);
    }
    let mut lines: Vec<DiffLine> = Vec::new();
    let body = text.strip_suffix('\n');
    let no_newline = body.is_none();
    for (i, l) in body.unwrap_or(&text).split('\n').enumerate() {
        lines.push(DiffLine {
            kind: LineKind::Added,
            content: l.to_string(),
            old_no: None,
            new_no: Some(i as u32 + 1),
        });
    }
    let n = lines.len() as u32;
    if no_newline {
        lines.push(no_newline_line());
    }
    fd.hunks.push(Hunk {
        header: format!("@@ -0,0 +1,{n} @@"),
        old_start: 0,
        old_lines: 0,
        new_start: 1,
        new_lines: n,
        lines,
    });
    Ok(fd)
}

fn no_newline_line() -> DiffLine {
    DiffLine {
        kind: LineKind::NoNewline,
        content: "\\ No newline at end of file".into(),
        old_no: None,
        new_no: None,
    }
}

/// `parse_unified` strips `\r`; put it back on lines whose raw text ended with one.
fn restore_cr(fd: &mut FileDiff, raw: &str) {
    let mut hunk: Option<usize> = None;
    let mut idx = 0usize;
    for line in raw.split('\n') {
        if line.starts_with("@@ ") {
            hunk = Some(hunk.map_or(0, |h| h + 1));
            idx = 0;
            continue;
        }
        let Some(h) = hunk else { continue };
        let Some(hk) = fd.hunks.get_mut(h) else { break };
        if let Some(dl) = hk.lines.get_mut(idx) {
            if line.ends_with('\r') && dl.kind != LineKind::NoNewline && !dl.content.ends_with('\r')
            {
                dl.content.push('\r');
            }
            idx += 1;
        }
    }
}

/// Is line `(h, l)` an added or removed line (i.e. selectable)?
pub fn is_change(fd: &FileDiff, h: usize, l: usize) -> bool {
    fd.hunks
        .get(h)
        .and_then(|hk| hk.lines.get(l))
        .is_some_and(|dl| matches!(dl.kind, LineKind::Added | LineKind::Removed))
}

/// All changed lines of hunk `h`.
pub fn hunk_changes(fd: &FileDiff, h: usize) -> Vec<(usize, usize)> {
    fd.hunks
        .get(h)
        .map(|hk| {
            hk.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| matches!(l.kind, LineKind::Added | LineKind::Removed))
                .map(|(i, _)| (h, i))
                .collect()
        })
        .unwrap_or_default()
}

/// Every changed line of the file.
pub fn all_changes(fd: &FileDiff) -> Selection {
    (0..fd.hunks.len())
        .flat_map(|h| hunk_changes(fd, h))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Ctx,
    Add,
    Rem,
}

/// One line of the patch being built. `nonl` = the source line had no trailing newline on its side.
struct Op<'a> {
    kind: OpKind,
    text: &'a str,
    nonl: bool,
}

/// How the patch header describes the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMode {
    Modify,
    Create,
    Delete,
}

/// Build a patch applying only `selected` lines of `fd`. `reverse` = the patch will be applied with
/// `--reverse` (its new side must match the target). Returns `None` when nothing applicable is selected.
pub fn build_patch(fd: &FileDiff, selected: &Selection, reverse: bool) -> Option<String> {
    if fd.binary {
        return None;
    }
    let path = fd.path();
    let all = all_changes(fd);
    let chosen: Selection = selected.intersection(&all).copied().collect();
    if chosen.is_empty() {
        return None;
    }
    let everything = chosen.len() == all.len();
    let mode = match (reverse, fd.kind) {
        (false, ChangeKind::Deleted) if everything => FileMode::Delete,
        (true, ChangeKind::Added) if everything => FileMode::Create,
        (true, ChangeKind::Deleted) => FileMode::Delete,
        _ => FileMode::Modify,
    };

    let mut body = String::new();
    let mut delta: i64 = 0;
    for (hi, hunk) in fd.hunks.iter().enumerate() {
        let ops = hunk_ops(hunk, hi, &chosen, reverse);
        if !ops.iter().any(|o| o.kind != OpKind::Ctx) {
            continue;
        }
        let old_count = ops.iter().filter(|o| o.kind != OpKind::Add).count() as i64;
        let new_count = ops.iter().filter(|o| o.kind != OpKind::Rem).count() as i64;
        let (old_start, new_start) = if reverse {
            let new_start = hunk.new_start as i64;
            let new_pos = if new_count > 0 {
                new_start
            } else {
                new_start + 1
            };
            let old_pos = new_pos - delta;
            (if old_count > 0 { old_pos } else { old_pos - 1 }, new_start)
        } else {
            let old_start = hunk.old_start as i64;
            let old_pos = if old_count > 0 {
                old_start
            } else {
                old_start + 1
            };
            let new_pos = old_pos + delta;
            (old_start, if new_count > 0 { new_pos } else { new_pos - 1 })
        };
        delta += new_count - old_count;
        body.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start.max(0),
            old_count,
            new_start.max(0),
            new_count
        ));
        emit_ops(&ops, &mut body);
    }
    if body.is_empty() {
        return None;
    }
    Some(format!("{}{body}", patch_header(fd, path, mode)))
}

/// Turn one hunk into patch operations according to the selection.
fn hunk_ops<'a>(hunk: &'a Hunk, hi: usize, chosen: &Selection, reverse: bool) -> Vec<Op<'a>> {
    let mut ops = Vec::new();
    for (i, l) in hunk.lines.iter().enumerate() {
        let nonl = hunk
            .lines
            .get(i + 1)
            .is_some_and(|n| n.kind == LineKind::NoNewline);
        let sel = chosen.contains(&(hi, i));
        let kind = match (l.kind, sel, reverse) {
            (LineKind::Context, _, _) => OpKind::Ctx,
            (LineKind::Added, true, _) => OpKind::Add,
            (LineKind::Removed, true, _) => OpKind::Rem,
            (LineKind::Added, false, true) | (LineKind::Removed, false, false) => OpKind::Ctx,
            (LineKind::Added, false, false) | (LineKind::Removed, false, true) => continue,
            (LineKind::NoNewline, _, _) => continue,
        };
        ops.push(Op {
            kind,
            text: &l.content,
            nonl,
        });
    }
    ops
}

/// Write ops as patch lines, placing `\ No newline at end of file` after the last line of each side
/// when that side ends without a newline. A context line that would need different newline handling
/// on the two sides is split into a `-`/`+` pair.
fn emit_ops(ops: &[Op], out: &mut String) {
    const MARKER: &str = "\\ No newline at end of file\n";
    let last_old = ops.iter().rposition(|o| o.kind != OpKind::Add);
    let last_new = ops.iter().rposition(|o| o.kind != OpKind::Rem);
    let old_nonl = last_old.is_some_and(|i| ops[i].nonl);
    let new_nonl = last_new.is_some_and(|i| ops[i].nonl);
    let line = |out: &mut String, sign: char, text: &str, marker: bool| {
        out.push(sign);
        out.push_str(text);
        out.push('\n');
        if marker {
            out.push_str(MARKER);
        }
    };
    for (i, op) in ops.iter().enumerate() {
        let old_marker = Some(i) == last_old && old_nonl;
        let new_marker = Some(i) == last_new && new_nonl;
        match op.kind {
            OpKind::Ctx if old_marker == new_marker => line(out, ' ', op.text, old_marker),
            OpKind::Ctx => {
                line(out, '-', op.text, old_marker);
                line(out, '+', op.text, new_marker);
            }
            OpKind::Rem => line(out, '-', op.text, old_marker),
            OpKind::Add => line(out, '+', op.text, new_marker),
        }
    }
}

fn patch_header(fd: &FileDiff, path: &str, mode: FileMode) -> String {
    let q = quote_path(path);
    let tab = if path.contains(' ') { "\t" } else { "" };
    let mut h = format!("diff --git a/{q} b/{q}\n");
    match mode {
        FileMode::Modify => {
            h.push_str(&format!("--- a/{q}{tab}\n+++ b/{q}{tab}\n"));
        }
        FileMode::Create => {
            let m = fd.new_mode.as_deref().unwrap_or("100644");
            h.push_str(&format!(
                "new file mode {m}\n--- /dev/null\n+++ b/{q}{tab}\n"
            ));
        }
        FileMode::Delete => {
            let m = fd.old_mode.as_deref().unwrap_or("100644");
            h.push_str(&format!(
                "deleted file mode {m}\n--- a/{q}{tab}\n+++ /dev/null\n"
            ));
        }
    }
    h
}

/// C-style quoting as git does for paths with special characters (non-ASCII stays as UTF-8).
pub fn quote_path(p: &str) -> String {
    let needs = p
        .chars()
        .any(|c| c == '"' || c == '\\' || (c as u32) < 0x20 || c == '\u{7f}');
    if !needs {
        return p.to_string();
    }
    let mut s = String::from("\"");
    for c in p.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => s.push_str(&format!("\\{:03o}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// True when some hunk of the patch has no context line at all (needs `--unidiff-zero`).
fn needs_unidiff_zero(patch: &str) -> bool {
    let mut in_hunk = false;
    let mut has_ctx = true;
    for l in patch.lines() {
        if l.starts_with("@@ ") {
            if in_hunk && !has_ctx {
                return true;
            }
            in_hunk = true;
            has_ctx = false;
        } else if in_hunk && l.starts_with(' ') {
            has_ctx = true;
        }
    }
    in_hunk && !has_ctx
}

/// Apply a patch built by [`build_patch`] in `dir`.
pub fn apply_patch(repo: &Repo, patch: &str, dir: Direction) -> Result<()> {
    let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
    if dir != Direction::Discard {
        args.push("--cached");
    }
    if dir.reverse() {
        args.push("--reverse");
    }
    if needs_unidiff_zero(patch) {
        args.push("--unidiff-zero");
    }
    args.push("-");
    repo.run_stdin(&args, patch.as_bytes())?;
    Ok(())
}

/// Stage / unstage / discard the `selected` lines of `fd`. `untracked` = the file is not in the index
/// yet (it is registered with `git add -N` before staging).
pub fn apply_lines(
    repo: &Repo,
    fd: &FileDiff,
    selected: &Selection,
    dir: Direction,
    untracked: bool,
) -> Result<()> {
    if fd.binary {
        bail!(
            "{} is a binary file; select the whole file instead",
            fd.path()
        );
    }
    let patch = build_patch(fd, selected, dir.reverse())
        .ok_or_else(|| anyhow!("No changed lines selected"))?;
    let path = fd.path();
    let intent_to_add = untracked && dir == Direction::Stage;
    if intent_to_add {
        repo.run(&["--literal-pathspecs", "add", "-N", "--", path])?;
    }
    let res = apply_patch(repo, &patch, dir);
    if res.is_err() && intent_to_add {
        let _ = repo.run(&[
            "--literal-pathspecs",
            "rm",
            "--cached",
            "-q",
            "--force",
            "--",
            path,
        ]);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn read(repo: &Repo, rel: &str) -> String {
        std::fs::read_to_string(repo.path(rel)).unwrap()
    }

    fn index(repo: &Repo, rel: &str) -> String {
        repo.run(&["show", &format!(":{rel}")]).unwrap()
    }

    /// Select the changed lines whose content (without `\r`) is in `texts`, with the given kind.
    fn pick(fd: &FileDiff, picks: &[(LineKind, &str)]) -> Selection {
        let mut s = Selection::new();
        for (hi, h) in fd.hunks.iter().enumerate() {
            for (li, l) in h.lines.iter().enumerate() {
                let c = l.content.trim_end_matches('\r');
                if picks.iter().any(|(k, t)| *k == l.kind && *t == c) {
                    s.insert((hi, li));
                }
            }
        }
        s
    }

    fn stage(repo: &Repo, path: &str, picks: &[(LineKind, &str)], untracked: bool) {
        let fd = load_diff(repo, path, false, untracked).unwrap().unwrap();
        apply_lines(repo, &fd, &pick(&fd, picks), Direction::Stage, untracked).unwrap();
    }

    use LineKind::{Added as A, Removed as R};

    #[test]
    fn stage_single_added_line() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "init");
        write(&repo, "f.txt", "a\nX\nb\nc\nY\n");
        stage(&repo, "f.txt", &[(A, "Y")], false);
        assert_eq!(index(&repo, "f.txt"), "a\nb\nc\nY\n");
        assert_eq!(read(&repo, "f.txt"), "a\nX\nb\nc\nY\n");
    }

    #[test]
    fn stage_removed_line_only_keeps_unselected_removal_as_context() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "1\n2\n3\n4\n")], "init");
        write(&repo, "f.txt", "1\n4\n");
        stage(&repo, "f.txt", &[(R, "3")], false);
        assert_eq!(index(&repo, "f.txt"), "1\n2\n4\n");
    }

    #[test]
    fn stage_across_hunks_recomputes_offsets() {
        let (_d, repo) = temp_repo();
        let orig: String = (1..=30).map(|i| format!("l{i}\n")).collect();
        commit_files(&repo, &[("f.txt", &orig)], "init");
        let changed = orig
            .replace("l2\n", "l2\nnew-a\nnew-b\n")
            .replace("l25\n", "L25\n");
        write(&repo, "f.txt", &changed);
        let fd = load_diff(&repo, "f.txt", false, false).unwrap().unwrap();
        assert_eq!(fd.hunks.len(), 2);
        // Skip the first hunk entirely, stage only the second.
        let sel = pick(&fd, &[(R, "l25"), (A, "L25")]);
        apply_lines(&repo, &fd, &sel, Direction::Stage, false).unwrap();
        assert_eq!(index(&repo, "f.txt"), orig.replace("l25\n", "L25\n"));
        // Now stage one line of the first hunk.
        stage(&repo, "f.txt", &[(A, "new-b")], false);
        assert_eq!(
            index(&repo, "f.txt"),
            orig.replace("l25\n", "L25\n")
                .replace("l2\n", "l2\nnew-b\n")
        );
    }

    #[test]
    fn unstage_selected_lines() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "init");
        write(&repo, "f.txt", "a\nB\nc\nd\n");
        repo.run(&["add", "f.txt"]).unwrap();
        let fd = load_diff(&repo, "f.txt", true, false).unwrap().unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(A, "d")]),
            Direction::Unstage,
            false,
        )
        .unwrap();
        assert_eq!(index(&repo, "f.txt"), "a\nB\nc\n");
        let fd = load_diff(&repo, "f.txt", true, false).unwrap().unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(R, "b")]),
            Direction::Unstage,
            false,
        )
        .unwrap();
        assert_eq!(index(&repo, "f.txt"), "a\nb\nB\nc\n");
        assert_eq!(read(&repo, "f.txt"), "a\nB\nc\nd\n");
    }

    #[test]
    fn discard_selected_lines() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "init");
        write(&repo, "f.txt", "a\nB\nc\nd\n");
        let fd = load_diff(&repo, "f.txt", false, false).unwrap().unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(R, "b"), (A, "B")]),
            Direction::Discard,
            false,
        )
        .unwrap();
        assert_eq!(read(&repo, "f.txt"), "a\nb\nc\nd\n");
        assert_eq!(index(&repo, "f.txt"), "a\nb\nc\n");
    }

    #[test]
    fn untracked_file_partial_stage_and_discard() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("keep.txt", "k\n")], "init");
        write(&repo, "dir with space/new é.txt", "one\ntwo\nthree\n");
        stage(
            &repo,
            "dir with space/new é.txt",
            &[(A, "one"), (A, "three")],
            true,
        );
        assert_eq!(index(&repo, "dir with space/new é.txt"), "one\nthree\n");
        // Remaining unstaged line can be discarded from the work tree.
        let fd = load_diff(&repo, "dir with space/new é.txt", false, false)
            .unwrap()
            .unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(A, "two")]),
            Direction::Discard,
            false,
        )
        .unwrap();
        assert_eq!(read(&repo, "dir with space/new é.txt"), "one\nthree\n");
    }

    #[test]
    fn untracked_discard_everything_deletes_file() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("keep.txt", "k\n")], "init");
        write(&repo, "n.txt", "x\ny\n");
        let fd = load_diff(&repo, "n.txt", false, true).unwrap().unwrap();
        apply_lines(&repo, &fd, &all_changes(&fd), Direction::Discard, true).unwrap();
        assert!(!repo.path("n.txt").exists());
    }

    #[test]
    fn untracked_stage_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n2\n");
        stage(&repo, "a.txt", &[(A, "2")], true);
        assert_eq!(index(&repo, "a.txt"), "2\n");
        // Unstage it again from the staged diff of an empty repo.
        let fd = load_diff(&repo, "a.txt", true, false).unwrap().unwrap();
        apply_lines(&repo, &fd, &all_changes(&fd), Direction::Unstage, false).unwrap();
        assert!(repo
            .run(&["ls-files", "--", "a.txt"])
            .unwrap()
            .trim()
            .is_empty());
    }

    #[test]
    fn deletions_partial_and_full() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("d.txt", "1\n2\n3\n"), ("e.txt", "x\n")], "init");
        std::fs::remove_file(repo.path("d.txt")).unwrap();
        stage(&repo, "d.txt", &[(R, "2")], false);
        assert_eq!(index(&repo, "d.txt"), "1\n3\n");
        let fd = load_diff(&repo, "d.txt", false, false).unwrap().unwrap();
        apply_lines(&repo, &fd, &all_changes(&fd), Direction::Stage, false).unwrap();
        assert!(repo
            .run(&["ls-files", "--", "d.txt"])
            .unwrap()
            .trim()
            .is_empty());
        // Partially unstage the staged deletion: the file comes back in the index with that line.
        let fd = load_diff(&repo, "d.txt", true, false).unwrap().unwrap();
        assert_eq!(fd.kind, ChangeKind::Deleted);
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(R, "3")]),
            Direction::Unstage,
            false,
        )
        .unwrap();
        assert_eq!(index(&repo, "d.txt"), "3\n");
    }

    #[test]
    fn no_newline_at_end_of_file() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("n.txt", "a\nlast")], "init");
        write(&repo, "n.txt", "a\nlast2\nmore");
        let fd = load_diff(&repo, "n.txt", false, false).unwrap().unwrap();
        // Keep "last" (no newline) and add only "more" after it.
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(A, "more")]),
            Direction::Stage,
            false,
        )
        .unwrap();
        assert_eq!(index(&repo, "n.txt"), "a\nlast\nmore");
        repo.run(&["reset", "-q", "--", "n.txt"]).unwrap();
        // Replace the last line only.
        stage(&repo, "n.txt", &[(R, "last"), (A, "last2")], false);
        assert_eq!(index(&repo, "n.txt"), "a\nlast2\n");
        repo.run(&["reset", "-q", "--", "n.txt"]).unwrap();
        // Remove the last line only: previous line keeps its newline.
        stage(&repo, "n.txt", &[(R, "last")], false);
        assert_eq!(index(&repo, "n.txt"), "a\n");
        // Reverse direction with a no-newline line kept as context.
        repo.run(&["reset", "-q", "--", "n.txt"]).unwrap();
        repo.run(&["add", "n.txt"]).unwrap();
        let fd = load_diff(&repo, "n.txt", true, false).unwrap().unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(R, "last")]),
            Direction::Unstage,
            false,
        )
        .unwrap();
        assert_eq!(index(&repo, "n.txt"), "a\nlast\nlast2\nmore");
    }

    #[test]
    fn untracked_without_trailing_newline() {
        let (_d, repo) = temp_repo();
        write(&repo, "t.txt", "x\ny");
        let fd = untracked_file_diff(&repo, "t.txt").unwrap();
        assert_eq!(fd.hunks[0].lines.last().unwrap().kind, LineKind::NoNewline);
        stage(&repo, "t.txt", &[(A, "x"), (A, "y")], true);
        assert_eq!(index(&repo, "t.txt"), "x\ny");
    }

    #[test]
    fn crlf_content_is_preserved() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("w.txt", "a\r\nb\r\nc\r\n")], "init");
        write(&repo, "w.txt", "a\r\nB\r\nc\r\nd\r\n");
        let fd = load_diff(&repo, "w.txt", false, false).unwrap().unwrap();
        assert!(fd.hunks[0]
            .lines
            .iter()
            .all(|l| l.kind == LineKind::NoNewline || l.content.ends_with('\r')));
        apply_lines(&repo, &fd, &pick(&fd, &[(A, "d")]), Direction::Stage, false).unwrap();
        assert_eq!(index(&repo, "w.txt"), "a\r\nb\r\nc\r\nd\r\n");
        let fd = load_diff(&repo, "w.txt", false, false).unwrap().unwrap();
        apply_lines(
            &repo,
            &fd,
            &pick(&fd, &[(R, "b"), (A, "B")]),
            Direction::Discard,
            false,
        )
        .unwrap();
        assert_eq!(read(&repo, "w.txt"), "a\r\nb\r\nc\r\nd\r\n");
    }

    #[test]
    fn build_patch_pure() {
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,3 @@\n x\n+y\n z\n";
        let fd = parse_unified(diff).pop().unwrap();
        let only_y: Selection = [(1, 1)].into_iter().collect();
        let p = build_patch(&fd, &only_y, false).unwrap();
        assert!(p.contains("@@ -10,2 +10,3 @@\n x\n+y\n z\n"));
        assert!(!p.contains("@@ -1,"));
        let only_b: Selection = [(0, 1)].into_iter().collect();
        let p = build_patch(&fd, &only_b, false).unwrap();
        assert!(p.contains("@@ -1,3 +1,2 @@\n a\n-b\n c\n"));
        // Reverse: unselected '+' becomes context, unselected '-' dropped.
        let p = build_patch(&fd, &only_b, true).unwrap();
        assert!(p.contains("@@ -1,4 +1,3 @@\n a\n-b\n B\n c\n"), "{p}");
        assert!(build_patch(&fd, &Selection::new(), false).is_none());
        // Context lines in the selection are ignored.
        let ctx: Selection = [(0, 0)].into_iter().collect();
        assert!(build_patch(&fd, &ctx, false).is_none());
    }

    #[test]
    fn offsets_shift_for_later_hunks() {
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,4 @@\n a\n+n1\n+n2\n b\n@@ -20,2 +22,2 @@\n x\n-y\n+Y\n";
        let fd = parse_unified(diff).pop().unwrap();
        let sel: Selection = [(0, 1), (1, 1), (1, 2)].into_iter().collect();
        let p = build_patch(&fd, &sel, false).unwrap();
        assert!(p.contains("@@ -1,2 +1,3 @@"));
        assert!(p.contains("@@ -20,2 +21,2 @@"), "{p}");
        let p = build_patch(&fd, &sel, true).unwrap();
        assert!(p.contains("@@ -1,3 +1,4 @@"), "{p}");
        assert!(p.contains("@@ -21,2 +22,2 @@"), "{p}");
    }

    #[test]
    fn quoting_and_zero_context() {
        assert_eq!(quote_path("a b/é.txt"), "a b/é.txt");
        assert_eq!(quote_path("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_path("t\tx"), "\"t\\tx\"");
        assert!(needs_unidiff_zero("@@ -0,0 +1,1 @@\n+x\n"));
        assert!(!needs_unidiff_zero("@@ -1,1 +1,2 @@\n a\n+x\n"));
    }

    #[test]
    fn binary_is_rejected() {
        let (_d, repo) = temp_repo();
        std::fs::write(repo.path("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let fd = load_diff(&repo, "b.bin", false, true).unwrap().unwrap();
        assert!(fd.binary);
        assert!(apply_lines(&repo, &fd, &Selection::new(), Direction::Stage, true).is_err());
    }
}
