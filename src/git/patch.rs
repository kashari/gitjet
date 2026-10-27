//! Create patch files from changes or commits, apply patch files (`git apply` / `git am`).
//!
//! Patches of local changes are produced with a *temporary index*: it starts from `HEAD` (or the
//! empty tree), the selected work-tree paths are added to it, and `git diff --cached --binary`
//! then yields one complete, binary-safe patch that includes untracked, deleted and renamed files
//! without touching the user's real index and without long command lines.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};

use super::diff::{parse_unified, ChangeKind, DiffLine, FileDiff, Hunk, LineKind};
use super::Repo;

/// Hash of git's empty tree (base for diffs in a repository without commits).
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Flags that make diff output stable regardless of user configuration.
const DIFF_FLAGS: [&str; 5] = [
    "--no-color",
    "--no-ext-diff",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--binary",
];

/// Untracked/shelved files larger than this are not rendered line by line in previews.
const PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// A NUL-separated pathspec list for `--pathspec-from-file=- --pathspec-file-nul`.
pub fn nul_paths<S: AsRef<str>>(paths: &[S]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in paths {
        v.extend_from_slice(p.as_ref().as_bytes());
        v.push(0);
    }
    v
}

/// Run git with extra environment variables and optional stdin; non-zero exit is an error.
pub(crate) fn run_env(
    repo: &Repo,
    env: &[(&str, &Path)],
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<String> {
    let mut c = repo.command();
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    if input.is_some() {
        c.stdin(Stdio::piped());
    }
    let mut child = c
        .spawn()
        .map_err(|e| anyhow!("could not run `git {}`: {e}", args.join(" ")))?;
    let writer = match (input, child.stdin.take()) {
        (Some(data), Some(mut stdin)) => {
            let data = data.to_vec();
            Some(std::thread::spawn(move || stdin.write_all(&data)))
        }
        _ => None,
    };
    let out = child.wait_with_output()?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        let shown: Vec<&str> = args.iter().take(4).copied().collect();
        bail!("git {} failed: {}", shown.join(" "), msg);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A throw-away index file inside the git dir, removed on drop.
pub(crate) struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    /// A temporary index containing the tree of `HEAD` (or nothing in an empty repository).
    pub fn from_head(repo: &Repo) -> Result<TempIndex> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = repo
            .git_dir
            .join(format!("gitjet-index-{}-{nanos}", std::process::id()));
        let idx = TempIndex { path };
        if repo.head_commit().is_some() {
            idx.run(repo, &["read-tree", "HEAD"], None)?;
        } else {
            idx.run(repo, &["read-tree", "--empty"], None)?;
        }
        Ok(idx)
    }

    /// Run git against this index.
    pub fn run(&self, repo: &Repo, args: &[&str], input: Option<&[u8]>) -> Result<String> {
        run_env(repo, &[("GIT_INDEX_FILE", &self.path)], args, input)
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let mut lock = self.path.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(PathBuf::from(lock));
    }
}

/// The base to diff against: `HEAD`, or the empty tree in a repository without commits.
fn base_rev(repo: &Repo) -> String {
    if repo.head_commit().is_some() {
        "HEAD".to_string()
    } else {
        EMPTY_TREE.to_string()
    }
}

/// Whether a patch records renames as such or as a deletion plus an addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renames {
    /// Detect renames (`-M`): compact, readable patches for sharing
    Detect,
    /// Never detect renames: every path gets its own section, so sections can be applied one by one
    Off,
}

