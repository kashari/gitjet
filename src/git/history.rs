//! History operations: checkout revision, cherry-pick, revert, reset (soft/mixed/hard/keep), undo last
//! commit, branch/tag at a revision and rewording HEAD.
//!
//! Every operation returns a human-readable success message. Operations that can stop half-way with
//! conflicts (cherry-pick, revert, checkout) check [`Repo::state`] on failure and return an error that
//! points the user to the Conflicts tab.

use anyhow::{anyhow, bail, Result};

use super::{Repo, RepoState};

/// `git reset` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Move the branch only; changes stay staged
    Soft,
    /// Move the branch and reset the index; changes stay in the work tree
    Mixed,
    /// Discard all changes
    Hard,
    /// Like hard, but keep (and refuse to overwrite) local changes
    Keep,
}

impl ResetMode {
    pub const ALL: [ResetMode; 4] = [
        ResetMode::Soft,
        ResetMode::Mixed,
        ResetMode::Hard,
        ResetMode::Keep,
    ];

    /// The `git reset` flag.
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
            ResetMode::Keep => "--keep",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResetMode::Soft => "Soft",
            ResetMode::Mixed => "Mixed",
            ResetMode::Hard => "Hard",
            ResetMode::Keep => "Keep",
        }
    }

    /// One-line explanation shown next to the mode.
    pub fn description(self) -> &'static str {
        match self {
            ResetMode::Soft => "keep changes staged",
            ResetMode::Mixed => "keep changes in the work tree, unstaged",
            ResetMode::Hard => "discard all changes",
            ResetMode::Keep => "discard commits, keep local changes",
        }
    }
}

/// Short hash for messages (falls back to the input).
fn short(repo: &Repo, rev: &str) -> String {
    repo.try_run(&["rev-parse", "--short", rev])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| rev.to_string())
}

/// Turn a failed operation into a helpful error: when git stopped in the middle of `op`, tell the user
/// to resolve conflicts in the Conflicts tab instead of repeating git's raw message. When git stopped
/// without conflicts (a failing hook or commit signing), git's own message is kept, because it names
/// the real problem.
fn stopped_or(repo: &Repo, op: &str, err: anyhow::Error) -> anyhow::Error {
    match repo.state() {
        RepoState::Clean => err,
        state if has_conflicts(repo) => anyhow!(
            "{op} stopped with conflicts ({}). Resolve them in the Conflicts tab (6), then continue or abort there.",
            state.label().to_lowercase()
        ),
        state => anyhow!(
            "{op} stopped ({}) without conflicts: {err:#}\n\nThe changes are staged. Fix the problem, then continue or abort in the Conflicts tab (6).",
            state.label().to_lowercase()
        ),
    }
}

/// True when the repository has unmerged (conflicted) paths.
fn has_conflicts(repo: &Repo) -> bool {
    repo.try_run(&["diff", "--name-only", "--diff-filter=U"])
        .is_some_and(|s| !s.trim().is_empty())
}

/// True when the index matches HEAD exactly (nothing staged). Errors (unborn HEAD) count as "not
/// empty" so callers never throw changes away on doubt.
fn index_matches_head(repo: &Repo) -> bool {
    // Exit code 1 (differences) and errors both make `run` fail.
    repo.run(&["diff", "--cached", "--quiet", "HEAD", "--"])
        .is_ok()
}

/// True when `rev` has more than one parent.
pub fn is_merge(repo: &Repo, rev: &str) -> Result<bool> {
    let out = repo.run(&["rev-list", "--parents", "-n", "1", rev, "--"])?;
    Ok(out.split_whitespace().count() > 2)
}

/// True when `rev` is HEAD or one of its ancestors.
pub fn is_ancestor_of_head(repo: &Repo, rev: &str) -> bool {
    repo.run(&["merge-base", "--is-ancestor", rev, "HEAD"])
        .is_ok()
}

/// Remote branches that already contain `rev` (non-empty = the commit has been pushed).
pub fn pushed_to(repo: &Repo, rev: &str) -> Result<Vec<String>> {
    let out = repo.run(&[
        "branch",
        "-r",
        "--contains",
        rev,
        "--format=%(refname:short)",
    ])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
        .map(str::to_string)
        .collect())
}

