//! Repository administration: tags, remotes, worktrees, submodules, .gitignore, config, and the
//! nested/recent repositories used by the repository switcher.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::git::Repo;

// ===== tags =====================================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Short hash of the commit the tag points to (peeled)
    pub target: String,
    /// Tagger date (annotated) or commit date (lightweight), unix seconds
    pub date: i64,
    /// First line of the tag message (annotated) or of the commit message (lightweight)
    pub subject: String,
    pub annotated: bool,
}

/// All tags, newest first.
pub fn list_tags(repo: &Repo) -> Result<Vec<Tag>> {
    let out = repo.run(&[
        "for-each-ref",
        "--sort=-creatordate",
        "--format=%(refname:short)%00%(objecttype)%00%(objectname:short)%00%(*objectname:short)%00%(creatordate:unix)%00%(contents:subject)",
        "refs/tags",
    ])?;
    Ok(parse_tags(&out))
}

fn parse_tags(out: &str) -> Vec<Tag> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\0').collect();
            if f.len() < 6 {
                return None;
            }
            let annotated = f[1] == "tag";
            let target = if annotated && !f[3].is_empty() {
                f[3]
            } else {
                f[2]
            };
            Some(Tag {
                name: f[0].to_string(),
                target: target.to_string(),
                date: f[4].trim().parse().unwrap_or(0),
                subject: f[5].to_string(),
                annotated,
            })
        })
        .collect()
}

/// Create a tag at `rev` (HEAD when empty); a non-empty `message` makes it annotated.
pub fn create_tag(repo: &Repo, name: &str, rev: &str, message: &str) -> Result<()> {
    validate_ref_name(repo, &format!("refs/tags/{name}"))?;
    let rev = if rev.trim().is_empty() {
        "HEAD"
    } else {
        rev.trim()
    };
    if message.trim().is_empty() {
        repo.run(&["tag", "--", name, rev])?;
    } else {
        repo.run(&["tag", "-a", "-m", message, "--", name, rev])?;
    }
    Ok(())
}

pub fn delete_tag(repo: &Repo, name: &str) -> Result<()> {
    repo.run(&["tag", "-d", "--", name])?;
    Ok(())
}

/// Push one tag (network: run via `Action::spawn`).
pub fn push_tag(repo: &Repo, remote: &str, name: &str) -> Result<String> {
    repo.run(&[
        "push",
        remote,
        &format!("refs/tags/{name}:refs/tags/{name}"),
    ])?;
    Ok(format!("Pushed tag {name} to {remote}"))
}

/// Push all tags (network).
pub fn push_all_tags(repo: &Repo, remote: &str) -> Result<String> {
    repo.run(&["push", remote, "--tags"])?;
    Ok(format!("Pushed all tags to {remote}"))
}

/// Delete a tag on the remote (network).
pub fn delete_remote_tag(repo: &Repo, remote: &str, name: &str) -> Result<String> {
    repo.run(&["push", remote, "--delete", &format!("refs/tags/{name}")])?;
    Ok(format!("Deleted tag {name} on {remote}"))
}

/// Check out a tag (detached HEAD).
pub fn checkout_tag(repo: &Repo, name: &str) -> Result<()> {
    repo.run(&["checkout", "--detach", &format!("refs/tags/{name}")])?;
    Ok(())
}

fn validate_ref_name(repo: &Repo, full: &str) -> Result<()> {
    if repo.run(&["check-ref-format", full]).is_err() {
        bail!(
            "{:?} is not a valid name",
            full.rsplit('/').next().unwrap_or(full)
        );
    }
    Ok(())
}

// ===== remotes ==================================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    /// Push URLs (usually one, equal to the fetch URL)
    pub push_urls: Vec<String>,
}

pub fn list_remotes(repo: &Repo) -> Result<Vec<Remote>> {
    Ok(parse_remotes(&repo.run(&["remote", "-v"])?))
}

fn parse_remotes(out: &str) -> Vec<Remote> {
    let mut v: Vec<Remote> = Vec::new();
    for line in out.lines() {
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let (url, kind) = match rest.rsplit_once(' ') {
            Some((u, k)) if k == "(fetch)" || k == "(push)" => (u, k),
            _ => (rest, "(fetch)"),
        };
        let idx = match v.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                v.push(Remote {
                    name: name.to_string(),
                    fetch_url: String::new(),
                    push_urls: Vec::new(),
                });
                v.len() - 1
            }
        };
        let r = &mut v[idx];
        if kind == "(fetch)" {
            r.fetch_url = url.to_string();
        } else {
            r.push_urls.push(url.to_string());
        }
    }
    v
}

/// The remote to use when the user did not pick one: the current branch's remote, `origin`, or the only one.
pub fn default_remote(repo: &Repo, remotes: &[Remote]) -> Option<String> {
    if let Some(b) = repo.head_branch() {
        if let Some(r) = repo.config_get(&format!("branch.{b}.remote")) {
            if remotes.iter().any(|x| x.name == r) {
                return Some(r);
            }
        }
    }
    remotes
        .iter()
        .find(|r| r.name == "origin")
        .or(remotes.first())
        .map(|r| r.name.clone())
}

pub fn add_remote(repo: &Repo, name: &str, url: &str) -> Result<()> {
    repo.run(&["remote", "add", "--", name.trim(), url.trim()])?;
    Ok(())
}

pub fn remove_remote(repo: &Repo, name: &str) -> Result<()> {
    repo.run(&["remote", "remove", name])?;
    Ok(())
}

pub fn rename_remote(repo: &Repo, old: &str, new: &str) -> Result<()> {
    repo.run(&["remote", "rename", old, new.trim()])?;
    Ok(())
}

