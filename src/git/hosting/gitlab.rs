//! GitLab REST API v4 (gitlab.com and self-managed): merge requests.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::http::{encode, Api, MAX_ITEMS};
use super::json::{self as j, arr, b, labels, normalize_text, s, time, u};
use super::{
    need_text, ChangedFile, Checks, Comment, Created, Hosting, MergeMethod, NewPr, PrDetails,
    PrProvider, PrState, PullRequest, Review, ReviewKind, StateFilter,
};

pub struct GitLab {
    api: Api,
    base: String,
    /// `/projects/<url-encoded path>`
    project: String,
}

impl GitLab {
    pub fn new(api: Api, base: String, h: &Hosting) -> Self {
        Self {
            api,
            project: format!("{base}/projects/{}", encode(&h.slug())),
            base,
        }
    }

    fn mr(&self, iid: u64) -> String {
        format!("{}/merge_requests/{iid}", self.project)
    }
}

pub fn parse_state(state: &str) -> PrState {
    match state {
        "merged" => PrState::Merged,
        "closed" => PrState::Closed,
        "locked" => PrState::Locked,
        _ => PrState::Open,
    }
}

pub fn parse_mr(v: &Value) -> PullRequest {
    let fork = u(v, "source_project_id") != u(v, "target_project_id");
    PullRequest {
        number: u(v, "iid").unwrap_or(0),
        title: s(v, "title"),
        author: s(v, "author.username"),
        head: s(v, "source_branch"),
        base: s(v, "target_branch"),
        state: parse_state(&s(v, "state")),
        draft: b(v, "draft") || b(v, "work_in_progress"),
        updated: time(v, "updated_at"),
        review: None,
        url: s(v, "web_url"),
        labels: labels(v, "labels"),
        // The project path of a fork is not in the listing; the MR ref covers fetching.
        source_repo: None,
        source_clone_url: None,
        head_sha: Some(s(v, "sha")).filter(|x| !x.is_empty() && !fork),
    }
}

/// +/− counts of a unified diff body.
fn count_lines(diff: &str) -> (u64, u64) {
    let mut add = 0;
    let mut del = 0;
    for l in diff.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            add += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            del += 1;
        }
    }
    (add, del)
}

/// Details from the MR, its notes, approvals, pipeline jobs and diffs.
pub fn parse_details(
    mr: &Value,
    notes: &[Value],
    approvals: &Value,
    jobs: &[Value],
    diffs: &[Value],
) -> PrDetails {
    let pr = parse_mr(mr);
    let mut checks = Checks::default();
    for job in jobs {
        checks.push(
            format!("{} ({})", s(job, "name"), s(job, "stage")),
            &s(job, "status"),
        );
    }
    if checks.is_empty() {
        if let Some(p) = j::get(mr, "head_pipeline") {
            checks.push(format!("pipeline #{}", s(p, "id")), &s(p, "status"));
        }
    }
    let mut reviews: Vec<Review> = arr(approvals, "approved_by")
        .iter()
        .map(|a| Review {
            author: s(a, "user.username"),
            state: "APPROVED".into(),
            body: String::new(),
            submitted: None,
        })
        .collect();
    for r in arr(mr, "reviewers") {
        let who = s(r, "username");
        if !reviews.iter().any(|x| x.author == who) {
            reviews.push(Review {
                author: who,
                state: "REQUESTED".into(),
                body: String::new(),
                submitted: None,
            });
        }
    }
    let files: Vec<ChangedFile> = diffs
        .iter()
        .map(|d| {
            let (a, r) = count_lines(&s(d, "diff"));
            ChangedFile {
                path: s(d, "new_path"),
                additions: Some(a),
                deletions: Some(r),
            }
        })
        .collect();
    let status = j::first(mr, &["detailed_merge_status", "merge_status"]);
    let mergeable =
        (pr.state == PrState::Open && !status.is_empty()).then(|| match status.as_str() {
            "mergeable" | "can_be_merged" => "mergeable".to_string(),
            "broken_status" | "cannot_be_merged" | "conflict" => "conflicting".to_string(),
            other => other.replace('_', " "),
        });
    let mut d = PrDetails {
        body: normalize_text(&s(mr, "description")),
        comments: notes
            .iter()
            .filter(|n| !b(n, "system"))
            .map(|n| Comment {
                author: s(n, "author.username"),
                body: normalize_text(&s(n, "body")),
                created: time(n, "created_at"),
            })
            .collect(),
        reviews,
        checks,
        files,
        additions: None,
        deletions: None,
        mergeable,
        changed_files: u(mr, "changes_count"),
        pr,
    };
    d.fill_totals();
    if d.pr.review.is_none()
        && b(approvals, "approved")
        && !arr(approvals, "approved_by").is_empty()
    {
        d.pr.review = Some(super::ReviewDecision::Approved);
    }
    d
}

