//! Everyday housekeeping: bring all local branches up to date, find branches that can go, step
//! back to before the last operation, and fold staged changes into an earlier commit.

use anyhow::{anyhow, bail, Result};

use super::branch::{self, Branch};
use super::{admin, remote, Repo};

// ----- sync ----------------------------------------------------------------------------------

/// What [`sync_branches`] did with each local branch that has an upstream.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Fast-forwarded: (branch, commits it was behind)
    pub updated: Vec<(String, u32)>,
    /// Has commits of its own and is behind: needs a merge or rebase by hand
    pub diverged: Vec<String>,
    /// Behind, but could not be moved: (branch, why)
    pub skipped: Vec<(String, String)>,
    pub up_to_date: usize,
}

impl SyncReport {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.updated.is_empty() {
            let list: Vec<String> = self
                .updated
                .iter()
                .map(|(b, n)| format!("{b} +{n}"))
                .collect();
            parts.push(format!("updated {}", list.join(", ")));
        }
        if !self.diverged.is_empty() {
            parts.push(format!(
                "diverged (merge or rebase by hand): {}",
                self.diverged.join(", ")
            ));
        }
        for (b, why) in &self.skipped {
            parts.push(format!("{b} not updated: {why}"));
        }
        if self.up_to_date > 0 || parts.is_empty() {
            parts.push(format!("{} up to date", self.up_to_date));
        }
        format!("Branches: {}", parts.join("; "))
    }
}

/// Fast-forward every local branch that is only behind its upstream, without checking it out.
/// The current branch is fast-forwarded in the work tree (and skipped if local changes are in the
/// way); branches open in another work tree are skipped. Branches with their own commits are
/// never touched.
pub fn sync_branches(repo: &Repo, fetch: bool, prune: bool) -> Result<SyncReport> {
    if fetch && !remote::remotes(repo)?.is_empty() {
        remote::fetch_all(repo, prune)?;
    }
    let elsewhere: Vec<String> = admin::list_worktrees(repo)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.path != repo.root)
        .filter_map(|w| w.branch)
        .collect();
    let mut report = SyncReport::default();
    for b in branch::list_branches(repo)? {
        let Some(upstream) = b.upstream.as_ref().filter(|_| !b.is_remote()) else {
            continue;
        };
        if b.upstream_gone {
            continue;
        }
        match (b.ahead, b.behind) {
            (_, 0) => report.up_to_date += 1,
            (0, behind) => {
                let moved = if b.is_head {
                    repo.run(&["merge", "--ff-only", "--quiet", upstream])
                        .map_err(|_| "local changes are in the way".to_string())
                } else if elsewhere.contains(&b.name) {
                    Err("it is open in another work tree".to_string())
                } else {
                    // `fetch .` moves a branch only as a fast-forward.
                    let spec = format!("refs/remotes/{upstream}:{}", b.full_ref);
                    repo.run(&["fetch", "--quiet", ".", &spec])
                        .map_err(|e| format!("{e:#}"))
                };
                match moved {
                    Ok(_) => report.updated.push((b.name, behind)),
                    Err(why) => report.skipped.push((b.name, why)),
                }
            }
            _ => report.diverged.push(b.name),
        }
    }
    Ok(report)
}

// ----- clean up ------------------------------------------------------------------------------

/// A local branch that is probably no longer needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub name: String,
    /// All of its commits are in the main branch: deleting it loses nothing
    pub merged: bool,
    /// Its upstream branch was deleted on the remote
    pub upstream_gone: bool,
    /// Commits that exist only on this branch (0 when merged)
    pub unmerged: u32,
}

impl Stale {
    pub fn describe(&self) -> String {
        match (self.merged, self.upstream_gone) {
            (true, true) => format!("{} (merged, remote branch deleted)", self.name),
            (true, false) => format!("{} (merged)", self.name),
            _ => format!(
                "{} (remote branch deleted, {} not merged)",
                self.name,
                branch::commits(self.unmerged)
            ),
        }
    }
}

/// The branch others are merged into: where `origin/HEAD` points, else `main` or `master`.
pub fn main_branch(repo: &Repo) -> Option<String> {
    for remote in remote::remotes(repo).unwrap_or_default() {
        let head = format!("refs/remotes/{remote}/HEAD");
        if let Some(r) = repo.try_run(&["symbolic-ref", "-q", "--short", &head]) {
            let r = r.trim();
            if !r.is_empty() {
                return Some(r.to_string());
            }
        }
    }
    ["main", "master"]
        .into_iter()
        .find(|b| branch::local_exists(repo, b))
        .map(str::to_string)
}