/// Cheap "already pushed" check: the current branch's upstream and push target (`@{upstream}`,
/// `@{push}`) that contain `rev`. Unlike [`pushed_to`] it never walks every remote branch.
pub fn pushed_to_upstream(repo: &Repo, rev: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for spec in ["@{upstream}", "@{push}"] {
        if let Some(name) = repo
            .try_run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", spec])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !seen.contains(s))
        {
            seen.push(name);
        }
    }
    seen.into_iter()
        .filter(|name| {
            repo.run(&["merge-base", "--is-ancestor", rev, name])
                .is_ok()
        })
        .collect()
}

/// Check out `rev` as a detached HEAD.
pub fn checkout_detached(repo: &Repo, rev: &str) -> Result<String> {
    repo.run(&["checkout", "--detach", rev, "--"])
        .map_err(|e| stopped_or(repo, "Checkout", e))?;
    Ok(format!("HEAD is now detached at {}", short(repo, rev)))
}

/// Cherry-pick `revs` in the given order (callers pass oldest first). Merge commits are picked against
/// their first parent. Commits that turn out to be empty (already applied) are skipped.
pub fn cherry_pick(repo: &Repo, revs: &[String]) -> Result<String> {
    if revs.is_empty() {
        bail!("nothing to cherry-pick");
    }
    let mut skipped = 0;
    for rev in revs {
        let mut args = vec!["cherry-pick"];
        if is_merge(repo, rev)? {
            args.extend(["-m", "1"]);
        }
        args.push(rev);
        if let Err(e) = repo.run(&args) {
            // Skip only a pick that really is empty (the change is already on this branch). A pick that
            // stopped for another reason (signing, hooks, rerere) has staged changes and must be kept.
            if repo.state() == RepoState::CherryPicking
                && !has_conflicts(repo)
                && index_matches_head(repo)
            {
                repo.run(&["cherry-pick", "--skip"])
                    .or_else(|_| repo.run(&["cherry-pick", "--abort"]))
                    .map_err(|e| stopped_or(repo, "Cherry-pick", e))?;
                skipped += 1;
                continue;
            }
            return Err(stopped_or(repo, "Cherry-pick", e));
        }
    }
    let picked = revs.len() - skipped;
    let mut msg = match revs {
        [one] if picked == 1 => format!("Cherry-picked {}", short(repo, one)),
        _ => format!(
            "Cherry-picked {picked} commit{}",
            if picked == 1 { "" } else { "s" }
        ),
    };
    if skipped > 0 {
        msg.push_str(&format!(" ({skipped} already applied, skipped)"));
    }
    Ok(msg)
}

/// Create a commit reverting `rev`. Merge commits are reverted against their first parent (`-m 1`).
pub fn revert(repo: &Repo, rev: &str, is_merge: bool) -> Result<String> {
    let mut args = vec!["revert", "--no-edit"];
    if is_merge {
        args.extend(["-m", "1"]);
    }
    args.push(rev);
    repo.run(&args).map_err(|e| stopped_or(repo, "Revert", e))?;
    Ok(format!("Reverted {}", short(repo, rev)))
}

/// Reset the current branch (or detached HEAD) to `rev`.
pub fn reset(repo: &Repo, mode: ResetMode, rev: &str) -> Result<String> {
    let target = short(repo, rev);
    repo.run(&["reset", "-q", mode.flag(), rev, "--"])?;
    let what = repo.head_branch().unwrap_or_else(|| "HEAD".into());
    Ok(format!("{} reset of {what} to {target}", mode.label()))
}

/// Undo the last commit, keeping its changes staged. Works for the root
/// commit too, which leaves the branch unborn.
pub fn undo_last_commit(repo: &Repo) -> Result<String> {
    let head = repo
        .head_commit()
        .ok_or_else(|| anyhow!("there is no commit to undo"))?;
    let label = short(repo, &head);
    if repo.rev_parse("HEAD~1").is_ok() {
        repo.run(&["reset", "-q", "--soft", "HEAD~1", "--"])?;
    } else {
        repo.run(&["update-ref", "-d", "HEAD"])?;
    }
    Ok(format!("Undid commit {label}; its changes are staged"))
}

