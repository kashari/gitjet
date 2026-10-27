//! Local git helpers for creating requests: the default branch from local refs, where the branch
//! is pushed (never onto the base or default branch), fork detection and title/description
//! suggestions from commits.

use anyhow::{anyhow, bail, Result};

use super::detect::remotes;
use super::json::normalize_text;
use super::remote_url::{parse_remote_url, RemoteUrl};
use super::Hosting;
use crate::git::Repo;

/// Default branch guessed from local refs: `<remote>/HEAD`, then `main`/`master`/`develop` on the
/// remote, then local branches, else `main`.
pub fn local_default_branch(repo: &Repo, remote: &str) -> String {
    let head_ref = format!("refs/remotes/{remote}/HEAD");
    if let Some(out) = repo.try_run(&["symbolic-ref", "--short", "-q", &head_ref]) {
        let full = out.trim();
        if let Some(b) = full.strip_prefix(&format!("{remote}/")) {
            if !b.is_empty() {
                return b.to_string();
            }
        }
    }
    for b in ["main", "master", "develop"] {
        if repo
            .try_run(&[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/remotes/{remote}/{b}"),
            ])
            .is_some()
        {
            return b.to_string();
        }
    }
    for b in ["main", "master"] {
        if repo
            .try_run(&["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")])
            .is_some()
        {
            return b.to_string();
        }
    }
    "main".into()
}

/// Remote explicitly configured for pushing `branch`: `branch.<b>.pushRemote`, then
/// `remote.pushDefault` (only remotes that exist).
fn explicit_push_remote(repo: &Repo, branch: &str, all: &[(String, String)]) -> Option<String> {
    [
        repo.config_get(&format!("branch.{branch}.pushRemote")),
        repo.config_get("remote.pushDefault"),
    ]
    .into_iter()
    .flatten()
    .find(|r| all.iter().any(|(n, _)| n == r))
}

/// Remote a new branch is pushed to: `branch.<b>.pushRemote`, `remote.pushDefault`, `origin`,
/// `branch.<b>.remote` (a real remote, never `.`), else `fallback`.
pub fn push_remote(repo: &Repo, branch: &str, fallback: &str) -> String {
    let all = remotes(repo);
    let exists = |r: &str| all.iter().any(|(n, _)| n == r);
    explicit_push_remote(repo, branch, &all)
        .or_else(|| exists("origin").then(|| "origin".to_string()))
        .or_else(|| {
            repo.config_get(&format!("branch.{branch}.remote"))
                .filter(|r| exists(r))
        })
        .unwrap_or_else(|| fallback.to_string())
}

/// Where [`ensure_pushed`] sends a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub remote: String,
    /// Branch name on the remote.
    pub branch: String,
    /// True when the configured upstream is reused as-is; false means `git push -u` to
    /// `remote/branch`, which also makes it the branch's new upstream.
    pub reuse_upstream: bool,
}

impl PushTarget {
    /// `remote/branch`.
    pub fn display(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
}

/// Resolve where `branch` is pushed for a pull request.
///
/// The configured upstream is reused only when it is a real remote (not `.`, which tracks a local
/// branch), agrees with any explicit push remote, and has the same branch name — or `push.default`
/// is explicitly `upstream`. So a branch started from `origin/main` goes to `origin/<branch>`, never
/// onto `main`.
pub fn push_target(repo: &Repo, branch: &str, fallback_remote: &str) -> PushTarget {
    let all = remotes(repo);
    let explicit = explicit_push_remote(repo, branch, &all);
    let up_remote = repo
        .config_get(&format!("branch.{branch}.remote"))
        .filter(|r| all.iter().any(|(n, _)| n == r));
    let up_branch = repo
        .config_get(&format!("branch.{branch}.merge"))
        .and_then(|m| m.strip_prefix("refs/heads/").map(str::to_string))
        .filter(|b| !b.is_empty());
    let push_default_upstream = matches!(
        repo.config_get("push.default").as_deref(),
        Some("upstream") | Some("tracking")
    );
    if let (Some(remote), Some(remote_branch)) = (up_remote, up_branch) {
        let agrees = explicit.is_none() || explicit.as_ref() == Some(&remote);
        if agrees && (remote_branch == branch || push_default_upstream) {
            return PushTarget {
                remote,
                branch: remote_branch,
                reuse_upstream: true,
            };
        }
    }
    PushTarget {
        remote: push_remote(repo, branch, fallback_remote),
        branch: branch.to_string(),
        reuse_upstream: false,
    }
}

/// Why pushing `local_branch` to `target` for a request into `base` of the repository behind
/// `target_remote` is refused, if it is.
///
/// - The head would be the base branch of the target repository itself.
/// - Commits would land on the base or default branch under another name, or on the target
///   repository's default branch — publishing unreviewed work.
pub fn push_refusal(
    target: &PushTarget,
    local_branch: &str,
    target_remote: &str,
    base: &str,
    default_branch: &str,
    needs_push: bool,
) -> Option<String> {
    let same_repo = target.remote == target_remote;
    if same_repo && target.branch == base {
        return Some(format!(
            "'{local_branch}' would be pushed to {}, the base branch itself; \
             create a feature branch for the change first",
            target.display()
        ));
    }
    let protected = target.branch == base || target.branch == default_branch;
    if needs_push && protected && (same_repo || target.branch != local_branch) {
        return Some(format!(
            "Refusing to push '{local_branch}' to {}: it is the base or default branch; \
             create a feature branch for the change first",
            target.display()
        ));
    }
    None
}

/// Where the branch was pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedBranch {
    pub remote: String,
    /// Branch name on the remote.
    pub branch: String,
    /// True if commits were pushed by this call.
    pub pushed: bool,
}

