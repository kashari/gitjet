//! Compare with branch/revision, compare two arbitrary files, helpers used by the diff viewer.
//!
//! The UI for these comparisons lives in `ui::diff_view` (palette entries); this module only talks to git.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};

use super::Repo;

/// What kind of ref a [`RefEntry`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

impl RefKind {
    /// Short label shown next to the ref in choosers.
    pub fn label(self) -> &'static str {
        match self {
            RefKind::Local => "branch",
            RefKind::Remote => "remote",
            RefKind::Tag => "tag",
        }
    }
}

/// A branch, remote-tracking branch or tag that can be compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    /// Short name usable as a revision (`main`, `origin/main`, `v1.0`)
    pub name: String,
    /// Full ref name (`refs/heads/main`)
    pub full: String,
    pub kind: RefKind,
}

/// Branches (local, then remote) and tags, most recently updated first within each group.
/// Symbolic `refs/remotes/*/HEAD` entries are skipped.
pub fn list_refs(repo: &Repo) -> Result<Vec<RefEntry>> {
    let out = repo.run(&[
        "for-each-ref",
        "--sort=-creatordate",
        "--format=%(refname)%00%(refname:short)%00%(symref)",
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ])?;
    Ok(parse_refs(&out))
}

/// Parse the `for-each-ref` output produced by [`list_refs`].
pub fn parse_refs(out: &str) -> Vec<RefEntry> {
    let mut v: Vec<RefEntry> = out
        .lines()
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut parts = line.split('\0');
            let full = parts.next()?.to_string();
            let name = parts.next()?.to_string();
            let symref = parts.next().unwrap_or("");
            if !symref.is_empty() || name.is_empty() {
                return None;
            }
            let kind = if full.starts_with("refs/heads/") {
                RefKind::Local
            } else if full.starts_with("refs/remotes/") {
                RefKind::Remote
            } else if full.starts_with("refs/tags/") {
                RefKind::Tag
            } else {
                return None;
            };
            Some(RefEntry { name, full, kind })
        })
        .collect();
    // Stable: keeps the recency order inside each group.
    v.sort_by_key(|r| r.kind);
    v
}

/// Best common ancestor of `a` and `b` (full hash).
pub fn merge_base(repo: &Repo, a: &str, b: &str) -> Result<String> {
    let out = repo
        .run_allow(&["merge-base", a, b], &[0, 1])
        .map_err(|e| anyhow!("could not find a merge base of {a} and {b}: {e:#}"))?;
    let base = out.trim();
    if base.is_empty() {
        bail!("{a} and {b} have no common history");
    }
    Ok(base.to_string())
}

/// Resolve user input (`main`, `HEAD~2`, an abbreviated hash...) to a full commit hash, with a friendly error.
pub fn resolve_revision(repo: &Repo, rev: &str) -> Result<String> {
    let rev = rev.trim();
    if rev.is_empty() {
        bail!("no revision given");
    }
    repo.rev_parse(rev)
        .map_err(|_| anyhow!("'{rev}' is not a known branch, tag or commit"))
}

/// `(from, to)` for "what does `rev` contain since it diverged from HEAD" (`git diff HEAD...rev`).
pub fn since_merge_base(repo: &Repo, rev: &str) -> Result<(String, String)> {
    if repo.head_commit().is_none() {
        bail!("the repository has no commits yet");
    }
    resolve_revision(repo, rev)?;
    let base = merge_base(repo, "HEAD", rev)?;
    Ok((base, rev.to_string()))
}

/// Every path with local changes (staged, unstaged, untracked, conflicted), including the source
/// paths of renames so rename detection still works when the list is used as a pathspec.
pub fn changed_paths(repo: &Repo) -> Result<Vec<String>> {
    let st = repo.status()?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in st.entries.iter().filter(|e| !e.is_ignored()) {
        for p in std::iter::once(&e.path).chain(e.orig_path.iter()) {
            if seen.insert(p.clone()) {
                out.push(p.clone());
            }
        }
    }
    Ok(out)
}

/// Turn user input into an absolute path of an existing file: `~` expands to the home directory,
/// relative paths are relative to the work tree root. Surrounding quotes are removed.
pub fn resolve_file(repo: &Repo, input: &str) -> Result<PathBuf> {
    let s = input.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s);
    if s.is_empty() {
        bail!("no file given");
    }
    let expanded = expand_home(s);
    let p = if expanded.is_absolute() {
        expanded
    } else {
        repo.root.join(expanded)
    };
    if !p.exists() {
        bail!("{} does not exist", p.display());
    }
    if p.is_dir() {
        bail!("{} is a directory; choose a file", p.display());
    }
    Ok(p)
}

