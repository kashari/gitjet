//! Local History: periodic snapshots of changed files, independent of commits.
//!
//! Storage lives under `<data dir>/local-history/` (one store per work tree):
//! - `objects/<aa>/<rest>`: file contents, content-addressed by a 64-bit FNV-1a hash (deduplicated)
//! - `index.jsonl`: append-only JSON lines, one [`Snapshot`] per line (`hash: null` records a deletion)
//! - `state.json`: path → (mtime, size) cache so unchanged files are skipped without reading them
//! - `lock`: a lock file serializing writers (the app's timer thread and user actions)
//!
//! Every write is either an append of complete lines or a write-to-temp-then-rename, so a crash never
//! leaves a half-written object or index behind. Malformed index lines are skipped when reading.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::LocalHistoryConfig;
use crate::git::Repo;

/// Upper bound of files examined per run, so a huge untracked tree cannot stall the snapshot thread.
const MAX_CANDIDATES: usize = 5000;
/// A lock file older than this is considered left over from a crashed process.
const STALE_LOCK: Duration = Duration::from_secs(60);
/// How long a writer waits for the lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// A file modified this recently may still change within the same mtime tick (coarse timestamps on
/// HFS+, FAT, network shares), so its (mtime, size) stamp is not trusted (git's "racy clean" rule).
const RACY_WINDOW: Duration = Duration::from_secs(3);
/// Pseudo path of project-wide label markers in the index.
pub const LABEL_PATH: &str = "";

/// One entry of the local history index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Repo-relative path with `/` separators ([`LABEL_PATH`] for a project-wide label marker)
    pub path: String,
    /// Unix time (seconds)
    pub time: i64,
    /// Content hash (see [`object_path`]); `None` records that the file was deleted
    pub hash: Option<String>,
    /// "Put Label" text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Snapshot {
    /// True for a project-wide label marker (not tied to a file).
    pub fn is_marker(&self) -> bool {
        self.path == LABEL_PATH
    }

    /// True when this entry records a deletion of the file.
    pub fn is_deletion(&self) -> bool {
        !self.is_marker() && self.hash.is_none()
    }
}

/// A file that has local history, for the file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryFile {
    pub path: String,
    /// Time of the newest snapshot
    pub last_time: i64,
    pub count: usize,
    /// The newest entry records a deletion
    pub deleted: bool,
}

/// 64-bit FNV-1a hash.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Object name for `data`: the FNV-1a hash plus the length, which makes accidental collisions
/// between different contents practically impossible for source files.
pub fn content_hash(data: &[u8]) -> String {
    format!("{:016x}{:x}", fnv1a64(data), data.len())
}

/// Root of this work tree's store. Linked worktrees get their own subdirectory since paths in the
/// index are relative to the work tree.
fn store_dir(repo: &Repo) -> Result<PathBuf> {
    let base = repo.data_dir()?.join("local-history");
    let dir = if repo.git_dir != repo.common_dir {
        let key = fnv1a64(repo.root.to_string_lossy().as_bytes());
        base.join("worktrees").join(format!("{key:016x}"))
    } else {
        base
    };
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn index_file(dir: &Path) -> PathBuf {
    dir.join("index.jsonl")
}

fn objects_dir(dir: &Path) -> PathBuf {
    dir.join("objects")
}

fn object_in(dir: &Path, hash: &str) -> PathBuf {
    let (a, b) = hash.split_at(hash.len().min(2));
    objects_dir(dir).join(a).join(b)
}

/// Absolute path of the stored content for `hash` (a file on disk, usable with `diff --no-index`).
pub fn object_path(repo: &Repo, hash: &str) -> Result<PathBuf> {
    if hash.len() < 3 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid local history object id {hash:?}");
    }
    Ok(object_in(&store_dir(repo)?, hash))
}

