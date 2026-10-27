//! Parsing Git remote URLs into host / owner / repository, for every common format:
//! `https://[user[:pw]@]host[:port]/owner/repo(.git)`, `ssh://[user@]host[:port]/owner/repo`,
//! `git://…`, scp-like `[user@]host:owner/repo`, plus Azure DevOps (`/_git/` paths,
//! `ssh.dev.azure.com:v3/…`, `org.visualstudio.com`) and Bitbucket Server (`/scm/` paths).

/// The parts of a remote URL that identify a hosted repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    /// `https`, `http`, `ssh` or `git`.
    pub scheme: String,
    /// Lower-cased host name without port or user.
    pub host: String,
    /// Port of the URL (for http(s) it is also part of the web address).
    pub port: Option<u16>,
    /// Owner / namespace; may contain `/` (GitLab subgroups, Azure `org/project`).
    pub owner: String,
    /// Repository name without `.git`.
    pub name: String,
    /// `https://host[:port][/prefix]` for the web UI.
    pub web_base: String,
}

impl RemoteUrl {
    /// `owner/name`.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `host[:port]` as git's credential protocol wants it (port only for http(s)).
    pub fn credential_host(&self) -> String {
        match (self.scheme.as_str(), self.port) {
            ("http" | "https", Some(p)) => format!("{}:{p}", self.host),
            _ => self.host.clone(),
        }
    }

    /// `http` or `https`, as used by the web UI.
    pub fn web_protocol(&self) -> &'static str {
        if self.web_base.starts_with("http://") {
            "http"
        } else {
            "https"
        }
    }

    /// Path part of `web_base` (`/bitbucket`), empty when the server lives at the root.
    #[cfg(test)]
    pub fn web_prefix(&self) -> String {
        let rest = self
            .web_base
            .split_once("://")
            .map(|(_, r)| r)
            .unwrap_or_default();
        rest.find('/')
            .map(|i| rest[i..].trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
}

/// Parse a remote URL. Local paths and `file://` URLs give `None`.
pub fn parse_remote_url(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (scheme, authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "file" => return None,
            "git+ssh" | "ssh+git" => "ssh".to_string(),
            s => s.to_string(),
        };
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (scheme, authority, path)
    } else {
        let (left, path) = url.split_once(':')?;
        // `C:\x` / `C:/x` are Windows paths; `./a:b` and `/a:b` are local paths.
        if left.chars().count() < 2
            || left.contains('/')
            || left.contains('\\')
            || path.starts_with('\\')
            || path.starts_with("//")
        {
            return None;
        }
        ("ssh".to_string(), left, path)
    };
    let host_port = authority.rsplit('@').next()?;
    let (host, port) = split_port(host_port);
    let mut host = normalize_host(host);
    if host.is_empty() {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    let mut segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();

    // Azure DevOps SSH: ssh.dev.azure.com:v3/org/project/repo, vs-ssh.visualstudio.com:v3/org/project/repo
    let azure_ssh = matches!(
        host.as_str(),
        "ssh.dev.azure.com" | "vs-ssh.visualstudio.com"
    );
    if azure_ssh && segments.first().map(String::as_str) == Some("v3") {
        segments.remove(0);
        host = "dev.azure.com".into();
        if segments.len() == 3 {
            let repo = segments.pop().unwrap_or_default();
            segments.push("_git".into());
            segments.push(repo);
        }
    }
    let mut prefix = String::new();
    let (owner, name) = if let Some(i) = segments.iter().position(|s| s == "_git") {
        // Azure DevOps / TFS: [collection…/]project/_git/repo
        let name = segments.get(i + 1)?.clone();
        let mut owner: Vec<String> = segments[..i].to_vec();
        if host == "dev.azure.com" && owner.len() == 1 {
            // https://dev.azure.com/org/_git/repo: the repository of the same-named project.
            owner.push(name.clone());
        }
        if owner.is_empty() {
            return None;
        }
        (owner.join("/"), name)
    } else {
        let http = matches!(scheme.as_str(), "http" | "https");
        let big = matches!(host.as_str(), "github.com" | "gitlab.com" | "bitbucket.org");
        let scm = segments
            .iter()
            .position(|s| s == "scm")
            .filter(|_| http && !big);
        if let Some(i) = scm {
            // Bitbucket Server HTTP: [context/]scm/PROJECT/repo
            if segments.len() == i + 3 {
                prefix = segments[..i].iter().map(|s| format!("/{s}")).collect();
                segments.drain(..=i);
            }
        }
        if segments.len() < 2 {
            return None;
        }
        let name = segments[segments.len() - 1].clone();
        (segments[..segments.len() - 1].join("/"), name)
    };
    let web_base = match (scheme.as_str(), port) {
        ("http", Some(p)) | ("https", Some(p)) => format!("{scheme}://{host}:{p}{prefix}"),
        ("http", None) => format!("http://{host}{prefix}"),
        _ => format!("https://{host}{prefix}"),
    };
    Some(RemoteUrl {
        scheme,
        host,
        port,
        owner,
        name,
        web_base,
    })
}

fn split_port(host_port: &str) -> (&str, Option<u16>) {
    // [v6::addr]:port
    if let Some(rest) = host_port.strip_prefix('[') {
        if let Some((h, after)) = rest.split_once(']') {
            let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
            return (h, port);
        }
    }
    match host_port.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, p.parse().ok())
        }
        Some((h, "")) => (h, None),
        _ => (host_port, None),
    }
}

