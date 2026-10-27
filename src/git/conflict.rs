//! Merge conflicts: conflicted files, base/ours/theirs content, 3-way chunk model, resolve,
//! continue/abort/skip of the operation in progress (merge, rebase, cherry-pick, revert, `git am`).

pub mod merge;

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};

pub use merge::{
    count_markers, marker_lines, Chunk, ChunkKind, MarkerStyle, MergeModel, Resolution,
};

use super::status::ConflictKind;
use super::{Repo, RepoState};

/// Work-tree files larger than this are not scanned for conflict markers.
const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;

/// One side of a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Stage 2: the version on HEAD ("yours").
    Ours,
    /// Stage 3: the version being merged in ("theirs").
    Theirs,
}

impl Side {
    fn flag(self) -> &'static str {
        match self {
            Side::Ours => "--ours",
            Side::Theirs => "--theirs",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Side::Ours => "yours",
            Side::Theirs => "theirs",
        }
    }
}

/// Which index stages exist for an unmerged path (blob ids).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stages {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

impl Stages {
    pub fn side(&self, side: Side) -> Option<&String> {
        match side {
            Side::Ours => self.ours.as_ref(),
            Side::Theirs => self.theirs.as_ref(),
        }
    }

    /// One side deleted the file while the other kept (or modified) it.
    pub fn is_delete_conflict(&self) -> bool {
        self.ours.is_none() || self.theirs.is_none()
    }

    /// No stage at all: the path is not in conflict.
    pub fn is_empty(&self) -> bool {
        self.base.is_none() && self.ours.is_none() && self.theirs.is_none()
    }

    /// The conflict kind implied by which stages exist.
    pub fn kind(&self) -> ConflictKind {
        match (
            self.base.is_some(),
            self.ours.is_some(),
            self.theirs.is_some(),
        ) {
            (true, true, true) => ConflictKind::BothModified,
            (false, true, true) => ConflictKind::BothAdded,
            (true, false, true) => ConflictKind::DeletedByUs,
            (true, true, false) => ConflictKind::DeletedByThem,
            (false, true, false) => ConflictKind::AddedByUs,
            (false, false, true) => ConflictKind::AddedByThem,
            _ => ConflictKind::BothDeleted,
        }
    }
}

/// A conflicted (unmerged) path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: String,
    pub kind: ConflictKind,
    pub stages: Stages,
}

impl ConflictFile {
    /// Our side has no version of the file (we deleted it, or only they added it).
    pub fn ours_missing(&self) -> bool {
        self.stages.ours.is_none()
    }

    /// Their side has no version of the file.
    pub fn theirs_missing(&self) -> bool {
        self.stages.theirs.is_none()
    }

    /// What accepting `side` does, for menus ("delete file" when that side has no file).
    pub fn accept_label(&self, side: Side) -> String {
        match (side, self.stages.side(side).is_some()) {
            (Side::Ours, true) => "Accept yours".into(),
            (Side::Theirs, true) => "Accept theirs".into(),
            (Side::Ours, false) => "Accept yours (delete the file)".into(),
            (Side::Theirs, false) => "Accept theirs (delete the file)".into(),
        }
    }
}

/// Parse `git ls-files -u -z` (`<mode> <oid> <stage>\t<path>\0`) into stages per path.
pub fn parse_unmerged(out: &str) -> BTreeMap<String, Stages> {
    let mut map: BTreeMap<String, Stages> = BTreeMap::new();
    for rec in out.split('\0').filter(|r| !r.is_empty()) {
        let Some((meta, path)) = rec.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let (_mode, oid, stage) = (parts.next(), parts.next(), parts.next());
        let (Some(oid), Some(stage)) = (oid, stage) else {
            continue;
        };
        let e = map.entry(path.to_string()).or_default();
        match stage {
            "1" => e.base = Some(oid.to_string()),
            "2" => e.ours = Some(oid.to_string()),
            "3" => e.theirs = Some(oid.to_string()),
            _ => {}
        }
    }
    map
}

/// Stages of every unmerged path.
pub fn all_stages(repo: &Repo) -> Result<BTreeMap<String, Stages>> {
    Ok(parse_unmerged(&repo.run(&["ls-files", "-u", "-z"])?))
}

/// Stages of one path (all None when the path is not in conflict).
pub fn stages(repo: &Repo, path: &str) -> Result<Stages> {
    let out = repo.run(&["--literal-pathspecs", "ls-files", "-u", "-z", "--", path])?;
    Ok(parse_unmerged(&out).remove(path).unwrap_or_default())
}

/// All conflicted files, sorted by path.
///
/// Built from `git ls-files -u` alone (no `git status`, which stats the whole work tree): the
/// conflict kind follows from which stages exist, exactly as `git status` derives it.
pub fn list(repo: &Repo) -> Result<Vec<ConflictFile>> {
    Ok(all_stages(repo)?
        .into_iter()
        .map(|(path, stages)| ConflictFile {
            path,
            kind: stages.kind(),
            stages,
        })
        .collect())
}

