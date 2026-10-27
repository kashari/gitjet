//! Bitbucket Cloud REST API 2.0.

use anyhow::Result;
use serde_json::{json, Value};

use super::http::{encode, Api, MAX_ITEMS};
use super::json::{self as j, arr, b, first, normalize_text, s, time, u};
use super::{
    need_text, unsupported, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr,
    PrDetails, PrProvider, PrState, ProviderKind, PullRequest, Review, ReviewKind, StateFilter,
};

pub struct BitbucketCloud {
    api: Api,
    base: String,
    repo: String,
    slug: String,
}

impl BitbucketCloud {
    pub fn new(api: Api, base: String, h: &Hosting) -> Self {
        Self {
            api,
            repo: format!(
                "{base}/repositories/{}/{}",
                encode(&h.url.owner),
                encode(&h.url.name)
            ),
            base,
            slug: h.slug(),
        }
    }

    fn pr(&self, n: u64) -> String {
        format!("{}/pullrequests/{n}", self.repo)
    }

    /// `values` of every page (`next` links), up to `cap`.
    fn values(&self, url: &str, cap: usize) -> Result<Vec<Value>> {
        self.api.paged(
            url,
            cap,
            |v| arr(v, "values").to_vec(),
            |_, v| Some(s(v, "next")).filter(|n| !n.is_empty()),
        )
    }
}

fn user(v: &Value, path: &str) -> String {
    let u = j::get(v, path).cloned().unwrap_or_default();
    first(&u, &["nickname", "display_name", "username"])
}

pub fn parse_state(s: &str) -> PrState {
    match s {
        "MERGED" => PrState::Merged,
        "DECLINED" | "SUPERSEDED" => PrState::Closed,
        _ => PrState::Open,
    }
}

pub fn parse_pr(v: &Value, slug: &str) -> PullRequest {
    let source = s(v, "source.repository.full_name");
    PullRequest {
        number: u(v, "id").unwrap_or(0),
        title: s(v, "title"),
        author: user(v, "author"),
        head: s(v, "source.branch.name"),
        base: s(v, "destination.branch.name"),
        state: parse_state(&s(v, "state")),
        draft: b(v, "draft"),
        updated: time(v, "updated_on"),
        review: None,
        url: s(v, "links.html.href"),
        labels: Vec::new(),
        source_repo: (!source.is_empty() && !source.eq_ignore_ascii_case(slug)).then_some(source),
        source_clone_url: None,
        head_sha: Some(s(v, "source.commit.hash")).filter(|x| !x.is_empty()),
    }
}

pub fn parse_details(
    pull: &Value,
    slug: &str,
    comments: &[Value],
    statuses: &[Value],
    diffstat: &[Value],
) -> PrDetails {
    let pr = parse_pr(pull, slug);
    let mut checks = Checks::default();
    for st in statuses {
        checks.push(first(st, &["name", "key"]), &s(st, "state"));
    }
    let reviews = arr(pull, "participants")
        .iter()
        .filter_map(|p| {
            let state = match s(p, "state").as_str() {
                "approved" => "APPROVED",
                "changes_requested" => "CHANGES_REQUESTED",
                _ if b(p, "approved") => "APPROVED",
                _ if s(p, "role") == "REVIEWER" => "REQUESTED",
                _ => return None,
            };
            Some(Review {
                author: user(p, "user"),
                state: state.into(),
                body: String::new(),
                submitted: time(p, "participated_on"),
            })
        })
        .collect();
    let mut d = PrDetails {
        body: normalize_text(&s(pull, "description")),
        comments: comments
            .iter()
            .filter(|c| !b(c, "deleted"))
            .map(|c| {
                let mut body = normalize_text(&s(c, "content.raw"));
                let path = s(c, "inline.path");
                if !path.is_empty() {
                    body = format!("[{path}] {body}");
                }
                Comment {
                    author: user(c, "user"),
                    body,
                    created: time(c, "created_on"),
                }
            })
            .collect(),
        reviews,
        checks,
        files: diffstat
            .iter()
            .map(|f| ChangedFile {
                path: first(f, &["new.path", "old.path"]),
                additions: u(f, "lines_added"),
                deletions: u(f, "lines_removed"),
            })
            .collect(),
        ..Default::default()
    };
    d.pr = pr;
    d.fill_totals();
    d
}

