//! Bring a list of commits, given by hash, onto a branch (cherry-pick) or undo them (revert).
//!
//! Used by `gitjet pick` / `gitjet revert` and by the "Cherry-pick commits…" dialog. The list is
//! checked as a whole first ([`plan`]): every unknown or ambiguous hash is reported together and
//! nothing runs until all of them resolve. [`PickPlan::run`] then applies it.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};

use super::history;
use super::log::Commit;
use super::Repo;

/// What happens to one commit of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickStatus {
    /// Will be applied
    Apply,
    /// Already part of the target branch's history: left out
    OnTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickItem {
    pub commit: Commit,
    pub status: PickStatus,
}

/// A checked list of commits to cherry-pick, in the order they will be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickPlan {
    /// Branch the commits go onto; `None` is the current branch (or a detached HEAD)
    pub onto: Option<String>,
    /// Name of the target for messages
    pub target: String,
    pub items: Vec<PickItem>,
}

/// Split pasted text into revisions: separated by whitespace, commas or semicolons, with
/// surrounding quotes, brackets and list markers removed (hashes copied from a ticket or a chat).
pub fn parse_revs(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|t| {
            t.trim_matches(|c: char| {
                matches!(
                    c,
                    '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '<' | '>' | '*' | '.'
                )
            })
        })
        .filter(|t| !t.is_empty() && *t != "-")
        .map(str::to_string)
        .collect()
}

/// Resolve every revision to a commit. All problems are collected into one error.
fn resolve(repo: &Repo, revs: &[String]) -> Result<Vec<Commit>> {
    if revs.is_empty() {
        bail!("no commits given");
    }
    let mut commits: Vec<Commit> = Vec::new();
    let mut problems = Vec::new();
    for rev in revs {
        match repo.run(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")]) {
            Ok(id) => {
                let id = id.trim();
                if !commits.iter().any(|c| c.id == id) {
                    commits.push(repo.commit_info(id)?);
                }
            }
            Err(e) => {
                let why = if format!("{e:#}").contains("ambiguous") {
                    "ambiguous, give more characters"
                } else {
                    "not a commit in this repository"
                };
                problems.push(format!("  {rev}: {why}"));
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "{} of {} could not be found (nothing was changed):\n{}",
            problems.len(),
            revs.len(),
            problems.join("\n")
        );
    }
    Ok(commits)
}

/// Check `revs` and decide the order and what to leave out.
///
/// Commits are applied oldest first by commit date, so a list pasted in any order applies the way
/// it was written; `keep_order` applies them exactly as given. Commits that the target already
/// contains are marked [`PickStatus::OnTarget`].
pub fn plan(
    repo: &Repo,
    revs: &[String],
    onto: Option<&str>,
    keep_order: bool,
) -> Result<PickPlan> {
    let mut commits = resolve(repo, revs)?;
    if !keep_order {
        // Stable: commits made in the same second keep the order they were given in.
        commits.sort_by_key(|c| c.commit_time);
    }
    let current = repo.head_branch();
    let onto = onto
        .map(|b| b.trim().to_string())
        .filter(|b| current.as_deref() != Some(b.as_str()));
    let target_rev = match &onto {
        Some(b) => {
            repo.run(&["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")])
                .map_err(|_| anyhow!("{b} is not a local branch"))?;
            format!("refs/heads/{b}")
        }
        None => "HEAD".to_string(),
    };
    let target = onto
        .clone()
        .or(current)
        .unwrap_or_else(|| "the detached HEAD".into());
    let items = commits
        .into_iter()
        .map(|commit| {
            let on_target = repo
                .run(&["merge-base", "--is-ancestor", &commit.id, &target_rev])
                .is_ok();
            PickItem {
                commit,
                status: if on_target {
                    PickStatus::OnTarget
                } else {
                    PickStatus::Apply
                },
            }
        })
        .collect();
    Ok(PickPlan {
        onto,
        target,
        items,
    })
}

impl PickPlan {
    /// Hashes that will be applied, in order.
    pub fn to_apply(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.status == PickStatus::Apply)
            .map(|i| i.commit.id.clone())
            .collect()
    }

    /// The plan as text, one commit per line, for a confirmation dialog or the terminal.
    pub fn describe(&self) -> String {
        let n = self.to_apply().len();
        let mut s = format!(
            "Cherry-pick {n} commit{} onto {}:\n",
            if n == 1 { "" } else { "s" },
            self.target
        );
        for i in &self.items {
            let note = match i.status {
                PickStatus::Apply => "",
                PickStatus::OnTarget => "  (already on the branch, skipped)",
            };
            s.push_str(&format!(
                "  {} {} — {}{note}\n",
                i.commit.short,
                crate::util::truncate(&i.commit.subject, 60),
                i.commit.author
            ));
        }
        s.trim_end().to_string()
    }

    /// Apply the plan. Onto the current branch this is a normal cherry-pick (conflicts stop it and
    /// are resolved in the Conflicts tab). Onto another branch the commits are applied in a
    /// temporary work tree, so the files you are working on are not touched; a conflict there
    /// undoes the whole pick and leaves that branch as it was.
    pub fn run(&self, repo: &Repo) -> Result<String> {
        let revs = self.to_apply();
        if revs.is_empty() {
            return Ok(format!(
                "Nothing to do: {} already has {}",
                self.target,
                if self.items.len() == 1 {
                    "this commit"
                } else {
                    "all of these commits"
                }
            ));
        }
        match &self.onto {
            None => history::cherry_pick(repo, &revs),
            Some(branch) => {
                let msg = in_worktree(repo, branch, |wt| history::cherry_pick(wt, &revs))?;
                Ok(format!("{msg} onto {branch}"))
            }
        }
    }
}

