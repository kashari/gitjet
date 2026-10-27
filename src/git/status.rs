//! Working tree status (`git status --porcelain=v2 -z`).

use anyhow::Result;

use super::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Ignored,
    Conflicted,
}

impl FileState {
    fn from_code(c: char) -> FileState {
        match c {
            'M' => FileState::Modified,
            'A' => FileState::Added,
            'D' => FileState::Deleted,
            'R' => FileState::Renamed,
            'C' => FileState::Copied,
            'T' => FileState::TypeChanged,
            'U' => FileState::Conflicted,
            _ => FileState::Unmodified,
        }
    }

    /// One-letter code as shown in file lists.
    pub fn letter(self) -> char {
        match self {
            FileState::Unmodified => ' ',
            FileState::Modified => 'M',
            FileState::Added => 'A',
            FileState::Deleted => 'D',
            FileState::Renamed => 'R',
            FileState::Copied => 'C',
            FileState::TypeChanged => 'T',
            FileState::Untracked => '?',
            FileState::Ignored => '!',
            FileState::Conflicted => 'U',
        }
    }
}

/// Which side added/deleted in a conflict (`DD`, `AU`, `UD`, `UA`, `DU`, `AA`, `UU`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    BothDeleted,
    AddedByUs,
    DeletedByThem,
    AddedByThem,
    DeletedByUs,
    BothAdded,
    BothModified,
}

impl ConflictKind {
    fn from_xy(xy: &str) -> Option<ConflictKind> {
        Some(match xy {
            "DD" => ConflictKind::BothDeleted,
            "AU" => ConflictKind::AddedByUs,
            "UD" => ConflictKind::DeletedByThem,
            "UA" => ConflictKind::AddedByThem,
            "DU" => ConflictKind::DeletedByUs,
            "AA" => ConflictKind::BothAdded,
            "UU" => ConflictKind::BothModified,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ConflictKind::BothDeleted => "both deleted",
            ConflictKind::AddedByUs => "added by us",
            ConflictKind::DeletedByThem => "deleted by them",
            ConflictKind::AddedByThem => "added by them",
            ConflictKind::DeletedByUs => "deleted by us",
            ConflictKind::BothAdded => "both added",
            ConflictKind::BothModified => "both modified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Repo-relative path with `/` separators
    pub path: String,
    /// Source path of a rename/copy
    pub orig_path: Option<String>,
    /// State in the index (X)
    pub index: FileState,
    /// State in the work tree (Y)
    pub worktree: FileState,
    pub conflict: Option<ConflictKind>,
    /// Entry is a submodule
    pub submodule: bool,
}

impl StatusEntry {
    pub fn is_untracked(&self) -> bool {
        self.index == FileState::Untracked
    }
    pub fn is_ignored(&self) -> bool {
        self.index == FileState::Ignored
    }
    pub fn is_conflicted(&self) -> bool {
        self.conflict.is_some()
    }
    /// Has changes in the index.
    pub fn is_staged(&self) -> bool {
        !self.is_conflicted()
            && !matches!(
                self.index,
                FileState::Unmodified | FileState::Untracked | FileState::Ignored
            )
    }
    /// Has changes in the work tree not yet staged (untracked files count).
    pub fn is_unstaged(&self) -> bool {
        self.is_untracked() || (!self.is_conflicted() && self.worktree != FileState::Unmodified)
    }
    /// The most relevant state for display: conflict > work tree > index.
    pub fn display_state(&self) -> FileState {
        if self.is_conflicted() {
            FileState::Conflicted
        } else if self.worktree != FileState::Unmodified {
            self.worktree
        } else {
            self.index
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Current branch, None when detached
    pub branch: Option<String>,
    /// HEAD commit, None in an empty repository
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<StatusEntry>,
}

impl Status {
    pub fn conflicted(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_conflicted())
    }
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| e.is_ignored())
    }
}

impl Repo {
    /// Status including untracked files (individually) but not ignored ones.
    pub fn status(&self) -> Result<Status> {
        let out = self.run(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])?;
        Ok(parse_porcelain_v2(&out))
    }

    /// Ignored files (for "show ignored" toggles).
    pub fn ignored_files(&self) -> Result<Vec<String>> {
        let out = self.run(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--ignored=matching",
            "--untracked-files=all",
        ])?;
        Ok(parse_porcelain_v2(&out)
            .entries
            .into_iter()
            .filter(|e| e.is_ignored())
            .map(|e| e.path)
            .collect())
    }
}

pub fn parse_porcelain_v2(out: &str) -> Status {
    let mut st = Status::default();
    let mut fields = out.split('\0');
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" if v != "(initial)" => st.head = Some(v.to_string()),
                "branch.head" if v != "(detached)" => st.branch = Some(v.to_string()),
                "branch.upstream" => st.upstream = Some(v.to_string()),
                "branch.ab" => {
                    for p in v.split(' ') {
                        if let Some(a) = p.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = p.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' | b'2' | b'u' => {
                // 1 XY sub mH mI mW hH hI path
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let n = match kind {
                    b'1' => 8,
                    b'2' => 9,
                    _ => 10,
                };
                let parts: Vec<&str> = rec.splitn(n + 1, ' ').collect();
                if parts.len() < n + 1 {
                    continue;
                }
                let xy = parts[1];
                let mut chars = xy.chars();
                let x = chars.next().unwrap_or('.');
                let y = chars.next().unwrap_or('.');
                let path = parts[n].to_string();
                let orig_path = if kind == b'2' {
                    fields.next().map(str::to_string)
                } else {
                    None
                };
                let conflict = if kind == b'u' {
                    ConflictKind::from_xy(xy).or(Some(ConflictKind::BothModified))
                } else {
                    None
                };
                st.entries.push(StatusEntry {
                    path,
                    orig_path,
                    index: FileState::from_code(x),
                    worktree: FileState::from_code(y),
                    conflict,
                    submodule: parts[2].starts_with('S'),
                });
            }
            b'?' | b'!' => {
                let state = if kind == b'?' {
                    FileState::Untracked
                } else {
                    FileState::Ignored
                };
                st.entries.push(StatusEntry {
                    path: rec[2..].to_string(),
                    orig_path: None,
                    index: state,
                    worktree: state,
                    conflict: None,
                    submodule: false,
                });
            }
            _ => {}
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    #[test]
    fn parses_all_record_kinds() {
        let raw = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 h1 h2 src/a b.rs\0\
2 R. N... 100644 100644 100644 h1 h2 R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 h1 h2 h3 conf.txt\0\
? untracked.txt\0! ignored.log\0";
        let st = parse_porcelain_v2(raw);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.entries.len(), 5);
        assert_eq!(st.entries[0].path, "src/a b.rs");
        assert!(st.entries[0].is_unstaged() && !st.entries[0].is_staged());
        assert_eq!(st.entries[1].orig_path.as_deref(), Some("old.rs"));
        assert!(st.entries[1].is_staged());
        assert_eq!(st.entries[2].conflict, Some(ConflictKind::BothModified));
        assert!(st.entries[3].is_untracked());
        assert!(st.entries[4].is_ignored());
    }

    #[test]
    fn real_repo_status() {
        let (_d, repo) = temp_repo();
        let st = repo.status().unwrap();
        assert!(st.head.is_none());
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "a.txt", "2\n");
        write(&repo, "dir/new.txt", "n\n");
        let st = repo.status().unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.entries.len(), 2);
        assert!(st
            .entries
            .iter()
            .any(|e| e.path == "dir/new.txt" && e.is_untracked()));
    }
}