fn expand_home(s: &str) -> PathBuf {
    let rest = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"));
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if s == "~" => dirs::home_dir().unwrap_or_else(|| PathBuf::from(s)),
        _ => PathBuf::from(s),
    }
}

/// How to show a path in titles: relative to the work tree when inside it, otherwise as given.
pub fn display_path(repo: &Repo, p: &Path) -> String {
    match p.strip_prefix(&repo.root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    #[test]
    fn parses_refs_and_groups() {
        let out = "refs/tags/v1\0v1\0\nrefs/remotes/origin/HEAD\0origin\0refs/remotes/origin/main\n\
                   refs/remotes/origin/main\0origin/main\0\nrefs/heads/main\0main\0\r\nrefs/heads/feat\0feat\0\n";
        let r = parse_refs(out);
        let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["main", "feat", "origin/main", "v1"]);
        assert_eq!(r[2].kind, RefKind::Remote);
        assert_eq!(r[3].kind, RefKind::Tag);
        assert_eq!(r[0].full, "refs/heads/main");
    }

    #[test]
    fn lists_real_refs() {
        let (_d, repo) = temp_repo();
        assert!(list_refs(&repo).unwrap().is_empty());
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        repo.run(&["branch", "feature/x"]).unwrap();
        repo.run(&["tag", "v1.0"]).unwrap();
        let r = list_refs(&repo).unwrap();
        assert!(r
            .iter()
            .any(|e| e.name == "main" && e.kind == RefKind::Local));
        assert!(r
            .iter()
            .any(|e| e.name == "feature/x" && e.kind == RefKind::Local));
        assert!(r.iter().any(|e| e.name == "v1.0" && e.kind == RefKind::Tag));
        assert_eq!(r.last().unwrap().kind, RefKind::Tag);
    }

    #[test]
    fn merge_base_and_since() {
        let (_d, repo) = temp_repo();
        let base = commit_files(&repo, &[("a.txt", "1\n")], "base");
        repo.run(&["checkout", "-q", "-b", "topic"]).unwrap();
        let topic = commit_files(&repo, &[("b.txt", "topic\n")], "topic");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("c.txt", "main\n")], "main work");
        assert_eq!(merge_base(&repo, "HEAD", "topic").unwrap(), base);
        let (from, to) = since_merge_base(&repo, "topic").unwrap();
        assert_eq!(from, base);
        assert_eq!(to, "topic");
        // Only the topic's own change shows up, not main's.
        let d = repo
            .diff_range(&from, &to, &[], &crate::git::diff::DiffOptions::default())
            .unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path(), "b.txt");
        assert_eq!(resolve_revision(&repo, "topic").unwrap(), topic);
        assert!(resolve_revision(&repo, "nope").is_err());
        assert!(resolve_revision(&repo, "  ").is_err());
    }

    #[test]
    fn unrelated_histories_have_no_merge_base() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        repo.run(&["checkout", "-q", "--orphan", "other"]).unwrap();
        repo.run(&["rm", "-rfq", "."]).unwrap();
        commit_files(&repo, &[("b.txt", "2\n")], "two");
        assert!(merge_base(&repo, "main", "other").is_err());
    }

    #[test]
    fn since_merge_base_needs_commits() {
        let (_d, repo) = temp_repo();
        assert!(since_merge_base(&repo, "main").is_err());
    }

    #[test]
    fn changed_paths_include_untracked_and_rename_sources() {
        let (_d, repo) = temp_repo();
        commit_files(
            &repo,
            &[("a.txt", "1\n"), ("old name.txt", "x\ny\nz\n")],
            "one",
        );
        write(&repo, "a.txt", "2\n");
        write(&repo, "dir/new é.txt", "n\n");
        repo.run(&["mv", "old name.txt", "renamed.txt"]).unwrap();
        let p = changed_paths(&repo).unwrap();
        assert!(p.contains(&"a.txt".to_string()));
        assert!(p.contains(&"dir/new é.txt".to_string()));
        assert!(p.contains(&"renamed.txt".to_string()));
        assert!(p.contains(&"old name.txt".to_string()));
        let mut dedup = p.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), p.len());
    }

    #[test]
    fn resolves_files() {
        let (_d, repo) = temp_repo();
        write(&repo, "sub/a b.txt", "x\n");
        let p = resolve_file(&repo, " \"sub/a b.txt\" ").unwrap();
        assert!(p.is_absolute());
        assert_eq!(display_path(&repo, &p), "sub/a b.txt");
        assert!(resolve_file(&repo, "missing.txt").is_err());
        assert!(resolve_file(&repo, "sub").is_err());
        assert!(resolve_file(&repo, "").is_err());
        let abs = repo.root.join("sub").join("a b.txt");
        assert_eq!(resolve_file(&repo, &abs.to_string_lossy()).unwrap(), abs);
    }
}
