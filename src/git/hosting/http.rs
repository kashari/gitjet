//! HTTP for the hosting APIs: a tiny [`Transport`] trait (faked in tests, so no test touches the
//! network), the real [`UreqTransport`] (rustls, proxies from the environment, timeouts), and
//! [`Api`], which adds authentication, JSON, pagination and readable errors.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::Value;

use super::json;

/// One HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    #[cfg(test)]
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// One HTTP response, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Sends requests. Non-2xx statuses are responses, not errors; `Err` means the server could not be
/// reached at all.
pub trait Transport: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse>;
}

/// `gitjet/<version>`.
pub fn user_agent() -> String {
    format!("gitjet/{}", env!("CARGO_PKG_VERSION"))
}

/// The real transport: ureq with rustls, `HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` from the
/// environment, a 10 s connect and 30 s read timeout.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self::with_timeouts(Duration::from_secs(10), Duration::from_secs(30))
    }

    pub fn with_timeouts(connect: Duration, read: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(connect)
            .timeout_read(read)
            .timeout_write(read)
            .try_proxy_from_env(true)
            .user_agent(&user_agent())
            .build();
        Self { agent }
    }

    /// Shared default transport.
    pub fn shared() -> Arc<dyn Transport> {
        Arc::new(Self::new())
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn to_response(r: ureq::Response) -> HttpResponse {
    let status = r.status();
    let headers = r
        .headers_names()
        .into_iter()
        .filter_map(|n| r.header(&n).map(|v| (n.clone(), v.to_string())))
        .collect();
    let body = r.into_string().unwrap_or_default();
    HttpResponse {
        status,
        headers,
        body,
    }
}

impl Transport for UreqTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse> {
        let mut r = self.agent.request(&req.method, &req.url);
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        let result = match &req.body {
            Some(b) => r.send_string(b),
            None => r.call(),
        };
        match result {
            Ok(resp) => Ok(to_response(resp)),
            Err(ureq::Error::Status(_, resp)) => Ok(to_response(resp)),
            Err(ureq::Error::Transport(t)) => {
                let host = url_host(&req.url);
                Err(anyhow!("could not reach {host}: {t}"))
            }
        }
    }
}

/// Host part of a URL, for messages.
pub fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// URL of the `rel="next"` page from an RFC 8288 `Link` header (GitHub, GitLab, Gitea).
pub fn next_link(resp: &HttpResponse) -> Option<String> {
    let link = resp.header("link")?;
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let is_next = params
            .split(';')
            .any(|p| p.trim().replace(' ', "") == "rel=\"next\"");
        is_next.then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

/// The provider's own error text from a JSON (or short plain-text) error body.
pub fn api_message(body: &str) -> Option<String> {
    let v: Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(_) => {
            let t = body.trim();
            return (!t.is_empty() && t.len() <= 200 && !t.starts_with('<')).then(|| t.to_string());
        }
    };
    let direct = json::first(
        &v,
        &[
            "message",
            "error.message",
            "error_description",
            "errors.0.message",
            "errors.0",
            "error",
        ],
    );
    let msg = if direct.is_empty() {
        // GitLab validation errors: {"message": {"title": ["can't be blank"]}}
        v.get("message")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| format!("{k} {}", v.to_string().trim_matches(['[', ']', '"'])))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    } else {
        direct
    };
    let extra = json::s(&v, "errors.0.message");
    let msg = if !extra.is_empty() && !msg.contains(&extra) {
        format!("{msg} ({extra})")
    } else {
        msg
    };
    (!msg.trim().is_empty()).then(|| msg.trim().to_string())
}