/// Create branch `name` at `rev`, optionally checking it out.
pub fn create_branch_at(repo: &Repo, name: &str, rev: &str, checkout: bool) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("branch name is empty");
    }
    repo.run(&["check-ref-format", "--branch", name])
        .map_err(|_| anyhow!("'{name}' is not a valid branch name"))?;
    if checkout {
        repo.run(&["checkout", "-b", name, rev, "--"])
            .map_err(|e| stopped_or(repo, "Checkout", e))?;
        Ok(format!(
            "Created and checked out {name} at {}",
            short(repo, rev)
        ))
    } else {
        repo.run(&["branch", name, rev])?;
        Ok(format!("Created branch {name} at {}", short(repo, rev)))
    }
}

/// Create tag `name` at `rev`; annotated when `message` is non-empty.
pub fn create_tag_at(repo: &Repo, name: &str, rev: &str, message: Option<&str>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("tag name is empty");
    }
    repo.run(&["check-ref-format", &format!("refs/tags/{name}")])
        .map_err(|_| anyhow!("'{name}' is not a valid tag name"))?;
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(msg) => {
            repo.run_stdin(&["tag", "-a", name, "-F", "-", rev], msg.as_bytes())?;
            Ok(format!(
                "Created annotated tag {name} at {}",
                short(repo, rev)
            ))
        }
        None => {
            repo.run(&["tag", name, rev])?;
            Ok(format!("Created tag {name} at {}", short(repo, rev)))
        }
    }
}

