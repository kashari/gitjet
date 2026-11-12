//! Subcommands that do one git task from the shell without opening the UI (`gitjet pick`,
//! `gitjet revert`). They call the same `git::` functions as the dialogs in the UI.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::git::remote::{self, PushSpec};
use crate::git::{pick, upkeep, Repo, RepoState};

/// Options shared by `pick` and `revert`.
pub struct Common {
    pub path: Option<PathBuf>,
    pub revs: Vec<String>,
    pub push: bool,
    pub dry_run: bool,
    pub yes: bool,
}

impl Common {
    fn repo(&self) -> Result<Repo> {
        let repo = Repo::discover(self.path.as_deref().unwrap_or(Path::new(".")))?;
        let state = repo.state();
        if state != RepoState::Clean {
            bail!(
                "the repository is in the middle of an operation ({}); finish or abort it first",
                state.label().to_lowercase()
            );
        }
        Ok(repo)
    }

    /// The revisions from the command line, or from standard input when none are given (or `-`).
    fn revisions(&self) -> Result<Vec<String>> {
        let from_stdin = self.revs.is_empty() || self.revs == ["-"];
        let text = if from_stdin {
            if std::io::stdin().is_terminal() {
                bail!("no commits given: pass hashes as arguments, or pipe a list in");
            }
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        } else {
            self.revs.join(" ")
        };
        Ok(pick::parse_revs(&text))
    }

    fn confirm(&self, question: &str) -> Result<bool> {
        ask(self.yes, question)
    }
}

