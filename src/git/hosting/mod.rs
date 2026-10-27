//! Pull / merge requests for any Git hosting provider, through the providers' REST APIs and plain
//! git — no provider CLI is needed.
//!
//! - [`remote_url`]: parse every remote URL format (https, ssh, scp-like, Azure DevOps, Bitbucket Server).
//! - [`detect`]: which provider serves a remote (config overrides, known hosts, host-name hints, an
//!   API probe cached in `hosting.json`), else a *generic* host without a pull request API.
//! - [`auth`]: tokens from the environment or the user's git credential helper; sign-in stores a
//!   token with `git credential approve` — never in config files.
//! - [`http`]: the transport (faked in tests) and an authenticated JSON client.
//! - One [`PrProvider`] per API: GitHub (+ Enterprise), GitLab, Bitbucket Cloud, Bitbucket
//!   Server / Data Center, Azure DevOps, Gitea / Forgejo / Codeberg.
//! - [`refs`]: provider-neutral diff and checkout by fetching the pull request's git ref.
//! - [`web`]: browser URLs for repository, branch, commit, request and "create" pages.
//! - [`local`]: pushing the branch before a request is created, description suggestions.

pub mod auth;
pub mod detect;
pub mod http;
mod json;
pub mod local;
pub mod refs;
pub mod remote_url;
pub mod web;

mod azure;
mod bitbucket;
mod bitbucket_server;
mod gitea;
mod github;
mod gitlab;

use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::config::HostingConfig;

use super::Repo;

pub use auth::Credentials;
pub use detect::{detect, detect_local, DetectedBy, Detection};
pub use http::{Transport, UreqTransport};
pub use local::*;
pub use remote_url::{parse_remote_url, RemoteUrl};
pub use web::open_url;

// ----- providers ---------------------------------------------------------------------------------

/// A code hosting service (or a generic Git server without a known pull request API).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    GitHub,
    GitLab,
    BitbucketCloud,
    BitbucketServer,
    Azure,
    Gitea,
    Generic,
}

impl ProviderKind {
    /// Every kind with a pull request API.
    #[cfg(test)]
    pub const WITH_API: [ProviderKind; 6] = [
        ProviderKind::GitHub,
        ProviderKind::GitLab,
        ProviderKind::BitbucketCloud,
        ProviderKind::BitbucketServer,
        ProviderKind::Azure,
        ProviderKind::Gitea,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "GitHub",
            ProviderKind::GitLab => "GitLab",
            ProviderKind::BitbucketCloud => "Bitbucket Cloud",
            ProviderKind::BitbucketServer => "Bitbucket Server",
            ProviderKind::Azure => "Azure DevOps",
            ProviderKind::Gitea => "Gitea/Forgejo",
            ProviderKind::Generic => "Git server",
        }
    }

    /// Value of `kind` in `[[hosting.hosts]]`.
    pub fn config_name(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "github",
            ProviderKind::GitLab => "gitlab",
            ProviderKind::BitbucketCloud => "bitbucket",
            ProviderKind::BitbucketServer => "bitbucket-server",
            ProviderKind::Azure => "azure",
            ProviderKind::Gitea => "gitea",
            ProviderKind::Generic => "none",
        }
    }

    /// Parse a config `kind` (with common aliases).
    pub fn from_config(s: &str) -> Option<ProviderKind> {
        let k = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Some(match k.as_str() {
            "github" | "github-enterprise" | "ghe" => ProviderKind::GitHub,
            "gitlab" => ProviderKind::GitLab,
            "bitbucket" | "bitbucket-cloud" | "bitbucket.org" => ProviderKind::BitbucketCloud,
            "bitbucket-server"
            | "bitbucket-dc"
            | "bitbucket-datacenter"
            | "bitbucket-data-center"
            | "stash" => ProviderKind::BitbucketServer,
            "azure" | "azure-devops" | "azuredevops" | "ado" | "tfs" | "vsts" => {
                ProviderKind::Azure
            }
            "gitea" | "forgejo" | "codeberg" | "gogs" => ProviderKind::Gitea,
            "none" | "generic" | "other" | "git" => ProviderKind::Generic,
            _ => return None,
        })
    }

    pub fn has_api(self) -> bool {
        self != ProviderKind::Generic
    }

    /// "pull request" / "merge request".
    pub fn noun(self) -> &'static str {
        match self {
            ProviderKind::GitLab => "merge request",
            _ => "pull request",
        }
    }

    /// "PR" / "MR".
    pub fn short(self) -> &'static str {
        match self {
            ProviderKind::GitLab => "MR",
            _ => "PR",
        }
    }

    /// Prefix of a request reference: `#12`, or `!12` on GitLab and Azure DevOps.
    pub fn sigil(self) -> char {
        match self {
            ProviderKind::GitLab | ProviderKind::Azure => '!',
            _ => '#',
        }
    }

    /// Where to create a token for signing in.
    pub fn token_help(self) -> &'static str {
        match self {
            ProviderKind::GitHub => {
                "Settings → Developer settings → Personal access tokens (scope: repo)"
            }
            ProviderKind::GitLab => "Preferences → Access tokens (scope: api)",
            ProviderKind::BitbucketCloud => {
                "Personal settings → App passwords (pull requests: write) with your username, \
                 or a repository access token without one"
            }
            ProviderKind::BitbucketServer => {
                "Profile → Manage account → HTTP access tokens (project/repository write)"
            }
            ProviderKind::Azure => "User settings → Personal access tokens (Code: read & write)",
            ProviderKind::Gitea => {
                "Settings → Applications → Access tokens (repository: read & write)"
            }
            ProviderKind::Generic => "",
        }
    }

    /// Whether the sign-in dialog asks for a user name (optional where a token alone works).
    pub fn asks_username(self) -> bool {
        matches!(
            self,
            ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer
        )
    }
}

