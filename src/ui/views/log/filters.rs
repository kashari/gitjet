//! Log filters (branch / user / date / paths / text search) and their dialogs.

use std::collections::HashMap;

use anyhow::Result;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::log::{short_ref, Commit, LogQuery};
use crate::git::Repo;
use crate::ui::popups::{InputPopup, MenuItem, MenuPopup};

use super::mailbox::{post, Msg};

/// Which commits the log walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchFilter {
    /// Every ref (`--all`)
    All,
    /// The current branch / HEAD only
    Head,
    /// One branch, tag or revision
    Rev(String),
}

/// What the text filter searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    /// Commit messages (`--grep`)
    Message,
    /// Added/removed content (`-S`, "pickaxe")
    Diff,
}

/// Author filter: `label` is shown in the title, `pattern` is matched (as a fixed string) against
/// "name <email>".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFilter {
    pub label: String,
    pub pattern: String,
}

/// All active log filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filters {
    pub branch: BranchFilter,
    pub user: Option<UserFilter>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub paths: Vec<String>,
    pub text: Option<String>,
    pub text_mode: TextMode,
    pub first_parent: bool,
    pub no_merges: bool,
}

impl Filters {
    /// No filters; `all_branches` picks `--all` vs HEAD.
    pub fn new(all_branches: bool) -> Self {
        Self {
            branch: if all_branches {
                BranchFilter::All
            } else {
                BranchFilter::Head
            },
            user: None,
            since: None,
            until: None,
            paths: Vec::new(),
            text: None,
            text_mode: TextMode::Message,
            first_parent: false,
            no_merges: false,
        }
    }

    /// Reset everything except the All/HEAD default.
    pub fn clear(&mut self, all_branches: bool) {
        *self = Self::new(all_branches);
    }

    /// Flip between all branches and HEAD (a specific revision counts as "not all").
    pub fn toggle_all(&mut self) {
        self.branch = match self.branch {
            BranchFilter::All => BranchFilter::Head,
            _ => BranchFilter::All,
        };
    }

    /// True when filters hide commits from the middle of the history, so parents of listed commits are
    /// often not listed (the graph then only connects commits that are both shown).
    pub fn hides_commits(&self) -> bool {
        self.user.is_some() || self.text.is_some() || self.no_merges
    }

    /// True when every commit between a listed commit and a listed ancestor is listed too, so ancestry
    /// can be decided from the loaded commits alone (topological order puts children first).
    pub fn keeps_ancestry(&self) -> bool {
        !self.hides_commits()
            && self.since.is_none()
            && self.until.is_none()
            && self.paths.is_empty()
            && !self.first_parent
    }

    /// Clear the filters that can hide a specific commit (user, text, date, path, no merges), keeping
    /// the branch and first-parent choices. Returns true when anything changed.
    pub fn clear_hiding(&mut self) -> bool {
        let before = self.clone();
        self.user = None;
        self.text = None;
        self.since = None;
        self.until = None;
        self.paths.clear();
        self.no_merges = false;
        *self != before
    }

    /// True when any filter besides the branch choice is active.
    pub fn is_filtered(&self) -> bool {
        self.hides_commits()
            || self.since.is_some()
            || self.until.is_some()
            || !self.paths.is_empty()
            || self.first_parent
            || matches!(self.branch, BranchFilter::Rev(_))
    }

    /// The git query for one page of `max` commits starting at `skip`.
    pub fn to_query(&self, skip: usize, max: usize) -> LogQuery {
        let mut q = LogQuery {
            max,
            skip,
            topo: true,
            first_parent: self.first_parent,
            no_merges: self.no_merges,
            since: self.since.clone(),
            until: self.until.clone(),
            paths: self.paths.clone(),
            ..LogQuery::default()
        };
        match &self.branch {
            BranchFilter::All => q.all = true,
            BranchFilter::Head => {}
            BranchFilter::Rev(r) => q.revs.push(r.clone()),
        }
        q.author = self.user.as_ref().map(|u| u.pattern.clone());
        match (self.text_mode, &self.text) {
            (TextMode::Message, Some(t)) => q.grep = Some(t.clone()),
            (TextMode::Diff, Some(t)) => q.pickaxe = Some(t.clone()),
            _ => {}
        }
        // Extra `git log` options travel in `revs` (they are placed before `--`):
        // patterns are plain text, and path filters rewrite parents so the graph
        // connects the commits that touch the path ([`restore_real_parents`] undoes that for actions).
        if q.author.is_some() || q.grep.is_some() {
            q.revs.push("--fixed-strings".into());
        }
        if !q.paths.is_empty() {
            q.revs.push("--parents".into());
        }
        q
    }

