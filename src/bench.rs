//! `gitjet __bench [dir]`: time the loads behind the slowest screens, without the terminal.
//!
//! Hidden; for comparing a change before and after on the same repository. Every line is the
//! best of three runs, in milliseconds.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;

use crate::git::graph;
use crate::git::log::LogQuery;
use crate::git::Repo;

pub const ARG: &str = "__bench";

fn time<T>(name: &str, mut f: impl FnMut() -> T) -> T {
    let mut best = f64::MAX;
    let mut last = None;
    for _ in 0..3 {
        let t = Instant::now();
        last = Some(f());
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("{best:>10.1} ms  {name}");
    last.expect("ran three times")
}

pub fn run(dir: &Path) -> Result<()> {
    let repo = time("discover repository", || Repo::discover(dir))?;
    let page = crate::config::Config::load()
        .log
        .page_size
        .clamp(50, 10_000);

    println!("-- startup and status bar");
    time("status bar (branch, state, ahead/behind)", || {
        crate::app::load_head(&repo).branch.len()
    });
    let status = time("status (Changes tab)", || repo.status())?;
    println!("{:>13}  {} changed files", "", status.entries.len());

    println!("-- log");
    let all = |skip: usize, max: usize| LogQuery {
        all: true,
        skip,
        max,
        ..LogQuery::default()
    };
    time(&format!("log first page ({page} commits)"), || {
        repo.log(&all(0, page)).map(|c| c.len())
    })?;
    time("log page 20", || {
        repo.log(&all(page * 19, page)).map(|c| c.len())
    })?;
    time(
        "log pages 1 to 20, a new git log per page (before 0.3.0)",
        || {
            (0..20)
                .map(|i| repo.log(&all(page * i, page)).map_or(0, |c| c.len()))
                .sum::<usize>()
        },
    );
    time("log pages 1 to 20, one git log kept open", || {
        let mut stream = repo.log_stream(&all(0, page))?;
        let mut n = 0;
        for _ in 0..20 {
            n += stream.next_page(page)?.len();
        }
        anyhow::Ok(n)
    })?;
    time("log first page, topological order", || {
        repo.log_stream(&all(0, page))?
            .next_page(page)
            .map(|c| c.len())
    })?;
    let by_date = LogQuery {
        topo: false,
        ..all(0, page)
    };
    time("log first page, date order (default from 0.3.0)", || {
        repo.log_stream(&by_date)?.next_page(page).map(|c| c.len())
    })?;
    time("log pages 1 to 20, date order", || {
        let mut stream = repo.log_stream(&by_date)?;
        let mut n = 0;
        for _ in 0..20 {
            n += stream.next_page(page)?.len();
        }
        anyhow::Ok(n)
    })?;
    let commits = time("log 10,000 commits in one query", || {
        repo.log(&all(0, 10_000))
    })?;
    time("graph layout, 10,000 commits", || {
        graph::layout(&commits).len()
    });
    time("graph layout, 20 pages laid out as they arrive", || {
        (1..=20)
            .map(|i| graph::layout(&commits[..(page * i).min(commits.len())]).len())
            .sum::<usize>()
    });

    println!("-- diff");
    if let Some(c) = commits.iter().find(|c| !c.is_merge()) {
        let opts = crate::git::diff::DiffOptions::default();
        time("diff of one commit", || {
            repo.diff_commit(&c.id, &[], &opts).map(|d| d.len())
        })?;
    }
    time("syntax definitions (first diff only)", || {
        syntect::parsing::SyntaxSet::load_defaults_newlines()
            .syntaxes()
            .len()
    });
    Ok(())
}