impl PrProvider for GitLab {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>> {
        let state = match filter {
            StateFilter::Open => "opened",
            StateFilter::Closed => "closed",
            StateFilter::Merged => "merged",
            StateFilter::All => "all",
        };
        let url = format!(
            "{}/merge_requests?state={state}&order_by=updated_at&sort=desc&per_page=50",
            self.project
        );
        let items = self.api.paged_array(&url, MAX_ITEMS)?;
        Ok(items.iter().map(parse_mr).collect())
    }

    fn details(&self, number: u64) -> Result<PrDetails> {
        let mr = self.api.get(&self.mr(number))?;
        let list = |path: String| self.api.paged_array(&path, MAX_ITEMS).unwrap_or_default();
        let notes = list(format!(
            "{}/notes?sort=asc&order_by=created_at&per_page=100",
            self.mr(number)
        ));
        let approvals = self
            .api
            .get(&format!("{}/approvals", self.mr(number)))
            .unwrap_or_default();
        let jobs = match u(&mr, "head_pipeline.id") {
            Some(id) => list(format!("{}/pipelines/{id}/jobs?per_page=100", self.project)),
            None => Vec::new(),
        };
        let diffs = list(format!("{}/diffs?per_page=100", self.mr(number)));
        Ok(parse_details(&mr, &notes, &approvals, &jobs, &diffs))
    }

    fn create(&self, pr: &NewPr) -> Result<Created> {
        let title = if pr.draft && !pr.title.to_ascii_lowercase().starts_with("draft:") {
            format!("Draft: {}", pr.title)
        } else {
            pr.title.clone()
        };
        let mut payload = json!({
            "source_branch": pr.head,
            "target_branch": pr.base,
            "title": title,
            "description": pr.body,
            "remove_source_branch": false,
        });
        // A fork's MR is created on the fork, pointing at the target project.
        let url = match &pr.head_repo {
            Some(fork) => {
                let target = self.api.get(&self.project)?;
                let id = u(&target, "id").ok_or_else(|| anyhow!("target project has no id"))?;
                payload["target_project_id"] = json!(id);
                format!(
                    "{}/projects/{}/merge_requests",
                    self.base,
                    encode(&fork.slug())
                )
            }
            None => format!("{}/merge_requests", self.project),
        };
        let v = self.api.send("POST", &url, &payload)?;
        Ok(Created {
            number: u(&v, "iid").unwrap_or(0),
            url: s(&v, "web_url"),
        })
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(need_text("A comment"));
        }
        self.api.send(
            "POST",
            &format!("{}/notes", self.mr(number)),
            &json!({ "body": body }),
        )?;
        Ok(())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String> {
        match kind {
            ReviewKind::Approve => {
                self.api
                    .call("POST", &format!("{}/approve", self.mr(number)))?;
                if !body.trim().is_empty() {
                    self.comment(number, body)?;
                }
                Ok("Approved".into())
            }
            ReviewKind::RequestChanges => {
                // The REST API has no "request changes" verdict: withdraw an approval and say so.
                let _ = self
                    .api
                    .call("POST", &format!("{}/unapprove", self.mr(number)));
                let text = if body.trim().is_empty() {
                    "**Changes requested**".to_string()
                } else {
                    format!("**Changes requested**\n\n{}", body.trim())
                };
                self.comment(number, &text)?;
                Ok("Changes requested (GitLab's API records this as a comment; any approval of yours was withdrawn)".into())
            }
            ReviewKind::Comment => {
                self.comment(number, body)?;
                Ok("Comment added".into())
            }
        }
    }

    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()> {
        if method == MergeMethod::Rebase {
            return Err(anyhow!(
                "GitLab sets rebase-merging per project (Settings → Merge requests → fast-forward); \
                 merge with a merge commit or squash instead"
            ));
        }
        self.api.send(
            "PUT",
            &format!("{}/merge", self.mr(pr.number)),
            &json!({
                "squash": method == MergeMethod::Squash,
                "should_remove_source_branch": delete_branch,
            }),
        )?;
        Ok(())
    }

    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()> {
        self.api.send(
            "PUT",
            &self.mr(pr.number),
            &json!({ "state_event": if reopen { "reopen" } else { "close" } }),
        )?;
        Ok(())
    }

