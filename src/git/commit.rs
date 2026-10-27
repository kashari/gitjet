//! Creating commits: amend, sign-off, author override, GPG signing, message history, commit checks.
//!
//! Also the file-level operations of the Commit tool window: stage/unstage, rollback, deleting
//! unversioned files and adding them to `.gitignore`.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use super::status::{FileState, StatusEntry};
use super::Repo;

/// Maximum number of remembered commit messages.
pub const HISTORY_LIMIT: usize = 50;

/// Everything the commit dialog decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitRequest {
    pub message: String,
    pub amend: bool,
    pub sign_off: bool,
    pub gpg_sign: bool,
    /// `Name <email>` overriding the configured author
    pub author: Option<String>,
    /// Commit exactly these paths' work-tree state instead of the index (`git commit --only`)
    pub paths: Option<Vec<String>>,
    /// Paths among `paths` that are not tracked yet (they are added first)
    pub untracked: Vec<String>,
}

/// NUL-separated pathspec input for `--pathspec-from-file=- --pathspec-file-nul`.
fn nul_paths<S: AsRef<str>>(paths: &[S]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in paths {
        v.extend_from_slice(p.as_ref().as_bytes());
        v.push(0);
    }
    v
}

/// Run a git subcommand with literal pathspecs read from stdin. `args` start with the subcommand.
fn run_with_paths<S: AsRef<str>>(repo: &Repo, args: &[&str], paths: &[S]) -> Result<String> {
    let mut full = vec!["--literal-pathspecs"];
    full.extend_from_slice(args);
    full.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    repo.run_stdin(&full, &nul_paths(paths))
}

/// First line of a message.
pub fn subject(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim_end_matches('\r')
}

/// Whether the subject line exceeds `limit` characters.
pub fn subject_too_long(message: &str, limit: usize) -> bool {
    limit > 0 && subject(message).chars().count() > limit
}

/// Check an author override of the form `Name <email>`.
pub fn validate_author(author: &str) -> Result<()> {
    let a = author.trim();
    let ok = match (a.find('<'), a.rfind('>')) {
        (Some(lt), Some(gt)) => {
            lt > 0 && gt == a.len() - 1 && lt < gt && !a[..lt].trim().is_empty()
        }
        _ => false,
    };
    if !ok {
        bail!("Author must look like \"Name <email>\"");
    }
    Ok(())
}

/// Whether commits are signed by default: gitjet config or git's `commit.gpgSign`.
pub fn gpg_sign_default(repo: &Repo, configured: bool) -> bool {
    configured
        || repo
            .config_get("commit.gpgsign")
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1"))
}

/// Full message of HEAD (for amend), None in an empty repository.
pub fn last_message(repo: &Repo) -> Option<String> {
    repo.head_commit()?;
    repo.try_run(&["log", "-1", "--format=%B", "HEAD"])
        .map(|s| s.trim_end().to_string())
}

/// Create the commit. Returns a one-line summary like `Committed 1a2b3c4: subject`.
pub fn commit(repo: &Repo, req: &CommitRequest) -> Result<String> {
    let message = req.message.replace("\r\n", "\n");
    if message.trim().is_empty() {
        bail!("Commit message is empty");
    }
    if let Some(a) = &req.author {
        validate_author(a)?;
    }
    let head = repo.head_commit();
    if req.amend && head.is_none() {
        bail!("There is no commit to amend yet");
    }
    let msg_file = repo.data_dir()?.join("COMMIT_MSG");
    std::fs::write(&msg_file, format!("{}\n", message.trim_end()))
        .with_context(|| format!("writing {}", msg_file.display()))?;
    let res = run_commit(repo, req, &msg_file, head.is_some());
    let _ = std::fs::remove_file(&msg_file);
    res?;
    let _ = remember_message(repo, &message);
    let line = repo.run(&["log", "-1", "--format=%h %s", "HEAD"])?;
    Ok(format!("Committed {}", line.trim().replacen(' ', ": ", 1)))
}

