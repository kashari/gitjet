//! Pure tree layout of the Branches tab: sections, remote groups and branch rows.

use std::collections::HashSet;

use crate::git::branch::{Branch, Favorites};

/// Top-level groups, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    Favorites,
    Local,
    Remote,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Recent,
        Section::Favorites,
        Section::Local,
        Section::Remote,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Recent => "Recent",
            Section::Favorites => "Favorites",
            Section::Local => "Local",
            Section::Remote => "Remote",
        }
    }

    /// Key used for the collapsed-state set.
    pub fn key(self) -> String {
        format!("s:{}", self.label())
    }
}

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section {
        section: Section,
        count: usize,
        collapsed: bool,
    },
    /// A remote inside the Remote section
    Group {
        remote: String,
        count: usize,
        collapsed: bool,
    },
    /// Index into the branch list; `nested` when shown under a remote group
    Branch {
        idx: usize,
        section: Section,
        nested: bool,
    },
}

impl Row {
    /// Stable identity used to keep the selection across refreshes.
    pub fn id(&self, branches: &[Branch]) -> String {
        match self {
            Row::Section { section, .. } => section.key(),
            Row::Group { remote, .. } => group_key(remote),
            Row::Branch { idx, section, .. } => {
                format!(
                    "b:{}:{}",
                    section.label(),
                    branches
                        .get(*idx)
                        .map(|b| b.full_ref.as_str())
                        .unwrap_or("")
                )
            }
        }
    }

    /// Key in the collapsed set for section and group rows.
    pub fn collapse_key(&self) -> Option<String> {
        match self {
            Row::Section { section, .. } => Some(section.key()),
            Row::Group { remote, .. } => Some(group_key(remote)),
            Row::Branch { .. } => None,
        }
    }
}

pub fn group_key(remote: &str) -> String {
    format!("r:{remote}")
}

/// Everything the tree is built from.
pub struct TreeInput<'a> {
    pub branches: &'a [Branch],
    /// Recently checked-out branch names, newest first
    pub recent: &'a [String],
    pub favorites: &'a Favorites,
    pub collapsed: &'a HashSet<String>,
    /// Case-insensitive name filter; every whitespace-separated term must match
    pub filter: &'a str,
}

/// True if `name` contains every term of `filter` (case-insensitive).
pub fn matches(name: &str, filter: &str) -> bool {
    let n = name.to_lowercase();
    filter
        .split_whitespace()
        .all(|t| n.contains(&t.to_lowercase()))
}

/// Build the visible rows. Empty sections are hidden; while filtering everything is expanded.
pub fn build_rows(input: &TreeInput) -> Vec<Row> {
    let filtering = !input.filter.trim().is_empty();
    let ok = |b: &Branch| !filtering || matches(&b.name, input.filter);
    let is_collapsed = |key: &str| !filtering && input.collapsed.contains(key);
    let mut rows = Vec::new();
    for section in Section::ALL {
        let members = section_members(section, input, &ok);
        if members.is_empty() {
            continue;
        }
        let collapsed = is_collapsed(&section.key());
        rows.push(Row::Section {
            section,
            count: members.len(),
            collapsed,
        });
        if collapsed {
            continue;
        }
        if section == Section::Remote {
            push_remote_groups(&mut rows, input.branches, &members, &is_collapsed);
        } else {
            rows.extend(members.into_iter().map(|idx| Row::Branch {
                idx,
                section,
                nested: false,
            }));
        }
    }
    rows
}

fn section_members(
    section: Section,
    input: &TreeInput,
    ok: &dyn Fn(&Branch) -> bool,
) -> Vec<usize> {
    let b = input.branches;
    match section {
        Section::Recent => input
            .recent
            .iter()
            .filter_map(|name| b.iter().position(|x| !x.is_remote() && &x.name == name))
            .filter(|&i| !b[i].is_head && ok(&b[i]))
            .take(5)
            .collect(),
        Section::Favorites => (0..b.len())
            .filter(|&i| input.favorites.contains(&b[i].full_ref) && ok(&b[i]))
            .collect(),
        Section::Local => (0..b.len())
            .filter(|&i| !b[i].is_remote() && ok(&b[i]))
            .collect(),
        Section::Remote => (0..b.len())
            .filter(|&i| b[i].is_remote() && ok(&b[i]))
            .collect(),
    }
}

