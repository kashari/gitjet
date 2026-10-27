//! Browser URLs derived from the remote (no API needed) and a cross-platform [`open_url`].
//!
//! For generic hosts the GitHub-style `/tree/<branch>` and `/commit/<sha>` layout is used, which
//! many smaller hosts share; "create pull request" has no generic URL.

use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::http::{encode, encode_path};
use super::{Hosting, ProviderKind};

/// Bitbucket Server project path: `projects/KEY` or `users/name` for personal (`~name`) repos.
fn bbs_repo_path(h: &Hosting) -> String {
    let owner = &h.url.owner;
    match owner.strip_prefix('~') {
        Some(user) => format!("users/{}/repos/{}", encode(user), encode(&h.url.name)),
        None => format!("projects/{}/repos/{}", encode(owner), encode(&h.url.name)),
    }
}

/// The repository's home page.
pub fn repo_url(h: &Hosting) -> String {
    let web = h.url.web_base.trim_end_matches('/');
    match h.kind {
        ProviderKind::BitbucketServer => format!("{web}/{}/browse", bbs_repo_path(h)),
        ProviderKind::Azure => format!(
            "{web}/{}/_git/{}",
            encode_path(&h.url.owner),
            encode(&h.url.name)
        ),
        _ => format!("{web}/{}", encode_path(&h.slug())),
    }
}

fn repo_root(h: &Hosting) -> String {
    match h.kind {
        ProviderKind::BitbucketServer => {
            format!(
                "{}/{}",
                h.url.web_base.trim_end_matches('/'),
                bbs_repo_path(h)
            )
        }
        _ => repo_url(h),
    }
}

/// A branch's page.
pub fn branch_url(h: &Hosting, branch: &str) -> String {
    let root = repo_root(h);
    let b = encode_path(branch);
    match h.kind {
        ProviderKind::GitLab => format!("{root}/-/tree/{b}"),
        ProviderKind::Gitea => format!("{root}/src/branch/{b}"),
        ProviderKind::BitbucketCloud => format!("{root}/src/{b}"),
        ProviderKind::BitbucketServer => {
            format!(
                "{root}/browse?at={}",
                encode(&format!("refs/heads/{branch}"))
            )
        }
        ProviderKind::Azure => format!("{root}?version=GB{}", encode(branch)),
        ProviderKind::GitHub | ProviderKind::Generic => format!("{root}/tree/{b}"),
    }
}

/// A commit's page.
pub fn commit_url(h: &Hosting, sha: &str) -> String {
    let root = repo_root(h);
    let sha = encode(sha);
    match h.kind {
        ProviderKind::GitLab => format!("{root}/-/commit/{sha}"),
        ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer => {
            format!("{root}/commits/{sha}")
        }
        _ => format!("{root}/commit/{sha}"),
    }
}

/// A request's page.
pub fn pr_url(h: &Hosting, number: u64) -> String {
    let root = repo_root(h);
    match h.kind {
        ProviderKind::GitLab => format!("{root}/-/merge_requests/{number}"),
        ProviderKind::Gitea => format!("{root}/pulls/{number}"),
        ProviderKind::BitbucketCloud => format!("{root}/pull-requests/{number}"),
        ProviderKind::BitbucketServer => format!("{root}/pull-requests/{number}/overview"),
        ProviderKind::Azure => format!("{root}/pullrequest/{number}"),
        ProviderKind::GitHub | ProviderKind::Generic => format!("{root}/pull/{number}"),
    }
}

/// The list of requests.
pub fn pr_list_url(h: &Hosting) -> Option<String> {
    let root = repo_root(h);
    Some(match h.kind {
        ProviderKind::GitLab => format!("{root}/-/merge_requests"),
        ProviderKind::GitHub | ProviderKind::Gitea => format!("{root}/pulls"),
        ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer => {
            format!("{root}/pull-requests")
        }
        ProviderKind::Azure => format!("{root}/pullrequests"),
        ProviderKind::Generic => return None,
    })
}