/// Make sure the checked-out `branch` exists on a remote with all local commits; returns where.
///
/// `target_remote` is the remote of the repository the request is opened against. Pushes that
/// [`push_refusal`] rejects fail before anything is sent. When the upstream is not reused the
/// branch is pushed with `git push -u <remote> HEAD:refs/heads/<branch>`.
pub fn ensure_pushed(
    repo: &Repo,
    branch: &str,
    target_remote: &str,
    base: &str,
    default_branch: &str,
) -> Result<PushedBranch> {
    let target = push_target(repo, branch, target_remote);
    let head = repo
        .head_commit()
        .ok_or_else(|| anyhow!("the repository has no commits yet"))?;
    let needs_push = if target.reuse_upstream {
        let ahead: Option<u64> = repo
            .try_run(&["rev-list", "--count", "@{upstream}..HEAD"])
            .and_then(|s| s.trim().parse().ok());
        ahead != Some(0)
    } else {
        let tracking = format!("refs/remotes/{}/{}", target.remote, target.branch);
        repo.try_run(&["rev-parse", "--verify", "-q", &tracking])
            .map(|s| s.trim().to_string())
            != Some(head)
    };
    if let Some(why) = push_refusal(
        &target,
        branch,
        target_remote,
        base,
        default_branch,
        needs_push,
    ) {
        bail!(why);
    }
    let refspec = format!("HEAD:refs/heads/{}", target.branch);
    if !target.reuse_upstream {
        // Runs even when up to date: it points the upstream at the request's branch.
        repo.run(&["push", "-u", &target.remote, &refspec])?;
    } else if needs_push {
        repo.run(&["push", &target.remote, &refspec])?;
    }
    Ok(PushedBranch {
        remote: target.remote,
        branch: target.branch,
        pushed: needs_push,
    })
}

/// The fork a pushed branch lives in, when it differs from the target repository.
pub fn fork_of(repo: &Repo, hosting: &Hosting, push_remote: &str) -> Option<RemoteUrl> {
    if push_remote == hosting.remote {
        return None;
    }
    let url = repo.try_run(&["remote", "get-url", push_remote])?;
    let parsed = parse_remote_url(&url)?;
    (parsed.host == hosting.url.host && parsed.slug() != hosting.url.slug()).then_some(parsed)
}

