//! Azure DevOps Services REST API 7.1 (dev.azure.com, *.visualstudio.com; Azure DevOps Server
//! collections via `api_url`).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::http::{encode, Api};
use super::json::{arr, b, first, i, normalize_text, s, time, u};
use super::{
    need_text, web, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr, PrDetails,
    PrProvider, PrState, PullRequest, Review, ReviewKind, StateFilter,
};

const V: &str = "api-version=7.1";

pub struct Azure {
    api: Api,
    /// Collection URL (`https://dev.azure.com/org`).
    collection: String,
    /// `{collection}/{project}/_apis/git/repositories/{repo}`
    repo: String,
    hosting: Hosting,
}

impl Azure {
    pub fn new(api: Api, base: String, h: &Hosting) -> Self {
        let project = h.url.owner.rsplit('/').next().unwrap_or_default();
        Self {
            api,
            repo: format!(
                "{base}/{}/_apis/git/repositories/{}",
                encode(project),
                encode(&h.url.name)
            ),
            collection: base,
            hosting: h.clone(),
        }
    }

    fn pr(&self, n: u64) -> String {
        format!("{}/pullrequests/{n}", self.repo)
    }

    fn my_id(&self) -> Result<String> {
        let v = self
            .api
            .get(&format!("{}/_apis/connectionData", self.collection))?;
        let id = s(&v, "authenticatedUser.id");
        if id.is_empty() || id == "00000000-0000-0000-0000-000000000000" {
            return Err(anyhow!("Azure DevOps did not report a signed-in user"));
        }
        Ok(id)
    }
}

fn branch(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_string()
}

/// Reviewer vote → review state (10 approved, 5 approved with suggestions, -5 waiting for
/// author, -10 rejected).
fn vote_state(vote: i64) -> &'static str {
    match vote {
        v if v >= 5 => "APPROVED",
        v if v <= -5 => "CHANGES_REQUESTED",
        _ => "REQUESTED",
    }
}

pub fn parse_pr(v: &Value, h: &Hosting) -> PullRequest {
    let state = match s(v, "status").as_str() {
        "completed" => PrState::Merged,
        "abandoned" => PrState::Closed,
        _ => PrState::Open,
    };
    let number = u(v, "pullRequestId").unwrap_or(0);
    let fork = s(v, "forkSource.repository.name");
    PullRequest {
        number,
        title: s(v, "title"),
        author: first(v, &["createdBy.uniqueName", "createdBy.displayName"]),
        head: branch(&s(v, "sourceRefName")),
        base: branch(&s(v, "targetRefName")),
        state,
        draft: b(v, "isDraft"),
        updated: time(v, "closedDate").or_else(|| time(v, "creationDate")),
        review: None,
        url: web::pr_url(h, number),
        labels: arr(v, "labels")
            .iter()
            .filter(|l| l.get("active").and_then(Value::as_bool) != Some(false))
            .map(|l| s(l, "name"))
            .collect(),
        source_repo: None,
        source_clone_url: Some(s(v, "forkSource.repository.remoteUrl"))
            .filter(|x| !x.is_empty() && !fork.is_empty()),
        head_sha: Some(s(v, "lastMergeSourceCommit.commitId")).filter(|x| !x.is_empty()),
    }
}

