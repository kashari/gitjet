//! Shelf: named saved patches in the gitjet data dir, shelve/unshelve (whole or partial), delete, rename.
//!
//! Layout: `<data dir>/shelf/<id>/` holds `meta.json` ([`ShelfMeta`]), `changes.patch` (binary-safe
//! diff of the tracked files against `HEAD`) and `untracked/<path>` copies of untracked files.
//! An entry whose files have all been unshelved stays around in the "applied" state
//! ("recently unshelved") until it is deleted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::diff::{parse_unified, ChangeKind, FileDiff};
use super::patch::{self, nul_paths, ApplyOutcome};
use super::status::FileState;
use super::Repo;

const META: &str = "meta.json";
const PATCH: &str = "changes.patch";
const UNTRACKED: &str = "untracked";

/// One shelved file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelvedFile {
    /// Repo-relative path (`/` separators)
    pub path: String,
    /// Source path of a rename
    #[serde(default)]
    pub orig_path: Option<String>,
    /// One-letter status at shelve time (`M`, `A`, `D`, `R`, `?`...)
    pub status: char,
    /// Stored as a file copy under `untracked/` instead of in the patch
    pub untracked: bool,
}

/// `meta.json` of a shelf entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelfMeta {
    pub name: String,
    /// Creation time (unix seconds)
    pub created: i64,
    pub files: Vec<ShelvedFile>,
    /// Branch checked out when shelving
    #[serde(default)]
    pub branch: Option<String>,
    /// `HEAD` commit when shelving (None in an empty repository)
    #[serde(default)]
    pub base: Option<String>,
    /// Paths already unshelved
    #[serde(default)]
    pub unshelved: Vec<String>,
    /// When the last file was unshelved (the entry is then "applied")
    #[serde(default)]
    pub applied_at: Option<i64>,
}

/// A shelf entry on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfEntry {
    /// Directory name, unique and sortable by creation time
    pub id: String,
    pub dir: PathBuf,
    pub meta: ShelfMeta,
}

impl ShelfEntry {
    /// True once every file has been unshelved.
    pub fn is_applied(&self) -> bool {
        self.meta.applied_at.is_some()
    }

    pub fn is_unshelved(&self, path: &str) -> bool {
        self.meta.unshelved.iter().any(|p| p == path)
    }

    /// Files not yet unshelved (all files when the entry is applied).
    pub fn pending_files(&self) -> Vec<&ShelvedFile> {
        if self.is_applied() {
            return self.meta.files.iter().collect();
        }
        self.meta
            .files
            .iter()
            .filter(|f| !self.is_unshelved(&f.path))
            .collect()
    }

    fn patch_file(&self) -> PathBuf {
        self.dir.join(PATCH)
    }

    /// Where the copy of an untracked file is stored.
    pub fn untracked_copy(&self, path: &str) -> PathBuf {
        path.split('/')
            .filter(|s| !s.is_empty())
            .fold(self.dir.join(UNTRACKED), |p, s| p.join(s))
    }

    /// Raw `changes.patch` (tracked files).
    pub fn patch_text(&self) -> Result<String> {
        match std::fs::read(self.patch_file()) {
            Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(anyhow!("reading {}: {e}", self.patch_file().display())),
        }
    }

    /// Diffs of every shelved file, in `meta.files` order (for previews).
    /// Renames are stored as a deletion plus an addition and shown as one rename.
    pub fn diffs(&self) -> Result<Vec<FileDiff>> {
        let tracked = parse_unified(&self.patch_text()?);
        let touching = |p: &str| {
            tracked
                .iter()
                .find(|d| d.new_path.as_deref() == Some(p) || d.old_path.as_deref() == Some(p))
        };
        let mut out = Vec::new();
        for f in &self.meta.files {
            if f.untracked {
                out.push(patch::file_as_added_diff(
                    &f.path,
                    &self.untracked_copy(&f.path),
                )?);
                continue;
            }
            let Some(d) = touching(&f.path) else { continue };
            let deleted_orig = f
                .orig_path
                .as_deref()
                .and_then(|o| touching(o).map(|od| (o, od)))
                .filter(|(_, od)| od.kind == ChangeKind::Deleted && d.kind == ChangeKind::Added);
            match deleted_orig {
                Some((o, od)) => out.push(patch::rename_preview(o, &f.path, od, d)),
                None => out.push(d.clone()),
            }
        }
        Ok(out)
    }