fn run_commit(repo: &Repo, req: &CommitRequest, msg_file: &Path, has_head: bool) -> Result<()> {
    let file = msg_file.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec!["commit".into(), "-F".into(), file];
    if req.amend {
        args.push("--amend".into());
    }
    if req.sign_off {
        args.push("--signoff".into());
    }
    args.push(if req.gpg_sign {
        "-S".into()
    } else {
        "--no-gpg-sign".into()
    });
    if let Some(a) = &req.author {
        args.push(format!("--author={}", a.trim()));
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match &req.paths {
        Some(paths) if !paths.is_empty() => {
            if !req.untracked.is_empty() {
                run_with_paths(repo, &["add", "-A"], &req.untracked)?;
            }
            if has_head {
                let mut a = argv.clone();
                a.push("--only");
                run_with_paths(repo, &a, paths)?;
            } else {
                // A partial commit needs a HEAD to start from; in an empty repository nothing else is
                // staged (otherwise the index would have been committed), so staging is equivalent.
                run_with_paths(repo, &["add", "-A"], paths)?;
                repo.run(&argv)?;
            }
        }
        _ => {
            repo.run(&argv)?;
        }
    }
    Ok(())
}

// ----- message history -------------------------------------------------------------------------

/// Put `msg` at the front of `history` (deduplicated, capped at [`HISTORY_LIMIT`]).
pub fn push_message(history: &mut Vec<String>, msg: &str) {
    let m = msg.trim_end().to_string();
    if m.trim().is_empty() {
        return;
    }
    history.retain(|h| h != &m);
    history.insert(0, m);
    history.truncate(HISTORY_LIMIT);
}

/// Previously used commit messages, newest first.
pub fn load_history(repo: &Repo) -> Vec<String> {
    repo.data_dir()
        .ok()
        .and_then(|d| std::fs::read_to_string(d.join("commit_history.json")).ok())
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

/// Add a message to the stored history.
pub fn remember_message(repo: &Repo, msg: &str) -> Result<()> {
    let mut h = load_history(repo);
    push_message(&mut h, msg);
    let path = repo.data_dir()?.join("commit_history.json");
    std::fs::write(&path, serde_json::to_string_pretty(&h)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ----- commit checks ---------------------------------------------------------------------------

/// Result of running the configured commit checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub passed: bool,
    /// Combined output of every check, with headers
    pub output: String,
}

/// Run `(name, command)` checks through the platform shell in the repository root, in order.
pub fn run_checks(repo: &Repo, checks: &[(String, String)]) -> CheckReport {
    let mut output = String::new();
    let mut passed = true;
    for (name, cmd) in checks {
        output.push_str(&format!("$ {name}: {cmd}\n"));
        let res = crate::util::shell(cmd)
            .current_dir(&repo.root)
            .stdin(std::process::Stdio::null())
            .output();
        match res {
            Ok(out) => {
                output.push_str(&String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"));
                output.push_str(&String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"));
                if out.status.success() {
                    output.push_str("✓ passed\n\n");
                } else {
                    passed = false;
                    output.push_str(&format!("✗ failed ({})\n\n", out.status));
                }
            }
            Err(e) => {
                passed = false;
                output.push_str(&format!("✗ could not run: {e}\n\n"));
            }
        }
    }
    CheckReport { passed, output }
}

// ----- staging ---------------------------------------------------------------------------------

/// Stage the given paths (additions, modifications and deletions; marks conflicts resolved).
pub fn stage_paths(repo: &Repo, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    run_with_paths(repo, &["add", "-A"], paths)?;
    Ok(())
}

/// Remove the given paths from the index, keeping work-tree content.
pub fn unstage_paths(repo: &Repo, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if repo.head_commit().is_some() {
        run_with_paths(repo, &["restore", "--staged"], paths)?;
    } else {
        run_with_paths(
            repo,
            &["rm", "--cached", "-r", "-q", "--force", "--ignore-unmatch"],
            paths,
        )?;
    }
    Ok(())
}

pub fn stage_all(repo: &Repo) -> Result<()> {
    repo.run(&["add", "-A"])?;
    Ok(())
}

pub fn unstage_all(repo: &Repo) -> Result<()> {
    if repo.head_commit().is_some() {
        repo.run(&["reset", "-q"])?;
    } else {
        repo.run(&["read-tree", "--empty"])?;
    }
    Ok(())
}

// ----- rollback / unversioned ------------------------------------------------------------------

/// Whether the entry is a file that does not exist in HEAD (rolled back by deleting it).
fn is_added(e: &StatusEntry) -> bool {
    e.index == FileState::Added || e.worktree == FileState::Added
}

/// Revert entries to their HEAD state: tracked files are restored, added files are removed from the
/// index and disk, untracked files are deleted, renames are undone.
pub fn rollback(repo: &Repo, entries: &[StatusEntry]) -> Result<()> {
    let has_head = repo.head_commit().is_some();
    let mut restore: Vec<String> = Vec::new();
    let mut unindex: Vec<String> = Vec::new();
    let mut delete: Vec<String> = Vec::new();
    let mut conflicted: Vec<String> = Vec::new();
    for e in entries {
        if e.is_untracked() {
            delete.push(e.path.clone());
        } else if e.is_conflicted() {
            conflicted.push(e.path.clone());
        } else if !has_head || is_added(e) || e.index == FileState::Copied {
            unindex.push(e.path.clone());
            delete.push(e.path.clone());
        } else if e.index == FileState::Renamed {
            unindex.push(e.path.clone());
            delete.push(e.path.clone());
            if let Some(o) = &e.orig_path {
                restore.push(o.clone());
            }
        } else {
            restore.push(e.path.clone());
        }
    }
    if !unindex.is_empty() {
        run_with_paths(
            repo,
            &["rm", "--cached", "-r", "-q", "--force", "--ignore-unmatch"],
            &unindex,
        )?;
    }
    if !restore.is_empty() {
        run_with_paths(
            repo,
            &["restore", "--source=HEAD", "--staged", "--worktree"],
            &restore,
        )?;
    }
    if !conflicted.is_empty() {
        if has_head {
            run_with_paths(repo, &["checkout", "HEAD"], &conflicted)?;
        } else {
            unindex_and_delete(repo, &conflicted)?;
        }
    }
    delete_files(repo, &delete)
}

fn unindex_and_delete(repo: &Repo, paths: &[String]) -> Result<()> {
    run_with_paths(
        repo,
        &["rm", "--cached", "-r", "-q", "--force", "--ignore-unmatch"],
        paths,
    )?;
    delete_files(repo, paths)
}

/// Delete work-tree files (missing files are fine) and prune directories left empty.
pub fn delete_files(repo: &Repo, paths: &[String]) -> Result<()> {
    for p in paths {
        let abs = repo.path(p);
        match std::fs::symlink_metadata(&abs) {
            Ok(m) if m.is_dir() => {
                std::fs::remove_dir_all(&abs).with_context(|| format!("deleting {p}"))?
            }
            Ok(_) => std::fs::remove_file(&abs).with_context(|| format!("deleting {p}"))?,
            Err(_) => continue,
        }
        let mut dir = abs.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if d == repo.root || !d.starts_with(&repo.root) || std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    Ok(())
}

/// A root-anchored `.gitignore` pattern matching exactly `path`.
pub fn gitignore_pattern(path: &str) -> String {
    let mut s = String::from("/");
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    if s.ends_with(' ') {
        let trimmed = s.trim_end_matches(' ').len();
        let spaces = s.len() - trimmed;
        s.truncate(trimmed);
        for _ in 0..spaces {
            s.push_str("\\ ");
        }
    }
    s
}

/// Append `pattern` to the root `.gitignore` (keeps its line endings; no duplicates).
pub fn add_to_gitignore(repo: &Repo, pattern: &str) -> Result<()> {
    let pattern = pattern.trim_end_matches(['\r', '\n']);
    if pattern.trim().is_empty() {
        return Err(anyhow!("Empty .gitignore pattern"));
    }
    let path = repo.path(".gitignore");
    // Work on bytes so a .gitignore that is not valid UTF-8 is preserved untouched;
    // only a missing file counts as empty.
    let existing = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    let pat = pattern.as_bytes();
    if existing
        .split(|&c| c == b'\n')
        .any(|l| l.strip_suffix(b"\r").unwrap_or(l) == pat)
    {
        return Ok(());
    }
    let eol: &[u8] = if existing.windows(2).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut tail = Vec::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        tail.extend_from_slice(eol);
    }
    tail.extend_from_slice(pat);
    tail.extend_from_slice(eol);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    std::io::Write::write_all(&mut file, &tail)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::*;

    fn req(msg: &str) -> CommitRequest {
        CommitRequest {
            message: msg.into(),
            ..Default::default()
        }
    }

    fn files_in(repo: &Repo, rev: &str) -> Vec<String> {
        repo.run(&["ls-tree", "-r", "--name-only", rev])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn first_commit_and_multiline_message() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        stage_all(&repo).unwrap();
        let out = commit(
            &repo,
            &req("Subject line\n\nBody with \"quotes\" and ünïcode\r\nsecond"),
        )
        .unwrap();
        assert!(out.starts_with("Committed "), "{out}");
        assert!(out.ends_with(": Subject line"));
        let msg = last_message(&repo).unwrap();
        assert_eq!(
            msg,
            "Subject line\n\nBody with \"quotes\" and ünïcode\nsecond"
        );
        assert_eq!(load_history(&repo)[0], msg);
    }

    #[test]
    fn pathspec_commit_in_empty_repo_and_after() {
        let (_d, repo) = temp_repo();
        write(&repo, "x y.txt", "x\n");
        write(&repo, "other.txt", "o\n");
        let r = CommitRequest {
            paths: Some(vec!["x y.txt".into()]),
            untracked: vec!["x y.txt".into()],
            ..req("only x")
        };
        commit(&repo, &r).unwrap();
        assert_eq!(files_in(&repo, "HEAD"), vec!["x y.txt"]);
        // With HEAD: commit one modified + one untracked file, leave the rest.
        write(&repo, "x y.txt", "x2\n");
        write(&repo, "n[1].txt", "n\n");
        let r = CommitRequest {
            paths: Some(vec!["x y.txt".into(), "n[1].txt".into()]),
            untracked: vec!["n[1].txt".into()],
            ..req("two")
        };
        commit(&repo, &r).unwrap();
        assert_eq!(files_in(&repo, "HEAD"), vec!["n[1].txt", "x y.txt"]);
        assert!(repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.path == "other.txt" && e.is_untracked()));
    }

    #[test]
    fn amend_without_changes_signoff_and_author() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a", "1\n")], "old");
        let before = repo.head_commit().unwrap();
        let r = CommitRequest {
            amend: true,
            sign_off: true,
            author: Some("Someone Else <else@example.com>".into()),
            ..req("new message")
        };
        commit(&repo, &r).unwrap();
        assert_ne!(repo.head_commit().unwrap(), before);
        assert_eq!(
            repo.run(&["rev-list", "--count", "HEAD"]).unwrap().trim(),
            "1"
        );
        let msg = last_message(&repo).unwrap();
        assert!(msg.starts_with("new message"));
        assert!(msg.contains("Signed-off-by: Test User <test@example.com>"));
        assert_eq!(
            repo.run(&["log", "-1", "--format=%an"]).unwrap().trim(),
            "Someone Else"
        );
    }

    #[test]
    fn rejects_bad_input() {
        let (_d, repo) = temp_repo();
        assert!(commit(&repo, &req("  \n")).is_err());
        assert!(commit(
            &repo,
            &CommitRequest {
                amend: true,
                ..req("x")
            }
        )
        .is_err());
        assert!(validate_author("Name <a@b>").is_ok());
        assert!(validate_author("<a@b>").is_err());
        assert!(validate_author("Name a@b").is_err());
        assert!(validate_author("Name <a@b> x").is_err());
        assert!(subject_too_long(&"x".repeat(73), 72));
        assert!(!subject_too_long(
            "short\nvery long body line that does not matter at all for the subject check",
            10
        ));
    }

    #[test]
    fn history_dedupes_and_caps() {
        let mut h = Vec::new();
        for i in 0..60 {
            push_message(&mut h, &format!("m{i}"));
        }
        assert_eq!(h.len(), HISTORY_LIMIT);
        assert_eq!(h[0], "m59");
        push_message(&mut h, "m50");
        assert_eq!(h[0], "m50");
        assert_eq!(h.iter().filter(|m| *m == "m50").count(), 1);
        push_message(&mut h, "   ");
        assert_eq!(h[0], "m50");
    }

    #[test]
    fn stage_unstage_paths_and_all() {
        let (_d, repo) = temp_repo();
        write(&repo, "a", "1\n");
        stage_paths(&repo, &["a".into()]).unwrap();
        assert!(repo.status().unwrap().entries[0].is_staged());
        unstage_paths(&repo, &["a".into()]).unwrap();
        assert!(repo.status().unwrap().entries[0].is_untracked());
        stage_all(&repo).unwrap();
        unstage_all(&repo).unwrap();
        assert!(repo.status().unwrap().entries[0].is_untracked());
        commit_files(&repo, &[("a", "1\n")], "init");
        write(&repo, "a", "2\n");
        std::fs::remove_file(repo.path("a")).unwrap();
        stage_paths(&repo, &["a".into()]).unwrap();
        assert_eq!(repo.status().unwrap().entries[0].index, FileState::Deleted);
        unstage_paths(&repo, &["a".into()]).unwrap();
        assert!(!repo.status().unwrap().entries[0].is_staged());
        stage_all(&repo).unwrap();
        unstage_all(&repo).unwrap();
        assert!(!repo.status().unwrap().entries[0].is_staged());
    }

    #[test]
    fn rollback_all_kinds() {
        let (_d, repo) = temp_repo();
        commit_files(
            &repo,
            &[("m.txt", "m\n"), ("d.txt", "d\n"), ("old.txt", "o\n")],
            "init",
        );
        write(&repo, "m.txt", "changed\n");
        repo.run(&["add", "m.txt"]).unwrap();
        write(&repo, "m.txt", "changed again\n");
        std::fs::remove_file(repo.path("d.txt")).unwrap();
        write(&repo, "added/new.txt", "n\n");
        repo.run(&["add", "added/new.txt"]).unwrap();
        write(&repo, "u/untracked.txt", "u\n");
        repo.run(&["mv", "old.txt", "renamed.txt"]).unwrap();
        let st = repo.status().unwrap();
        rollback(&repo, &st.entries).unwrap();
        assert!(
            repo.status().unwrap().is_clean(),
            "{:?}",
            repo.status().unwrap().entries
        );
        assert_eq!(std::fs::read_to_string(repo.path("m.txt")).unwrap(), "m\n");
        assert!(repo.path("d.txt").exists());
        assert!(!repo.path("added").exists());
        assert!(!repo.path("u").exists());
        assert!(repo.path("old.txt").exists() && !repo.path("renamed.txt").exists());
    }

    #[test]
    fn rollback_in_empty_repo() {
        let (_d, repo) = temp_repo();
        write(&repo, "a.txt", "1\n");
        write(&repo, "b.txt", "2\n");
        repo.run(&["add", "a.txt"]).unwrap();
        let st = repo.status().unwrap();
        rollback(&repo, &st.entries).unwrap();
        assert!(repo.status().unwrap().is_clean());
        assert!(!repo.path("a.txt").exists());
    }

    #[test]
    fn gitignore_patterns() {
        assert_eq!(gitignore_pattern("build/out.log"), "/build/out.log");
        assert_eq!(gitignore_pattern("a[1]*.txt"), "/a\\[1]\\*.txt");
        assert_eq!(gitignore_pattern("#x"), "/#x");
        assert_eq!(gitignore_pattern("t "), "/t\\ ");
        let (_d, repo) = temp_repo();
        write(&repo, ".gitignore", "target\r\n*.tmp");
        write(&repo, "a b.log", "x\n");
        write(&repo, "c[1].log", "x\n");
        add_to_gitignore(&repo, &gitignore_pattern("a b.log")).unwrap();
        add_to_gitignore(&repo, &gitignore_pattern("c[1].log")).unwrap();
        add_to_gitignore(&repo, &gitignore_pattern("a b.log")).unwrap();
        let text = std::fs::read_to_string(repo.path(".gitignore")).unwrap();
        assert_eq!(text, "target\r\n*.tmp\r\n/a b.log\r\n/c\\[1].log\r\n");
        let st = repo.status().unwrap();
        assert!(
            st.entries.iter().all(|e| e.path == ".gitignore"),
            "{:?}",
            st.entries
        );
    }

    #[test]
    fn gitignore_keeps_non_utf8_content() {
        let (_d, repo) = temp_repo();
        let latin1: &[u8] = b"# caf\xe9 rules\nbuild/\n";
        std::fs::write(repo.path(".gitignore"), latin1).unwrap();
        add_to_gitignore(&repo, "/new.log").unwrap();
        let bytes = std::fs::read(repo.path(".gitignore")).unwrap();
        assert_eq!(bytes, [latin1, b"/new.log\n"].concat());
        // Missing file: created from scratch.
        std::fs::remove_file(repo.path(".gitignore")).unwrap();
        add_to_gitignore(&repo, "/x").unwrap();
        assert_eq!(std::fs::read(repo.path(".gitignore")).unwrap(), b"/x\n");
    }

    #[test]
    fn checks_report_failures() {
        let (_d, repo) = temp_repo();
        let ok = run_checks(&repo, &[("echo".into(), "echo hello".into())]);
        assert!(ok.passed);
        assert!(ok.output.contains("hello"));
        let bad = run_checks(
            &repo,
            &[
                ("fail".into(), "exit 3".into()),
                ("echo".into(), "echo after".into()),
            ],
        );
        assert!(!bad.passed);
        assert!(bad.output.contains("failed"));
        assert!(bad.output.contains("after"));
    }

    #[test]
    fn delete_prunes_empty_dirs() {
        let (_d, repo) = temp_repo();
        write(&repo, "a/b/c.txt", "x\n");
        write(&repo, "a/keep.txt", "x\n");
        delete_files(&repo, &["a/b/c.txt".into(), "missing.txt".into()]).unwrap();
        assert!(!repo.path("a/b").exists());
        assert!(repo.path("a/keep.txt").exists());
    }

    #[test]
    fn gpg_default_reads_git_config() {
        let (_d, repo) = temp_repo();
        assert!(!gpg_sign_default(&repo, false));
        assert!(gpg_sign_default(&repo, true));
        repo.run(&["config", "commit.gpgsign", "true"]).unwrap();
        assert!(gpg_sign_default(&repo, false));
    }
}
