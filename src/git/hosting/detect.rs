//! Which provider serves a repository's remote.
//!
//! Order: `[[hosting.hosts]]` config entries, well-known hosts, host-name / path hints, the cached
//! result of an earlier API probe (`<git dir>/gitjet/hosting.json`), and finally — on a background
//! thread only — an API probe of the unknown server. Anything else is a generic host: no pull
//! request API, but browser links still work.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::HostingConfig;
use crate::git::Repo;

use super::http::{HttpRequest, Transport};
use super::{parse_remote_url, Hosting, ProviderKind, RemoteUrl};

/// How the provider was determined (shown on the setup screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedBy {
    Config,
    KnownHost,
    HostName,
    Probe,
    /// Nothing matched: generic host.
    Fallback,
}

impl DetectedBy {
    pub fn label(self) -> &'static str {
        match self {
            DetectedBy::Config => "from [[hosting.hosts]] in the config file",
            DetectedBy::KnownHost => "well-known host",
            DetectedBy::HostName => "guessed from the host name",
            DetectedBy::Probe => "detected by probing the server's API",
            DetectedBy::Fallback => "no known pull request API found",
        }
    }
}

/// Result of looking at the remotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    NoRemotes,
    /// Remotes exist but none is a network URL (local paths, `file://`).
    NotHosted {
        remote: String,
        url: String,
    },
    Found(Hosting),
}

impl Detection {
    #[cfg(test)]
    pub fn hosting(&self) -> Option<&Hosting> {
        match self {
            Detection::Found(h) => Some(h),
            _ => None,
        }
    }
}

/// Provider of a host from config, known hosts and hints; `None` when only a probe can tell.
pub fn classify(
    url: &RemoteUrl,
    cfg: &HostingConfig,
) -> Option<(ProviderKind, Option<String>, DetectedBy)> {
    let host = url.host.as_str();
    let with_port = url.credential_host();
    for e in &cfg.hosts {
        let h = e.host.trim().to_ascii_lowercase();
        let h = h
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        if h == host || h == with_port {
            if let Some(kind) = ProviderKind::from_config(&e.kind) {
                return Some((kind, e.api_url.clone(), DetectedBy::Config));
            }
        }
    }
    if let Some(kind) = known_host(host) {
        return Some((kind, None, DetectedBy::KnownHost));
    }
    hint(url).map(|k| (k, None, DetectedBy::HostName))
}

fn known_host(host: &str) -> Option<ProviderKind> {
    Some(match host {
        "github.com" => ProviderKind::GitHub,
        "gitlab.com"
        | "salsa.debian.org"
        | "invent.kde.org"
        | "framagit.org"
        | "gitlab.freedesktop.org"
        | "gitlab.gnome.org" => ProviderKind::GitLab,
        "bitbucket.org" => ProviderKind::BitbucketCloud,
        "dev.azure.com" | "ssh.dev.azure.com" => ProviderKind::Azure,
        h if h.ends_with(".visualstudio.com") => ProviderKind::Azure,
        "codeberg.org" | "gitea.com" | "next.forgejo.org" | "git.disroot.org" => {
            ProviderKind::Gitea
        }
        "git.sr.ht" | "git.kernel.org" | "repo.or.cz" => ProviderKind::Generic,
        _ => return None,
    })
}

fn hint(url: &RemoteUrl) -> Option<ProviderKind> {
    let h = url.host.as_str();
    if h.contains("github") {
        Some(ProviderKind::GitHub)
    } else if h.contains("gitlab") {
        Some(ProviderKind::GitLab)
    } else if h.contains("gitea") || h.contains("forgejo") || h.contains("codeberg") {
        Some(ProviderKind::Gitea)
    } else if h.contains("bitbucket") || h.contains("stash") {
        Some(ProviderKind::BitbucketServer)
    } else if h.contains("azure") || h.contains("tfs") {
        Some(ProviderKind::Azure)
    } else {
        None
    }
}

/// Remote names in preference order: `upstream` (fork workflows), `origin`, then the rest.
fn rank(name: &str) -> u8 {
    match name {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    }
}

/// What to do with the remotes, before any network access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Done(Detection),
    /// The best remote is on an unknown server: probe `url.web_base`.
    Probe {
        remote: String,
        fetch_url: String,
        url: RemoteUrl,
    },
}