/// The hosted repository pull requests are listed for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hosting {
    pub kind: ProviderKind,
    /// Git remote name (e.g. `origin`, `upstream`).
    pub remote: String,
    /// The remote's fetch URL as configured.
    pub fetch_url: String,
    pub url: RemoteUrl,
    /// REST API base; `None` for generic hosts.
    pub api_base: Option<String>,
    pub detected_by: DetectedBy,
}

impl Hosting {
    /// Build the hosting for `kind`, applying provider URL conventions and the API base
    /// (`api_override` from config wins).
    pub fn new(
        kind: ProviderKind,
        remote: &str,
        fetch_url: &str,
        url: RemoteUrl,
        api_override: Option<&str>,
        detected_by: DetectedBy,
    ) -> Hosting {
        let api_base = api_override
            .map(|a| a.trim().trim_end_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .or_else(|| default_api_base(kind, &url));
        Hosting {
            kind,
            remote: remote.to_string(),
            fetch_url: fetch_url.to_string(),
            url,
            api_base,
            detected_by,
        }
    }

    pub fn slug(&self) -> String {
        self.url.slug()
    }

    /// `host/owner/name` (with port for http(s)).
    pub fn location(&self) -> String {
        format!("{}/{}", self.url.credential_host(), self.slug())
    }

    /// "Azure DevOps · dev.azure.com/org/proj/repo".
    pub fn display(&self) -> String {
        format!("{} · {}", self.kind.name(), self.location())
    }

    /// `#12` / `!12`.
    pub fn reference(&self, number: u64) -> String {
        format!("{}{number}", self.kind.sigil())
    }

    /// Clone URL of another repository (a fork) on the same server, in the remote's format.
    pub fn clone_url_of(&self, slug: &str) -> String {
        remote_url::sibling_clone_url(&self.fetch_url, &self.slug(), slug)
    }
}

fn default_api_base(kind: ProviderKind, url: &RemoteUrl) -> Option<String> {
    let web = url.web_base.trim_end_matches('/');
    Some(match kind {
        ProviderKind::GitHub if url.host == "github.com" => "https://api.github.com".into(),
        ProviderKind::GitHub => format!("{web}/api/v3"),
        ProviderKind::GitLab => format!("{web}/api/v4"),
        ProviderKind::BitbucketCloud => "https://api.bitbucket.org/2.0".into(),
        ProviderKind::BitbucketServer => format!("{web}/rest/api/1.0"),
        ProviderKind::Azure => {
            // The collection: everything before the project.
            let collection = url.owner.rsplit_once('/').map(|(c, _)| c).unwrap_or("");
            if collection.is_empty() {
                web.to_string()
            } else {
                format!("{web}/{}", http::encode_path(collection))
            }
        }
        ProviderKind::Gitea => format!("{web}/api/v1"),
        ProviderKind::Generic => return None,
    })
}

// ----- data model ------------------------------------------------------------------------------

/// Lifecycle state of a pull / merge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrState {
    #[default]
    Open,
    /// Closed without merging (declined on Bitbucket, abandoned on Azure DevOps).
    Closed,
    Merged,
    /// GitLab: discussion locked.
    Locked,
}