/// Number of unmerged paths.
pub fn unmerged_count(repo: &Repo) -> Result<usize> {
    Ok(all_stages(repo)?.len())
}

/// Conflict markers in the work-tree file: None when missing, unreadable, huge or binary.
pub fn worktree_markers(repo: &Repo, path: &str) -> Option<usize> {
    let p = repo.path(path);
    let meta = std::fs::metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
        return None;
    }
    let bytes = std::fs::read(&p).ok()?;
    if is_binary(&bytes) {
        return None;
    }
    Some(count_markers(&String::from_utf8_lossy(&bytes)))
}

/// Size and modification time of a work-tree file, used to skip rescanning unchanged files.
type FileStamp = (u64, Option<std::time::SystemTime>);

/// Caches conflict-marker counts of work-tree files by size and modification time, so refreshing
/// the Conflicts tab (which happens on every refresh, even while it is hidden) only reads files
/// that changed. A refresh scans at most `max_files` files and `byte_budget` bytes of changed
/// files; the rest report None ("not scanned") until a later refresh.
#[derive(Debug)]
pub struct MarkerCache {
    entries: std::collections::HashMap<String, (FileStamp, Option<usize>)>,
    max_files: usize,
    byte_budget: u64,
}

impl Default for MarkerCache {
    fn default() -> Self {
        Self::new(200, 16 * 1024 * 1024)
    }
}

impl MarkerCache {
    pub fn new(max_files: usize, byte_budget: u64) -> Self {
        Self {
            entries: Default::default(),
            max_files,
            byte_budget,
        }
    }

    /// Marker counts for `paths` (in order). Entries for paths no longer listed are dropped.
    pub fn scan(&mut self, repo: &Repo, paths: &[&str]) -> Vec<Option<usize>> {
        let mut budget = self.byte_budget;
        let mut old = std::mem::take(&mut self.entries);
        let mut out = Vec::with_capacity(paths.len());
        for (i, &path) in paths.iter().enumerate() {
            let stamp = std::fs::metadata(repo.path(path))
                .ok()
                .filter(|m| m.is_file())
                .map(|m| (m.len(), m.modified().ok()));
            let Some(stamp) = stamp else {
                out.push(None);
                continue;
            };
            let count = match old.remove(path) {
                Some((s, c)) if s == stamp => Some(c),
                _ if i < self.max_files && stamp.0 <= budget => {
                    budget -= stamp.0;
                    Some(worktree_markers(repo, path))
                }
                _ => None,
            };
            if let Some(c) = count {
                self.entries.insert(path.to_string(), (stamp, c));
            }
            out.push(count.flatten());
        }
        out
    }
}

/// Binary heuristic: a NUL byte in the first 8000 bytes, or invalid UTF-8 (which could not be
/// merged line by line without corrupting it).
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0) || std::str::from_utf8(bytes).is_err()
}

/// The three versions of a conflicted file. Missing stages are None.
#[derive(Debug, Clone, Default)]
pub struct MergeInput {
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

impl MergeInput {
    /// Any present version is binary (or not valid UTF-8).
    pub fn is_binary(&self) -> bool {
        [&self.base, &self.ours, &self.theirs]
            .into_iter()
            .flatten()
            .any(|b| is_binary(b))
    }

    /// Build the chunk model (missing versions count as empty). Only meaningful for text inputs.
    pub fn model(&self) -> MergeModel {
        fn text(b: &Option<Vec<u8>>) -> std::borrow::Cow<'_, str> {
            b.as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default()
        }
        MergeModel::new(&text(&self.base), &text(&self.ours), &text(&self.theirs))
    }
}

/// Read one index stage of `path` as it would be checked out: smudge filters and end-of-line
/// conversion (`core.autocrlf`, `.gitattributes eol=crlf`) applied, like the conflicted work-tree
/// file git wrote. Falls back to the raw blob when a filter cannot run (e.g. git-lfs missing).
fn read_stage(repo: &Repo, path: &str, oid: &str) -> Result<Vec<u8>> {
    let path_arg = format!("--path={path}");
    repo.run_bytes(&["cat-file", "--filters", &path_arg, oid])
        .or_else(|_| repo.run_bytes(&["cat-file", "blob", oid]))
        .with_context(|| format!("reading {oid} ({path})"))
}

/// Read base/ours/theirs (index stages 1, 2 and 3) with work-tree conversion applied, so the
/// line endings match what git wrote to the work tree. Missing stages are None.
pub fn load_input(repo: &Repo, path: &str) -> Result<MergeInput> {
    let st = stages(repo, path)?;
    let read = |oid: &Option<String>| -> Result<Option<Vec<u8>>> {
        oid.as_deref()
            .map(|o| read_stage(repo, path, o))
            .transpose()
    };
    Ok(MergeInput {
        base: read(&st.base)?,
        ours: read(&st.ours)?,
        theirs: read(&st.theirs)?,
    })
}