    fn save(&self) -> Result<()> {
        write_meta(&self.dir, &self.meta)
    }
}

fn write_meta(dir: &Path, meta: &ShelfMeta) -> Result<()> {
    let text = serde_json::to_string_pretty(meta)?;
    let tmp = dir.join(format!("{META}.tmp"));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(META))
        .with_context(|| format!("saving {}", dir.join(META).display()))
}

/// `<data dir>/shelf`
pub fn shelf_dir(repo: &Repo) -> Result<PathBuf> {
    let d = repo.data_dir()?.join("shelf");
    std::fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    Ok(d)
}

/// All shelf entries: pending ones first, then applied ones, each newest first.
/// Directories without a readable `meta.json` (e.g. interrupted shelving) are skipped.
pub fn list(repo: &Repo) -> Result<Vec<ShelfEntry>> {
    let root = shelf_dir(repo)?;
    let mut v = Vec::new();
    for e in std::fs::read_dir(&root).with_context(|| format!("reading {}", root.display()))? {
        let Ok(e) = e else { continue };
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(dir.join(META)) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<ShelfMeta>(&text) else {
            continue;
        };
        v.push(ShelfEntry {
            id: e.file_name().to_string_lossy().into_owned(),
            dir,
            meta,
        });
    }
    v.sort_by(|a, b| {
        a.is_applied()
            .cmp(&b.is_applied())
            .then(b.meta.created.cmp(&a.meta.created))
            .then(b.id.cmp(&a.id))
    });
    Ok(v)
}

/// Load one entry by id.
pub fn get(repo: &Repo, id: &str) -> Result<ShelfEntry> {
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        bail!("invalid shelf id {id:?}");
    }
    let dir = shelf_dir(repo)?.join(id);
    let text = std::fs::read_to_string(dir.join(META))
        .with_context(|| format!("shelf entry {id} not found"))?;
    let meta =
        serde_json::from_str(&text).with_context(|| format!("shelf entry {id} is damaged"))?;
    Ok(ShelfEntry {
        id: id.to_string(),
        dir,
        meta,
    })
}

/// Default name for a new shelf entry, like "Changes 2026-10-27 14:03".
pub fn default_name() -> String {
    format!("Changes {}", chrono::Local::now().format("%Y-%m-%d %H:%M"))
}

fn new_id(root: &Path) -> String {
    let base = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut n = 0u32;
    loop {
        let id = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        if !root.join(&id).exists() {
            return id;
        }
        n += 1;
    }
}

/// Shelve `paths` (work tree changes vs `HEAD`) under `name`. Unless `keep_in_worktree`, the
/// shelved paths are then reverted: tracked files restored from `HEAD`, new and untracked files deleted.
pub fn shelve(
    repo: &Repo,
    name: &str,
    paths: &[String],
    keep_in_worktree: bool,
) -> Result<ShelfEntry> {
    let files = collect_files(repo, paths)?;
    if files.is_empty() {
        bail!("Nothing to shelve: the selected files have no changes");
    }
    let root = shelf_dir(repo)?;
    let id = new_id(&root);
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = if name.trim().is_empty() {
        default_name()
    } else {
        name.trim().to_string()
    };
    let entry = ShelfEntry {
        id,
        dir: dir.clone(),
        meta: ShelfMeta {
            name,
            created: chrono::Utc::now().timestamp(),
            files,
            branch: repo.head_branch(),
            base: repo.head_commit(),
            unshelved: Vec::new(),
            applied_at: None,
        },
    };
    if let Err(e) = store(repo, &entry) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    if !keep_in_worktree {
        revert(repo, &entry.meta.files)
            .context("Changes were shelved, but reverting them failed")?;
    }
    Ok(entry)
}

/// True if `path` is one of `wanted` or lies inside one of them (directories select their contents).
pub fn path_selected(path: &str, wanted: &[String]) -> bool {
    wanted.iter().any(|w| {
        let w = w.trim_end_matches('/');
        path == w
            || (path.len() > w.len() && path.starts_with(w) && path.as_bytes()[w.len()] == b'/')
    })
}