/// An empty file inside the store, to diff a snapshot against a file that no longer exists.
pub fn empty_file(repo: &Repo) -> Result<PathBuf> {
    let p = store_dir(repo)?.join("empty");
    if !p.exists() {
        fs::write(&p, b"").with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(p)
}

/// Size and modification time of the index, to cheaply detect whether it changed since last read.
pub fn index_stamp(repo: &Repo) -> Option<(u64, SystemTime)> {
    let dir = store_dir(repo).ok()?;
    let m = fs::metadata(index_file(&dir)).ok()?;
    Some((m.len(), m.modified().ok()?))
}

// ----- locking & atomic writes -------------------------------------------------------------------

/// Exclusive writer lock; released on drop.
struct Lock(PathBuf);

impl Lock {
    fn acquire(dir: &Path) -> Result<Lock> {
        let path = dir.join("lock");
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Lock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().unwrap_or_default() > STALE_LOCK)
                        .unwrap_or(false);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() > LOCK_WAIT {
                        bail!("local history is busy (lock file {})", path.display());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Write `data` to `path` via a temporary file and rename, so readers never see partial content.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(())
}

// ----- index & state -----------------------------------------------------------------------------

fn parse_index(text: &str) -> Vec<Snapshot> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Snapshot>(l).ok())
        .collect()
}

fn read_index(dir: &Path) -> Vec<Snapshot> {
    fs::read_to_string(index_file(dir))
        .map(|t| parse_index(&t))
        .unwrap_or_default()
}

fn append_index(dir: &Path, entries: &[Snapshot]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(e)?);
        text.push('\n');
    }
    let path = index_file(dir);
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    f.write_all(text.as_bytes())
        .with_context(|| format!("appending to {}", path.display()))
}

type StatCache = HashMap<String, (i128, u64)>;

