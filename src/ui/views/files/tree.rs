//! Project file tree: building it from paths, flattening the expanded part into rows, and the
//! fuzzy filter that replaces the tree with a flat list.

use std::collections::{BTreeMap, HashSet};

use crate::git::status::FileState;

/// A file shown in the browser with its (non-clean) status, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    pub path: String,
    pub state: Option<FileState>,
}

/// A directory or file node. Directories list sub-directories first, then files, both sorted
/// case-insensitively.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// Repo-relative path (`""` for the root)
    pub path: String,
    pub is_dir: bool,
    pub state: Option<FileState>,
    /// Number of files below this node (1 for a file)
    pub files: usize,
    /// Number of changed files below this node
    pub changed: usize,
    pub children: Vec<Node>,
}

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub depth: usize,
    /// Display name (`a/b/c` for compacted directory chains, the full path in filtered mode)
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub expanded: bool,
    pub state: Option<FileState>,
    pub files: usize,
    pub changed: usize,
}

#[derive(Default)]
struct Builder {
    dirs: BTreeMap<String, Builder>,
    files: Vec<(String, Option<FileState>)>,
}

fn is_changed(s: Option<FileState>) -> bool {
    s.is_some_and(|s| s != FileState::Unmodified)
}

/// Build the tree for `items` (paths use `/` separators).
pub fn build(items: &[FileItem]) -> Node {
    let mut root = Builder::default();
    for it in items {
        let mut parts: Vec<&str> = it.path.split('/').filter(|p| !p.is_empty()).collect();
        let Some(file) = parts.pop() else {
            continue;
        };
        let mut b = &mut root;
        for dir in parts {
            b = b.dirs.entry(dir.to_string()).or_default();
        }
        b.files.push((file.to_string(), it.state));
    }
    finish(root, String::new(), String::new())
}

fn finish(b: Builder, name: String, path: String) -> Node {
    let join = |n: &str| {
        if path.is_empty() {
            n.to_string()
        } else {
            format!("{path}/{n}")
        }
    };
    let mut dirs: Vec<Node> = b
        .dirs
        .into_iter()
        .map(|(n, sub)| {
            let p = join(&n);
            finish(sub, n, p)
        })
        .collect();
    dirs.sort_by_key(|d| d.name.to_lowercase());
    let mut files: Vec<Node> = b
        .files
        .into_iter()
        .map(|(n, state)| Node {
            path: join(&n),
            name: n,
            is_dir: false,
            state,
            files: 1,
            changed: usize::from(is_changed(state)),
            children: Vec::new(),
        })
        .collect();
    files.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.name.cmp(&b.name))
    });
    files.dedup_by(|a, b| a.path == b.path);
    let children: Vec<Node> = dirs.into_iter().chain(files).collect();
    Node {
        name,
        path,
        is_dir: true,
        state: None,
        files: children.iter().map(|c| c.files).sum(),
        changed: children.iter().map(|c| c.changed).sum(),
        children,
    }
}

/// Follow a chain of directories that each contain exactly one sub-directory and nothing else
/// (compact middle directories). Returns the display name and the deepest node.
fn compact(node: &Node) -> (String, &Node) {
    let mut name = node.name.clone();
    let mut cur = node;
    while cur.children.len() == 1 && cur.children[0].is_dir {
        cur = &cur.children[0];
        name = format!("{name}/{}", cur.name);
    }
    (name, cur)
}

/// The visible rows of the tree: children of expanded directories (keyed by path) are included.
pub fn flatten(root: &Node, expanded: &HashSet<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    for c in &root.children {
        push_rows(c, 0, expanded, &mut rows);
    }
    rows
}

fn push_rows(node: &Node, depth: usize, expanded: &HashSet<String>, rows: &mut Vec<Row>) {
    let (name, deepest) = if node.is_dir {
        compact(node)
    } else {
        (node.name.clone(), node)
    };
    let open = deepest.is_dir && expanded.contains(&deepest.path);
    rows.push(Row {
        depth,
        name,
        path: deepest.path.clone(),
        is_dir: deepest.is_dir,
        expanded: open,
        state: deepest.state,
        files: deepest.files,
        changed: deepest.changed,
    });
    if open {
        for c in &deepest.children {
            push_rows(c, depth + 1, expanded, rows);
        }
    }
}

/// Paths of all directories in the tree (for "expand all").
pub fn all_dirs(root: &Node) -> Vec<String> {
    let mut v = Vec::new();
    let mut stack: Vec<&Node> = root.children.iter().collect();
    while let Some(n) = stack.pop() {
        if n.is_dir {
            v.push(n.path.clone());
            stack.extend(n.children.iter());
        }
    }
    v
}

/// Every ancestor directory of `path` (`a/b/c.txt` → `a`, `a/b`).
pub fn ancestors(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    (1..parts.len()).map(|i| parts[..i].join("/")).collect()
}

/// Parent directory of `path` (`""` at top level).
pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