    /// Short descriptions of the active filters for the panel title.
    pub fn describe(&self) -> Vec<String> {
        let mut v = vec![match &self.branch {
            BranchFilter::All => "all branches".to_string(),
            BranchFilter::Head => "HEAD".to_string(),
            BranchFilter::Rev(r) => format!("branch: {}", shorten_rev(r)),
        }];
        if let Some(u) = &self.user {
            v.push(format!("user: {}", u.label));
        }
        if let Some(s) = &self.since {
            v.push(format!("since: {s}"));
        }
        if let Some(u) = &self.until {
            v.push(format!("until: {u}"));
        }
        match self.paths.as_slice() {
            [] => {}
            [one] => v.push(format!("path: {one}")),
            [first, rest @ ..] => v.push(format!("paths: {first} +{}", rest.len())),
        }
        if let Some(t) = &self.text {
            let what = match self.text_mode {
                TextMode::Message => "text",
                TextMode::Diff => "diff",
            };
            v.push(format!("{what}: \"{t}\""));
        }
        if self.first_parent {
            v.push("first-parent".into());
        }
        if self.no_merges {
            v.push("no merges".into());
        }
        v
    }
}

/// With a path filter, `--parents` rewrites each commit's parents to the previous commits that touch
/// the path. That is right for the graph but wrong for everything else (compare with previous, revert,
/// blame of deleted files, the details pane), so this puts the real parents back into `commits` and
/// returns the rewritten ones (by commit id) for the graph layout.
pub fn restore_real_parents(
    repo: &Repo,
    commits: &mut [Commit],
) -> Result<HashMap<String, Vec<String>>> {
    if commits.is_empty() {
        return Ok(HashMap::new());
    }
    let input: String = commits.iter().map(|c| format!("{}\n", c.id)).collect();
    let out = repo.run_stdin(
        &["rev-list", "--no-walk=unsorted", "--parents", "--stdin"],
        input.as_bytes(),
    )?;
    let mut real: HashMap<&str, Vec<String>> = HashMap::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        if let Some(id) = it.next() {
            real.insert(id, it.map(str::to_string).collect());
        }
    }
    let mut rewritten = HashMap::new();
    for c in commits.iter_mut() {
        if let Some(parents) = real.get(c.id.as_str()) {
            let graph_parents = std::mem::replace(&mut c.parents, parents.clone());
            rewritten.insert(c.id.clone(), graph_parents);
        }
    }
    Ok(rewritten)
}

/// Display form of a revision: full hashes abbreviated, `refs/...` prefixes removed.
fn shorten_rev(r: &str) -> String {
    if r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()) {
        r[..10].to_string()
    } else {
        short_ref(r.strip_prefix("refs/tags/").unwrap_or(r))
    }
}

