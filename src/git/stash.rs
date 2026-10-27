//! Stash: list, show, save (untracked/keep-index/message), apply, pop, drop, clear, branch from stash.

use anyhow::{bail, Result};

use super::diff::{DiffOptions, FileDiff};
use super::patch::{nul_paths, EMPTY_TREE};
use super::Repo;

/// One entry of `git stash list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// N in `stash@{N}`
    pub index: usize,
    /// Commit hash of the stash
    pub hash: String,
    /// Branch the stash was created on (`None` for detached HEAD)
    pub branch: Option<String>,
    /// Message, or the HEAD subject for automatic "WIP on" stashes
    pub message: String,
    /// True for stashes without a user message ("WIP on <branch>")
    pub wip: bool,
    /// Creation time (unix seconds)
    pub time: i64,
    /// Parents: HEAD at stash time, index commit, and the untracked-files commit when present
    pub parents: Vec<String>,
}

impl StashEntry {
    /// `stash@{N}`
    pub fn refname(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }

    /// The commit holding untracked files (third parent), if the stash has one.
    pub fn untracked_commit(&self) -> Option<&str> {
        self.parents.get(2).map(String::as_str)
    }
}

/// Options for `git stash push`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashOptions {
    pub message: String,
    pub include_untracked: bool,
    pub keep_index: bool,
}

/// Result of applying/popping a stash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StashApplyOutcome {
    Clean,
    /// Applied with conflicts in these files (a popped stash is kept)
    Conflicts(Vec<String>),
}

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';

/// All stashes, newest first.
pub fn list(repo: &Repo) -> Result<Vec<StashEntry>> {
    if repo
        .try_run(&["rev-parse", "--verify", "-q", "refs/stash"])
        .is_none()
    {
        return Ok(Vec::new());
    }
    let out = repo.run(&[
        "stash",
        "list",
        "--format=%gd%x1f%H%x1f%ct%x1f%P%x1f%gs%x1e",
    ])?;
    Ok(parse_list(&out))
}

/// Parse the `--format=%gd%x1f%H%x1f%ct%x1f%P%x1f%gs%x1e` output of `git stash list`.
pub fn parse_list(out: &str) -> Vec<StashEntry> {
    let mut v = Vec::new();
    for rec in out.split(RECORD) {
        let rec = rec.trim_matches(|c| c == '\n' || c == '\r');
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(5, FIELD).collect();
        if f.len() < 5 {
            continue;
        }
        let Some(index) = f[0]
            .strip_prefix("stash@{")
            .and_then(|s| s.strip_suffix('}'))
            .and_then(|s| s.parse().ok())
        else {
            continue;
        };
        let (branch, message, wip) = parse_subject(f[4]);
        v.push(StashEntry {
            index,
            hash: f[1].to_string(),
            time: f[2].parse().unwrap_or(0),
            parents: f[3].split_whitespace().map(str::to_string).collect(),
            branch,
            message,
            wip,
        });
    }
    v
}

/// Split a stash reflog subject (`WIP on main: abc123 subject` / `On main: message`).
fn parse_subject(s: &str) -> (Option<String>, String, bool) {
    let (rest, wip) = if let Some(r) = s.strip_prefix("WIP on ") {
        (r, true)
    } else if let Some(r) = s.strip_prefix("On ") {
        (r, false)
    } else {
        return (None, s.to_string(), false);
    };
    match rest.split_once(": ") {
        Some((b, m)) => {
            let branch = (b != "(no branch)").then(|| b.to_string());
            (branch, m.to_string(), wip)
        }
        None => (None, rest.trim_end_matches(':').to_string(), wip),
    }
}

/// `git stash push`; `paths` limits the stash to those files (empty = everything).
/// Returns git's message (e.g. "No local changes to save").
pub fn push(repo: &Repo, opts: &StashOptions, paths: &[String]) -> Result<String> {
    if repo.head_commit().is_none() {
        bail!("Cannot stash in a repository without commits");
    }
    let mut args = vec!["stash", "push"];
    if opts.include_untracked {
        args.push("--include-untracked");
    }
    if opts.keep_index {
        args.push("--keep-index");
    }
    let msg = opts.message.trim();
    if !msg.is_empty() {
        args.extend(["-m", msg]);
    }
    let out = if paths.is_empty() {
        repo.run(&args)?
    } else {
        args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        // `stash` uses pathspec magic internally, so mark each path literal individually.
        let literal: Vec<String> = paths.iter().map(|p| format!(":(literal){p}")).collect();
        repo.run_stdin(&args, &nul_paths(&literal))?
    };
    Ok(out.trim().to_string())
}

