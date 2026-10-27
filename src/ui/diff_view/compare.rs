//! Comparison dialogs offered in the action palette: compare with a branch/tag/revision, compare
//! two files, show local/staged/unstaged changes.

pub mod chooser;

use std::path::PathBuf;

use super::{open, short_rev, DiffSource};
use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::compare::{
    changed_paths, display_path, list_refs, resolve_file, resolve_revision, since_merge_base,
};
use crate::ui::popups::{InputPopup, MenuItem, MenuPopup};
use crate::ui::PaletteItem;
use chooser::{Choice, RefChooser};

/// Refs offered in the chooser; anything else can be typed in.
const MAX_REFS: usize = 1000;

/// Palette entries of the diff feature.
pub fn palette() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            name: "Compare with branch or revision…",
            run: compare_with_ref,
        },
        PaletteItem {
            name: "Compare two files…",
            run: compare_files,
        },
        PaletteItem {
            name: "Show all local changes",
            run: show_local_changes,
        },
        PaletteItem {
            name: "Show staged changes",
            run: show_staged_changes,
        },
        PaletteItem {
            name: "Show unstaged changes",
            run: show_unstaged_changes,
        },
    ]
}

/// Chooser of branches and tags (plus free text) to compare the current state with. Typed text
/// that matches no ref stays selectable as a revision expression.
pub fn compare_with_ref(ctx: &Ctx) -> Action {
    let refs = match list_refs(&ctx.repo) {
        Ok(r) => r,
        Err(e) => return Action::error(format!("{e:#}")),
    };
    let refs = refs.into_iter().take(MAX_REFS).collect();
    Action::popup(RefChooser::new(
        "Compare with branch or revision",
        refs,
        on_choice,
    ))
}

fn on_choice(ctx: &Ctx, c: Choice) -> Action {
    match c {
        Choice::EnterRevision => enter_revision(),
        Choice::Rev(rev) => compare_options(ctx, &rev),
    }
}

fn enter_revision() -> Action {
    Action::popup(
        InputPopup::new("Compare with revision", "", |rev, ctx| {
            compare_options(ctx, rev.trim())
        })
        .prompt("Branch, tag, commit hash or expression (HEAD~3, origin/main, v1.2^{}...)"),
    )
}

/// What to compare against `rev`: the work tree, or branch changes since the merge base.
pub fn compare_options(ctx: &Ctx, rev: &str) -> Action {
    if let Err(e) = resolve_revision(&ctx.repo, rev) {
        return Action::error(format!("{e:#}"));
    }
    let rev = rev.to_string();
    let shown = short_rev(&rev);
    let has_head = ctx.repo.head_commit().is_some();
    let head = ctx.repo.head_branch().unwrap_or_else(|| "HEAD".into());
    let r1 = rev.clone();
    let r2 = rev.clone();
    let r3 = rev.clone();
    let r4 = rev;
    let items = vec![
        MenuItem::new(format!("Working tree vs {shown}"), "w", move |_| {
            open(
                format!("Working tree vs {}", short_rev(&r1)),
                DiffSource::RevToWorktree {
                    rev: r1,
                    paths: Vec::new(),
                },
            )
        }),
        MenuItem::new(
            format!("Changes on {shown} since it diverged from {head}"),
            "b",
            move |ctx| match since_merge_base(&ctx.repo, &r2) {
                Ok((from, to)) => open(
                    format!("{} since merge base", short_rev(&to)),
                    DiffSource::Range {
                        from,
                        to,
                        paths: Vec::new(),
                    },
                ),
                Err(e) => Action::error(format!("{e:#}")),
            },
        )
        .enabled(has_head),
        MenuItem::new(
            format!("Changes on {head} since it diverged from {shown}"),
            "c",
            move |ctx| match since_merge_base(&ctx.repo, &r3) {
                Ok((from, _)) => open(
                    "HEAD since merge base".to_string(),
                    DiffSource::Range {
                        from,
                        to: "HEAD".into(),
                        paths: Vec::new(),
                    },
                ),
                Err(e) => Action::error(format!("{e:#}")),
            },
        )
        .enabled(has_head),
        MenuItem::new(format!("{head} vs {shown} (direct)"), "d", move |_| {
            open(
                format!("HEAD vs {}", short_rev(&r4)),
                DiffSource::Range {
                    from: "HEAD".into(),
                    to: r4,
                    paths: Vec::new(),
                },
            )
        })
        .enabled(has_head),
    ];
    Action::popup(MenuPopup::new(format!("Compare with {shown}"), items))
}