fn push_remote_groups(
    rows: &mut Vec<Row>,
    branches: &[Branch],
    members: &[usize],
    is_collapsed: &dyn Fn(&str) -> bool,
) {
    let mut remotes: Vec<&str> = Vec::new();
    for &i in members {
        if let Some(r) = branches[i].remote.as_deref() {
            if !remotes.contains(&r) {
                remotes.push(r);
            }
        }
    }
    for remote in remotes {
        let in_remote: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| branches[i].remote.as_deref() == Some(remote))
            .collect();
        let collapsed = is_collapsed(&group_key(remote));
        rows.push(Row::Group {
            remote: remote.to_string(),
            count: in_remote.len(),
            collapsed,
        });
        if !collapsed {
            rows.extend(in_remote.into_iter().map(|idx| Row::Branch {
                idx,
                section: Section::Remote,
                nested: true,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn br(name: &str, remote: Option<&str>, head: bool) -> Branch {
        let full_ref = match remote {
            Some(_) => format!("refs/remotes/{name}"),
            None => format!("refs/heads/{name}"),
        };
        Branch {
            name: name.into(),
            full_ref,
            remote: remote.map(str::to_string),
            is_head: head,
            upstream: None,
            upstream_gone: false,
            ahead: 0,
            behind: 0,
            short: "abc".into(),
            time: 0,
            subject: String::new(),
        }
    }

    fn sample() -> Vec<Branch> {
        vec![
            br("dev", None, false),
            br("feature/login", None, true),
            br("main", None, false),
            br("origin/dev", Some("origin"), false),
            br("origin/main", Some("origin"), false),
            br("up/main", Some("up"), false),
        ]
    }

    fn labels(rows: &[Row], b: &[Branch]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Section {
                    section,
                    count,
                    collapsed,
                } => {
                    format!(
                        "{}{}({count})",
                        if *collapsed { "+" } else { "-" },
                        section.label()
                    )
                }
                Row::Group { remote, count, .. } => format!("  {remote}({count})"),
                Row::Branch { idx, nested, .. } => {
                    format!("{}{}", if *nested { "    " } else { "  " }, b[*idx].name)
                }
            })
            .collect()
    }

    #[test]
    fn full_tree() {
        let b = sample();
        let recent = vec![
            "feature/login".to_string(),
            "dev".to_string(),
            "gone".to_string(),
            "main".to_string(),
        ];
        let fav = Favorites::default();
        let collapsed = HashSet::new();
        let rows = build_rows(&TreeInput {
            branches: &b,
            recent: &recent,
            favorites: &fav,
            collapsed: &collapsed,
            filter: "",
        });
        assert_eq!(
            labels(&rows, &b),
            vec![
                "-Recent(2)",
                "  dev",
                "  main",
                "-Favorites(1)",
                "  main",
                "-Local(3)",
                "  dev",
                "  feature/login",
                "  main",
                "-Remote(3)",
                "  origin(2)",
                "    origin/dev",
                "    origin/main",
                "  up(1)",
                "    up/main",
            ]
        );
        assert_eq!(rows[1].id(&b), "b:Recent:refs/heads/dev");
        assert_eq!(rows[10].id(&b), "r:origin");
    }

    #[test]
    fn collapse_and_filter() {
        let b = sample();
        let fav = Favorites {
            refs: Default::default(),
        };
        let mut collapsed = HashSet::new();
        collapsed.insert(Section::Local.key());
        collapsed.insert(group_key("origin"));
        let input = TreeInput {
            branches: &b,
            recent: &[],
            favorites: &fav,
            collapsed: &collapsed,
            filter: "",
        };
        assert_eq!(
            labels(&build_rows(&input), &b),
            vec![
                "+Local(3)",
                "-Remote(3)",
                "  origin(2)",
                "  up(1)",
                "    up/main"
            ]
        );
        // Filtering ignores collapsed state and hides empty groups.
        let input = TreeInput {
            filter: "MAIN",
            ..input
        };
        assert_eq!(
            labels(&build_rows(&input), &b),
            vec![
                "-Local(1)",
                "  main",
                "-Remote(2)",
                "  origin(1)",
                "    origin/main",
                "  up(1)",
                "    up/main"
            ]
        );
        let input = TreeInput {
            filter: "or ma",
            ..input
        };
        assert_eq!(
            labels(&build_rows(&input), &b),
            vec!["-Remote(1)", "  origin(1)", "    origin/main"]
        );
        assert!(matches("feature/Login", "feat log"));
        assert!(!matches("feature/login", "main"));
    }

    #[test]
    fn empty_repo_has_no_rows() {
        let fav = Favorites::default();
        let collapsed = HashSet::new();
        let rows = build_rows(&TreeInput {
            branches: &[],
            recent: &[],
            favorites: &fav,
            collapsed: &collapsed,
            filter: "",
        });
        assert!(rows.is_empty());
    }
}
