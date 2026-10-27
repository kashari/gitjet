//! Commit history queries.

use anyhow::Result;

use super::Repo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time (unix seconds)
    pub time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub subject: String,
    /// Decorations like `HEAD -> main`, `origin/main`, `tag: v1.0`
    pub refs: Vec<String>,
}

impl Commit {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// Filters for [`Repo::log`]. Defaults: HEAD only, newest first, 500 commits.
#[derive(Debug, Clone)]
pub struct LogQuery {
    /// Revisions/ranges to walk (empty = HEAD, unless `all`)
    pub revs: Vec<String>,
    /// Walk all branches, remotes and tags (`--all` without `refs/stash`)
    pub all: bool,
    pub author: Option<String>,
    /// Message search (`--grep`, case-insensitive)
    pub grep: Option<String>,
    /// Commits whose diff adds/removes this string (`-S`)
    pub pickaxe: Option<String>,
    /// e.g. "2 weeks ago", "2024-01-01"
    pub since: Option<String>,
    pub until: Option<String>,
    pub paths: Vec<String>,
    /// Follow renames (only valid with exactly one path)
    pub follow: bool,
    pub first_parent: bool,
    pub no_merges: bool,
    pub max: usize,
    pub skip: usize,
    /// Topological order (best for graphs); otherwise date order
    pub topo: bool,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            revs: Vec::new(),
            all: false,
            author: None,
            grep: None,
            pickaxe: None,
            since: None,
            until: None,
            paths: Vec::new(),
            follow: false,
            first_parent: false,
            no_merges: false,
            max: 500,
            skip: 0,
            topo: true,
        }
    }
}

const FORMAT: &str =
    "--format=%H%x00%h%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%s%x00%D%x1e";