const FILE_PROMPT: &str = "Path relative to the repository root, absolute, or ~/...";

/// Ask for two files and diff them.
pub fn compare_files(_ctx: &Ctx) -> Action {
    Action::popup(first_file(String::new()))
}

fn first_file(initial: String) -> InputPopup {
    InputPopup::new(
        "Compare files — first file",
        &initial,
        |input, ctx| match resolve_file(&ctx.repo, &input) {
            Ok(left) => Action::popup(second_file(left, String::new(), ctx)),
            Err(e) => Action::popup(first_file(input)).then(Action::error(format!("{e:#}"))),
        },
    )
    .prompt(FILE_PROMPT)
}

fn second_file(left: PathBuf, initial: String, ctx: &Ctx) -> InputPopup {
    let shown = display_path(&ctx.repo, &left);
    InputPopup::new(
        "Compare files — second file",
        &initial,
        move |input, ctx| match resolve_file(&ctx.repo, &input) {
            Ok(right) => {
                let title = format!(
                    "{} ↔ {}",
                    display_path(&ctx.repo, &left),
                    display_path(&ctx.repo, &right)
                );
                open(title, DiffSource::Files { left, right })
            }
            Err(e) => {
                Action::popup(second_file(left, input, ctx)).then(Action::error(format!("{e:#}")))
            }
        },
    )
    .prompt(format!("Compare {shown} with:\n{FILE_PROMPT}"))
}

/// All local changes (staged, unstaged and untracked) against HEAD.
pub fn show_local_changes(ctx: &Ctx) -> Action {
    match changed_paths(&ctx.repo) {
        Ok(p) if p.is_empty() => Action::Toast("No local changes".into()),
        Ok(paths) => open("Local changes", DiffSource::Local { paths }),
        Err(e) => Action::error(format!("{e:#}")),
    }
}

/// Index vs HEAD.
pub fn show_staged_changes(ctx: &Ctx) -> Action {
    match ctx.repo.status() {
        Ok(st) if !st.entries.iter().any(|e| e.is_staged()) => {
            Action::Toast("Nothing is staged".into())
        }
        Ok(_) => open("Staged changes", DiffSource::Staged { paths: Vec::new() }),
        Err(e) => Action::error(format!("{e:#}")),
    }
}

/// Work tree vs index, including untracked files.
pub fn show_unstaged_changes(ctx: &Ctx) -> Action {
    match ctx.repo.status() {
        Ok(st) => {
            let paths: Vec<String> = st
                .entries
                .iter()
                .filter(|e| e.is_unstaged() && !e.is_ignored())
                .map(|e| e.path.clone())
                .collect();
            if paths.is_empty() {
                Action::Toast("No unstaged changes".into())
            } else {
                open("Unstaged changes", DiffSource::Unstaged { paths })
            }
        }
        Err(e) => Action::error(format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::git::testutil::*;

    fn ctx_with(repo: crate::git::Repo) -> Ctx {
        Ctx::new(repo, Config::default())
    }

    #[test]
    fn palette_actions_produce_dialogs_or_toasts() {
        let (_d, repo) = temp_repo();
        let ctx = ctx_with(repo.clone());
        assert!(matches!(show_local_changes(&ctx), Action::Toast(_)));
        assert!(matches!(show_staged_changes(&ctx), Action::Toast(_)));
        assert!(matches!(show_unstaged_changes(&ctx), Action::Toast(_)));
        assert!(matches!(compare_with_ref(&ctx), Action::Popup(_)));
        assert!(matches!(compare_files(&ctx), Action::Popup(_)));
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        write(&repo, "a.txt", "2\n");
        write(&repo, "u.txt", "u\n");
        assert!(matches!(show_local_changes(&ctx), Action::Popup(_)));
        assert!(matches!(show_unstaged_changes(&ctx), Action::Popup(_)));
        repo.run(&["add", "a.txt"]).unwrap();
        assert!(matches!(show_staged_changes(&ctx), Action::Popup(_)));
    }

    #[test]
    fn compare_options_validate_revision() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        let ctx = ctx_with(repo);
        assert!(matches!(compare_options(&ctx, "nope"), Action::Error(_)));
        assert!(matches!(compare_options(&ctx, "main"), Action::Popup(_)));
        assert!(matches!(compare_options(&ctx, "HEAD~0"), Action::Popup(_)));
        assert!(matches!(
            on_choice(&ctx, Choice::EnterRevision),
            Action::Popup(_)
        ));
        assert!(matches!(
            on_choice(&ctx, Choice::Rev("nope".into())),
            Action::Error(_)
        ));
    }
}