/// Binary-safe patch of the work-tree state of `paths` against `HEAD` (tracked, untracked, deleted
/// files alike). Paths are literal. Paths that exist neither in the work tree nor in `HEAD` (e.g. a
/// file staged as new and then deleted from disk) have no diff and are skipped.
pub fn paths_patch(repo: &Repo, paths: &[String], renames: Renames) -> Result<String> {
    let paths = with_diff_against_head(repo, paths)?;
    if paths.is_empty() {
        return Ok(String::new());
    }
    let idx = TempIndex::from_head(repo)?;
    idx.run(
        repo,
        &[
            "--literal-pathspecs",
            "add",
            "-A",
            "-f",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        Some(&nul_paths(&paths)),
    )?;
    let base = base_rev(repo);
    let rename_flag = match renames {
        Renames::Detect => "-M",
        Renames::Off => "--no-renames",
    };
    let mut args = vec!["diff", "--cached", rename_flag];
    args.extend(DIFF_FLAGS);
    args.push(&base);
    idx.run(repo, &args, None)
}

/// The paths that are in the work tree or in `HEAD` (only those can differ from `HEAD`).
fn with_diff_against_head(repo: &Repo, paths: &[String]) -> Result<Vec<String>> {
    let (in_head, _) = split_by_head(repo, paths)?;
    let in_head: HashSet<&String> = in_head.iter().collect();
    Ok(paths
        .iter()
        .filter(|p| in_head.contains(p) || on_disk(repo, p))
        .cloned()
        .collect())
}

/// True if something (file, directory or dangling symlink) exists at the repo path `rel`.
fn on_disk(repo: &Repo, rel: &str) -> bool {
    std::fs::symlink_metadata(repo.path(rel)).is_ok()
}

/// Partition paths into (present in `HEAD`, absent from `HEAD`), keeping their order.
pub(crate) fn split_by_head(repo: &Repo, paths: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    if paths.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if repo.head_commit().is_none() {
        return Ok((Vec::new(), paths.to_vec()));
    }
    let mut input = String::new();
    for p in paths {
        if p.contains('\n') {
            bail!("unsupported file name: {p:?}");
        }
        input.push_str("HEAD:");
        input.push_str(p);
        input.push('\n');
    }
    let out = repo.run_stdin(
        &["cat-file", "--batch-check=%(objecttype)"],
        input.as_bytes(),
    )?;
    let answers: Vec<&str> = out.lines().collect();
    if answers.len() != paths.len() {
        bail!(
            "git cat-file returned {} answers for {} paths",
            answers.len(),
            paths.len()
        );
    }
    let mut yes = Vec::new();
    let mut no = Vec::new();
    for (p, a) in paths.iter().zip(answers) {
        if a.ends_with("missing") {
            no.push(p.clone());
        } else {
            yes.push(p.clone());
        }
    }
    Ok((yes, no))
}

/// Every locally changed path (staged, unstaged and untracked, ignored files excluded; rename
/// sources included). Fails on unresolved conflicts or when there are no changes.
pub fn local_change_paths(repo: &Repo) -> Result<Vec<String>> {
    let st = repo.status()?;
    let mut paths: Vec<String> = Vec::new();
    for e in st.entries.iter().filter(|e| !e.is_ignored()) {
        if e.is_conflicted() {
            bail!(
                "{} has unresolved conflicts; resolve them before creating a patch",
                e.path
            );
        }
        paths.push(e.path.clone());
        if let Some(o) = &e.orig_path {
            paths.push(o.clone());
        }
    }
    if paths.is_empty() {
        bail!("There are no local changes");
    }
    Ok(paths)
}

/// Every local change (staged, unstaged and untracked, ignored files excluded) as one patch.
pub fn local_changes_patch(repo: &Repo) -> Result<String> {
    let text = paths_patch(repo, &local_change_paths(repo)?, Renames::Detect)?;
    if text.trim().is_empty() {
        bail!("There are no local changes");
    }
    Ok(text)
}

/// `git format-patch` output (mail format, binary-safe) for a single commit.
pub fn commit_patch(repo: &Repo, rev: &str) -> Result<String> {
    let hash = repo.rev_parse(rev)?;
    // A root commit has no first parent (cheap check, unlike walking the history).
    let root = repo
        .try_run(&["rev-parse", "--verify", "-q", &format!("{hash}^")])
        .is_none();
    let mut args = vec![
        "format-patch",
        "-1",
        "--stdout",
        "--binary",
        "--no-color",
        "--no-signature",
    ];
    if root {
        args.push("--root");
    }
    args.push(&hash);
    repo.run(&args)
}

/// Patch with "new file" diffs for files given as (repo path, file on disk), e.g. shelved copies.
pub fn new_files_patch(repo: &Repo, files: &[(String, PathBuf)]) -> Result<String> {
    if files.is_empty() {
        return Ok(String::new());
    }
    let idx = TempIndex::from_head(repo)?;
    idx.run(repo, &["read-tree", "--empty"], None)?;
    let mut list = String::new();
    for (_, disk) in files {
        let s = disk.to_string_lossy();
        if s.contains('\n') {
            bail!("unsupported file name: {s}");
        }
        list.push_str(&s);
        list.push('\n');
    }
    let hashes = repo.run_stdin(
        &["hash-object", "-w", "--no-filters", "--stdin-paths"],
        list.as_bytes(),
    )?;
    let hashes: Vec<&str> = hashes.lines().collect();
    if hashes.len() != files.len() {
        bail!(
            "git hash-object returned {} hashes for {} files",
            hashes.len(),
            files.len()
        );
    }
    let mut info = Vec::new();
    for ((path, disk), hash) in files.iter().zip(hashes) {
        let mode = if is_executable(disk) {
            "100755"
        } else {
            "100644"
        };
        info.extend_from_slice(format!("{mode} {hash}\t{path}").as_bytes());
        info.push(0);
    }
    idx.run(repo, &["update-index", "-z", "--index-info"], Some(&info))?;
    let mut args = vec!["diff", "--cached"];
    args.extend(DIFF_FLAGS);
    args.push(EMPTY_TREE);
    idx.run(repo, &args, None)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_p: &Path) -> bool {
    false
}

/// One `diff --git` section of a patch: its raw text and the paths it touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchChunk {
    pub text: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
}

