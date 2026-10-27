//! GitHub REST API v3 (github.com and GitHub Enterprise Server at `https://<host>/api/v3`).

use anyhow::Result;
use serde_json::{json, Value};

use super::http::{encode, encode_path, Api, MAX_ITEMS};
use super::json::{self as j, arr, b, labels, normalize_text, s, time, u};
use super::{
    need_text, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr, PrDetails,
    PrProvider, PrState, PullRequest, Review, ReviewKind, StateFilter,
};

pub struct GitHub {
    api: Api,
    base: String,
    /// `/repos/owner/name`
    repo: String,
    slug: String,
}

impl GitHub {
    pub fn new(mut api: Api, base: String, h: &Hosting) -> Self {
        api.headers = vec![
            ("Accept".into(), "application/vnd.github+json".into()),
            ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
        ];
        Self {
            api,
            repo: format!(
                "{base}/repos/{}/{}",
                encode(&h.url.owner),
                encode(&h.url.name)
            ),
            base,
            slug: h.slug(),
        }
    }
}

pub fn parse_pr(v: &Value, slug: &str) -> PullRequest {
    let merged = j::get(v, "merged_at").is_some() || b(v, "merged");
    let state = if merged {
        PrState::Merged
    } else if s(v, "state").eq_ignore_ascii_case("closed") {
        PrState::Closed
    } else {
        PrState::Open
    };
    let source = s(v, "head.repo.full_name");
    PullRequest {
        number: u(v, "number").unwrap_or(0),
        title: s(v, "title"),
        author: s(v, "user.login"),
        head: s(v, "head.ref"),
        base: s(v, "base.ref"),
        state,
        draft: b(v, "draft"),
        updated: time(v, "updated_at"),
        review: None,
        url: s(v, "html_url"),
        labels: labels(v, "labels"),
        source_repo: (!source.is_empty() && !source.eq_ignore_ascii_case(slug)).then_some(source),
        source_clone_url: Some(s(v, "head.repo.clone_url")).filter(|x| !x.is_empty()),
        head_sha: Some(s(v, "head.sha")).filter(|x| !x.is_empty()),
    }
}

pub fn parse_list(items: &[Value], slug: &str, filter: StateFilter) -> Vec<PullRequest> {
    items
        .iter()
        .map(|v| parse_pr(v, slug))
        .filter(|p| filter.accepts(p.state))
        .collect()
}

/// Assemble details from the pull, its issue comments, reviews, check runs, commit statuses and files.
pub fn parse_details(
    pull: &Value,
    slug: &str,
    comments: &[Value],
    reviews: &[Value],
    check_runs: &Value,
    status: &Value,
    files: &[Value],
) -> PrDetails {
    let pr = parse_pr(pull, slug);
    let mut checks = Checks::default();
    for c in arr(check_runs, "check_runs") {
        let outcome = if s(c, "status") == "completed" {
            s(c, "conclusion")
        } else {
            "pending".into()
        };
        checks.push(s(c, "name"), &outcome);
    }
    for st in arr(status, "statuses") {
        checks.push(s(st, "context"), &s(st, "state"));
    }
    let mergeable = match j::opt_bool(pull, "mergeable") {
        _ if pr.state != PrState::Open => None,
        Some(true) => Some(match s(pull, "mergeable_state").as_str() {
            "blocked" => "blocked".to_string(),
            "unstable" => "mergeable (checks failing)".to_string(),
            _ => "mergeable".to_string(),
        }),
        Some(false) => Some("conflicting".to_string()),
        None => None,
    };
    let mut d = PrDetails {
        body: normalize_text(&s(pull, "body")),
        comments: comments
            .iter()
            .map(|c| Comment {
                author: s(c, "user.login"),
                body: normalize_text(&s(c, "body")),
                created: time(c, "created_at"),
            })
            .collect(),
        reviews: reviews
            .iter()
            .filter(|r| s(r, "state") != "PENDING")
            .map(|r| Review {
                author: s(r, "user.login"),
                state: s(r, "state"),
                body: normalize_text(&s(r, "body")),
                submitted: time(r, "submitted_at"),
            })
            .collect(),
        checks,
        files: files
            .iter()
            .map(|f| ChangedFile {
                path: s(f, "filename"),
                additions: u(f, "additions"),
                deletions: u(f, "deletions"),
            })
            .collect(),
        additions: u(pull, "additions"),
        deletions: u(pull, "deletions"),
        mergeable,
        changed_files: u(pull, "changed_files"),
        pr,
    };
    d.fill_totals();
    d
}

