//! Provider-neutral authentication. Tokens never live in gitjet's config file.
//!
//! Lookup order:
//! 1. `GITJET_TOKEN_<HOST>` (host upper-cased, other characters `_`), then `GITJET_TOKEN`
//!    (user names: `GITJET_USERNAME_<HOST>` / `GITJET_USERNAME`);
//! 2. the provider's conventional variable (`GITHUB_TOKEN`, `GITLAB_TOKEN`, `BITBUCKET_TOKEN` or
//!    `BITBUCKET_USERNAME` + `BITBUCKET_APP_PASSWORD`, `AZURE_DEVOPS_EXT_PAT`, `GITEA_TOKEN`…);
//! 3. with `hosting.use_git_credentials`, the user's own git credential helper
//!    (`git credential fill`, never prompting).
//!
//! Signing in stores a token with `git credential approve`, i.e. in the OS credential store the
//! user's helper manages (Keychain, Windows Credential Manager, libsecret, GCM…).

use std::io::Write;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, bail, Context, Result};

use crate::config::HostingConfig;
use crate::git::Repo;

use super::{Hosting, ProviderKind};

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Bumped whenever credentials are stored, so cached connections know to look them up again.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// A token (or app password / password) and where it came from.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: Option<String>,
    pub secret: String,
    /// Human description of the source ("GITJET_TOKEN", "git credential helper").
    pub source: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("secret", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// `git.example.com:8443` → `GIT_EXAMPLE_COM_8443`.
pub fn host_env_suffix(host: &str) -> String {
    host.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Conventional token variables of a provider, most specific first.
pub fn provider_env_vars(kind: ProviderKind, host: &str) -> &'static [&'static str] {
    match kind {
        ProviderKind::GitHub if host == "github.com" => &["GITHUB_TOKEN", "GH_TOKEN"],
        ProviderKind::GitHub => &[
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_TOKEN",
            "GH_TOKEN",
        ],
        ProviderKind::GitLab => &["GITLAB_TOKEN", "GITLAB_PRIVATE_TOKEN"],
        ProviderKind::BitbucketCloud => &["BITBUCKET_TOKEN"],
        ProviderKind::BitbucketServer => &["BITBUCKET_SERVER_TOKEN", "BITBUCKET_TOKEN"],
        ProviderKind::Azure => &[
            "AZURE_DEVOPS_EXT_PAT",
            "AZURE_DEVOPS_TOKEN",
            "SYSTEM_ACCESSTOKEN",
        ],
        ProviderKind::Gitea => &["GITEA_TOKEN", "FORGEJO_TOKEN", "CODEBERG_TOKEN"],
        ProviderKind::Generic => &[],
    }
}

/// Credentials from environment variables (`env` is `std::env::var` outside tests).
pub fn from_env(
    kind: ProviderKind,
    host: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<Credentials> {
    let get = |k: &str| {
        env(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let suffix = host_env_suffix(host);
    let username = get(&format!("GITJET_USERNAME_{suffix}")).or_else(|| get("GITJET_USERNAME"));
    for var in [format!("GITJET_TOKEN_{suffix}"), "GITJET_TOKEN".to_string()] {
        if let Some(secret) = get(&var) {
            return Some(Credentials {
                username,
                secret,
                source: var,
            });
        }
    }
    if kind == ProviderKind::BitbucketCloud {
        if let (Some(user), Some(pw)) = (get("BITBUCKET_USERNAME"), get("BITBUCKET_APP_PASSWORD")) {
            return Some(Credentials {
                username: Some(user),
                secret: pw,
                source: "BITBUCKET_APP_PASSWORD".into(),
            });
        }
    }
    for var in provider_env_vars(kind, host) {
        if let Some(secret) = get(var) {
            let username = match kind {
                ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer => {
                    get("BITBUCKET_USERNAME").or(username.clone())
                }
                _ => username.clone(),
            };
            return Some(Credentials {
                username,
                secret,
                source: (*var).to_string(),
            });
        }
    }
    None
}

/// Input for `git credential fill/approve/reject`.
pub fn credential_input(
    protocol: &str,
    host: &str,
    path: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
) -> String {
    let clean = |s: &str| s.replace(['\n', '\r', '\0'], "");
    let mut s = format!("protocol={}\nhost={}\n", clean(protocol), clean(host));
    if let Some(p) = path {
        s.push_str(&format!("path={}\n", clean(p)));
    }
    if let Some(u) = username {
        s.push_str(&format!("username={}\n", clean(u)));
    }
    if let Some(p) = password {
        s.push_str(&format!("password={}\n", clean(p)));
    }
    s.push('\n');
    s
}

/// (username, password) from `git credential fill` output.
pub fn parse_credential_output(text: &str) -> (Option<String>, Option<String>) {
    let mut user = None;
    let mut pass = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("username=") {
            user = Some(v.to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("password=") {
            pass = Some(v.to_string()).filter(|v| !v.is_empty());
        }
    }
    (user, pass)
}

/// Run `git credential <action>` without any chance of an interactive prompt.
fn git_credential(repo: &Repo, action: &str, input: &str) -> Result<String> {
    let mut c = repo.command();
    c.args(["-c", "credential.interactive=never", "credential", action])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c
        .spawn()
        .with_context(|| format!("could not run `git credential {action}`"))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let data = input.as_bytes().to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&data));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        bail!(
            "git credential {action} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `path=` for credential lookups: Azure DevOps credentials are often stored per path
/// (`credential.useHttpPath`).
fn credential_path(h: &Hosting) -> Option<String> {
    (h.kind == ProviderKind::Azure)
        .then(|| {
            h.url
                .owner
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|p| !p.is_empty())
}

/// Credentials from the user's git credential helper.
pub fn from_git(repo: &Repo, h: &Hosting) -> Option<Credentials> {
    let protocol = h.url.web_protocol();
    let host = h.url.credential_host();
    let mut attempts = vec![None];
    if let Some(p) = credential_path(h) {
        attempts.insert(0, Some(p));
    }
    for path in attempts {
        let input = credential_input(protocol, &host, path.as_deref(), None, None);
        if let Ok(out) = git_credential(repo, "fill", &input) {
            if let (user, Some(secret)) = parse_credential_output(&out) {
                return Some(Credentials {
                    username: user,
                    secret,
                    source: "git credential helper".into(),
                });
            }
        }
    }
    None
}

/// Credentials for `h`: environment first, then (when enabled) git's credential helper.
pub fn resolve(repo: &Repo, h: &Hosting, cfg: &HostingConfig) -> Option<Credentials> {
    if !h.kind.has_api() {
        return None;
    }
    from_env(h.kind, &h.url.credential_host(), &|k| std::env::var(k).ok())
        .or_else(|| cfg.use_git_credentials.then(|| from_git(repo, h)).flatten())
}

/// Standard base64 (for HTTP Basic).
pub fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn basic(user: &str, secret: &str) -> (String, String) {
    (
        "Authorization".into(),
        format!("Basic {}", base64(format!("{user}:{secret}").as_bytes())),
    )
}

fn bearer(secret: &str) -> (String, String) {
    ("Authorization".into(), format!("Bearer {secret}"))
}

/// User names that mark "the password is a token" rather than a real account.
fn is_token_username(u: &str) -> bool {
    matches!(
        u,
        "x-access-token"
            | "x-token-auth"
            | "oauth2"
            | "token"
            | "gitjet"
            | "pat"
            | "PersonalAccessToken"
    )
}

fn looks_like_jwt(s: &str) -> bool {
    s.starts_with("eyJ") && s.matches('.').count() == 2
}

/// The HTTP header that authenticates `creds` against `kind`'s API.
///
/// - GitHub: `Authorization: Bearer` (PATs, fine-grained and OAuth tokens alike).
/// - GitLab: `PRIVATE-TOKEN`, or `Bearer` for OAuth tokens (user name `oauth2`).
/// - Bitbucket Cloud: Basic `user:app-password`; Bearer for access tokens (no user name).
/// - Bitbucket Server: Basic `user:token`; Bearer for HTTP access tokens without a user name.
/// - Azure DevOps: Basic `:PAT`; Bearer for OAuth (JWT) tokens.
/// - Gitea/Forgejo: `Authorization: token` for 40-hex access tokens, Basic for passwords.
pub fn auth_header(kind: ProviderKind, creds: &Credentials) -> (String, String) {
    let secret = creds.secret.as_str();
    let user = creds
        .username
        .as_deref()
        .filter(|u| !u.is_empty() && !is_token_username(u));
    match kind {
        ProviderKind::GitHub | ProviderKind::Generic => bearer(secret),
        ProviderKind::GitLab => {
            if creds.username.as_deref() == Some("oauth2") || looks_like_jwt(secret) {
                bearer(secret)
            } else {
                ("PRIVATE-TOKEN".into(), secret.to_string())
            }
        }
        ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer => match user {
            Some(u) => basic(u, secret),
            None => bearer(secret),
        },
        ProviderKind::Azure => {
            if looks_like_jwt(secret) {
                bearer(secret)
            } else {
                basic(creds.username.as_deref().unwrap_or(""), secret)
            }
        }
        ProviderKind::Gitea => {
            let tokenish = secret.len() == 40 && secret.bytes().all(|b| b.is_ascii_hexdigit());
            match user {
                Some(u) if !tokenish => basic(u, secret),
                _ => ("Authorization".into(), format!("token {secret}")),
            }
        }
    }
}

/// User name stored with a token when the user gave none (git's credential store needs one).
pub fn default_username(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GitHub => "x-access-token",
        ProviderKind::GitLab => "oauth2",
        ProviderKind::BitbucketCloud | ProviderKind::BitbucketServer => "x-token-auth",
        _ => "gitjet",
    }
}

/// Whether a credential helper is configured for the host.
fn has_helper(repo: &Repo, h: &Hosting) -> bool {
    let url = format!("{}://{}/", h.url.web_protocol(), h.url.credential_host());
    repo.try_run(&["config", "--get-urlmatch", "credential.helper", &url])
        .is_some_and(|s| !s.trim().is_empty())
}

/// Store a token in the user's credential store via `git credential approve`.
pub fn store(repo: &Repo, h: &Hosting, username: Option<&str>, secret: &str) -> Result<()> {
    if !has_helper(repo, h) {
        bail!(
            "No git credential helper is configured, so the token cannot be stored. Configure one \
             (e.g. `git config --global credential.helper {}`) or set GITJET_TOKEN instead.",
            if cfg!(windows) {
                "manager"
            } else if cfg!(target_os = "macos") {
                "osxkeychain"
            } else {
                "libsecret"
            }
        );
    }
    let user = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| default_username(h.kind));
    let input = credential_input(
        h.url.web_protocol(),
        &h.url.credential_host(),
        credential_path(h).as_deref(),
        Some(user),
        Some(secret.trim()),
    );
    git_credential(repo, "approve", &input)?;
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Ask the credential helper to forget rejected credentials.
#[cfg(test)]
pub fn reject(repo: &Repo, h: &Hosting, creds: &Credentials) {
    let input = credential_input(
        h.url.web_protocol(),
        &h.url.credential_host(),
        credential_path(h).as_deref(),
        creds.username.as_deref(),
        Some(&creds.secret),
    );
    let _ = git_credential(repo, "reject", &input);
}

/// Check a token against the API, then store it. Returns the signed-in user name.
pub fn sign_in(
    repo: &Repo,
    h: &Hosting,
    cfg: &HostingConfig,
    username: Option<String>,
    secret: &str,
    transport: std::sync::Arc<dyn super::Transport>,
) -> Result<String> {
    if !cfg.use_git_credentials {
        bail!(
            "hosting.use_git_credentials is off, so gitjet does not use stored credentials. Turn it \
             on in {} or set GITJET_TOKEN.",
            crate::config::Config::path().display()
        );
    }
    let secret = secret.trim();
    if secret.is_empty() {
        bail!("The token is empty");
    }
    let creds = Credentials {
        username: username.clone().filter(|u| !u.trim().is_empty()),
        secret: secret.to_string(),
        source: "git credential helper".into(),
    };
    let api = super::client(h, Some(&creds), transport)
        .ok_or_else(|| anyhow!("{} has no pull request API to sign in to", h.url.host))?;
    let who = api.current_user().context("the token was not accepted")?;
    store(repo, h, creds.username.as_deref(), secret)?;
    Ok(who)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(map: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    fn creds(user: Option<&str>, secret: &str) -> Credentials {
        Credentials {
            username: user.map(str::to_string),
            secret: secret.into(),
            source: "test".into(),
        }
    }

    #[test]
    fn env_lookup_order() {
        let e = env(&[
            ("GITJET_TOKEN", "generic"),
            ("GITJET_TOKEN_GIT_CORP_COM_8443", "specific"),
            ("GITLAB_TOKEN", "gl"),
        ]);
        let c = from_env(ProviderKind::GitLab, "git.corp.com:8443", &e).unwrap();
        assert_eq!(
            (c.secret.as_str(), c.source.as_str()),
            ("specific", "GITJET_TOKEN_GIT_CORP_COM_8443")
        );
        let c = from_env(ProviderKind::GitLab, "gitlab.com", &e).unwrap();
        assert_eq!(c.secret, "generic");
        let e = env(&[
            ("GITLAB_TOKEN", "gl"),
            ("GH_TOKEN", "ghtok"),
            ("GITHUB_TOKEN", " "),
        ]);
        assert_eq!(
            from_env(ProviderKind::GitLab, "gitlab.com", &e)
                .unwrap()
                .secret,
            "gl"
        );
        assert_eq!(
            from_env(ProviderKind::GitHub, "github.com", &e)
                .unwrap()
                .source,
            "GH_TOKEN"
        );
        assert!(from_env(ProviderKind::Gitea, "codeberg.org", &e).is_none());
        let e = env(&[
            ("BITBUCKET_USERNAME", "me"),
            ("BITBUCKET_APP_PASSWORD", "pw"),
        ]);
        let c = from_env(ProviderKind::BitbucketCloud, "bitbucket.org", &e).unwrap();
        assert_eq!(
            (c.username.as_deref(), c.secret.as_str()),
            (Some("me"), "pw")
        );
        let e = env(&[("AZURE_DEVOPS_EXT_PAT", "pat")]);
        assert_eq!(
            from_env(ProviderKind::Azure, "dev.azure.com", &e)
                .unwrap()
                .secret,
            "pat"
        );
        let e = env(&[("GITEA_TOKEN", "t"), ("GITJET_USERNAME", "bob")]);
        assert_eq!(
            from_env(ProviderKind::Gitea, "tea.corp", &e)
                .unwrap()
                .username
                .as_deref(),
            Some("bob")
        );
        assert_eq!(
            host_env_suffix("git-lab.example.com:8443"),
            "GIT_LAB_EXAMPLE_COM_8443"
        );
        assert!(!format!("{:?}", creds(None, "s3cret")).contains("s3cret"));
    }

    #[test]
    fn auth_headers_per_provider() {
        let h = |k, c: &Credentials| auth_header(k, c);
        assert_eq!(
            h(ProviderKind::GitHub, &creds(Some("me"), "ghp_x")),
            ("Authorization".into(), "Bearer ghp_x".into())
        );
        assert_eq!(
            h(ProviderKind::GitLab, &creds(None, "glpat-x")),
            ("PRIVATE-TOKEN".into(), "glpat-x".into())
        );
        assert_eq!(
            h(ProviderKind::GitLab, &creds(Some("oauth2"), "abc")),
            ("Authorization".into(), "Bearer abc".into())
        );
        assert_eq!(
            h(ProviderKind::BitbucketCloud, &creds(Some("me"), "pw")),
            (
                "Authorization".into(),
                format!("Basic {}", base64(b"me:pw"))
            )
        );
        assert_eq!(
            h(
                ProviderKind::BitbucketCloud,
                &creds(Some("x-token-auth"), "tok")
            ),
            ("Authorization".into(), "Bearer tok".into())
        );
        assert_eq!(
            h(ProviderKind::BitbucketServer, &creds(None, "tok")),
            ("Authorization".into(), "Bearer tok".into())
        );
        assert_eq!(
            h(ProviderKind::Azure, &creds(None, "pat")),
            ("Authorization".into(), format!("Basic {}", base64(b":pat")))
        );
        assert_eq!(
            h(ProviderKind::Azure, &creds(None, "eyJa.b.c")).1,
            "Bearer eyJa.b.c"
        );
        let hex = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            h(ProviderKind::Gitea, &creds(Some("me"), hex)).1,
            format!("token {hex}")
        );
        assert_eq!(
            h(ProviderKind::Gitea, &creds(Some("me"), "password")).1,
            format!("Basic {}", base64(b"me:password"))
        );
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b":pat"), "OnBhdA==");
    }

    #[test]
    fn credential_protocol_text() {
        assert_eq!(
            credential_input("https", "gitlab.com", None, None, None),
            "protocol=https\nhost=gitlab.com\n\n"
        );
        assert_eq!(
            credential_input("https", "h", Some("org"), Some("u"), Some("p\nx")),
            "protocol=https\nhost=h\npath=org\nusername=u\npassword=px\n\n"
        );
        assert_eq!(
            parse_credential_output("protocol=https\nhost=h\nusername=me\npassword=tok\n"),
            (Some("me".into()), Some("tok".into()))
        );
        assert_eq!(parse_credential_output(""), (None, None));
    }

    #[test]
    fn git_credential_store_round_trip() {
        let (_d, repo) = crate::git::testutil::temp_repo();
        let store_file = repo.root.join("creds.txt");
        let helper = format!(
            "store --file={}",
            store_file.display().to_string().replace('\\', "/")
        );
        // An empty entry resets the helper list, so the user's real keychain is never touched.
        repo.run(&["config", "--add", "credential.helper", ""])
            .unwrap();
        repo.run(&["config", "--add", "credential.helper", &helper])
            .unwrap();
        let url = crate::git::hosting::parse_remote_url("https://tea.corp/me/app.git").unwrap();
        let h = Hosting::new(
            ProviderKind::Gitea,
            "origin",
            "https://tea.corp/me/app.git",
            url,
            None,
            super::super::DetectedBy::KnownHost,
        );
        assert!(from_git(&repo, &h).is_none());
        store(&repo, &h, None, "tok123").unwrap();
        let c = from_git(&repo, &h).unwrap();
        assert_eq!(
            (c.username.as_deref(), c.secret.as_str()),
            (Some("gitjet"), "tok123")
        );
        let cfg = HostingConfig::default();
        if std::env::var("GITJET_TOKEN").is_err() && std::env::var("GITEA_TOKEN").is_err() {
            assert_eq!(resolve(&repo, &h, &cfg).unwrap().secret, "tok123");
        }
        let off = HostingConfig {
            use_git_credentials: false,
            hosts: Vec::new(),
        };
        if std::env::var("GITJET_TOKEN").is_err() && std::env::var("GITEA_TOKEN").is_err() {
            assert!(resolve(&repo, &h, &off).is_none());
        }
        reject(&repo, &h, &c);
        assert!(from_git(&repo, &h).is_none());
    }
}