impl PatchChunk {
    /// True if this chunk changes `path` (on either side).
    pub fn touches(&self, path: &str) -> bool {
        self.old_path.as_deref() == Some(path) || self.new_path.as_deref() == Some(path)
    }
}

/// Split a patch into per-file sections (anything before the first `diff --git` is dropped).
pub fn split_patch(text: &str) -> Vec<PatchChunk> {
    let mut starts: Vec<usize> = Vec::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(pos);
        }
        pos += line.len();
    }
    let mut chunks = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(text.len());
        let chunk = &text[s..e];
        let parsed = parse_unified(chunk);
        let (old_path, new_path) = parsed
            .first()
            .map(|f| (f.old_path.clone(), f.new_path.clone()))
            .unwrap_or((None, None));
        chunks.push(PatchChunk {
            text: chunk.to_string(),
            old_path,
            new_path,
        });
    }
    chunks
}

/// The sections of `text` touching any of `paths`, joined into a new patch.
pub fn filter_patch(text: &str, paths: &[String]) -> String {
    split_patch(text)
        .into_iter()
        .filter(|c| paths.iter().any(|p| c.touches(p)))
        .map(|c| c.text)
        .collect()
}

/// Every path a patch touches (old and new sides), deduplicated in order.
pub fn patch_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in split_patch(text) {
        for p in [c.old_path, c.new_path].into_iter().flatten() {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// A synthetic "all lines added" diff for file contents (shelved untracked copies).
pub fn synthetic_added_diff(path: &str, bytes: &[u8]) -> FileDiff {
    let binary = bytes.iter().take(8000).any(|b| *b == 0);
    let mut fd = FileDiff {
        old_path: None,
        new_path: Some(path.to_string()),
        kind: ChangeKind::Added,
        binary,
        old_mode: None,
        new_mode: None,
        header: vec!["new file (untracked)".to_string()],
        hunks: Vec::new(),
    };
    if binary {
        return fd;
    }
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<DiffLine> = text
        .lines()
        .enumerate()
        .map(|(i, l)| DiffLine {
            kind: LineKind::Added,
            content: l.strip_suffix('\r').unwrap_or(l).to_string(),
            old_no: None,
            new_no: Some(i as u32 + 1),
        })
        .collect();
    if !lines.is_empty() {
        fd.hunks.push(Hunk {
            header: format!("@@ -0,0 +1,{} @@", lines.len()),
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: lines.len() as u32,
            lines,
        });
    }
    fd
}

/// Preview of a rename stored as a deletion (`deleted`, of `orig`) plus an addition (`added`, of
/// `path`): one "renamed" diff with the line changes between the two contents.
pub fn rename_preview(orig: &str, path: &str, deleted: &FileDiff, added: &FileDiff) -> FileDiff {
    let mut fd = FileDiff {
        old_path: Some(orig.to_string()),
        new_path: Some(path.to_string()),
        kind: ChangeKind::Renamed,
        binary: deleted.binary || added.binary,
        old_mode: deleted.old_mode.clone(),
        new_mode: added.new_mode.clone(),
        header: vec![format!("rename from {orig}"), format!("rename to {path}")],
        hunks: Vec::new(),
    };
    if fd.binary {
        return fd;
    }
    let old = lines_of(deleted, LineKind::Removed);
    let new = lines_of(added, LineKind::Added);
    let diff = similar::TextDiff::from_slices(&old, &new);
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_start = first.old_range().start;
        let new_start = first.new_range().start;
        let old_len = last.old_range().end - old_start;
        let new_len = last.new_range().end - new_start;
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    similar::ChangeTag::Equal => LineKind::Context,
                    similar::ChangeTag::Delete => LineKind::Removed,
                    similar::ChangeTag::Insert => LineKind::Added,
                };
                lines.push(DiffLine {
                    kind,
                    content: change.value().to_string(),
                    old_no: change.old_index().map(|i| i as u32 + 1),
                    new_no: change.new_index().map(|i| i as u32 + 1),
                });
            }
        }
        // Unified-diff convention: an empty range starts at the line before it.
        let start = |s: usize, len: usize| if len == 0 { s } else { s + 1 };
        fd.hunks.push(Hunk {
            header: format!(
                "@@ -{},{old_len} +{},{new_len} @@",
                start(old_start, old_len),
                start(new_start, new_len)
            ),
            old_start: start(old_start, old_len) as u32,
            old_lines: old_len as u32,
            new_start: start(new_start, new_len) as u32,
            new_lines: new_len as u32,
            lines,
        });
    }
    fd
}