fn apply_like(
    repo: &Repo,
    verb: &str,
    index: usize,
    restore_index: bool,
) -> Result<StashApplyOutcome> {
    let r = format!("stash@{{{index}}}");
    let mut args = vec!["stash", verb];
    if restore_index {
        args.push("--index");
    }
    args.push(&r);
    let before: Vec<String> = repo
        .status()?
        .conflicted()
        .map(|e| e.path.clone())
        .collect();
    match repo.run(&args) {
        Ok(_) => Ok(StashApplyOutcome::Clean),
        Err(e) => {
            let conflicts: Vec<String> = repo
                .status()?
                .conflicted()
                .map(|e| e.path.clone())
                .filter(|p| !before.contains(p))
                .collect();
            if conflicts.is_empty() {
                Err(e)
            } else {
                Ok(StashApplyOutcome::Conflicts(conflicts))
            }
        }
    }
}

/// `git stash apply [--index] stash@{N}`.
pub fn apply(repo: &Repo, index: usize, restore_index: bool) -> Result<StashApplyOutcome> {
    apply_like(repo, "apply", index, restore_index)
}

/// `git stash pop [--index] stash@{N}` (on conflicts git keeps the stash).
pub fn pop(repo: &Repo, index: usize, restore_index: bool) -> Result<StashApplyOutcome> {
    apply_like(repo, "pop", index, restore_index)
}

/// `git stash drop stash@{N}`.
pub fn drop(repo: &Repo, index: usize) -> Result<()> {
    repo.run(&["stash", "drop", "-q", &format!("stash@{{{index}}}")])
        .map(|_| ())
}

/// `git stash clear` — removes every stash.
pub fn clear(repo: &Repo) -> Result<()> {
    repo.run(&["stash", "clear"]).map(|_| ())
}

/// `git stash branch <name> stash@{N}`: new branch at the stash's base, stash applied and dropped.
pub fn branch(repo: &Repo, name: &str, index: usize) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Branch name is empty");
    }
    repo.run(&["check-ref-format", "--branch", name])
        .map_err(|_| anyhow::anyhow!("'{name}' is not a valid branch name"))?;
    repo.run(&["stash", "branch", name, &format!("stash@{{{index}}}")])
        .map(|_| ())
}

/// Fail unless `stash@{index}` still points at `hash`, so an operation confirmed for one stash
/// never hits another one after the stash list changed (stash created or dropped elsewhere).
pub fn ensure_unchanged(repo: &Repo, index: usize, hash: &str) -> Result<()> {
    let r = format!("stash@{{{index}}}");
    let current = repo.try_run(&["rev-parse", "--verify", "-q", &r]);
    match current.as_deref().map(str::trim) {
        Some(h) if h == hash => Ok(()),
        _ => bail!("The stash list changed since {r} was selected; refresh (F5) and try again"),
    }
}

