//! Changelists: named groups of changed files, stored in the gitjet data dir.
//!
//! The first list is the default one (initially "Changes"): it can be renamed but not deleted, and it
//! receives the files of deleted lists. Exactly one list is *active*; newly changed files are assigned
//! to it by [`Changelists::sync`], which also forgets files that are no longer changed.
//!
//! Stored as `changelists.json` (`changelists.<worktree>.json` for linked worktrees, since the data
//! dir is shared by all worktrees of a repository).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::Repo;

/// Name of the default changelist in a fresh repository.
pub const DEFAULT_NAME: &str = "Changes";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changelist {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub active: bool,
    /// Repo-relative paths assigned to this list
    #[serde(default)]
    pub files: Vec<String>,
}

impl Changelist {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: String::new(),
            active: false,
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changelists {
    pub lists: Vec<Changelist>,
}

impl Default for Changelists {
    fn default() -> Self {
        let mut first = Changelist::new(DEFAULT_NAME);
        first.active = true;
        Self { lists: vec![first] }
    }
}

/// Where this work tree's changelists are stored.
pub fn store_path(repo: &Repo) -> Result<PathBuf> {
    let dir = repo.data_dir()?;
    let name = if repo.git_dir == repo.common_dir {
        "changelists.json".to_string()
    } else {
        let wt = repo
            .git_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worktree".into());
        format!("changelists.{wt}.json")
    };
    Ok(dir.join(name))
}

/// Rename a damaged store file to a free `<name>.bak` / `<name>.N.bak` path and return it.
fn move_aside(path: &std::path::Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut backup = path.with_file_name(format!("{name}.bak"));
    let mut n = 2;
    while backup.exists() {
        backup = path.with_file_name(format!("{name}.{n}.bak"));
        n += 1;
    }
    std::fs::rename(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
    Ok(backup)
}

impl Changelists {
    /// Load from disk; a missing file yields the default single list. See [`Changelists::load_checked`].
    pub fn load(repo: &Repo) -> Result<Self> {
        Ok(Self::load_checked(repo)?.0)
    }

    /// Load from disk. A missing file yields the default single list. A damaged file (invalid
    /// JSON, e.g. after a hand edit) is moved aside to a `.bak` file before falling back to the
    /// default, so the next save cannot destroy it; its path is returned so the caller can tell
    /// the user. Other read errors fail, leaving the file untouched.
    pub fn load_checked(repo: &Repo) -> Result<(Self, Option<PathBuf>)> {
        let path = store_path(repo)?;
        let bytes = match std::fs::read(&path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let (mut cl, backup) = match bytes {
            None => (Changelists::default(), None),
            Some(b) => match serde_json::from_slice::<Changelists>(&b) {
                Ok(cl) => (cl, None),
                Err(_) => (Changelists::default(), Some(move_aside(&path)?)),
            },
        };
        cl.normalize();
        Ok((cl, backup))
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, repo: &Repo) -> Result<()> {
        let path = store_path(repo)?;
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Repair invariants: at least one list, exactly one active, unique non-empty names, each file in one list.
    pub fn normalize(&mut self) {
        if self.lists.is_empty() {
            *self = Changelists::default();
            return;
        }
        let mut names = HashSet::new();
        for (i, l) in self.lists.iter_mut().enumerate() {
            let base = if l.name.trim().is_empty() {
                format!("Changelist {}", i + 1)
            } else {
                l.name.trim().to_string()
            };
            let mut name = base.clone();
            let mut n = 2;
            while !names.insert(name.to_lowercase()) {
                name = format!("{base} ({n})");
                n += 1;
            }
            l.name = name;
        }
        let active = self.lists.iter().position(|l| l.active).unwrap_or(0);
        for (i, l) in self.lists.iter_mut().enumerate() {
            l.active = i == active;
        }
        let mut seen = HashSet::new();
        for l in &mut self.lists {
            l.files.retain(|f| seen.insert(f.clone()));
        }
    }

    /// Index of the active list.
    pub fn active(&self) -> usize {
        self.lists.iter().position(|l| l.active).unwrap_or(0)
    }

    /// Index of the list containing `path` (the active list when unassigned).
    pub fn list_of(&self, path: &str) -> usize {
        self.lists
            .iter()
            .position(|l| l.files.iter().any(|f| f == path))
            .unwrap_or_else(|| self.active())
    }

    /// Path → list index for every assigned file (fast lookups over many files).
    pub fn assignments(&self) -> HashMap<&str, usize> {
        self.lists
            .iter()
            .enumerate()
            .flat_map(|(i, l)| l.files.iter().map(move |f| (f.as_str(), i)))
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.lists
            .iter()
            .position(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Assign unassigned `changed` files to the active list and drop files that are no longer changed.
    /// Returns true when anything changed (so the caller can save).
    pub fn sync(&mut self, changed: &[String]) -> bool {
        let set: HashSet<&str> = changed.iter().map(String::as_str).collect();
        let mut dirty = false;
        for l in &mut self.lists {
            let before = l.files.len();
            l.files.retain(|f| set.contains(f.as_str()));
            dirty |= l.files.len() != before;
        }
        let assigned: HashSet<String> = self
            .lists
            .iter()
            .flat_map(|l| l.files.iter().cloned())
            .collect();
        let active = self.active();
        for p in changed {
            if !assigned.contains(p) {
                self.lists[active].files.push(p.clone());
                dirty = true;
            }
        }
        dirty
    }

    /// Add a new list; returns its index.
    pub fn create(&mut self, name: &str, description: &str) -> Result<usize> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Changelist name cannot be empty");
        }
        if self.find(name).is_some() {
            bail!("A changelist named \"{name}\" already exists");
        }
        let mut l = Changelist::new(name);
        l.description = description.trim().to_string();
        self.lists.push(l);
        Ok(self.lists.len() - 1)
    }

    pub fn rename(&mut self, idx: usize, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Changelist name cannot be empty");
        }
        if self.find(name).is_some_and(|i| i != idx) {
            bail!("A changelist named \"{name}\" already exists");
        }
        match self.lists.get_mut(idx) {
            Some(l) => {
                l.name = name.to_string();
                Ok(())
            }
            None => bail!("No such changelist"),
        }
    }

    /// Delete list `idx`; its files move to the default list, which also becomes active if needed.
    pub fn remove(&mut self, idx: usize) -> Result<()> {
        if idx == 0 {
            bail!("The default changelist cannot be deleted");
        }
        if idx >= self.lists.len() {
            bail!("No such changelist");
        }
        let l = self.lists.remove(idx);
        self.lists[0].files.extend(l.files);
        if l.active {
            self.lists[0].active = true;
        }
        Ok(())
    }

    pub fn set_active(&mut self, idx: usize) {
        if idx < self.lists.len() {
            for (i, l) in self.lists.iter_mut().enumerate() {
                l.active = i == idx;
            }
        }
    }

    pub fn set_description(&mut self, idx: usize, description: &str) {
        if let Some(l) = self.lists.get_mut(idx) {
            l.description = description.trim().to_string();
        }
    }

    /// Move `paths` to list `to`.
    pub fn move_files(&mut self, paths: &[String], to: usize) {
        if to >= self.lists.len() {
            return;
        }
        let set: HashSet<&str> = paths.iter().map(String::as_str).collect();
        for l in &mut self.lists {
            l.files.retain(|f| !set.contains(f.as_str()));
        }
        self.lists[to].files.extend(paths.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_and_sync() {
        let mut cl = Changelists::default();
        assert_eq!(cl.lists[0].name, DEFAULT_NAME);
        assert!(cl.sync(&paths(&["a", "b"])));
        assert_eq!(cl.lists[0].files, paths(&["a", "b"]));
        assert!(!cl.sync(&paths(&["a", "b"])));
        let feat = cl.create("Feature", "work in progress").unwrap();
        cl.set_active(feat);
        assert!(cl.sync(&paths(&["a", "b", "c"])));
        assert_eq!(cl.lists[feat].files, paths(&["c"]));
        // b is no longer changed: assignment is cleaned.
        assert!(cl.sync(&paths(&["a", "c"])));
        assert_eq!(cl.lists[0].files, paths(&["a"]));
        assert_eq!(cl.list_of("zzz"), feat);
        let map = cl.assignments();
        assert_eq!(map.get("c"), Some(&feat));
        assert_eq!(map.get("a"), Some(&0));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn create_rename_remove_move() {
        let mut cl = Changelists::default();
        cl.sync(&paths(&["x", "y"]));
        let i = cl.create("UI", "").unwrap();
        assert!(cl.create("ui", "").is_err());
        assert!(cl.create("  ", "").is_err());
        cl.move_files(&paths(&["y"]), i);
        assert_eq!(cl.list_of("y"), i);
        cl.rename(i, "Frontend").unwrap();
        assert!(cl.rename(i, DEFAULT_NAME).is_err());
        cl.set_description(i, " polish ");
        assert_eq!(cl.lists[i].description, "polish");
        cl.set_active(i);
        assert!(cl.remove(0).is_err());
        cl.remove(i).unwrap();
        assert_eq!(cl.lists.len(), 1);
        assert!(cl.lists[0].active);
        assert_eq!(cl.list_of("y"), 0);
    }

    #[test]
    fn normalize_repairs() {
        let mut cl = Changelists {
            lists: vec![
                Changelist {
                    name: "A".into(),
                    description: String::new(),
                    active: true,
                    files: paths(&["f"]),
                },
                Changelist {
                    name: "a".into(),
                    description: String::new(),
                    active: true,
                    files: paths(&["f", "g"]),
                },
            ],
        };
        cl.normalize();
        assert_eq!(cl.lists[1].name, "a (2)");
        assert!(cl.lists[0].active && !cl.lists[1].active);
        assert_eq!(cl.lists[1].files, paths(&["g"]));
        let mut empty = Changelists { lists: vec![] };
        empty.normalize();
        assert_eq!(empty, Changelists::default());
    }

    #[test]
    fn persists_in_data_dir() {
        let (_d, repo) = temp_repo();
        let mut cl = Changelists::load(&repo).unwrap();
        assert_eq!(cl, Changelists::default());
        cl.create("Later", "desc").unwrap();
        cl.save(&repo).unwrap();
        assert!(repo.data_dir().unwrap().join("changelists.json").exists());
        let back = Changelists::load(&repo).unwrap();
        assert_eq!(back.lists[1].name, "Later");
        assert_eq!(back.lists[1].description, "desc");
    }

    #[test]
    fn damaged_store_is_backed_up() {
        let (_d, repo) = temp_repo();
        let path = store_path(&repo).unwrap();
        std::fs::write(&path, "{ \"lists\": [ {\"name\": \"Half").unwrap();
        let (cl, backup) = Changelists::load_checked(&repo).unwrap();
        assert_eq!(cl, Changelists::default());
        let backup = backup.expect("backup path");
        assert_eq!(backup.file_name().unwrap(), "changelists.json.bak");
        assert!(std::fs::read_to_string(&backup).unwrap().contains("Half"));
        assert!(!path.exists());
        // Saving the fallback does not touch the backup; a second damaged file gets a new name.
        cl.save(&repo).unwrap();
        assert!(Changelists::load_checked(&repo).unwrap().1.is_none());
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        let second = Changelists::load_checked(&repo).unwrap().1.unwrap();
        assert_eq!(second.file_name().unwrap(), "changelists.json.2.bak");
        assert!(backup.exists());
    }
}