impl PrProvider for GitHub {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let state = match filter {
            StateFilter::Open => "open",
            StateFilter::Closed | StateFilter::Merged => "closed",
            StateFilter::All => "all",
        };
        let url = format!(
            "{}/pulls?state={state}&sort=updated&direction=desc&per_page=50",
            self.repo
        );
        let items = self.api.paged_array(&url, MAX_ITEMS)?;
        Ok(parse_list(&items, &self.slug, filter))
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let pull = self.api.get(&format!("{}/pulls/{number}", self.repo))?;
        let sha = s(&pull, "head.sha");
        let list = |path: &str| {
            self.api
                .paged_array(&format!("{}/{path}", self.repo), MAX_ITEMS)
                .unwrap_or_default()
        };
        let comments = list(&format!("issues/{number}/comments?per_page=100"));
        let reviews = list(&format!("pulls/{number}/reviews?per_page=100"));
        let files = list(&format!("pulls/{number}/files?per_page=100"));
        let (runs, status) = if sha.is_empty() {
            (Value::Null, Value::Null)
        } else {
            let get = |p: String| {
                self.api
                    .get(&format!("{}/{p}", self.repo))
                    .unwrap_or_default()
            };
            (
                get(format!("commits/{sha}/check-runs?per_page=100")),
                get(format!("commits/{sha}/status")),
            )
        };
        Ok(parse_details(
            &pull, &self.slug, &comments, &reviews, &runs, &status, &files,
        ))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        let head = match &pr.head_repo {
            Some(fork) => format!("{}:{}", fork.owner, pr.head),
            None => pr.head.clone(),
        };
        let v = self.api.send(
            "POST",
            &format!("{}/pulls", self.repo),
            &json!({"title": pr.title, "body": pr.body, "head": head, "base": pr.base, "draft": pr.draft}),
        )?;
        Ok(Created {
            number: u(&v, "number").unwrap_or(0),
            url: s(&v, "html_url"),
        })
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(need_text("A comment"));
        }
        self.api.send(
            "POST",
            &format!("{}/issues/{number}/comments", self.repo),
            &json!({ "body": body }),
        )?;
        Ok(())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String> {
        let event = match kind {
            ReviewKind::Approve => "APPROVE",
            ReviewKind::RequestChanges => "REQUEST_CHANGES",
            ReviewKind::Comment => "COMMENT",
        };
        if kind != ReviewKind::Approve && body.trim().is_empty() {
            return Err(need_text(&format!("A \"{}\" review", kind.label())));
        }
        let mut payload = json!({ "event": event });
        if !body.trim().is_empty() {
            payload["body"] = json!(body);
        }
        self.api.send(
            "POST",
            &format!("{}/pulls/{number}/reviews", self.repo),
            &payload,
        )?;
        Ok(format!("{} review submitted", kind.label()))
    }

    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()> {
        let m = match method {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        };
        self.api.send(
            "PUT",
            &format!("{}/pulls/{}/merge", self.repo, pr.number),
            &json!({ "merge_method": m }),
        )?;
        if delete_branch && !pr.head.is_empty() {
            let repo = match &pr.source_repo {
                Some(slug) => format!("{}/repos/{}", self.base, encode_path(slug)),
                None => self.repo.clone(),
            };
            self.api.call(
                "DELETE",
                &format!("{repo}/git/refs/heads/{}", encode_path(&pr.head)),
            )?;
        }
        Ok(())
    }

    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()> {
        self.api.send(
            "PATCH",
            &format!("{}/pulls/{}", self.repo, pr.number),
            &json!({ "state": if reopen { "open" } else { "closed" } }),
        )?;
        Ok(())
    }

    fn default_branch(&self) -> Result<String> {
        Ok(s(&self.api.get(&self.repo)?, "default_branch"))
    }

    fn current_user(&self) -> Result<String> {
        Ok(s(&self.api.get(&format!("{}/user", self.base))?, "login"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{
        client, parse_remote_url, Credentials, DetectedBy, ProviderKind, ReviewDecision,
    };

    const PULLS: &str = r#"[
      {"number":42,"title":"Fix the thing","user":{"login":"alice"},"state":"open","draft":false,
       "head":{"ref":"feature/x","sha":"abc123","repo":{"full_name":"alice/hello","clone_url":"https://github.com/alice/hello.git"}},
       "base":{"ref":"main","repo":{"full_name":"octo/hello"}},
       "updated_at":"2026-10-20T10:00:00Z","html_url":"https://github.com/octo/hello/pull/42",
       "labels":[{"name":"bug"}],"merged_at":null},
      {"number":7,"title":"Ünïcode","user":{"login":"bob"},"state":"closed","draft":true,
       "head":{"ref":"wip","repo":{"full_name":"octo/hello"}},"base":{"ref":"main"},
       "updated_at":null,"html_url":"https://github.com/octo/hello/pull/7","labels":[],
       "merged_at":"2026-10-01T00:00:00Z"},
      {"number":3,"title":"Nope","user":{"login":"eve"},"state":"closed","head":{"ref":"x","repo":null},
       "base":{"ref":"main"},"merged_at":null}
    ]"#;

    #[test]
    fn parses_list() {
        let items: Vec<Value> = serde_json::from_str(PULLS).unwrap();
        let all = parse_list(&items, "octo/hello", StateFilter::All);
        assert_eq!(all.len(), 3);
        let p = &all[0];
        assert_eq!(
            (p.number, p.author.as_str(), p.head.as_str()),
            (42, "alice", "feature/x")
        );
        assert_eq!(p.source_repo.as_deref(), Some("alice/hello"));
        assert_eq!(p.head_sha.as_deref(), Some("abc123"));
        assert_eq!(p.labels, ["bug"]);
        assert_eq!(all[1].state, PrState::Merged);
        assert!(all[1].draft && all[1].source_repo.is_none());
        assert_eq!(all[2].state, PrState::Closed);
        assert_eq!(
            parse_list(&items, "octo/hello", StateFilter::Merged).len(),
            1
        );
        assert_eq!(
            parse_list(&items, "octo/hello", StateFilter::Closed)[0].number,
            3
        );
    }

    #[test]
    fn parses_details() {
        let pull: Value = serde_json::from_str(
            r#"{"number":5,"title":"Add thing","body":"Why\r\nthis","user":{"login":"al"},"state":"open",
                "head":{"ref":"feat","sha":"s1"},"base":{"ref":"main"},"additions":3,"deletions":1,
                "changed_files":1,"mergeable":false,"mergeable_state":"dirty"}"#,
        )
        .unwrap();
        let comments: Vec<Value> = serde_json::from_str(
            r#"[{"user":{"login":"bo"},"body":"Nice","created_at":"2026-10-01T00:00:00Z"}]"#,
        )
        .unwrap();
        let reviews: Vec<Value> = serde_json::from_str(
            r#"[{"user":{"login":"cy"},"state":"CHANGES_REQUESTED","body":"fix"},
                {"user":{"login":"cy"},"state":"APPROVED","body":""},
                {"user":{"login":"zz"},"state":"PENDING","body":""}]"#,
        )
        .unwrap();
        let runs: Value = serde_json::from_str(
            r#"{"check_runs":[{"name":"ci","status":"completed","conclusion":"failure"},
                              {"name":"lint","status":"in_progress","conclusion":null}]}"#,
        )
        .unwrap();
        let status: Value =
            serde_json::from_str(r#"{"statuses":[{"context":"deploy","state":"success"}]}"#)
                .unwrap();
        let files: Vec<Value> =
            serde_json::from_str(r#"[{"filename":"a.rs","additions":3,"deletions":1}]"#).unwrap();
        let d = parse_details(&pull, "o/r", &comments, &reviews, &runs, &status, &files);
        assert_eq!(d.body, "Why\nthis");
        assert_eq!(d.comments[0].author, "bo");
        assert_eq!(d.reviews.len(), 2);
        assert_eq!(d.pr.review, Some(ReviewDecision::Approved));
        assert_eq!(d.checks.count(super::super::CheckOutcome::Failure), 1);
        assert_eq!(d.checks.count(super::super::CheckOutcome::Pending), 1);
        assert_eq!(d.checks.count(super::super::CheckOutcome::Success), 1);
        assert_eq!(d.mergeable.as_deref(), Some("conflicting"));
        assert_eq!(
            (d.additions, d.deletions, d.changed_files),
            (Some(3), Some(1), Some(1))
        );
        assert_eq!(d.files[0].path, "a.rs");
    }

    fn github(t: std::sync::Arc<FakeTransport>, url: &str) -> Box<dyn PrProvider> {
        let h = Hosting::new(
            ProviderKind::GitHub,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::KnownHost,
        );
        let c = Credentials {
            username: None,
            secret: "ghp_t".into(),
            source: "test".into(),
        };
        client(&h, Some(&c), t).unwrap()
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        t.on("GET", "/repos/octo/hello/pulls?state=closed", 200, PULLS)
            .on(
                "POST",
                "/pulls",
                201,
                r#"{"number":9,"html_url":"https://github.com/octo/hello/pull/9"}"#,
            )
            .on("POST", "/comments", 201, "{}")
            .on("POST", "/reviews", 200, "{}")
            .on("PUT", "/merge", 200, r#"{"merged":true}"#)
            .on("DELETE", "/git/refs/heads/", 204, "")
            .on("PATCH", "/pulls/4", 200, "{}")
            .on("GET", "/user", 200, r#"{"login":"me"}"#);
        let api = github(t.clone(), "git@github.com:octo/hello.git");
        assert_eq!(api.list(StateFilter::Merged).unwrap().len(), 1);
        let r = t.last("GET");
        assert!(r
            .url
            .starts_with("https://api.github.com/repos/octo/hello/pulls?state=closed"));
        assert_eq!(r.header("authorization"), Some("Bearer ghp_t"));
        assert_eq!(r.header("x-github-api-version"), Some("2022-11-28"));

        let fork = parse_remote_url("git@github.com:me/hello.git").unwrap();
        let created = api
            .create(&NewPr {
                title: "T".into(),
                body: "B".into(),
                base: "main".into(),
                head: "feat".into(),
                head_repo: Some(fork),
                draft: true,
            })
            .unwrap();
        assert_eq!(created.number, 9);
        let r = t.last("POST");
        assert_eq!(r.url, "https://api.github.com/repos/octo/hello/pulls");
        let body: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"title":"T","body":"B","head":"me:feat","base":"main","draft":true})
        );

        api.comment(4, "hi").unwrap();
        assert_eq!(
            t.last("POST").url,
            "https://api.github.com/repos/octo/hello/issues/4/comments"
        );
        assert!(api.comment(4, "  ").is_err());
        api.review(4, ReviewKind::RequestChanges, "please fix")
            .unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({"event":"REQUEST_CHANGES","body":"please fix"}));
        api.review(4, ReviewKind::Approve, "").unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({"event":"APPROVE"}));

        let pr = PullRequest {
            number: 4,
            head: "feat/x".into(),
            source_repo: Some("me/hello".into()),
            ..Default::default()
        };
        api.merge(&pr, MergeMethod::Squash, true).unwrap();
        let body: Value = serde_json::from_str(t.last("PUT").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["merge_method"], "squash");
        assert_eq!(
            t.last("DELETE").url,
            "https://api.github.com/repos/me/hello/git/refs/heads/feat/x"
        );
        api.set_state(&pr, false).unwrap();
        assert_eq!(
            t.last("PATCH").body.as_deref(),
            Some(r#"{"state":"closed"}"#)
        );
        assert_eq!(api.current_user().unwrap(), "me");

        let ghe = github(t.clone(), "https://ghe.corp.com/team/app.git");
        let _ = ghe.default_branch();
        assert_eq!(
            t.last("GET").url,
            "https://ghe.corp.com/api/v3/repos/team/app"
        );
    }
}
