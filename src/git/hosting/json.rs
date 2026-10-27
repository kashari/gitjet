//! Lenient JSON accessors shared by the provider implementations.
//!
//! Provider responses are read through `serde_json::Value` with dotted paths (`"user.login"`), so a
//! missing or renamed field degrades one value instead of failing a whole listing.

use serde_json::Value;

/// Value at a dotted path (`"head.repo.full_name"`); numeric segments index arrays.
pub fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    (!cur.is_null()).then_some(cur)
}

/// String at `path` (numbers and booleans are stringified); empty when missing or null.
pub fn s(v: &Value, path: &str) -> String {
    match get(v, path) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// First non-empty string among `paths`.
pub fn first(v: &Value, paths: &[&str]) -> String {
    paths
        .iter()
        .map(|p| s(v, p))
        .find(|x| !x.is_empty())
        .unwrap_or_default()
}

pub fn b(v: &Value, path: &str) -> bool {
    match get(v, path) {
        Some(Value::Bool(x)) => *x,
        Some(Value::String(x)) => x.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

pub fn opt_bool(v: &Value, path: &str) -> Option<bool> {
    get(v, path).and_then(Value::as_bool)
}

pub fn u(v: &Value, path: &str) -> Option<u64> {
    match get(v, path)? {
        Value::Number(n) => n.as_u64(),
        Value::String(x) => x.trim().parse().ok(),
        _ => None,
    }
}

pub fn i(v: &Value, path: &str) -> Option<i64> {
    match get(v, path)? {
        Value::Number(n) => n.as_i64(),
        Value::String(x) => x.trim().parse().ok(),
        _ => None,
    }
}

/// Array at `path`, empty when missing.
pub fn arr<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    get(v, path)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// RFC 3339 timestamp → unix seconds.
pub fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.timestamp())
}

/// Timestamp at `path`: RFC 3339 text, or epoch milliseconds (Bitbucket Server).
pub fn time(v: &Value, path: &str) -> Option<i64> {
    match get(v, path)? {
        Value::String(x) => parse_time(x),
        Value::Number(n) => n.as_i64().map(|ms| ms / 1000),
        _ => None,
    }
}

/// Unix newlines.
pub fn normalize_text(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Label names from `[{name}]` or `["name"]`.
pub fn labels(v: &Value, path: &str) -> Vec<String> {
    arr(v, path)
        .iter()
        .map(|l| match l {
            Value::String(x) => x.clone(),
            other => s(other, "name"),
        })
        .filter(|x| !x.is_empty())
        .collect()
}

/// Parse a response body; an empty body is `null`.
pub fn parse(text: &str) -> anyhow::Result<Value> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(t).map_err(|e| anyhow::anyhow!("unexpected response (not JSON): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotted_paths_and_types() {
        let v = json!({"a": {"b": [{"c": 5}], "t": "2026-10-01T00:00:00Z", "ms": 1_700_000_000_000u64,
                        "n": null, "flag": "true"}});
        assert_eq!(u(&v, "a.b.0.c"), Some(5));
        assert_eq!(s(&v, "a.b.0.c"), "5");
        assert_eq!(s(&v, "a.missing"), "");
        assert!(get(&v, "a.n").is_none());
        assert!(b(&v, "a.flag"));
        assert_eq!(time(&v, "a.t"), Some(1_790_812_800));
        assert_eq!(time(&v, "a.ms"), Some(1_700_000_000));
        assert_eq!(first(&v, &["a.x", "a.t"]), "2026-10-01T00:00:00Z");
        assert_eq!(arr(&v, "a.b").len(), 1);
        assert_eq!(labels(&json!({"l": ["x", {"name": "y"}]}), "l"), ["x", "y"]);
        assert!(parse("").unwrap().is_null());
        assert!(parse("<html>").is_err());
    }
}