/// Status of the requested paths; renames pull in their source path. Conflicts are refused.
fn collect_files(repo: &Repo, paths: &[String]) -> Result<Vec<ShelvedFile>> {
    let st = repo.status()?;
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for e in st.entries.iter().filter(|e| path_selected(&e.path, paths)) {
        if !seen.insert(e.path.as_str()) {
            continue;
        }
        if e.is_ignored() || e.submodule {
            continue;
        }
        if e.is_conflicted() {
            bail!("{} has unresolved conflicts and cannot be shelved", e.path);
        }
        let state = e.display_state();
        let status = if e.orig_path.is_some() {
            'R'
        } else {
            state.letter()
        };
        files.push(ShelvedFile {
            path: e.path.clone(),
            orig_path: e.orig_path.clone(),
            status,
            untracked: state == FileState::Untracked,
        });
    }
    Ok(files)
}

/// Write the patch, the untracked copies and finally `meta.json`.
fn store(repo: &Repo, entry: &ShelfEntry) -> Result<()> {
    let tracked = tracked_pathspec(&entry.meta.files);
    // No rename detection: every shelved path gets its own patch section, so a partial unshelve
    // never drags in the other side of a detected rename.
    let text = patch::paths_patch(repo, &tracked, patch::Renames::Off)?;
    std::fs::write(entry.patch_file(), &text).context("writing changes.patch")?;
    for f in entry.meta.files.iter().filter(|f| f.untracked) {
        let dst = entry.untracked_copy(&f.path);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(repo.path(&f.path), &dst).with_context(|| format!("copying {}", f.path))?;
    }
    entry.save()
}

/// Tracked paths of `files`, including rename sources.
fn tracked_pathspec(files: &[ShelvedFile]) -> Vec<String> {
    let mut v = Vec::new();
    for f in files.iter().filter(|f| !f.untracked) {
        v.push(f.path.clone());
        if let Some(o) = &f.orig_path {
            v.push(o.clone());
        }
    }
    v
}