/// Subject of the last commit (title suggestion), empty in a repository without commits.
pub fn last_commit_subject(repo: &Repo) -> String {
    repo.try_run(&["log", "-1", "--format=%s"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Description suggestion: the commit body when the branch has one commit over `remote/base`,
/// a bullet list of subjects when it has several (newest last), else the last commit's body.
pub fn suggested_description(repo: &Repo, remote: &str, base: &str) -> String {
    let range = format!("refs/remotes/{remote}/{base}..HEAD");
    let subjects: Vec<String> = repo
        .try_run(&["log", "--reverse", "--max-count=50", "--format=%s", &range])
        .map(|out| {
            out.lines()
                .map(|l| l.trim_end_matches('\r').to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if subjects.len() > 1 {
        return subjects
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    repo.try_run(&["log", "-1", "--format=%b"])
        .map(|s| normalize_text(s.trim()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;
    use std::process::Command;

    #[test]
    fn default_branch_and_descriptions() {
        let (_d, repo) = temp_repo();
        assert_eq!(local_default_branch(&repo, "origin"), "main");
        assert_eq!(last_commit_subject(&repo), "");
        commit_files(&repo, &[("a.txt", "1\n")], "first\n\nThe body");
        assert_eq!(last_commit_subject(&repo), "first");
        assert_eq!(suggested_description(&repo, "origin", "main"), "The body");
        repo.run(&["update-ref", "refs/remotes/origin/main", "HEAD"])
            .unwrap();
        commit_files(&repo, &[("b.txt", "2\n")], "second");
        commit_files(&repo, &[("c.txt", "3\n")], "third");
        assert_eq!(
            suggested_description(&repo, "origin", "main"),
            "- second\n- third"
        );
        assert_eq!(local_default_branch(&repo, "origin"), "main");
    }

    #[test]
    fn ensure_pushed_sets_upstream_and_pushes_new_commits() {
        let (_d, repo) = temp_repo();
        let bare = tempfile::tempdir().unwrap();
        let ok = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(bare.path())
            .status()
            .unwrap()
            .success();
        assert!(ok);
        repo.run(&["remote", "add", "origin", &bare.path().to_string_lossy()])
            .unwrap();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        repo.run(&["checkout", "-q", "-b", "feature/x"]).unwrap();
        commit_files(&repo, &[("b.txt", "2\n")], "second");
        let p = ensure_pushed(&repo, "feature/x", "origin", "main", "main").unwrap();
        assert_eq!(
            p,
            PushedBranch {
                remote: "origin".into(),
                branch: "feature/x".into(),
                pushed: true
            }
        );
        let again = ensure_pushed(&repo, "feature/x", "origin", "main", "main").unwrap();
        assert!(!again.pushed);
        commit_files(&repo, &[("c.txt", "3\n")], "third");
        assert!(
            ensure_pushed(&repo, "feature/x", "origin", "main", "main")
                .unwrap()
                .pushed
        );
        let remote_head = Command::new("git")
            .arg("-C")
            .arg(bare.path())
            .args(["rev-parse", "refs/heads/feature/x"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            repo.head_commit().unwrap()
        );
        assert_eq!(push_remote(&repo, "feature/x", "upstream"), "origin");
    }

    /// A repository with a bare `origin` holding `main` (one commit), checked out on `main`.
    fn repo_with_bare_origin() -> (tempfile::TempDir, tempfile::TempDir, Repo) {
        let (d, repo) = temp_repo();
        let bare = tempfile::tempdir().unwrap();
        let ok = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(bare.path())
            .status()
            .unwrap()
            .success();
        assert!(ok);
        repo.run(&["remote", "add", "origin", &bare.path().to_string_lossy()])
            .unwrap();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        repo.run(&["branch", "-M", "main"]).unwrap();
        repo.run(&["push", "-q", "-u", "origin", "main"]).unwrap();
        (d, bare, repo)
    }

    fn bare_ref(bare: &tempfile::TempDir, r: &str) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(bare.path())
            .args(["rev-parse", "--verify", "-q", r])
            .output();
        out.ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    #[test]
    fn branch_tracking_origin_main_is_pushed_under_its_own_name() {
        let (_d, bare, repo) = repo_with_bare_origin();
        let main_before = bare_ref(&bare, "refs/heads/main").unwrap();
        repo.run(&["checkout", "-q", "-b", "feature", "--track", "origin/main"])
            .unwrap();
        commit_files(&repo, &[("b.txt", "2\n")], "feature work");
        let t = push_target(&repo, "feature", "origin");
        assert_eq!(
            t,
            PushTarget {
                remote: "origin".into(),
                branch: "feature".into(),
                reuse_upstream: false
            }
        );
        let p = ensure_pushed(&repo, "feature", "origin", "main", "main").unwrap();
        assert_eq!(
            p,
            PushedBranch {
                remote: "origin".into(),
                branch: "feature".into(),
                pushed: true
            }
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            Some(main_before),
            "remote main must not move"
        );
        assert_eq!(bare_ref(&bare, "refs/heads/feature"), repo.head_commit());
        assert_eq!(
            repo.config_get("branch.feature.merge").as_deref(),
            Some("refs/heads/feature")
        );
        // Now the upstream has the branch's name and is reused.
        assert!(push_target(&repo, "feature", "origin").reuse_upstream);
        assert!(
            !ensure_pushed(&repo, "feature", "origin", "main", "main")
                .unwrap()
                .pushed
        );
    }

    #[test]
    fn branch_tracking_a_local_branch_never_rewrites_it() {
        let (_d, bare, repo) = repo_with_bare_origin();
        let local_main = repo.rev_parse("main").unwrap();
        repo.run(&["checkout", "-q", "-b", "topic", "--track", "main"])
            .unwrap();
        assert_eq!(repo.config_get("branch.topic.remote").as_deref(), Some("."));
        commit_files(&repo, &[("t.txt", "t\n")], "topic work");
        let p = ensure_pushed(&repo, "topic", "origin", "main", "main").unwrap();
        assert_eq!(p.remote, "origin");
        assert_eq!(p.branch, "topic");
        assert_eq!(
            repo.rev_parse("main").unwrap(),
            local_main,
            "local main must not move"
        );
        assert_eq!(bare_ref(&bare, "refs/heads/topic"), repo.head_commit());
    }

    #[test]
    fn pushes_onto_base_or_default_branch_are_refused() {
        let (_d, bare, repo) = repo_with_bare_origin();
        let main_before = bare_ref(&bare, "refs/heads/main");
        // On main itself: head would equal base.
        commit_files(&repo, &[("m.txt", "m\n")], "on main");
        let err = ensure_pushed(&repo, "main", "origin", "main", "main").unwrap_err();
        assert!(format!("{err:#}").contains("base branch"));
        assert_eq!(bare_ref(&bare, "refs/heads/main"), main_before);
        // push.default=upstream makes the upstream main the target: refused before pushing.
        repo.run(&["checkout", "-q", "-b", "feature", "--track", "origin/main"])
            .unwrap();
        repo.run(&["config", "push.default", "upstream"]).unwrap();
        commit_files(&repo, &[("f.txt", "f\n")], "feature work");
        let t = push_target(&repo, "feature", "origin");
        assert!(t.reuse_upstream);
        assert_eq!(t.branch, "main");
        assert!(ensure_pushed(&repo, "feature", "origin", "develop", "main").is_err());
        assert_eq!(bare_ref(&bare, "refs/heads/main"), main_before);
        assert!(bare_ref(&bare, "refs/heads/feature").is_none());
    }

    #[test]
    fn push_refusal_rules() {
        let t = |remote: &str, branch: &str| PushTarget {
            remote: remote.into(),
            branch: branch.into(),
            reuse_upstream: false,
        };
        // Same repository, head == base.
        assert!(push_refusal(
            &t("origin", "main"),
            "main",
            "origin",
            "main",
            "main",
            false
        )
        .is_some());
        // Default branch with commits to push, as a PR into another base.
        assert!(push_refusal(
            &t("origin", "main"),
            "main",
            "origin",
            "develop",
            "main",
            true
        )
        .is_some());
        // ... but nothing to push is fine (a request from main into develop).
        assert!(push_refusal(
            &t("origin", "main"),
            "main",
            "origin",
            "develop",
            "main",
            false
        )
        .is_none());
        // A fork's own main into upstream main is a normal fork workflow.
        assert!(push_refusal(
            &t("origin", "main"),
            "main",
            "upstream",
            "main",
            "main",
            true
        )
        .is_none());
        // Pushing a differently named branch onto a fork's main is refused.
        assert!(push_refusal(
            &t("origin", "main"),
            "feature",
            "upstream",
            "main",
            "main",
            true
        )
        .is_some());
        assert!(push_refusal(
            &t("origin", "feature"),
            "feature",
            "origin",
            "main",
            "main",
            true
        )
        .is_none());
    }

    #[test]
    fn fork_detection() {
        let (_d, repo) = temp_repo();
        repo.run(&["remote", "add", "origin", "git@github.com:me/hello.git"])
            .unwrap();
        repo.run(&[
            "remote",
            "add",
            "upstream",
            "https://github.com/octo/hello.git",
        ])
        .unwrap();
        let cfg = crate::config::HostingConfig::default();
        let super::super::Detection::Found(h) = super::super::detect_local(&repo, &cfg) else {
            panic!("expected hosting")
        };
        assert_eq!(h.remote, "upstream");
        let fork = fork_of(&repo, &h, "origin").unwrap();
        assert_eq!(fork.owner, "me");
        assert!(fork_of(&repo, &h, "upstream").is_none());
    }
}