/// Readable error for a non-2xx response: what failed, why, and what to do about it.
pub fn status_error(
    provider: &str,
    host: &str,
    resp: &HttpResponse,
    signed_in: bool,
) -> anyhow::Error {
    let on = if host.is_empty() {
        String::new()
    } else {
        format!(" on {host}")
    };
    let detail = api_message(&resp.body)
        .map(|m| format!(": {m}"))
        .unwrap_or_default();
    let code = resp.status;
    let remaining_zero = resp.header("x-ratelimit-remaining") == Some("0")
        || resp.header("ratelimit-remaining") == Some("0");
    let rate_limited = code == 429 || (code == 403 && remaining_zero);
    if rate_limited {
        let when = resp
            .header("retry-after")
            .map(|s| format!(" — retry after {s} s"))
            .or_else(|| {
                resp.header("x-ratelimit-reset")
                    .or_else(|| resp.header("ratelimit-reset"))
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .map(|ts| format!(" — resets at {}", crate::util::format_time(ts)))
            })
            .unwrap_or_default();
        let hint = if signed_in {
            ""
        } else {
            " Anonymous requests have a low limit; press L to sign in."
        };
        return anyhow!("{provider} rate limit exceeded{on} (HTTP {code}){when}.{hint}");
    }
    let sign_in = "press L to sign in with a personal access token";
    match code {
        401 if signed_in => anyhow!(
            "{provider} rejected the token{on} (HTTP 401){detail}. It may be expired or revoked — {sign_in}."
        ),
        401 => anyhow!("{provider} requires signing in{on} (HTTP 401){detail} — {sign_in}."),
        403 if signed_in => anyhow!(
            "{provider} denied access{on} (HTTP 403){detail}. The token may lack the needed scopes/permissions."
        ),
        403 => anyhow!("{provider} denied access{on} (HTTP 403){detail} — {sign_in}."),
        404 if signed_in => anyhow!(
            "Not found{on} (HTTP 404){detail}. Check the remote URL and that the token can access this repository."
        ),
        404 => anyhow!(
            "Not found{on} (HTTP 404){detail}. Private repositories are hidden without a token — {sign_in}."
        ),
        409 | 405 | 422 | 400 => anyhow!("{provider} refused the request (HTTP {code}){detail}"),
        c if c >= 500 => anyhow!("{provider} server error{on} (HTTP {c}){detail}. Try again later."),
        c => anyhow!("{provider} request failed{on} (HTTP {c}){detail}"),
    }
}

/// An authenticated JSON API client for one provider.
#[derive(Clone)]
pub struct Api {
    pub transport: Arc<dyn Transport>,
    /// Display name of the provider ("GitHub"), for errors.
    pub provider: &'static str,
    /// Host for error messages.
    pub host: String,
    /// Authentication header, when signed in.
    pub auth: Option<(String, String)>,
    /// Extra headers sent with every request (`Accept`, API version…).
    pub headers: Vec<(String, String)>,
}

/// Most items fetched from any paginated listing.
pub const MAX_ITEMS: usize = 100;

impl Api {
    pub fn signed_in(&self) -> bool {
        self.auth.is_some()
    }