/// The contents of every line of `kind` in `d`.
fn lines_of(d: &FileDiff, kind: LineKind) -> Vec<&str> {
    d.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind)
        .map(|l| l.content.as_str())
        .collect()
}

/// Preview diff for a file on disk shown as newly added; very large files are summarized.
pub fn file_as_added_diff(path: &str, disk: &Path) -> Result<FileDiff> {
    let len = std::fs::metadata(disk)
        .with_context(|| format!("reading {}", disk.display()))?
        .len();
    if len > PREVIEW_MAX_BYTES {
        let mut fd = synthetic_added_diff(path, &[]);
        fd.binary = true;
        fd.header
            .push(format!("large file ({} KB) not shown", len / 1024));
        return Ok(fd);
    }
    let bytes = std::fs::read(disk).with_context(|| format!("reading {}", disk.display()))?;
    Ok(synthetic_added_diff(path, &bytes))
}

/// What applying a patch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Applied without conflicts
    Clean,
    /// Applied with a 3-way merge that left conflicts in these files
    Conflicts(Vec<String>),
}

/// Apply a plain diff to the work tree. Tries a plain `git apply` first (atomic, keeps the index
/// untouched); when that fails, falls back to `git apply --3way`, which merges against the
/// recorded base blobs and may leave conflict markers. After a clean 3-way merge the affected
/// paths are unstaged again so the result looks like ordinary local changes.
pub fn apply_to_worktree(repo: &Repo, patch: &str) -> Result<ApplyOutcome> {
    if patch.trim().is_empty() {
        return Ok(ApplyOutcome::Clean);
    }
    let file = write_temp_patch(repo, patch)?;
    let path = file.path.to_string_lossy().into_owned();
    let plain = repo.run(&["apply", "--whitespace=nowarn", &path]);
    let plain_err = match plain {
        Ok(_) => return Ok(ApplyOutcome::Clean),
        Err(e) => e,
    };
    match repo.run(&["apply", "--3way", "--whitespace=nowarn", &path]) {
        Ok(_) => {
            unstage(repo, &patch_paths(patch))?;
            Ok(ApplyOutcome::Clean)
        }
        Err(three_way_err) => {
            let touched = patch_paths(patch);
            let conflicts: Vec<String> = repo
                .status()?
                .conflicted()
                .map(|e| e.path.clone())
                .filter(|p| touched.contains(p))
                .collect();
            if conflicts.is_empty() {
                Err(anyhow!("{plain_err:#}\n\n3-way merge: {three_way_err:#}"))
            } else {
                Ok(ApplyOutcome::Conflicts(conflicts))
            }
        }
    }
}