/// Revert shelved paths in the work tree and index.
fn revert(repo: &Repo, files: &[ShelvedFile]) -> Result<()> {
    let tracked = tracked_pathspec(files);
    let (in_head, not_in_head) = patch::split_by_head(repo, &tracked)?;
    if !in_head.is_empty() {
        repo.run_stdin(
            &[
                "--literal-pathspecs",
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &nul_paths(&in_head),
        )?;
    }
    if !not_in_head.is_empty() {
        repo.run_stdin(
            &[
                "--literal-pathspecs",
                "rm",
                "-q",
                "--cached",
                "--force",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &nul_paths(&not_in_head),
        )?;
    }
    let deleted = not_in_head
        .iter()
        .cloned()
        .chain(files.iter().filter(|f| f.untracked).map(|f| f.path.clone()));
    for p in deleted {
        remove_file_and_empty_parents(repo, &p)?;
    }
    Ok(())
}

fn remove_file_and_empty_parents(repo: &Repo, rel: &str) -> Result<()> {
    let p = repo.path(rel);
    match std::fs::remove_file(&p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow!("deleting {rel}: {e}")),
    }
    let mut dir = p.parent();
    while let Some(d) = dir {
        if d == repo.root || !d.starts_with(&repo.root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// Untracked files among `paths` whose shelved copy would overwrite a different existing file.
pub fn untracked_collisions(repo: &Repo, entry: &ShelfEntry, paths: &[String]) -> Vec<String> {
    entry
        .meta
        .files
        .iter()
        .filter(|f| f.untracked && paths.contains(&f.path))
        .filter(|f| {
            let target = repo.path(&f.path);
            target.exists()
                && std::fs::read(&target).ok() != std::fs::read(entry.untracked_copy(&f.path)).ok()
        })
        .map(|f| f.path.clone())
        .collect()
}

/// Result of an unshelve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnshelveOutcome {
    /// Files restored
    pub applied: usize,
    /// Files left with conflict markers by the 3-way merge
    pub conflicts: Vec<String>,
    /// Untracked files not restored because a different file exists (and overwriting was declined)
    pub skipped: Vec<String>,
    /// Every file of the entry has now been unshelved
    pub entry_applied: bool,
}

/// Unshelve `paths` of entry `id` (empty = every pending file). The patch part is applied with a
/// 3-way merge fallback; untracked copies are restored, replacing different existing files only
/// when `overwrite` is set.
pub fn unshelve(
    repo: &Repo,
    id: &str,
    paths: &[String],
    overwrite: bool,
) -> Result<UnshelveOutcome> {
    let mut entry = get(repo, id)?;
    let selected: Vec<ShelvedFile> = if paths.is_empty() {
        entry.pending_files().into_iter().cloned().collect()
    } else {
        entry
            .meta
            .files
            .iter()
            .filter(|f| paths.contains(&f.path))
            .cloned()
            .collect()
    };
    if selected.is_empty() {
        bail!("No files selected to unshelve");
    }
    let tracked = tracked_pathspec(&selected);
    let sub = patch::filter_patch(&entry.patch_text()?, &tracked);
    let conflicts = match patch::apply_to_worktree(repo, &sub)? {
        ApplyOutcome::Clean => Vec::new(),
        ApplyOutcome::Conflicts(c) => c,
    };
    let mut skipped = Vec::new();
    let mut applied = selected.iter().filter(|f| !f.untracked).count();
    for f in selected.iter().filter(|f| f.untracked) {
        let src = entry.untracked_copy(&f.path);
        let dst = repo.path(&f.path);
        if dst.exists() && !overwrite && std::fs::read(&dst).ok() != std::fs::read(&src).ok() {
            skipped.push(f.path.clone());
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("restoring {}", f.path))?;
        applied += 1;
    }
    for f in &selected {
        if !skipped.contains(&f.path) && !entry.is_unshelved(&f.path) {
            entry.meta.unshelved.push(f.path.clone());
        }
    }
    let all = entry.meta.files.iter().all(|f| entry.is_unshelved(&f.path));
    if all {
        entry.meta.applied_at = Some(chrono::Utc::now().timestamp());
    }
    entry.save()?;
    Ok(UnshelveOutcome {
        applied,
        conflicts,
        skipped,
        entry_applied: all,
    })
}

/// Rename an entry.
pub fn rename(repo: &Repo, id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Name is empty");
    }
    let mut e = get(repo, id)?;
    e.meta.name = name.to_string();
    e.save()
}

/// Delete an entry and its files.
pub fn delete(repo: &Repo, id: &str) -> Result<()> {
    let e = get(repo, id)?;
    std::fs::remove_dir_all(&e.dir).with_context(|| format!("deleting {}", e.dir.display()))
}

/// Delete every applied ("recently unshelved") entry; returns how many were removed.
pub fn delete_applied(repo: &Repo) -> Result<usize> {
    let mut n = 0;
    for e in list(repo)?.into_iter().filter(ShelfEntry::is_applied) {
        std::fs::remove_dir_all(&e.dir).with_context(|| format!("deleting {}", e.dir.display()))?;
        n += 1;
    }
    Ok(n)
}

/// The whole entry as one patch (tracked changes plus untracked files as new-file diffs).
pub fn export_patch(repo: &Repo, id: &str) -> Result<String> {
    let e = get(repo, id)?;
    let mut text = e.patch_text()?;
    let copies: Vec<(String, PathBuf)> = e
        .meta
        .files
        .iter()
        .filter(|f| f.untracked)
        .map(|f| (f.path.clone(), e.untracked_copy(&f.path)))
        .collect();
    let extra = patch::new_files_patch(repo, &copies)?;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&extra);
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn read(repo: &Repo, p: &str) -> String {
        std::fs::read_to_string(repo.path(p))
            .unwrap()
            .replace("\r\n", "\n")
    }

    const BIN: [u8; 6] = [0, 1, 2, 255, 0, 7];
    const BIN2: [u8; 4] = [9, 0, 9, 0];

    /// Modified, staged-new, deleted, renamed, binary and untracked changes.
    fn setup() -> (TempDir, Repo, Vec<String>) {
        let (d, repo) = temp_repo();
        commit_files(
            &repo,
            &[
                ("a.txt", "1\n2\n"),
                ("del.txt", "d\n"),
                ("old.txt", "same\n"),
                ("keep.txt", "k\n"),
            ],
            "init",
        );
        std::fs::write(repo.path("img.bin"), BIN).unwrap();
        repo.run(&["add", "img.bin"]).unwrap();
        repo.run(&["commit", "-qm", "bin"]).unwrap();
        write(&repo, "a.txt", "1\nTWO\n");
        std::fs::remove_file(repo.path("del.txt")).unwrap();
        repo.run(&["mv", "old.txt", "renamed.txt"]).unwrap();
        std::fs::write(repo.path("img.bin"), BIN2).unwrap();
        write(&repo, "staged new.txt", "sn\n");
        repo.run(&["add", "staged new.txt"]).unwrap();
        write(&repo, "u dir/untracked.txt", "u\n");
        let paths = [
            "a.txt",
            "del.txt",
            "renamed.txt",
            "img.bin",
            "staged new.txt",
            "u dir/untracked.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        (d, repo, paths)
    }

    fn assert_shelved_state(repo: &Repo) {
        assert_eq!(read(repo, "a.txt"), "1\n2\n");
        assert_eq!(read(repo, "del.txt"), "d\n");
        assert_eq!(read(repo, "old.txt"), "same\n");
        assert!(!repo.path("renamed.txt").exists());
        assert_eq!(std::fs::read(repo.path("img.bin")).unwrap(), BIN);
        assert!(!repo.path("staged new.txt").exists());
        assert!(!repo.path("u dir").exists());
        assert!(repo.status().unwrap().is_clean());
    }

    fn assert_restored_state(repo: &Repo) {
        assert_eq!(read(repo, "a.txt"), "1\nTWO\n");
        assert!(!repo.path("del.txt").exists());
        assert!(!repo.path("old.txt").exists());
        assert_eq!(read(repo, "renamed.txt"), "same\n");
        assert_eq!(std::fs::read(repo.path("img.bin")).unwrap(), BIN2);
        assert_eq!(read(repo, "staged new.txt"), "sn\n");
        assert_eq!(read(repo, "u dir/untracked.txt"), "u\n");
    }

    #[test]
    fn shelve_unshelve_roundtrip() {
        let (_d, repo, paths) = setup();
        let e = shelve(&repo, "my work", &paths, false).unwrap();
        assert_eq!(e.meta.files.len(), 6);
        assert_eq!(e.meta.branch.as_deref(), Some("main"));
        assert!(e.meta.base.is_some());
        let renamed = e
            .meta
            .files
            .iter()
            .find(|f| f.path == "renamed.txt")
            .unwrap();
        assert_eq!(renamed.orig_path.as_deref(), Some("old.txt"));
        assert!(
            e.meta
                .files
                .iter()
                .find(|f| f.path == "u dir/untracked.txt")
                .unwrap()
                .untracked
        );
        assert_shelved_state(&repo);
        // Listing and previews.
        let l = list(&repo).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].meta.name, "my work");
        let diffs = l[0].diffs().unwrap();
        assert_eq!(diffs.len(), 6);
        assert!(diffs.iter().any(|d| d.path() == "img.bin" && d.binary));
        // Unshelve everything.
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert!(out.conflicts.is_empty() && out.skipped.is_empty());
        assert!(out.entry_applied);
        assert_eq!(out.applied, 6);
        assert_restored_state(&repo);
        // Entry is kept as applied and can be unshelved again (after reverting).
        let e2 = get(&repo, &e.id).unwrap();
        assert!(e2.is_applied());
        assert_eq!(e2.pending_files().len(), 6);
        assert_eq!(list(&repo).unwrap().len(), 1);
        assert_eq!(delete_applied(&repo).unwrap(), 1);
        assert!(list(&repo).unwrap().is_empty());
    }

    #[test]
    fn keep_in_worktree_and_export() {
        let (_d, repo, paths) = setup();
        let e = shelve(&repo, "", &paths, true).unwrap();
        assert!(e.meta.name.starts_with("Changes "));
        assert_restored_state(&repo);
        let exported = export_patch(&repo, &e.id).unwrap();
        // The exported patch recreates everything from a clean tree.
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        repo.run(&["clean", "-fdq"]).unwrap();
        assert_eq!(
            patch::apply_to_worktree(&repo, &exported).unwrap(),
            ApplyOutcome::Clean
        );
        assert_restored_state(&repo);
    }

    #[test]
    fn partial_unshelve_then_rest() {
        let (_d, repo, paths) = setup();
        let e = shelve(&repo, "partial", &paths, false).unwrap();
        let out = unshelve(
            &repo,
            &e.id,
            &[
                "a.txt".into(),
                "renamed.txt".into(),
                "u dir/untracked.txt".into(),
            ],
            false,
        )
        .unwrap();
        assert_eq!(out.applied, 3);
        assert!(!out.entry_applied);
        assert_eq!(read(&repo, "a.txt"), "1\nTWO\n");
        assert_eq!(read(&repo, "renamed.txt"), "same\n");
        assert!(!repo.path("old.txt").exists());
        assert_eq!(read(&repo, "u dir/untracked.txt"), "u\n");
        // Not selected: still in shelved state.
        assert_eq!(read(&repo, "del.txt"), "d\n");
        assert_eq!(std::fs::read(repo.path("img.bin")).unwrap(), BIN);
        let e2 = get(&repo, &e.id).unwrap();
        assert!(!e2.is_applied());
        let pending: Vec<&str> = e2.pending_files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(pending, vec!["del.txt", "img.bin", "staged new.txt"]);
        // Unshelving the rest (empty selection = pending files) completes the entry.
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert_eq!(out.applied, 3);
        assert!(out.entry_applied);
        assert_restored_state(&repo);
    }

    #[test]
    fn shelve_with_staged_then_deleted_file() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "ad.txt", "x\n");
        repo.run(&["add", "ad.txt"]).unwrap();
        std::fs::remove_file(repo.path("ad.txt")).unwrap();
        write(&repo, "a.txt", "2\n");
        let e = shelve(&repo, "ad", &["a.txt".into(), "ad.txt".into()], false).unwrap();
        assert_eq!(e.meta.files.len(), 2);
        // The stale index entry is gone and the work tree is clean.
        assert!(repo.status().unwrap().is_clean());
        assert_eq!(read(&repo, "a.txt"), "1\n");
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert!(out.entry_applied && out.conflicts.is_empty());
        assert_eq!(read(&repo, "a.txt"), "2\n");
        assert!(!repo.path("ad.txt").exists());
    }

    #[test]
    fn partial_unshelve_of_one_side_of_a_move() {
        let (_d, repo) = temp_repo();
        let content = "line 1\nline 2\nline 3\nline 4\n";
        commit_files(&repo, &[("old.txt", content)], "init");
        // A move git status reports as two changes: old.txt deleted, new.txt added.
        std::fs::rename(repo.path("old.txt"), repo.path("new.txt")).unwrap();
        repo.run(&["add", "new.txt"]).unwrap();
        repo.run(&["config", "diff.renames", "true"]).unwrap();
        let e = shelve(&repo, "move", &["old.txt".into(), "new.txt".into()], false).unwrap();
        assert_eq!(e.meta.files.len(), 2);
        assert_eq!(e.diffs().unwrap().len(), 2);
        assert_eq!(read(&repo, "old.txt"), content);
        // Only new.txt: old.txt must stay.
        let out = unshelve(&repo, &e.id, &["new.txt".into()], false).unwrap();
        assert!(!out.entry_applied);
        assert_eq!(read(&repo, "new.txt"), content);
        assert_eq!(read(&repo, "old.txt"), content);
        // The rest completes the entry.
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert!(out.entry_applied);
        assert!(!repo.path("old.txt").exists());
    }

    #[test]
    fn renames_are_previewed_as_one_file() {
        let (_d, repo, paths) = setup();
        let e = shelve(&repo, "r", &paths, true).unwrap();
        let diffs = e.diffs().unwrap();
        let r = diffs.iter().find(|d| d.path() == "renamed.txt").unwrap();
        assert_eq!(r.kind, ChangeKind::Renamed);
        assert_eq!(r.old_path.as_deref(), Some("old.txt"));
        assert!(r.hunks.is_empty());
        assert!(!diffs.iter().any(|d| d.path() == "old.txt"));
    }

    #[test]
    fn untracked_collisions_need_overwrite() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("x", "1\n")], "init");
        write(&repo, "u.txt", "shelved\n");
        let e = shelve(&repo, "u", &["u.txt".into()], false).unwrap();
        assert!(!repo.path("u.txt").exists());
        write(&repo, "u.txt", "other\n");
        assert_eq!(
            untracked_collisions(&repo, &e, &["u.txt".into()]),
            vec!["u.txt"]
        );
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert_eq!(out.skipped, vec!["u.txt"]);
        assert!(!out.entry_applied);
        assert_eq!(read(&repo, "u.txt"), "other\n");
        let out = unshelve(&repo, &e.id, &[], true).unwrap();
        assert!(out.entry_applied);
        assert_eq!(read(&repo, "u.txt"), "shelved\n");
        // Identical content is not a collision.
        assert!(
            untracked_collisions(&repo, &get(&repo, &e.id).unwrap(), &["u.txt".into()]).is_empty()
        );
    }

    #[test]
    fn unshelve_conflict_after_upstream_change() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "base\n")], "init");
        write(&repo, "a.txt", "shelved\n");
        let e = shelve(&repo, "c", &["a.txt".into()], false).unwrap();
        commit_files(&repo, &[("a.txt", "upstream\n")], "upstream");
        let out = unshelve(&repo, &e.id, &[], false).unwrap();
        assert_eq!(out.conflicts, vec!["a.txt"]);
        assert!(read(&repo, "a.txt").contains("<<<<<<<"));
    }

    #[test]
    fn shelve_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        write(&repo, "b.txt", "2\n");
        repo.run(&["add", "b.txt"]).unwrap();
        let e = shelve(&repo, "empty", &["a.txt".into(), "b.txt".into()], false).unwrap();
        assert!(e.meta.base.is_none());
        assert!(!repo.path("a.txt").exists() && !repo.path("b.txt").exists());
        unshelve(&repo, &e.id, &[], false).unwrap();
        assert_eq!(read(&repo, "a.txt"), "1\n");
        assert_eq!(read(&repo, "b.txt"), "2\n");
    }

    #[test]
    fn directory_selection() {
        let w = vec!["src/".to_string(), "a.txt".to_string()];
        assert!(path_selected("src/x/y.rs", &w));
        assert!(path_selected("a.txt", &w));
        assert!(!path_selected("srcfoo/z", &w));
        assert!(!path_selected("a.txt2", &w));
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("x", "1\n")], "init");
        write(&repo, "dir/one.txt", "1\n");
        write(&repo, "dir/sub/two.txt", "2\n");
        write(&repo, "other.txt", "o\n");
        let e = shelve(&repo, "dir", &["dir".into()], false).unwrap();
        assert_eq!(e.meta.files.len(), 2);
        assert!(repo.path("other.txt").exists() && !repo.path("dir").exists());
    }

    #[test]
    fn rename_delete_and_errors() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("x", "1\n")], "init");
        assert!(shelve(&repo, "n", &["x".into()], false).is_err());
        write(&repo, "x", "2\n");
        let e = shelve(&repo, "n", &["x".into()], false).unwrap();
        rename(&repo, &e.id, "  renamed  ").unwrap();
        assert_eq!(get(&repo, &e.id).unwrap().meta.name, "renamed");
        assert!(rename(&repo, &e.id, " ").is_err());
        assert!(get(&repo, "../x").is_err());
        // A second entry within the same second gets a distinct id.
        write(&repo, "x", "3\n");
        let e2 = shelve(&repo, "n2", &["x".into()], false).unwrap();
        assert_ne!(e.id, e2.id);
        assert_eq!(list(&repo).unwrap()[0].id, e2.id);
        delete(&repo, &e.id).unwrap();
        assert_eq!(list(&repo).unwrap().len(), 1);
        // Damaged entries are ignored by the listing.
        std::fs::create_dir_all(shelf_dir(&repo).unwrap().join("broken")).unwrap();
        assert_eq!(list(&repo).unwrap().len(), 1);
    }
}