impl PrState {
    pub fn label(self) -> &'static str {
        match self {
            PrState::Open => "Open",
            PrState::Closed => "Closed",
            PrState::Merged => "Merged",
            PrState::Locked => "Locked",
        }
    }
}

/// Which requests to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateFilter {
    #[default]
    Open,
    Closed,
    Merged,
    All,
}

impl StateFilter {
    /// open → closed → merged → all → open.
    pub fn next(self) -> StateFilter {
        match self {
            StateFilter::Open => StateFilter::Closed,
            StateFilter::Closed => StateFilter::Merged,
            StateFilter::Merged => StateFilter::All,
            StateFilter::All => StateFilter::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::Merged => "merged",
            StateFilter::All => "all",
        }
    }

    /// Whether a request in `state` belongs in this listing.
    pub fn accepts(self, state: PrState) -> bool {
        match self {
            StateFilter::Open => matches!(state, PrState::Open | PrState::Locked),
            StateFilter::Closed => state == PrState::Closed,
            StateFilter::Merged => state == PrState::Merged,
            StateFilter::All => true,
        }
    }
}

/// Aggregated review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl ReviewDecision {
    pub fn label(self) -> &'static str {
        match self {
            ReviewDecision::Approved => "approved",
            ReviewDecision::ChangesRequested => "changes requested",
            ReviewDecision::ReviewRequired => "review required",
        }
    }

    /// Aggregate per-reviewer states (latest per reviewer): any "changes requested" wins, then any
    /// approval, then outstanding requests.
    pub fn aggregate(reviews: &[Review]) -> Option<ReviewDecision> {
        let mut latest: Vec<(&str, &str)> = Vec::new();
        for r in reviews {
            if matches!(r.state.as_str(), "COMMENTED" | "DISMISSED" | "PENDING") {
                continue;
            }
            match latest.iter_mut().find(|(a, _)| *a == r.author) {
                Some(e) => e.1 = &r.state,
                None => latest.push((&r.author, &r.state)),
            }
        }
        if latest.iter().any(|(_, s)| *s == "CHANGES_REQUESTED") {
            Some(ReviewDecision::ChangesRequested)
        } else if latest.iter().any(|(_, s)| *s == "APPROVED") {
            Some(ReviewDecision::Approved)
        } else if latest.iter().any(|(_, s)| *s == "REQUESTED") {
            Some(ReviewDecision::ReviewRequired)
        } else {
            None
        }
    }
}

/// One row of the request list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// Source branch.
    pub head: String,
    /// Target branch.
    pub base: String,
    pub state: PrState,
    pub draft: bool,
    /// Unix timestamp of the last update.
    pub updated: Option<i64>,
    pub review: Option<ReviewDecision>,
    /// Web page of the request.
    pub url: String,
    pub labels: Vec<String>,
    /// `owner/name` of the source repository when it is a fork.
    pub source_repo: Option<String>,
    /// Clone URL of the source repository, when the API reports one.
    pub source_clone_url: Option<String>,
    /// Commit at the head of the source branch, when known.
    pub head_sha: Option<String>,
}

/// A conversation comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created: Option<i64>,
}

/// A submitted review or a reviewer's vote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub author: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`, `REQUESTED`.
    pub state: String,
    pub body: String,
    pub submitted: Option<i64>,
}

/// Outcome of one CI check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Success,
    Failure,
    Pending,
    Skipped,
}

impl CheckOutcome {
    /// Map the many provider spellings (`success`, `SUCCESSFUL`, `failed`, `in_progress`…).
    pub fn parse(s: &str) -> CheckOutcome {
        match s.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "success" | "successful" | "succeeded" | "passed" | "completed" | "manual" => {
                CheckOutcome::Success
            }
            "failure" | "failed" | "error" | "errored" | "timed_out" | "action_required"
            | "startup_failure" | "cancelled" | "canceled" | "stopped" | "rejected" => {
                CheckOutcome::Failure
            }
            "skipped" | "neutral" | "not_applicable" | "notapplicable" | "stale" => {
                CheckOutcome::Skipped
            }
            _ => CheckOutcome::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub outcome: CheckOutcome,
}

/// CI status summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checks {
    pub items: Vec<Check>,
}