/// Run `job` in a temporary linked work tree that has `branch` checked out, then remove it. When
/// `job` fails, an operation it left half-way is aborted first, so `branch` ends where it started.
fn in_worktree<T>(repo: &Repo, branch: &str, job: impl FnOnce(&Repo) -> Result<T>) -> Result<T> {
    let dir: PathBuf = repo
        .data_dir()?
        .join(format!("worktree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.to_string_lossy().into_owned();
    repo.run(&["worktree", "add", "--quiet", &path, branch])
        .with_context(|| {
            format!("{branch} cannot be checked out for this (is it open in another work tree?)")
        })?;
    let result = Repo::discover(&dir).and_then(|wt| {
        let before = wt.head_commit();
        job(&wt).map_err(|e| {
            for op in ["cherry-pick", "revert", "merge"] {
                let _ = wt.run(&[op, "--abort"]);
            }
            if let Some(before) = &before {
                let _ = wt.run(&["reset", "--hard", "-q", before]);
            }
            anyhow!(
                "{branch} was left as it was. To resolve this by hand, switch to {branch} and \
                 repeat the operation there.\n\n{}",
                format!("{e:#}").replace(
                    " Resolve them in the Conflicts tab (6), then continue or abort there.",
                    ""
                )
            )
        })
    });
    let _ = repo.run(&["worktree", "remove", "--force", &path]);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = repo.run(&["worktree", "prune"]);
    result
}

/// Check `revs` for a revert: all must resolve; returned newest first, the order they are
/// reverted in (later changes are undone first so the reverts apply cleanly).
pub fn revert_plan(repo: &Repo, revs: &[String]) -> Result<Vec<Commit>> {
    let mut commits = resolve(repo, revs)?;
    commits.sort_by_key(|c| std::cmp::Reverse(c.commit_time));
    Ok(commits)
}

/// Revert `commits` in the given order, one revert commit each.
pub fn revert(repo: &Repo, commits: &[Commit]) -> Result<String> {
    for c in commits {
        history::revert(repo, &c.id, c.is_merge())?;
    }
    Ok(match commits {
        [one] => format!("Reverted {}", one.short),
        _ => format!("Reverted {} commits", commits.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    /// main: base, m1 | feature (from base): f1, f2, f3 touching separate files.
    fn setup() -> (TempDir, Repo, Vec<String>) {
        let (d, repo) = temp_repo();
        commit_files(&repo, &[("base.txt", "base\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        let f: Vec<String> = (1..=3)
            .map(|i| {
                let name = format!("f{i}.txt");
                let id = commit_files(&repo, &[(name.as_str(), "x\n")], &format!("feature {i}"));
                // Distinct commit dates, so the default order is well defined.
                std::thread::sleep(std::time::Duration::from_millis(1100));
                id
            })
            .collect();
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("m1.txt", "m\n")], "main 1");
        (d, repo, f)
    }

    fn subjects(repo: &Repo, rev: &str, n: usize) -> Vec<String> {
        repo.run(&["log", "--format=%s", &format!("-{n}"), rev])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn parses_pasted_lists() {
        assert_eq!(
            parse_revs("abc123, def456;\n - `0a1b2c3`\n(fedcba9) \"1234567\""),
            ["abc123", "def456", "0a1b2c3", "fedcba9", "1234567"]
        );
        assert!(parse_revs(" \n ,").is_empty());
    }

    #[test]
    fn picks_full_hashes_oldest_first_whatever_the_order_given() {
        let (_d, repo, f) = setup();
        let revs = vec![f[2].clone(), f[0].clone(), f[1].clone(), f[0].clone()];
        let p = plan(&repo, &revs, None, false).unwrap();
        assert_eq!(p.to_apply(), f, "sorted by date, duplicate dropped");
        assert_eq!(p.target, "main");
        assert!(p.describe().starts_with("Cherry-pick 3 commits onto main:"));
        assert_eq!(p.run(&repo).unwrap(), "Cherry-picked 3 commits");
        assert_eq!(
            subjects(&repo, "HEAD", 4),
            ["feature 3", "feature 2", "feature 1", "main 1"]
        );
    }

    #[test]
    fn keep_order_applies_as_given() {
        let (_d, repo, f) = setup();
        let revs = vec![f[2].clone(), f[0].clone()];
        let p = plan(&repo, &revs, None, true).unwrap();
        assert_eq!(p.to_apply(), revs);
        p.run(&repo).unwrap();
        assert_eq!(subjects(&repo, "HEAD", 2), ["feature 1", "feature 3"]);
    }

    #[test]
    fn every_bad_hash_is_reported_and_nothing_runs() {
        let (_d, repo, f) = setup();
        let head = repo.head_commit();
        let revs = vec![
            f[0].clone(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
            "nope".to_string(),
        ];
        let err = format!("{:#}", plan(&repo, &revs, None, false).unwrap_err());
        assert!(err.contains("2 of 3 could not be found"), "{err}");
        assert!(
            err.contains("0123456789abcdef0123456789abcdef01234567"),
            "{err}"
        );
        assert!(err.contains("nope"), "{err}");
        assert_eq!(repo.head_commit(), head);
        assert!(plan(&repo, &[], None, false).is_err());
    }

    #[test]
    fn commits_already_on_the_branch_are_left_out() {
        let (_d, repo, f) = setup();
        repo.run(&["checkout", "-q", "feature"]).unwrap();
        let p = plan(&repo, &f[..2], None, false).unwrap();
        assert!(p.items.iter().all(|i| i.status == PickStatus::OnTarget));
        assert!(p.describe().contains("already on the branch"));
        let head = repo.head_commit();
        assert!(p.run(&repo).unwrap().starts_with("Nothing to do"));
        assert_eq!(repo.head_commit(), head);
    }

    #[test]
    fn onto_another_branch_leaves_the_work_tree_alone() {
        let (_d, repo, f) = setup();
        repo.run(&["branch", "release", "main"]).unwrap();
        // Uncommitted work on main must survive.
        write(&repo, "m1.txt", "work in progress\n");
        let p = plan(&repo, &f[..2], Some("release"), false).unwrap();
        assert_eq!(p.target, "release");
        assert_eq!(
            p.run(&repo).unwrap(),
            "Cherry-picked 2 commits onto release"
        );
        assert_eq!(
            subjects(&repo, "release", 3),
            ["feature 2", "feature 1", "main 1"]
        );
        assert_eq!(repo.head_branch().as_deref(), Some("main"));
        assert_eq!(subjects(&repo, "HEAD", 1), ["main 1"]);
        assert_eq!(
            std::fs::read_to_string(repo.path("m1.txt")).unwrap(),
            "work in progress\n"
        );
        // The temporary work tree is gone.
        let list = repo.run(&["worktree", "list"]).unwrap();
        assert_eq!(list.lines().count(), 1, "{list}");

        // The current branch named explicitly is a normal pick.
        let p = plan(&repo, &f[2..], Some("main"), false).unwrap();
        assert_eq!(p.onto, None);
        assert!(plan(&repo, &f, Some("no-such-branch"), false).is_err());
    }

    #[test]
    fn conflict_onto_another_branch_leaves_that_branch_as_it_was() {
        let (_d, repo, _f) = setup();
        repo.run(&["checkout", "-q", "-b", "release"]).unwrap();
        commit_files(&repo, &[("base.txt", "release side\n")], "release change");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        let clash = commit_files(&repo, &[("base.txt", "main side\n")], "main change");
        let release = repo.rev_parse("release").unwrap();
        let p = plan(&repo, &[clash], Some("release"), false).unwrap();
        let err = format!("{:#}", p.run(&repo).unwrap_err());
        assert!(err.contains("release was left as it was"), "{err}");
        assert!(err.contains("conflicts"), "{err}");
        assert_eq!(repo.rev_parse("release").unwrap(), release);
        assert_eq!(repo.state(), crate::git::RepoState::Clean);
        assert_eq!(repo.run(&["worktree", "list"]).unwrap().lines().count(), 1);
    }

    #[test]
    fn reverts_newest_first() {
        let (_d, repo, f) = setup();
        repo.run(&["checkout", "-q", "feature"]).unwrap();
        let commits = revert_plan(&repo, &[f[0].clone(), f[2].clone()]).unwrap();
        assert_eq!(commits[0].id, f[2]);
        assert_eq!(revert(&repo, &commits).unwrap(), "Reverted 2 commits");
        assert_eq!(
            subjects(&repo, "HEAD", 2),
            ["Revert \"feature 1\"", "Revert \"feature 3\""]
        );
        assert!(!repo.path("f1.txt").exists() && !repo.path("f3.txt").exists());
        assert!(revert_plan(&repo, &["nope".to_string()]).is_err());
    }
}