/// Pick the remote and provider. A remote on a known provider wins over unknown ones; among
/// equals the preference order of [`rank`] applies. `cache` holds earlier probe results
/// (host → config kind).
pub fn choose(
    remotes: &[(String, String)],
    cfg: &HostingConfig,
    cache: &BTreeMap<String, String>,
) -> Choice {
    if remotes.is_empty() {
        return Choice::Done(Detection::NoRemotes);
    }
    let mut ordered: Vec<&(String, String)> = remotes.iter().collect();
    ordered.sort_by_key(|(name, _)| rank(name));
    let parsed: Vec<(&String, &String, RemoteUrl)> = ordered
        .iter()
        .filter_map(|(n, u)| parse_remote_url(u).map(|p| (n, u, p)))
        .collect();
    let Some(first) = parsed.first() else {
        let (remote, url) = ordered[0].clone();
        return Choice::Done(Detection::NotHosted { remote, url });
    };
    let resolve = |url: &RemoteUrl| {
        classify(url, cfg).or_else(|| {
            cache
                .get(&url.credential_host())
                .and_then(|k| ProviderKind::from_config(k))
                .map(|k| (k, None, DetectedBy::Probe))
        })
    };
    let found =
        |(name, fetch, url): &(&String, &String, RemoteUrl), kind, api: Option<String>, by| {
            let by = if kind == ProviderKind::Generic && by == DetectedBy::Probe {
                DetectedBy::Fallback
            } else {
                by
            };
            Choice::Done(Detection::Found(Hosting::new(
                kind,
                name,
                fetch,
                url.clone(),
                api.as_deref(),
                by,
            )))
        };
    for p in &parsed {
        if let Some((kind, api, by)) = resolve(&p.2) {
            if kind.has_api() {
                return found(p, kind, api, by);
            }
        }
    }
    match resolve(&first.2) {
        Some((kind, api, by)) => found(first, kind, api, by),
        None => Choice::Probe {
            remote: first.0.clone(),
            fetch_url: first.1.clone(),
            url: first.2.clone(),
        },
    }
}

/// Outcome of probing an unknown server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Found(ProviderKind),
    /// The server answered, but none of the known APIs.
    NothingFound,
    /// The server could not be reached (offline, DNS…); nothing is cached.
    Unreachable(String),
}

/// Ask the server which API it speaks: GitLab (`/api/v4/version`), Gitea/Forgejo
/// (`/api/v1/version`), Bitbucket Server (`/rest/api/1.0/application-properties`) and GitHub
/// Enterprise (`/api/v3/meta`). Requests are anonymous.
pub fn probe(transport: &dyn Transport, web_base: &str) -> ProbeResult {
    let base = web_base.trim_end_matches('/');
    let probes: [(&str, ProviderKind); 4] = [
        ("/api/v4/version", ProviderKind::GitLab),
        ("/api/v1/version", ProviderKind::Gitea),
        (
            "/rest/api/1.0/application-properties",
            ProviderKind::BitbucketServer,
        ),
        ("/api/v3/meta", ProviderKind::GitHub),
    ];
    let mut reached = false;
    let mut last_error = String::new();
    for (path, kind) in probes {
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("{base}{path}"),
            headers: vec![
                ("Accept".into(), "application/json".into()),
                ("User-Agent".into(), super::http::user_agent()),
            ],
            body: None,
        };
        let resp = match transport.send(&req) {
            Ok(r) => r,
            Err(e) => {
                last_error = format!("{e:#}");
                continue;
            }
        };
        reached = true;
        let v: serde_json::Value = serde_json::from_str(resp.body.trim()).unwrap_or_default();
        let hit = match kind {
            ProviderKind::GitLab => {
                (resp.ok() && v.get("version").is_some())
                    || resp.header("x-gitlab-meta").is_some()
                    || (resp.status == 401 && super::json::s(&v, "message").starts_with("401"))
            }
            ProviderKind::Gitea => resp.ok() && v.get("version").is_some(),
            ProviderKind::BitbucketServer => {
                resp.ok()
                    && (super::json::s(&v, "displayName").contains("Bitbucket")
                        || v.get("buildNumber").is_some())
            }
            ProviderKind::GitHub => {
                resp.ok()
                    && (v.get("installed_version").is_some()
                        || v.get("verifiable_password_authentication").is_some())
            }
            _ => false,
        };
        if hit {
            return ProbeResult::Found(kind);
        }
    }
    if reached {
        ProbeResult::NothingFound
    } else {
        ProbeResult::Unreachable(last_error)
    }
}

