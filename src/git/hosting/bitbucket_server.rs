//! Bitbucket Server / Data Center REST API 1.0.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::http::{encode, Api, MAX_ITEMS};
use super::json::{arr, b, first, normalize_text, s, time, u};
use super::{
    need_text, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr, PrDetails,
    PrProvider, PrState, PullRequest, Review, ReviewKind, StateFilter,
};

pub struct BitbucketServer {
    api: Api,
    /// `{web}` (for build-status, branch-utils and whoami, which live outside `/rest/api/1.0`).
    web: String,
    repo: String,
    project: String,
    name: String,
}

impl BitbucketServer {
    pub fn new(api: Api, base: String, h: &Hosting) -> Self {
        let web = base
            .strip_suffix("/rest/api/1.0")
            .map(str::to_string)
            .unwrap_or_else(|| h.url.web_base.trim_end_matches('/').to_string());
        Self {
            api,
            repo: format!(
                "{base}/projects/{}/repos/{}",
                encode(&h.url.owner),
                encode(&h.url.name)
            ),
            web,
            project: h.url.owner.clone(),
            name: h.url.name.clone(),
        }
    }

    fn pr(&self, n: u64) -> String {
        format!("{}/pull-requests/{n}", self.repo)
    }

    /// `values` of every page (`isLastPage` / `nextPageStart`), up to `cap`.
    fn values(&self, url: &str, cap: usize) -> Result<Vec<Value>> {
        let sep = if url.contains('?') { '&' } else { '?' };
        let base = url.to_string();
        self.api.paged(
            url,
            cap,
            |v| arr(v, "values").to_vec(),
            move |_, v| {
                (!b(v, "isLastPage"))
                    .then(|| u(v, "nextPageStart"))
                    .flatten()
                    .map(|start| format!("{base}{sep}start={start}"))
            },
        )
    }

    /// Current version of the request (optimistic locking for merge/decline/reopen).
    fn version(&self, n: u64) -> Result<u64> {
        let v = self.api.get(&self.pr(n))?;
        u(&v, "version").ok_or_else(|| anyhow!("the server did not report the request's version"))
    }

    fn whoami(&self) -> Result<String> {
        let who = self
            .api
            .get_text(&format!("{}/plugins/servlet/applinks/whoami", self.web))?;
        let who = who.trim().to_string();
        if who.is_empty() {
            return Err(anyhow!(
                "Bitbucket Server did not report the signed-in user"
            ));
        }
        Ok(who)
    }
}

fn user(v: &Value, path: &str) -> String {
    let u = super::json::get(v, path).cloned().unwrap_or_default();
    first(&u, &["name", "slug", "displayName"])
}

pub fn parse_pr(v: &Value) -> PullRequest {
    let state = match s(v, "state").as_str() {
        "MERGED" => PrState::Merged,
        "DECLINED" => PrState::Closed,
        _ => PrState::Open,
    };
    let from_repo = format!(
        "{}/{}",
        s(v, "fromRef.repository.project.key"),
        s(v, "fromRef.repository.slug")
    );
    let to_repo = format!(
        "{}/{}",
        s(v, "toRef.repository.project.key"),
        s(v, "toRef.repository.slug")
    );
    let clone = arr(v, "fromRef.repository.links.clone")
        .iter()
        .find(|l| s(l, "name") == "http")
        .map(|l| s(l, "href"));
    PullRequest {
        number: u(v, "id").unwrap_or(0),
        title: s(v, "title"),
        author: user(v, "author.user"),
        head: s(v, "fromRef.displayId"),
        base: s(v, "toRef.displayId"),
        state,
        draft: b(v, "draft"),
        updated: time(v, "updatedDate"),
        review: None,
        url: s(v, "links.self.0.href"),
        labels: Vec::new(),
        source_repo: (from_repo != "/" && !from_repo.eq_ignore_ascii_case(&to_repo))
            .then_some(from_repo),
        source_clone_url: clone,
        head_sha: Some(s(v, "fromRef.latestCommit")).filter(|x| !x.is_empty()),
    }
}

