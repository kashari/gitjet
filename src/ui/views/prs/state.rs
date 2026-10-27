//! Background loading for the Pull Requests tab.
//!
//! Every network call (provider probe, credential lookup, list, details, default branch) runs on a
//! plain `std::thread` that writes into [`Shared`] behind a mutex; the view only copies results out
//! while drawing, so the UI never blocks on the network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::config::HostingConfig;
use crate::git::hosting::{
    self, auth, detect, Connection, Detection, Hosting, PrDetails, PullRequest, StateFilter,
    Transport, UreqTransport,
};
use crate::git::Repo;

/// Whether pull requests can be listed, and why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// First check still running.
    Detecting,
    NoRemotes,
    /// Only local-path remotes.
    NotHosted {
        remote: String,
        url: String,
    },
    /// The host is known but requests cannot be listed: a generic host, or the API refused
    /// anonymous access. Browser links and git-based checkout still work.
    Limited {
        hosting: Hosting,
        reason: String,
    },
    Ready(Hosting),
}

impl Setup {
    pub fn hosting(&self) -> Option<&Hosting> {
        match self {
            Setup::Limited { hosting, .. } | Setup::Ready(hosting) => Some(hosting),
            _ => None,
        }
    }

    pub fn ready(&self) -> Option<&Hosting> {
        match self {
            Setup::Ready(h) => Some(h),
            _ => None,
        }
    }
}

/// Cached details of one request.
#[derive(Debug, Default)]
pub struct DetailEntry {
    pub data: Option<Result<Arc<PrDetails>, String>>,
    pub loading: bool,
    /// False once a refresh made the data stale; it is reloaded when next shown.
    pub fresh: bool,
}

/// How much a reload re-checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reload {
    /// Reuse the connection (credentials) when the hosting is unchanged.
    Soft,
    /// Probe unknown servers again, look up credentials again, drop cached details.
    Full,
}

/// State written by loader threads.
pub struct Shared {
    pub setup: Setup,
    pub list: Option<Result<Vec<PullRequest>, String>>,
    pub list_loading: bool,
    /// Bumped per list request; results of superseded requests are dropped.
    pub list_gen: u64,
    /// Bumped whenever `list` changes, so the view knows to copy it.
    pub list_version: u64,
    pub default_branch: Option<String>,
    pub details: HashMap<u64, DetailEntry>,
    /// API client with resolved credentials, and the credential generation it was built at.
    pub conn: Option<(Arc<Connection>, u64)>,
    /// Where the credentials came from (`None`: not signed in).
    pub auth_source: Option<String>,
    /// Network transport (faked in tests).
    pub transport: Arc<dyn Transport>,
    /// Transport for probing unknown servers (shorter timeouts).
    pub probe_transport: Arc<dyn Transport>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            setup: Setup::Detecting,
            list: None,
            list_loading: false,
            list_gen: 0,
            list_version: 0,
            default_branch: None,
            details: HashMap::new(),
            conn: None,
            auth_source: None,
            transport: UreqTransport::shared(),
            probe_transport: Arc::new(UreqTransport::with_timeouts(
                Duration::from_secs(5),
                Duration::from_secs(10),
            )),
        }
    }
}

pub type SharedRef = Arc<Mutex<Shared>>;

/// Lock, recovering from a poisoned mutex (a panicking loader must not take the UI down).
pub fn lock(s: &SharedRef) -> MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// The connection for `h`, reusing the cached one while the hosting and stored credentials are
/// unchanged. Errors only for hosts without an API.
fn connection(
    shared: &SharedRef,
    repo: &Repo,
    h: &Hosting,
    cfg: &HostingConfig,
) -> Result<Arc<Connection>, String> {
    let gen = auth::generation();
    let (cached, transport) = {
        let g = lock(shared);
        let cached = g
            .conn
            .as_ref()
            .filter(|(c, cgen)| c.hosting == *h && *cgen == gen)
            .map(|(c, _)| c.clone());
        (cached, g.transport.clone())
    };
    if let Some(c) = cached {
        return Ok(c);
    }
    let conn =
        Arc::new(hosting::connect_with(repo, h, cfg, transport).map_err(|e| format!("{e:#}"))?);
    lock(shared).conn = Some((conn.clone(), gen));
    Ok(conn)
}