/// (name, fetch URL) of every remote.
pub fn remotes(repo: &Repo) -> Vec<(String, String)> {
    let out = repo.try_run(&["remote", "-v"]).unwrap_or_default();
    let mut v: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some(url) = rest.strip_suffix(" (fetch)") else {
            continue;
        };
        if !v.iter().any(|(n, _)| n == name) {
            v.push((name.to_string(), url.trim().to_string()));
        }
    }
    v
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    hosts: BTreeMap<String, String>,
}

fn cache_path(repo: &Repo) -> Option<std::path::PathBuf> {
    repo.data_dir().ok().map(|d| d.join("hosting.json"))
}

/// Probe results remembered for this repository (host → kind).
pub fn load_cache(repo: &Repo) -> BTreeMap<String, String> {
    cache_path(repo)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
        .map(|c| c.hosts)
        .unwrap_or_default()
}

fn save_cache(repo: &Repo, host: &str, kind: ProviderKind) {
    let Some(path) = cache_path(repo) else { return };
    let mut c = Cache {
        hosts: load_cache(repo),
    };
    c.hosts
        .insert(host.to_string(), kind.config_name().to_string());
    if let Ok(text) = serde_json::to_string_pretty(&c) {
        let _ = std::fs::write(path, text);
    }
}

/// Forget cached probe results (a forced reload probes again).
pub fn clear_cache(repo: &Repo) {
    if let Some(p) = cache_path(repo) {
        let _ = std::fs::remove_file(p);
    }
}

/// Local detection only (config, known hosts, hints, cache) — safe on the UI thread. An unknown
/// server that was never probed is treated as generic.
pub fn detect_local(repo: &Repo, cfg: &HostingConfig) -> Detection {
    match choose(&remotes(repo), cfg, &load_cache(repo)) {
        Choice::Done(d) => d,
        Choice::Probe {
            remote,
            fetch_url,
            url,
        } => Detection::Found(Hosting::new(
            ProviderKind::Generic,
            &remote,
            &fetch_url,
            url,
            None,
            DetectedBy::Fallback,
        )),
    }
}