/// Feed NUL-separated paths to a git command taking `--pathspec-from-file=-` (no command-line limits).
fn run_paths(repo: &Repo, args: &[&str], paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut full = vec!["--literal-pathspecs"];
    full.extend_from_slice(args);
    full.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    repo.run_stdin(&full, &input)?;
    Ok(())
}

/// Resolve `paths` by taking one side wholesale (`git checkout --ours/--theirs` + `git add`);
/// paths that side deleted are removed (`git rm`). Returns the number of paths resolved.
pub fn accept_side(repo: &Repo, paths: &[String], side: Side) -> Result<usize> {
    let all = all_stages(repo)?;
    let (mut keep, mut remove) = (Vec::new(), Vec::new());
    for p in paths {
        match all.get(p) {
            Some(st) if st.side(side).is_some() => keep.push(p.clone()),
            Some(_) => remove.push(p.clone()),
            None => {}
        }
    }
    run_paths(repo, &["checkout", side.flag()], &keep)?;
    run_paths(repo, &["add", "-A"], &keep)?;
    delete(repo, &remove)?;
    Ok(keep.len() + remove.len())
}

/// Mark paths resolved with their current work-tree content (deleted files are staged as deleted).
pub fn mark_resolved(repo: &Repo, paths: &[String]) -> Result<()> {
    run_paths(repo, &["add", "-A"], paths)
}

/// Resolve paths by deleting them from the index and the work tree.
pub fn delete(repo: &Repo, paths: &[String]) -> Result<()> {
    run_paths(repo, &["rm", "-q", "-f", "--ignore-unmatch"], paths)
}

/// Write the merged content to the work-tree file and optionally stage it (mark resolved).
pub fn write_result(repo: &Repo, path: &str, content: &[u8], resolve: bool) -> Result<()> {
    let abs = repo.path(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&abs, content).with_context(|| format!("writing {}", abs.display()))?;
    if resolve {
        mark_resolved(repo, &[path.to_string()])?;
    }
    Ok(())
}

/// Whether conflict markers should include the base section.
pub fn uses_diff3(repo: &Repo) -> bool {
    repo.config_get("merge.conflictStyle")
        .is_some_and(|s| s.eq_ignore_ascii_case("diff3") || s.eq_ignore_ascii_case("zdiff3"))
}

/// The operation in progress, with what is needed to describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub state: RepoState,
    /// Current branch (None when detached).
    pub branch: Option<String>,
    /// (current, total) for rebase and `git am`.
    pub step: Option<(usize, usize)>,
    /// Branch being rebased.
    pub head_name: Option<String>,
    /// Where a rebase replays commits onto.
    pub onto: Option<String>,
    /// Label for the incoming ("theirs") side: branch or commit being merged, picked or replayed.
    pub other: Option<String>,
    /// Subject of the commit (or patch) being applied.
    pub subject: Option<String>,
}

impl Operation {
    /// A merge, rebase, cherry-pick, revert or `git am` is in progress.
    pub fn in_progress(&self) -> bool {
        matches!(
            self.state,
            RepoState::Merging
                | RepoState::Rebasing
                | RepoState::CherryPicking
                | RepoState::Reverting
                | RepoState::ApplyingMailbox
        )
    }

    /// Skipping the current commit is possible (not for merges).
    pub fn can_skip(&self) -> bool {
        self.in_progress() && self.state != RepoState::Merging
    }

    /// Noun for messages: "merge", "rebase"...
    pub fn noun(&self) -> &'static str {
        match self.state {
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::ApplyingMailbox => "patch application",
            RepoState::Bisecting => "bisect",
            RepoState::Clean => "operation",
        }
    }

    /// One-line description for the banner.
    pub fn describe(&self) -> String {
        let step = self
            .step
            .map(|(a, b)| format!(" (step {a}/{b})"))
            .unwrap_or_default();
        let subject = self
            .subject
            .as_deref()
            .map(|s| format!(" — \"{s}\""))
            .unwrap_or_default();
        let other = self.other.as_deref().unwrap_or("another branch");
        let here = self.branch.as_deref().unwrap_or("detached HEAD");
        match self.state {
            RepoState::Clean => String::new(),
            RepoState::Merging => format!("Merging {other} into {here}"),
            RepoState::Rebasing => {
                let head = self.head_name.as_deref().unwrap_or("detached HEAD");
                let onto = self
                    .onto
                    .as_deref()
                    .map(|o| format!(" onto {o}"))
                    .unwrap_or_default();
                format!("Rebasing {head}{onto}{step}{subject}")
            }
            RepoState::CherryPicking => format!("Cherry-picking {other}{subject}"),
            RepoState::Reverting => format!(
                "Reverting {}{subject}",
                self.other.as_deref().unwrap_or("a commit")
            ),
            RepoState::ApplyingMailbox => format!("Applying patches{step}{subject}"),
            RepoState::Bisecting => "Bisecting".to_string(),
        }
    }

    /// Title of the "theirs" pane.
    pub fn theirs_label(&self) -> String {
        match (self.state, &self.other) {
            (RepoState::Reverting, Some(o)) => {
                format!("parent of {}", o.trim_start_matches("revert of "))
            }
            (_, Some(o)) => o.clone(),
            _ => "theirs".to_string(),
        }
    }
}

