mod action;
mod app;
mod config;
mod ctx;
mod git;
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
        _ => {}
    }

    let cli = Cli::parse();
    let start = match cli.command {
        Some(Cmd::ConfigPath) => {
            println!("{}", config::Config::path().display());
            return Ok(());
        }
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