/// Ask before changing anything, unless `--yes`. A list piped in leaves no terminal to ask on.
fn ask(yes: bool, question: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("not run: pass --yes to run without asking (there is no terminal to ask on)");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn open(path: Option<&Path>) -> Result<Repo> {
    Repo::discover(path.unwrap_or(Path::new(".")))
}

/// What to tell someone at a shell when an operation stopped half-way.
fn stopped_hint(e: anyhow::Error, op: &str) -> anyhow::Error {
    let text = format!("{e:#}");
    if !text.contains("Conflicts tab") {
        return e;
    }
    anyhow::anyhow!(
        "{text}\nOpen gitjet to resolve them, or use `git {op} --continue` / `git {op} --abort`."
    )
}

fn push(repo: &Repo, branch: Option<&str>) -> Result<()> {
    let Some(branch) = branch else {
        bail!("not pushed: HEAD is detached, there is no branch to push");
    };
    println!(
        "{}",
        remote::push(repo, &PushSpec::for_branch(repo, branch)?)?
    );
    Ok(())
}

pub fn pick(c: &Common, onto: Option<&str>, keep_order: bool) -> Result<()> {
    let repo = c.repo()?;
    let plan = pick::plan(&repo, &c.revisions()?, onto, keep_order)?;
    println!("{}", plan.describe());
    if c.dry_run {
        println!("(dry run: nothing was changed)");
        return Ok(());
    }
    if plan.to_apply().is_empty() {
        println!("{}", plan.run(&repo)?);
        return Ok(());
    }
    let question = if c.push {
        "Cherry-pick and push?"
    } else {
        "Cherry-pick?"
    };
    if !c.confirm(question)? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    println!(
        "{}",
        plan.run(&repo)
            .map_err(|e| stopped_hint(e, "cherry-pick"))?
    );
    if c.push {
        let branch = plan.onto.clone().or_else(|| repo.head_branch());
        push(&repo, branch.as_deref())?;
    }
    Ok(())
}

pub fn revert(c: &Common) -> Result<()> {
    let repo = c.repo()?;
    let commits = pick::revert_plan(&repo, &c.revisions()?)?;
    let n = commits.len();
    println!(
        "Revert {n} commit{} on {}:",
        if n == 1 { "" } else { "s" },
        repo.head_branch()
            .unwrap_or_else(|| "the detached HEAD".into())
    );
    for commit in &commits {
        println!(
            "  {} {} — {}",
            commit.short,
            crate::util::truncate(&commit.subject, 60),
            commit.author
        );
    }
    if c.dry_run {
        println!("(dry run: nothing was changed)");
        return Ok(());
    }
    let question = if c.push {
        "Revert and push?"
    } else {
        "Revert?"
    };
    if !c.confirm(question)? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    println!(
        "{}",
        pick::revert(&repo, &commits).map_err(|e| stopped_hint(e, "revert"))?
    );
    if c.push {
        push(&repo, repo.head_branch().as_deref())?;
    }
    Ok(())
}

/// `gitjet sync`: fetch, then fast-forward every local branch that is only behind.
pub fn sync(path: Option<&Path>, fetch: bool) -> Result<()> {
    let repo = open(path)?;
    let prune = crate::config::Config::load().update.prune;
    println!("{}", upkeep::sync_branches(&repo, fetch, prune)?.describe());
    Ok(())
}

/// `gitjet cleanup`: delete local branches that are merged or whose remote branch is gone.
pub fn cleanup(path: Option<&Path>, gone: bool, dry_run: bool, yes: bool) -> Result<()> {
    let repo = open(path)?;
    let (base, stale) = upkeep::stale_branches(&repo)?;
    if stale.is_empty() {
        println!(
            "Nothing to clean up: no local branch is merged into {base} or lost its remote branch."
        );
        return Ok(());
    }
    let (delete, keep): (Vec<_>, Vec<_>) = stale.into_iter().partition(|s| s.merged || gone);
    if !delete.is_empty() {
        println!("Delete:");
        for s in &delete {
            println!("  {}", s.describe());
        }
    }
    if !keep.is_empty() {
        println!(
            "Kept, because they have commits that are not in {base} (--gone deletes them too):"
        );
        for s in &keep {
            println!("  {}", s.describe());
        }
    }
    if dry_run {
        println!("(dry run: nothing was changed)");
        return Ok(());
    }
    if delete.is_empty() {
        return Ok(());
    }
    let n = delete.len();
    let question = format!("Delete {n} branch{}?", if n == 1 { "" } else { "es" });
    if !ask(yes, &question)? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    println!("{}", upkeep::delete_stale(&repo, &delete, gone)?);
    Ok(())
}

/// `gitjet undo`: move the current branch back to before one of its last operations.
pub fn undo(path: Option<&Path>, steps: usize, list: bool, dry_run: bool, yes: bool) -> Result<()> {
    let repo = open(path)?;
    let points = upkeep::undo_points(&repo, steps.max(10))?;
    if points.is_empty() {
        bail!("there is nothing to undo on this branch");
    }
    if list {
        for (i, p) in points.iter().enumerate() {
            println!(
                "{:>3}  {}  ({})",
                i + 1,
                p.what,
                crate::util::relative_time(p.when)
            );
        }
        println!("`gitjet undo <n>` goes back to before operation n.");
        return Ok(());
    }
    let Some(point) = points.get(steps.max(1) - 1) else {
        bail!("only {} operations are known for this branch", points.len());
    };
    let branch = repo
        .head_branch()
        .unwrap_or_else(|| "the detached HEAD".into());
    println!(
        "Move {branch} back to {} (before: {}).",
        &point.target[..point.target.len().min(8)],
        point.what
    );
    println!("Uncommitted changes are kept. Running `gitjet undo` again brings this state back.");
    if dry_run {
        println!("(dry run: nothing was changed)");
        return Ok(());
    }
    if !ask(yes, "Undo?")? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    println!("{}", upkeep::undo_to(&repo, point)?);
    Ok(())
}

/// `gitjet fixup <hash>`: fold the staged changes into an earlier commit.
pub fn fixup(path: Option<&Path>, rev: &str, yes: bool) -> Result<()> {
    let repo = open(path)?;
    let commit = repo
        .rev_parse(rev)
        .and_then(|id| repo.commit_info(&id))
        .map_err(|_| anyhow::anyhow!("{rev} is not a commit in this repository"))?;
    println!(
        "Fold the staged changes into {} {}.",
        commit.short,
        crate::util::truncate(&commit.subject, 60)
    );
    println!("This rewrites that commit and every commit after it.");
    if !ask(yes, "Fix up?")? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    println!("{}", upkeep::fixup_into(&repo, &commit.id)?);
    Ok(())
}
