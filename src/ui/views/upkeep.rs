//! Palette entries for everyday housekeeping: sync all branches, clean up branches, undo the
//! last operation. The work is in `git::upkeep`.

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::upkeep::{self, Stale, UndoPoint};
use crate::ui::popups::{ConfirmPopup, MenuItem, MenuPopup, MessagePopup};
use crate::ui::PaletteItem;
use crate::util::{relative_time, truncate};

pub fn palette() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            name: "Sync all branches",
            run: sync,
        },
        PaletteItem {
            name: "Clean up branches…",
            run: clean_up,
        },
        PaletteItem {
            name: "Undo last operation…",
            run: undo,
        },
    ]
}

/// Fetch, then fast-forward every local branch that is only behind its upstream.
fn sync(ctx: &Ctx) -> Action {
    let prune = ctx.config.update.prune;
    Action::spawn("Sync branches", move |repo| {
        upkeep::sync_branches(repo, true, prune).map(|r| r.describe())
    })
}

/// List the branches that are merged or lost their remote branch; delete the merged ones.
fn clean_up(ctx: &Ctx) -> Action {
    Action::run(
        &ctx.repo,
        "Looking for branches to clean up",
        upkeep::stale_branches,
        |found| match found {
            Ok((base, stale)) => confirm_clean_up(&base, stale),
            Err(e) => Action::Error(format!("{e:#}")),
        },
    )
}

fn confirm_clean_up(base: &str, stale: Vec<Stale>) -> Action {
    let (delete, keep): (Vec<Stale>, Vec<Stale>) = stale.into_iter().partition(|s| s.merged);
    let list = |items: &[Stale]| {
        items
            .iter()
            .map(|s| format!("  {}", s.describe()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let kept = if keep.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nKept, because they have commits that are not in {base}\n(delete them in the Branches tab if you are sure):\n{}",
            list(&keep)
        )
    };
    if delete.is_empty() {
        let text = if keep.is_empty() {
            format!("Nothing to clean up: no local branch is merged into {base}.")
        } else {
            format!("No local branch is merged into {base}.{kept}")
        };
        return Action::popup(MessagePopup::new("Clean up branches", text));
    }
    let n = delete.len();
    let message = format!(
        "Delete {n} merged branch{}?\n{}{kept}",
        if n == 1 { "" } else { "es" },
        list(&delete)
    );
    Action::popup(ConfirmPopup::new("Clean up branches", message, move |_| {
        Action::spawn("Clean up branches", move |repo| {
            upkeep::delete_stale(repo, &delete, false)
        })
    }))
}

/// Pick one of the branch's last operations and go back to before it.
fn undo(ctx: &Ctx) -> Action {
    Action::run(
        &ctx.repo,
        "Reading the last operations",
        |repo| (upkeep::undo_points(repo, 15), repo.head_branch()),
        |(points, branch)| match points {
            Ok(points) if points.is_empty() => {
                Action::Error("There is nothing to undo on this branch".into())
            }
            Ok(points) => undo_menu(points, branch.unwrap_or_else(|| "HEAD".into())),
            Err(e) => Action::Error(format!("{e:#}")),
        },
    )
}

fn undo_menu(points: Vec<UndoPoint>, branch: String) -> Action {
    let items = points
        .into_iter()
        .map(|p| {
            let label = format!(
                "before {}  ({})",
                truncate(&p.what, 70),
                relative_time(p.when)
            );
            let branch = branch.clone();
            MenuItem::new(label, "", move |_| confirm_undo(p, &branch))
        })
        .collect();
    Action::popup(MenuPopup::new(format!("Move {branch} back to…"), items))
}

fn confirm_undo(point: UndoPoint, branch: &str) -> Action {
    let short = &point.target[..point.target.len().min(8)];
    let message = format!(
        "Move {branch} back to {short}, where it was before:\n  {}\n\nUncommitted changes are kept. This is itself an operation you can undo.",
        point.what
    );
    Action::popup(
        ConfirmPopup::new("Undo", message, move |_| {
            Action::spawn("Undo", move |repo| upkeep::undo_to(repo, &point))
        })
        .danger(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn popup_text(a: Action, ctx: &Ctx) -> String {
        let popup = match a {
            Action::Popup(p) => Some(p),
            Action::Batch(v) => v.into_iter().find_map(|a| match a {
                Action::Popup(p) => Some(p),
                _ => None,
            }),
            _ => None,
        };
        let mut p = popup.expect("a popup");
        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        term.draw(|f| p.draw(f, f.area(), ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..24)
            .map(|y| (0..110).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn clean_up_asks_with_the_merged_branches_and_undo_lists_operations() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "first");
        repo.run(&["checkout", "-q", "-b", "done"]).unwrap();
        commit_files(&repo, &[("d.txt", "d\n")], "done work");
        repo.run(&["checkout", "-q", "main"]).unwrap();
        repo.run(&["merge", "-q", "--no-ff", "-m", "merge done", "done"])
            .unwrap();
        let ctx = Ctx::new(repo, Config::default());

        let s = popup_text(clean_up(&ctx), &ctx);
        assert!(s.contains("Delete 1 merged branch?"), "{s}");
        assert!(s.contains("done (merged)"), "{s}");

        let s = popup_text(undo(&ctx), &ctx);
        assert!(s.contains("Move main back to"), "{s}");
        assert!(s.contains("before merge done"), "{s}");
        assert!(!s.contains("years ago"), "{s}");
    }
}
