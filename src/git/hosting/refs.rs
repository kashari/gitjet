//! Provider-neutral diff and checkout: fetch the request's git ref (or its source branch) into a
//! local branch `pr/<N>` with plain git, then diff / check out locally. No API is involved.
//!
//! | Provider | Ref |
//! |---|---|
//! | GitHub, Gitea/Forgejo | `refs/pull/N/head` |
//! | GitLab | `refs/merge-requests/N/head` |
//! | Azure DevOps | `refs/pull/N/merge` (its second parent is the head) |
//! | Bitbucket Server | `refs/pull-requests/N/from` |
//! | Bitbucket Cloud | none: the source branch is fetched from the source repository |

use anyhow::{anyhow, bail, Result};

use crate::git::Repo;

use super::{Hosting, ProviderKind, PullRequest};

/// The ref the provider publishes for request `number`, if any.
pub fn pr_ref(kind: ProviderKind, number: u64) -> Option<String> {
    Some(match kind {
        ProviderKind::GitHub | ProviderKind::Gitea => format!("refs/pull/{number}/head"),
        ProviderKind::GitLab => format!("refs/merge-requests/{number}/head"),
        ProviderKind::Azure => format!("refs/pull/{number}/merge"),
        ProviderKind::BitbucketServer => format!("refs/pull-requests/{number}/from"),
        ProviderKind::BitbucketCloud | ProviderKind::Generic => return None,
    })
}

/// Local branch a request is checked out as.
pub fn local_branch(number: u64) -> String {
    format!("pr/{number}")
}

/// Private ref the fetch writes to (never a checked-out branch, so fetching always works).
fn fetched_ref(number: u64) -> String {
    format!("refs/gitjet/pull/{number}")
}

/// Where the request's commits come from when its ref is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrSource {
    /// Source branch.
    pub head: String,
    /// Target branch.
    pub base: String,
    /// Clone URL of the source repository; `None` means the remote itself.
    pub clone_url: Option<String>,
}

impl PrSource {
    /// From a listed request, deriving a fork's clone URL from the remote's format.
    pub fn of(h: &Hosting, pr: &PullRequest) -> PrSource {
        let clone_url = pr.source_repo.as_ref().map(|slug| {
            // Prefer the remote's own format (ssh stays ssh) over the API's https URL.
            let derived = h.clone_url_of(slug);
            if derived != h.fetch_url {
                derived
            } else {
                pr.source_clone_url.clone().unwrap_or(derived)
            }
        });
        PrSource {
            head: pr.head.clone(),
            base: pr.base.clone(),
            clone_url,
        }
    }
}

/// Result of [`fetch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPr {
    /// `pr/<N>`.
    pub branch: String,
    /// Head commit of the request.
    pub head: String,
    /// Tip of the target branch after fetching it, when known.
    pub base_tip: Option<String>,
    /// What was fetched (a ref name or `branch from url`).
    pub via: String,
    /// Set when `pr/<N>` could not be moved to the new head (local commits on it).
    pub note: Option<String>,
}

fn fetch_into(repo: &Repo, source: &str, refspec_src: &str, dst: &str) -> Result<()> {
    let refspec = format!("+{refspec_src}:{dst}");
    repo.run(&["fetch", "--no-tags", "--quiet", source, &refspec])
        .map(|_| ())
}