pub fn parse_details(
    pull: &Value,
    activities: &[Value],
    builds: &[Value],
    changes: &[Value],
    merge: &Value,
) -> PrDetails {
    let pr = parse_pr(pull);
    let mut checks = Checks::default();
    for st in builds {
        checks.push(first(st, &["name", "key"]), &s(st, "state"));
    }
    let reviews = arr(pull, "reviewers")
        .iter()
        .map(|r| Review {
            author: user(r, "user"),
            state: match s(r, "status").as_str() {
                "APPROVED" => "APPROVED",
                "NEEDS_WORK" => "CHANGES_REQUESTED",
                _ => "REQUESTED",
            }
            .into(),
            body: String::new(),
            submitted: None,
        })
        .collect();
    let mut comments: Vec<Comment> = activities
        .iter()
        .filter(|a| s(a, "action") == "COMMENTED")
        .map(|a| Comment {
            author: user(a, "comment.author"),
            body: normalize_text(&s(a, "comment.text")),
            created: time(a, "comment.createdDate"),
        })
        .collect();
    comments.reverse(); // activities are newest first
    let mergeable = (pr.state == PrState::Open && merge.is_object()).then(|| {
        if b(merge, "conflicted") {
            "conflicting".to_string()
        } else if b(merge, "canMerge") {
            "mergeable".to_string()
        } else {
            let veto = s(merge, "vetoes.0.summaryMessage");
            if veto.is_empty() {
                "blocked".to_string()
            } else {
                format!("blocked: {veto}")
            }
        }
    });
    let mut d = PrDetails {
        body: normalize_text(&s(pull, "description")),
        comments,
        reviews,
        checks,
        files: changes
            .iter()
            .map(|c| ChangedFile {
                path: s(c, "path.toString"),
                additions: None,
                deletions: None,
            })
            .collect(),
        mergeable,
        ..Default::default()
    };
    d.pr = pr;
    d.fill_totals();
    d
}