/// Reload the list in the background.
pub fn start_list_load(
    shared: &SharedRef,
    repo: Repo,
    cfg: HostingConfig,
    filter: StateFilter,
    mode: Reload,
) {
    let gen = {
        let mut g = lock(shared);
        g.list_gen += 1;
        g.list_loading = true;
        if mode == Reload::Full {
            g.conn = None;
            g.default_branch = None;
            g.details.clear();
        } else {
            g.details.values_mut().for_each(|e| e.fresh = false);
        }
        g.list_gen
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        if mode == Reload::Full {
            detect::clear_cache(&repo);
        }
        let probe = lock(&shared).probe_transport.clone();
        let detection = hosting::detect(&repo, &cfg, &*probe);
        let finish = |setup: Setup, list: Option<Result<Vec<PullRequest>, String>>| {
            let mut g = lock(&shared);
            if g.list_gen != gen {
                return false;
            }
            if g.setup.hosting() != setup.hosting() {
                g.details.clear();
                g.default_branch = None;
            }
            g.setup = setup;
            g.list = list;
            g.list_loading = false;
            g.list_version += 1;
            true
        };
        let h = match detection {
            Detection::NoRemotes => {
                finish(Setup::NoRemotes, None);
                return;
            }
            Detection::NotHosted { remote, url } => {
                finish(Setup::NotHosted { remote, url }, None);
                return;
            }
            Detection::Found(h) => h,
        };
        if !h.kind.has_api() {
            let reason = format!("No pull request API is known for {}.", h.url.host);
            finish(Setup::Limited { hosting: h, reason }, None);
            return;
        }
        let conn = match connection(&shared, &repo, &h, &cfg) {
            Ok(c) => c,
            Err(reason) => {
                finish(Setup::Limited { hosting: h, reason }, None);
                return;
            }
        };
        lock(&shared).auth_source = conn.credentials.as_ref().map(|c| c.source.clone());
        let result = conn.api.list(filter).map_err(|e| format!("{e:#}"));
        let ok = match result {
            Err(reason) if !conn.signed_in() => finish(
                Setup::Limited {
                    hosting: h.clone(),
                    reason,
                },
                None,
            ),
            r => finish(Setup::Ready(h.clone()), Some(r)),
        };
        if ok && lock(&shared).default_branch.is_none() {
            let branch = conn
                .api
                .default_branch()
                .ok()
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| hosting::local_default_branch(&repo, &h.remote));
            lock(&shared).default_branch = Some(branch);
        }
    });
}

/// Load one request's details in the background (keeps showing stale data meanwhile).
pub fn start_details_load(shared: &SharedRef, hosting: Hosting, number: u64) {
    let conn = {
        let mut g = lock(shared);
        let conn = g
            .conn
            .as_ref()
            .filter(|(c, _)| c.hosting == hosting)
            .map(|(c, _)| c.clone());
        let e = g.details.entry(number).or_default();
        if e.loading || conn.is_none() {
            return;
        }
        e.loading = true;
        e.fresh = true;
        conn
    };
    let Some(conn) = conn else { return };
    let shared = shared.clone();
    std::thread::spawn(move || {
        let r = conn
            .api
            .details(number)
            .map(Arc::new)
            .map_err(|e| format!("{e:#}"));
        let mut g = lock(&shared);
        if g.setup.hosting() != Some(&hosting) {
            return;
        }
        let e = g.details.entry(number).or_default();
        e.data = Some(r);
        e.loading = false;
    });
}

/// What the details pane can show for one request.
pub struct DetailsView {
    pub data: Option<Result<Arc<PrDetails>, String>>,
    pub loading: bool,
    /// Needs a (re)load: never loaded, or stale and not loading.
    pub wanted: bool,
}