/// The provider's "create pull request" page for `head` → `base`. `head_owner` is the fork's
/// owner when the branch lives in a fork. `None` for generic hosts.
pub fn create_url(h: &Hosting, head: &str, base: &str, head_owner: Option<&str>) -> Option<String> {
    let root = repo_root(h);
    let q = encode;
    Some(match h.kind {
        ProviderKind::GitHub | ProviderKind::Gitea => {
            let head = match head_owner {
                Some(o) => format!("{}:{}", encode(o), encode_path(head)),
                None => encode_path(head),
            };
            let expand = if h.kind == ProviderKind::GitHub {
                "?expand=1"
            } else {
                ""
            };
            format!("{root}/compare/{}...{head}{expand}", encode_path(base))
        }
        ProviderKind::GitLab => format!(
            "{root}/-/merge_requests/new?{}={}&{}={}",
            q("merge_request[source_branch]"),
            q(head),
            q("merge_request[target_branch]"),
            q(base)
        ),
        ProviderKind::BitbucketCloud => {
            let source = match head_owner {
                Some(o) => format!("{o}/{}:{head}", h.url.name),
                None => head.to_string(),
            };
            format!(
                "{root}/pull-requests/new?source={}&dest={}",
                q(&source),
                q(base)
            )
        }
        ProviderKind::BitbucketServer => format!(
            "{root}/pull-requests?create&sourceBranch={}&targetBranch={}",
            q(&format!("refs/heads/{head}")),
            q(&format!("refs/heads/{base}"))
        ),
        ProviderKind::Azure => format!(
            "{root}/pullrequestcreate?sourceRef={}&targetRef={}",
            q(head),
            q(base)
        ),
        ProviderKind::Generic => return None,
    })
}

/// Argument line for `cmd /C start "" "<url>"` on Windows: quoted so `&`, `|`, `^`, `<`, `>`
/// stay literal, with quotes percent-encoded.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_start_args(url: &str) -> String {
    let safe: String = url
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '"' => "%22".to_string(),
            c => c.to_string(),
        })
        .collect();
    format!("/C start \"\" \"{safe}\"")
}

