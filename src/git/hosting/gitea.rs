//! Gitea / Forgejo / Codeberg API v1.

use anyhow::Result;
use serde_json::{json, Value};

use super::http::{encode, Api, MAX_ITEMS};
use super::json::{self as j, arr, b, labels, normalize_text, s, time, u};
use super::{
    need_text, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr, PrDetails,
    PrProvider, PrState, PullRequest, Review, ReviewKind, StateFilter,
};

pub struct Gitea {
    api: Api,
    base: String,
    repo: String,
    slug: String,
}

impl Gitea {
    pub fn new(api: Api, base: String, h: &Hosting) -> Self {
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

fn is_wip(title: &str) -> bool {
    let t = title.trim_start().to_ascii_lowercase();
    ["wip:", "[wip]", "draft:", "[draft]"]
        .iter()
        .any(|p| t.starts_with(p))
}

pub fn parse_pr(v: &Value, slug: &str) -> PullRequest {
    let state = if b(v, "merged") {
        PrState::Merged
    } else if s(v, "state") == "closed" {
        PrState::Closed
    } else {
        PrState::Open
    };
    let title = s(v, "title");
    let source = s(v, "head.repo.full_name");
    PullRequest {
        number: u(v, "number").unwrap_or(0),
        author: s(v, "user.login"),
        head: s(v, "head.ref"),
        base: s(v, "base.ref"),
        state,
        draft: b(v, "draft") || is_wip(&title),
        title,
        updated: time(v, "updated_at"),
        review: None,
        url: s(v, "html_url"),
        labels: labels(v, "labels"),
        source_repo: (!source.is_empty() && !source.eq_ignore_ascii_case(slug)).then_some(source),
        source_clone_url: Some(s(v, "head.repo.clone_url")).filter(|x| !x.is_empty()),
        head_sha: Some(s(v, "head.sha")).filter(|x| !x.is_empty()),
    }
}

/// Gitea review states → common ones.
fn review_state(s: &str) -> String {
    match s {
        "APPROVED" => "APPROVED",
        "REQUEST_CHANGES" => "CHANGES_REQUESTED",
        "REQUEST_REVIEW" => "REQUESTED",
        "PENDING" => "PENDING",
        _ => "COMMENTED",
    }
    .to_string()
}

pub fn parse_details(
    pull: &Value,
    slug: &str,
    comments: &[Value],
    reviews: &[Value],
    status: &Value,
    files: &[Value],
) -> PrDetails {
    let pr = parse_pr(pull, slug);
    let mut checks = Checks::default();
    for st in arr(status, "statuses") {
        checks.push(s(st, "context"), &j::first(st, &["status", "state"]));
    }
    let mergeable = (pr.state == PrState::Open)
        .then(|| j::opt_bool(pull, "mergeable"))
        .flatten()
        .map(|m| if m { "mergeable" } else { "conflicting" }.to_string());
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
            .map(|r| Review {
                author: s(r, "user.login"),
                state: review_state(&s(r, "state")),
                body: normalize_text(&s(r, "body")),
                submitted: time(r, "submitted_at"),
            })
            .filter(|r| r.state != "PENDING")
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

impl PrProvider for Gitea {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let state = match filter {
            StateFilter::Open => "open",
            StateFilter::Closed | StateFilter::Merged => "closed",
            StateFilter::All => "all",
        };
        let url = format!(
            "{}/pulls?state={state}&sort=recentupdate&limit=50",
            self.repo
        );
        let items = self.api.paged_array(&url, MAX_ITEMS)?;
        Ok(items
            .iter()
            .map(|v| parse_pr(v, &self.slug))
            .filter(|p| filter.accepts(p.state))
            .collect())
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let pull = self.api.get(&format!("{}/pulls/{number}", self.repo))?;
        let list = |p: String| self.api.paged_array(&p, MAX_ITEMS).unwrap_or_default();
        let comments = list(format!("{}/issues/{number}/comments?limit=50", self.repo));
        let reviews = list(format!("{}/pulls/{number}/reviews?limit=50", self.repo));
        let files = list(format!("{}/pulls/{number}/files?limit=50", self.repo));
        let sha = s(&pull, "head.sha");
        let status = if sha.is_empty() {
            Value::Null
        } else {
            self.api
                .get(&format!("{}/commits/{sha}/status", self.repo))
                .unwrap_or_default()
        };
        Ok(parse_details(
            &pull, &self.slug, &comments, &reviews, &status, &files,
        ))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        let head = match &pr.head_repo {
            Some(fork) => format!("{}:{}", fork.owner, pr.head),
            None => pr.head.clone(),
        };
        let title = if pr.draft && !is_wip(&pr.title) {
            format!("WIP: {}", pr.title)
        } else {
            pr.title.clone()
        };
        let v = self.api.send(
            "POST",
            &format!("{}/pulls", self.repo),
            &json!({"title": title, "body": pr.body, "head": head, "base": pr.base}),
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
            ReviewKind::Approve => "APPROVED",
            ReviewKind::RequestChanges => "REQUEST_CHANGES",
            ReviewKind::Comment => "COMMENT",
        };
        if kind != ReviewKind::Approve && body.trim().is_empty() {
            return Err(need_text(&format!("A \"{}\" review", kind.label())));
        }
        self.api.send(
            "POST",
            &format!("{}/pulls/{number}/reviews", self.repo),
            &json!({ "event": event, "body": body }),
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
            "POST",
            &format!("{}/pulls/{}/merge", self.repo, pr.number),
            &json!({ "Do": m, "delete_branch_after_merge": delete_branch }),
        )?;
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
        client, parse_remote_url, CheckOutcome, Credentials, DetectedBy, ProviderKind,
    };

    #[test]
    fn parses_list_and_details() {
        let items: Vec<Value> = serde_json::from_str(
            r#"[{"number":5,"title":"WIP: thing","user":{"login":"al"},"state":"open","merged":false,
                 "head":{"ref":"t","sha":"s5","repo":{"full_name":"al/b","clone_url":"https://codeberg.org/al/b.git"}},
                 "base":{"ref":"main"},"updated_at":"2026-10-01T00:00:00+02:00","html_url":"https://codeberg.org/a/b/pulls/5",
                 "labels":[{"name":"enhancement"}],"mergeable":true,"body":"Body","additions":4,"deletions":2},
                {"number":4,"title":"Done","user":{"login":"bo"},"state":"closed","merged":true,
                 "head":{"ref":"d","repo":{"full_name":"a/b"}},"base":{"ref":"main"}}]"#,
        )
        .unwrap();
        let p = parse_pr(&items[0], "a/b");
        assert!(p.draft);
        assert_eq!(p.source_repo.as_deref(), Some("al/b"));
        assert_eq!(parse_pr(&items[1], "a/b").state, PrState::Merged);
        let reviews: Vec<Value> = serde_json::from_str(
            r#"[{"user":{"login":"cy"},"state":"REQUEST_CHANGES","body":"no"},
                {"user":{"login":"dd"},"state":"COMMENT","body":"hm"}]"#,
        )
        .unwrap();
        let status: Value = serde_json::from_str(
            r#"{"statuses":[{"context":"ci/woodpecker","status":"success"},{"context":"lint","status":"pending"}]}"#,
        )
        .unwrap();
        let comments: Vec<Value> =
            serde_json::from_str(r#"[{"user":{"login":"ee"},"body":"thanks"}]"#).unwrap();
        let files: Vec<Value> =
            serde_json::from_str(r#"[{"filename":"x.go","additions":4,"deletions":2}]"#).unwrap();
        let d = parse_details(&items[0], "a/b", &comments, &reviews, &status, &files);
        assert_eq!(
            d.pr.review,
            Some(super::super::ReviewDecision::ChangesRequested)
        );
        assert_eq!(d.reviews[1].state, "COMMENTED");
        assert_eq!(d.checks.count(CheckOutcome::Success), 1);
        assert_eq!(d.checks.count(CheckOutcome::Pending), 1);
        assert_eq!(d.mergeable.as_deref(), Some("mergeable"));
        assert_eq!((d.additions, d.changed_files), (Some(4), Some(1)));
        assert_eq!(d.comments[0].body, "thanks");
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        t.on(
            "POST",
            "/pulls",
            201,
            r#"{"number":6,"html_url":"https://codeberg.org/a/b/pulls/6"}"#,
        )
        .on("POST", "/reviews", 200, "{}")
        .on("POST", "/merge", 200, "")
        .on("PATCH", "/pulls/6", 201, "{}");
        let url = "https://codeberg.org/a/b.git";
        let h = Hosting::new(
            ProviderKind::Gitea,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::KnownHost,
        );
        let tok = "0123456789abcdef0123456789abcdef01234567";
        let c = Credentials {
            username: None,
            secret: tok.into(),
            source: "t".into(),
        };
        let api = client(&h, Some(&c), t.clone()).unwrap();
        api.create(&NewPr {
            title: "T".into(),
            body: "".into(),
            base: "main".into(),
            head: "f".into(),
            head_repo: None,
            draft: true,
        })
        .unwrap();
        let r = t.last("POST");
        assert_eq!(r.url, "https://codeberg.org/api/v1/repos/a/b/pulls");
        assert_eq!(
            r.header("authorization"),
            Some(format!("token {tok}").as_str())
        );
        let body: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["title"], "WIP: T");
        api.review(6, ReviewKind::Approve, "").unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["event"], "APPROVED");
        let pr = PullRequest {
            number: 6,
            ..Default::default()
        };
        api.merge(&pr, MergeMethod::Rebase, true).unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"Do": "rebase", "delete_branch_after_merge": true})
        );
        api.set_state(&pr, false).unwrap();
        assert_eq!(
            t.last("PATCH").url,
            "https://codeberg.org/api/v1/repos/a/b/pulls/6"
        );
    }
}