/// Reset the index entries of `paths` to `HEAD`, keeping new files visible via intent-to-add.
fn unstage(repo: &Repo, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let list = nul_paths(paths);
    if repo.head_commit().is_some() {
        repo.run_stdin(
            &[
                "--literal-pathspecs",
                "reset",
                "-q",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &list,
        )?;
    } else {
        repo.run_stdin(
            &[
                "--literal-pathspecs",
                "rm",
                "-q",
                "-r",
                "--cached",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &list,
        )?;
    }
    let existing: Vec<&String> = paths.iter().filter(|p| repo.path(p).is_file()).collect();
    let untracked: Vec<String> = {
        let st = repo.status()?;
        existing
            .into_iter()
            .filter(|p| st.entries.iter().any(|e| &e.path == *p && e.is_untracked()))
            .cloned()
            .collect()
    };
    if !untracked.is_empty() {
        repo.run_stdin(
            &[
                "--literal-pathspecs",
                "add",
                "-N",
                "-f",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            &nul_paths(&untracked),
        )?;
    }
    Ok(())
}

/// A temporary patch file in the git dir, removed on drop.
struct TempFile {
    path: PathBuf,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn write_temp_patch(repo: &Repo, text: &str) -> Result<TempFile> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = repo
        .git_dir
        .join(format!("gitjet-patch-{}-{nanos}.patch", std::process::id()));
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(TempFile { path })
}

/// Kind of patch file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchFormat {
    /// `git format-patch` / mbox: applied with `git am` (creates commits)
    Mailbox,
    /// Plain unified diff: applied to the work tree
    Diff,
}

/// Detect mbox/format-patch files by their mail header before the first diff.
pub fn detect_format(text: &str) -> PatchFormat {
    let head: Vec<&str> = text
        .lines()
        .take_while(|l| !l.starts_with("diff --git ") && !l.starts_with("--- "))
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    let first = head
        .iter()
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("");
    let has_header = |name: &str| head.iter().any(|l| l.starts_with(name));
    let mbox_line = first.starts_with("From ") && first.len() > 5;
    if (mbox_line || first.starts_with("From: ")) && has_header("Subject: ") {
        PatchFormat::Mailbox
    } else {
        PatchFormat::Diff
    }
}

/// Subjects of the mails in an mbox patch (for previews).
pub fn mailbox_subjects(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Subject: "))
        .map(|s| s.trim_end_matches('\r').to_string())
        .collect()
}

/// What applying a patch file did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchFileOutcome {
    /// Diff applied to the work tree
    Applied(ApplyOutcome),
    /// `git am` created this many commits
    Committed(usize),
    /// `git am` stopped (conflicts or a patch that does not apply); the repository is mid-`am`
    AmStopped {
        conflicts: Vec<String>,
        message: String,
    },
}

/// Apply a patch file: mailboxes via `git am --3way`, plain diffs to the work tree.
pub fn apply_patch_file(repo: &Repo, file: &Path) -> Result<PatchFileOutcome> {
    let text = read_patch(file)?;
    match detect_format(&text) {
        PatchFormat::Diff => Ok(PatchFileOutcome::Applied(apply_to_worktree(repo, &text)?)),
        PatchFormat::Mailbox => {
            let before = repo.head_commit();
            let path = file.to_string_lossy().into_owned();
            match repo.run(&["am", "--3way", "--keep-cr", &path]) {
                Ok(_) => Ok(PatchFileOutcome::Committed(count_new_commits(
                    repo,
                    before.as_deref(),
                ))),
                Err(e) => {
                    if repo.state() != super::RepoState::ApplyingMailbox {
                        return Err(e);
                    }
                    let conflicts = repo
                        .status()?
                        .conflicted()
                        .map(|e| e.path.clone())
                        .collect();
                    Ok(PatchFileOutcome::AmStopped {
                        conflicts,
                        message: format!("{e:#}"),
                    })
                }
            }
        }
    }
}

fn count_new_commits(repo: &Repo, before: Option<&str>) -> usize {
    let range = match before {
        Some(b) => format!("{b}..HEAD"),
        None => "HEAD".to_string(),
    };
    repo.try_run(&["rev-list", "--count", &range])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Abort an in-progress `git am`, restoring the original branch.
pub fn am_abort(repo: &Repo) -> Result<()> {
    repo.run(&["am", "--abort"]).map(|_| ())
}

/// Continue an in-progress `git am` after resolving conflicts (resolved files must be staged).
pub fn am_continue(repo: &Repo) -> Result<()> {
    repo.run(&["am", "--continue"]).map(|_| ())
}

/// Skip the current patch of an in-progress `git am`.
pub fn am_skip(repo: &Repo) -> Result<()> {
    repo.run(&["am", "--skip"]).map(|_| ())
}

/// Read a patch file (lossy UTF-8; binary patches are base85 so stay intact).
pub fn read_patch(file: &Path) -> Result<String> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Write a patch file, creating parent directories.
pub fn write_patch(file: &Path, text: &str) -> Result<()> {
    if let Some(parent) = file.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(file, text).with_context(|| format!("writing {}", file.display()))
}

/// Resolve a path typed by the user: surrounding quotes removed, `~` expanded, relative paths
/// taken from the repository root.
pub fn resolve_user_path(repo: &Repo, input: &str) -> PathBuf {
    let mut s = input.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            s = &s[1..s.len() - 1];
        }
    }
    let home_rel = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"));
    if let (Some(rest), Some(home)) = (home_rel, dirs::home_dir()) {
        return home.join(rest);
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    let p = PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        repo.root.join(p)
    }
}

