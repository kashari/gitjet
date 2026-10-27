//! File history (`git log --follow`), line-range history (`git log -L`), file content at a revision.

use anyhow::{bail, Result};

use super::diff::{parse_unified, unquote, FileDiff};
use super::log::{short_ref, Commit};
use super::Repo;

/// One commit in the history of a file, with the file's path at that commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevision {
    pub commit: Commit,
    /// Path of the file in this commit (after the change)
    pub path: String,
    /// Path before the change, for renames and copies
    pub old_path: Option<String>,
    /// Name-status letter: `A`, `M`, `D`, `R`, `C`, `T` (`M` for merges without a listed change)
    pub status: char,
}

/// One commit of a line-range history with the diff hunks touching the range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRevision {
    pub commit: Commit,
    /// Path of the file in this commit
    pub path: String,
    pub diffs: Vec<FileDiff>,
}

/// Fields of the commit header used by the history parsers (NUL separated, record starts with RS).
const FORMAT: &str =
    "--format=%x1e%H%x00%h%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%s%x00%D%x00";

/// True when the bytes look binary (a NUL in the first 8000 bytes, like git's heuristic).
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Split text into lines, accepting LF and CRLF endings.
pub fn text_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

impl Repo {
    /// Raw content of `path` at `rev` (`git show rev:path`).
    pub fn show_file(&self, rev: &str, path: &str) -> Result<Vec<u8>> {
        let spec = format!("{rev}:{}", path.trim_start_matches("./"));
        self.run_bytes(&["show", "--no-textconv", &spec, "--"])
    }

    /// Restore the work tree file `dest` to the content `rev_path` had at `rev`.
    ///
    /// When the paths match this is `git checkout <rev> -- <path>` (index and work tree); across a
    /// rename the old content is written to the current path so the file keeps its new name.
    pub fn restore_file(&self, rev: &str, rev_path: &str, dest: &str) -> Result<()> {
        if rev_path == dest {
            self.run(&["checkout", rev, "--", dest])?;
            return Ok(());
        }
        let bytes = self.show_file(rev, rev_path)?;
        let abs = self.path(dest);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&abs, bytes)?;
        Ok(())
    }

    /// Commits that touched `path`, newest first, following renames (at most `max`).
    pub fn file_log(&self, path: &str, max: usize) -> Result<Vec<FileRevision>> {
        if self.head_commit().is_none() {
            return Ok(Vec::new());
        }
        let max_arg = format!("--max-count={max}");
        let out = self.run(&[
            "log",
            FORMAT,
            "--decorate=full",
            "--follow",
            "--name-status",
            "-M",
            "--no-ext-diff",
            &max_arg,
            "--",
            path,
        ])?;
        Ok(parse_file_log(&out, path))
    }

    /// History of lines `start..=end` (1-based) of `path` as seen at `rev` (HEAD when `None`).
    pub fn line_log(
        &self,
        path: &str,
        start: u32,
        end: u32,
        rev: Option<&str>,
        max: usize,
    ) -> Result<Vec<LineRevision>> {
        if start == 0 || end < start {
            bail!("invalid line range {start},{end}");
        }
        if rev.is_none() && self.head_commit().is_none() {
            return Ok(Vec::new());
        }
        let range = format!("-L{start},{end}:{path}");
        let max_arg = format!("--max-count={max}");
        let mut args = vec![
            "log",
            FORMAT,
            "--decorate=full",
            &range,
            &max_arg,
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ];
        if let Some(r) = rev {
            args.push(r);
        }
        let out = self.run(&args)?;
        Ok(parse_line_log(&out, path))
    }
}