impl Checks {
    pub fn count(&self, outcome: CheckOutcome) -> usize {
        self.items.iter().filter(|c| c.outcome == outcome).count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push(&mut self, name: impl Into<String>, outcome: &str) {
        self.items.push(Check {
            name: name.into(),
            outcome: CheckOutcome::parse(outcome),
        });
    }
}

/// A file touched by the request; line counts are `None` where the API does not report them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

/// Everything shown in the details pane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrDetails {
    pub pr: PullRequest,
    pub body: String,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub checks: Checks,
    pub files: Vec<ChangedFile>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// Mergeability verdict in words (`mergeable`, `conflicting`, `blocked`…).
    pub mergeable: Option<String>,
    /// Number of changed files when per-file data is not available.
    pub changed_files: Option<u64>,
}

impl PrDetails {
    /// Sum file stats into the totals when the API only reports per-file numbers.
    fn fill_totals(&mut self) {
        let all_counted = self.files.iter().all(|f| f.additions.is_some());
        if self.additions.is_none() && all_counted && !self.files.is_empty() {
            self.additions = Some(self.files.iter().filter_map(|f| f.additions).sum());
            self.deletions = Some(self.files.iter().filter_map(|f| f.deletions).sum());
        }
        if self.changed_files.is_none() && !self.files.is_empty() {
            self.changed_files = Some(self.files.len() as u64);
        }
        if self.pr.review.is_none() {
            self.pr.review = ReviewDecision::aggregate(&self.reviews);
        }
    }
}

/// How to merge a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Merge => "a merge commit",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

/// Kind of review to submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewKind {
    pub fn label(self) -> &'static str {
        match self {
            ReviewKind::Approve => "Approve",
            ReviewKind::RequestChanges => "Request changes",
            ReviewKind::Comment => "Comment",
        }
    }
}

/// Parameters of a new pull / merge request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPr {
    pub title: String,
    pub body: String,
    /// Target branch.
    pub base: String,
    /// Source branch name on the push remote.
    pub head: String,
    /// The fork the branch lives in, when it is not the target repository.
    pub head_repo: Option<RemoteUrl>,
    pub draft: bool,
}

/// A request that was just created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub number: u64,
    pub url: String,
}

/// One provider's pull request API. Operations the provider does not offer return an error
/// explaining so, instead of being hidden.
pub trait PrProvider: Send + Sync {
    fn list(&self, filter: StateFilter) -> Result<Vec<PullRequest>>;
    fn details(&self, number: u64) -> Result<PrDetails>;
    fn create(&self, pr: &NewPr) -> Result<Created>;
    fn comment(&self, number: u64, body: &str) -> Result<()>;
    /// Submit a review; returns a short description of what was done.
    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> Result<String>;
    fn merge(&self, pr: &PullRequest, method: MergeMethod, delete_branch: bool) -> Result<()>;
    /// Close (decline / abandon) or reopen.
    fn set_state(&self, pr: &PullRequest, reopen: bool) -> Result<()>;
    fn default_branch(&self) -> Result<String>;
    /// Login of the authenticated user (checks a token).
    fn current_user(&self) -> Result<String>;
}

/// Error for an operation a provider has no API for.
pub fn unsupported(kind: ProviderKind, what: &str) -> anyhow::Error {
    anyhow!("{} does not support {what} through its API", kind.name())
}

/// Error for a review/comment that needs text.
fn need_text(what: &str) -> anyhow::Error {
    anyhow!("{what} needs some text")
}

/// The API client for `h`, or `None` for a generic host.
pub fn client(
    h: &Hosting,
    creds: Option<&Credentials>,
    transport: Arc<dyn Transport>,
) -> Option<Box<dyn PrProvider>> {
    let base = h.api_base.clone()?;
    let api = http::Api {
        transport,
        provider: h.kind.name(),
        host: h.url.credential_host(),
        auth: creds.map(|c| auth::auth_header(h.kind, c)),
        headers: Vec::new(),
    };
    Some(match h.kind {
        ProviderKind::GitHub => Box::new(github::GitHub::new(api, base, h)),
        ProviderKind::GitLab => Box::new(gitlab::GitLab::new(api, base, h)),
        ProviderKind::BitbucketCloud => Box::new(bitbucket::BitbucketCloud::new(api, base, h)),
        ProviderKind::BitbucketServer => {
            Box::new(bitbucket_server::BitbucketServer::new(api, base, h))
        }
        ProviderKind::Azure => Box::new(azure::Azure::new(api, base, h)),
        ProviderKind::Gitea => Box::new(gitea::Gitea::new(api, base, h)),
        ProviderKind::Generic => return None,
    })
}