/// Set the fetch URL (`push = false`) or the push URL.
pub fn set_remote_url(repo: &Repo, name: &str, url: &str, push: bool) -> Result<()> {
    if push {
        repo.run(&["remote", "set-url", "--push", name, url.trim()])?;
    } else {
        repo.run(&["remote", "set-url", name, url.trim()])?;
    }
    Ok(())
}

/// Fetch one remote (network).
pub fn fetch_remote(repo: &Repo, name: &str, prune: bool) -> Result<String> {
    let mut args = vec!["fetch"];
    if prune {
        args.push("--prune");
    }
    args.push(name);
    repo.run(&args)?;
    Ok(format!("Fetched {name}"))
}

/// Fetch every remote (network).
pub fn fetch_all(repo: &Repo, prune: bool) -> Result<String> {
    let mut args = vec!["fetch", "--all"];
    if prune {
        args.push("--prune");
    }
    repo.run(&args)?;
    Ok("Fetched all remotes".into())
}

/// Delete remote-tracking branches that no longer exist on the remote (network).
pub fn prune_remote(repo: &Repo, name: &str) -> Result<String> {
    let out = repo.run(&["remote", "prune", name])?;
    let n = out.lines().filter(|l| l.contains("[pruned]")).count();
    Ok(format!(
        "Pruned {n} stale branch{} from {name}",
        if n == 1 { "" } else { "es" }
    ))
}

// ===== worktrees ================================================================================

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Worktree {
    pub path: PathBuf,
    /// Checked-out commit (None for a bare main repository or an unborn branch)
    pub head: Option<String>,
    /// Short branch name (None when detached)
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    /// Lock reason (empty string when locked without a reason)
    pub locked: Option<String>,
    /// Reason the worktree can be pruned (its directory is gone)
    pub prunable: Option<String>,
    /// The main worktree (first in the list)
    pub is_main: bool,
}

pub fn list_worktrees(repo: &Repo) -> Result<Vec<Worktree>> {
    Ok(parse_worktrees(&repo.run(&[
        "worktree",
        "list",
        "--porcelain",
    ])?))
}

fn parse_worktrees(out: &str) -> Vec<Worktree> {
    let mut v: Vec<Worktree> = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        if line.is_empty() {
            v.extend(cur.take());
            continue;
        }
        let (key, val) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            v.extend(cur.take());
            cur = Some(Worktree {
                path: PathBuf::from(val),
                ..Worktree::default()
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match key {
            "HEAD" => w.head = Some(val.to_string()),
            "branch" => w.branch = Some(val.strip_prefix("refs/heads/").unwrap_or(val).to_string()),
            "bare" => w.bare = true,
            "detached" => w.detached = true,
            "locked" => w.locked = Some(val.to_string()),
            "prunable" => w.prunable = Some(val.to_string()),
            _ => {}
        }
    }
    v.extend(cur);
    if let Some(first) = v.first_mut() {
        first.is_main = true;
    }
    v
}

/// How a new worktree gets its checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeCheckout {
    /// Create `branch` starting at `base` (HEAD when empty)
    NewBranch { branch: String, base: String },
    /// Check out an existing local branch
    Existing(String),
    /// Detached HEAD at a revision
    Detached(String),
}

/// `git worktree add` arguments for `path` and `how`.
fn worktree_add_args(path: &str, how: &WorktreeCheckout) -> Vec<String> {
    let mut a: Vec<String> = vec!["worktree".into(), "add".into()];
    let or_head = |s: &str| {
        if s.trim().is_empty() {
            "HEAD".to_string()
        } else {
            s.trim().to_string()
        }
    };
    match how {
        WorktreeCheckout::NewBranch { branch, base } => {
            a.extend([
                "-b".into(),
                branch.trim().into(),
                "--".into(),
                path.into(),
                or_head(base),
            ]);
        }
        WorktreeCheckout::Existing(b) => a.extend(["--".into(), path.into(), b.clone()]),
        WorktreeCheckout::Detached(rev) => {
            a.extend(["--detach".into(), "--".into(), path.into(), or_head(rev)])
        }
    }
    a
}

/// Create a worktree (may take a while: run via `Action::spawn`).
pub fn add_worktree(repo: &Repo, path: &str, how: &WorktreeCheckout) -> Result<String> {
    let args = worktree_add_args(path, how);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.run(&refs)?;
    Ok(format!("Created worktree at {path}"))
}

/// True when the worktree at `path` has uncommitted changes or untracked files.
pub fn worktree_is_dirty(path: &Path) -> bool {
    match Repo::discover(path) {
        Ok(r) => r.status().map(|s| !s.is_clean()).unwrap_or(false),
        Err(_) => false,
    }
}

pub fn remove_worktree(repo: &Repo, path: &Path, force: bool) -> Result<()> {
    let p = path.to_string_lossy();
    if force {
        repo.run(&["worktree", "remove", "--force", "--", &p])?;
    } else {
        repo.run(&["worktree", "remove", "--", &p])?;
    }
    Ok(())
}

pub fn lock_worktree(repo: &Repo, path: &Path, reason: &str) -> Result<()> {
    let p = path.to_string_lossy();
    if reason.trim().is_empty() {
        repo.run(&["worktree", "lock", "--", &p])?;
    } else {
        repo.run(&["worktree", "lock", "--reason", reason.trim(), "--", &p])?;
    }
    Ok(())
}

pub fn unlock_worktree(repo: &Repo, path: &Path) -> Result<()> {
    repo.run(&["worktree", "unlock", "--", &path.to_string_lossy()])?;
    Ok(())
}