    fn default_branch(&self) -> Result<String> {
        Ok(s(&self.api.get(&self.project)?, "default_branch"))
    }

    fn current_user(&self) -> Result<String> {
        Ok(s(
            &self.api.get(&format!("{}/user", self.base))?,
            "username",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::{
        client, parse_remote_url, CheckOutcome, Credentials, DetectedBy, ProviderKind,
    };

    const MRS: &str = r#"[
      {"iid":12,"title":"Add feature","author":{"username":"alice"},"source_branch":"feat",
       "target_branch":"main","state":"opened","draft":true,"updated_at":"2026-10-20T10:00:00.000Z",
       "web_url":"https://gitlab.example.com/group/sub/proj/-/merge_requests/12","labels":["ux","backend"],
       "source_project_id":1,"target_project_id":1,"sha":"deadbeef"},
      {"iid":3,"title":"Old","author":{"username":"bob"},"source_branch":"old","target_branch":"main",
       "state":"merged","work_in_progress":false,"labels":[],"source_project_id":2,"target_project_id":1}
    ]"#;

    const MR: &str = r#"{"iid":12,"title":"Add feature","description":"Details","author":{"username":"alice"},
      "source_branch":"feat","target_branch":"main","state":"opened","draft":false,
      "detailed_merge_status":"mergeable","changes_count":"2",
      "head_pipeline":{"id":99,"status":"failed"},
      "reviewers":[{"username":"carol"},{"username":"dave"}]}"#;

    #[test]
    fn parses_list_and_details() {
        let items: Vec<Value> = serde_json::from_str(MRS).unwrap();
        let v: Vec<PullRequest> = items.iter().map(parse_mr).collect();
        assert_eq!((v[0].number, v[0].draft, v[0].labels.len()), (12, true, 2));
        assert_eq!(v[0].head_sha.as_deref(), Some("deadbeef"));
        assert_eq!(v[1].state, PrState::Merged);
        assert!(v[0].updated.is_some());

        let mr: Value = serde_json::from_str(MR).unwrap();
        let notes: Vec<Value> = serde_json::from_str(
            r#"[{"author":{"username":"bot"},"body":"added 1 commit","system":true},
                {"author":{"username":"bob"},"body":"LGTM","system":false,"created_at":"2026-10-01T00:00:00Z"}]"#,
        )
        .unwrap();
        let approvals: Value = serde_json::from_str(
            r#"{"approved":true,"approved_by":[{"user":{"username":"carol"}}]}"#,
        )
        .unwrap();
        let jobs: Vec<Value> = serde_json::from_str(
            r#"[{"name":"test","stage":"test","status":"failed"},{"name":"build","stage":"build","status":"success"}]"#,
        )
        .unwrap();
        let diffs: Vec<Value> =
            serde_json::from_str(r#"[{"new_path":"a.rs","diff":"@@ -1 +1,2 @@\n-x\n+y\n+z\n"}]"#)
                .unwrap();
        let d = parse_details(&mr, &notes, &approvals, &jobs, &diffs);
        assert_eq!(d.body, "Details");
        assert_eq!(d.comments.len(), 1);
        assert_eq!(d.reviews.len(), 2, "carol approved, dave requested");
        assert_eq!(d.pr.review, Some(super::super::ReviewDecision::Approved));
        assert_eq!(d.checks.count(CheckOutcome::Failure), 1);
        assert_eq!(d.mergeable.as_deref(), Some("mergeable"));
        assert_eq!((d.additions, d.deletions), (Some(2), Some(1)));
        assert_eq!(d.changed_files, Some(2));
        // Without jobs, the head pipeline is the check.
        let d = parse_details(&mr, &[], &Value::Null, &[], &[]);
        assert_eq!(d.checks.items[0].name, "pipeline #99");
    }

    #[test]
    fn builds_requests() {
        let t = FakeTransport::new();
        t.on(
            "GET",
            "/merge_requests?state=merged&order_by=updated_at",
            200,
            MRS,
        )
        .on(
            "GET",
            "/projects/group%2Fsub%2Fproj",
            200,
            r#"{"id":77,"default_branch":"develop"}"#,
        )
        .on(
            "POST",
            "/merge_requests",
            201,
            r#"{"iid":13,"web_url":"https://x/13"}"#,
        )
        .on("POST", "/notes", 201, "{}")
        .on("POST", "/approve", 201, "{}")
        .on("POST", "/unapprove", 404, "{}")
        .on("PUT", "/merge", 200, "{}")
        .on("PUT", "/merge_requests/12", 200, "{}");
        let url = "git@gitlab.example.com:group/sub/proj.git";
        let h = Hosting::new(
            ProviderKind::GitLab,
            "origin",
            url,
            parse_remote_url(url).unwrap(),
            None,
            DetectedBy::HostName,
        );
        let c = Credentials {
            username: None,
            secret: "glpat".into(),
            source: "t".into(),
        };
        let api = client(&h, Some(&c), t.clone()).unwrap();
        assert_eq!(api.list(StateFilter::Merged).unwrap().len(), 2);
        let r = t.last("GET");
        assert!(r.url.starts_with(
            "https://gitlab.example.com/api/v4/projects/group%2Fsub%2Fproj/merge_requests?state=merged"
        ));
        assert_eq!(r.header("private-token"), Some("glpat"));
        assert_eq!(api.default_branch().unwrap(), "develop");

        let fork = parse_remote_url("git@gitlab.example.com:me/proj.git").unwrap();
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
        assert_eq!(created.number, 13);
        let r = t.last("POST");
        assert_eq!(
            r.url,
            "https://gitlab.example.com/api/v4/projects/me%2Fproj/merge_requests"
        );
        let body: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["title"], "Draft: T");
        assert_eq!(body["target_project_id"], 77);

        let msg = api
            .review(12, ReviewKind::RequestChanges, "fix it")
            .unwrap();
        assert!(msg.contains("comment"));
        let body: Value = serde_json::from_str(t.last("POST").body.as_deref().unwrap()).unwrap();
        assert_eq!(body["body"], "**Changes requested**\n\nfix it");
        api.review(12, ReviewKind::Approve, "").unwrap();
        assert!(t.last("POST").url.ends_with("/merge_requests/12/approve"));

        let pr = PullRequest {
            number: 12,
            ..Default::default()
        };
        api.merge(&pr, MergeMethod::Squash, true).unwrap();
        let body: Value = serde_json::from_str(t.last("PUT").body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"squash": true, "should_remove_source_branch": true})
        );
        assert!(api.merge(&pr, MergeMethod::Rebase, false).is_err());
        api.set_state(&pr, true).unwrap();
        assert_eq!(
            t.last("PUT").body.as_deref(),
            Some(r#"{"state_event":"reopen"}"#)
        );
    }
}
