//! Searching the work tree: text in files (`git grep`), files by name, and declarations
//! (classes, functions...) by name. Tracked and untracked files are searched; ignored ones are not.

use std::io::{BufRead, BufReader};
use std::process::Stdio;

use anyhow::Result;

use super::Repo;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GrepOptions {
    pub case_sensitive: bool,
    /// The pattern is an extended regular expression instead of plain text
    pub regex: bool,
    pub whole_word: bool,
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// 1-based
    pub line: usize,
    /// 1-based byte column of the first match in the line
    pub column: usize,
    pub text: String,
}

/// Lines longer than this are cut (minified files).
const MAX_TEXT: usize = 400;

/// Lines matching `pattern`, at most `limit`; the flag says that there were more. git is
/// stopped as soon as the limit is reached, so a pattern that matches everywhere stays fast.
pub fn grep(
    repo: &Repo,
    pattern: &str,
    opts: GrepOptions,
    limit: usize,
) -> Result<(Vec<Hit>, bool)> {
    let mut args = vec![
        "grep",
        "-n",
        "--column",
        "-I",
        "--untracked",
        "-z",
        "--no-color",
    ];
    args.push(if opts.regex { "-E" } else { "-F" });
    if !opts.case_sensitive {
        args.push("-i");
    }
    if opts.whole_word {
        args.push("-w");
    }
    args.extend(["-e", pattern, "--"]);
    let mut c = repo.command();
    c.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = std::time::Instant::now();
    let mut child = c.spawn().map_err(|e| super::spawn_error(e, &args))?;
    let mut out = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut hits = Vec::new();
    let mut more = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        if out.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if hits.len() == limit {
            more = true;
            let _ = child.kill();
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        // path NUL line NUL column NUL text
        let mut parts = line.splitn(4, |b| *b == 0);
        let (Some(path), Some(n), Some(col), Some(text)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let num = |b: &[u8]| String::from_utf8_lossy(b).parse::<usize>().unwrap_or(1);
        let text = String::from_utf8_lossy(text);
        let text = text.trim_end_matches('\r');
        let cut = text
            .char_indices()
            .nth(MAX_TEXT)
            .map_or(text.len(), |(i, _)| i);
        hits.push(Hit {
            path: String::from_utf8_lossy(path).into_owned(),
            line: num(n),
            column: num(col),
            text: text[..cut].to_string(),
        });
    }
    drop(out);
    let done = child.wait_with_output()?;
    super::trace(&args, started);
    // 1 means "no match"; a real failure (a bad regular expression) has a message.
    if !more && hits.is_empty() && done.status.code() != Some(1) && !done.status.success() {
        let msg = String::from_utf8_lossy(&done.stderr);
        anyhow::bail!("{}", msg.trim().trim_start_matches("fatal: "));
    }
    Ok((hits, more))
}

/// Every file of the work tree that is tracked or untracked-but-not-ignored.
pub fn files(repo: &Repo) -> Result<Vec<String>> {
    let out = repo.run_bytes(&[
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ])?;
    let mut files: Vec<String> = String::from_utf8_lossy(&out)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Keywords that introduce a named declaration in common languages.
const DECLARATIONS: &str = "class|interface|struct|enum|trait|record|object|type|impl|module|\
                            namespace|def|fn|func|function|fun|sub";

/// Declarations (class, struct, function...) whose name contains `name`, ignoring case.
pub fn symbols(repo: &Repo, name: &str, limit: usize) -> Result<Vec<Hit>> {
    let name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("(^|[^[:alnum:]_])({DECLARATIONS})[[:space:]]+[[:alnum:]_]*{name}");
    let opts = GrepOptions {
        regex: true,
        ..GrepOptions::default()
    };
    Ok(grep(repo, &pattern, opts, limit)?.0)
}

/// `text` as an extended regular expression that matches it literally (understood by both git
/// and the `regex` crate).
fn ere_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if ".[]()*+?{}|^$\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The pattern for a code search: the text itself (escaped unless `regex`), as a whole word if asked.
fn code_pattern(text: &str, opts: GrepOptions) -> String {
    let p = if opts.regex {
        text.to_string()
    } else {
        ere_escape(text)
    };
    if opts.whole_word {
        format!("(^|[^[:alnum:]_])({p})($|[^[:alnum:]_])")
    } else {
        p
    }
}

/// Matches the lines a code search is about, the same way git does for [`code_history`].
pub fn code_matcher(text: &str, opts: GrepOptions) -> Result<regex::Regex> {
    Ok(regex::RegexBuilder::new(&code_pattern(text, opts))
        .case_insensitive(!opts.case_sensitive)
        .build()?)
}

/// Commits of the current branch that added, removed or changed a line containing `text`
/// (`git log -G`), newest first, at most `limit`.
pub fn code_history(
    repo: &Repo,
    text: &str,
    opts: GrepOptions,
    limit: usize,
) -> Result<Vec<super::log::Commit>> {
    repo.log(&super::log::LogQuery {
        changed_lines: Some(code_pattern(text, opts)),
        ignore_case: !opts.case_sensitive,
        topo: false,
        max: limit,
        ..Default::default()
    })
}

/// What commit `rev` did to the searched code: only the hunks with an added or removed line
/// that `matcher` matches, in the files where that happened.
pub fn code_changes(
    repo: &Repo,
    rev: &str,
    matcher: &regex::Regex,
) -> Result<Vec<super::diff::FileDiff>> {
    use super::diff::LineKind;
    let mut diffs = repo.diff_commit(rev, &[], &super::diff::DiffOptions::default())?;
    for d in &mut diffs {
        d.hunks.retain(|h| {
            h.lines.iter().any(|l| {
                matches!(l.kind, LineKind::Added | LineKind::Removed)
                    && matcher.is_match(&l.content)
            })
        });
    }
    diffs.retain(|d| !d.hunks.is_empty());
    Ok(diffs)
}

/// Lines of `diffs` matching `matcher` that were (added, removed).
pub fn count_changed(diffs: &[super::diff::FileDiff], matcher: &regex::Regex) -> (usize, usize) {
    use super::diff::LineKind;
    let mut n = (0, 0);
    for l in diffs.iter().flat_map(|d| &d.hunks).flat_map(|h| &h.lines) {
        if matcher.is_match(&l.content) {
            match l.kind {
                LineKind::Added => n.0 += 1,
                LineKind::Removed => n.1 += 1,
                _ => {}
            }
        }
    }
    n
}

/// Name declared on `line` after one of the [`DECLARATIONS`] keywords (`class UserService` →
/// `UserService`).
fn declared_name(line: &str) -> Option<&str> {
    let keywords: Vec<&str> = DECLARATIONS.split('|').collect();
    let mut words = line
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty());
    while let Some(w) = words.next() {
        if keywords.contains(&w) {
            return words.next();
        }
    }
    None
}

/// Declarations of exactly `name` (a class, struct, function... called that), ignoring
/// declarations whose name only contains it.
pub fn declarations_of(repo: &Repo, name: &str) -> Result<Vec<Hit>> {
    let mut found = symbols(repo, name, 200)?;
    found.retain(|h| declared_name(&h.text).is_some_and(|n| n.eq_ignore_ascii_case(name)));
    // An exact-case match first.
    found.sort_by_key(|h| declared_name(&h.text) != Some(name));
    Ok(found)
}

/// True for a line that looks like the start of a method or function without a declaration
/// keyword (`public void login() {`, `int main(void)`), and not a statement such as `if (x) {`.
fn looks_like_signature(line: &str) -> bool {
    const STATEMENTS: &[&str] = &[
        "if",
        "else",
        "for",
        "foreach",
        "while",
        "do",
        "switch",
        "case",
        "catch",
        "try",
        "return",
        "throw",
        "new",
        "match",
        "when",
        "synchronized",
        "using",
        "lock",
        "with",
        "elif",
    ];
    let t = line.trim();
    let first = t
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    let Some(open) = t.find('(') else {
        return false;
    };
    let before = t[..open].trim_end();
    // `name(` preceded by something (a type or modifiers): `void login(`, not a call `login(`.
    let has_type = before.contains(|c: char| c.is_whitespace()) || before.ends_with('>');
    !first.is_empty()
        && !STATEMENTS.contains(&first)
        && has_type
        && !before.contains('=')
        && !t.starts_with("//")
        && !t.starts_with('*')
        && (t.ends_with('{') || t.ends_with(')') || t.ends_with(':') || t.ends_with(','))
}

/// The declaration (method, function, class...) that contains 1-based `line` of a file: the
/// nearest line above it, less indented, that declares something. 1-based.
pub fn enclosing_declaration(lines: &[String], line: usize) -> Option<usize> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let start = line.checked_sub(1)?.min(lines.len().checked_sub(1)?);
    let here = &lines[start];
    // The line itself may be the declaration.
    if declared_name(here).is_some() || looks_like_signature(here) {
        return Some(start + 1);
    }
    let mut limit = if here.trim().is_empty() {
        usize::MAX
    } else {
        indent(here)
    };
    for i in (0..start).rev() {
        let l = &lines[i];
        if l.trim().is_empty() {
            continue;
        }
        let ind = indent(l);
        if ind < limit {
            if declared_name(l).is_some() || looks_like_signature(l) {
                return Some(i + 1);
            }
            limit = ind;
        }
    }
    None
}