    fn build(&self, method: &str, url: &str, body: Option<String>) -> HttpRequest {
        let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
        for (k, v) in &self.headers {
            headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
            headers.push((k.clone(), v.clone()));
        }
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        if let Some((k, v)) = &self.auth {
            headers.push((k.clone(), v.clone()));
        }
        HttpRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers,
            body,
        }
    }

    /// Send a request; non-2xx becomes a readable error.
    pub fn raw(&self, method: &str, url: &str, body: Option<&Value>) -> Result<HttpResponse> {
        let req = self.build(method, url, body.map(Value::to_string));
        let resp = self.transport.send(&req)?;
        if resp.ok() {
            return Ok(resp);
        }
        Err(status_error(
            self.provider,
            &self.host,
            &resp,
            self.signed_in(),
        ))
    }

    pub fn get(&self, url: &str) -> Result<Value> {
        json::parse(&self.raw("GET", url, None)?.body)
    }

    /// GET returning the raw body (plain-text endpoints).
    pub fn get_text(&self, url: &str) -> Result<String> {
        Ok(self.raw("GET", url, None)?.body)
    }

    pub fn send(&self, method: &str, url: &str, body: &Value) -> Result<Value> {
        json::parse(&self.raw(method, url, Some(body))?.body)
    }

    /// A request without a body (POST /approve, DELETE…).
    pub fn call(&self, method: &str, url: &str) -> Result<Value> {
        json::parse(&self.raw(method, url, None)?.body)
    }

    /// Follow pages from `url` until `next` gives none or `cap` items were read.
    pub fn paged(
        &self,
        url: &str,
        cap: usize,
        items: impl Fn(&Value) -> Vec<Value>,
        next: impl Fn(&HttpResponse, &Value) -> Option<String>,
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut url = url.to_string();
        for _ in 0..20 {
            let resp = self.raw("GET", &url, None)?;
            let v = json::parse(&resp.body)?;
            out.extend(items(&v));
            if out.len() >= cap {
                out.truncate(cap);
                break;
            }
            match next(&resp, &v) {
                Some(n) if n != url => url = n,
                _ => break,
            }
        }
        Ok(out)
    }

    /// Pages of a top-level JSON array linked with `Link: rel="next"`.
    pub fn paged_array(&self, url: &str, cap: usize) -> Result<Vec<Value>> {
        self.paged(
            url,
            cap,
            |v| v.as_array().cloned().unwrap_or_default(),
            |r, _| next_link(r),
        )
    }
}