/// SSH-over-HTTPS endpoints map to the host serving the web UI and API.
fn normalize_host(host: &str) -> String {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    match h.as_str() {
        "ssh.github.com" => "github.com".into(),
        "altssh.gitlab.com" => "gitlab.com".into(),
        "altssh.bitbucket.org" => "bitbucket.org".into(),
        _ => h,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The clone URL of `slug` (e.g. a fork) on the same server, in the same format as `remote_url`.
pub fn sibling_clone_url(remote_url: &str, current_slug: &str, slug: &str) -> String {
    let lower = remote_url.to_ascii_lowercase();
    match lower.rfind(&current_slug.to_ascii_lowercase()) {
        Some(i) => format!(
            "{}{}{}",
            &remote_url[..i],
            slug,
            &remote_url[i + current_slug.len()..]
        ),
        None => remote_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(u: &str) -> RemoteUrl {
        parse_remote_url(u).unwrap_or_else(|| panic!("should parse {u}"))
    }

    fn parts(u: &str) -> (String, String, String, String) {
        let r = url(u);
        (r.host, r.owner, r.name, r.web_base)
    }

    fn t(h: &str, o: &str, n: &str, w: &str) -> (String, String, String, String) {
        (h.into(), o.into(), n.into(), w.into())
    }

    #[test]
    fn https_and_http() {
        assert_eq!(
            parts("https://github.com/octo/hello.git"),
            t("github.com", "octo", "hello", "https://github.com")
        );
        assert_eq!(
            parts("https://user:secret@GitLab.Example.com:8443/group/sub/proj.git/"),
            t(
                "gitlab.example.com",
                "group/sub",
                "proj",
                "https://gitlab.example.com:8443"
            )
        );
        assert_eq!(
            url("https://gitlab.example.com:8443/g/p").credential_host(),
            "gitlab.example.com:8443"
        );
        assert_eq!(
            parts("http://gitea.local:3000/me/repo"),
            t("gitea.local", "me", "repo", "http://gitea.local:3000")
        );
        assert_eq!(
            url("http://gitea.local:3000/me/repo").web_protocol(),
            "http"
        );
        assert_eq!(
            parts("https://codeberg.org/forgejo/forgejo"),
            t("codeberg.org", "forgejo", "forgejo", "https://codeberg.org")
        );
        assert_eq!(
            parts("https://bitbucket.org/team/repo.git"),
            t("bitbucket.org", "team", "repo", "https://bitbucket.org")
        );
        assert_eq!(
            parts("git://example.org/pub/scm/tool.git"),
            t("example.org", "pub/scm", "tool", "https://example.org")
        );
    }

    #[test]
    fn ssh_and_scp() {
        assert_eq!(
            parts("git@github.com:octo/hello.git"),
            t("github.com", "octo", "hello", "https://github.com")
        );
        assert_eq!(
            parts("ssh://git@ssh.github.com:443/octo/hello.git"),
            t("github.com", "octo", "hello", "https://github.com")
        );
        let r = url("ssh://git@bitbucket.example.com:7999/proj/repo.git");
        assert_eq!(
            (r.host.as_str(), r.port, r.owner.as_str()),
            ("bitbucket.example.com", Some(7999), "proj")
        );
        assert_eq!(r.web_base, "https://bitbucket.example.com");
        assert_eq!(r.credential_host(), "bitbucket.example.com");
        assert_eq!(
            parts("git+ssh://git@gitlab.com/a/b/c.git"),
            t("gitlab.com", "a/b", "c", "https://gitlab.com")
        );
        assert_eq!(
            parts("gitea.example.com:me/tool"),
            t(
                "gitea.example.com",
                "me",
                "tool",
                "https://gitea.example.com"
            )
        );
        assert_eq!(parts("ssh://git@[::1]:2222/me/tool.git").0, "::1");
    }

    #[test]
    fn azure_devops_formats() {
        let expect = t("dev.azure.com", "org/proj", "repo", "https://dev.azure.com");
        assert_eq!(parts("https://dev.azure.com/org/proj/_git/repo"), expect);
        assert_eq!(
            parts("https://org@dev.azure.com/org/proj/_git/repo"),
            expect
        );
        assert_eq!(parts("git@ssh.dev.azure.com:v3/org/proj/repo"), expect);
        assert_eq!(
            parts("ssh://git@ssh.dev.azure.com/v3/org/proj/repo"),
            expect
        );
        assert_eq!(
            parts("org@vs-ssh.visualstudio.com:v3/org/proj/repo"),
            expect
        );
        assert_eq!(
            parts("https://dev.azure.com/org/My%20Project/_git/My%20Repo"),
            t(
                "dev.azure.com",
                "org/My Project",
                "My Repo",
                "https://dev.azure.com"
            )
        );
        assert_eq!(
            parts("https://org.visualstudio.com/proj/_git/repo"),
            t(
                "org.visualstudio.com",
                "proj",
                "repo",
                "https://org.visualstudio.com"
            )
        );
        assert_eq!(
            parts("https://org.visualstudio.com/DefaultCollection/proj/_git/repo").1,
            "DefaultCollection/proj"
        );
        assert_eq!(
            parts("https://dev.azure.com/org/_git/proj"),
            t("dev.azure.com", "org/proj", "proj", "https://dev.azure.com")
        );
        assert_eq!(
            parts("https://tfs.corp.local/tfs/Coll/proj/_git/repo").1,
            "tfs/Coll/proj"
        );
    }

    #[test]
    fn bitbucket_server_scm_paths() {
        let r = url("https://bb.corp.com/scm/PROJ/repo.git");
        assert_eq!((r.owner.as_str(), r.name.as_str()), ("PROJ", "repo"));
        assert_eq!(r.web_base, "https://bb.corp.com");
        let r = url("https://bb.corp.com/bitbucket/scm/~me/repo.git");
        assert_eq!((r.owner.as_str(), r.name.as_str()), ("~me", "repo"));
        assert_eq!(r.web_base, "https://bb.corp.com/bitbucket");
        assert_eq!(r.web_prefix(), "/bitbucket");
    }

    #[test]
    fn rejects_local_paths() {
        for u in [
            "/srv/git/repo.git",
            "./repo",
            "../x:y",
            "C:\\repos\\x",
            "C:/repos/x",
            "file:///srv/repo.git",
            "\\\\server\\share\\repo",
            "https://host/onlyone",
        ] {
            assert!(parse_remote_url(u).is_none(), "{u} should not parse");
        }
    }

    #[test]
    fn sibling_urls() {
        assert_eq!(
            sibling_clone_url("git@github.com:octo/hello.git", "octo/hello", "me/hello"),
            "git@github.com:me/hello.git"
        );
        assert_eq!(
            sibling_clone_url("https://bitbucket.org/Team/Repo", "team/repo", "me/repo"),
            "https://bitbucket.org/me/repo"
        );
    }
}