/// Local branches that are merged into the main branch or whose upstream is gone. Never the
/// current branch, the main branch itself or a branch open in another work tree.
pub fn stale_branches(repo: &Repo) -> Result<(String, Vec<Stale>)> {
    let base = main_branch(repo)
        .ok_or_else(|| anyhow!("cannot tell which branch is the main one (no main or master)"))?;
    let base_local = base.rsplit_once('/').map_or(base.as_str(), |(_, b)| b);
    // Merged into the main branch on the remote, or into the local one (a merge not pushed yet).
    let mut merged: Vec<String> = Vec::new();
    for into in [base.as_str(), base_local] {
        if let Some(out) = repo.try_run(&["branch", "--format=%(refname:short)", "--merged", into])
        {
            merged.extend(out.lines().map(|l| l.trim().to_string()));
        }
    }
    let elsewhere: Vec<String> = admin::list_worktrees(repo)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.branch)
        .collect();
    let mut stale = Vec::new();
    for b in branch::list_branches(repo)? {
        let b: Branch = b;
        if b.is_remote()
            || b.is_head
            || b.name == base
            || b.name == base_local
            || matches!(b.name.as_str(), "main" | "master")
            || elsewhere.contains(&b.name)
        {
            continue;
        }
        let is_merged = merged.contains(&b.name);
        if !is_merged && !b.upstream_gone {
            continue;
        }
        let unmerged = if is_merged {
            0
        } else {
            [base.as_str(), base_local]
                .into_iter()
                .filter_map(|into| branch::ahead_behind(repo, &b.full_ref, into).ok())
                .map(|(ahead, _)| ahead)
                .min()
                .unwrap_or(0)
        };
        stale.push(Stale {
            name: b.name,
            merged: is_merged,
            upstream_gone: b.upstream_gone,
            unmerged,
        });
    }
    Ok((base, stale))
}

/// Delete `branches`; the ones that are not merged need `force`. Returns what was deleted.
pub fn delete_stale(repo: &Repo, branches: &[Stale], force: bool) -> Result<String> {
    let mut deleted = Vec::new();
    for b in branches {
        if b.merged || force {
            // Merged into the main branch, which need not be HEAD: `-D` after our own check.
            branch::delete_branch(repo, &b.name, true)?;
            deleted.push(b.name.clone());
        }
    }
    Ok(match deleted.len() {
        0 => "No branches deleted".to_string(),
        1 => format!("Deleted branch {}", deleted[0]),
        n => format!("Deleted {n} branches: {}", deleted.join(", ")),
    })
}

// ----- undo ----------------------------------------------------------------------------------

/// A state the current branch can go back to: where it was before one of its last operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPoint {
    /// Commit the branch pointed to before the operation
    pub target: String,
    /// The operation in plain words, e.g. `merge develop`, `commit "Fix login"`
    pub what: String,
    /// When the operation happened (unix seconds)
    pub when: i64,
}

/// Reword a reflog subject for people: `commit: Fix x` → `commit "Fix x"`.
fn plain_operation(subject: &str) -> String {
    let (op, rest) = subject.split_once(':').unwrap_or((subject, ""));
    let rest = rest.trim();
    match op.trim() {
        "commit" | "commit (initial)" => format!("commit \"{rest}\""),
        "commit (amend)" => format!("amend of \"{rest}\""),
        "commit (merge)" => format!("merge commit \"{rest}\""),
        "cherry-pick" => format!("cherry-pick of \"{rest}\""),
        "revert" => format!("revert \"{rest}\""),
        "reset" => format!("reset ({rest})"),
        op if op.starts_with("merge ") => op.to_string(),
        op if op.starts_with("pull") => format!("pull ({rest})"),
        op if op.starts_with("rebase") => "rebase".to_string(),
        op if op.starts_with("branch") => format!("branch created ({rest})"),
        _ => subject.to_string(),
    }
}