/// Split one record into the commit and whatever git printed after the header.
fn parse_commit_record(rec: &str) -> Option<(Commit, &str)> {
    let mut f = rec.splitn(12, '\0');
    let mut next = || f.next();
    let id = next()?.to_string();
    let short = next()?.to_string();
    let parents = next()?.split_whitespace().map(str::to_string).collect();
    let author = next()?.to_string();
    let email = next()?.to_string();
    let time = next()?.parse().unwrap_or(0);
    let committer = next()?.to_string();
    let committer_email = next()?.to_string();
    let commit_time = next()?.parse().unwrap_or(0);
    let subject = next()?.to_string();
    let refs = next()?
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(short_ref)
        .collect();
    let rest = next().unwrap_or("");
    if id.is_empty() {
        return None;
    }
    Some((
        Commit {
            id,
            short,
            parents,
            author,
            email,
            time,
            committer,
            committer_email,
            commit_time,
            subject,
            refs,
        },
        rest,
    ))
}

/// Parse `git log --follow --name-status` output for a file currently called `path`.
///
/// Walking from newest to oldest, a rename `R old new` means older commits know the file as `old`.
pub fn parse_file_log(out: &str, path: &str) -> Vec<FileRevision> {
    let mut current = path.to_string();
    let mut res = Vec::new();
    for rec in out.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let Some((commit, rest)) = parse_commit_record(rec) else {
            continue;
        };
        let mut status = 'M';
        let mut new_path = current.clone();
        let mut old_path = None;
        for line in rest
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.is_empty())
        {
            let mut parts = line.split('\t');
            let code = parts.next().unwrap_or("");
            let Some(c) = code.chars().next().filter(|c| c.is_ascii_uppercase()) else {
                continue;
            };
            let first = parts.next().map(unquote);
            let second = parts.next().map(unquote);
            match (c, first, second) {
                // With --follow only the followed file is listed, but be defensive.
                ('R' | 'C', Some(old), Some(new)) if new == current || old_path.is_none() => {
                    status = c;
                    new_path = new;
                    old_path = Some(old);
                }
                ('R' | 'C', _, _) => {}
                (_, Some(p), _) if p == current || old_path.is_none() => {
                    status = c;
                    new_path = p;
                }
                _ => {}
            }
        }
        if let Some(o) = &old_path {
            current = o.clone();
        } else {
            current = new_path.clone();
        }
        res.push(FileRevision {
            commit,
            path: new_path,
            old_path,
            status,
        });
    }
    res
}

