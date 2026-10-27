//! Blame/annotate (`git blame --porcelain`).
//!
//! [`Repo::blame`] annotates a file in the work tree or at a revision. Commit metadata is printed by
//! git only for the first line of each commit, so the parser keeps a per-commit table and copies the
//! information onto every [`BlameLine`].

use std::collections::HashMap;

use anyhow::Result;

use super::Repo;

/// The all-zero hash git uses for lines that are not committed yet.
pub const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

/// Author name git reports for uncommitted lines.
pub const NOT_COMMITTED_YET: &str = "Not Committed Yet";

/// One annotated line of the blamed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// Full hash of the commit that last changed the line ([`UNCOMMITTED`] for local changes)
    pub commit: String,
    /// Line number in the commit that introduced the line (1-based)
    pub orig_line: u32,
    /// Line number in the blamed version of the file (1-based)
    pub final_line: u32,
    pub author: String,
    /// Author e-mail without the angle brackets
    pub author_mail: String,
    /// Author time (unix seconds)
    pub author_time: i64,
    /// Commit subject
    pub summary: String,
    /// Path of the file in `commit` (differs from the blamed path across renames)
    pub orig_path: String,
    /// Parent commit and the file's path there (`None` for root/boundary commits)
    pub previous: Option<(String, String)>,
    /// Line text without the trailing newline (CR stripped)
    pub content: String,
}

impl BlameLine {
    /// True for lines changed in the work tree / index but not committed yet.
    pub fn is_uncommitted(&self) -> bool {
        self.commit == UNCOMMITTED
    }

    /// Abbreviated hash for the gutter.
    pub fn short(&self) -> &str {
        &self.commit[..self.commit.len().min(8)]
    }
}

/// Per-commit header data printed by git once per commit.
#[derive(Debug, Clone, Default)]
struct CommitInfo {
    author: String,
    author_mail: String,
    author_time: i64,
    summary: String,
    filename: String,
    previous: Option<(String, String)>,
}

impl Repo {
    /// Annotate `path` at `rev`, or the work tree version when `rev` is `None`.
    ///
    /// In an empty repository (no HEAD) every line of the work tree file is reported as
    /// "Not Committed Yet".
    pub fn blame(&self, path: &str, rev: Option<&str>) -> Result<Vec<BlameLine>> {
        if rev.is_none() && self.head_commit().is_none() {
            let bytes = std::fs::read(self.path(path))?;
            return Ok(uncommitted_lines(path, &String::from_utf8_lossy(&bytes)));
        }
        let mut args = vec!["blame", "--porcelain"];
        if let Some(r) = rev {
            args.push(r);
        }
        args.extend(["--", path]);
        let out = self.run_bytes(&args)?;
        Ok(parse_porcelain(&String::from_utf8_lossy(&out)))
    }
}

/// Synthetic blame where every line is uncommitted.
fn uncommitted_lines(path: &str, text: &str) -> Vec<BlameLine> {
    text.lines()
        .enumerate()
        .map(|(i, l)| BlameLine {
            commit: UNCOMMITTED.to_string(),
            orig_line: i as u32 + 1,
            final_line: i as u32 + 1,
            author: NOT_COMMITTED_YET.to_string(),
            author_mail: "not.committed.yet".to_string(),
            author_time: chrono::Utc::now().timestamp(),
            summary: NOT_COMMITTED_YET.to_string(),
            orig_path: path.to_string(),
            previous: None,
            content: l.strip_suffix('\r').unwrap_or(l).to_string(),
        })
        .collect()
}