/// Percent-encode everything but unreserved characters (a single path segment or query value).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Percent-encode a path, keeping `/` separators (branch names in URLs).
pub fn encode_path(s: &str) -> String {
    s.split('/').map(encode).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
pub mod fake {
    //! A scripted [`Transport`] that records every request.

    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeTransport {
        routes: Mutex<Vec<(String, String, HttpResponse)>>,
        pub requests: Mutex<Vec<HttpRequest>>,
    }

    impl FakeTransport {
        pub fn new() -> Arc<FakeTransport> {
            Arc::new(FakeTransport::default())
        }

        /// Answer `method` requests whose URL contains `pattern` (the longest matching pattern wins).
        pub fn on(&self, method: &str, pattern: &str, status: u16, body: &str) -> &Self {
            self.on_with(method, pattern, status, body, &[])
        }

        pub fn on_with(
            &self,
            method: &str,
            pattern: &str,
            status: u16,
            body: &str,
            headers: &[(&str, &str)],
        ) -> &Self {
            self.routes.lock().unwrap().push((
                method.to_string(),
                pattern.to_string(),
                HttpResponse {
                    status,
                    headers: headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    body: body.to_string(),
                },
            ));
            self
        }

        pub fn log(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }

        /// The last request sent with `method`.
        pub fn last(&self, method: &str) -> HttpRequest {
            self.log()
                .into_iter()
                .rev()
                .find(|r| r.method == method)
                .unwrap_or_else(|| panic!("no {method} request in {:#?}", self.log()))
        }
    }

    impl Transport for FakeTransport {
        fn send(&self, req: &HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(req.clone());
            let routes = self.routes.lock().unwrap();
            let best = routes
                .iter()
                .filter(|(m, p, _)| *m == req.method && req.url.contains(p.as_str()))
                .max_by_key(|(_, p, _)| p.len());
            match best {
                Some((_, _, r)) => Ok(r.clone()),
                None if req.url.contains("unreachable") => Err(anyhow!("could not reach host")),
                None => Ok(HttpResponse {
                    status: 404,
                    headers: Vec::new(),
                    body: r#"{"message":"Not Found"}"#.into(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeTransport;
    use super::*;

    fn api(t: Arc<FakeTransport>, auth: bool) -> Api {
        Api {
            transport: t,
            provider: "GitHub",
            host: "github.com".into(),
            auth: auth.then(|| ("Authorization".to_string(), "Bearer x".to_string())),
            headers: vec![("Accept".into(), "application/vnd.github+json".into())],
        }
    }

    #[test]
    fn link_header_pagination() {
        let r = HttpResponse {
            status: 200,
            headers: vec![(
                "Link".into(),
                r#"<https://api.x/r?page=2>; rel="next", <https://api.x/r?page=5>; rel="last""#
                    .into(),
            )],
            body: String::new(),
        };
        assert_eq!(next_link(&r).as_deref(), Some("https://api.x/r?page=2"));
        assert_eq!(next_link(&HttpResponse::default()), None);

        let t = FakeTransport::new();
        let items: Vec<String> = (0..60).map(|i| format!("{{\"n\":{i}}}")).collect();
        let page1 = format!("[{}]", items[..50].join(","));
        let page2 = format!("[{}]", items[50..].join(","));
        t.on_with(
            "GET",
            "/list",
            200,
            &page1,
            &[("Link", r#"<https://api.x/list?page=2>; rel="next""#)],
        )
        .on("GET", "/list?page=2", 200, &page2);
        let a = api(t.clone(), false);
        assert_eq!(a.paged_array("https://api.x/list", 100).unwrap().len(), 60);
        assert_eq!(a.paged_array("https://api.x/list", 10).unwrap().len(), 10);
        let sent = t.log();
        assert_eq!(
            sent[0].header("accept"),
            Some("application/vnd.github+json")
        );
        assert!(sent[0].header("authorization").is_none());
    }

    #[test]
    fn errors_are_readable() {
        let t = FakeTransport::new();
        t.on("GET", "/private", 404, r#"{"message":"Not Found"}"#)
            .on("GET", "/auth", 401, r#"{"message":"Bad credentials"}"#)
            .on_with(
                "GET",
                "/limited",
                403,
                r#"{"message":"API rate limit exceeded"}"#,
                &[("X-RateLimit-Remaining", "0"), ("Retry-After", "60")],
            )
            .on("GET", "/boom", 502, "<html>bad gateway</html>");
        let anon = api(t.clone(), false);
        let e = format!("{:#}", anon.get("https://api.x/private").unwrap_err());
        assert!(e.contains("HTTP 404") && e.contains("press L"), "{e}");
        let authed = api(t.clone(), true);
        let e = format!("{:#}", authed.get("https://api.x/auth").unwrap_err());
        assert!(
            e.contains("rejected the token") && e.contains("Bad credentials"),
            "{e}"
        );
        let e = format!("{:#}", authed.get("https://api.x/limited").unwrap_err());
        assert!(e.contains("rate limit") && e.contains("60 s"), "{e}");
        let e = format!("{:#}", authed.get("https://api.x/boom").unwrap_err());
        assert!(e.contains("server error") && !e.contains("html"), "{e}");
        assert!(
            format!("{:#}", authed.get("https://unreachable/x").unwrap_err())
                .contains("could not reach")
        );
        assert_eq!(authed.last_auth_header(&t), Some("Bearer x".to_string()));
    }

    impl Api {
        fn last_auth_header(&self, t: &FakeTransport) -> Option<String> {
            t.log()
                .last()
                .and_then(|r| r.header("authorization").map(str::to_string))
        }
    }

    #[test]
    fn messages_and_encoding() {
        assert_eq!(
            api_message(r#"{"message":{"title":["can't be blank"]}}"#).as_deref(),
            Some("title can't be blank")
        );
        assert_eq!(
            api_message(r#"{"error":{"message":"Bad request"}}"#).as_deref(),
            Some("Bad request")
        );
        assert_eq!(
            api_message(r#"{"message":"Validation Failed","errors":[{"message":"No commits"}]}"#)
                .as_deref(),
            Some("Validation Failed (No commits)")
        );
        assert_eq!(api_message("<html></html>"), None);
        assert_eq!(encode("group/sub proj"), "group%2Fsub%20proj");
        assert_eq!(encode_path("feature/ä b"), "feature/%C3%A4%20b");
        assert_eq!(url_host("https://user@host:8443/api?x"), "host:8443");
        assert!(user_agent().starts_with("gitjet/"));
    }
}