impl Repo {
    pub fn log(&self, q: &LogQuery) -> Result<Vec<Commit>> {
        let mut args: Vec<String> = vec![
            "log".into(),
            FORMAT.into(),
            "--decorate=full".into(),
            format!("--max-count={}", q.max),
            format!("--skip={}", q.skip),
        ];
        if q.topo {
            args.push("--topo-order".into());
        }
        if q.all {
            // Every branch, remote and tag, but not stash internals.
            args.push("--exclude=refs/stash".into());
            args.push("--all".into());
        }
        if let Some(a) = &q.author {
            args.push(format!("--author={a}"));
            args.push("--regexp-ignore-case".into());
        }
        if let Some(g) = &q.grep {
            args.push(format!("--grep={g}"));
            args.push("--regexp-ignore-case".into());
        }
        if let Some(s) = &q.pickaxe {
            args.push(format!("-S{s}"));
        }
        if let Some(s) = &q.since {
            args.push(format!("--since={s}"));
        }
        if let Some(u) = &q.until {
            args.push(format!("--until={u}"));
        }
        if q.follow && q.paths.len() == 1 {
            args.push("--follow".into());
        }
        if q.first_parent {
            args.push("--first-parent".into());
        }
        if q.no_merges {
            args.push("--no-merges".into());
        }
        args.extend(q.revs.iter().cloned());
        args.push("--".into());
        args.extend(q.paths.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.run(&refs) {
            Ok(out) => Ok(parse_log(&out)),
            // An empty repository has no HEAD to walk; checking only on failure saves a process.
            Err(_) if q.revs.is_empty() && self.head_commit().is_none() => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// A single commit's metadata.
    pub fn commit_info(&self, rev: &str) -> Result<Commit> {
        let out = self.run(&["show", "-s", FORMAT, "--decorate=full", rev, "--"])?;
        parse_log(&out)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("commit {rev} not found"))
    }

    /// Full commit message (subject + body).
    pub fn commit_message(&self, rev: &str) -> Result<String> {
        Ok(self
            .run(&["show", "-s", "--format=%B", rev, "--"])?
            .trim_end()
            .to_string())
    }

    /// Files changed by a commit as (status letter, path, old path for renames). First parent for merges.
    pub fn commit_files(&self, rev: &str) -> Result<Vec<(char, String, Option<String>)>> {
        let out = self.run(&[
            "show",
            "--format=",
            "--name-status",
            "-z",
            "-M",
            "--diff-merges=first-parent",
            rev,
            "--",
        ])?;
        let mut res = Vec::new();
        let mut it = out.split('\0').filter(|s| !s.is_empty());
        while let Some(st) = it.next() {
            let st = st.trim();
            let c = st.chars().next().unwrap_or('M');
            if c == 'R' || c == 'C' {
                let old = it.next().unwrap_or_default().to_string();
                let new = it.next().unwrap_or_default().to_string();
                res.push((c, new, Some(old)));
            } else if let Some(p) = it.next() {
                res.push((c, p.to_string(), None));
            }
        }
        Ok(res)
    }
}

/// Shorten `refs/heads/x` → `x`, `refs/remotes/o/x` → `o/x`, `refs/tags/v` → `tag: v`.
pub fn short_ref(r: &str) -> String {
    let r = r.trim();
    if let Some(rest) = r.strip_prefix("HEAD -> ") {
        return format!("HEAD -> {}", short_ref(rest));
    }
    if let Some(rest) = r.strip_prefix("tag: ") {
        return format!("tag: {}", rest.trim_start_matches("refs/tags/"));
    }
    r.trim_start_matches("refs/heads/")
        .trim_start_matches("refs/remotes/")
        .to_string()
}

pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\n', '\r']);
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.split('\0').collect();
            if f.len() < 11 {
                return None;
            }
            Some(Commit {
                id: f[0].to_string(),
                short: f[1].to_string(),
                parents: f[2].split_whitespace().map(str::to_string).collect(),
                author: f[3].to_string(),
                email: f[4].to_string(),
                time: f[5].parse().unwrap_or(0),
                committer: f[6].to_string(),
                committer_email: f[7].to_string(),
                commit_time: f[8].parse().unwrap_or(0),
                subject: f[9].to_string(),
                refs: f[10]
                    .split(", ")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(short_ref)
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    #[test]
    fn log_and_files() {
        let (_d, repo) = temp_repo();
        assert!(repo.log(&LogQuery::default()).unwrap().is_empty());
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        let c2 = commit_files(&repo, &[("b.txt", "2\n")], "second\n\nbody text");
        repo.run(&["tag", "v1"]).unwrap();
        let log = repo.log(&LogQuery::default()).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].id, c2);
        assert_eq!(log[0].subject, "second");
        assert!(log[0].refs.iter().any(|r| r == "HEAD -> main"));
        assert!(log[0].refs.iter().any(|r| r == "tag: v1"));
        assert_eq!(log[1].parents.len(), 0);
        assert_eq!(log[0].parents.len(), 1);
        assert_eq!(repo.commit_message(&c2).unwrap(), "second\n\nbody text");
        let files = repo.commit_files(&c2).unwrap();
        assert_eq!(files, vec![('A', "b.txt".to_string(), None)]);
        let q = LogQuery {
            grep: Some("FIRST".into()),
            ..Default::default()
        };
        assert_eq!(repo.log(&q).unwrap().len(), 1);
        let q = LogQuery {
            paths: vec!["b.txt".into()],
            ..Default::default()
        };
        assert_eq!(repo.log(&q).unwrap().len(), 1);
        assert_eq!(repo.commit_info("HEAD").unwrap().id, c2);
    }

    #[test]
    fn short_refs() {
        assert_eq!(short_ref("HEAD -> refs/heads/main"), "HEAD -> main");
        assert_eq!(short_ref("refs/remotes/origin/dev"), "origin/dev");
        assert_eq!(short_ref("tag: refs/tags/v1"), "tag: v1");
    }

    #[test]
    fn all_branches_hides_stash() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        write(&repo, "a.txt", "2\n");
        repo.run(&["stash", "push", "-m", "wip"]).unwrap();
        repo.run(&["branch", "other"]).unwrap();
        let q = LogQuery {
            all: true,
            ..Default::default()
        };
        let log = repo.log(&q).unwrap();
        assert_eq!(log.len(), 1);
        assert!(log.iter().all(|c| !c.subject.contains("wip")));
    }
}