/// How well file `path` answers `query`, higher is better; `None` when it does not match.
///
/// As in an IDE: the query is matched against the file name first. `UsSe` finds
/// `UserService.java` (word starts), `userserv` finds it too, and a query with a `/` is
/// matched against the whole path.
pub fn file_score(path: &str, query: &str) -> Option<i64> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.split('.').next().unwrap_or(name);
    let (lname, lq) = (name.to_lowercase(), q.to_lowercase());
    let depth = path.matches('/').count() as i64;
    let len = name.chars().count() as i64;
    if q.contains('/') {
        return path
            .to_lowercase()
            .contains(&lq)
            .then_some(500 - depth - len);
    }
    let score = if stem.eq_ignore_ascii_case(q) || lname == lq {
        1000
    } else if lname.starts_with(&lq) {
        800
    } else if humps_match(stem, q) {
        700
    } else if lname.contains(&lq) {
        600
    } else if subsequence(&lname, &lq) {
        300
    } else if subsequence(&path.to_lowercase(), &lq) {
        100
    } else {
        return None;
    };
    Some(score - depth - len)
}

fn subsequence(text: &str, query: &str) -> bool {
    let mut chars = text.chars();
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|q| chars.any(|c| c == q))
}

/// The words of an identifier: `UserService` → `User`, `Service`; `user_service` → `user`, `service`.
fn words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            prev_lower = false;
            words.push(String::new());
            continue;
        }
        if words.is_empty() || (c.is_uppercase() && prev_lower) {
            words.push(String::new());
        }
        prev_lower = c.is_lowercase() || c.is_numeric();
        if let Some(w) = words.last_mut() {
            w.extend(c.to_lowercase());
        }
    }
    words.retain(|w| !w.is_empty());
    words
}