/// Open an http(s) URL with the platform's default browser.
pub fn open_url(url: &str) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        bail!("refusing to open a non-web URL: {url}");
    }
    if cfg!(test) {
        // Tests must never launch a browser.
        return Ok(());
    }
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.raw_arg(windows_start_args(url));
        c
    };
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut c = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = c
        .spawn()
        .with_context(|| format!("could not open {url} in a browser"))?;
    // Reap the launcher so it does not linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::{parse_remote_url, DetectedBy};

    fn h(kind: ProviderKind, url: &str) -> Hosting {
        Hosting::new(
            kind,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::KnownHost,
        )
    }

    #[test]
    fn github_and_generic() {
        let g = h(ProviderKind::GitHub, "git@github.com:octo/hello.git");
        assert_eq!(repo_url(&g), "https://github.com/octo/hello");
        assert_eq!(
            branch_url(&g, "feat/x"),
            "https://github.com/octo/hello/tree/feat/x"
        );
        assert_eq!(
            commit_url(&g, "abc"),
            "https://github.com/octo/hello/commit/abc"
        );
        assert_eq!(pr_url(&g, 4), "https://github.com/octo/hello/pull/4");
        assert_eq!(
            create_url(&g, "feat/x", "main", Some("me")).unwrap(),
            "https://github.com/octo/hello/compare/main...me:feat/x?expand=1"
        );
        let x = h(ProviderKind::Generic, "https://git.sr.ht/~me/tool");
        assert_eq!(repo_url(&x), "https://git.sr.ht/~me/tool");
        assert_eq!(
            commit_url(&x, "abc"),
            "https://git.sr.ht/~me/tool/commit/abc"
        );
        assert!(create_url(&x, "a", "b", None).is_none());
        assert!(pr_list_url(&x).is_none());
    }

    #[test]
    fn gitlab_gitea_bitbucket() {
        let g = h(ProviderKind::GitLab, "git@gitlab.com:group/sub/proj.git");
        assert_eq!(
            branch_url(&g, "dev"),
            "https://gitlab.com/group/sub/proj/-/tree/dev"
        );
        assert_eq!(
            pr_url(&g, 9),
            "https://gitlab.com/group/sub/proj/-/merge_requests/9"
        );
        assert_eq!(
            create_url(&g, "feat", "main", None).unwrap(),
            "https://gitlab.com/group/sub/proj/-/merge_requests/new?merge_request%5Bsource_branch%5D=feat&merge_request%5Btarget_branch%5D=main"
        );
        let t = h(ProviderKind::Gitea, "https://codeberg.org/a/b.git");
        assert_eq!(
            branch_url(&t, "main"),
            "https://codeberg.org/a/b/src/branch/main"
        );
        assert_eq!(pr_url(&t, 2), "https://codeberg.org/a/b/pulls/2");
        assert_eq!(
            create_url(&t, "f", "main", None).unwrap(),
            "https://codeberg.org/a/b/compare/main...f"
        );
        let b = h(
            ProviderKind::BitbucketCloud,
            "git@bitbucket.org:team/repo.git",
        );
        assert_eq!(
            commit_url(&b, "abc"),
            "https://bitbucket.org/team/repo/commits/abc"
        );
        assert_eq!(
            pr_url(&b, 3),
            "https://bitbucket.org/team/repo/pull-requests/3"
        );
        assert_eq!(
            create_url(&b, "f", "main", None).unwrap(),
            "https://bitbucket.org/team/repo/pull-requests/new?source=f&dest=main"
        );
        let s = h(
            ProviderKind::BitbucketServer,
            "https://bb.corp/scm/PROJ/repo.git",
        );
        assert_eq!(
            repo_url(&s),
            "https://bb.corp/projects/PROJ/repos/repo/browse"
        );
        assert_eq!(
            pr_url(&s, 8),
            "https://bb.corp/projects/PROJ/repos/repo/pull-requests/8/overview"
        );
        assert_eq!(
            branch_url(&s, "dev"),
            "https://bb.corp/projects/PROJ/repos/repo/browse?at=refs%2Fheads%2Fdev"
        );
        let p = h(
            ProviderKind::BitbucketServer,
            "ssh://git@bb.corp:7999/~me/repo.git",
        );
        assert_eq!(repo_url(&p), "https://bb.corp/users/me/repos/repo/browse");
        assert!(create_url(&s, "f", "main", None)
            .unwrap()
            .contains("sourceBranch=refs%2Fheads%2Ff"));
    }

    #[test]
    fn azure() {
        let a = h(
            ProviderKind::Azure,
            "git@ssh.dev.azure.com:v3/org/My Proj/repo",
        );
        assert_eq!(
            repo_url(&a),
            "https://dev.azure.com/org/My%20Proj/_git/repo"
        );
        assert_eq!(
            pr_url(&a, 77),
            "https://dev.azure.com/org/My%20Proj/_git/repo/pullrequest/77"
        );
        assert_eq!(
            branch_url(&a, "feat/x"),
            "https://dev.azure.com/org/My%20Proj/_git/repo?version=GBfeat%2Fx"
        );
        assert_eq!(
            create_url(&a, "f", "main", None).unwrap(),
            "https://dev.azure.com/org/My%20Proj/_git/repo/pullrequestcreate?sourceRef=f&targetRef=main"
        );
        let v = h(
            ProviderKind::Azure,
            "https://org.visualstudio.com/proj/_git/repo",
        );
        assert_eq!(repo_url(&v), "https://org.visualstudio.com/proj/_git/repo");
    }

    #[test]
    fn windows_launch_quoting() {
        assert_eq!(
            windows_start_args("https://x/a?b=1&c=\"2\""),
            "/C start \"\" \"https://x/a?b=1&c=%222%22\""
        );
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
    }
}
