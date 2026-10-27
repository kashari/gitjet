//! Row model of the changes list: groups (changelists, conflicts, unversioned), optional directory
//! tree, file rows. Pure logic so it can be tested without a terminal.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::git::changelist::Changelists;
use crate::git::status::StatusEntry;

/// Which top-level node a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Merge conflicts (shown first)
    Conflicts,
    /// Changelist by index
    List(usize),
    /// Untracked files (shown last)
    Unversioned,
}

impl Group {
    /// Stable key for collapse state.
    pub fn key(self, lists: &Changelists) -> String {
        match self {
            Group::Conflicts => "g:conflicts".into(),
            Group::Unversioned => "g:unversioned".into(),
            Group::List(i) => format!(
                "g:cl:{}",
                lists.lists.get(i).map(|l| l.name.as_str()).unwrap_or("")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Group {
        name: String,
        description: String,
        active: bool,
    },
    Dir {
        /// Full directory path (repo-relative, no trailing slash)
        path: String,
        /// Display label (compressed chain like `src/ui`)
        label: String,
    },
    File {
        /// Index into the status entries
        entry: usize,
        /// Display name (file name in tree mode, full path in flat mode)
        name: String,
        /// Parent directory shown dimmed in flat mode
        dir: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub group: Group,
    pub depth: usize,
    /// Entry indices of every file in (or under) this row
    pub files: Vec<usize>,
    pub collapsed: bool,
}

impl Row {
    /// Stable identity used to keep the selection across refreshes.
    pub fn key(&self, lists: &Changelists, entries: &[StatusEntry]) -> String {
        let g = self.group.key(lists);
        match &self.kind {
            RowKind::Group { .. } => g,
            RowKind::Dir { path, .. } => format!("d:{g}:{path}"),
            RowKind::File { entry, .. } => format!(
                "f:{}",
                entries.get(*entry).map(|e| e.path.as_str()).unwrap_or("")
            ),
        }
    }

    pub fn entry(&self) -> Option<usize> {
        match self.kind {
            RowKind::File { entry, .. } => Some(entry),
            _ => None,
        }
    }
}

/// Which group an entry is shown in, given the changelist assignments and the active list.
pub fn group_of(e: &StatusEntry, assigned: &HashMap<&str, usize>, active: usize) -> Group {
    if e.is_conflicted() {
        Group::Conflicts
    } else if e.is_untracked() {
        Group::Unversioned
    } else {
        Group::List(assigned.get(e.path.as_str()).copied().unwrap_or(active))
    }
}

/// Options for [`build_rows`].
pub struct BuildOpts<'a> {
    pub tree: bool,
    pub collapsed: &'a HashSet<String>,
    /// Case-insensitive substring filter on paths (empty = all)
    pub filter: &'a str,
}

/// Build the visible rows. Changelists are always shown (even empty, unless filtering);
/// conflicts and unversioned groups only when they have files.
pub fn build_rows(entries: &[StatusEntry], lists: &Changelists, opts: &BuildOpts) -> Vec<Row> {
    let q = opts.filter.to_lowercase();
    let mut groups: Vec<(Group, Vec<usize>)> = Vec::new();
    groups.push((Group::Conflicts, Vec::new()));
    for i in 0..lists.lists.len() {
        groups.push((Group::List(i), Vec::new()));
    }
    groups.push((Group::Unversioned, Vec::new()));
    let assigned = lists.assignments();
    let active = lists.active();
    for (i, e) in entries.iter().enumerate() {
        if e.is_ignored() || (!q.is_empty() && !e.path.to_lowercase().contains(&q)) {
            continue;
        }
        let slot = match group_of(e, &assigned, active) {
            Group::Conflicts => 0,
            Group::List(l) => (l + 1).min(groups.len() - 2),
            Group::Unversioned => groups.len() - 1,
        };
        groups[slot].1.push(i);
    }
    let mut rows = Vec::new();
    for (g, mut files) in groups {
        let is_list = matches!(g, Group::List(_));
        if files.is_empty() && (!is_list || !q.is_empty()) {
            continue;
        }
        files.sort_by(|a, b| entries[*a].path.cmp(&entries[*b].path));
        let (name, description, active) = match g {
            Group::Conflicts => ("Merge Conflicts".to_string(), String::new(), false),
            Group::Unversioned => ("Unversioned Files".to_string(), String::new(), false),
            Group::List(i) => {
                let l = &lists.lists[i];
                (l.name.clone(), l.description.clone(), l.active)
            }
        };
        let key = g.key(lists);
        let collapsed = opts.collapsed.contains(&key);
        rows.push(Row {
            kind: RowKind::Group {
                name,
                description,
                active,
            },
            group: g,
            depth: 0,
            files: files.clone(),
            collapsed,
        });
        if collapsed {
            continue;
        }
        if opts.tree {
            let mut root = Node::default();
            for &i in &files {
                root.insert(&entries[i].path, i);
            }
            emit_tree(&root, "", g, &key, 1, opts.collapsed, &mut rows);
        } else {
            for &i in &files {
                let (dir, name) = split_path(&entries[i].path);
                rows.push(Row {
                    kind: RowKind::File {
                        entry: i,
                        name: name.to_string(),
                        dir: dir.to_string(),
                    },
                    group: g,
                    depth: 1,
                    files: vec![i],
                    collapsed: false,
                });
            }
        }
    }
    rows
}

/// `("a/b", "c.txt")` for `a/b/c.txt`, `("", "c.txt")` for a root file.
pub fn split_path(p: &str) -> (&str, &str) {
    match p.rsplit_once('/') {
        Some((d, n)) => (d, n),
        None => ("", p),
    }
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
}

impl Node {
    fn insert(&mut self, path: &str, entry: usize) {
        match path.split_once('/') {
            Some((d, rest)) if !d.is_empty() && !rest.is_empty() => self
                .dirs
                .entry(d.to_string())
                .or_default()
                .insert(rest, entry),
            _ => self.files.push((path.to_string(), entry)),
        }
    }

    fn all_files(&self, out: &mut Vec<usize>) {
        for d in self.dirs.values() {
            d.all_files(out);
        }
        out.extend(self.files.iter().map(|(_, e)| *e));
    }
}

fn emit_tree(
    node: &Node,
    prefix: &str,
    g: Group,
    gkey: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    for (name, child) in &node.dirs {
        // Compress chains of single-directory nodes: `src/ui/views`.
        let mut label = name.clone();
        let mut cur = child;
        while cur.files.is_empty() && cur.dirs.len() == 1 {
            let (n, c) = cur.dirs.iter().next().expect("one dir");
            label = format!("{label}/{n}");
            cur = c;
        }
        let path = if prefix.is_empty() {
            label.clone()
        } else {
            format!("{prefix}/{label}")
        };
        let mut files = Vec::new();
        cur.all_files(&mut files);
        let is_collapsed = collapsed.contains(&format!("d:{gkey}:{path}"));
        rows.push(Row {
            kind: RowKind::Dir {
                path: path.clone(),
                label,
            },
            group: g,
            depth,
            files,
            collapsed: is_collapsed,
        });
        if !is_collapsed {
            emit_tree(cur, &path, g, gkey, depth + 1, collapsed, rows);
        }
    }
    for (name, entry) in &node.files {
        rows.push(Row {
            kind: RowKind::File {
                entry: *entry,
                name: name.clone(),
                dir: prefix.to_string(),
            },
            group: g,
            depth,
            files: vec![*entry],
            collapsed: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::status::{parse_porcelain_v2, Status};

    fn status() -> Status {
        parse_porcelain_v2(
            "1 .M N... 100644 100644 100644 h h src/ui/views/a.rs\0\
1 M. N... 100644 100644 100644 h h src/ui/views/b.rs\0\
1 .M N... 100644 100644 100644 h h README.md\0\
u UU N... 100644 100644 100644 100644 h h h conf.txt\0\
? new dir/x.txt\0",
        )
    }

    fn opts<'a>(tree: bool, collapsed: &'a HashSet<String>, filter: &'a str) -> BuildOpts<'a> {
        BuildOpts {
            tree,
            collapsed,
            filter,
        }
    }

    #[test]
    fn flat_groups() {
        let st = status();
        let mut lists = Changelists::default();
        let other = lists.create("Other", "later").unwrap();
        lists.move_files(&["README.md".to_string()], other);
        let none = HashSet::new();
        let rows = build_rows(&st.entries, &lists, &opts(false, &none, ""));
        let names: Vec<String> = rows
            .iter()
            .map(|r| match &r.kind {
                RowKind::Group { name, .. } => format!("#{name}"),
                RowKind::File { name, dir, .. } => format!("{dir}|{name}"),
                RowKind::Dir { label, .. } => format!("D{label}"),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "#Merge Conflicts",
                "|conf.txt",
                "#Changes",
                "src/ui/views|a.rs",
                "src/ui/views|b.rs",
                "#Other",
                "|README.md",
                "#Unversioned Files",
                "new dir|x.txt",
            ]
        );
        assert_eq!(rows[2].files.len(), 2);
        assert!(matches!(rows[2].kind, RowKind::Group { active: true, .. }));
    }

    #[test]
    fn tree_compresses_and_collapses() {
        let st = status();
        let lists = Changelists::default();
        let none = HashSet::new();
        let rows = build_rows(&st.entries, &lists, &opts(true, &none, ""));
        let dir = rows
            .iter()
            .find(|r| matches!(&r.kind, RowKind::Dir { label, .. } if label == "src/ui/views"))
            .unwrap();
        assert_eq!(dir.files.len(), 2);
        assert_eq!(dir.depth, 1);
        let key = dir.key(&lists, &st.entries);
        let collapsed: HashSet<String> = [key].into_iter().collect();
        let rows2 = build_rows(&st.entries, &lists, &opts(true, &collapsed, ""));
        assert_eq!(rows2.len(), rows.len() - 2);
        let gkey = Group::List(0).key(&lists);
        let collapsed: HashSet<String> = [gkey].into_iter().collect();
        let rows3 = build_rows(&st.entries, &lists, &opts(true, &collapsed, ""));
        let g = rows3.iter().find(|r| r.group == Group::List(0)).unwrap();
        assert!(g.collapsed);
        assert_eq!(g.files.len(), 3);
        assert_eq!(
            rows3.iter().filter(|r| r.group == Group::List(0)).count(),
            1
        );
    }

    #[test]
    fn filter_hides_empty_groups() {
        let st = status();
        let mut lists = Changelists::default();
        lists.create("Empty", "").unwrap();
        let none = HashSet::new();
        let all = build_rows(&st.entries, &lists, &opts(false, &none, ""));
        assert!(all
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Group { name, .. } if name == "Empty")));
        let rows = build_rows(&st.entries, &lists, &opts(false, &none, "READ"));
        assert_eq!(rows.len(), 2);
        assert_eq!(split_path("a/b/c"), ("a/b", "c"));
        assert_eq!(split_path("c"), ("", "c"));
    }
}
