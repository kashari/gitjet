mod action;
mod app;
mod bench;
mod cli;
mod config;
mod ctx;
mod git;
mod screenshot;
mod theme;
mod ui;
mod util;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gitjet",
    version,
    about = "A keyboard-driven terminal Git client with IDE-grade features"
)]
struct Cli {
    /// Repository to open (defaults to the current directory)
    #[arg(short = 'C', long = "path", value_name = "DIR")]
    path: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Clone a repository, then open it
    Clone { url: String, dir: Option<PathBuf> },
    /// Create a new repository, then open it
    Init { dir: Option<PathBuf> },
    /// Print the location of the config file
    ConfigPath,
    /// Write a config file with all default settings (if none exists)
    ConfigInit,
    /// Cherry-pick commits by hash onto the current branch (or --onto another one)
    Pick {
        /// Commit hashes, in any order; read from standard input when none are given
        revs: Vec<String>,
        /// Put the commits on this branch instead, without switching to it
        #[arg(long, value_name = "BRANCH")]
        onto: Option<String>,
        /// Push the branch afterwards
        #[arg(long)]
        push: bool,
        /// Apply in the order given instead of oldest first
        #[arg(long)]
        keep_order: bool,
        /// Only show what would be done
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Do not ask before running
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Fetch, then fast-forward every local branch that is only behind its upstream
    Sync {
        /// Do not fetch first; use what was fetched before
        #[arg(long)]
        no_fetch: bool,
    },
    /// Delete local branches that are merged into the main branch
    Cleanup {
        /// Also delete branches whose remote branch is gone but that have unmerged commits
        #[arg(long)]
        gone: bool,
        /// Only show what would be deleted
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Do not ask before deleting
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Move the current branch back to before its last operation (merge, commit, reset, rebase...)
    Undo {
        /// Go back to before the n-th last operation (see --list)
        #[arg(default_value_t = 1)]
        steps: usize,
        /// List the last operations instead
        #[arg(long, short = 'l')]
        list: bool,
        /// Only show what would be done
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Do not ask before running
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Fold the staged changes into an earlier commit of the current branch
    Fixup {
        /// The commit that should contain the staged changes
        rev: String,
        /// Do not ask before running
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Revert commits by hash on the current branch, newest first
    Revert {
        /// Commit hashes, in any order; read from standard input when none are given
        revs: Vec<String>,
        /// Push the branch afterwards
        #[arg(long)]
        push: bool,
        /// Only show what would be done
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Do not ask before running
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

fn main() -> Result<()> {
    // Hidden helpers used when gitjet acts as GIT_EDITOR / GIT_SEQUENCE_EDITOR.
    // Handled before clap so git can pass arbitrary trailing arguments.
    let raw: Vec<String> = std::env::args().collect();
    match raw.get(1).map(String::as_str) {
        Some(util::NOOP_EDITOR_ARG) => return Ok(()),
        Some(util::WRITE_FILE_ARG) => {
            let (Some(src), Some(dst)) = (raw.get(2), raw.last()) else {
                bail!("usage: gitjet {} <src> <dst>", util::WRITE_FILE_ARG);
            };
            std::fs::copy(src, dst).with_context(|| format!("copying {src} to {dst}"))?;
            return Ok(());
        }
        Some(screenshot::ARG) => return screenshot::run(&raw[2..]),
        Some(bench::ARG) => {
            return bench::run(std::path::Path::new(raw.get(2).map_or(".", String::as_str)));
        }
        _ => {}
    }

    let cli = Cli::parse();
    let start = match cli.command {
        Some(Cmd::ConfigPath) => {
            println!("{}", config::Config::path().display());
            return Ok(());
        }
        Some(Cmd::Pick {
            revs,
            onto,
            push,
            keep_order,
            dry_run,
            yes,
        }) => {
            let common = cli::Common {
                path: cli.path,
                revs,
                push,
                dry_run,
                yes,
            };
            return cli::pick(&common, onto.as_deref(), keep_order);
        }
        Some(Cmd::Revert {
            revs,
            push,
            dry_run,
            yes,
        }) => {
            let common = cli::Common {
                path: cli.path,
                revs,
                push,
                dry_run,
                yes,
            };
            return cli::revert(&common);
        }
        Some(Cmd::Sync { no_fetch }) => return cli::sync(cli.path.as_deref(), !no_fetch),
        Some(Cmd::Cleanup { gone, dry_run, yes }) => {
            return cli::cleanup(cli.path.as_deref(), gone, dry_run, yes)
        }
        Some(Cmd::Undo {
            steps,
            list,
            dry_run,
            yes,
        }) => return cli::undo(cli.path.as_deref(), steps, list, dry_run, yes),
        Some(Cmd::Fixup { rev, yes }) => return cli::fixup(cli.path.as_deref(), &rev, yes),
        Some(Cmd::ConfigInit) => {
            let path = config::Config::write_default()?;
            println!("{}", path.display());
            return Ok(());
        }
        Some(Cmd::Clone { url, dir }) => {
            let dir = dir.unwrap_or_else(|| PathBuf::from(util::repo_name_from_url(&url)));
            let status = Command::new("git")
                .arg("clone")
                .arg("--progress")
                .arg(&url)
                .arg(&dir)
                .status()
                .context("running git clone (is git installed and on PATH?)")?;
            if !status.success() {
                bail!("git clone failed");
            }
            dir
        }
        Some(Cmd::Init { dir }) => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("."));
            std::fs::create_dir_all(&dir)?;
            let status = Command::new("git")
                .arg("init")
                .arg(&dir)
                .status()
                .context("running git init (is git installed and on PATH?)")?;
            if !status.success() {
                bail!("git init failed");
            }
            dir
        }
        None => cli.path.unwrap_or_else(|| PathBuf::from(".")),
    };

    let repo = git::Repo::discover(&start)?;
    let config = config::Config::load();
    app::run(ctx::Ctx::new(repo, config))
}