/// Parse `git log -L` output (commit headers followed by the hunks touching the range).
pub fn parse_line_log(out: &str, path: &str) -> Vec<LineRevision> {
    let mut current = path.to_string();
    out.split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|rec| {
            let (commit, rest) = parse_commit_record(rec)?;
            let diffs = parse_unified(rest);
            if let Some(d) = diffs.first() {
                current = d
                    .new_path
                    .clone()
                    .or_else(|| d.old_path.clone())
                    .unwrap_or(current.clone());
            }
            let rev = LineRevision {
                commit,
                path: current.clone(),
                diffs,
            };
            if let Some(old) = rev.diffs.first().and_then(|d| d.old_path.clone()) {
                current = old;
            }
            Some(rev)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::LineKind;
    use crate::git::testutil::*;

    #[test]
    fn file_log_follows_renames() {
        let (_d, repo) = temp_repo();
        assert!(repo.file_log("a.txt", 10).unwrap().is_empty());
        let c1 = commit_files(
            &repo,
            &[("a.txt", "1\n2\n3\n"), ("other.txt", "o\n")],
            "add a",
        );
        let c2 = commit_files(&repo, &[("a.txt", "1\n2\n3\n4\n")], "edit a");
        commit_files(&repo, &[("other.txt", "o2\n")], "unrelated");
        std::fs::create_dir_all(repo.path("dir with space")).unwrap();
        repo.run(&["mv", "a.txt", "dir with space/b.txt"]).unwrap();
        repo.run(&["commit", "-q", "-m", "move a"]).unwrap();
        let c4 = repo.head_commit().unwrap();
        let c5 = commit_files(
            &repo,
            &[("dir with space/b.txt", "1\n2\n3\n4\n5\n")],
            "edit b",
        );

        let h = repo.file_log("dir with space/b.txt", 100).unwrap();
        let ids: Vec<&str> = h.iter().map(|r| r.commit.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![c5.as_str(), c4.as_str(), c2.as_str(), c1.as_str()]
        );
        assert_eq!(h[0].status, 'M');
        assert_eq!(h[0].path, "dir with space/b.txt");
        assert_eq!(h[1].status, 'R');
        assert_eq!(h[1].old_path.as_deref(), Some("a.txt"));
        assert_eq!(h[2].path, "a.txt");
        assert_eq!(h[3].status, 'A');
        assert_eq!(h[3].commit.subject, "add a");
        assert!(h[0].commit.refs.iter().any(|r| r == "HEAD -> main"));

        assert_eq!(repo.file_log("dir with space/b.txt", 2).unwrap().len(), 2);
    }

    #[test]
    fn line_log_tracks_range() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("f.txt", "a\nb\nc\nd\n")], "one");
        let c2 = commit_files(&repo, &[("f.txt", "a\nB\nc\nd\n")], "two");
        commit_files(&repo, &[("f.txt", "a\nB\nc\nD\n")], "three");
        let h = repo.line_log("f.txt", 2, 2, None, 50).unwrap();
        let ids: Vec<&str> = h.iter().map(|r| r.commit.id.as_str()).collect();
        assert_eq!(ids, vec![c2.as_str(), c1.as_str()]);
        assert_eq!(h[0].path, "f.txt");
        let lines = &h[0].diffs[0].hunks[0].lines;
        assert!(lines
            .iter()
            .any(|l| l.kind == LineKind::Added && l.content == "B"));
        assert!(lines
            .iter()
            .any(|l| l.kind == LineKind::Removed && l.content == "b"));
        assert!(repo.line_log("f.txt", 3, 1, None, 10).is_err());
        let at_c1 = repo.line_log("f.txt", 1, 4, Some(&c1), 10).unwrap();
        assert_eq!(at_c1.len(), 1);
    }

    #[test]
    fn show_file_at_revisions() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("ü dir/f.txt", "v1\r\nline\r\n")], "one");
        commit_files(&repo, &[("ü dir/f.txt", "v2\n")], "two");
        let old = repo.show_file(&c1, "ü dir/f.txt").unwrap();
        assert_eq!(text_lines(&old), vec!["v1", "line"]);
        assert_eq!(
            text_lines(&repo.show_file("HEAD", "ü dir/f.txt").unwrap()),
            vec!["v2"]
        );
        assert!(repo.show_file("HEAD", "missing.txt").is_err());
        assert!(!is_binary(&old));
        assert!(is_binary(b"a\0b"));
    }

    #[test]
    fn restore_file_same_and_renamed_path() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "old\n")], "one");
        commit_files(&repo, &[("a.txt", "new\n")], "two");
        repo.restore_file(&c1, "a.txt", "a.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path("a.txt")).unwrap(),
            "old\n"
        );
        repo.run(&["mv", "a.txt", "b.txt"]).unwrap();
        repo.run(&["commit", "-q", "-m", "mv"]).unwrap();
        commit_files(&repo, &[("b.txt", "newest\n")], "three");
        repo.restore_file(&c1, "a.txt", "b.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path("b.txt")).unwrap(),
            "old\n"
        );
        assert!(!repo.path("a.txt").exists());
        assert!(repo.restore_file(&c1, "missing", "x").is_err());
    }

    #[test]
    fn parse_file_log_sample() {
        let rec = |id: &str, rest: &str| {
            format!("\x1e{id}\0{id}\0\0A\0a@x\0100\0A\0a@x\0100\0subj {id}\0\0\n\n{rest}")
        };
        let out = format!(
            "{}{}{}",
            rec("c3", "M\tnew.txt\n"),
            rec("c2", "R087\t\"old\\tname\"\tnew.txt\n"),
            rec("c1", "A\t\"old\\tname\"\n")
        );
        let h = parse_file_log(&out, "new.txt");
        assert_eq!(h.len(), 3);
        assert_eq!(h[1].status, 'R');
        assert_eq!(h[1].old_path.as_deref(), Some("old\tname"));
        assert_eq!(h[2].path, "old\tname");
        assert_eq!(h[2].status, 'A');
        assert_eq!(h[0].commit.subject, "subj c3");
    }
}