/// Split a path filter input: `;` separates several paths, surrounding whitespace is ignored and
/// Windows separators are normalized (paths may contain spaces).
pub fn parse_paths(input: &str) -> Vec<String> {
    input
        .split(';')
        .map(|p| p.trim().replace('\\', "/"))
        .map(|p| p.trim_start_matches("./").to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Authors of `commits` as "name <email>", most frequent first (at most `limit`).
pub fn authors(commits: &[Commit], limit: usize) -> Vec<(String, String)> {
    let mut counts: Vec<((String, String), usize)> = Vec::new();
    for c in commits {
        let key = (c.author.clone(), c.email.clone());
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
        if counts.len() > 5000 {
            break;
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().take(limit).map(|(k, _)| k).collect()
}

fn edit(f: impl FnOnce(&mut Filters) + 'static) -> Action {
    post(Msg::Edit(Box::new(f)));
    Action::None
}

fn current_label(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "any".into())
}

/// The `f` filter menu.
pub fn filter_menu(f: &Filters, commits: &[Commit], me: Option<(String, String)>) -> Action {
    let branch = match &f.branch {
        BranchFilter::All => "all".to_string(),
        BranchFilter::Head => "HEAD".to_string(),
        BranchFilter::Rev(r) => shorten_rev(r),
    };
    let user = f
        .user
        .as_ref()
        .map(|u| u.label.clone())
        .unwrap_or_else(|| "any".into());
    let date = match (&f.since, &f.until) {
        (None, None) => "any".to_string(),
        (s, u) => format!("{} … {}", current_label(s), current_label(u)),
    };
    let path = if f.paths.is_empty() {
        "any".to_string()
    } else {
        f.paths.join("; ")
    };
    let text = current_label(&f.text);
    let mode = match f.text_mode {
        TextMode::Message => "commit messages",
        TextMode::Diff => "diff content (-S)",
    };
    let on = |b: bool| if b { "on" } else { "off" };
    let author_list = authors(commits, 300);
    let path_initial = f.paths.join("; ");
    let text_initial = f.text.clone().unwrap_or_default();
    let (since, until) = (f.since.clone(), f.until.clone());
    let items = vec![
        MenuItem::new(format!("Branch: {branch}…"), "b", branch_picker),
        MenuItem::new(format!("User: {user}…"), "u", move |_| {
            user_picker(author_list, me)
        }),
        MenuItem::new(format!("Date: {date}…"), "d", move |_| {
            date_menu(since, until)
        }),
        MenuItem::new(format!("Path: {path}…"), "p", move |_| {
            path_input(&path_initial)
        }),
        MenuItem::new(format!("Text: {text}…"), "t", move |_| {
            text_input(&text_initial)
        }),
        MenuItem::new(format!("Search text in: {mode}"), "s", |_| {
            edit(|f| {
                f.text_mode = match f.text_mode {
                    TextMode::Message => TextMode::Diff,
                    TextMode::Diff => TextMode::Message,
                }
            })
        }),
        MenuItem::new(
            format!("First-parent only: {}", on(f.first_parent)),
            "F",
            |_| edit(|f| f.first_parent = !f.first_parent),
        ),
        MenuItem::new(format!("No merges: {}", on(f.no_merges)), "n", |_| {
            edit(|f| f.no_merges = !f.no_merges)
        }),
        MenuItem::new("Clear all filters", "x", |_| {
            post(Msg::ClearFilters);
            Action::None
        }),
    ];
    Action::popup(MenuPopup::new("Filter log", items))
}

/// Pick All / HEAD / one branch (local and remote). Usable from the palette.
pub fn branch_picker(ctx: &Ctx) -> Action {
    let out = match ctx.repo.run(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ]) {
        Ok(o) => o,
        Err(e) => return Action::error(format!("{e:#}")),
    };
    let mut items = vec![
        MenuItem::new("All branches", "", |_| {
            edit(|f| f.branch = BranchFilter::All)
        }),
        MenuItem::new("HEAD (current branch)", "", |_| {
            edit(|f| f.branch = BranchFilter::Head)
        }),
    ];
    for full in out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
    {
        let label = shorten_rev(full);
        let label = if full.starts_with("refs/tags/") {
            format!("tag: {label}")
        } else {
            label
        };
        // The full ref name is unambiguous even when a branch and a tag share a name.
        let rev = full.to_string();
        items.push(MenuItem::new(label, "", move |_| {
            edit(move |f| f.branch = BranchFilter::Rev(rev))
        }));
    }
    Action::popup(MenuPopup::new("Filter by branch", items).filterable())
}

/// Pick an author: anyone, me, one of the authors seen in the log, or type a name.
pub fn user_picker(authors: Vec<(String, String)>, me: Option<(String, String)>) -> Action {
    let mut items = vec![MenuItem::new("Any user", "", |_| edit(|f| f.user = None))];
    if let Some((name, email)) = me {
        items.push(MenuItem::new(
            format!("me ({name} <{email}>)"),
            "",
            move |_| {
                edit(move |f| {
                    f.user = Some(UserFilter {
                        label: "me".into(),
                        pattern: format!("<{email}>"),
                    })
                })
            },
        ));
    }
    items.push(MenuItem::new("Type a name or email…", "", |_| {
        user_input()
    }));
    for (name, email) in authors {
        items.push(MenuItem::new(format!("{name} <{email}>"), "", move |_| {
            edit(move |f| {
                f.user = Some(UserFilter {
                    label: name.clone(),
                    pattern: format!("{name} <{email}>"),
                })
            })
        }));
    }
    Action::popup(MenuPopup::new("Filter by user", items).filterable())
}

/// Free-text author filter. `me` stands for the configured `user.email`.
pub fn user_input() -> Action {
    Action::popup(
        InputPopup::new("Filter by user", "", |text, ctx| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return edit(|f| f.user = None);
            }
            let user = if text.eq_ignore_ascii_case("me") {
                match ctx.repo.config_get("user.email") {
                    Some(email) => UserFilter {
                        label: "me".into(),
                        pattern: format!("<{email}>"),
                    },
                    None => return Action::Error("user.email is not configured".into()),
                }
            } else {
                UserFilter {
                    label: text.clone(),
                    pattern: text,
                }
            };
            edit(move |f| f.user = Some(user))
        })
        .prompt("Part of an author name or email; \"me\" for yourself; empty for anyone")
        .allow_empty(),
    )
}