/// The last `n` operations that moved the current branch (or a detached HEAD), newest first.
pub fn undo_points(repo: &Repo, n: usize) -> Result<Vec<UndoPoint>> {
    let reference = match repo.head_branch() {
        Some(b) => format!("refs/heads/{b}"),
        None => "HEAD".to_string(),
    };
    // One more than asked: each entry's "before" is the next entry's commit.
    let out = repo
        .run(&[
            "reflog",
            "show",
            &format!("-n{}", n + 1),
            "--date=unix",
            "--format=%H%x00%gs%x00%gd",
            &reference,
            "--",
        ])
        .unwrap_or_default();
    // (commit, what happened, when): with `--date=unix`, %gd is `ref@{<seconds>}`.
    let entries: Vec<(String, String, i64)> = out
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\0');
            let (id, subject, selector) = (f.next()?, f.next()?, f.next()?);
            let when = selector
                .rsplit_once("@{")
                .and_then(|(_, t)| t.trim_end_matches('}').parse().ok())
                .unwrap_or(0);
            Some((id.to_string(), subject.to_string(), when))
        })
        .collect();
    let mut points = Vec::new();
    for pair in entries.windows(2) {
        let (current, subject, when) = &pair[0];
        let before = &pair[1].0;
        if current == before {
            continue;
        }
        points.push(UndoPoint {
            target: before.clone(),
            what: plain_operation(subject),
            when: *when,
        });
    }
    points.truncate(n);
    Ok(points)
}

/// Move the current branch back to `point`. Commits made since stay reachable through the reflog
/// (undoing is itself an operation that can be undone). Local changes are kept; when they touch
/// files that differ between the two states, git refuses and nothing changes.
pub fn undo_to(repo: &Repo, point: &UndoPoint) -> Result<String> {
    if repo.state() != super::RepoState::Clean {
        bail!("an operation is in progress; finish or abort it in the Conflicts tab first");
    }
    repo.run(&["reset", "--keep", "-q", &point.target, "--"])
        .map_err(|e| {
            anyhow!(
                "could not go back: your uncommitted changes touch files that would change. \
                 Commit, stash or shelve them first.\n\n{e:#}"
            )
        })?;
    let short = &point.target[..point.target.len().min(8)];
    Ok(format!("Undid {}: back at {short}", point.what))
}

// ----- fixup ---------------------------------------------------------------------------------

