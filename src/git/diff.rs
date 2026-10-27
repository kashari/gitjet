//! Unified diff model and loaders. Every diff in the app is parsed into [`FileDiff`]s.

use anyhow::Result;

use super::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` (applies to the previous line)
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line text without the leading `+`/`-`/` ` and without the trailing newline
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Full `@@ -a,b +c,d @@ section` line
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    ModeChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path before the change (None for added files)
    pub old_path: Option<String>,
    /// Path after the change (None for deleted files)
    pub new_path: Option<String>,
    pub kind: ChangeKind,
    pub binary: bool,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// Extended header lines (`index ...`, `similarity index ...`, mode lines...)
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path to show: new path, or old path for deletions.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }

    pub fn added_count(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Added)
            .count()
    }

    pub fn removed_count(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Removed)
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    pub context: u32,
    pub ignore_whitespace: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context: 3,
            ignore_whitespace: false,
        }
    }
}

impl DiffOptions {
    pub fn from_config(c: &crate::config::DiffConfig) -> Self {
        Self {
            context: c.context_lines,
            ignore_whitespace: c.ignore_whitespace,
        }
    }

    /// Flags common to every diff invocation. Prefixes are forced so user config
    /// (`diff.noprefix`, `diff.mnemonicPrefix`, external diff tools) cannot break parsing.
    pub fn args(&self) -> Vec<String> {
        let mut v = vec![
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "--src-prefix=a/".to_string(),
            "--dst-prefix=b/".to_string(),
            format!("-U{}", self.context),
            "-M".to_string(),
        ];
        if self.ignore_whitespace {
            v.push("--ignore-all-space".to_string());
        }
        v
    }
}

impl Repo {
    fn diff_with(
        &self,
        head: &[&str],
        opts: &DiffOptions,
        paths: &[String],
    ) -> Result<Vec<FileDiff>> {
        let extra = opts.args();
        let mut args: Vec<&str> = head.to_vec();
        args.extend(extra.iter().map(String::as_str));
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        Ok(parse_unified(&self.run(&args)?))
    }

    /// Work tree vs index (unstaged changes). Untracked files are not included; see [`untracked_diff`](Self::untracked_diff).
    pub fn diff_unstaged(&self, paths: &[String], opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        self.diff_with(&["diff"], opts, paths)
    }

    /// Index vs HEAD (staged changes).
    pub fn diff_staged(&self, paths: &[String], opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        if self.head_commit().is_none() {
            // Empty repo: compare against the empty tree.
            return self.diff_with(
                &[
                    "diff",
                    "--cached",
                    "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
                ],
                opts,
                paths,
            );
        }
        self.diff_with(&["diff", "--cached"], opts, paths)
    }

    /// Work tree vs HEAD (staged + unstaged).
    pub fn diff_local(&self, paths: &[String], opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        if self.head_commit().is_none() {
            return self.diff_staged(paths, opts);
        }
        self.diff_with(&["diff", "HEAD"], opts, paths)
    }

    /// Changes introduced by one commit (first parent for merges, empty tree for root commits).
    pub fn diff_commit(
        &self,
        rev: &str,
        paths: &[String],
        opts: &DiffOptions,
    ) -> Result<Vec<FileDiff>> {
        self.diff_with(
            &["show", "--format=", "--diff-merges=first-parent", rev],
            opts,
            paths,
        )
    }

    /// Diff between two revisions (`from` → `to`).
    pub fn diff_range(
        &self,
        from: &str,
        to: &str,
        paths: &[String],
        opts: &DiffOptions,
    ) -> Result<Vec<FileDiff>> {
        self.diff_with(&["diff", from, to], opts, paths)
    }

    /// Diff between a revision and the work tree.
    pub fn diff_rev_to_worktree(
        &self,
        rev: &str,
        paths: &[String],
        opts: &DiffOptions,
    ) -> Result<Vec<FileDiff>> {
        self.diff_with(&["diff", rev], opts, paths)
    }

    /// Diff of two arbitrary files on disk (absolute or repo-relative paths).
    pub fn diff_files(&self, left: &str, right: &str, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        let extra = opts.args();
        let mut args: Vec<&str> = vec!["diff", "--no-index"];
        args.extend(extra.iter().map(String::as_str));
        args.extend(["--", left, right]);
        Ok(parse_unified(&self.run_allow(&args, &[0, 1])?))
    }

    /// A synthetic "all lines added" diff for an untracked file.
    pub fn untracked_diff(&self, path: &str) -> Result<FileDiff> {
        let bytes = std::fs::read(self.path(path))?;
        let binary = bytes.iter().take(8000).any(|b| *b == 0);
        let mut fd = FileDiff {
            old_path: None,
            new_path: Some(path.to_string()),
            kind: ChangeKind::Added,
            binary,
            old_mode: None,
            new_mode: None,
            header: vec!["new file (untracked)".to_string()],
            hunks: Vec::new(),
        };
        if !binary {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<DiffLine> = text
                .lines()
                .enumerate()
                .map(|(i, l)| DiffLine {
                    kind: LineKind::Added,
                    content: l.strip_suffix('\r').unwrap_or(l).to_string(),
                    old_no: None,
                    new_no: Some(i as u32 + 1),
                })
                .collect();
            if !lines.is_empty() {
                fd.hunks.push(Hunk {
                    header: format!("@@ -0,0 +1,{} @@", lines.len()),
                    old_start: 0,
                    old_lines: 0,
                    new_start: 1,
                    new_lines: lines.len() as u32,
                    lines,
                });
            }
        }
        Ok(fd)
    }
}

fn strip_prefix_path(s: &str, prefix: &str) -> Option<String> {
    let s = s.trim_end_matches('\t').trim_end();
    if s == "/dev/null" {
        return None;
    }
    let unq = unquote(s);
    Some(unq.strip_prefix(prefix).unwrap_or(&unq).to_string())
}

/// Undo git's C-style quoting of unusual paths (`"a\tb"`).
pub fn unquote(s: &str) -> String {
    if !(s.len() >= 2 && s.starts_with('"') && s.ends_with('"')) {
        return s.to_string();
    }
    let inner = &s[1..s.len() - 1];
    let mut bytes = Vec::new();
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'"') => bytes.push(b'"'),
            Some(b'\\') => bytes.push(b'\\'),
            Some(d @ b'0'..=b'7') => {
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    if let Some(&n @ b'0'..=b'7') = it.peek() {
                        v = v * 8 + (n - b'0') as u32;
                        it.next();
                    }
                }
                bytes.push(v as u8);
            }
            Some(o) => bytes.push(o),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_range(s: &str) -> (u32, u32) {
    let s = &s[1..];
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

/// Parse `git diff` / `git show` / `git diff --no-index` / `.patch` output.
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut in_hunk = false;

    let finish = |cur: &mut Option<FileDiff>, files: &mut Vec<FileDiff>| {
        if let Some(f) = cur.take() {
            files.push(f);
        }
    };

    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish(&mut cur, &mut files);
            in_hunk = false;
            // Best-effort paths from the header; refined by ---/+++/rename lines.
            let (a, b) = split_git_header(rest);
            cur = Some(FileDiff {
                old_path: a,
                new_path: b,
                kind: ChangeKind::Modified,
                binary: false,
                old_mode: None,
                new_mode: None,
                header: Vec::new(),
                hunks: Vec::new(),
            });
            continue;
        }
        if line.starts_with("diff --cc ") || line.starts_with("diff --combined ") {
            finish(&mut cur, &mut files);
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else {
            // `.patch` files from format-patch have a mail header before the first diff.
            continue;
        };
        if line.starts_with("@@ ") {
            let parts: Vec<&str> = line.split(' ').collect();
            if parts.len() >= 3 {
                let (os, ol) = parse_range(parts[1]);
                let (ns, nl) = parse_range(parts[2]);
                old_no = os;
                new_no = ns;
                f.hunks.push(Hunk {
                    header: line.to_string(),
                    old_start: os,
                    old_lines: ol,
                    new_start: ns,
                    new_lines: nl,
                    lines: Vec::new(),
                });
                in_hunk = true;
            }
            continue;
        }
        if in_hunk {
            let h = f.hunks.last_mut().expect("hunk");
            let seen_old = h
                .lines
                .iter()
                .filter(|l| matches!(l.kind, LineKind::Context | LineKind::Removed))
                .count() as u32;
            let seen_new = h
                .lines
                .iter()
                .filter(|l| matches!(l.kind, LineKind::Context | LineKind::Added))
                .count() as u32;
            if seen_old >= h.old_lines && seen_new >= h.new_lines && !line.starts_with('\\') {
                // Hunk complete; anything else is trailer (e.g. format-patch signature).
                in_hunk = false;
                continue;
            }
            let (kind, content) = match line.as_bytes().first() {
                Some(b'+') => (LineKind::Added, &line[1..]),
                Some(b'-') => (LineKind::Removed, &line[1..]),
                Some(b' ') => (LineKind::Context, &line[1..]),
                Some(b'\\') => (LineKind::NoNewline, line),
                None => (LineKind::Context, ""),
                _ => {
                    in_hunk = false;
                    continue;
                }
            };
            let (o, n) = match kind {
                LineKind::Added => {
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Removed => {
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
                LineKind::NoNewline => (None, None),
            };
            h.lines.push(DiffLine {
                kind,
                content: content.to_string(),
                old_no: o,
                new_no: n,
            });
            continue;
        }
        // Extended header lines.
        if let Some(p) = line.strip_prefix("--- ") {
            f.old_path = strip_prefix_path(p, "a/");
            if f.old_path.is_none() {
                f.kind = ChangeKind::Added;
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            f.new_path = strip_prefix_path(p, "b/");
            if f.new_path.is_none() {
                f.kind = ChangeKind::Deleted;
            }
        } else if let Some(m) = line.strip_prefix("new file mode ") {
            f.kind = ChangeKind::Added;
            f.new_mode = Some(m.to_string());
            f.old_path = None;
            f.header.push(line.to_string());
        } else if let Some(m) = line.strip_prefix("deleted file mode ") {
            f.kind = ChangeKind::Deleted;
            f.old_mode = Some(m.to_string());
            f.new_path = None;
            f.header.push(line.to_string());
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.kind = ChangeKind::Renamed;
            f.old_path = Some(unquote(p));
            f.header.push(line.to_string());
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.kind = ChangeKind::Renamed;
            f.new_path = Some(unquote(p));
            f.header.push(line.to_string());
        } else if let Some(p) = line.strip_prefix("copy from ") {
            f.kind = ChangeKind::Copied;
            f.old_path = Some(unquote(p));
            f.header.push(line.to_string());
        } else if let Some(p) = line.strip_prefix("copy to ") {
            f.kind = ChangeKind::Copied;
            f.new_path = Some(unquote(p));
            f.header.push(line.to_string());
        } else if let Some(m) = line.strip_prefix("old mode ") {
            f.old_mode = Some(m.to_string());
            if f.kind == ChangeKind::Modified {
                f.kind = ChangeKind::ModeChanged;
            }
            f.header.push(line.to_string());
        } else if let Some(m) = line.strip_prefix("new mode ") {
            f.new_mode = Some(m.to_string());
            f.header.push(line.to_string());
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
            f.header.push(line.to_string());
        } else if !line.is_empty() {
            f.header.push(line.to_string());
        }
    }
    finish(&mut cur, &mut files);
    for f in &mut files {
        if f.kind == ChangeKind::ModeChanged && !f.hunks.is_empty() {
            f.kind = ChangeKind::Modified;
        }
    }
    files
}

/// Split `a/x b/y` from a `diff --git` header. Ambiguous with spaces; prefers the symmetric split.
fn split_git_header(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(quoted) = rest.strip_prefix('"') {
        if let Some(end) = quoted.find("\" ") {
            let a = &rest[..end + 2];
            let b = &rest[end + 3..];
            return (strip_prefix_path(a, "a/"), strip_prefix_path(b, "b/"));
        }
    }
    let bytes = rest.as_bytes();
    // Symmetric split: "a/<p> b/<p>"
    if rest.len() % 2 == 1 {
        let mid = rest.len() / 2;
        if bytes[mid] == b' ' {
            let (a, b) = (&rest[..mid], &rest[mid + 1..]);
            if a.len() > 2 && b.len() > 2 && a[2..] == b[2..] {
                return (strip_prefix_path(a, "a/"), strip_prefix_path(b, "b/"));
            }
        }
    }
    match rest.find(" b/") {
        Some(i) => (
            strip_prefix_path(&rest[..i], "a/"),
            strip_prefix_path(&rest[i + 1..], "b/"),
        ),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    const SAMPLE: &str = "diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@ fn main() {
 one
-two
+TWO
+three
 four
diff --git a/new file.txt b/new file.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new file.txt
@@ -0,0 +1 @@
+hello
\\ No newline at end of file
diff --git a/old.txt b/renamed.txt
similarity index 100%
rename from old.txt
rename to renamed.txt
diff --git a/img.png b/img.png
index 4444444..5555555 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_sample() {
        let files = parse_unified(SAMPLE);
        assert_eq!(files.len(), 4);
        let f = &files[0];
        assert_eq!(f.path(), "src/main.rs");
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.added_count(), 2);
        assert_eq!(f.removed_count(), 1);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[4].old_no, Some(3));
        assert_eq!(h.lines[4].new_no, Some(4));
        assert_eq!(files[1].kind, ChangeKind::Added);
        assert_eq!(files[1].path(), "new file.txt");
        assert_eq!(
            files[1].hunks[0].lines.last().unwrap().kind,
            LineKind::NoNewline
        );
        assert_eq!(files[2].kind, ChangeKind::Renamed);
        assert_eq!(files[2].old_path.as_deref(), Some("old.txt"));
        assert_eq!(files[2].new_path.as_deref(), Some("renamed.txt"));
        assert!(files[3].binary);
    }

    #[test]
    fn unquotes() {
        assert_eq!(unquote("\"a\\tb\""), "a\tb");
        assert_eq!(unquote("\"\\303\\251\""), "é");
        assert_eq!(unquote("plain"), "plain");
    }

    #[test]
    fn real_diffs() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n2\n3\n")], "one");
        write(&repo, "a.txt", "1\nX\n3\n");
        let o = DiffOptions::default();
        let d = repo.diff_unstaged(&[], &o).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].added_count(), 1);
        assert!(repo.diff_staged(&[], &o).unwrap().is_empty());
        repo.run(&["add", "a.txt"]).unwrap();
        assert_eq!(repo.diff_staged(&[], &o).unwrap().len(), 1);
        let c2 = commit_files(&repo, &[], "two");
        assert_eq!(repo.diff_commit(&c2, &[], &o).unwrap().len(), 1);
        assert_eq!(
            repo.diff_commit(&c1, &[], &o).unwrap()[0].kind,
            ChangeKind::Added
        );
        assert_eq!(repo.diff_range(&c1, &c2, &[], &o).unwrap().len(), 1);
        write(&repo, "u.txt", "x\ny\n");
        assert_eq!(repo.untracked_diff("u.txt").unwrap().added_count(), 2);
        write(&repo, "l.txt", "a\n");
        write(&repo, "r.txt", "b\n");
        assert_eq!(repo.diff_files("l.txt", "r.txt", &o).unwrap().len(), 1);
    }

    #[test]
    fn staged_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        repo.run(&["add", "a.txt"]).unwrap();
        let d = repo.diff_staged(&[], &DiffOptions::default()).unwrap();
        assert_eq!(d.len(), 1);
    }
}