/// Every change of a stash as diffs: tracked changes (stash vs its base) followed by the
/// untracked files stored in the third parent.
pub fn diffs(repo: &Repo, entry: &StashEntry, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
    let base = entry
        .parents
        .first()
        .map(String::as_str)
        .unwrap_or(EMPTY_TREE);
    let mut v = repo.diff_range(base, &entry.hash, &[], opts)?;
    if let Some(u) = entry.untracked_commit() {
        let mut untracked = repo.diff_range(EMPTY_TREE, u, &[], opts)?;
        for d in &mut untracked {
            d.header.insert(0, "untracked file".to_string());
        }
        v.extend(untracked);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn read(repo: &Repo, p: &str) -> String {
        std::fs::read_to_string(repo.path(p))
            .unwrap()
            .replace("\r\n", "\n")
    }

    #[test]
    fn ensure_unchanged_detects_shifted_stashes() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "a.txt", "2\n");
        push(&repo, &StashOptions::default(), &[]).unwrap();
        let first = list(&repo).unwrap().remove(0);
        ensure_unchanged(&repo, 0, &first.hash).unwrap();
        // Another stash is created elsewhere: stash@{0} is now a different stash.
        write(&repo, "a.txt", "3\n");
        push(&repo, &StashOptions::default(), &[]).unwrap();
        assert!(ensure_unchanged(&repo, 0, &first.hash).is_err());
        ensure_unchanged(&repo, 1, &first.hash).unwrap();
        assert!(ensure_unchanged(&repo, 7, &first.hash).is_err());
    }

    #[test]
    fn parses_list_and_subjects() {
        let raw = "stash@{0}\u{1f}aaa\u{1f}100\u{1f}p1 p2 p3\u{1f}On main: my work\u{1e}\n\
stash@{1}\u{1f}bbb\u{1f}50\u{1f}p1 p2\u{1f}WIP on feature/x: 1234567 subject: with colon\u{1e}\n\
stash@{2}\u{1f}ccc\u{1f}10\u{1f}p1 p2\u{1f}WIP on (no branch): 1234567 s\u{1e}\n";
        let l = parse_list(raw);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].index, 0);
        assert_eq!(l[0].branch.as_deref(), Some("main"));
        assert_eq!(l[0].message, "my work");
        assert!(!l[0].wip);
        assert_eq!(l[0].untracked_commit(), Some("p3"));
        assert_eq!(l[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(l[1].message, "1234567 subject: with colon");
        assert!(l[1].wip);
        assert_eq!(l[1].untracked_commit(), None);
        assert_eq!(l[2].branch, None);
        assert_eq!(l[1].refname(), "stash@{1}");
        assert_eq!(parse_subject("weird"), (None, "weird".to_string(), false));
    }

    #[test]
    fn push_list_apply_pop_drop_with_untracked() {
        let (_d, repo) = temp_repo();
        assert!(list(&repo).unwrap().is_empty());
        commit_files(&repo, &[("a.txt", "1\n"), ("b.txt", "b\n")], "init");
        write(&repo, "a.txt", "2\n");
        write(&repo, "new dir/u.txt", "untracked\n");
        let opts = StashOptions {
            message: "first".into(),
            include_untracked: true,
            keep_index: false,
        };
        push(&repo, &opts, &[]).unwrap();
        assert!(!repo.path("new dir/u.txt").exists());
        assert_eq!(read(&repo, "a.txt"), "1\n");
        write(&repo, "b.txt", "B\n");
        push(&repo, &StashOptions::default(), &[]).unwrap();
        let l = list(&repo).unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].wip);
        assert_eq!(l[1].message, "first");
        assert_eq!(l[1].branch.as_deref(), Some("main"));
        assert!(l[1].untracked_commit().is_some());
        // Files of the stash include the untracked part.
        let d = diffs(&repo, &l[1], &DiffOptions::default()).unwrap();
        let paths: Vec<&str> = d.iter().map(|f| f.path()).collect();
        assert_eq!(paths, vec!["a.txt", "new dir/u.txt"]);
        // Apply keeps the entry, pop removes it.
        assert_eq!(apply(&repo, 1, false).unwrap(), StashApplyOutcome::Clean);
        assert_eq!(read(&repo, "new dir/u.txt"), "untracked\n");
        assert_eq!(list(&repo).unwrap().len(), 2);
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        repo.run(&["clean", "-fdq"]).unwrap();
        assert_eq!(pop(&repo, 0, false).unwrap(), StashApplyOutcome::Clean);
        assert_eq!(read(&repo, "b.txt"), "B\n");
        assert_eq!(list(&repo).unwrap().len(), 1);
        drop(&repo, 0).unwrap();
        assert!(list(&repo).unwrap().is_empty());
        // Nothing to stash.
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        assert!(push(&repo, &StashOptions::default(), &[])
            .unwrap()
            .contains("No local changes"));
    }

    #[test]
    fn keep_index_paths_restore_index_and_clear() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n"), ("b.txt", "1\n")], "init");
        write(&repo, "a.txt", "staged\n");
        repo.run(&["add", "a.txt"]).unwrap();
        write(&repo, "b.txt", "unstaged\n");
        let opts = StashOptions {
            keep_index: true,
            ..Default::default()
        };
        push(&repo, &opts, &[]).unwrap();
        assert_eq!(read(&repo, "a.txt"), "staged\n");
        assert_eq!(read(&repo, "b.txt"), "1\n");
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        assert_eq!(pop(&repo, 0, true).unwrap(), StashApplyOutcome::Clean);
        let staged = repo.run(&["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged.trim(), "a.txt");
        // Path-limited stash.
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        write(&repo, "a.txt", "x\n");
        write(&repo, "b.txt", "y\n");
        push(&repo, &StashOptions::default(), &["b.txt".to_string()]).unwrap();
        assert_eq!(read(&repo, "a.txt"), "x\n");
        assert_eq!(read(&repo, "b.txt"), "1\n");
        push(&repo, &StashOptions::default(), &[]).unwrap();
        assert_eq!(list(&repo).unwrap().len(), 2);
        clear(&repo).unwrap();
        assert!(list(&repo).unwrap().is_empty());
    }

    #[test]
    fn conflicts_and_branch() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "a.txt", "stashed\n");
        push(&repo, &StashOptions::default(), &[]).unwrap();
        commit_files(&repo, &[("a.txt", "committed\n")], "conflicting");
        match pop(&repo, 0, false).unwrap() {
            StashApplyOutcome::Conflicts(c) => assert_eq!(c, vec!["a.txt"]),
            o => panic!("expected conflicts, got {o:?}"),
        }
        // The stash is kept after a conflicting pop.
        assert_eq!(list(&repo).unwrap().len(), 1);
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        assert!(branch(&repo, "bad..name", 0).is_err());
        branch(&repo, "from-stash", 0).unwrap();
        assert_eq!(repo.head_branch().as_deref(), Some("from-stash"));
        assert_eq!(read(&repo, "a.txt"), "stashed\n");
        assert!(list(&repo).unwrap().is_empty());
    }

    #[test]
    fn empty_repo_cannot_stash() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        assert!(push(&repo, &StashOptions::default(), &[]).is_err());
    }
}