/// Commit the staged changes as a fixup of `rev` and fold it in right away (autosquash), so
/// `rev` ends up containing them. Rewrites `rev` and every commit after it.
pub fn fixup_into(repo: &Repo, rev: &str) -> Result<String> {
    let target = repo.rev_parse(rev)?;
    if repo
        .run(&["diff", "--cached", "--quiet", "HEAD", "--"])
        .is_ok()
    {
        bail!("nothing is staged: stage the changes that belong to that commit first");
    }
    if repo
        .run(&["merge-base", "--is-ancestor", &target, "HEAD"])
        .is_err()
    {
        bail!("that commit is not part of the current branch");
    }
    let merges = repo.run(&[
        "rev-list",
        "--merges",
        "--count",
        &format!("{target}..HEAD"),
    ])?;
    if merges.trim() != "0" || super::history::is_merge(repo, &target)? {
        bail!(
            "there are merge commits from that commit on; fixing up across merges is not supported"
        );
    }
    repo.run(&["commit", "-q", &format!("--fixup={target}")])?;
    let root = repo.rev_parse(&format!("{target}~1")).is_err();
    let base = format!("{target}~1");
    let mut args = vec!["rebase", "-i", "--autosquash", "--autostash", "-q"];
    if root {
        args.push("--root");
    } else {
        args.push(&base);
    }
    if let Err(e) = repo.run(&args) {
        // Put everything back: abort the rebase and take the fixup commit off again.
        let _ = repo.run(&["rebase", "--abort"]);
        let _ = repo.run(&["reset", "-q", "--soft", "HEAD~1", "--"]);
        bail!("the staged changes do not fit that commit without conflicts; nothing was changed.\n\n{e:#}");
    }
    let short = &target[..target.len().min(8)];
    Ok(format!("Folded the staged changes into {short}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    /// A repository with a bare "origin" it was cloned from, both on `main`.
    fn with_remote() -> (TempDir, Repo, Repo) {
        let (d, origin) = temp_repo();
        commit_files(&origin, &[("a.txt", "1\n")], "first");
        let clone_dir = d.path().parent().unwrap().join(format!(
            "clone-{}-{}",
            std::process::id(),
            d.path().file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&clone_dir);
        origin
            .run(&[
                "clone",
                "-q",
                origin.root.to_str().unwrap(),
                clone_dir.to_str().unwrap(),
            ])
            .unwrap();
        let repo = Repo::discover(&clone_dir).unwrap();
        for (k, v) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            repo.run(&["config", k, v]).unwrap();
        }
        (d, origin, repo)
    }

    fn cleanup(repo: &Repo) {
        let _ = std::fs::remove_dir_all(&repo.root);
    }

    #[test]
    fn sync_fast_forwards_branches_that_are_only_behind() {
        let (_d, origin, repo) = with_remote();
        origin.run(&["branch", "dev"]).unwrap();
        origin.run(&["branch", "mine"]).unwrap();
        repo.run(&["fetch", "-q"]).unwrap();
        repo.run(&["branch", "-q", "--track", "dev", "origin/dev"])
            .unwrap();
        repo.run(&["checkout", "-q", "--track", "-b", "mine", "origin/mine"])
            .unwrap();
        commit_files(&repo, &[("mine.txt", "m\n")], "my work");
        repo.run(&["checkout", "-q", "main"]).unwrap();

        // The remote moves on: main +1, dev +2, mine +1 (so `mine` diverges).
        commit_files(&origin, &[("a.txt", "2\n")], "main 2");
        origin.run(&["checkout", "-q", "dev"]).unwrap();
        commit_files(&origin, &[("d.txt", "1\n")], "dev 1");
        commit_files(&origin, &[("d.txt", "2\n")], "dev 2");
        origin.run(&["checkout", "-q", "mine"]).unwrap();
        commit_files(&origin, &[("o.txt", "1\n")], "theirs");
        origin.run(&["checkout", "-q", "main"]).unwrap();

        let r = sync_branches(&repo, true, false).unwrap();
        let mut updated = r.updated.clone();
        updated.sort();
        assert_eq!(updated, [("dev".to_string(), 2), ("main".to_string(), 1)]);
        assert_eq!(r.diverged, ["mine"]);
        assert!(r.skipped.is_empty());
        assert_eq!(
            repo.rev_parse("dev").unwrap(),
            repo.rev_parse("origin/dev").unwrap()
        );
        assert_eq!(std::fs::read_to_string(repo.path("a.txt")).unwrap(), "2\n");
        assert_eq!(repo.head_branch().as_deref(), Some("main"));
        assert!(r.describe().contains("diverged"), "{}", r.describe());

        let again = sync_branches(&repo, true, false).unwrap();
        assert!(again.updated.is_empty());
        assert_eq!(again.up_to_date, 2);
        cleanup(&repo);
    }

    #[test]
    fn stale_branches_are_merged_or_have_lost_their_upstream() {
        let (_d, origin, repo) = with_remote();
        // merged: branch whose commit is merged into main
        repo.run(&["checkout", "-q", "-b", "merged"]).unwrap();
        commit_files(&repo, &[("m.txt", "m\n")], "merged work");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge", "merged"])
            .unwrap();
        // gone: pushed, then deleted on the remote, with a commit that is not in main
        repo.run(&["checkout", "-q", "-b", "gone"]).unwrap();
        commit_files(&repo, &[("g.txt", "g\n")], "unmerged work");
        repo.run(&["push", "-q", "-u", "origin", "gone"]).unwrap();
        origin.run(&["branch", "-D", "gone"]).unwrap();
        // active: unmerged, never pushed -> not stale
        repo.run(&["checkout", "-q", "-b", "active", "main"])
            .unwrap();
        commit_files(&repo, &[("x.txt", "x\n")], "active work");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["fetch", "-q", "--prune"]).unwrap();

        let (base, stale) = stale_branches(&repo).unwrap();
        assert!(base == "origin/main" || base == "main", "{base}");
        let names: Vec<&str> = stale.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["gone", "merged"]);
        let gone = &stale[0];
        assert!(!gone.merged && gone.upstream_gone && gone.unmerged == 1);
        assert!(
            gone.describe().contains("1 commit not merged"),
            "{}",
            gone.describe()
        );
        assert!(stale[1].merged);

        assert_eq!(
            delete_stale(&repo, &stale, false).unwrap(),
            "Deleted branch merged"
        );
        assert!(branch::local_exists(&repo, "gone"));
        let (_, stale) = stale_branches(&repo).unwrap();
        assert_eq!(
            delete_stale(&repo, &stale, true).unwrap(),
            "Deleted branch gone"
        );
        assert!(branch::local_exists(&repo, "active"));
        cleanup(&repo);
    }

    #[test]
    fn undo_goes_back_to_before_an_operation_and_can_be_undone() {
        let (_d, repo) = temp_repo();
        let first = commit_files(&repo, &[("a.txt", "1\n")], "first");
        let second = commit_files(&repo, &[("a.txt", "2\n")], "second");
        repo.run(&["checkout", "-q", "-b", "side", &first]).unwrap();
        commit_files(&repo, &[("s.txt", "s\n")], "side work");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge side", "side"])
            .unwrap();
        let merged = repo.head_commit().unwrap();

        let points = undo_points(&repo, 10).unwrap();
        assert_eq!(points[0].target, second);
        assert!(
            points[0].what.starts_with("merge side"),
            "{}",
            points[0].what
        );
        assert_eq!(points[1].target, first);
        assert_eq!(points[1].what, "commit \"second\"");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((now - points[0].when).abs() < 600, "{}", points[0].when);

        // Unrelated local changes survive.
        write(&repo, "notes.txt", "wip\n");
        let msg = undo_to(&repo, &points[0]).unwrap();
        assert!(msg.starts_with("Undid merge side"), "{msg}");
        assert_eq!(repo.head_commit().unwrap(), second);
        assert!(repo.path("notes.txt").exists());

        // The undo is the newest operation now: undoing it brings the merge back.
        let points = undo_points(&repo, 10).unwrap();
        assert_eq!(points[0].target, merged);
        undo_to(&repo, &points[0]).unwrap();
        assert_eq!(repo.head_commit().unwrap(), merged);
    }

    #[test]
    fn undo_refuses_when_local_changes_are_in_the_way() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        commit_files(&repo, &[("a.txt", "2\n")], "second");
        write(&repo, "a.txt", "local edit\n");
        let points = undo_points(&repo, 5).unwrap();
        let head = repo.head_commit();
        let err = format!("{:#}", undo_to(&repo, &points[0]).unwrap_err());
        assert!(err.contains("uncommitted changes"), "{err}");
        assert_eq!(repo.head_commit(), head);
        assert_eq!(
            std::fs::read_to_string(repo.path("a.txt")).unwrap(),
            "local edit\n"
        );
    }

    #[test]
    fn fixup_folds_staged_changes_into_an_earlier_commit() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "a1\n")], "first");
        let target = commit_files(&repo, &[("b.txt", "b1\n")], "add b");
        commit_files(&repo, &[("c.txt", "c1\n")], "add c");
        assert!(fixup_into(&repo, &target).is_err(), "nothing staged");

        write(&repo, "b.txt", "b1 fixed\n");
        repo.run(&["add", "b.txt"]).unwrap();
        let msg = fixup_into(&repo, &target).unwrap();
        assert!(msg.starts_with("Folded the staged changes into"), "{msg}");
        let log = repo.run(&["log", "--format=%s"]).unwrap();
        assert_eq!(log.lines().collect::<Vec<_>>(), ["add c", "add b", "first"]);
        assert_eq!(repo.run(&["show", "HEAD~1:b.txt"]).unwrap(), "b1 fixed\n");
        assert!(repo.status().unwrap().is_clean());
    }

    #[test]
    fn fixup_that_conflicts_changes_nothing() {
        let (_d, repo) = temp_repo();
        let target = commit_files(&repo, &[("a.txt", "one\n")], "first");
        commit_files(&repo, &[("a.txt", "two\n")], "second");
        let head = repo.head_commit();
        // Staged against "two", but folded into the commit that says "one": the next commit
        // cannot be replayed on top.
        write(&repo, "a.txt", "three\n");
        repo.run(&["add", "a.txt"]).unwrap();
        let err = format!("{:#}", fixup_into(&repo, &target).unwrap_err());
        assert!(err.contains("nothing was changed"), "{err}");
        assert_eq!(repo.head_commit(), head);
        assert_eq!(repo.state(), crate::git::RepoState::Clean);
        assert_eq!(
            repo.run(&["diff", "--cached", "--name-only"])
                .unwrap()
                .trim(),
            "a.txt"
        );
    }
}
