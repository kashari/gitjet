mod action;
#[allow(dead_code)] // the runtime half (parse, resolve, run) lands with the `ai` feature
mod ai;
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
    /// Print the AI tool registry as JSON (for the training pipeline)
    #[command(hide = true)]
    DumpTools {
        /// Print only the registry hash
        #[arg(long)]
        hash: bool,
    },
    /// Show which git action the built-in AI model picks for a request (runs nothing)
    #[cfg(feature = "ai")]
    Ask {
        /// What you'd say, e.g. "merge develop into uat and push"
        request: Vec<String>,
        /// Score the model on the built-in golden requests instead
        #[arg(long)]
        golden: bool,
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
        _ => {}
    }

    let cli = Cli::parse();
    let start = match cli.command {
        Some(Cmd::ConfigPath) => {
            println!("{}", config::Config::path().display());
            return Ok(());
        }
        Some(Cmd::DumpTools { hash }) => {
            if hash {
                println!("{}", ai::tools::registry_hash());
            } else {
                println!("{}", serde_json::to_string_pretty(&ai::tools::dump())?);
            }
            return Ok(());
        }
        #[cfg(feature = "ai")]
        Some(Cmd::Ask { request, golden }) => return ask(&request.join(" "), golden),
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

#[cfg(feature = "ai")]
fn ask(request: &str, golden: bool) -> Result<()> {
    use std::time::Instant;
    let t = Instant::now();
    let engine = ai::model::Engine::load()?;
    let mut session = engine.session()?;
    eprintln!("model ready in {:.1}s", t.elapsed().as_secs_f32());
    if !golden {
        if request.trim().is_empty() {
            bail!("usage: gitjet ask <request>");
        }
        let t = Instant::now();
        let (name, args) = session.ask(request)?;
        println!("{name} {args}");
        eprintln!("({:.2}s)", t.elapsed().as_secs_f32());
        return Ok(());
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("ai/testdata/golden_prompts.json"))?;
    let cases = fixture["cases"].as_array().cloned().unwrap_or_default();
    let (mut exact, mut tool) = (0, 0);
    let t = Instant::now();
    for case in &cases {
        let req = case["request"].as_str().unwrap_or_default();
        let (want_name, want) =
            ai::call::parse_raw(case["completion"].as_str().unwrap_or_default())?;
        let got = session.generate(req)?;
        let got = ai::call::parse_raw(&got).map_err(|_| got);
        let free_text = matches!(want_name.as_str(), "clarify" | "unsupported");
        match &got {
            Ok((n, a)) if *n == want_name && (free_text || *a == want) => {
                exact += 1;
                tool += 1;
            }
            other => {
                if matches!(other, Ok((n, _)) if *n == want_name) {
                    tool += 1;
                }
                println!("- {req:?}\n    want {want_name} {want}\n    got  {other:?}");
            }
        }
    }
    let n = cases.len();
    println!(
        "\ngolden: {exact}/{n} exact ({:.1}%), {tool}/{n} right tool, {:.2}s per request",
        100.0 * exact as f32 / n as f32,
        t.elapsed().as_secs_f32() / n as f32
    );
    Ok(())
}
