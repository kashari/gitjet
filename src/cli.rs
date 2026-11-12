//! Subcommands that do one git task from the shell without opening the UI (`gitjet pick`,
//! `gitjet revert`). They call the same `git::` functions as the dialogs in the UI.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::git::remote::{self, PushSpec};
use crate::git::{pick, Repo, RepoState};

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

    /// Ask before changing anything, unless `--yes`. A list piped in leaves no terminal to ask on.
    fn confirm(&self, question: &str) -> Result<bool> {
        if self.yes {
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