/// Remove administrative data of worktrees whose directories are gone.
pub fn prune_worktrees(repo: &Repo) -> Result<String> {
    let out = repo.run(&["worktree", "prune", "-v"])?;
    let n = out.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(format!(
        "Pruned {n} worktree{}",
        if n == 1 { "" } else { "s" }
    ))
}

/// Local branch names (for "add worktree from existing branch").
pub fn local_branches(repo: &Repo) -> Result<Vec<String>> {
    Ok(repo
        .run(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])?
        .lines()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect())
}

/// Compare two paths after resolving symlinks / `..` (falls back to a plain comparison).
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

// ===== submodules ===============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleState {
    /// Not initialized (`-`)
    Uninitialized,
    /// Checked out at the recorded commit
    UpToDate,
    /// Checked out at a different commit than recorded (`+`)
    Modified,
    /// Merge conflict (`U`)
    Conflict,
}

impl SubmoduleState {
    pub fn label(self) -> &'static str {
        match self {
            SubmoduleState::Uninitialized => "not initialized",
            SubmoduleState::UpToDate => "up to date",
            SubmoduleState::Modified => "different commit checked out",
            SubmoduleState::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// Path relative to the work tree root (`/` separators)
    pub path: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub branch: Option<String>,
    pub commit: String,
    pub state: SubmoduleState,
    /// `git describe` of the checked-out commit
    pub describe: Option<String>,
    /// Nesting depth (0 = direct submodule)
    pub depth: usize,
}

/// Submodules (recursively), with URLs from `.gitmodules` for direct ones.
pub fn list_submodules(repo: &Repo) -> Result<Vec<Submodule>> {
    if !repo.path(".gitmodules").exists() {
        return Ok(Vec::new());
    }
    let mut subs = parse_submodule_status(&repo.run(&["submodule", "status", "--recursive"])?);
    let cfg = repo.run_allow(
        &[
            "config",
            "-f",
            ".gitmodules",
            "-z",
            "--get-regexp",
            r"^submodule\..*\.(path|url|branch)$",
        ],
        &[0, 1],
    )?;
    let modules = parse_gitmodules(&cfg);
    for s in subs.iter_mut() {
        if let Some(m) = modules
            .iter()
            .find(|m| m.path.as_deref() == Some(s.path.as_str()))
        {
            s.name = Some(m.name.clone());
            s.url = m.url.clone();
            s.branch = m.branch.clone();
        }
    }
    Ok(subs)
}

fn parse_submodule_status(out: &str) -> Vec<Submodule> {
    out.lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            let mut chars = l.chars();
            let state = match chars.next()? {
                '-' => SubmoduleState::Uninitialized,
                '+' => SubmoduleState::Modified,
                'U' => SubmoduleState::Conflict,
                _ => SubmoduleState::UpToDate,
            };
            let rest = &l[1..];
            let (commit, rest) = rest.split_once(' ')?;
            let (path, describe) = match rest.strip_suffix(')').and_then(|r| r.rsplit_once(" (")) {
                Some((p, d)) => (p, Some(d.to_string())),
                None => (rest, None),
            };
            Some(Submodule {
                path: path.to_string(),
                name: None,
                url: None,
                branch: None,
                commit: commit.to_string(),
                state,
                describe,
                depth: 0,
            })
        })
        .scan(Vec::<String>::new(), |parents, mut s| {
            // Nested submodules follow their parent; depth = number of listed ancestors.
            parents.retain(|p| s.path.starts_with(&format!("{p}/")));
            s.depth = parents.len();
            parents.push(s.path.clone());
            Some(s)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ModuleConfig {
    name: String,
    path: Option<String>,
    url: Option<String>,
    branch: Option<String>,
}

fn parse_gitmodules(out: &str) -> Vec<ModuleConfig> {
    let mut v: Vec<ModuleConfig> = Vec::new();
    for rec in out.split('\0') {
        let Some((key, val)) = rec.trim_start_matches('\n').split_once('\n') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("submodule.") else {
            continue;
        };
        let Some((name, field)) = rest.rsplit_once('.') else {
            continue;
        };
        let idx = match v.iter().position(|m| m.name == name) {
            Some(i) => i,
            None => {
                v.push(ModuleConfig {
                    name: name.to_string(),
                    ..ModuleConfig::default()
                });
                v.len() - 1
            }
        };
        let val = Some(val.to_string());
        match field {
            "path" => v[idx].path = val,
            "url" => v[idx].url = val,
            "branch" => v[idx].branch = val,
            _ => {}
        }
    }
    v
}

pub fn submodule_init(repo: &Repo) -> Result<String> {
    repo.run(&["submodule", "init"])?;
    Ok("Submodules initialized".into())
}

/// `git submodule update --init --recursive` (network).
pub fn submodule_update(repo: &Repo) -> Result<String> {
    repo.run(&["submodule", "update", "--init", "--recursive"])?;
    Ok("Submodules updated".into())
}

pub fn submodule_sync(repo: &Repo) -> Result<String> {
    repo.run(&["submodule", "sync", "--recursive"])?;
    Ok("Submodule URLs synchronized".into())
}

/// Arguments of `git submodule add` (exposed for tests that need extra `-c` options).
fn submodule_add_args(url: &str, path: &str) -> Vec<String> {
    let mut a = vec![
        "submodule".to_string(),
        "add".to_string(),
        "--".to_string(),
        url.trim().to_string(),
    ];
    if !path.trim().is_empty() {
        a.push(path.trim().to_string());
    }
    a
}

/// Add a new submodule (network).
pub fn submodule_add(repo: &Repo, url: &str, path: &str) -> Result<String> {
    let args = submodule_add_args(url, path);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.run(&refs)?;
    Ok(format!(
        "Added submodule {}",
        if path.trim().is_empty() {
            url.trim()
        } else {
            path.trim()
        }
    ))
}

/// Unregister a submodule and remove its work tree (`deinit -f`).
pub fn submodule_deinit(repo: &Repo, path: &str) -> Result<()> {
    repo.run(&["submodule", "deinit", "-f", "--", path])?;
    Ok(())
}

// ===== ignore rules =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreFile {
    /// `<root>/.gitignore` (shared via the repository)
    GitIgnore,
    /// `.git/info/exclude` (local only)
    Exclude,
}

impl IgnoreFile {
    pub fn label(self) -> &'static str {
        match self {
            IgnoreFile::GitIgnore => ".gitignore",
            IgnoreFile::Exclude => ".git/info/exclude",
        }
    }

    pub fn path(self, repo: &Repo) -> PathBuf {
        match self {
            IgnoreFile::GitIgnore => repo.root.join(".gitignore"),
            IgnoreFile::Exclude => repo.common_dir.join("info").join("exclude"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRule {
    pub file: IgnoreFile,
    /// 1-based line number in the file
    pub line: usize,
    pub pattern: String,
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Patterns (not comments/blank lines) of the root .gitignore, then of info/exclude.
pub fn ignore_rules(repo: &Repo) -> Vec<IgnoreRule> {
    let mut v = Vec::new();
    for file in [IgnoreFile::GitIgnore, IgnoreFile::Exclude] {
        v.extend(parse_ignore(&read_text(&file.path(repo)), file));
    }
    v
}

fn parse_ignore(text: &str, file: IgnoreFile) -> Vec<IgnoreRule> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let p = l.trim_end_matches('\r');
            if p.trim().is_empty() || p.starts_with('#') {
                return None;
            }
            Some(IgnoreRule {
                file,
                line: i + 1,
                pattern: p.to_string(),
            })
        })
        .collect()
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// `text` with `pattern` appended as a new line (keeping the file's line endings).
fn with_pattern(text: &str, pattern: &str) -> String {
    let nl = line_ending(text);
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    out.push_str(pattern.trim());
    out.push_str(nl);
    out
}

/// `text` without line `line` (1-based), provided it still holds `pattern`.
fn without_line(text: &str, line: usize, pattern: &str) -> Result<String> {
    let mut parts: Vec<&str> = text.split_inclusive('\n').collect();
    let idx = line.checked_sub(1).filter(|i| *i < parts.len());
    let Some(i) = idx.filter(|i| parts[*i].trim_end_matches(['\r', '\n']) == pattern) else {
        bail!("the ignore file changed on disk; refresh and try again");
    };
    parts.remove(i);
    Ok(parts.concat())
}

pub fn add_ignore_pattern(repo: &Repo, file: IgnoreFile, pattern: &str) -> Result<()> {
    if pattern.trim().is_empty() {
        bail!("pattern must not be empty");
    }
    let path = file.path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = with_pattern(&read_text(&path), pattern);
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

pub fn remove_ignore_rule(repo: &Repo, rule: &IgnoreRule) -> Result<()> {
    let path = rule.file.path(repo);
    let text = without_line(&read_text(&path), rule.line, &rule.pattern)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreMatch {
    /// File defining the rule, as git reports it
    pub source: String,
    pub line: usize,
    pub pattern: String,
}

impl IgnoreMatch {
    /// A matching `!pattern` re-includes the path.
    pub fn ignored(&self) -> bool {
        !self.pattern.starts_with('!')
    }
}

/// Which rule decides whether `path` is ignored (None: no rule matches).
pub fn check_ignore(repo: &Repo, path: &str) -> Result<Option<IgnoreMatch>> {
    let out = repo.run_allow(
        &["check-ignore", "-v", "--no-index", "--", path.trim()],
        &[0, 1],
    )?;
    Ok(out.lines().next().and_then(parse_check_ignore))
}

fn parse_check_ignore(line: &str) -> Option<IgnoreMatch> {
    let (rule, _path) = line.split_once('\t')?;
    let re = regex::Regex::new(r"^(.*?):(\d+):(.*)$").ok()?;
    let c = re.captures(rule)?;
    Some(IgnoreMatch {
        source: c[1].to_string(),
        line: c[2].parse().ok()?,
        pattern: c[3].to_string(),
    })
}

// ===== config ===================================================================================

/// Settings shown in the Config section.
pub const CONFIG_KEYS: [&str; 7] = [
    "user.name",
    "user.email",
    "core.autocrlf",
    "pull.rebase",
    "commit.gpgsign",
    "init.defaultBranch",
    "core.editor",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Local,
    Global,
}

impl ConfigScope {
    fn flag(self) -> &'static str {
        match self {
            ConfigScope::Local => "--local",
            ConfigScope::Global => "--global",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub key: String,
    pub local: Option<String>,
    pub global: Option<String>,
    /// The value git actually uses (any scope)
    pub effective: Option<String>,
}

/// Regex matching exactly [`CONFIG_KEYS`] (git compares against the lowercased canonical names).
fn config_keys_regex() -> String {
    let alts: Vec<String> = CONFIG_KEYS
        .iter()
        .map(|k| regex::escape(&k.to_ascii_lowercase()))
        .collect();
    format!("^({})$", alts.join("|"))
}

/// Parse `git config -z --get-regexp` output (`key\nvalue\0` records; a bare key has no `\n`).
/// For keys set several times the last value wins, as with `git config --get`.
fn parse_config_z(out: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for rec in out.split('\0').filter(|r| !r.is_empty()) {
        let (key, value) = rec.split_once('\n').unwrap_or((rec, ""));
        m.insert(
            key.to_ascii_lowercase(),
            value.trim_end_matches(['\r', '\n']).to_string(),
        );
    }
    m
}

/// Values of [`CONFIG_KEYS`] in one scope (`None`: effective values) with a single git call.
fn config_scope_values(repo: &Repo, scope: Option<&str>, regex: &str) -> HashMap<String, String> {
    let mut args = vec!["config"];
    args.extend(scope);
    args.extend(["-z", "--get-regexp", regex]);
    // Exit code 1: none of the keys is set in this scope.
    repo.run_allow(&args, &[0, 1])
        .map(|out| parse_config_z(&out))
        .unwrap_or_default()
}

/// Repo, global and effective values of [`CONFIG_KEYS`] (three git calls in total).
pub fn config_entries(repo: &Repo) -> Vec<ConfigEntry> {
    let regex = config_keys_regex();
    let local = config_scope_values(repo, Some("--local"), &regex);
    let global = config_scope_values(repo, Some("--global"), &regex);
    let effective = config_scope_values(repo, None, &regex);
    CONFIG_KEYS
        .iter()
        .map(|k| {
            let lk = k.to_ascii_lowercase();
            ConfigEntry {
                key: k.to_string(),
                local: local.get(&lk).cloned(),
                global: global.get(&lk).cloned(),
                effective: effective.get(&lk).cloned(),
            }
        })
        .collect()
}

/// Set `key` in `scope`; an empty value unsets it.
pub fn set_config(repo: &Repo, scope: ConfigScope, key: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        // Exit code 5: the key was not set; nothing to do.
        repo.run_allow(&["config", scope.flag(), "--unset-all", key], &[0, 5])?;
    } else {
        repo.run(&["config", scope.flag(), key, value.trim()])?;
    }
    Ok(())
}

// ===== related repositories =====================================================================

/// Work tree of the superproject when this repository is a submodule.
pub fn superproject(repo: &Repo) -> Option<PathBuf> {
    repo.try_run(&["rev-parse", "--show-superproject-working-tree"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

const SKIP_DIRS: [&str; 12] = [
    ".git",
    "node_modules",
    "target",
    "vendor",
    "build",
    "dist",
    "out",
    ".idea",
    ".gradle",
    ".venv",
    "__pycache__",
    "bower_components",
];

/// Cap on directories visited while scanning, to stay responsive in huge trees.
const MAX_SCAN_DIRS: usize = 20_000;

/// Git repositories nested below `root` (not descending into them), up to `max_depth` levels deep.
pub fn find_nested_repos(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_SCAN_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for e in entries.flatten() {
            // file_type() does not follow symlinks, so link loops are never entered.
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = e.file_name();
            if SKIP_DIRS.iter().any(|s| name == *s) {
                continue;
            }
            subdirs.push(e.path());
        }
        subdirs.sort();
        for d in subdirs.into_iter().rev() {
            if d.join(".git").exists() {
                found.push(d);
            } else if depth + 1 < max_depth {
                stack.push((d, depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// Maximum number of remembered recent repositories.
pub const MAX_RECENT: usize = 20;

/// Where the recent repository list is stored.
pub fn recent_file() -> PathBuf {
    crate::config::Config::dir().join("recent.json")
}

/// Recent repositories (most recent first) that still exist.
pub fn recent_repos() -> Vec<PathBuf> {
    load_recent_in(&recent_file())
}

/// Remember `path` as the most recently opened repository.
pub fn record_recent(path: &Path) -> Result<()> {
    record_recent_in(&recent_file(), path)
}

fn load_recent_in(file: &Path) -> Vec<PathBuf> {
    let list: Vec<PathBuf> = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    list.into_iter().filter(|p| p.is_dir()).collect()
}

fn record_recent_in(file: &Path, path: &Path) -> Result<()> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut list = load_recent_in(file);
    list.retain(|p| !same_path(p, &path));
    list.insert(0, path);
    list.truncate(MAX_RECENT);
    let mut seen = HashSet::new();
    list.retain(|p| seen.insert(p.clone()));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = file.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&list)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

/// Resolve a user-typed directory: `~/` expands to the home directory, relative paths are taken
/// relative to `base`.
pub fn resolve_dir(input: &str, base: &Path) -> PathBuf {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    let p = PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

/// Clone `url` into `dir` (network) and remember it in the recent list.
pub fn clone_repo(repo: &Repo, url: &str, dir: &Path) -> Result<String> {
    if dir.exists()
        && std::fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        bail!("{} already exists and is not empty", dir.display());
    }
    repo.run(&["clone", "--", url.trim(), &dir.to_string_lossy()])?;
    let _ = record_recent(dir);
    Ok(format!(
        "Cloned into {} — open it with \"Switch repository…\"",
        dir.display()
    ))
}

/// `git init` in `dir` (created if needed) and remember it in the recent list.
pub fn init_repo(repo: &Repo, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    repo.run(&["init", "--", &dir.to_string_lossy()])?;
    let _ = record_recent(dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    #[test]
    fn tags_roundtrip() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a", "1\n")], "first");
        commit_files(&repo, &[("a", "2\n")], "second");
        create_tag(&repo, "v1", &c1, "").unwrap();
        create_tag(&repo, "v2", "", "Release two\n\nbody").unwrap();
        assert!(create_tag(&repo, "bad name", "", "").is_err());
        let tags = list_tags(&repo).unwrap();
        assert_eq!(tags.len(), 2);
        let v1 = tags.iter().find(|t| t.name == "v1").unwrap();
        assert!(!v1.annotated);
        assert!(c1.starts_with(&v1.target));
        assert_eq!(v1.subject, "first");
        let v2 = tags.iter().find(|t| t.name == "v2").unwrap();
        assert!(v2.annotated);
        assert_eq!(v2.subject, "Release two");
        assert!(repo.head_commit().unwrap().starts_with(&v2.target));
        checkout_tag(&repo, "v1").unwrap();
        assert!(repo.head_branch().is_none());
        delete_tag(&repo, "v1").unwrap();
        assert_eq!(list_tags(&repo).unwrap().len(), 1);
    }

    #[test]
    fn tag_push_and_remote_delete() {
        let (_r, remote) = temp_repo();
        remote
            .run(&["config", "receive.denyCurrentBranch", "ignore"])
            .unwrap();
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a", "1\n")], "first");
        add_remote(&repo, "origin", &remote.root.to_string_lossy()).unwrap();
        create_tag(&repo, "v1", "", "").unwrap();
        create_tag(&repo, "v2", "", "").unwrap();
        push_tag(&repo, "origin", "v1").unwrap();
        assert_eq!(list_tags(&remote).unwrap().len(), 1);
        push_all_tags(&repo, "origin").unwrap();
        assert_eq!(list_tags(&remote).unwrap().len(), 2);
        delete_remote_tag(&repo, "origin", "v1").unwrap();
        assert_eq!(list_tags(&remote).unwrap()[0].name, "v2");
    }

    #[test]
    fn parse_tag_lines() {
        let t = parse_tags("v1\0tag\0aaaa\0bbbb\x001700000000\0Msg\nlw\0commit\0cccc\0\x001600000000\0Subj\nbroken\n");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].target, "bbbb");
        assert!(t[0].annotated);
        assert_eq!(t[1].target, "cccc");
        assert_eq!(t[1].date, 1_600_000_000);
    }

    #[test]
    fn remotes_lifecycle() {
        let (_d, repo) = temp_repo();
        assert!(list_remotes(&repo).unwrap().is_empty());
        assert_eq!(default_remote(&repo, &[]), None);
        add_remote(&repo, "origin", "https://example.com/a.git").unwrap();
        add_remote(&repo, "up", "git@example.com:b.git").unwrap();
        set_remote_url(&repo, "up", "https://example.com/push.git", true).unwrap();
        let rs = list_remotes(&repo).unwrap();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].fetch_url, "https://example.com/a.git");
        let up = rs.iter().find(|r| r.name == "up").unwrap();
        assert_eq!(up.fetch_url, "git@example.com:b.git");
        assert_eq!(
            up.push_urls,
            vec!["https://example.com/push.git".to_string()]
        );
        assert_eq!(default_remote(&repo, &rs).as_deref(), Some("origin"));
        rename_remote(&repo, "up", "upstream").unwrap();
        remove_remote(&repo, "origin").unwrap();
        let rs = list_remotes(&repo).unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].name, "upstream");
    }

    #[test]
    fn fetch_and_prune_local_remote() {
        let (_r, remote) = temp_repo();
        commit_files(&remote, &[("a", "1\n")], "first");
        remote.run(&["branch", "gone"]).unwrap();
        let (_d, repo) = temp_repo();
        add_remote(&repo, "origin", &remote.root.to_string_lossy()).unwrap();
        fetch_remote(&repo, "origin", false).unwrap();
        assert!(repo.rev_parse("origin/gone").is_ok());
        remote.run(&["branch", "-D", "gone"]).unwrap();
        let msg = prune_remote(&repo, "origin").unwrap();
        assert!(msg.contains("Pruned 1"), "{msg}");
        assert!(repo.rev_parse("origin/gone").is_err());
        fetch_all(&repo, true).unwrap();
    }

    #[test]
    fn parse_remote_lines() {
        let rs = parse_remotes(
            "o\thttps://x/y z.git (fetch)\no\thttps://x/y z.git (push)\no\tssh://m (push)\n",
        );
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].fetch_url, "https://x/y z.git");
        assert_eq!(rs[0].push_urls.len(), 2);
    }

    #[test]
    fn worktrees_lifecycle() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a", "1\n")], "first");
        repo.run(&["branch", "existing"]).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let p1 = parent.path().join("wt new");
        let p2 = parent.path().join("wt-existing");
        let p3 = parent.path().join("wt-detached");
        let s = |p: &PathBuf| p.to_string_lossy().into_owned();
        add_worktree(
            &repo,
            &s(&p1),
            &WorktreeCheckout::NewBranch {
                branch: "feature".into(),
                base: "".into(),
            },
        )
        .unwrap();
        add_worktree(
            &repo,
            &s(&p2),
            &WorktreeCheckout::Existing("existing".into()),
        )
        .unwrap();
        add_worktree(&repo, &s(&p3), &WorktreeCheckout::Detached("HEAD".into())).unwrap();
        let wts = list_worktrees(&repo).unwrap();
        assert_eq!(wts.len(), 4);
        assert!(wts[0].is_main && same_path(&wts[0].path, &repo.root));
        let w1 = wts.iter().find(|w| same_path(&w.path, &p1)).unwrap();
        assert_eq!(w1.branch.as_deref(), Some("feature"));
        assert!(wts.iter().any(|w| same_path(&w.path, &p3) && w.detached));
        lock_worktree(&repo, &p2, "on usb").unwrap();
        let w2 = list_worktrees(&repo)
            .unwrap()
            .into_iter()
            .find(|w| same_path(&w.path, &p2))
            .unwrap();
        assert_eq!(w2.locked.as_deref(), Some("on usb"));
        unlock_worktree(&repo, &p2).unwrap();
        std::fs::write(p1.join("dirty.txt"), "x").unwrap();
        assert!(worktree_is_dirty(&p1));
        assert!(remove_worktree(&repo, &p1, false).is_err());
        remove_worktree(&repo, &p1, true).unwrap();
        std::fs::remove_dir_all(&p3).unwrap();
        assert!(list_worktrees(&repo)
            .unwrap()
            .iter()
            .any(|w| w.prunable.is_some()));
        prune_worktrees(&repo).unwrap();
        assert_eq!(list_worktrees(&repo).unwrap().len(), 2);
        assert_eq!(
            local_branches(&repo).unwrap(),
            vec!["existing", "feature", "main"]
        );
    }

    #[test]
    fn parse_worktree_porcelain() {
        let out = "worktree /a\nHEAD 111\nbranch refs/heads/main\n\nworktree /b c\nHEAD 222\ndetached\nlocked\n\nworktree /bare\nbare\n";
        let w = parse_worktrees(out);
        assert_eq!(w.len(), 3);
        assert!(w[0].is_main && !w[1].is_main);
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert_eq!(w[1].path, PathBuf::from("/b c"));
        assert_eq!(w[1].locked.as_deref(), Some(""));
        assert!(w[2].bare);
    }

    #[test]
    fn submodules_lifecycle() {
        let (_s, sub) = temp_repo();
        commit_files(&sub, &[("lib.txt", "lib\n")], "lib");
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a", "1\n")], "first");
        assert!(list_submodules(&repo).unwrap().is_empty());
        let mut args = vec!["-c".to_string(), "protocol.file.allow=always".to_string()];
        args.extend(submodule_add_args(
            &sub.root.to_string_lossy(),
            "libs/my lib",
        ));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        repo.run(&refs).unwrap();
        let subs = list_submodules(&repo).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].path, "libs/my lib");
        assert_eq!(subs[0].state, SubmoduleState::UpToDate);
        assert_eq!(
            subs[0].url.as_deref(),
            Some(sub.root.to_string_lossy().as_ref())
        );
        assert_eq!(subs[0].name.as_deref(), Some("libs/my lib"));
        submodule_sync(&repo).unwrap();
        submodule_init(&repo).unwrap();
        submodule_deinit(&repo, "libs/my lib").unwrap();
        assert_eq!(
            list_submodules(&repo).unwrap()[0].state,
            SubmoduleState::Uninitialized
        );
    }

    #[test]
    fn parse_submodule_lines() {
        let out = " 1111 a (heads/main)\n+3333 a/x (v1.0-2-g3333)\n-2222 b c\nU4444 d\n";
        let s = parse_submodule_status(out);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].describe.as_deref(), Some("heads/main"));
        assert_eq!(s[2].path, "b c");
        assert_eq!(s[2].state, SubmoduleState::Uninitialized);
        assert_eq!(s[1].state, SubmoduleState::Modified);
        assert_eq!(s[1].depth, 1);
        assert_eq!(s[2].depth, 0);
        assert_eq!(s[3].state, SubmoduleState::Conflict);
        assert_eq!(s[3].depth, 0);
        let m = parse_gitmodules(
            "submodule.x.y.path\np\0submodule.x.y.url\nu v\0submodule.z.branch\ndev\0",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "x.y");
        assert_eq!(m[0].url.as_deref(), Some("u v"));
    }

    #[test]
    fn ignore_rules_edit_and_check() {
        let (_d, repo) = temp_repo();
        std::fs::write(
            repo.path(".gitignore"),
            "# comment\r\n*.log\r\n\r\nbuild/\r\n!keep.log",
        )
        .unwrap();
        let rules = ignore_rules(&repo);
        let gi: Vec<_> = rules
            .iter()
            .filter(|r| r.file == IgnoreFile::GitIgnore)
            .collect();
        assert_eq!(
            gi.iter().map(|r| r.pattern.as_str()).collect::<Vec<_>>(),
            vec!["*.log", "build/", "!keep.log"]
        );
        assert_eq!(gi[1].line, 4);
        add_ignore_pattern(&repo, IgnoreFile::GitIgnore, "tmp/").unwrap();
        let text = std::fs::read_to_string(repo.path(".gitignore")).unwrap();
        assert!(text.ends_with("!keep.log\r\ntmp/\r\n"));
        let rule = ignore_rules(&repo)
            .into_iter()
            .find(|r| r.pattern == "build/")
            .unwrap();
        remove_ignore_rule(&repo, &rule).unwrap();
        assert!(remove_ignore_rule(&repo, &rule).is_err());
        let text = std::fs::read_to_string(repo.path(".gitignore")).unwrap();
        assert_eq!(text, "# comment\r\n*.log\r\n\r\n!keep.log\r\ntmp/\r\n");
        add_ignore_pattern(&repo, IgnoreFile::Exclude, "secret.txt").unwrap();
        assert!(ignore_rules(&repo)
            .iter()
            .any(|r| r.file == IgnoreFile::Exclude && r.pattern == "secret.txt"));

        let m = check_ignore(&repo, "dir/app.log").unwrap().unwrap();
        assert_eq!(m.pattern, "*.log");
        assert_eq!(m.line, 2);
        assert!(m.ignored());
        let m = check_ignore(&repo, "keep.log").unwrap().unwrap();
        assert!(!m.ignored());
        assert!(check_ignore(&repo, "src/main.rs").unwrap().is_none());
        let m = check_ignore(&repo, "secret.txt").unwrap().unwrap();
        assert!(m.source.ends_with("exclude"));
        write(&repo, "x.log", "x");
        assert_eq!(repo.ignored_files().unwrap(), vec!["x.log".to_string()]);
    }

    #[test]
    fn parse_check_ignore_windows_paths() {
        let m = parse_check_ignore("C:/r/.gitignore:12:a:b\tpath with space").unwrap();
        assert_eq!(m.source, "C:/r/.gitignore");
        assert_eq!(m.line, 12);
        assert_eq!(m.pattern, "a:b");
        assert!(without_line("a\nb\n", 3, "x").is_err());
        assert_eq!(with_pattern("", "x"), "x\n");
        assert_eq!(with_pattern("a", "x"), "a\nx\n");
    }

    #[test]
    fn config_local_values() {
        let (_d, repo) = temp_repo();
        let e = config_entries(&repo);
        assert_eq!(e.len(), CONFIG_KEYS.len());
        let name = e.iter().find(|e| e.key == "user.name").unwrap();
        assert_eq!(name.local.as_deref(), Some("Test User"));
        assert_eq!(name.effective.as_deref(), Some("Test User"));
        set_config(&repo, ConfigScope::Local, "pull.rebase", "true").unwrap();
        assert_eq!(repo.config_get("pull.rebase").as_deref(), Some("true"));
        set_config(&repo, ConfigScope::Local, "pull.rebase", "").unwrap();
        let pr = config_entries(&repo)
            .into_iter()
            .find(|e| e.key == "pull.rebase")
            .unwrap();
        assert_eq!(pr.local, None);
        set_config(&repo, ConfigScope::Local, "pull.rebase", "").unwrap();
    }

    #[test]
    fn config_z_parsing() {
        let m = parse_config_z(
            "user.name\nA B\0init.defaultBranch\nmain\r\n\0core.bare\0user.name\nLast\0",
        );
        assert_eq!(m.get("user.name").map(String::as_str), Some("Last"));
        assert_eq!(
            m.get("init.defaultbranch").map(String::as_str),
            Some("main")
        );
        assert_eq!(m.get("core.bare").map(String::as_str), Some(""));
        assert!(parse_config_z("").is_empty());
        let re = regex::Regex::new(&config_keys_regex()).unwrap();
        assert!(re.is_match("init.defaultbranch"));
        assert!(!re.is_match("user.namex"));
        assert!(!re.is_match("user_name"));
    }

    #[test]
    fn config_mixed_case_key_and_multi_value() {
        let (_d, repo) = temp_repo();
        set_config(&repo, ConfigScope::Local, "init.defaultBranch", "trunk").unwrap();
        repo.run(&["config", "--local", "--add", "core.editor", "vi"])
            .unwrap();
        repo.run(&["config", "--local", "--add", "core.editor", "nano -w"])
            .unwrap();
        let e = config_entries(&repo);
        let get = |k: &str| e.iter().find(|x| x.key == k).unwrap().clone();
        assert_eq!(get("init.defaultBranch").local.as_deref(), Some("trunk"));
        assert_eq!(
            get("init.defaultBranch").effective.as_deref(),
            Some("trunk")
        );
        assert_eq!(get("core.editor").local.as_deref(), Some("nano -w"));
        assert_eq!(get("commit.gpgsign").local.as_deref(), Some("false"));
        assert_eq!(get("pull.rebase").local, None);
    }

    #[test]
    fn nested_repo_scan() {
        let (_d, repo) = temp_repo();
        for p in ["a/inner", "b/c/deep", "node_modules/pkg", "x/y/z/too-deep"] {
            let dir = repo.root.join(p);
            std::fs::create_dir_all(dir.join(".git")).unwrap();
        }
        let found = find_nested_repos(&repo.root, 3);
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(&repo.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["a/inner", "b/c/deep"]);
    }

    #[test]
    fn recent_list_dedupes_and_caps() {
        let store = tempfile::tempdir().unwrap();
        let file = store.path().join("sub").join("recent.json");
        let dirs: Vec<tempfile::TempDir> = (0..25).map(|_| tempfile::tempdir().unwrap()).collect();
        for d in &dirs {
            record_recent_in(&file, d.path()).unwrap();
        }
        record_recent_in(&file, dirs[3].path()).unwrap();
        let list = load_recent_in(&file);
        assert_eq!(list.len(), MAX_RECENT);
        assert!(same_path(&list[0], dirs[3].path()));
        assert!(same_path(&list[1], dirs[24].path()));
        assert_eq!(
            list.iter().filter(|p| same_path(p, dirs[3].path())).count(),
            1
        );
        // Paths that no longer exist are dropped.
        let gone = dirs[24].path().to_path_buf();
        drop(dirs);
        assert!(!load_recent_in(&file).iter().any(|p| same_path(p, &gone)));
        std::fs::write(&file, "not json").unwrap();
        assert!(load_recent_in(&file).is_empty());
    }

    #[test]
    fn superproject_and_init() {
        let (_d, repo) = temp_repo();
        assert!(superproject(&repo).is_none());
        let base = tempfile::tempdir().unwrap();
        let target = resolve_dir("new repo", base.path());
        assert_eq!(target, base.path().join("new repo"));
        let p = resolve_dir("/abs", base.path());
        assert!(p.is_absolute() || cfg!(windows));
        let file_store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(&target).unwrap();
        repo.run(&["init", "--", &target.to_string_lossy()])
            .unwrap();
        assert!(Repo::discover(&target).is_ok());
        record_recent_in(&file_store.path().join("r.json"), &target).unwrap();
        assert_eq!(load_recent_in(&file_store.path().join("r.json")).len(), 1);
    }

    #[test]
    fn clone_refuses_non_empty_target() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a", "1\n")], "first");
        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("f"), "x").unwrap();
        assert!(clone_repo(&repo, &repo.root.to_string_lossy(), base.path()).is_err());
    }
}