impl PrProvider for BitbucketServer {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let state = match filter {
            StateFilter::Open => "OPEN",
            StateFilter::Closed => "DECLINED",
            StateFilter::Merged => "MERGED",
            StateFilter::All => "ALL",
        };
        let url = format!(
            "{}/pull-requests?state={state}&order=NEWEST&limit=50",
            self.repo
        );
        Ok(self.values(&url, MAX_ITEMS)?.iter().map(parse_pr).collect())
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let pull = self.api.get(&self.pr(number))?;
        let activities = self
            .values(
                &format!("{}/activities?limit=100", self.pr(number)),
                MAX_ITEMS,
            )
            .unwrap_or_default();
        let changes = self
            .values(&format!("{}/changes?limit=100", self.pr(number)), MAX_ITEMS)
            .unwrap_or_default();
        let sha = s(&pull, "fromRef.latestCommit");
        let builds = if sha.is_empty() {
            Vec::new()
        } else {
            self.values(
                &format!("{}/rest/build-status/1.0/commits/{sha}", self.web),
                50,
            )
            .unwrap_or_default()
        };
        let merge = if s(&pull, "state") == "OPEN" {
            self.api
                .get(&format!("{}/merge", self.pr(number)))
                .unwrap_or_default()
        } else {
            Value::Null
        };
        Ok(parse_details(&pull, &activities, &builds, &changes, &merge))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        let repo_ref =
            |project: &str, slug: &str| json!({ "slug": slug, "project": { "key": project } });
        let from_repo = match &pr.head_repo {
            Some(fork) => repo_ref(&fork.owner, &fork.name),
            None => repo_ref(&self.project, &self.name),
        };
        let v = self.api.send(
            "POST",
            &format!("{}/pull-requests", self.repo),
            &json!({
                "title": pr.title,
                "description": pr.body,
                "draft": pr.draft,
                "fromRef": { "id": format!("refs/heads/{}", pr.head), "repository": from_repo },
                "toRef": { "id": format!("refs/heads/{}", pr.base),
                           "repository": repo_ref(&self.project, &self.name) },
            }),
        )?;
        Ok(Created {
            number: u(&v, "id").unwrap_or(0),
            url: s(&v, "links.self.0.href"),
        })
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(need_text("A comment"));
        }
        self.api.send(
            "POST",
            &format!("{}/comments", self.pr(number)),
            &json!({ "text": body }),
        )?;
        Ok(())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String> {
        let status = match kind {
            ReviewKind::Approve => Some("APPROVED"),
            ReviewKind::RequestChanges => Some("NEEDS_WORK"),
            ReviewKind::Comment => None,
        };
        if let Some(status) = status {
            let me = self.whoami()?;
            self.api.send(
                "PUT",
                &format!("{}/participants/{}", self.pr(number), encode(&me)),
                &json!({ "status": status }),
            )?;
        }
        if !body.trim().is_empty() || kind == ReviewKind::Comment {
            self.comment(number, body)?;
        }
        Ok(match kind {
            ReviewKind::RequestChanges => "Marked as needs work".into(),
            k => format!("{} submitted", k.label()),
        })
    }

    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()> {
        let version = self.version(pr.number)?;
        let strategy = match method {
            MergeMethod::Merge => "no-ff",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase-no-ff",
        };
        self.api.send(
            "POST",
            &format!("{}/merge?version={version}", self.pr(pr.number)),
            &json!({ "strategyId": strategy }),
        )?;
        if delete_branch && pr.source_repo.is_none() && !pr.head.is_empty() {
            self.api.send(
                "DELETE",
                &format!(
                    "{}/rest/branch-utils/1.0/projects/{}/repos/{}/branches",
                    self.web,
                    encode(&self.project),
                    encode(&self.name)
                ),
                &json!({ "name": format!("refs/heads/{}", pr.head), "dryRun": false }),
            )?;
        }
        Ok(())
    }

    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()> {
        let version = self.version(pr.number)?;
        let action = if reopen { "reopen" } else { "decline" };
        self.api.send(
            "POST",
            &format!("{}/{action}?version={version}", self.pr(pr.number)),
            &json!({}),
        )?;
        Ok(())
    }

    fn default_branch(&self) -> Result<String> {
        let v = self
            .api
            .get(&format!("{}/default-branch", self.repo))
            .or_else(|_| self.api.get(&format!("{}/branches/default", self.repo)))?;
        Ok(s(&v, "displayId"))
    }

    fn current_user(&self) -> Result<String> {
        self.whoami()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{
        client, parse_remote_url, CheckOutcome, Credentials, DetectedBy, ProviderKind,
    };

    const PR: &str = r#"{"id":8,"version":3,"title":"Feature","description":"Text","state":"OPEN","open":true,
      "author":{"user":{"name":"alice","displayName":"Alice"}},
      "fromRef":{"id":"refs/heads/feat","displayId":"feat","latestCommit":"c0ffee",
                 "repository":{"slug":"repo","project":{"key":"PROJ"},
                   "links":{"clone":[{"href":"ssh://git@bb.corp:7999/proj/repo.git","name":"ssh"},
                                     {"href":"https://bb.corp/scm/proj/repo.git","name":"http"}]}}},
      "toRef":{"id":"refs/heads/main","displayId":"main","repository":{"slug":"repo","project":{"key":"PROJ"}}},
      "updatedDate":1790812800000,
      "reviewers":[{"user":{"name":"bob"},"status":"APPROVED"},{"user":{"name":"cy"},"status":"UNAPPROVED"}],
      "links":{"self":[{"href":"https://bb.corp/projects/PROJ/repos/repo/pull-requests/8"}]}}"#;

    #[test]
    fn parses_list_and_details() {
        let v: Value = serde_json::from_str(PR).unwrap();
        let p = parse_pr(&v);
        assert_eq!(
            (p.number, p.head.as_str(), p.base.as_str()),
            (8, "feat", "main")
        );
        assert_eq!(p.updated, Some(1_790_812_800));
        assert!(p.source_repo.is_none());
        assert_eq!(
            p.source_clone_url.as_deref(),
            Some("https://bb.corp/scm/proj/repo.git")
        );
        let activities: Vec<Value> = serde_json::from_str(
            r#"[{"action":"COMMENTED","comment":{"text":"second","author":{"name":"bob"},"createdDate":1790812900000}},
                {"action":"APPROVED","user":{"name":"bob"}},
                {"action":"COMMENTED","comment":{"text":"first","author":{"name":"cy"}}}]"#,
        )
        .unwrap();
        let builds: Vec<Value> = serde_json::from_str(
            r#"[{"state":"FAILED","key":"ci","name":"CI build"},{"state":"SUCCESSFUL","key":"lint"}]"#,
        )
        .unwrap();
        let changes: Vec<Value> =
            serde_json::from_str(r#"[{"path":{"toString":"src/A.java"}}]"#).unwrap();
        let merge: Value = serde_json::from_str(
            r#"{"canMerge":false,"conflicted":false,"vetoes":[{"summaryMessage":"Needs 2 approvals"}]}"#,
        )
        .unwrap();
        let d = parse_details(&v, &activities, &builds, &changes, &merge);
        assert_eq!(d.comments[0].body, "first");
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.pr.review, Some(super::super::ReviewDecision::Approved));
        assert_eq!(d.checks.count(CheckOutcome::Failure), 1);
        assert_eq!(d.mergeable.as_deref(), Some("blocked: Needs 2 approvals"));
        assert_eq!(
            (d.files[0].path.as_str(), d.files[0].additions),
            ("src/A.java", None)
        );
        assert_eq!(d.changed_files, Some(1));
        assert_eq!(d.additions, None);
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        let page = format!(r#"{{"values":[{PR}],"isLastPage":false,"nextPageStart":25}}"#);
        t.on("GET", "/pull-requests?state=OPEN", 200, &page)
            .on(
                "GET",
                "/pull-requests?state=OPEN&order=NEWEST&limit=50&start=25",
                200,
                r#"{"values":[],"isLastPage":true}"#,
            )
            .on("GET", "/pull-requests/8", 200, PR)
            .on("GET", "/whoami", 200, "alice\n")
            .on("PUT", "/participants/alice", 200, "{}")
            .on("POST", "/merge?version=3", 200, "{}")
            .on(
                "DELETE",
                "/branch-utils/1.0/projects/PROJ/repos/repo/branches",
                204,
                "",
            )
            .on("POST", "/decline?version=3", 200, "{}");
        let url = "https://bb.corp/scm/PROJ/repo.git";
        let h = Hosting::new(
            ProviderKind::BitbucketServer,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::Probe,
        );
        let c = Credentials {
            username: None,
            secret: "httptoken".into(),
            source: "t".into(),
        };
        let api = client(&h, Some(&c), t.clone()).unwrap();
        assert_eq!(api.list(StateFilter::Open).unwrap().len(), 1);
        assert_eq!(
            t.log()[0].url,
            "https://bb.corp/rest/api/1.0/projects/PROJ/repos/repo/pull-requests?state=OPEN&order=NEWEST&limit=50"
        );
        assert_eq!(t.log()[0].header("authorization"), Some("Bearer httptoken"));
        assert_eq!(t.log().len(), 2);
        api.review(8, ReviewKind::RequestChanges, "").unwrap();
        let r = t.last("PUT");
        assert!(r.url.ends_with("/pull-requests/8/participants/alice"));
        assert_eq!(r.body.as_deref(), Some(r#"{"status":"NEEDS_WORK"}"#));
        let pr = parse_pr(&serde_json::from_str(PR).unwrap());
        api.merge(&pr, MergeMethod::Squash, true).unwrap();
        let log = t.log();
        let merge = log
            .iter()
            .find(|r| r.url.contains("/merge?version=3"))
            .unwrap();
        assert_eq!(merge.body.as_deref(), Some(r#"{"strategyId":"squash"}"#));
        let del = t.last("DELETE");
        assert_eq!(
            del.url,
            "https://bb.corp/rest/branch-utils/1.0/projects/PROJ/repos/repo/branches"
        );
        assert_eq!(
            del.body.as_deref(),
            Some(r#"{"dryRun":false,"name":"refs/heads/feat"}"#)
        );
        api.set_state(&pr, false).unwrap();
        assert!(t
            .last("POST")
            .url
            .ends_with("/pull-requests/8/decline?version=3"));
    }
}