fn read_state(dir: &Path) -> StatCache {
    fs::read_to_string(dir.join("state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, state: &StatCache) -> Result<()> {
    write_atomic(&dir.join("state.json"), &serde_json::to_vec(state)?)
}

/// Latest entry per path (in index order, later wins).
fn latest_by_path(index: &[Snapshot]) -> HashMap<&str, &Snapshot> {
    let mut m = HashMap::new();
    for e in index.iter().filter(|e| !e.is_marker()) {
        m.insert(e.path.as_str(), e);
    }
    m
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True when the file's mtime is too recent (or in the future) for its stamp to prove "unchanged".
fn is_racy(m: &fs::Metadata) -> bool {
    match m.modified() {
        Ok(t) => t.elapsed().map(|age| age < RACY_WINDOW).unwrap_or(true),
        Err(_) => true,
    }
}

fn mtime_key(m: &fs::Metadata) -> i128 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

/// Store `data` as an object (no-op if already present); returns its hash.
fn store_object(dir: &Path, data: &[u8]) -> Result<String> {
    let hash = content_hash(data);
    let p = object_in(dir, &hash);
    if !p.exists() {
        write_atomic(&p, data)?;
    }
    Ok(hash)
}

// ----- snapshotting ------------------------------------------------------------------------------

/// A file to examine in one run.
struct Candidate {
    path: String,
    /// Changed or untracked according to `git status` (as opposed to only known from the history)
    changed: bool,
}

/// Files worth looking at: everything changed/untracked per status, plus every path already in the
/// history (so reverts to the committed version and deletions of untracked files are recorded too).
fn candidates(repo: &Repo, index: &[Snapshot]) -> Result<Vec<Candidate>> {
    let st = repo.status()?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let changed = st
        .entries
        .iter()
        .filter(|e| !e.is_ignored() && !e.submodule)
        .map(|e| (e.path.clone(), true));
    let known = index
        .iter()
        .filter(|e| !e.is_marker())
        .map(|e| (e.path.clone(), false));
    for (path, changed) in changed.chain(known) {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        if seen.insert(path.clone()) {
            out.push(Candidate { path, changed });
        }
    }
    Ok(out)
}

struct Run<'a> {
    repo: &'a Repo,
    dir: PathBuf,
    max_bytes: u64,
    state: StatCache,
    new_entries: Vec<Snapshot>,
}

impl Run<'_> {
    /// Examine one file; pushes a new entry if its content differs from the latest snapshot.
    /// With `force_label`, an entry is recorded even if the content did not change.
    fn visit(
        &mut self,
        path: &str,
        latest: Option<&Snapshot>,
        force_label: Option<&str>,
    ) -> Result<()> {
        let abs = self.repo.path(path);
        let meta = match fs::symlink_metadata(&abs) {
            Ok(m) => m,
            Err(_) => {
                self.state.remove(path);
                // Gone: record a deletion if we have content for it.
                if latest.is_some_and(|l| l.hash.is_some()) {
                    self.push(path, None, force_label);
                }
                return Ok(());
            }
        };
        if !meta.is_file() || meta.len() > self.max_bytes {
            return Ok(());
        }
        let stamp = (mtime_key(&meta), meta.len());
        let have_content = latest.is_some_and(|l| l.hash.is_some());
        if force_label.is_none() && have_content && self.state.get(path) == Some(&stamp) {
            return Ok(());
        }
        let data = match fs::read(&abs) {
            Ok(d) => d,
            // Deleted or locked between stat and read: try again next run.
            Err(_) => return Ok(()),
        };
        if is_racy(&meta) {
            // A later edit may keep this exact stamp: re-read the file next run instead.
            self.state.remove(path);
        } else {
            self.state.insert(path.to_string(), stamp);
        }
        let hash = content_hash(&data);
        let same = latest.and_then(|l| l.hash.as_deref()) == Some(hash.as_str());
        if same && force_label.is_none() {
            return Ok(());
        }
        store_object(&self.dir, &data)?;
        self.push(path, Some(hash), force_label);
        Ok(())
    }

    fn push(&mut self, path: &str, hash: Option<String>, label: Option<&str>) {
        self.new_entries.push(Snapshot {
            path: path.to_string(),
            time: now(),
            hash,
            label: label.map(str::to_string),
        });
    }
}

fn snapshot_impl(
    repo: &Repo,
    cfg: &LocalHistoryConfig,
    label: Option<&str>,
    only: Option<&str>,
) -> Result<usize> {
    let dir = store_dir(repo)?;
    let _lock = Lock::acquire(&dir)?;
    let index = read_index(&dir);
    let paths = match only {
        Some(p) => vec![Candidate {
            path: p.to_string(),
            changed: true,
        }],
        None => candidates(repo, &index)?,
    };
    let latest = latest_by_path(&index);
    let mut run = Run {
        repo,
        dir: dir.clone(),
        max_bytes: cfg.max_file_kb.saturating_mul(1024),
        state: read_state(&dir),
        new_entries: Vec::new(),
    };
    for c in &paths {
        // Labels go on files that are actually changed now, not on every file that ever had history.
        let force = label.filter(|_| c.changed);
        run.visit(&c.path, latest.get(c.path.as_str()).copied(), force)?;
    }
    if let Some(text) = label {
        run.new_entries.push(Snapshot {
            path: LABEL_PATH.to_string(),
            time: now(),
            hash: None,
            label: Some(text.to_string()),
        });
    }
    let n = run.new_entries.iter().filter(|e| !e.is_marker()).count();
    append_index(&dir, &run.new_entries)?;
    write_state(&dir, &run.state)?;
    prune_locked(&dir, &read_index(&dir), cfg.keep_days)?;
    Ok(n)
}

/// Snapshot every changed/untracked file whose content differs from its latest snapshot.
/// Called periodically on a background thread by the app; returns the number of new snapshots.
pub fn snapshot_changed(repo: &Repo, cfg: &LocalHistoryConfig) -> Result<usize> {
    snapshot_impl(repo, cfg, None, None)
}

/// "Put label": snapshot all changed files (even unchanged since their last snapshot) under
/// `label`, plus a project-wide marker. Returns the number of file snapshots taken.
pub fn snapshot_with_label(repo: &Repo, cfg: &LocalHistoryConfig, label: &str) -> Result<usize> {
    let label = label.trim();
    if label.is_empty() {
        bail!("label must not be empty");
    }
    snapshot_impl(repo, cfg, Some(label), None)
}

/// Snapshot a single file if it changed (used before restoring and by "snapshot this file").
pub fn snapshot_path(repo: &Repo, cfg: &LocalHistoryConfig, path: &str) -> Result<usize> {
    snapshot_impl(repo, cfg, None, Some(path))
}

// ----- pruning -----------------------------------------------------------------------------------

/// Drop entries older than `keep_days` and delete objects nothing references any more.
fn prune_locked(dir: &Path, index: &[Snapshot], keep_days: u64) -> Result<usize> {
    let cutoff = now() - (keep_days.min(100_000) as i64) * 86_400;
    let keep: Vec<Snapshot> = index.iter().filter(|e| e.time >= cutoff).cloned().collect();
    let removed = index.len() - keep.len();
    if removed == 0 {
        return Ok(0);
    }
    let mut text = String::new();
    for e in &keep {
        text.push_str(&serde_json::to_string(e)?);
        text.push('\n');
    }
    write_atomic(&index_file(dir), text.as_bytes())?;
    let live: HashSet<&str> = keep.iter().filter_map(|e| e.hash.as_deref()).collect();
    gc_objects(dir, &live);
    Ok(removed)
}

fn gc_objects(dir: &Path, live: &HashSet<&str>) {
    let Ok(fans) = fs::read_dir(objects_dir(dir)) else {
        return;
    };
    for fan in fans.flatten() {
        let prefix = fan.file_name().to_string_lossy().into_owned();
        let Ok(files) = fs::read_dir(fan.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = format!("{prefix}{}", f.file_name().to_string_lossy());
            if !live.contains(name.as_str()) {
                let _ = fs::remove_file(f.path());
            }
        }
        let _ = fs::remove_dir(fan.path());
    }
}

/// Remove entries older than `keep_days`; returns how many were removed.
pub fn prune(repo: &Repo, keep_days: u64) -> Result<usize> {
    let dir = store_dir(repo)?;
    let _lock = Lock::acquire(&dir)?;
    let index = read_index(&dir);
    prune_locked(&dir, &index, keep_days)
}

/// Refuse to restore over anything but a regular file (or a missing one) inside the work tree:
/// writing through a symlink would overwrite its target, possibly outside the repository, without
/// saving that content first.
fn ensure_plain_target(repo: &Repo, path: &str) -> Result<()> {
    let abs = repo.path(path);
    if let Ok(m) = fs::symlink_metadata(&abs) {
        if m.file_type().is_symlink() {
            bail!("{path} is a symbolic link; local history does not write through links");
        }
        if !m.is_file() {
            bail!("{path} is not a regular file");
        }
    }
    // The nearest existing ancestor must resolve inside the work tree (no symlinked directories).
    let root = fs::canonicalize(&repo.root)
        .with_context(|| format!("resolving {}", repo.root.display()))?;
    let mut dir = abs.parent();
    while let Some(d) = dir {
        if let Ok(real) = fs::canonicalize(d) {
            if !real.starts_with(&root) {
                bail!("{path} resolves outside the work tree");
            }
            return Ok(());
        }
        dir = d.parent();
    }
    Ok(())
}

// ----- queries -----------------------------------------------------------------------------------

/// All index entries, newest first (the "timeline").
pub fn all_snapshots(repo: &Repo) -> Result<Vec<Snapshot>> {
    let mut v = read_index(&store_dir(repo)?);
    v.reverse();
    // Stable sort keeps append order (newest first after reverse) for equal times.
    v.sort_by_key(|s| std::cmp::Reverse(s.time));
    Ok(v)
}

/// Files with local history, most recently changed first.
pub fn list_files(repo: &Repo) -> Result<Vec<HistoryFile>> {
    Ok(files_from(&read_index(&store_dir(repo)?)))
}

fn files_from(index: &[Snapshot]) -> Vec<HistoryFile> {
    let mut m: HashMap<&str, HistoryFile> = HashMap::new();
    for e in index.iter().filter(|e| !e.is_marker()) {
        let f = m.entry(e.path.as_str()).or_insert_with(|| HistoryFile {
            path: e.path.clone(),
            last_time: e.time,
            count: 0,
            deleted: false,
        });
        f.count += 1;
        if e.time >= f.last_time {
            f.last_time = e.time;
            f.deleted = e.hash.is_none();
        }
    }
    let mut v: Vec<HistoryFile> = m.into_values().collect();
    v.sort_by(|a, b| {
        b.last_time
            .cmp(&a.last_time)
            .then_with(|| a.path.cmp(&b.path))
    });
    v
}

/// Snapshots of one file, newest first.
pub fn snapshots_for(repo: &Repo, path: &str) -> Result<Vec<Snapshot>> {
    Ok(all_snapshots(repo)?
        .into_iter()
        .filter(|e| e.path == path)
        .collect())
}

/// Stored content of a snapshot.
pub fn read_object(repo: &Repo, hash: &str) -> Result<Vec<u8>> {
    let p = object_path(repo, hash)?;
    fs::read(&p).with_context(|| format!("reading local history object {}", p.display()))
}

/// Revert `path` to a snapshot: `Some(hash)` writes that content, `None` deletes the file.
/// The current content is snapshotted first, so the revert itself can be undone.
pub fn restore(
    repo: &Repo,
    cfg: &LocalHistoryConfig,
    path: &str,
    hash: Option<&str>,
) -> Result<()> {
    let data = hash.map(|h| read_object(repo, h)).transpose()?;
    ensure_plain_target(repo, path)?;
    // Save the current content whatever its size: losing it would make the revert irreversible.
    let bigger = cfg.max_file_kb.max(
        fs::metadata(repo.path(path))
            .map(|m| m.len() / 1024 + 1)
            .unwrap_or(0),
    );
    let cfg = LocalHistoryConfig {
        max_file_kb: bigger,
        ..cfg.clone()
    };
    snapshot_path(repo, &cfg, path).context("saving current content before revert")?;
    let abs = repo.path(path);
    match data {
        // Plain write (not rename) keeps the file's permissions, e.g. the executable bit.
        Some(bytes) => {
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&abs, &bytes).with_context(|| format!("writing {}", abs.display()))
        }
        None => match fs::remove_file(&abs) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("deleting {}", abs.display()))
            }
            _ => Ok(()),
        },
    }?;
    // Record the reverted state right away so the history shows it.
    snapshot_path(repo, &cfg, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn cfg() -> LocalHistoryConfig {
        LocalHistoryConfig {
            enabled: true,
            interval_secs: 60,
            keep_days: 14,
            max_file_kb: 1,
        }
    }

    fn content(repo: &Repo, s: &Snapshot) -> String {
        String::from_utf8(read_object(repo, s.hash.as_deref().unwrap()).unwrap())
            .unwrap()
            .replace("\r\n", "\n")
    }

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
        assert_ne!(content_hash(b"x"), content_hash(b"y"));
    }

    #[test]
    fn snapshot_dedupe_and_skip_unchanged() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "one\n")], "init");
        // Nothing changed yet.
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
        write(&repo, "a.txt", "two\n");
        write(&repo, "dir/new file.txt", "new\n");
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 2);
        // Second run: unchanged (cheap skip).
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
        // Touch with identical content: no new snapshot (content dedupe).
        write(&repo, "a.txt", "two\n");
        std::fs::remove_file(
            repo.data_dir()
                .unwrap()
                .join("local-history")
                .join("state.json"),
        )
        .unwrap();
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
        write(&repo, "a.txt", "three\n");
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 1);
        let snaps = snapshots_for(&repo, "a.txt").unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(content(&repo, &snaps[0]), "three\n");
        assert_eq!(content(&repo, &snaps[1]), "two\n");
        let files = list_files(&repo).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .any(|f| f.path == "dir/new file.txt" && f.count == 1));
    }

    #[test]
    fn identical_content_shares_object() {
        let (_d, repo) = temp_repo();
        write(&repo, "x.txt", "same\n");
        write(&repo, "y.txt", "same\n");
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 2);
        let all = all_snapshots(&repo).unwrap();
        assert_eq!(all[0].hash, all[1].hash);
        let dir = store_dir(&repo).unwrap();
        let count: usize = fs::read_dir(objects_dir(&dir))
            .unwrap()
            .flatten()
            .map(|d| fs::read_dir(d.path()).unwrap().count())
            .sum();
        assert_eq!(count, 1);
    }

    #[test]
    fn large_files_are_skipped() {
        let (_d, repo) = temp_repo();
        write(&repo, "big.bin", &"x".repeat(4096));
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
    }

    #[test]
    fn records_deletion_of_untracked_file() {
        let (_d, repo) = temp_repo();
        write(&repo, "tmp.txt", "draft\n");
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 1);
        fs::remove_file(repo.path("tmp.txt")).unwrap();
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 1);
        let snaps = snapshots_for(&repo, "tmp.txt").unwrap();
        assert!(snaps[0].is_deletion());
        assert!(list_files(&repo).unwrap()[0].deleted);
        // Deletion is recorded only once.
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
    }

    #[test]
    fn labels_and_timeline() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        assert_eq!(
            snapshot_with_label(&repo, &cfg(), "before refactor").unwrap(),
            1
        );
        assert!(snapshot_with_label(&repo, &cfg(), "  ").is_err());
        let all = all_snapshots(&repo).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all
            .iter()
            .any(|s| s.is_marker() && s.label.as_deref() == Some("before refactor")));
        let snaps = snapshots_for(&repo, "a.txt").unwrap();
        assert_eq!(snaps[0].label.as_deref(), Some("before refactor"));
        assert_eq!(snaps[0].hash, snaps[1].hash);
    }

    #[test]
    fn prune_removes_old_entries_and_objects() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "old\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        let dir = store_dir(&repo).unwrap();
        // Age the existing entry by 30 days.
        let mut idx = read_index(&dir);
        idx[0].time -= 30 * 86_400;
        let old_hash = idx[0].hash.clone().unwrap();
        let text: String = idx
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        fs::write(index_file(&dir), text).unwrap();
        write(&repo, "a.txt", "new\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        let snaps = snapshots_for(&repo, "a.txt").unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(content(&repo, &snaps[0]), "new\n");
        assert!(!object_path(&repo, &old_hash).unwrap().exists());
        assert_eq!(prune(&repo, 14).unwrap(), 0);
    }

    #[test]
    fn restore_is_undoable() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "base\n")], "init");
        write(&repo, "a.txt", "v1\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        let v1 = snapshots_for(&repo, "a.txt").unwrap()[0].clone();
        write(&repo, "a.txt", "v2 unsaved\n");
        restore(&repo, &cfg(), "a.txt", v1.hash.as_deref()).unwrap();
        let now = fs::read_to_string(repo.path("a.txt"))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(now, "v1\n");
        let snaps = snapshots_for(&repo, "a.txt").unwrap();
        assert!(snaps.iter().any(|s| content(&repo, s) == "v2 unsaved\n"));
        // Restoring a deletion removes the file (after saving it).
        restore(&repo, &cfg(), "a.txt", None).unwrap();
        assert!(!repo.path("a.txt").exists());
        assert!(snapshots_for(&repo, "a.txt").unwrap()[0].is_deletion());
    }

    #[test]
    fn same_size_edit_in_same_mtime_tick_is_recorded() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "aaa\n");
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 1);
        let mtime = fs::metadata(repo.path("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        // Same size, and the filesystem reports the very same mtime (coarse timestamp).
        write(&repo, "a.txt", "bbb\n");
        fs::File::options()
            .write(true)
            .open(repo.path("a.txt"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 1);
        assert_eq!(
            content(&repo, &snapshots_for(&repo, "a.txt").unwrap()[0]),
            "bbb\n"
        );
        // Once the stamp is old enough it is trusted again (cheap skip without reading).
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(repo.path("a.txt"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(snapshot_changed(&repo, &cfg()).unwrap(), 0);
        let dir = store_dir(&repo).unwrap();
        assert!(read_state(&dir).contains_key("a.txt"));
    }

    #[test]
    fn label_only_tags_currently_changed_files() {
        let (_d, repo) = temp_repo();
        commit_files(
            &repo,
            &[("kept.txt", "base\n"), ("other.txt", "x\n")],
            "init",
        );
        // kept.txt gets history, then goes back to its committed content.
        write(&repo, "kept.txt", "edit\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        write(&repo, "kept.txt", "base\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        write(&repo, "other.txt", "changed\n");
        assert_eq!(snapshot_with_label(&repo, &cfg(), "checkpoint").unwrap(), 1);
        let kept = snapshots_for(&repo, "kept.txt").unwrap();
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|s| s.label.is_none()));
        let other = snapshots_for(&repo, "other.txt").unwrap();
        assert_eq!(other[0].label.as_deref(), Some("checkpoint"));
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_to_write_through_symlinks() {
        let (_d, repo) = temp_repo();
        let outside = tempdir_outside();
        let target = outside.join("secret.txt");
        fs::write(&target, "outside\n").unwrap();
        write(&repo, "a.txt", "v1\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        let v1 = snapshots_for(&repo, "a.txt").unwrap()[0].clone();
        fs::remove_file(repo.path("a.txt")).unwrap();
        std::os::unix::fs::symlink(&target, repo.path("a.txt")).unwrap();
        assert!(restore(&repo, &cfg(), "a.txt", v1.hash.as_deref()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "outside\n");
        // A symlinked parent directory pointing outside the work tree is refused too.
        fs::remove_file(repo.path("a.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, repo.path("linked")).unwrap();
        assert!(restore(&repo, &cfg(), "linked/secret.txt", v1.hash.as_deref()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "outside\n");
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    fn tempdir_outside() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "gitjet-lh-outside-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn malformed_index_lines_are_ignored_and_lock_is_released() {
        let (_d, repo) = temp_repo();
        let dir = store_dir(&repo).unwrap();
        fs::write(
            index_file(&dir),
            "garbage\n{\"path\":\"a\",\"time\":1,\"hash\":null}\n",
        )
        .unwrap();
        assert_eq!(read_index(&dir).len(), 1);
        write(&repo, "b.txt", "b\n");
        snapshot_changed(&repo, &cfg()).unwrap();
        assert!(!dir.join("lock").exists());
        assert!(object_path(&repo, "../../etc").is_err());
    }

    #[test]
    fn concurrent_snapshots_do_not_corrupt() {
        let (_d, repo) = temp_repo();
        for i in 0..10 {
            write(&repo, &format!("f{i}.txt"), &format!("{i}\n"));
        }
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let r = repo.clone();
                std::thread::spawn(move || snapshot_changed(&r, &cfg()).unwrap())
            })
            .collect();
        let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, 10);
        assert_eq!(all_snapshots(&repo).unwrap().len(), 10);
    }
}