/// Snapshot of the cached details for `number`.
pub fn details_of(shared: &SharedRef, number: u64) -> DetailsView {
    let g = lock(shared);
    match g.details.get(&number) {
        Some(e) => DetailsView {
            data: e.data.clone(),
            loading: e.loading,
            wanted: !e.loading && (!e.fresh || e.data.is_none()),
        },
        None => DetailsView {
            data: None,
            loading: false,
            wanted: true,
        },
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::hosting::ProviderKind;
    use crate::git::testutil::temp_repo;
    use std::time::Instant;

    pub fn wait_until(shared: &SharedRef, cond: impl Fn(&Shared) -> bool) {
        let start = Instant::now();
        while !cond(&lock(shared)) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out waiting for loader"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Shared state whose network goes to `t`.
    pub fn shared_with(t: Arc<FakeTransport>) -> SharedRef {
        let s = Shared {
            transport: t.clone(),
            probe_transport: t,
            ..Shared::default()
        };
        Arc::new(Mutex::new(s))
    }

    fn no_creds_cfg() -> HostingConfig {
        HostingConfig {
            use_git_credentials: false,
            hosts: Vec::new(),
        }
    }

    #[test]
    fn no_remotes_resolves_without_network() {
        let (_d, repo) = temp_repo();
        let t = FakeTransport::new();
        let shared = shared_with(t.clone());
        start_list_load(
            &shared,
            repo,
            no_creds_cfg(),
            StateFilter::Open,
            Reload::Soft,
        );
        wait_until(&shared, |g| !g.list_loading);
        let g = lock(&shared);
        assert_eq!(g.setup, Setup::NoRemotes);
        assert!(g.list.is_none());
        assert!(g.list_version > 0);
        assert!(t.log().is_empty());
    }

    #[test]
    fn generic_host_is_limited_after_probe() {
        let (_d, repo) = temp_repo();
        repo.run(&[
            "remote",
            "add",
            "origin",
            "https://git.plain.example/a/b.git",
        ])
        .unwrap();
        let t = FakeTransport::new();
        t.on("GET", "git.plain.example", 200, "<html></html>");
        let shared = shared_with(t.clone());
        start_list_load(
            &shared,
            repo,
            no_creds_cfg(),
            StateFilter::All,
            Reload::Full,
        );
        wait_until(&shared, |g| !g.list_loading);
        let g = lock(&shared);
        match &g.setup {
            Setup::Limited { hosting, reason } => {
                assert_eq!(hosting.kind, ProviderKind::Generic);
                assert!(reason.contains("No pull request API"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(t.log().len(), 4, "four API probes");
    }

    #[test]
    fn public_repository_lists_without_token_and_private_one_is_limited() {
        if std::env::var("GITJET_TOKEN").is_ok() || std::env::var("GITEA_TOKEN").is_ok() {
            return;
        }
        let (_d, repo) = temp_repo();
        repo.run(&["remote", "add", "origin", "https://codeberg.org/a/b.git"])
            .unwrap();
        let t = FakeTransport::new();
        t.on(
            "GET",
            "/repos/a/b/pulls?state=open",
            200,
            r#"[{"number":1,"title":"One","state":"open","user":{"login":"x"},"head":{"ref":"f"},"base":{"ref":"main"}}]"#,
        )
        .on("GET", "/repos/a/b", 200, r#"{"default_branch":"trunk"}"#);
        let shared = shared_with(t.clone());
        start_list_load(
            &shared,
            repo.clone(),
            no_creds_cfg(),
            StateFilter::Open,
            Reload::Soft,
        );
        wait_until(&shared, |g| !g.list_loading && g.default_branch.is_some());
        {
            let g = lock(&shared);
            assert!(matches!(g.setup, Setup::Ready(_)));
            assert_eq!(g.list.as_ref().unwrap().as_ref().unwrap().len(), 1);
            assert_eq!(g.default_branch.as_deref(), Some("trunk"));
            assert!(g.auth_source.is_none());
        }
        // Details come from the cached connection.
        let h = lock(&shared).setup.hosting().cloned().unwrap();
        t.on(
            "GET",
            "/repos/a/b/pulls/1",
            200,
            r#"{"number":1,"title":"One","body":"Hi"}"#,
        );
        start_details_load(&shared, h, 1);
        wait_until(&shared, |g| {
            g.details.get(&1).is_some_and(|e| e.data.is_some())
        });
        assert!(matches!(details_of(&shared, 1).data, Some(Ok(_))));

        // A private repository answers 404 without a token: limited mode with the reason.
        let t2 = FakeTransport::new();
        let shared2 = shared_with(t2);
        start_list_load(
            &shared2,
            repo,
            no_creds_cfg(),
            StateFilter::Open,
            Reload::Soft,
        );
        wait_until(&shared2, |g| !g.list_loading);
        let setup = lock(&shared2).setup.clone();
        match setup {
            Setup::Limited { reason, .. } => assert!(reason.contains("404"), "{reason}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn details_bookkeeping() {
        let shared: SharedRef = Arc::default();
        assert!(details_of(&shared, 3).wanted);
        lock(&shared).details.insert(
            3,
            DetailEntry {
                data: Some(Err("boom".into())),
                loading: false,
                fresh: true,
            },
        );
        assert!(!details_of(&shared, 3).wanted);
        lock(&shared)
            .details
            .values_mut()
            .for_each(|e| e.fresh = false);
        assert!(details_of(&shared, 3).wanted);
    }
}