/// An API client plus where its credentials came from.
pub struct Connection {
    pub hosting: Hosting,
    /// `None` when not signed in (public read access only).
    pub credentials: Option<Credentials>,
    pub api: Arc<dyn PrProvider>,
}

impl Connection {
    pub fn signed_in(&self) -> bool {
        self.credentials.is_some()
    }
}

/// Resolve credentials and build the API client (runs `git credential fill`: call off the UI thread).
pub fn connect(repo: &Repo, h: &Hosting, cfg: &HostingConfig) -> Result<Connection> {
    connect_with(repo, h, cfg, UreqTransport::shared())
}

pub fn connect_with(
    repo: &Repo,
    h: &Hosting,
    cfg: &HostingConfig,
    transport: Arc<dyn Transport>,
) -> Result<Connection> {
    let credentials = auth::resolve(repo, h, cfg);
    let api = client(h, credentials.as_ref(), transport).ok_or_else(|| {
        anyhow!(
            "{} has no known pull request API. If it runs GitHub, GitLab, Bitbucket, Azure DevOps or \
             Gitea/Forgejo, add it under [[hosting.hosts]] in {}",
            h.url.host,
            crate::config::Config::path().display()
        )
    })?;
    Ok(Connection {
        hosting: h.clone(),
        credentials,
        api: Arc::from(api),
    })
}