/// CamelHumps: each word of the query is the start of the next words of `name`, in order
/// (`UsSe` and `US` match `UserService`; `SeUs` does not).
fn humps_match(name: &str, query: &str) -> bool {
    let (name, query) = (words(name), words(query));
    if query.len() < 2 && query.first().map_or(true, |q| q.len() < 2) {
        return false;
    }
    // "US" is one word: also try its letters as separate humps.
    let letters: Vec<String> = query
        .iter()
        .flat_map(|w| w.chars().map(String::from))
        .collect();
    let fits = |parts: &[String]| {
        let mut it = name.iter();
        parts.iter().all(|p| it.any(|w| w.starts_with(p.as_str())))
    };
    fits(&query) || (query.len() == 1 && fits(&letters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn setup() -> (TempDir, Repo) {
        let (d, repo) = temp_repo();
        commit_files(
            &repo,
            &[
                (
                    "src/UserService.java",
                    "public class UserService {\n  void login() {}\n}\n",
                ),
                (
                    "src/util/strings.rs",
                    "pub struct Joiner;\npub fn join_all() {}\n// Login helper\n",
                ),
                (".gitignore", "target/\n"),
            ],
            "files",
        );
        write(&repo, "notes.txt", "remember the login flow\n");
        write(&repo, "target/out.txt", "login in an ignored file\n");
        (d, repo)
    }

    #[test]
    fn grep_finds_text_in_tracked_and_untracked_files_but_not_ignored_ones() {
        let (_d, repo) = setup();
        let (hits, more) = grep(&repo, "login", GrepOptions::default(), 100).unwrap();
        assert!(!more);
        let found: Vec<(&str, usize, usize)> = hits
            .iter()
            .map(|h| (h.path.as_str(), h.line, h.column))
            .collect();
        assert_eq!(
            found,
            [
                ("notes.txt", 1, 14),
                ("src/UserService.java", 2, 8),
                ("src/util/strings.rs", 3, 4)
            ]
        );
        assert_eq!(hits[1].text, "  void login() {}");

        let case = GrepOptions {
            case_sensitive: true,
            ..GrepOptions::default()
        };
        assert_eq!(grep(&repo, "Login", case, 100).unwrap().0.len(), 1);
        let word = GrepOptions {
            whole_word: true,
            ..GrepOptions::default()
        };
        assert!(grep(&repo, "logi", word, 100).unwrap().0.is_empty());
        assert!(grep(&repo, "zzz-not-there", GrepOptions::default(), 100)
            .unwrap()
            .0
            .is_empty());
    }

    #[test]
    fn grep_regex_limit_and_bad_pattern() {
        let (_d, repo) = setup();
        let re = GrepOptions {
            regex: true,
            ..GrepOptions::default()
        };
        let (hits, _) = grep(&repo, "(class|struct) [A-Z]", re, 100).unwrap();
        assert_eq!(hits.len(), 2);
        // Plain text mode takes the same characters literally.
        assert!(
            grep(&repo, "(class|struct) [A-Z]", GrepOptions::default(), 100)
                .unwrap()
                .0
                .is_empty()
        );
        let (hits, more) = grep(&repo, "login", GrepOptions::default(), 2).unwrap();
        assert_eq!((hits.len(), more), (2, true));
        let err = grep(&repo, "(unclosed", re, 100).unwrap_err();
        assert!(!format!("{err:#}").is_empty());
    }

    #[test]
    fn files_and_symbols() {
        let (_d, repo) = setup();
        assert_eq!(
            files(&repo).unwrap(),
            [
                ".gitignore",
                "notes.txt",
                "src/UserService.java",
                "src/util/strings.rs"
            ]
        );
        let found = symbols(&repo, "userserv", 50).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].path.as_str(), found[0].line),
            ("src/UserService.java", 1)
        );
        let found = symbols(&repo, "join", 50).unwrap();
        let lines: Vec<usize> = found.iter().map(|h| h.line).collect();
        assert_eq!(lines, [1, 2], "struct Joiner and fn join_all");
        assert!(symbols(&repo, "login helper", 50).unwrap().is_empty());
    }

    #[test]
    fn enclosing_declarations_in_several_languages() {
        let lines = |s: &str| s.lines().map(str::to_string).collect::<Vec<_>>();
        let java = lines(
            "public class Auth {\n    private int n;\n\n    public void login(String u) {\n        if (u != null) {\n            check(u);\n        }\n    }\n}\n",
        );
        assert_eq!(
            enclosing_declaration(&java, 6),
            Some(4),
            "method, not the if"
        );
        assert_eq!(enclosing_declaration(&java, 2), Some(1), "field: the class");
        assert_eq!(
            enclosing_declaration(&java, 4),
            Some(4),
            "the declaration itself"
        );
        let py = lines("class A:\n    def run(self):\n        x = call(1)\n");
        assert_eq!(enclosing_declaration(&py, 3), Some(2));
        let rs = lines("impl Foo {\n    pub fn go(&self) {\n        let x = bar(1);\n    }\n}\n");
        assert_eq!(enclosing_declaration(&rs, 3), Some(2));
        assert_eq!(enclosing_declaration(&lines("x = 1\n"), 1), None);
        assert_eq!(enclosing_declaration(&[], 1), None);
    }

    #[test]
    fn declarations_of_a_name_are_exact_and_code_changes_keep_matching_hunks() {
        let (_d, repo) = setup();
        let found = declarations_of(&repo, "Joiner").unwrap();
        assert_eq!(found.len(), 1);
        assert!(declarations_of(&repo, "Join").unwrap().is_empty());
        let m = code_matcher("JOIN_ALL", GrepOptions::default()).unwrap();
        let changes = code_changes(&repo, "HEAD", &m).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(count_changed(&changes, &m), (1, 0));
        let history = code_history(&repo, "fn join_all(", GrepOptions::default(), 10).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn file_names_rank_like_in_an_ide() {
        let s = |p: &str, q: &str| file_score(p, q);
        assert!(s("src/UserService.java", "UsSe").is_some(), "word starts");
        assert!(s("src/UserService.java", "US").is_some());
        assert!(
            s("src/UserService.java", "SeUs").is_none()
                || s("src/UserService.java", "SeUs") < s("src/UserService.java", "UsSe")
        );
        assert!(s("src/user_service.py", "usse").is_some());
        assert!(s("src/Other.java", "UsSe").is_none());
        // Exact name beats prefix beats a match somewhere in the name beats one in the path.
        let ranked = [
            s("a/User.java", "user"),
            s("a/UserService.java", "user"),
            s("a/SuperUser.java", "user"),
            s("user/Thing.java", "user"),
        ];
        assert!(ranked.windows(2).all(|w| w[0] > w[1]), "{ranked:?}");
        // A slash searches the path.
        assert!(s("src/util/strings.rs", "util/str").is_some());
        assert!(s("src/UserService.java", "util/str").is_none());
        assert_eq!(s("anything", " "), Some(0));
    }
}