/// Date presets plus free-form since/until.
pub fn date_menu(since: Option<String>, until: Option<String>) -> Action {
    let preset = |label: &str, key: &str, value: &'static str| {
        MenuItem::new(label.to_string(), key.to_string(), move |_| {
            edit(move |f| {
                f.since = Some(value.to_string());
                f.until = None;
            })
        })
    };
    let since_initial = since.unwrap_or_default();
    let until_initial = until.unwrap_or_default();
    let items = vec![
        preset("Last 24 hours", "1", "24 hours ago"),
        preset("Last 7 days", "7", "7 days ago"),
        preset("Last 30 days", "3", "30 days ago"),
        preset("Last year", "y", "1 year ago"),
        MenuItem::new("Since…", "s", move |_| {
            date_input("Show commits since", &since_initial, |v, f| f.since = v)
        }),
        MenuItem::new("Until…", "u", move |_| {
            date_input("Show commits until", &until_initial, |v, f| f.until = v)
        }),
        MenuItem::new("Any date", "x", |_| {
            edit(|f| {
                f.since = None;
                f.until = None;
            })
        }),
    ];
    Action::popup(MenuPopup::new("Filter by date", items))
}

fn date_input(title: &str, initial: &str, apply: fn(Option<String>, &mut Filters)) -> Action {
    Action::popup(
        InputPopup::new(title, initial, move |text, _| {
            let v = Some(text.trim().to_string()).filter(|s| !s.is_empty());
            edit(move |f| apply(v, f))
        })
        .prompt("Any date git understands: \"2 weeks ago\", \"yesterday\", \"2024-01-31\"; empty to clear")
        .allow_empty(),
    )
}

/// Path filter input (several paths separated by `;`).
pub fn path_input(initial: &str) -> Action {
    Action::popup(
        InputPopup::new("Filter by path", initial, |text, _| {
            let paths = parse_paths(&text);
            edit(move |f| f.paths = paths)
        })
        .prompt("File or directory relative to the repository root; separate several with ';'; empty to clear")
        .allow_empty(),
    )
}