fn is_hash(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse `git blame --porcelain` output.
pub fn parse_porcelain(out: &str) -> Vec<BlameLine> {
    let mut infos: HashMap<String, CommitInfo> = HashMap::new();
    let mut lines = Vec::new();
    // (commit, orig_line, final_line) of the header currently being read
    let mut cur: Option<(String, u32, u32)> = None;

    for raw in out.split('\n') {
        if let Some(content) = raw.strip_prefix('\t') {
            let Some((commit, orig_line, final_line)) = cur.take() else {
                continue;
            };
            let info = infos.get(&commit).cloned().unwrap_or_default();
            lines.push(BlameLine {
                commit,
                orig_line,
                final_line,
                author: info.author,
                author_mail: info.author_mail,
                author_time: info.author_time,
                summary: info.summary,
                orig_path: info.filename,
                previous: info.previous,
                content: content.strip_suffix('\r').unwrap_or(content).to_string(),
            });
            continue;
        }
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let (key, value) = raw.split_once(' ').unwrap_or((raw, ""));
        if is_hash(key) {
            let mut nums = value.split(' ').map(|n| n.parse::<u32>().unwrap_or(0));
            let orig = nums.next().unwrap_or(0);
            let fin = nums.next().unwrap_or(0);
            infos.entry(key.to_string()).or_default();
            cur = Some((key.to_string(), orig, fin));
            continue;
        }
        let Some((commit, _, _)) = &cur else {
            continue;
        };
        let Some(info) = infos.get_mut(commit) else {
            continue;
        };
        match key {
            "author" => info.author = value.to_string(),
            "author-mail" => {
                info.author_mail = value
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            }
            "author-time" => info.author_time = value.parse().unwrap_or(0),
            "summary" => info.summary = value.to_string(),
            "filename" => info.filename = crate::git::diff::unquote(value),
            "previous" => {
                info.previous = value
                    .split_once(' ')
                    .map(|(h, p)| (h.to_string(), crate::git::diff::unquote(p)))
            }
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn commit_as(repo: &Repo, name: &str, msg: &str) -> String {
        repo.run(&["add", "-A"]).unwrap();
        let user = format!("user.name={name}");
        let mail = format!("user.email={}@example.com", name.to_lowercase());
        repo.run(&["-c", &user, "-c", &mail, "commit", "-q", "-m", msg])
            .unwrap();
        repo.head_commit().unwrap()
    }

    #[test]
    fn blame_multiple_authors_and_uncommitted() {
        let (_d, repo) = temp_repo();
        write(&repo, "src/a b.txt", "one\ntwo\nthree\n");
        let c1 = commit_as(&repo, "Alice", "first");
        write(&repo, "src/a b.txt", "one\nTWO\nthree\nfour\n");
        let c2 = commit_as(&repo, "Bob", "second");
        write(&repo, "src/a b.txt", "one\nTWO\nthree\nfour\nfive\r\n");

        let lines = repo.blame("src/a b.txt", None).unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].commit, c1);
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[0].author_mail, "alice@example.com");
        assert_eq!(lines[0].summary, "first");
        assert!(lines[0].previous.is_none());
        // Info for repeated commits is reused from the first occurrence.
        assert_eq!(lines[2].commit, c1);
        assert_eq!(lines[2].author, "Alice");
        assert_eq!(lines[2].content, "three");
        assert_eq!(lines[1].commit, c2);
        assert_eq!(lines[1].author, "Bob");
        assert_eq!(
            lines[1].previous.as_ref().map(|p| p.0.as_str()),
            Some(c1.as_str())
        );
        assert_eq!(lines[3].final_line, 4);
        assert!(lines[4].is_uncommitted());
        assert_eq!(lines[4].author, NOT_COMMITTED_YET);
        assert_eq!(lines[4].content, "five");

        let at_c1 = repo.blame("src/a b.txt", Some(&c1)).unwrap();
        assert_eq!(at_c1.len(), 3);
        assert!(at_c1.iter().all(|l| l.commit == c1));
        assert!(at_c1[0].author_time > 0);
    }

    #[test]
    fn blame_follows_rename_and_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "new.txt", "a\nb\n");
        let empty = repo.blame("new.txt", None).unwrap();
        assert_eq!(empty.len(), 2);
        assert!(empty.iter().all(BlameLine::is_uncommitted));

        let c1 = commit_files(&repo, &[("old.txt", "x\ny\n")], "add old");
        repo.run(&["mv", "old.txt", "moved.txt"]).unwrap();
        repo.run(&["commit", "-q", "-m", "rename"]).unwrap();
        let lines = repo.blame("moved.txt", Some("HEAD")).unwrap();
        assert_eq!(lines[0].commit, c1);
        assert_eq!(lines[0].orig_path, "old.txt");
        assert!(repo.blame("untracked-nope.txt", None).is_err());
    }

    #[test]
    fn parse_porcelain_sample() {
        let h1 = "a".repeat(40);
        let h2 = "b".repeat(40);
        let out = format!(
            "{h1} 1 1 2\nauthor A\nauthor-mail <a@x>\nauthor-time 100\nsummary s1\nboundary\nfilename f\n\tl1\r\n\
{h1} 2 2\n\tl2\n{h2} 5 3 1\nauthor B\nauthor-mail <b@x>\nauthor-time 200\nsummary s2\nprevious {h1} \"o\\tld\"\nfilename f\n\tl3\n"
        );
        let l = parse_porcelain(&out);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].content, "l1");
        assert_eq!(l[1].author, "A");
        assert_eq!(l[1].final_line, 2);
        assert_eq!(l[2].orig_line, 5);
        assert_eq!(l[2].author_time, 200);
        assert_eq!(l[2].previous, Some((h1.clone(), "o\tld".to_string())));
        assert_eq!(l[2].short(), "bbbbbbbb");
    }
}