fn read_git_file(repo: &Repo, rel: &str) -> Option<String> {
    std::fs::read_to_string(repo.git_dir.join(rel))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn first_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Branch or remote-branch names pointing at `rev`, else its short hash.
fn name_of(repo: &Repo, rev: &str) -> Option<String> {
    let names = repo
        .try_run(&[
            "for-each-ref",
            "--points-at",
            rev,
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])
        .and_then(|s| first_line(&s));
    names.or_else(|| short(repo, rev))
}

fn short(repo: &Repo, rev: &str) -> Option<String> {
    repo.try_run(&[
        "rev-parse",
        "--short",
        "--verify",
        "-q",
        &format!("{rev}^{{commit}}"),
    ])
    .and_then(|s| first_line(&s))
}

fn subject_of(repo: &Repo, rev: &str) -> Option<String> {
    repo.try_run(&["log", "-1", "--format=%s", rev, "--"])
        .and_then(|s| first_line(&s))
}

/// The quoted name in a `MERGE_MSG` first line ("Merge branch 'feature' into main").
pub fn merge_msg_name(msg: &str) -> Option<String> {
    let line = msg.lines().next()?;
    let start = line.find('\'')? + 1;
    let len = line[start..].find('\'')?;
    Some(line[start..start + len].to_string()).filter(|s| !s.is_empty())
}

fn step_from(repo: &Repo, dir: &str, cur: &str, total: &str) -> Option<(usize, usize)> {
    let a = read_git_file(repo, &format!("{dir}/{cur}"))?.parse().ok()?;
    let b = read_git_file(repo, &format!("{dir}/{total}"))?
        .parse()
        .ok()?;
    Some((a, b))
}

/// Files under `.git` that determine [`operation`]'s result. Their contents (cheap file reads, no
/// git processes) form a fingerprint: while it is unchanged, the cached operation is still valid.
const OPERATION_FILES: [&str; 15] = [
    "HEAD",
    "MERGE_HEAD",
    "MERGE_MSG",
    "REBASE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge/msgnum",
    "rebase-merge/end",
    "rebase-merge/head-name",
    "rebase-merge/onto",
    "rebase-apply/next",
    "rebase-apply/last",
    "rebase-apply/head-name",
    "rebase-apply/onto",
    "rebase-apply/final-commit",
];

/// A cheap fingerprint of the operation in progress (see [`OPERATION_FILES`]).
pub fn operation_fingerprint(repo: &Repo) -> String {
    let mut fp = format!("{:?}", repo.state());
    for f in OPERATION_FILES {
        fp.push('\0');
        if let Ok(s) = std::fs::read_to_string(repo.git_dir.join(f)) {
            fp.push_str(&s);
        }
    }
    fp
}

/// Describe the operation in progress.
pub fn operation(repo: &Repo) -> Operation {
    let state = repo.state();
    let mut op = Operation {
        state,
        branch: repo.head_branch(),
        step: None,
        head_name: None,
        onto: None,
        other: None,
        subject: None,
    };
    match state {
        RepoState::Merging => {
            op.other = read_git_file(repo, "MERGE_MSG")
                .and_then(|m| merge_msg_name(&m))
                .or_else(|| name_of(repo, "MERGE_HEAD"));
        }
        RepoState::Rebasing => {
            let dir = if repo.git_dir.join("rebase-merge").exists() {
                "rebase-merge"
            } else {
                "rebase-apply"
            };
            op.step = step_from(repo, dir, "msgnum", "end")
                .or_else(|| step_from(repo, dir, "next", "last"));
            op.head_name = read_git_file(repo, &format!("{dir}/head-name"))
                .map(|h| h.trim_start_matches("refs/heads/").to_string())
                .filter(|h| h != "detached HEAD");
            op.onto = read_git_file(repo, &format!("{dir}/onto")).and_then(|o| name_of(repo, &o));
            if repo.git_dir.join("REBASE_HEAD").exists() {
                let sh = short(repo, "REBASE_HEAD");
                op.subject = subject_of(repo, "REBASE_HEAD");
                op.other = match (&op.head_name, sh) {
                    (Some(h), Some(s)) => Some(format!("{h} @ {s}")),
                    (None, Some(s)) => Some(s),
                    (h, None) => h.clone(),
                };
            } else {
                op.other = op.head_name.clone();
            }
        }
        RepoState::CherryPicking => {
            op.other = short(repo, "CHERRY_PICK_HEAD");
            op.subject = subject_of(repo, "CHERRY_PICK_HEAD");
        }
        RepoState::Reverting => {
            op.other = short(repo, "REVERT_HEAD").map(|s| format!("revert of {s}"));
            op.subject = subject_of(repo, "REVERT_HEAD");
        }
        RepoState::ApplyingMailbox => {
            op.step = step_from(repo, "rebase-apply", "next", "last");
            op.subject = read_git_file(repo, "rebase-apply/final-commit")
                .or_else(|| read_git_file(repo, "rebase-apply/msg"))
                .and_then(|m| first_line(&m));
            op.other = Some(match op.step {
                Some((n, _)) => format!("patch {n}"),
                None => "patch".to_string(),
            });
        }
        RepoState::Clean | RepoState::Bisecting => {}
    }
    op
}

fn op_args(state: RepoState, what: &'static str) -> Option<Vec<&'static str>> {
    let cmd = match state {
        RepoState::Merging => "merge",
        RepoState::Rebasing => "rebase",
        RepoState::CherryPicking => "cherry-pick",
        RepoState::Reverting => "revert",
        RepoState::ApplyingMailbox => "am",
        RepoState::Clean | RepoState::Bisecting => return None,
    };
    if state == RepoState::Merging && what == "--continue" {
        return Some(vec!["commit", "--no-edit"]);
    }
    if state == RepoState::Merging && what == "--skip" {
        return None;
    }
    Some(vec![cmd, what])
}

fn no_operation() -> anyhow::Error {
    anyhow::anyhow!("No merge, rebase, cherry-pick, revert or patch application is in progress")
}

/// What happened after continuing/skipping: finished, or stopped again (conflicts, next step).
fn outcome(repo: &Repo, before: &Operation, verb: &str) -> String {
    let now = operation(repo);
    let conflicts = unmerged_count(repo).unwrap_or(0);
    if conflicts > 0 {
        let step = now
            .step
            .map(|(a, b)| format!(" at step {a}/{b}"))
            .unwrap_or_default();
        format!(
            "{} stopped{step}: {conflicts} new conflict(s) to resolve",
            capitalize(before.noun())
        )
    } else if now.in_progress() {
        let step = now
            .step
            .map(|(a, b)| format!(" at step {a}/{b}"))
            .unwrap_or_default();
        format!("{} {verb}; stopped{step}", capitalize(before.noun()))
    } else {
        format!("{} finished", capitalize(before.noun()))
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Continue the operation (merge → `git commit --no-edit`; others → `--continue`). Refuses while
/// unmerged paths remain. New conflicts from the next step are reported as success.
pub fn continue_operation(repo: &Repo) -> Result<String> {
    let op = operation(repo);
    let args = op_args(op.state, "--continue").ok_or_else(no_operation)?;
    let n = unmerged_count(repo)?;
    if n > 0 {
        bail!(
            "Resolve all conflicts before continuing the {} ({n} file(s) still in conflict)",
            op.noun()
        );
    }
    match repo.run(&args) {
        Ok(_) => Ok(outcome(repo, &op, "continued")),
        Err(_) if unmerged_count(repo).unwrap_or(0) > 0 => Ok(outcome(repo, &op, "continued")),
        Err(e) => Err(e),
    }
}

/// Abort the operation, restoring the state before it started.
pub fn abort_operation(repo: &Repo) -> Result<String> {
    let op = operation(repo);
    let args = op_args(op.state, "--abort").ok_or_else(no_operation)?;
    repo.run(&args)?;
    Ok(format!("{} aborted", capitalize(op.noun())))
}

/// Skip the commit being applied (rebase, cherry-pick, revert, am).
pub fn skip_operation(repo: &Repo) -> Result<String> {
    let op = operation(repo);
    if op.state == RepoState::Merging {
        bail!("A merge cannot skip commits; continue or abort it");
    }
    let args = op_args(op.state, "--skip").ok_or_else(no_operation)?;
    match repo.run(&args) {
        Ok(_) => Ok(outcome(repo, &op, "skipped a commit")),
        Err(_) if unmerged_count(repo).unwrap_or(0) > 0 => {
            Ok(outcome(repo, &op, "skipped a commit"))
        }
        Err(e) => Err(e),
    }
}

/// Write the given stage of `path` to `dir/<file name>` (empty file when missing), for diffing.
pub fn export_stage(
    repo: &Repo,
    path: &str,
    side: Option<Side>,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let st = stages(repo, path)?;
    let oid = match side {
        None => st.base.as_ref(),
        Some(s) => st.side(s),
    };
    let bytes = match oid {
        Some(o) => read_stage(repo, path, o)?,
        None => Vec::new(),
    };
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    let out = dir.join(name);
    std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    /// main and feature both change line 2 of f.txt; main also appends a line; then merge.
    fn merge_conflict() -> (TempDir, Repo) {
        let (d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nB-feature\nc\n")], "feature change");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nB-main\nc\nd\n")], "main change");
        assert!(repo.run(&["merge", "feature"]).is_err());
        (d, repo)
    }

    fn norm(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    #[test]
    fn parses_unmerged_records() {
        let raw = [
            "100644 aaa 1\tsp ace.txt",
            "100644 bbb 2\tsp ace.txt",
            "100644 ccc 3\tsp ace.txt",
            "100644 ddd 1\tdel.txt",
            "100644 eee 3\tdel.txt",
            "",
        ]
        .join("\0");
        let m = parse_unmerged(&raw);
        assert_eq!(m.len(), 2);
        let s = &m["sp ace.txt"];
        assert_eq!(
            (s.base.as_deref(), s.ours.as_deref(), s.theirs.as_deref()),
            (Some("aaa"), Some("bbb"), Some("ccc"))
        );
        assert!(m["del.txt"].ours.is_none() && m["del.txt"].is_delete_conflict());
        assert_eq!(s.kind(), ConflictKind::BothModified);
        assert_eq!(m["del.txt"].kind(), ConflictKind::DeletedByUs);
        assert!(Stages::default().is_empty());
    }

    #[test]
    fn merge_message_names() {
        assert_eq!(
            merge_msg_name("Merge branch 'feature' into main\n").as_deref(),
            Some("feature")
        );
        assert_eq!(
            merge_msg_name("Merge remote-tracking branch 'origin/x'").as_deref(),
            Some("origin/x")
        );
        assert_eq!(merge_msg_name("Merge commit 'abc'").as_deref(), Some("abc"));
        assert_eq!(merge_msg_name("Merge whatever"), None);
    }

    #[test]
    fn end_to_end_merge_resolution() {
        let (_d, repo) = merge_conflict();
        let files = list(&repo).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "f.txt");
        assert_eq!(files[0].kind, ConflictKind::BothModified);
        assert_eq!(worktree_markers(&repo, "f.txt"), Some(1));

        let op = operation(&repo);
        assert_eq!(op.state, RepoState::Merging);
        assert_eq!(op.other.as_deref(), Some("feature"));
        assert!(op.describe().contains("Merging feature into main"));
        assert!(
            continue_operation(&repo).is_err(),
            "must refuse while conflicts remain"
        );

        let input = load_input(&repo, "f.txt").unwrap();
        assert!(!input.is_binary());
        let mut model = input.model();
        assert_eq!(model.conflicts_remaining(), 1);
        assert_eq!(
            model.pending_changes(),
            1,
            "main's appended line is one-sided"
        );
        model.apply_non_conflicting();
        let ci = model.find(None, true, Chunk::is_conflict).unwrap();
        model.resolve(ci, Resolution::OursThenTheirs);
        assert!(model.is_fully_resolved());
        let text = model.result_text();
        assert_eq!(norm(&text), "a\nB-main\nB-feature\nc\nd\n");

        write_result(&repo, "f.txt", text.as_bytes(), true).unwrap();
        assert!(list(&repo).unwrap().is_empty());
        let msg = continue_operation(&repo).unwrap();
        assert_eq!(msg, "Merge finished");
        assert_eq!(repo.state(), RepoState::Clean);
        let parents = repo.run(&["rev-list", "--parents", "-n1", "HEAD"]).unwrap();
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "merge commit has two parents"
        );
        let committed = repo.run(&["show", "HEAD:f.txt"]).unwrap();
        assert_eq!(norm(&committed), "a\nB-main\nB-feature\nc\nd\n");
    }

    #[test]
    fn autocrlf_work_tree_keeps_crlf_after_save() {
        let (_d, repo) = temp_repo();
        repo.run(&["config", "core.autocrlf", "true"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nb\nc\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nB-feature\nc\n")], "feature change");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nB-main\nc\nd\n")], "main change");
        assert!(repo.run(&["merge", "feature"]).is_err());
        let wt = std::fs::read_to_string(repo.path("f.txt")).unwrap();
        assert!(
            wt.contains("\r\n"),
            "git writes the conflicted file with CRLF: {wt:?}"
        );

        let input = load_input(&repo, "f.txt").unwrap();
        assert!(input
            .ours
            .as_deref()
            .is_some_and(|b| b.windows(2).any(|w| w == b"\r\n")));
        let mut model = input.model();
        assert_eq!(model.eol, "\r\n");
        model.apply_non_conflicting();
        let ci = model.find(None, true, Chunk::is_conflict).unwrap();
        model.resolve(ci, Resolution::Theirs);
        write_result(&repo, "f.txt", model.result_text().as_bytes(), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path("f.txt")).unwrap(),
            "a\r\nB-feature\r\nc\r\nd\r\n"
        );
        assert_eq!(
            repo.run(&["diff", "--name-only"]).unwrap().trim(),
            "",
            "work tree matches the index"
        );
        continue_operation(&repo).unwrap();
        let blob = repo.run_bytes(&["cat-file", "blob", "HEAD:f.txt"]).unwrap();
        assert_eq!(blob, b"a\nB-feature\nc\nd\n", "the repository keeps LF");
    }

    #[test]
    fn exported_stages_use_work_tree_line_endings() {
        let (_d, repo) = temp_repo();
        write(&repo, ".gitattributes", "*.txt eol=crlf\n");
        commit_files(&repo, &[("f.txt", "a\nb\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\ntheirs\n")], "t");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("f.txt", "a\nours\n")], "o");
        assert!(repo.run(&["merge", "feature"]).is_err());
        let dir = std::env::temp_dir().join(format!("gitjet-stage-{}", std::process::id()));
        let out = export_stage(&repo, "f.txt", Some(Side::Theirs), &dir).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "a\r\ntheirs\r\n");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_input(&repo, "f.txt").unwrap().model().eol, "\r\n");
    }

    #[test]
    fn marker_cache_rescans_only_changed_files() {
        let (_d, repo) = merge_conflict();
        let mut cache = MarkerCache::new(10, 1 << 20);
        assert_eq!(
            cache.scan(&repo, &["f.txt", "missing.txt"]),
            vec![Some(1), None]
        );
        // Unchanged files come from the cache, even with no budget left.
        cache.byte_budget = 0;
        assert_eq!(cache.scan(&repo, &["f.txt"]), vec![Some(1)]);
        // A changed file needs a rescan, which the budget does not allow.
        std::fs::write(repo.path("f.txt"), "resolved\n").unwrap();
        assert_eq!(cache.scan(&repo, &["f.txt"]), vec![None]);
        cache.byte_budget = 1 << 20;
        assert_eq!(cache.scan(&repo, &["f.txt"]), vec![Some(0)]);
        // Only the first max_files files are scanned.
        let mut capped = MarkerCache::new(0, 1 << 20);
        assert_eq!(capped.scan(&repo, &["f.txt"]), vec![None]);
    }

    #[test]
    fn operation_fingerprint_tracks_the_operation() {
        let (_d, repo) = merge_conflict();
        let during = operation_fingerprint(&repo);
        assert_eq!(during, operation_fingerprint(&repo));
        abort_operation(&repo).unwrap();
        assert_ne!(during, operation_fingerprint(&repo));
    }

    #[test]
    fn accept_sides_and_abort() {
        let (_d, repo) = merge_conflict();
        accept_side(&repo, &["f.txt".to_string()], Side::Theirs).unwrap();
        assert_eq!(unmerged_count(&repo).unwrap(), 0);
        assert_eq!(
            norm(&std::fs::read_to_string(repo.path("f.txt")).unwrap()),
            "a\nB-feature\nc\n"
        );
        assert_eq!(abort_operation(&repo).unwrap(), "Merge aborted");
        assert_eq!(repo.state(), RepoState::Clean);
        assert_eq!(
            norm(&std::fs::read_to_string(repo.path("f.txt")).unwrap()),
            "a\nB-main\nc\nd\n"
        );
        assert!(abort_operation(&repo).is_err());
        assert!(skip_operation(&repo).is_err());
    }

    #[test]
    fn delete_modify_conflict() {
        let (_d, repo) = temp_repo();
        commit_files(
            &repo,
            &[("dir/x y.txt", "1\n"), ("keep.txt", "k\n")],
            "base",
        );
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("dir/x y.txt", "2\n")], "modify");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["rm", "-q", "dir/x y.txt"]).unwrap();
        repo.run(&["commit", "-q", "-m", "delete"]).unwrap();
        assert!(repo.run(&["merge", "feature"]).is_err());
        let files = list(&repo).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.kind, ConflictKind::DeletedByUs);
        assert!(f.ours_missing() && !f.theirs_missing());
        assert_eq!(f.accept_label(Side::Ours), "Accept yours (delete the file)");
        let input = load_input(&repo, "dir/x y.txt").unwrap();
        assert!(input.ours.is_none() && input.theirs.is_some());

        // Accepting ours deletes the file.
        accept_side(&repo, std::slice::from_ref(&f.path), Side::Ours).unwrap();
        assert!(!repo.path("dir/x y.txt").exists());
        assert_eq!(unmerged_count(&repo).unwrap(), 0);
        continue_operation(&repo).unwrap();
        assert!(repo.run(&["cat-file", "-e", "HEAD:dir/x y.txt"]).is_err());
    }

    #[test]
    fn delete_modify_keep_theirs() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("a.txt", "2\n")], "modify");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["rm", "-q", "a.txt"]).unwrap();
        repo.run(&["commit", "-q", "-m", "delete"]).unwrap();
        assert!(repo.run(&["merge", "feature"]).is_err());
        accept_side(&repo, &["a.txt".to_string()], Side::Theirs).unwrap();
        assert_eq!(
            norm(&std::fs::read_to_string(repo.path("a.txt")).unwrap()),
            "2\n"
        );
        assert_eq!(unmerged_count(&repo).unwrap(), 0);
    }

    #[test]
    fn binary_conflict_detection() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("img.bin", "\0base")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("img.bin", "\0theirs")], "t");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("img.bin", "\0ours")], "o");
        assert!(repo.run(&["merge", "feature"]).is_err());
        let input = load_input(&repo, "img.bin").unwrap();
        assert!(input.is_binary());
        assert_eq!(worktree_markers(&repo, "img.bin"), None);
        accept_side(&repo, &["img.bin".to_string()], Side::Ours).unwrap();
        assert_eq!(std::fs::read(repo.path("img.bin")).unwrap(), b"\0ours");
    }

    #[test]
    fn mark_resolved_and_write_without_staging() {
        let (_d, repo) = merge_conflict();
        write_result(&repo, "f.txt", b"edited\n", false).unwrap();
        assert_eq!(unmerged_count(&repo).unwrap(), 1);
        assert_eq!(worktree_markers(&repo, "f.txt"), Some(0));
        mark_resolved(&repo, &["f.txt".to_string()]).unwrap();
        assert_eq!(unmerged_count(&repo).unwrap(), 0);
    }

    #[test]
    fn rebase_conflict_continue_and_skip() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "base\n"), ("g.txt", "g\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        commit_files(&repo, &[("f.txt", "feature 1\n")], "feature one");
        commit_files(&repo, &[("g.txt", "feature g\n")], "feature two");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(
            &repo,
            &[("f.txt", "main\n"), ("g.txt", "main g\n")],
            "main change",
        );
        repo.run(&["checkout", "-q", "feature"]).unwrap();
        assert!(repo.run(&["rebase", "main"]).is_err());

        let op = operation(&repo);
        assert_eq!(op.state, RepoState::Rebasing);
        assert_eq!(op.step, Some((1, 2)));
        assert_eq!(op.head_name.as_deref(), Some("feature"));
        assert_eq!(op.onto.as_deref(), Some("main"));
        assert_eq!(op.subject.as_deref(), Some("feature one"));
        assert!(op.other.as_deref().unwrap_or("").starts_with("feature @ "));
        assert!(op.can_skip());

        accept_side(&repo, &["f.txt".to_string()], Side::Theirs).unwrap();
        let msg = continue_operation(&repo).unwrap();
        assert!(
            msg.contains("new conflict"),
            "second commit conflicts too: {msg}"
        );
        assert_eq!(operation(&repo).step, Some((2, 2)));
        let msg = skip_operation(&repo).unwrap();
        assert_eq!(msg, "Rebase finished");
        assert_eq!(repo.state(), RepoState::Clean);
        assert_eq!(
            norm(&repo.run(&["show", "HEAD:f.txt"]).unwrap()),
            "feature 1\n"
        );
    }

    #[test]
    fn cherry_pick_operation() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("f.txt", "base\n")], "base");
        repo.run(&["checkout", "-q", "-b", "feature"]).unwrap();
        let pick = commit_files(&repo, &[("f.txt", "feature\n")], "pick me");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        commit_files(&repo, &[("f.txt", "main\n")], "main");
        assert!(repo.run(&["cherry-pick", &pick]).is_err());
        let op = operation(&repo);
        assert_eq!(op.state, RepoState::CherryPicking);
        assert_eq!(op.subject.as_deref(), Some("pick me"));
        assert!(pick.starts_with(op.other.as_deref().unwrap()));
        assert_eq!(abort_operation(&repo).unwrap(), "Cherry-pick aborted");
        assert_eq!(repo.state(), RepoState::Clean);
    }

    #[test]
    fn export_stages_for_comparison() {
        let (_d, repo) = merge_conflict();
        let dir = repo.data_dir().unwrap().join("test-export");
        let ours = export_stage(&repo, "f.txt", Some(Side::Ours), &dir.join("o")).unwrap();
        let theirs = export_stage(&repo, "f.txt", Some(Side::Theirs), &dir.join("t")).unwrap();
        assert_eq!(
            norm(&std::fs::read_to_string(ours).unwrap()),
            "a\nB-main\nc\nd\n"
        );
        assert_eq!(
            norm(&std::fs::read_to_string(theirs).unwrap()),
            "a\nB-feature\nc\n"
        );
    }

    #[test]
    fn clean_repo_has_no_operation() {
        let (_d, repo) = temp_repo();
        let op = operation(&repo);
        assert!(!op.in_progress());
        assert!(op.describe().is_empty());
        assert!(list(&repo).unwrap().is_empty());
        assert!(continue_operation(&repo).is_err());
    }
}