/// A file-name-safe slug of `s` (for default patch file names).
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.chars().count() >= 50 {
            break;
        }
    }
    let t = out.trim_end_matches('-').to_string();
    if t.is_empty() {
        "changes".to_string()
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn norm(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    fn read(repo: &Repo, p: &str) -> String {
        norm(&std::fs::read_to_string(repo.path(p)).unwrap())
    }

    #[test]
    fn split_and_filter() {
        let text = "From abc Mon\nSubject: x\n\ndiff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-1\n+2\n\
diff --git a/old b/new\nsimilarity index 100%\nrename from old\nrename to new\n\
diff --git a/c d.txt b/c d.txt\nnew file mode 100644\n--- /dev/null\n+++ b/c d.txt\n@@ -0,0 +1 @@\n+x\n";
        let chunks = split_patch(text);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].touches("a.txt"));
        assert!(chunks[1].touches("old") && chunks[1].touches("new"));
        assert_eq!(chunks[2].new_path.as_deref(), Some("c d.txt"));
        assert_eq!(chunks[2].old_path, None);
        let f = filter_patch(text, &["c d.txt".to_string(), "new".to_string()]);
        assert!(f.starts_with("diff --git a/old b/new"));
        assert!(f.contains("+x\n"));
        assert!(!f.contains("a/a.txt"));
        assert_eq!(patch_paths(text), vec!["a.txt", "old", "new", "c d.txt"]);
        assert!(split_patch("no diff here").is_empty());
    }

    #[test]
    fn formats() {
        assert_eq!(
            detect_format("From 1234567890abcdef Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] x\n\ndiff --git a/x b/x\n"),
            PatchFormat::Mailbox
        );
        assert_eq!(
            detect_format("From: A <a@b>\r\nSubject: hi\r\n\r\n---\r\n"),
            PatchFormat::Mailbox
        );
        assert_eq!(
            detect_format("diff --git a/x b/x\n--- a/x\n"),
            PatchFormat::Diff
        );
        assert_eq!(
            detect_format("Index: x\n--- a/x\n+++ b/x\n"),
            PatchFormat::Diff
        );
        assert_eq!(
            mailbox_subjects("Subject: [PATCH] one\r\nSubject: two\n"),
            vec!["[PATCH] one", "two"]
        );
    }

    #[test]
    fn synthetic_diffs() {
        let d = synthetic_added_diff("a.txt", b"one\r\ntwo\n");
        assert_eq!(d.added_count(), 2);
        assert_eq!(d.hunks[0].lines[0].content, "one");
        let b = synthetic_added_diff("b.bin", &[0, 1, 2]);
        assert!(b.binary && b.hunks.is_empty());
        assert!(synthetic_added_diff("e", b"").hunks.is_empty());
    }

    #[test]
    fn slugs_and_paths() {
        assert_eq!(slug("Fix: the Bug!"), "fix-the-bug");
        assert_eq!(slug("***"), "changes");
        let (_d, repo) = temp_repo();
        assert_eq!(
            resolve_user_path(&repo, " \"x y.patch\" "),
            repo.root.join("x y.patch")
        );
        let abs = repo.root.join("abs.patch");
        assert_eq!(resolve_user_path(&repo, &abs.to_string_lossy()), abs);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolve_user_path(&repo, "~/p.patch"), home.join("p.patch"));
        }
    }

    #[test]
    fn local_patch_roundtrip_with_untracked_binary_and_deleted() {
        let (_d, repo) = temp_repo();
        commit_files(
            &repo,
            &[
                ("a.txt", "1\n2\n"),
                ("gone.txt", "bye\n"),
                ("dir/b c.txt", "x\n"),
            ],
            "init",
        );
        write(&repo, "a.txt", "1\nTWO\n");
        std::fs::remove_file(repo.path("gone.txt")).unwrap();
        write(&repo, "new dir/n.txt", "new\n");
        std::fs::write(repo.path("bin.dat"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        write(&repo, "dir/b c.txt", "y\n");
        repo.run(&["add", "dir/b c.txt"]).unwrap();
        let patch = local_changes_patch(&repo).unwrap();
        // The real index is untouched (only the staged file is staged).
        let staged = repo.run(&["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged.trim(), "dir/b c.txt");
        assert!(patch.contains("GIT binary patch"));
        let paths = patch_paths(&patch);
        for p in [
            "a.txt",
            "gone.txt",
            "new dir/n.txt",
            "bin.dat",
            "dir/b c.txt",
        ] {
            assert!(paths.contains(&p.to_string()), "{p} missing in {paths:?}");
        }
        // Reset everything and re-apply the patch.
        repo.run(&["reset", "-q", "--hard"]).unwrap();
        repo.run(&["clean", "-fdq"]).unwrap();
        assert_eq!(
            apply_to_worktree(&repo, &patch).unwrap(),
            ApplyOutcome::Clean
        );
        assert_eq!(read(&repo, "a.txt"), "1\nTWO\n");
        assert!(!repo.path("gone.txt").exists());
        assert_eq!(read(&repo, "new dir/n.txt"), "new\n");
        assert_eq!(
            std::fs::read(repo.path("bin.dat")).unwrap(),
            vec![0u8, 159, 146, 150, 0, 1]
        );
        assert_eq!(read(&repo, "dir/b c.txt"), "y\n");
        // Plain apply leaves the index alone.
        assert!(repo
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .trim()
            .is_empty());
    }

    #[test]
    fn staged_then_deleted_file_is_skipped() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "ad.txt", "gone soon\n");
        repo.run(&["add", "ad.txt"]).unwrap();
        std::fs::remove_file(repo.path("ad.txt")).unwrap();
        // Only the staged-then-deleted file: it has no diff against HEAD.
        assert!(local_changes_patch(&repo).is_err());
        assert!(paths_patch(&repo, &["ad.txt".into()], Renames::Off)
            .unwrap()
            .is_empty());
        // Together with real changes the patch is created and leaves it out.
        write(&repo, "a.txt", "2\n");
        write(&repo, "u.txt", "u\n");
        let p = local_changes_patch(&repo).unwrap();
        assert_eq!(patch_paths(&p), vec!["a.txt", "u.txt"]);
    }

    #[test]
    fn staged_then_deleted_file_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "ad.txt", "x\n");
        repo.run(&["add", "ad.txt"]).unwrap();
        std::fs::remove_file(repo.path("ad.txt")).unwrap();
        write(&repo, "b.txt", "b\n");
        let p = local_changes_patch(&repo).unwrap();
        assert_eq!(patch_paths(&p), vec!["b.txt"]);
    }

    #[test]
    fn rename_detection_modes() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("old.txt", "1\n2\n3\n4\n5\n")], "init");
        repo.run(&["mv", "old.txt", "new.txt"]).unwrap();
        let paths = vec!["new.txt".to_string(), "old.txt".to_string()];
        let detected = split_patch(&paths_patch(&repo, &paths, Renames::Detect).unwrap());
        assert_eq!(detected.len(), 1);
        assert!(detected[0].touches("old.txt") && detected[0].touches("new.txt"));
        // Even with diff.renames enabled in the user's config, Off yields two sections.
        repo.run(&["config", "diff.renames", "copies"]).unwrap();
        let off = split_patch(&paths_patch(&repo, &paths, Renames::Off).unwrap());
        assert_eq!(off.len(), 2);
        assert!(off
            .iter()
            .all(|c| !(c.touches("old.txt") && c.touches("new.txt"))));
    }

    #[test]
    fn rename_preview_diffs_contents() {
        let del = parse_unified(
            "diff --git a/o.txt b/o.txt\ndeleted file mode 100644\n--- a/o.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-a\n-b\n-c\n",
        );
        let add = parse_unified(
            "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,3 @@\n+a\n+B\n+c\n",
        );
        let r = rename_preview("o.txt", "n.txt", &del[0], &add[0]);
        assert_eq!(r.kind, ChangeKind::Renamed);
        assert_eq!(r.old_path.as_deref(), Some("o.txt"));
        assert_eq!(r.path(), "n.txt");
        assert_eq!((r.added_count(), r.removed_count()), (1, 1));
        assert_eq!(r.hunks.len(), 1);
        assert_eq!(r.hunks[0].header, "@@ -1,3 +1,3 @@");
        let removed = r.hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Removed)
            .unwrap();
        assert_eq!((removed.content.as_str(), removed.old_no), ("b", Some(2)));
        // A pure rename has no hunks.
        let same = rename_preview(
            "o.txt",
            "n.txt",
            &del[0],
            &parse_unified(&add_text("a\nb\nc\n"))[0],
        );
        assert!(same.hunks.is_empty() && !same.binary);
    }

    fn add_text(content: &str) -> String {
        let lines: String = content.lines().map(|l| format!("+{l}\n")).collect();
        format!(
            "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,{} @@\n{lines}",
            content.lines().count()
        )
    }

    #[test]
    fn root_commit_detection() {
        let (_d, repo) = temp_repo();
        let root = commit_files(&repo, &[("a.txt", "1\n")], "root");
        commit_files(&repo, &[("a.txt", "2\n")], "child");
        let p = commit_patch(&repo, &root).unwrap();
        assert!(p.contains("+1"));
        assert!(commit_patch(&repo, "HEAD").unwrap().contains("-1\n+2"));
    }

    #[test]
    fn patch_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "hello\n");
        let patch = local_changes_patch(&repo).unwrap();
        assert!(patch.contains("new file mode"));
        std::fs::remove_file(repo.path("a.txt")).unwrap();
        assert_eq!(
            apply_to_worktree(&repo, &patch).unwrap(),
            ApplyOutcome::Clean
        );
        assert_eq!(read(&repo, "a.txt"), "hello\n");
        assert!(local_changes_patch(&TempRepoClean::new().repo).is_err());
    }

    struct TempRepoClean {
        _d: TempDir,
        repo: Repo,
    }

    impl TempRepoClean {
        fn new() -> Self {
            let (_d, repo) = temp_repo();
            commit_files(&repo, &[("x", "1\n")], "init");
            Self { _d, repo }
        }
    }

    #[test]
    fn three_way_fallback_and_conflicts() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n")], "init");
        write(&repo, "a.txt", "1\n2\n3\n4\n5\n6\n7\nEIGHT\n");
        let patch = local_changes_patch(&repo).unwrap();
        repo.run(&["checkout", "--", "a.txt"]).unwrap();
        // A non-overlapping upstream change: plain apply still works (fuzz-free context differs only above).
        commit_files(
            &repo,
            &[("a.txt", "ONE\n2\n3\n4\n5\n6\n7\n8\n")],
            "upstream",
        );
        assert_eq!(
            apply_to_worktree(&repo, &patch).unwrap(),
            ApplyOutcome::Clean
        );
        assert_eq!(read(&repo, "a.txt"), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n");
        repo.run(&["checkout", "--", "a.txt"]).unwrap();
        // A conflicting change: 3-way merge leaves conflicts.
        commit_files(
            &repo,
            &[("a.txt", "ONE\n2\n3\n4\n5\n6\n7\nacht\n")],
            "conflicting",
        );
        match apply_to_worktree(&repo, &patch).unwrap() {
            ApplyOutcome::Conflicts(c) => assert_eq!(c, vec!["a.txt"]),
            o => panic!("expected conflicts, got {o:?}"),
        }
        assert!(read(&repo, "a.txt").contains("<<<<<<<"));
    }

    #[test]
    fn three_way_clean_merge_is_unstaged() {
        let (_d, repo) = temp_repo();
        let lines: String = (1..=12).map(|i| format!("{i}\n")).collect();
        commit_files(&repo, &[("a.txt", &lines)], "init");
        write(&repo, "a.txt", &lines.replace("6\n", "six\n"));
        let patch = local_changes_patch(&repo).unwrap();
        repo.run(&["checkout", "--", "a.txt"]).unwrap();
        // Change the context lines right around the hunk so plain apply fails but 3-way merges.
        commit_files(
            &repo,
            &[(
                "a.txt",
                &lines.replace("4\n", "four\n").replace("8\n", "eight\n"),
            )],
            "ctx",
        );
        assert_eq!(
            apply_to_worktree(&repo, &patch).unwrap(),
            ApplyOutcome::Clean
        );
        let content = read(&repo, "a.txt");
        assert!(
            content.contains("four\n") && content.contains("six\n") && content.contains("eight\n")
        );
        assert!(repo
            .run(&["diff", "--cached", "--name-only"])
            .unwrap()
            .trim()
            .is_empty());
    }

    #[test]
    fn commit_patch_and_am_roundtrip() {
        let (_d, repo) = temp_repo();
        let root = commit_files(&repo, &[("a.txt", "1\n")], "root commit");
        let c2 = commit_files(
            &repo,
            &[("a.txt", "2\n"), ("b.txt", "b\n")],
            "second commit",
        );
        let p = commit_patch(&repo, &c2).unwrap();
        assert_eq!(detect_format(&p), PatchFormat::Mailbox);
        assert!(p.contains("Subject: [PATCH] second commit"));
        assert!(commit_patch(&repo, &root).unwrap().contains("root commit"));
        let file = repo.root.join("p dir").join("2.patch");
        write_patch(&file, &p).unwrap();
        repo.run(&["reset", "-q", "--hard", &root]).unwrap();
        assert_eq!(
            apply_patch_file(&repo, &file).unwrap(),
            PatchFileOutcome::Committed(1)
        );
        assert_eq!(read(&repo, "a.txt"), "2\n");
        let subject = repo.run(&["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(subject.trim(), "second commit");
        // A mailbox that conflicts stops `git am`.
        repo.run(&["reset", "-q", "--hard", &root]).unwrap();
        commit_files(&repo, &[("a.txt", "other\n")], "diverge");
        match apply_patch_file(&repo, &file).unwrap() {
            PatchFileOutcome::AmStopped { .. } => {}
            o => panic!("expected am to stop, got {o:?}"),
        }
        assert_eq!(repo.state(), crate::git::RepoState::ApplyingMailbox);
        am_abort(&repo).unwrap();
        assert_eq!(repo.state(), crate::git::RepoState::Clean);
    }

    #[test]
    fn plain_patch_file_applies() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "init");
        write(&repo, "a.txt", "2\n");
        let p = local_changes_patch(&repo).unwrap();
        let file = repo.git_dir.join("t.patch");
        write_patch(&file, &p).unwrap();
        repo.run(&["checkout", "--", "a.txt"]).unwrap();
        assert_eq!(
            apply_patch_file(&repo, &file).unwrap(),
            PatchFileOutcome::Applied(ApplyOutcome::Clean)
        );
        assert_eq!(read(&repo, "a.txt"), "2\n");
        // Applying again fails cleanly (already applied, no conflicts recorded).
        assert!(apply_patch_file(&repo, &file).is_err());
    }

    #[test]
    fn new_files_patch_from_copies() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("x", "1\n")], "init");
        let store = repo.git_dir.join("copies");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("u"), "untracked\n").unwrap();
        std::fs::write(store.join("b"), [0u8, 1, 2]).unwrap();
        let p = new_files_patch(
            &repo,
            &[
                ("sub/u file.txt".into(), store.join("u")),
                ("b.bin".into(), store.join("b")),
            ],
        )
        .unwrap();
        assert_eq!(apply_to_worktree(&repo, &p).unwrap(), ApplyOutcome::Clean);
        assert_eq!(read(&repo, "sub/u file.txt"), "untracked\n");
        assert_eq!(std::fs::read(repo.path("b.bin")).unwrap(), vec![0u8, 1, 2]);
    }
}