fn is_boundary(prev: Option<char>) -> bool {
    matches!(prev, None | Some('/' | '_' | '-' | '.' | ' '))
}

/// Score `text` against `query` as an ordered subsequence (case-insensitive); higher is better.
fn subsequence_score(text: &str, query: &[char]) -> Option<i64> {
    let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c != query[qi] {
            continue;
        }
        score += 10;
        if last.is_some_and(|l| l + 1 == i) {
            score += 15;
        }
        if is_boundary(i.checked_sub(1).map(|p| chars[p])) {
            score += 20;
        }
        last = Some(i);
        qi += 1;
    }
    (qi == query.len()).then_some(score)
}

/// Fuzzy score of `path` for `query`; matches inside the file name rank above matches spread
/// over directories, and shorter paths win ties. `None` when the query does not match.
pub fn fuzzy_score(path: &str, query: &str) -> Option<i64> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let penalty = path.chars().count() as i64 / 4;
    if let Some(s) = subsequence_score(name, &q) {
        let exact_prefix = name
            .to_lowercase()
            .starts_with(&q.iter().collect::<String>());
        return Some(s + 100 + if exact_prefix { 50 } else { 0 } - penalty);
    }
    subsequence_score(path, &q).map(|s| s - penalty)
}

/// Flat list of files matching `query`, best first, at most `limit` rows.
pub fn filter(items: &[FileItem], query: &str, limit: usize) -> Vec<Row> {
    let mut hits: Vec<(i64, &FileItem)> = items
        .iter()
        .filter_map(|it| fuzzy_score(&it.path, query).map(|s| (s, it)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    hits.into_iter()
        .take(limit)
        .map(|(_, it)| Row {
            depth: 0,
            name: it.path.clone(),
            path: it.path.clone(),
            is_dir: false,
            expanded: false,
            state: it.state,
            files: 1,
            changed: usize::from(is_changed(it.state)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(paths: &[&str]) -> Vec<FileItem> {
        paths
            .iter()
            .map(|p| FileItem {
                path: p.to_string(),
                state: if p.ends_with(".rs") {
                    Some(FileState::Modified)
                } else {
                    None
                },
            })
            .collect()
    }

    #[test]
    fn builds_sorted_tree_with_counts() {
        let root = build(&items(&[
            "b.txt",
            "src/main.rs",
            "A.md",
            "src/ui/app.rs",
            "src/lib.txt",
            "docs/x",
        ]));
        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["docs", "src", "A.md", "b.txt"]);
        assert_eq!(root.files, 6);
        assert_eq!(root.changed, 2);
        let src = &root.children[1];
        assert_eq!(src.path, "src");
        assert_eq!(src.children[0].name, "ui");
        assert_eq!(src.children[0].children[0].path, "src/ui/app.rs");
        assert_eq!(src.changed, 2);
    }

    #[test]
    fn flatten_respects_expansion_and_compaction() {
        let root = build(&items(&[
            "a/b/c/deep.txt",
            "a/b/c/other.txt",
            "top.txt",
            "x/y.txt",
            "x/z/w.txt",
        ]));
        let mut exp = HashSet::new();
        let rows = flatten(&root, &exp);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "a/b/c");
        assert_eq!(rows[0].path, "a/b/c");
        assert!(!rows[0].expanded);
        exp.insert("a/b/c".to_string());
        exp.insert("x".to_string());
        let rows = flatten(&root, &exp);
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "a/b/c",
                "a/b/c/deep.txt",
                "a/b/c/other.txt",
                "x",
                "x/z",
                "x/y.txt",
                "top.txt"
            ]
        );
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[4].name, "z");
        assert!(all_dirs(&root).contains(&"x/z".to_string()));
    }

    #[test]
    fn path_helpers() {
        assert_eq!(
            ancestors("a/b/c.txt"),
            vec!["a".to_string(), "a/b".to_string()]
        );
        assert!(ancestors("c.txt").is_empty());
        assert_eq!(parent_of("a/b/c.txt"), "a/b");
        assert_eq!(parent_of("c.txt"), "");
    }

    #[test]
    fn fuzzy_prefers_file_names() {
        assert!(fuzzy_score("src/main.rs", "mn").is_some());
        assert!(fuzzy_score("src/main.rs", "xyz").is_none());
        let name = fuzzy_score("deep/dir/blame.rs", "blame").unwrap();
        let spread = fuzzy_score("b/l/a/m/e.rs", "blame").unwrap();
        assert!(name > spread);
        let rows = filter(
            &items(&["src/ui/views/files.rs", "files.txt", "src/fi/le/s.rs"]),
            "files",
            10,
        );
        assert_eq!(rows[0].path, "files.txt");
        assert_eq!(rows.len(), 3);
        assert_eq!(filter(&items(&["a", "b"]), "", 1).len(), 1);
        assert!(fuzzy_score("Ünïcode/Datei.TXT", "datei").is_some());
    }
}