/// Quick text filter (`/`).
pub fn text_input(initial: &str) -> Action {
    Action::popup(
        InputPopup::new("Filter log by text", initial, |text, _| {
            let t = Some(text.trim().to_string()).filter(|s| !s.is_empty());
            edit(move |f| f.text = t)
        })
        .prompt("Search commit messages (switch to diff content in the filter menu: f, s); empty to clear")
        .allow_empty(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_query_walks_all_or_head() {
        let q = Filters::new(true).to_query(0, 100);
        assert!(q.all && q.revs.is_empty() && q.topo);
        assert_eq!(q.max, 100);
        let q = Filters::new(false).to_query(200, 50);
        assert!(!q.all);
        assert_eq!(q.skip, 200);
    }

    #[test]
    fn query_carries_every_filter() {
        let mut f = Filters::new(true);
        f.branch = BranchFilter::Rev("feature/x".into());
        f.user = Some(UserFilter {
            label: "me".into(),
            pattern: "<a@b.c>".into(),
        });
        f.since = Some("2 weeks ago".into());
        f.until = Some("yesterday".into());
        f.paths = vec!["src/a b.rs".into()];
        f.text = Some("fix(api)".into());
        f.first_parent = true;
        f.no_merges = true;
        let q = f.to_query(0, 10);
        assert!(!q.all);
        assert_eq!(q.revs, vec!["feature/x", "--fixed-strings", "--parents"]);
        assert_eq!(q.author.as_deref(), Some("<a@b.c>"));
        assert_eq!(q.grep.as_deref(), Some("fix(api)"));
        assert!(q.pickaxe.is_none());
        assert_eq!(q.paths, vec!["src/a b.rs"]);
        assert!(q.first_parent && q.no_merges);
        f.text_mode = TextMode::Diff;
        let q = f.to_query(0, 10);
        assert_eq!(q.pickaxe.as_deref(), Some("fix(api)"));
        assert!(q.grep.is_none());
    }

    #[test]
    fn describe_and_flags() {
        let mut f = Filters::new(true);
        assert_eq!(f.describe(), vec!["all branches"]);
        assert!(!f.is_filtered());
        f.toggle_all();
        assert_eq!(f.branch, BranchFilter::Head);
        f.branch = BranchFilter::Rev("0123456789abcdef0123456789abcdef01234567".into());
        f.paths = vec!["a".into(), "b".into()];
        f.text = Some("x".into());
        f.text_mode = TextMode::Diff;
        f.no_merges = true;
        assert_eq!(
            f.describe(),
            vec![
                "branch: 0123456789",
                "paths: a +1",
                "diff: \"x\"",
                "no merges"
            ]
        );
        assert!(f.is_filtered() && f.hides_commits());
        f.branch = BranchFilter::Rev("refs/remotes/origin/dev".into());
        assert_eq!(f.describe()[0], "branch: origin/dev");
        f.branch = BranchFilter::Rev("refs/tags/v1".into());
        assert_eq!(f.describe()[0], "branch: v1");
        f.toggle_all();
        assert_eq!(f.branch, BranchFilter::All);
        f.clear(false);
        assert_eq!(f, Filters::new(false));
    }

    #[test]
    fn path_filter_keeps_real_parents_for_actions() {
        use crate::git::testutil::*;
        let (_d, repo) = temp_repo();
        let one = commit_files(&repo, &[("p.txt", "1\n")], "one");
        let mid = commit_files(&repo, &[("other.txt", "x\n")], "unrelated");
        let three = commit_files(&repo, &[("p.txt", "3\n")], "three");
        let mut f = Filters::new(false);
        f.paths = vec!["p.txt".into()];
        let mut log = repo.log(&f.to_query(0, 10)).unwrap();
        assert_eq!(log[0].parents, vec![one.clone()], "git rewrote the parent");
        let graph = restore_real_parents(&repo, &mut log).unwrap();
        assert_eq!(log[0].id, three);
        assert_eq!(log[0].parents, vec![mid], "real parent restored");
        assert_eq!(
            graph[&three],
            vec![one.clone()],
            "rewritten parent kept for the graph"
        );
        assert!(log[1].parents.is_empty() && graph[&one].is_empty());
        assert!(restore_real_parents(&repo, &mut []).unwrap().is_empty());
    }

    #[test]
    fn clearing_hiding_filters() {
        let mut f = Filters::new(true);
        f.first_parent = true;
        assert!(!f.clear_hiding());
        assert!(!f.keeps_ancestry());
        f.first_parent = false;
        assert!(f.keeps_ancestry());
        f.text = Some("x".into());
        f.paths = vec!["a".into()];
        f.since = Some("1 day ago".into());
        f.branch = BranchFilter::Head;
        assert!(!f.keeps_ancestry());
        assert!(f.clear_hiding());
        assert_eq!(f, Filters::new(false));
    }

    #[test]
    fn path_parsing() {
        assert_eq!(
            parse_paths(" src\\ui ; ./my file.txt;; "),
            vec!["src/ui", "my file.txt"]
        );
        assert!(parse_paths("  ").is_empty());
    }

    #[test]
    fn author_ranking() {
        let mk = |a: &str| Commit {
            id: String::new(),
            short: String::new(),
            parents: vec![],
            author: a.into(),
            email: format!("{a}@x"),
            time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            subject: String::new(),
            refs: vec![],
        };
        let commits = [mk("bob"), mk("ann"), mk("bob"), mk("cy")];
        let a = authors(&commits, 2);
        assert_eq!(
            a,
            vec![
                ("bob".to_string(), "bob@x".to_string()),
                ("ann".into(), "ann@x".into())
            ]
        );
    }

    #[test]
    fn query_runs_against_real_repo() {
        use crate::git::testutil::*;
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a b/x.txt", "1\n")], "fix(api): one");
        commit_files(&repo, &[("other.txt", "2\n")], "two");
        commit_files(&repo, &[("a b/x.txt", "3\n")], "three");
        let mut f = Filters::new(false);
        f.text = Some("fix(api)".into());
        assert_eq!(repo.log(&f.to_query(0, 10)).unwrap().len(), 1);
        f.text = None;
        f.paths = vec!["a b/x.txt".into()];
        let log = repo.log(&f.to_query(0, 10)).unwrap();
        assert_eq!(log.len(), 2);
        // Parent rewriting: "three" now points straight at "one".
        assert_eq!(log[0].parents, vec![log[1].id.clone()]);
        f.paths.clear();
        f.user = Some(UserFilter {
            label: "me".into(),
            pattern: "<test@example.com>".into(),
        });
        assert_eq!(repo.log(&f.to_query(0, 10)).unwrap().len(), 3);
        f.user = Some(UserFilter {
            label: "x".into(),
            pattern: "nobody".into(),
        });
        assert!(repo.log(&f.to_query(0, 10)).unwrap().is_empty());
        let mut f = Filters::new(false);
        f.text = Some("three".into());
        f.text_mode = TextMode::Diff;
        assert!(repo.log(&f.to_query(0, 10)).unwrap().is_empty());
        f.text = Some("3".into());
        assert_eq!(repo.log(&f.to_query(0, 10)).unwrap().len(), 1);
    }
}