/// Full detection, probing unknown servers through `transport` (network: background threads only).
pub fn detect(repo: &Repo, cfg: &HostingConfig, transport: &dyn Transport) -> Detection {
    match choose(&remotes(repo), cfg, &load_cache(repo)) {
        Choice::Done(d) => d,
        Choice::Probe {
            remote,
            fetch_url,
            url,
        } => {
            let (kind, by) = match probe(transport, &url.web_base) {
                ProbeResult::Found(k) => {
                    save_cache(repo, &url.credential_host(), k);
                    (k, DetectedBy::Probe)
                }
                ProbeResult::NothingFound => {
                    save_cache(repo, &url.credential_host(), ProviderKind::Generic);
                    (ProviderKind::Generic, DetectedBy::Fallback)
                }
                ProbeResult::Unreachable(_) => (ProviderKind::Generic, DetectedBy::Fallback),
            };
            Detection::Found(Hosting::new(kind, &remote, &fetch_url, url, None, by))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HostEntry;
    use crate::git::hosting::http::fake::FakeTransport;
    use crate::git::testutil::temp_repo;

    fn remotes_of(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn kind_of(c: Choice) -> (ProviderKind, String, DetectedBy) {
        match c {
            Choice::Done(Detection::Found(h)) => (h.kind, h.remote, h.detected_by),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn cfg(hosts: &[(&str, &str, Option<&str>)]) -> HostingConfig {
        HostingConfig {
            use_git_credentials: true,
            hosts: hosts
                .iter()
                .map(|(h, k, a)| HostEntry {
                    host: h.to_string(),
                    kind: k.to_string(),
                    api_url: a.map(str::to_string),
                })
                .collect(),
        }
    }

    #[test]
    fn known_hosts_and_hints() {
        let none = BTreeMap::new();
        let c = HostingConfig::default();
        for (url, kind, by) in [
            (
                "git@github.com:o/r.git",
                ProviderKind::GitHub,
                DetectedBy::KnownHost,
            ),
            (
                "https://gitlab.com/g/p",
                ProviderKind::GitLab,
                DetectedBy::KnownHost,
            ),
            (
                "git@bitbucket.org:t/r.git",
                ProviderKind::BitbucketCloud,
                DetectedBy::KnownHost,
            ),
            (
                "git@ssh.dev.azure.com:v3/o/p/r",
                ProviderKind::Azure,
                DetectedBy::KnownHost,
            ),
            (
                "https://o.visualstudio.com/p/_git/r",
                ProviderKind::Azure,
                DetectedBy::KnownHost,
            ),
            (
                "https://codeberg.org/a/b",
                ProviderKind::Gitea,
                DetectedBy::KnownHost,
            ),
            (
                "https://gitea.com/a/b",
                ProviderKind::Gitea,
                DetectedBy::KnownHost,
            ),
            (
                "https://github.corp.com/a/b",
                ProviderKind::GitHub,
                DetectedBy::HostName,
            ),
            (
                "git@gitlab.corp.com:a/b",
                ProviderKind::GitLab,
                DetectedBy::HostName,
            ),
            (
                "https://forgejo.home/a/b",
                ProviderKind::Gitea,
                DetectedBy::HostName,
            ),
            (
                "ssh://git@bitbucket.corp:7999/p/r.git",
                ProviderKind::BitbucketServer,
                DetectedBy::HostName,
            ),
            (
                "https://git.sr.ht/~me/r",
                ProviderKind::Generic,
                DetectedBy::KnownHost,
            ),
        ] {
            let got = kind_of(choose(&remotes_of(&[("origin", url)]), &c, &none));
            assert_eq!((got.0, got.2), (kind, by), "{url}");
        }
    }

    #[test]
    fn config_overrides_win() {
        let none = BTreeMap::new();
        let c = cfg(&[
            (
                "git.corp.com",
                "gitlab",
                Some("https://git.corp.com/gl/api/v4"),
            ),
            ("GITHUB.example.com", "gitea", None),
            ("github.com", "none", None),
            ("bad.example.com", "nonsense", None),
        ]);
        let pick = |u: &str| match choose(&remotes_of(&[("origin", u)]), &c, &none) {
            Choice::Done(Detection::Found(h)) => Some(h),
            _ => None,
        };
        let h = pick("git@git.corp.com:team/app.git").unwrap();
        assert_eq!(
            (h.kind, h.detected_by),
            (ProviderKind::GitLab, DetectedBy::Config)
        );
        assert_eq!(
            h.api_base.as_deref(),
            Some("https://git.corp.com/gl/api/v4")
        );
        assert_eq!(
            pick("https://github.example.com/a/b").unwrap().kind,
            ProviderKind::Gitea
        );
        assert_eq!(
            pick("https://github.com/a/b").unwrap().kind,
            ProviderKind::Generic
        );
        // An invalid kind is ignored; the host is still unknown and needs a probe.
        assert!(matches!(
            choose(
                &remotes_of(&[("origin", "https://bad.example.com/a/b")]),
                &c,
                &none
            ),
            Choice::Probe { .. }
        ));
    }

    #[test]
    fn remote_preference() {
        let none = BTreeMap::new();
        let c = HostingConfig::default();
        let r = remotes_of(&[
            ("fork", "git@github.com:me/r.git"),
            ("origin", "https://github.com/me/r.git"),
            ("upstream", "https://github.com/octo/r.git"),
        ]);
        assert_eq!(kind_of(choose(&r, &c, &none)).1, "upstream");
        // A known provider beats an unknown server, even if the unknown one is preferred by name.
        let r = remotes_of(&[
            ("upstream", "https://git.unknown.org/a/b"),
            ("origin", "git@gitlab.com:me/b.git"),
        ]);
        assert_eq!(kind_of(choose(&r, &c, &none)).1, "origin");
        // Only unknown servers: probe the preferred one.
        let r = remotes_of(&[
            ("mirror", "https://a.org/x/y"),
            ("origin", "https://b.org/x/y"),
        ]);
        match choose(&r, &c, &none) {
            Choice::Probe { remote, .. } => assert_eq!(remote, "origin"),
            other => panic!("{other:?}"),
        }
        // ...unless the cache already knows.
        let mut cache = BTreeMap::new();
        cache.insert("b.org".to_string(), "gitea".to_string());
        assert_eq!(
            kind_of(choose(&r, &c, &cache)),
            (ProviderKind::Gitea, "origin".into(), DetectedBy::Probe)
        );
        cache.insert("b.org".to_string(), "none".to_string());
        assert_eq!(kind_of(choose(&r, &c, &cache)).2, DetectedBy::Fallback);
        assert_eq!(choose(&[], &c, &none), Choice::Done(Detection::NoRemotes));
        assert!(matches!(
            choose(&remotes_of(&[("origin", "/srv/git/r.git")]), &c, &none),
            Choice::Done(Detection::NotHosted { .. })
        ));
    }

    #[test]
    fn probes_identify_servers() {
        let t = FakeTransport::new();
        t.on(
            "GET",
            "gl.corp/api/v4/version",
            401,
            r#"{"message":"401 Unauthorized"}"#,
        )
        .on(
            "GET",
            "tea.corp/api/v1/version",
            200,
            r#"{"version":"1.22.0"}"#,
        )
        .on(
            "GET",
            "bb.corp/rest/api/1.0/application-properties",
            200,
            r#"{"version":"8.19.1","buildNumber":"8019001","displayName":"Bitbucket"}"#,
        )
        .on(
            "GET",
            "ghe.corp/api/v3/meta",
            200,
            r#"{"verifiable_password_authentication":true,"installed_version":"3.12.0"}"#,
        )
        .on("GET", "plain.corp", 200, "<html>hello</html>");
        assert_eq!(
            probe(&*t, "https://gl.corp"),
            ProbeResult::Found(ProviderKind::GitLab)
        );
        assert_eq!(
            probe(&*t, "https://tea.corp/"),
            ProbeResult::Found(ProviderKind::Gitea)
        );
        assert_eq!(
            probe(&*t, "https://bb.corp"),
            ProbeResult::Found(ProviderKind::BitbucketServer)
        );
        assert_eq!(
            probe(&*t, "https://ghe.corp"),
            ProbeResult::Found(ProviderKind::GitHub)
        );
        assert_eq!(probe(&*t, "https://plain.corp"), ProbeResult::NothingFound);
        assert!(matches!(
            probe(&*t, "https://unreachable.corp"),
            ProbeResult::Unreachable(_)
        ));
    }

    #[test]
    fn detect_probes_once_and_caches() {
        let (_d, repo) = temp_repo();
        let c = HostingConfig::default();
        assert_eq!(detect_local(&repo, &c), Detection::NoRemotes);
        repo.run(&["remote", "add", "origin", "https://tea.corp/me/app.git"])
            .unwrap();
        // Locally the unknown server is generic until probed.
        assert_eq!(
            detect_local(&repo, &c).hosting().unwrap().kind,
            ProviderKind::Generic
        );
        let t = FakeTransport::new();
        t.on(
            "GET",
            "tea.corp/api/v1/version",
            200,
            r#"{"version":"9.0.0"}"#,
        );
        let h = detect(&repo, &c, &*t).hosting().cloned().unwrap();
        assert_eq!(
            (h.kind, h.detected_by),
            (ProviderKind::Gitea, DetectedBy::Probe)
        );
        let probes = t.log().len();
        assert_eq!(
            load_cache(&repo).get("tea.corp").map(String::as_str),
            Some("gitea")
        );
        assert_eq!(
            detect(&repo, &c, &*t).hosting().unwrap().kind,
            ProviderKind::Gitea
        );
        assert_eq!(t.log().len(), probes, "cached result must not probe again");
        assert_eq!(
            detect_local(&repo, &c).hosting().unwrap().kind,
            ProviderKind::Gitea
        );
        clear_cache(&repo);
        assert!(load_cache(&repo).is_empty());
    }
}