pub fn parse_details(
    pull: &Value,
    h: &Hosting,
    threads: &[Value],
    statuses: &[Value],
    changes: &[Value],
) -> PrDetails {
    let pr = parse_pr(pull, h);
    let mut checks = Checks::default();
    for st in statuses {
        let name = first(st, &["context.name", "description"]);
        let genre = s(st, "context.genre");
        let name = if genre.is_empty() {
            name
        } else {
            format!("{genre}/{name}")
        };
        checks.push(name, &s(st, "state"));
    }
    let reviews = arr(pull, "reviewers")
        .iter()
        .map(|r| Review {
            author: first(r, &["uniqueName", "displayName"]),
            state: vote_state(i(r, "vote").unwrap_or(0)).into(),
            body: String::new(),
            submitted: None,
        })
        .collect();
    let comments = threads
        .iter()
        .filter(|t| !b(t, "isDeleted"))
        .flat_map(|t| {
            let path = s(t, "threadContext.filePath");
            arr(t, "comments")
                .iter()
                .filter(|c| s(c, "commentType") != "system" && !b(c, "isDeleted"))
                .map(move |c| {
                    let body = normalize_text(&s(c, "content"));
                    Comment {
                        author: first(c, &["author.uniqueName", "author.displayName"]),
                        body: if path.is_empty() {
                            body
                        } else {
                            format!("[{path}] {body}")
                        },
                        created: time(c, "publishedDate"),
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mergeable = (pr.state == PrState::Open).then(|| match s(pull, "mergeStatus").as_str() {
        "succeeded" => "mergeable".to_string(),
        "conflicts" => "conflicting".to_string(),
        "" | "notSet" | "queued" => "checking".to_string(),
        other => other.to_string(),
    });
    let mut d = PrDetails {
        body: normalize_text(&s(pull, "description")),
        comments,
        reviews,
        checks,
        files: changes
            .iter()
            .filter(|c| s(c, "item.gitObjectType") != "tree")
            .map(|c| ChangedFile {
                path: s(c, "item.path").trim_start_matches('/').to_string(),
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

impl PrProvider for Azure {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let status = match filter {
            StateFilter::Open => "active",
            StateFilter::Closed => "abandoned",
            StateFilter::Merged => "completed",
            StateFilter::All => "all",
        };
        let v = self.api.get(&format!(
            "{}/pullrequests?searchCriteria.status={status}&$top=100&{V}",
            self.repo
        ))?;
        Ok(arr(&v, "value")
            .iter()
            .map(|p| parse_pr(p, &self.hosting))
            .collect())
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let pull = self.api.get(&format!("{}?{V}", self.pr(number)))?;
        let values = |url: String| {
            self.api
                .get(&url)
                .map(|v| arr(&v, "value").to_vec())
                .unwrap_or_default()
        };
        let threads = values(format!("{}/threads?{V}", self.pr(number)));
        let statuses = values(format!("{}/statuses?{V}", self.pr(number)));
        let iterations = values(format!("{}/iterations?{V}", self.pr(number)));
        let changes = match iterations.last().and_then(|it| u(it, "id")) {
            Some(id) => self
                .api
                .get(&format!(
                    "{}/iterations/{id}/changes?$top=500&{V}",
                    self.pr(number)
                ))
                .map(|v| arr(&v, "changeEntries").to_vec())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Ok(parse_details(
            &pull,
            &self.hosting,
            &threads,
            &statuses,
            &changes,
        ))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        if pr.head_repo.is_some() {
            return Err(anyhow!(
                "Creating Azure DevOps pull requests from a fork is not supported here; \
                 use \"Create in browser\" instead"
            ));
        }
        let v = self.api.send(
            "POST",
            &format!("{}/pullrequests?{V}", self.repo),
            &json!({
                "sourceRefName": format!("refs/heads/{}", pr.head),
                "targetRefName": format!("refs/heads/{}", pr.base),
                "title": pr.title,
                "description": pr.body,
                "isDraft": pr.draft,
            }),
        )?;
        let number = u(&v, "pullRequestId").unwrap_or(0);
        Ok(Created {
            number,
            url: web::pr_url(&self.hosting, number),
        })
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(need_text("A comment"));
        }
        self.api.send(
            "POST",
            &format!("{}/threads?{V}", self.pr(number)),
            &json!({
                "comments": [{ "parentCommentId": 0, "content": body, "commentType": 1 }],
                "status": 1,
            }),
        )?;
        Ok(())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String> {
        let vote = match kind {
            ReviewKind::Approve => Some(10),
            ReviewKind::RequestChanges => Some(-5),
            ReviewKind::Comment => None,
        };
        if let Some(vote) = vote {
            let me = self.my_id()?;
            self.api.send(
                "PUT",
                &format!("{}/reviewers/{}?{V}", self.pr(number), encode(&me)),
                &json!({ "vote": vote }),
            )?;
        }
        if !body.trim().is_empty() || kind == ReviewKind::Comment {
            self.comment(number, body)?;
        }
        Ok(match kind {
            ReviewKind::Approve => "Voted: approved".into(),
            ReviewKind::RequestChanges => "Voted: waiting for author".into(),
            ReviewKind::Comment => "Comment added".into(),
        })
    }

    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()> {
        let current = self.api.get(&format!("{}?{V}", self.pr(pr.number)))?;
        let commit = s(&current, "lastMergeSourceCommit.commitId");
        if commit.is_empty() {
            return Err(anyhow!(
                "Azure DevOps did not report the source commit; try again"
            ));
        }
        let strategy = match method {
            MergeMethod::Merge => "noFastForward",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        };
        self.api.send(
            "PATCH",
            &format!("{}?{V}", self.pr(pr.number)),
            &json!({
                "status": "completed",
                "lastMergeSourceCommit": { "commitId": commit },
                "completionOptions": { "mergeStrategy": strategy, "deleteSourceBranch": delete_branch },
            }),
        )?;
        Ok(())
    }

    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()> {
        self.api.send(
            "PATCH",
            &format!("{}?{V}", self.pr(pr.number)),
            &json!({ "status": if reopen { "active" } else { "abandoned" } }),
        )?;
        Ok(())
    }

    fn default_branch(&self) -> Result<String> {
        Ok(branch(&s(
            &self.api.get(&format!("{}?{V}", self.repo))?,
            "defaultBranch",
        )))
    }

    fn current_user(&self) -> Result<String> {
        let v = self
            .api
            .get(&format!("{}/_apis/connectionData", self.collection))?;
        let name = first(
            &v,
            &[
                "authenticatedUser.providerDisplayName",
                "authenticatedUser.customDisplayName",
            ],
        );
        if name.is_empty() || name.eq_ignore_ascii_case("anonymous") {
            return Err(anyhow!("Azure DevOps did not accept the token"));
        }
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{
        client, parse_remote_url, CheckOutcome, Credentials, DetectedBy, ProviderKind,
    };

    fn hosting() -> Hosting {
        let url = "git@ssh.dev.azure.com:v3/org/proj/repo";
        Hosting::new(
            ProviderKind::Azure,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::KnownHost,
        )
    }

    const PR: &str = r#"{"pullRequestId":77,"title":"Update","description":"Desc","status":"active",
      "isDraft":false,"createdBy":{"displayName":"Alice","uniqueName":"alice@corp.com"},
      "sourceRefName":"refs/heads/feature/x","targetRefName":"refs/heads/main",
      "creationDate":"2026-10-20T10:00:00.1234567Z","mergeStatus":"conflicts",
      "lastMergeSourceCommit":{"commitId":"aaa"},
      "reviewers":[{"displayName":"Bob","uniqueName":"bob@corp.com","vote":10},
                   {"displayName":"Cy","uniqueName":"cy@corp.com","vote":-5},
                   {"displayName":"Team","uniqueName":"team","vote":0}],
      "labels":[{"name":"infra","active":true}]}"#;

    #[test]
    fn parses_list_and_details() {
        let v: Value = serde_json::from_str(PR).unwrap();
        let h = hosting();
        let p = parse_pr(&v, &h);
        assert_eq!(
            (p.number, p.head.as_str(), p.base.as_str()),
            (77, "feature/x", "main")
        );
        assert_eq!(p.author, "alice@corp.com");
        assert_eq!(
            p.url,
            "https://dev.azure.com/org/proj/_git/repo/pullrequest/77"
        );
        assert!(p.updated.is_some());
        assert_eq!(p.labels, ["infra"]);
        let threads: Vec<Value> = serde_json::from_str(
            r#"[{"comments":[{"author":{"uniqueName":"bob@corp.com"},"content":"Nice","publishedDate":"2026-10-21T00:00:00Z","commentType":"text"}]},
                {"comments":[{"author":{"displayName":"Microsoft.VisualStudio.Services.TFS"},"content":"Policy","commentType":"system"}]},
                {"threadContext":{"filePath":"/src/a.cs"},"comments":[{"author":{"uniqueName":"cy"},"content":"typo","commentType":"text"}]}]"#,
        )
        .unwrap();
        let statuses: Vec<Value> = serde_json::from_str(
            r#"[{"state":"succeeded","context":{"name":"build","genre":"ci"}},{"state":"pending","description":"deploy"}]"#,
        )
        .unwrap();
        let changes: Vec<Value> = serde_json::from_str(
            r#"[{"item":{"path":"/src/a.cs","gitObjectType":"blob"}},{"item":{"path":"/src","gitObjectType":"tree"}}]"#,
        )
        .unwrap();
        let d = parse_details(&v, &h, &threads, &statuses, &changes);
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.comments[1].body, "[/src/a.cs] typo");
        assert_eq!(d.reviews.len(), 3);
        assert_eq!(
            d.pr.review,
            Some(super::super::ReviewDecision::ChangesRequested)
        );
        assert_eq!(d.checks.items[0].name, "ci/build");
        assert_eq!(d.checks.count(CheckOutcome::Pending), 1);
        assert_eq!(d.mergeable.as_deref(), Some("conflicting"));
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "src/a.cs");
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        let list = format!(r#"{{"value":[{PR}],"count":1}}"#);
        t.on(
            "GET",
            "/pullrequests?searchCriteria.status=completed",
            200,
            &list,
        )
        .on(
            "GET",
            "/_apis/connectionData",
            200,
            r#"{"authenticatedUser":{"id":"u-1","providerDisplayName":"Me"}}"#,
        )
        .on("PUT", "/reviewers/u-1", 200, "{}")
        .on("GET", "/pullrequests/77?api-version", 200, PR)
        .on("PATCH", "/pullrequests/77?api-version", 200, "{}")
        .on("POST", "/threads", 200, "{}")
        .on(
            "POST",
            "/pullrequests?api-version",
            201,
            r#"{"pullRequestId":78}"#,
        );
        let c = Credentials {
            username: None,
            secret: "pat".into(),
            source: "t".into(),
        };
        let api = client(&hosting(), Some(&c), t.clone()).unwrap();
        assert_eq!(api.list(StateFilter::Merged).unwrap().len(), 1);
        let r = t.last("GET");
        assert_eq!(
            r.url,
            "https://dev.azure.com/org/proj/_apis/git/repositories/repo/pullrequests?searchCriteria.status=completed&$top=100&api-version=7.1"
        );
        assert_eq!(r.header("authorization"), Some("Basic OnBhdA=="));
        api.review(77, ReviewKind::Approve, "").unwrap();
        let r = t.last("PUT");
        assert!(r
            .url
            .contains("/pullrequests/77/reviewers/u-1?api-version=7.1"));
        assert_eq!(r.body.as_deref(), Some(r#"{"vote":10}"#));
        let pr = parse_pr(&serde_json::from_str(PR).unwrap(), &hosting());
        api.merge(&pr, MergeMethod::Squash, true).unwrap();
        let body: Value = serde_json::from_str(t.last("PATCH").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["status"], "completed");
        assert_eq!(body["lastMergeSourceCommit"]["commitId"], "aaa");
        assert_eq!(body["completionOptions"]["mergeStrategy"], "squash");
        assert_eq!(body["completionOptions"]["deleteSourceBranch"], true);
        api.set_state(&pr, false).unwrap();
        assert_eq!(
            t.last("PATCH").body.as_deref(),
            Some(r#"{"status":"abandoned"}"#)
        );
        api.comment(77, "hello").unwrap();
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["comments"][0]["content"], "hello");
        let created = api
            .create(&NewPr {
                title: "T".into(),
                body: "B".into(),
                base: "main".into(),
                head: "f".into(),
                head_repo: None,
                draft: true,
            })
            .unwrap();
        assert_eq!(
            created.url,
            "https://dev.azure.com/org/proj/_git/repo/pullrequest/78"
        );
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["sourceRefName"], "refs/heads/f");
        assert_eq!(body["isDraft"], true);
        assert_eq!(api.current_user().unwrap(), "Me");
    }
}