/// Fetch request `number` into `pr/<number>` (and its base branch into the remote-tracking ref).
pub fn fetch(
    repo: &Repo,
    h: &Hosting,
    number: u64,
    source: Option<&PrSource>,
) -> Result<FetchedPr> {
    let dst = fetched_ref(number);
    let mut errors = Vec::new();
    let mut via = None;
    if let Some(r) = pr_ref(h.kind, number) {
        match fetch_into(repo, &h.remote, &r, &dst) {
            Ok(()) => via = Some(r),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if via.is_none() {
        if let Some(src) = source.filter(|s| !s.head.is_empty()) {
            let url = src.clone_url.as_deref().unwrap_or(&h.remote);
            match fetch_into(repo, url, &format!("refs/heads/{}", src.head), &dst) {
                Ok(()) => via = Some(format!("{} from {url}", src.head)),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
    }
    let Some(via) = via else {
        if errors.is_empty() {
            bail!(
                "{} publishes no ref for {} and its source branch is unknown",
                h.kind.name(),
                h.reference(number)
            );
        }
        bail!(
            "Could not fetch {}: {}",
            h.reference(number),
            errors.join("; ")
        );
    };
    let fetched = repo.rev_parse(&dst)?;
    // Azure DevOps publishes the merge preview; its second parent is the source head.
    let head = if h.kind == ProviderKind::Azure && via.ends_with("/merge") {
        repo.rev_parse(&format!("{dst}^2")).unwrap_or(fetched)
    } else {
        fetched
    };
    let base_tip = source.filter(|s| !s.base.is_empty()).and_then(|s| {
        let tracking = format!("refs/remotes/{}/{}", h.remote, s.base);
        let _ = fetch_into(
            repo,
            &h.remote,
            &format!("refs/heads/{}", s.base),
            &tracking,
        );
        repo.rev_parse(&tracking).ok()
    });
    let branch = local_branch(number);
    let note = update_branch(repo, &branch, &head)?;
    Ok(FetchedPr {
        branch,
        head,
        base_tip,
        via,
        note,
    })
}

/// Point `branch` at `head` without losing local work: create it, fast-forward it, or leave it
/// (with a note) when it has commits of its own.
fn update_branch(repo: &Repo, branch: &str, head: &str) -> Result<Option<String>> {
    let full = format!("refs/heads/{branch}");
    let Ok(old) = repo.rev_parse(&full) else {
        repo.run(&["branch", branch, head])?;
        return Ok(None);
    };
    if old == head {
        return Ok(None);
    }
    let fast_forward = repo
        .run(&["merge-base", "--is-ancestor", &old, head])
        .is_ok();
    if !fast_forward {
        return Ok(Some(format!(
            "{branch} has local commits and was not moved; the request's head is {}",
            &head[..head.len().min(10)]
        )));
    }
    if repo.head_branch().as_deref() == Some(branch) {
        repo.run(&["merge", "--ff-only", "--quiet", head])
            .map_err(|e| anyhow!("could not fast-forward {branch}: {e:#}"))?;
    } else if repo.run(&["update-ref", &full, head, &old]).is_err() {
        return Ok(Some(format!(
            "{branch} is checked out elsewhere and was not moved"
        )));
    }
    Ok(None)
}

/// (from, to) for the request's diff: merge base of target and head → head, i.e.
/// `git diff <base>...pr/<N>`.
pub fn diff_range(repo: &Repo, fetched: &FetchedPr) -> Result<(String, String)> {
    let base = fetched
        .base_tip
        .as_deref()
        .ok_or_else(|| anyhow!("the target branch could not be fetched"))?;
    let mb = repo.run(&["merge-base", base, &fetched.head])?;
    Ok((mb.trim().to_string(), fetched.head.clone()))
}

/// Fetch and check out `pr/<N>`. Returns a message for the user.
pub fn checkout(
    repo: &Repo,
    h: &Hosting,
    number: u64,
    source: Option<&PrSource>,
) -> Result<String> {
    let f = fetch(repo, h, number, source)?;
    if repo.head_branch().as_deref() != Some(f.branch.as_str()) {
        repo.run(&["checkout", "--quiet", &f.branch])?;
    }
    let mut msg = format!("Checked out {} as {}", h.reference(number), f.branch);
    if let Some(n) = f.note {
        msg.push_str(&format!(" ({n})"));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::{parse_remote_url, DetectedBy};
    use crate::git::testutil::*;
    use std::process::Command;

    #[test]
    fn ref_names_per_provider() {
        assert_eq!(
            pr_ref(ProviderKind::GitHub, 1).as_deref(),
            Some("refs/pull/1/head")
        );
        assert_eq!(
            pr_ref(ProviderKind::Gitea, 2).as_deref(),
            Some("refs/pull/2/head")
        );
        assert_eq!(
            pr_ref(ProviderKind::GitLab, 3).as_deref(),
            Some("refs/merge-requests/3/head")
        );
        assert_eq!(
            pr_ref(ProviderKind::Azure, 4).as_deref(),
            Some("refs/pull/4/merge")
        );
        assert_eq!(
            pr_ref(ProviderKind::BitbucketServer, 5).as_deref(),
            Some("refs/pull-requests/5/from")
        );
        assert_eq!(pr_ref(ProviderKind::BitbucketCloud, 6), None);
        assert_eq!(pr_ref(ProviderKind::Generic, 7), None);
        assert_eq!(local_branch(9), "pr/9");
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A bare "server" with `main` and a request published as `pub_ref`, and a clone-like repo
    /// whose `origin` points at it.
    fn server_with_pr(kind: ProviderKind, pub_ref: &str) -> (TempDir, TempDir, Repo, Hosting) {
        let bare = tempfile::tempdir().unwrap();
        git_in(bare.path(), &["init", "-q", "--bare"]);
        let (d, author) = temp_repo();
        let bare_path = bare.path().to_string_lossy().to_string();
        author
            .run(&["remote", "add", "origin", &bare_path])
            .unwrap();
        commit_files(&author, &[("a.txt", "one\n")], "base");
        author.run(&["push", "-q", "origin", "main"]).unwrap();
        author.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(
            &author,
            &[("a.txt", "one\ntwo\n"), ("b.txt", "new\n")],
            "feature work",
        );
        author
            .run(&["push", "-q", "origin", &format!("HEAD:{pub_ref}")])
            .unwrap();
        author.run(&["push", "-q", "origin", "feature"]).unwrap();
        drop(d);
        let (d2, repo) = temp_repo();
        repo.run(&["remote", "add", "origin", &bare_path]).unwrap();
        repo.run(&["fetch", "-q", "origin"]).unwrap();
        repo.run(&["checkout", "-q", "-b", "main", "origin/main"])
            .ok();
        let url = parse_remote_url("https://github.com/o/r.git").unwrap();
        let h = Hosting::new(kind, "origin", &bare_path, url, None, DetectedBy::KnownHost);
        (bare, d2, repo, h)
    }

    #[test]
    fn fetches_pull_ref_and_diffs_against_base() {
        let (_bare, _d, repo, h) = server_with_pr(ProviderKind::GitHub, "refs/pull/1/head");
        let src = PrSource {
            head: "feature".into(),
            base: "main".into(),
            clone_url: None,
        };
        let f = fetch(&repo, &h, 1, Some(&src)).unwrap();
        assert_eq!(f.branch, "pr/1");
        assert_eq!(f.via, "refs/pull/1/head");
        assert_eq!(repo.rev_parse("pr/1").unwrap(), f.head);
        let (from, to) = diff_range(&repo, &f).unwrap();
        assert_eq!(from, repo.rev_parse("origin/main").unwrap());
        let diffs = repo
            .diff_range(&from, &to, &[], &crate::git::diff::DiffOptions::default())
            .unwrap();
        let mut paths: Vec<String> = diffs.iter().map(|d| d.path().to_string()).collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", "b.txt"]);
        let msg = checkout(&repo, &h, 1, Some(&src)).unwrap();
        assert!(msg.contains("pr/1"), "{msg}");
        assert_eq!(repo.head_branch().as_deref(), Some("pr/1"));
        // Fetching again while checked out is fine.
        assert!(fetch(&repo, &h, 1, Some(&src)).unwrap().note.is_none());
    }

    #[test]
    fn falls_back_to_the_source_branch() {
        // Bitbucket Cloud publishes no ref: the source branch is fetched instead.
        let (_bare, _d, repo, h) = server_with_pr(ProviderKind::BitbucketCloud, "refs/pull/2/head");
        assert!(fetch(&repo, &h, 2, None).is_err());
        let src = PrSource {
            head: "feature".into(),
            base: "main".into(),
            clone_url: None,
        };
        let f = fetch(&repo, &h, 2, Some(&src)).unwrap();
        assert!(f.via.starts_with("feature from origin"), "{}", f.via);
        assert_eq!(f.head, repo.rev_parse("origin/feature").unwrap());
        // A missing ref on GitHub also falls back.
        let (_b2, _d2, repo2, h2) = server_with_pr(ProviderKind::GitHub, "refs/heads/other");
        let f = fetch(&repo2, &h2, 3, Some(&src)).unwrap();
        assert!(f.via.contains("from"));
    }

    #[test]
    fn local_commits_on_the_pr_branch_are_kept() {
        let (_bare, _d, repo, h) = server_with_pr(ProviderKind::GitHub, "refs/pull/1/head");
        let f = fetch(&repo, &h, 1, None).unwrap();
        repo.run(&["checkout", "-q", "pr/1"]).unwrap();
        let mine = commit_files(&repo, &[("c.txt", "mine\n")], "local work");
        repo.run(&["update-ref", "refs/gitjet/pull/1", &f.head])
            .unwrap();
        let again = fetch(&repo, &h, 1, None).unwrap();
        assert!(again.note.is_some());
        assert_eq!(repo.rev_parse("pr/1").unwrap(), mine);
        assert!(
            diff_range(&repo, &again).is_err(),
            "no base without a source"
        );
    }
}