/// Request number from `12`, `#12`, `!12`, `PR-12` or a request web URL of any provider.
pub fn parse_pr_ref(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches('/');
    let s = if s.contains("://") {
        s.split(['?', '#']).next().unwrap_or(s)
    } else {
        s
    };
    let plain = s
        .trim_start_matches(['#', '!'])
        .trim_start_matches("PR-")
        .trim_start_matches("pr-");
    if let Ok(n) = plain.parse() {
        return Some(n);
    }
    if !s.contains("://") {
        return None;
    }
    for marker in [
        "/pull/",
        "/pulls/",
        "/merge_requests/",
        "/pull-requests/",
        "/pullrequest/",
    ] {
        if let Some(i) = s.rfind(marker) {
            let digits: String = s[i + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = digits.parse() {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn hosting(kind: ProviderKind, url: &str) -> Hosting {
        let parsed = parse_remote_url(url).expect("url");
        Hosting::new(kind, "origin", url, parsed, None, DetectedBy::KnownHost)
    }

    #[test]
    fn api_bases_per_provider() {
        let base = |k, u| hosting(k, u).api_base.unwrap_or_default();
        assert_eq!(
            base(ProviderKind::GitHub, "git@github.com:o/r.git"),
            "https://api.github.com"
        );
        assert_eq!(
            base(ProviderKind::GitHub, "https://ghe.corp.com/o/r"),
            "https://ghe.corp.com/api/v3"
        );
        assert_eq!(
            base(
                ProviderKind::GitLab,
                "https://gitlab.example.com:8443/g/s/p"
            ),
            "https://gitlab.example.com:8443/api/v4"
        );
        assert_eq!(
            base(ProviderKind::BitbucketCloud, "git@bitbucket.org:t/r.git"),
            "https://api.bitbucket.org/2.0"
        );
        assert_eq!(
            base(
                ProviderKind::BitbucketServer,
                "https://bb.corp/bitbucket/scm/P/r.git"
            ),
            "https://bb.corp/bitbucket/rest/api/1.0"
        );
        assert_eq!(
            base(
                ProviderKind::Azure,
                "https://dev.azure.com/org/proj/_git/repo"
            ),
            "https://dev.azure.com/org"
        );
        assert_eq!(
            base(
                ProviderKind::Azure,
                "https://org.visualstudio.com/proj/_git/repo"
            ),
            "https://org.visualstudio.com"
        );
        assert_eq!(
            base(ProviderKind::Gitea, "https://codeberg.org/a/b"),
            "https://codeberg.org/api/v1"
        );
        assert!(hosting(ProviderKind::Generic, "https://git.sr.ht/~me/x")
            .api_base
            .is_none());
        let p = parse_remote_url("https://git.corp/a/b").unwrap();
        let h = Hosting::new(
            ProviderKind::GitLab,
            "origin",
            "https://git.corp/a/b",
            p,
            Some("https://api.corp/gitlab/api/v4/"),
            DetectedBy::Config,
        );
        assert_eq!(
            h.api_base.as_deref(),
            Some("https://api.corp/gitlab/api/v4")
        );
    }

    #[test]
    fn names_and_references() {
        let h = hosting(
            ProviderKind::Azure,
            "https://dev.azure.com/org/proj/_git/repo",
        );
        assert_eq!(h.display(), "Azure DevOps · dev.azure.com/org/proj/repo");
        assert_eq!(h.reference(5), "!5");
        assert_eq!(
            hosting(ProviderKind::GitHub, "git@github.com:o/r").reference(5),
            "#5"
        );
        assert_eq!(ProviderKind::GitLab.noun(), "merge request");
        for k in ProviderKind::WITH_API {
            assert_eq!(ProviderKind::from_config(k.config_name()), Some(k));
            assert!(!k.token_help().is_empty());
        }
        assert_eq!(
            ProviderKind::from_config("Forgejo"),
            Some(ProviderKind::Gitea)
        );
        assert_eq!(
            ProviderKind::from_config("Bitbucket Data Center"),
            Some(ProviderKind::BitbucketServer)
        );
        assert_eq!(
            ProviderKind::from_config("none"),
            Some(ProviderKind::Generic)
        );
        assert_eq!(ProviderKind::from_config("svn"), None);
        assert_eq!(
            hosting(ProviderKind::GitHub, "git@github.com:octo/hello.git").clone_url_of("me/hello"),
            "git@github.com:me/hello.git"
        );
    }

    #[test]
    fn parses_pr_refs() {
        for (s, n) in [
            ("12", 12),
            ("#12", 12),
            ("!12", 12),
            (" PR-7 ", 7),
            ("https://github.com/o/r/pull/42", 42),
            ("https://github.com/o/r/pull/42/files", 42),
            ("https://gitlab.com/g/p/-/merge_requests/9#note_1", 9),
            ("https://bitbucket.org/t/r/pull-requests/3/diff", 3),
            (
                "https://bb.corp/projects/P/repos/r/pull-requests/8/overview",
                8,
            ),
            (
                "https://dev.azure.com/o/p/_git/r/pullrequest/77?_a=files",
                77,
            ),
            ("https://codeberg.org/a/b/pulls/5", 5),
        ] {
            assert_eq!(parse_pr_ref(s), Some(n), "{s}");
        }
        assert_eq!(parse_pr_ref("abc"), None);
        assert_eq!(parse_pr_ref("https://github.com/o/r"), None);
    }

    #[test]
    fn review_aggregation_and_check_outcomes() {
        let r = |a: &str, s: &str| Review {
            author: a.into(),
            state: s.into(),
            body: String::new(),
            submitted: None,
        };
        assert_eq!(
            ReviewDecision::aggregate(&[r("a", "CHANGES_REQUESTED"), r("a", "APPROVED")]),
            Some(ReviewDecision::Approved)
        );
        assert_eq!(
            ReviewDecision::aggregate(&[r("a", "APPROVED"), r("b", "CHANGES_REQUESTED")]),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(ReviewDecision::aggregate(&[r("a", "COMMENTED")]), None);
        assert_eq!(
            ReviewDecision::aggregate(&[r("a", "REQUESTED")]),
            Some(ReviewDecision::ReviewRequired)
        );
        assert_eq!(CheckOutcome::parse("SUCCESSFUL"), CheckOutcome::Success);
        assert_eq!(CheckOutcome::parse("timed_out"), CheckOutcome::Failure);
        assert_eq!(CheckOutcome::parse("INPROGRESS"), CheckOutcome::Pending);
        assert_eq!(CheckOutcome::parse("notApplicable"), CheckOutcome::Skipped);
        assert!(StateFilter::Open.accepts(PrState::Locked));
        assert!(!StateFilter::Closed.accepts(PrState::Merged));
        assert_eq!(StateFilter::All.next(), StateFilter::Open);
    }
}