/// Replace HEAD's message, leaving its content (and any staged changes) untouched.
pub fn amend_head_message(repo: &Repo, message: &str) -> Result<String> {
    let message = message.replace("\r\n", "\n");
    if message.trim().is_empty() {
        bail!("commit message is empty");
    }
    if repo.head_commit().is_none() {
        bail!("there is no commit to edit");
    }
    repo.run_stdin(
        &[
            "commit",
            "--amend",
            "--only",
            "--allow-empty",
            "--cleanup=strip",
            "-q",
            "-F",
            "-",
        ],
        message.as_bytes(),
    )?;
    Ok(format!("Updated message of {}", short(repo, "HEAD")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn read(repo: &Repo, rel: &str) -> String {
        std::fs::read_to_string(repo.path(rel))
            .unwrap()
            .replace("\r\n", "\n")
    }

    fn subjects(repo: &Repo) -> Vec<String> {
        repo.run(&["log", "--format=%s"])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn checkout_detached_moves_head() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        commit_files(&repo, &[("a.txt", "2\n")], "two");
        let msg = checkout_detached(&repo, &c1).unwrap();
        assert!(msg.contains("detached"));
        assert_eq!(repo.head_branch(), None);
        assert_eq!(repo.head_commit().unwrap(), c1);
        assert_eq!(read(&repo, "a.txt"), "1\n");
        assert!(checkout_detached(&repo, "does-not-exist").is_err());
    }

    #[test]
    fn cherry_pick_in_order_and_skip_empty() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("base.txt", "b\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        let f1 = commit_files(&repo, &[("f1.txt", "1\n")], "f1");
        let f2 = commit_files(&repo, &[("f2.txt", "2\n")], "f2");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        let msg = cherry_pick(&repo, &[f1.clone(), f2.clone()]).unwrap();
        assert_eq!(msg, "Cherry-picked 2 commits");
        assert_eq!(subjects(&repo), vec!["f2", "f1", "base"]);
        // Picking again: the changes are already there, so both are skipped instead of failing.
        let msg = cherry_pick(&repo, &[f1]).unwrap();
        assert!(msg.contains("skipped"), "{msg}");
        assert_eq!(repo.state(), RepoState::Clean);
        assert!(cherry_pick(&repo, &[]).is_err());
    }

    #[test]
    fn cherry_pick_failing_without_conflicts_keeps_changes() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("base.txt", "b\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        let f = commit_files(&repo, &[("b.txt", "feature\n")], "feature");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        // Commit signing that fails: git stops with CHERRY_PICK_HEAD, no conflicts and staged changes.
        repo.run(&["config", "commit.gpgsign", "true"]).unwrap();
        repo.run(&["config", "gpg.program", "false"]).unwrap();
        let err = cherry_pick(&repo, &[f]).unwrap_err().to_string();
        assert!(!err.contains("skipped"), "{err}");
        assert!(
            err.contains("without conflicts") && err.contains("Conflicts tab (6)"),
            "{err}"
        );
        assert_eq!(repo.state(), RepoState::CherryPicking);
        assert!(
            repo.status()
                .unwrap()
                .entries
                .iter()
                .any(|e| e.path == "b.txt" && e.is_staged()),
            "the picked change must stay staged"
        );
        assert_eq!(subjects(&repo), vec!["base"]);
    }

    #[test]
    fn cherry_pick_merge_commit_uses_first_parent() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("base.txt", "b\n")], "base");
        repo.run(&["checkout", "-q", "-b", "side"]).unwrap();
        commit_files(&repo, &[("side.txt", "s\n")], "side");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("main.txt", "m\n")], "main work");
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge side", "side"])
            .unwrap();
        let merge = repo.head_commit().unwrap();
        assert!(is_merge(&repo, &merge).unwrap());
        repo.run(&["checkout", "-q", "-b", "other", "HEAD~1"])
            .unwrap();
        cherry_pick(&repo, &[merge]).unwrap();
        assert_eq!(read(&repo, "side.txt"), "s\n");
    }

    #[test]
    fn cherry_pick_conflict_points_to_conflicts_tab() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "base\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        let f = commit_files(&repo, &[("a.txt", "feature\n")], "feature");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("a.txt", "main\n")], "main");
        let err = cherry_pick(&repo, &[f]).unwrap_err().to_string();
        assert!(err.contains("Conflicts tab (6)"), "{err}");
        assert_eq!(repo.state(), RepoState::CherryPicking);
    }

    #[test]
    fn revert_normal_and_merge() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        let c2 = commit_files(&repo, &[("b.txt", "2\n")], "two");
        let msg = revert(&repo, &c2, false).unwrap();
        assert!(msg.starts_with("Reverted"));
        assert!(!repo.path("b.txt").exists());
        assert!(subjects(&repo)[0].starts_with("Revert \"two\""));

        repo.run(&["checkout", "-q", "-b", "side"]).unwrap();
        commit_files(&repo, &[("s.txt", "s\n")], "side");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("m.txt", "m\n")], "main");
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge", "side"])
            .unwrap();
        let merge = repo.head_commit().unwrap();
        assert!(
            revert(&repo, &merge, false).is_err(),
            "merge without -m must fail"
        );
        repo.run(&["revert", "--abort"]).ok();
        revert(&repo, &merge, true).unwrap();
        assert!(!repo.path("s.txt").exists());
        assert!(repo.path("m.txt").exists());
    }

    #[test]
    fn revert_conflict_points_to_conflicts_tab() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        let c2 = commit_files(&repo, &[("a.txt", "2\n")], "two");
        commit_files(&repo, &[("a.txt", "3\n")], "three");
        let err = revert(&repo, &c2, false).unwrap_err().to_string();
        assert!(err.contains("Conflicts tab"), "{err}");
        assert_eq!(repo.state(), RepoState::Reverting);
    }

    #[test]
    fn reset_modes() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        let c2 = commit_files(&repo, &[("a.txt", "2\n")], "two");

        reset(&repo, ResetMode::Soft, &c1).unwrap();
        assert_eq!(repo.head_commit().unwrap(), c1);
        assert!(repo.status().unwrap().entries.iter().any(|e| e.is_staged()));

        reset(&repo, ResetMode::Mixed, &c2).unwrap();
        assert_eq!(repo.head_commit().unwrap(), c2);
        assert!(repo.status().unwrap().is_clean());

        reset(&repo, ResetMode::Mixed, &c1).unwrap();
        let st = repo.status().unwrap();
        assert!(st.entries.iter().all(|e| !e.is_staged()) && !st.is_clean());

        let msg = reset(&repo, ResetMode::Hard, &c2).unwrap();
        assert_eq!(msg, format!("Hard reset of main to {}", short(&repo, &c2)));
        assert_eq!(read(&repo, "a.txt"), "2\n");

        write(&repo, "other.txt", "local\n");
        repo.run(&["add", "other.txt"]).unwrap();
        reset(&repo, ResetMode::Keep, &c1).unwrap();
        assert_eq!(read(&repo, "a.txt"), "1\n");
        assert_eq!(read(&repo, "other.txt"), "local\n");
        assert_eq!(ResetMode::ALL.len(), 4);
        assert_eq!(ResetMode::Keep.flag(), "--keep");
    }

    #[test]
    fn undo_last_commit_keeps_changes_staged() {
        let (_d, repo) = temp_repo();
        assert!(undo_last_commit(&repo).is_err());
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        commit_files(&repo, &[("b.txt", "2\n")], "two");
        undo_last_commit(&repo).unwrap();
        assert_eq!(repo.head_commit().unwrap(), c1);
        let st = repo.status().unwrap();
        assert!(st
            .entries
            .iter()
            .any(|e| e.path == "b.txt" && e.is_staged()));
        // Root commit: the branch becomes unborn, the file stays staged.
        undo_last_commit(&repo).unwrap();
        assert!(repo.head_commit().is_none());
        assert!(repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.path == "a.txt" && e.is_staged()));
    }

    #[test]
    fn pushed_detection() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        assert!(pushed_to(&repo, &c1).unwrap().is_empty());
        repo.run(&["update-ref", "refs/remotes/origin/main", &c1])
            .unwrap();
        let c2 = commit_files(&repo, &[("a.txt", "2\n")], "two");
        assert_eq!(
            pushed_to(&repo, &c1).unwrap(),
            vec!["origin/main".to_string()]
        );
        assert!(pushed_to(&repo, &c2).unwrap().is_empty());
        assert!(is_ancestor_of_head(&repo, &c1));
        // Upstream-only check.
        assert!(
            pushed_to_upstream(&repo, &c1).is_empty(),
            "no upstream configured"
        );
        repo.run(&["config", "branch.main.remote", "origin"])
            .unwrap();
        repo.run(&["config", "branch.main.merge", "refs/heads/main"])
            .unwrap();
        repo.run(&[
            "config",
            "remote.origin.url",
            "https://example.invalid/r.git",
        ])
        .unwrap();
        repo.run(&[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ])
        .unwrap();
        assert_eq!(
            pushed_to_upstream(&repo, &c1),
            vec!["origin/main".to_string()]
        );
        assert!(pushed_to_upstream(&repo, &c2).is_empty());
        repo.run(&["checkout", "-q", "--orphan", "lonely"]).unwrap();
        commit_files(&repo, &[("z.txt", "z\n")], "orphan");
        assert!(!is_ancestor_of_head(&repo, &c1));
    }

    #[test]
    fn branches_and_tags_at_revision() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        commit_files(&repo, &[("a.txt", "2\n")], "two");
        create_branch_at(&repo, "old", &c1, false).unwrap();
        assert_eq!(repo.rev_parse("old").unwrap(), c1);
        assert_eq!(repo.head_branch().as_deref(), Some("main"));
        create_branch_at(&repo, "feature/x y", &c1, false).unwrap_err();
        create_branch_at(&repo, "feature/x", &c1, true).unwrap();
        assert_eq!(repo.head_branch().as_deref(), Some("feature/x"));
        assert!(create_branch_at(&repo, " ", &c1, false).is_err());

        create_tag_at(&repo, "v1", &c1, None).unwrap();
        assert_eq!(
            repo.run(&["cat-file", "-t", "v1"]).unwrap().trim(),
            "commit"
        );
        create_tag_at(&repo, "v2", &c1, Some("Release 2\n\nnotes")).unwrap();
        assert_eq!(repo.run(&["cat-file", "-t", "v2"]).unwrap().trim(), "tag");
        assert_eq!(repo.rev_parse("v2").unwrap(), c1);
        assert!(create_tag_at(&repo, "bad..name", &c1, None).is_err());
        assert!(
            create_tag_at(&repo, "v1", &c1, None).is_err(),
            "duplicate tag"
        );
    }

    #[test]
    fn amend_message_only() {
        let (_d, repo) = temp_repo();
        assert!(amend_head_message(&repo, "x").is_err());
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        write(&repo, "a.txt", "staged\n");
        repo.run(&["add", "a.txt"]).unwrap();
        amend_head_message(&repo, "better subject\r\n\r\nbody line").unwrap();
        assert_eq!(
            repo.commit_message("HEAD").unwrap(),
            "better subject\n\nbody line"
        );
        // The staged change was not folded into the commit.
        assert!(repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.path == "a.txt" && e.is_staged()));
        assert_eq!(
            repo.run(&["show", "HEAD:a.txt"])
                .unwrap()
                .replace("\r\n", "\n"),
            "1\n"
        );
        assert!(amend_head_message(&repo, "  ").is_err());
    }
}