impl PrProvider for BitbucketCloud {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let states: &[&str] = match filter {
            StateFilter::Open => &["OPEN"],
            StateFilter::Closed => &["DECLINED", "SUPERSEDED"],
            StateFilter::Merged => &["MERGED"],
            StateFilter::All => &["OPEN", "MERGED", "DECLINED", "SUPERSEDED"],
        };
        let q: String = states.iter().map(|s| format!("state={s}&")).collect();
        let url = format!("{}/pullrequests?{q}sort=-updated_on&pagelen=50", self.repo);
        let items = self.values(&url, MAX_ITEMS)?;
        Ok(items.iter().map(|v| parse_pr(v, &self.slug)).collect())
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let pull = self.api.get(&self.pr(number))?;
        let get = |p: &str| {
            self.values(&format!("{}/{p}", self.pr(number)), MAX_ITEMS)
                .unwrap_or_default()
        };
        let comments = get("comments?pagelen=100");
        let statuses = get("statuses?pagelen=100");
        let diffstat = get("diffstat?pagelen=100");
        Ok(parse_details(
            &pull, &self.slug, &comments, &statuses, &diffstat,
        ))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        let mut source = json!({ "branch": { "name": pr.head } });
        if let Some(fork) = &pr.head_repo {
            source["repository"] = json!({ "full_name": fork.slug() });
        }
        let v = self.api.send(
            "POST",
            &format!("{}/pullrequests", self.repo),
            &json!({
                "title": pr.title,
                "description": pr.body,
                "source": source,
                "destination": { "branch": { "name": pr.base } },
                "draft": pr.draft,
                "close_source_branch": false,
            }),
        )?;
        Ok(Created {
            number: u(&v, "id").unwrap_or(0),
            url: s(&v, "links.html.href"),
        })
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(need_text("A comment"));
        }
        self.api.send(
            "POST",
            &format!("{}/comments", self.pr(number)),
            &json!({ "content": { "raw": body } }),
        )?;
        Ok(())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String> {
        let verdict = match kind {
            ReviewKind::Approve => Some("approve"),
            ReviewKind::RequestChanges => Some("request-changes"),
            ReviewKind::Comment => None,
        };
        if let Some(v) = verdict {
            self.api.call("POST", &format!("{}/{v}", self.pr(number)))?;
        }
        if !body.trim().is_empty() || kind == ReviewKind::Comment {
            self.comment(number, body)?;
        }
        Ok(format!("{} submitted", kind.label()))
    }

    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()> {
        let strategy = match method {
            MergeMethod::Merge => "merge_commit",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "fast_forward",
        };
        self.api.send(
            "POST",
            &format!("{}/merge", self.pr(pr.number)),
            &json!({ "merge_strategy": strategy, "close_source_branch": delete_branch }),
        )?;
        Ok(())
    }

    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()> {
        if reopen {
            return Err(unsupported(
                ProviderKind::BitbucketCloud,
                "reopening a declined pull request (create a new one instead)",
            ));
        }
        self.api
            .call("POST", &format!("{}/decline", self.pr(pr.number)))?;
        Ok(())
    }

    fn default_branch(&self) -> Result<String> {
        Ok(s(&self.api.get(&self.repo)?, "mainbranch.name"))
    }

    fn current_user(&self) -> Result<String> {
        let v = self.api.get(&format!("{}/user", self.base))?;
        Ok(first(&v, &["username", "nickname", "display_name"]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{client, parse_remote_url, CheckOutcome, Credentials, DetectedBy};

    const PR: &str = r#"{"id":3,"title":"Add API","description":"desc","state":"OPEN","draft":false,
      "author":{"display_name":"Alice A","nickname":"alice"},
      "source":{"branch":{"name":"feat"},"repository":{"full_name":"alice/repo"},"commit":{"hash":"abc"}},
      "destination":{"branch":{"name":"main"},"repository":{"full_name":"team/repo"}},
      "updated_on":"2026-10-20T10:00:00.123456+00:00",
      "links":{"html":{"href":"https://bitbucket.org/team/repo/pull-requests/3"}},
      "participants":[{"user":{"nickname":"bob"},"role":"REVIEWER","approved":true,"state":"approved"},
                      {"user":{"nickname":"cy"},"role":"REVIEWER","approved":false,"state":"changes_requested"},
                      {"user":{"nickname":"dd"},"role":"REVIEWER","approved":false,"state":null},
                      {"user":{"nickname":"ee"},"role":"PARTICIPANT","approved":false}]}"#;

    #[test]
    fn parses_list_and_details() {
        let v: Value = serde_json::from_str(PR).unwrap();
        let p = parse_pr(&v, "team/repo");
        assert_eq!(
            (p.number, p.author.as_str(), p.head.as_str()),
            (3, "alice", "feat")
        );
        assert_eq!(p.source_repo.as_deref(), Some("alice/repo"));
        assert!(p.updated.is_some());
        assert_eq!(parse_state("DECLINED"), PrState::Closed);
        let comments: Vec<Value> = serde_json::from_str(
            r#"[{"user":{"nickname":"bob"},"content":{"raw":"Looks good"},"created_on":"2026-10-01T00:00:00+00:00"},
                {"user":{"nickname":"bob"},"content":{"raw":"x"},"deleted":true},
                {"user":{"nickname":"cy"},"content":{"raw":"typo"},"inline":{"path":"src/a.rs"}}]"#,
        )
        .unwrap();
        let statuses: Vec<Value> = serde_json::from_str(
            r#"[{"key":"build","name":"Pipeline #4","state":"SUCCESSFUL"},{"key":"x","state":"INPROGRESS"}]"#,
        )
        .unwrap();
        let diffstat: Vec<Value> = serde_json::from_str(
            r#"[{"new":{"path":"a.rs"},"lines_added":5,"lines_removed":2},
                {"old":{"path":"gone.rs"},"new":null,"lines_added":0,"lines_removed":9}]"#,
        )
        .unwrap();
        let d = parse_details(&v, "team/repo", &comments, &statuses, &diffstat);
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.comments[1].body, "[src/a.rs] typo");
        assert_eq!(d.reviews.len(), 3);
        assert_eq!(
            d.pr.review,
            Some(super::super::ReviewDecision::ChangesRequested)
        );
        assert_eq!(d.checks.count(CheckOutcome::Success), 1);
        assert_eq!(d.checks.count(CheckOutcome::Pending), 1);
        assert_eq!((d.additions, d.deletions), (Some(5), Some(11)));
        assert_eq!(d.files[1].path, "gone.rs");
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        let page1 = format!(
            r#"{{"values":[{PR}],"next":"https://api.bitbucket.org/2.0/repositories/team/repo/pullrequests?page=2"}}"#
        );
        t.on("GET", "/pullrequests?state=OPEN", 200, &page1)
            .on("GET", "/pullrequests?page=2", 200, r#"{"values":[]}"#)
            .on(
                "POST",
                "/pullrequests",
                201,
                r#"{"id":4,"links":{"html":{"href":"u"}}}"#,
            )
            .on("POST", "/request-changes", 200, "{}")
            .on("POST", "/comments", 201, "{}")
            .on("POST", "/merge", 200, "{}")
            .on("POST", "/decline", 200, "{}");
        let url = "git@bitbucket.org:team/repo.git";
        let h = Hosting::new(
            ProviderKind::BitbucketCloud,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::KnownHost,
        );
        let c = Credentials {
            username: Some("me".into()),
            secret: "app-pw".into(),
            source: "t".into(),
        };
        let api = client(&h, Some(&c), t.clone()).unwrap();
        assert_eq!(api.list(StateFilter::Open).unwrap().len(), 1);
        assert_eq!(t.log().len(), 2, "followed the next link");
        assert!(t.log()[0]
            .header("authorization")
            .unwrap()
            .starts_with("Basic "));
        api.review(3, ReviewKind::RequestChanges, "see comments")
            .unwrap();
        let log = t.log();
        assert!(log[log.len() - 2]
            .url
            .ends_with("/pullrequests/3/request-changes"));
        assert_eq!(
            log[log.len() - 1].body.as_deref(),
            Some(r#"{"content":{"raw":"see comments"}}"#)
        );
        let fork = parse_remote_url("git@bitbucket.org:me/repo.git").unwrap();
        api.create(&NewPr {
            title: "T".into(),
            body: "B".into(),
            base: "main".into(),
            head: "feat".into(),
            head_repo: Some(fork),
            draft: false,
        })
        .unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["source"]["repository"]["full_name"], "me/repo");
        assert_eq!(body["destination"]["branch"]["name"], "main");
        let pr = PullRequest {
            number: 3,
            ..Default::default()
        };
        api.merge(&pr, MergeMethod::Squash, true).unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"merge_strategy":"squash","close_source_branch":true})
        );
        api.set_state(&pr, false).unwrap();
        assert!(t.last("POST").url.ends_with("/pullrequests/3/decline"));
        let e = format!("{:#}", api.set_state(&pr, true).unwrap_err());
        assert!(e.contains("does not support reopening"), "{e}");
    }
}
