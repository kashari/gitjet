//! Commit and file context menus of the log, and the history operations behind them (shared with the
//! action palette).

use std::collections::{HashMap, HashSet};

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::history::{self, ResetMode};
use crate::git::log::Commit;
use crate::ui::diff_view::{self, DiffSource};
use crate::ui::popups::{ConfirmPopup, InputPopup, MenuItem, MenuPopup, TextAreaPopup};
use crate::ui::views::{files, rebase};
use crate::util::truncate;

use super::clipboard;
use super::details::ChangedFile;
use super::mailbox::{post, Msg};

/// What the commit menu acts on.
pub struct Selection {
    /// The commit under the cursor
    pub commit: Commit,
    /// Marked commits in log order (newest first); empty when nothing is marked
    pub marked: Vec<Commit>,
    /// Full hash of HEAD
    pub head: Option<String>,
    /// Remote branches that contain HEAD, computed in the background with the log (`None` = unknown)
    pub head_pushed: Option<Vec<String>>,
    /// Whether the selected commit is HEAD or one of its ancestors, when the loaded commits tell
    /// (`None` = unknown: filters hide commits; it is then checked when an action needs it)
    pub on_branch: Option<bool>,
}

impl Selection {
    /// Commits for multi-commit operations: the marked ones, or the selected one.
    fn targets(&self) -> Vec<Commit> {
        if self.marked.is_empty() {
            vec![self.commit.clone()]
        } else {
            self.marked.clone()
        }
    }
}

fn describe(c: &Commit) -> String {
    format!("{} \"{}\"", c.short, truncate(&c.subject, 50))
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Base for "rebase from here" / rewording: the commit's parent, or the whole history for roots.
pub fn rebase_base(c: &Commit) -> String {
    if c.parents.is_empty() {
        "--root".into()
    } else {
        format!("{}^", c.id)
    }
}

/// True when `commits[to]` is reachable from `commits[from]` through the parents of loaded commits.
/// `commits` must be in topological order (children first) with nothing hidden in between, so only
/// the commits between the two positions need to be walked.
pub fn reaches(commits: &[Commit], from: usize, to: usize) -> bool {
    if from == to {
        return true;
    }
    if from > to || to >= commits.len() {
        return false;
    }
    let index: HashMap<&str, usize> = commits[from..=to]
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), from + i))
        .collect();
    let mut seen: HashSet<usize> = HashSet::from([from]);
    let mut stack = vec![from];
    while let Some(i) = stack.pop() {
        for p in &commits[i].parents {
            match index.get(p.as_str()) {
                Some(&j) if j == to => return true,
                Some(&j) if seen.insert(j) => stack.push(j),
                _ => {}
            }
        }
    }
    false
}

/// The `.` / `m` menu for the selected (or marked) commits.
pub fn commit_menu(sel: Selection) -> Action {
    let c = sel.commit.clone();
    let targets = sel.targets();
    let n = targets.len();
    let is_head = sel.head.as_deref() == Some(c.id.as_str());
    let on_branch = is_head || sel.on_branch.unwrap_or(true);
    let verify_branch = !is_head && sel.on_branch.is_none();
    let head_pushed = sel.head_pushed.clone();
    let hashes = targets
        .iter()
        .map(|t| t.id.clone())
        .collect::<Vec<_>>()
        .join(" ");
    let many = n > 1;

    let mut items = vec![MenuItem::new(
        if many {
            format!("Copy {} hashes", n)
        } else {
            "Copy revision hash".into()
        },
        "c",
        move |_| clipboard::copy(&hashes),
    )];
    let c1 = c.clone();
    items.push(MenuItem::new(
        "Checkout revision (detached HEAD)…",
        "o",
        move |_| checkout(c1),
    ));
    let c1 = c.clone();
    items.push(MenuItem::new("New branch here…", "b", move |_| {
        new_branch(c1)
    }));
    let c1 = c.clone();
    items.push(MenuItem::new("New tag here…", "t", move |_| new_tag(c1)));
    let picks = targets.clone();
    items.push(MenuItem::new(
        if many {
            format!("Cherry-pick {}", plural(n, "commit"))
        } else {
            "Cherry-pick".into()
        },
        "p",
        move |_| cherry_pick(picks),
    ));
    let reverts = targets.clone();
    items.push(MenuItem::new(
        if many {
            format!("Revert {}", plural(n, "commit"))
        } else {
            "Revert commit".into()
        },
        "r",
        move |_| revert(reverts),
    ));
    let c1 = c.clone();
    items.push(MenuItem::new(
        "Reset current branch to here…",
        "R",
        move |ctx| reset_menu(ctx, &c1.id),
    ));
    let pushed = head_pushed.clone();
    items.push(
        MenuItem::new("Undo last commit", "u", move |ctx| {
            undo_head_with(ctx, pushed)
        })
        .enabled(is_head),
    );
    let c1 = c.clone();
    items.push(
        MenuItem::new("Edit commit message…", "e", move |ctx| {
            on_branch_or(ctx, &c1, verify_branch, |ctx| {
                edit_message(ctx, &c1, is_head, head_pushed)
            })
        })
        .enabled(on_branch),
    );
    let c1 = c.clone();
    items.push(
        MenuItem::new("Interactively rebase from here…", "i", move |ctx| {
            on_branch_or(ctx, &c1, verify_branch, |ctx| {
                rebase::open(ctx, &rebase_base(&c1))
            })
        })
        .enabled(on_branch),
    );
    let c1 = c.clone();
    items.push(MenuItem::new("Show diff", "d", move |_| show_diff(&c1)));
    let c1 = c.clone();
    items.push(
        MenuItem::new("Compare with previous", "P", move |_| compare_previous(&c1))
            .enabled(!c.parents.is_empty()),
    );
    let c1 = c.clone();
    items.push(MenuItem::new("Compare with local", "l", move |_| {
        compare_local(&c1, Vec::new())
    }));
    if let [newer, older] = sel.marked.as_slice() {
        let (newer, older) = (newer.clone(), older.clone());
        items.push(MenuItem::new(
            "Compare the two marked commits",
            "C",
            move |_| {
                diff_view::open(
                    format!("{} → {}", older.short, newer.short),
                    DiffSource::Range {
                        from: older.id.clone(),
                        to: newer.id.clone(),
                        paths: Vec::new(),
                    },
                )
            },
        ));
    }
    let id = c.id.clone();
    items.push(MenuItem::new("Mark / unmark", "v", move |_| {
        post(Msg::ToggleMark(id));
        Action::None
    }));
    let (name, email) = (c.author.clone(), c.email.clone());
    items.push(MenuItem::new(
        format!("Show only commits by {}", truncate(&c.author, 30)),
        "a",
        move |_| {
            post(Msg::Edit(Box::new(move |f| {
                f.user = Some(super::filters::UserFilter {
                    label: name.clone(),
                    pattern: format!("{name} <{email}>"),
                })
            })));
            Action::None
        },
    ));
    let title = if many {
        format!("{} marked", plural(n, "commit"))
    } else {
        describe(&c)
    };
    Action::popup(MenuPopup::new(title, items))
}

/// Run `then` unless `verify` is set and `c` turns out not to be on the current branch (checked only
/// when the user picks the action and the loaded log could not tell).
fn on_branch_or(ctx: &Ctx, c: &Commit, verify: bool, then: impl FnOnce(&Ctx) -> Action) -> Action {
    if verify && !history::is_ancestor_of_head(&ctx.repo, &c.id) {
        return Action::Error(format!("{} is not on the current branch", c.short));
    }
    then(ctx)
}

/// The `.` / `m` menu for a file in the details pane.
pub fn file_menu(c: &Commit, f: &ChangedFile) -> Action {
    let (c1, f1) = (c.clone(), f.clone());
    let mut items = vec![MenuItem::new("Show diff", "d", move |_| {
        file_diff(&c1, &f1)
    })];
    let (c1, f1) = (c.clone(), f.clone());
    items.push(MenuItem::new("Compare with local", "l", move |_| {
        compare_local(&c1, f1.diff_paths())
    }));
    let (c1, f1) = (c.clone(), f.clone());
    items.push(MenuItem::new(
        "Annotate (blame) at this revision",
        "b",
        move |ctx| blame(ctx, &c1, &f1),
    ));
    let p = f.path.clone();
    items.push(MenuItem::new("Show history", "h", move |ctx| {
        files::open_file_history(ctx, &p)
    }));
    let p = f.path.clone();
    items.push(MenuItem::new("Copy path", "c", move |_| {
        clipboard::copy(&p)
    }));
    Action::popup(MenuPopup::new(truncate(&f.path, 60), items))
}

/// Full diff of one commit.
pub fn show_diff(c: &Commit) -> Action {
    diff_view::open(
        describe(c),
        DiffSource::Commit {
            rev: c.id.clone(),
            paths: Vec::new(),
        },
    )
}

/// Diff of one file in a commit.
pub fn file_diff(c: &Commit, f: &ChangedFile) -> Action {
    diff_view::open(
        format!("{} @ {}", f.path, c.short),
        DiffSource::Commit {
            rev: c.id.clone(),
            paths: f.diff_paths(),
        },
    )
}

/// The commit's first parent compared with the commit.
pub fn compare_previous(c: &Commit) -> Action {
    match c.parents.first() {
        Some(p) => diff_view::open(
            format!("{}^ → {}", c.short, c.short),
            DiffSource::Range {
                from: p.clone(),
                to: c.id.clone(),
                paths: Vec::new(),
            },
        ),
        None => Action::Error(format!(
            "{} is a root commit and has no previous revision",
            c.short
        )),
    }
}

/// The commit compared with the work tree.
pub fn compare_local(c: &Commit, paths: Vec<String>) -> Action {
    diff_view::open(
        format!("{} → local", c.short),
        DiffSource::RevToWorktree {
            rev: c.id.clone(),
            paths,
        },
    )
}

/// Blame a file as of this commit (deleted files are blamed in the parent).
pub fn blame(ctx: &Ctx, c: &Commit, f: &ChangedFile) -> Action {
    if f.status == 'D' {
        match c.parents.first() {
            Some(p) => files::open_blame(ctx, f.old_path.as_deref().unwrap_or(&f.path), Some(p)),
            None => Action::Error(format!("{} does not exist in {}", f.path, c.short)),
        }
    } else {
        files::open_blame(ctx, &f.path, Some(&c.id))
    }
}

fn checkout(c: Commit) -> Action {
    let msg = format!(
        "Check out {} as a detached HEAD?\n\nNew commits made there belong to no branch unless you create one.",
        describe(&c)
    );
    Action::popup(ConfirmPopup::new("Checkout revision", msg, move |_| {
        let id = c.id.clone();
        Action::spawn("Checkout", move |repo| {
            history::checkout_detached(repo, &id)
        })
    }))
}

fn new_branch(c: Commit) -> Action {
    Action::popup(
        InputPopup::new(format!("New branch at {}", c.short), "", move |name, _| {
            let id = c.id.clone();
            Action::spawn("Create branch", move |repo| {
                history::create_branch_at(repo, &name, &id, true)
            })
        })
        .prompt("The new branch is checked out right away"),
    )
}

fn new_tag(c: Commit) -> Action {
    Action::popup(InputPopup::new(
        format!("New tag at {}", c.short),
        "",
        move |name, _| {
            let short = c.short.clone();
            let id = c.id.clone();
            Action::popup(
                InputPopup::new(
                    format!("Tag {} at {short}: message", name.trim()),
                    "",
                    move |message, _| {
                        Action::spawn("Create tag", move |repo| {
                            history::create_tag_at(repo, &name, &id, Some(&message))
                        })
                    },
                )
                .prompt("A message creates an annotated tag; leave empty for a lightweight tag")
                .allow_empty(),
            )
        },
    ))
}

/// Cherry-pick `commits` (given newest first, applied oldest first).
pub fn cherry_pick(commits: Vec<Commit>) -> Action {
    let revs: Vec<String> = commits.iter().rev().map(|c| c.id.clone()).collect();
    Action::spawn("Cherry-pick", move |repo| history::cherry_pick(repo, &revs))
}

/// Revert `commits` (given newest first, reverted newest first so later changes go first). Whether a
/// commit is a merge is read from git, not from the loaded parents (a path filter rewrites those).
pub fn revert(commits: Vec<Commit>) -> Action {
    let revs: Vec<String> = commits.iter().map(|c| c.id.clone()).collect();
    Action::spawn("Revert", move |repo| {
        let mut last = String::new();
        for rev in &revs {
            let merge = history::is_merge(repo, rev)?;
            last = history::revert(repo, rev, merge)?;
        }
        Ok(if revs.len() == 1 {
            last
        } else {
            format!("Reverted {} commits", revs.len())
        })
    })
}

/// Choose a reset mode for moving the current branch to `rev`.
pub fn reset_menu(ctx: &Ctx, rev: &str) -> Action {
    let branch = ctx.repo.head_branch().unwrap_or_else(|| "HEAD".into());
    let short = ctx
        .repo
        .try_run(&["rev-parse", "--short", rev])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| rev.to_string());
    let items = ResetMode::ALL
        .iter()
        .map(|&mode| {
            let key = match mode {
                ResetMode::Soft => "s",
                ResetMode::Mixed => "m",
                ResetMode::Hard => "h",
                ResetMode::Keep => "K",
            };
            let (rev, branch, short) = (rev.to_string(), branch.clone(), short.clone());
            MenuItem::new(format!("{:<6} {}", mode.label(), mode.description()), key, move |_| {
                let run = move || Action::spawn("Reset", move |repo| history::reset(repo, mode, &rev));
                if mode == ResetMode::Hard {
                    Action::popup(
                        ConfirmPopup::new(
                            "Hard reset",
                            format!(
                                "Reset {branch} to {short} and DISCARD all uncommitted changes?\n\nThis cannot be undone."
                            ),
                            move |_| run(),
                        )
                        .danger(),
                    )
                } else {
                    run()
                }
            })
        })
        .collect();
    Action::popup(MenuPopup::new(format!("Reset {branch} to {short}"), items))
}

/// Undo HEAD, keeping its changes staged; asks first when the commit was already pushed (palette entry:
/// only the branch's upstream and push target are checked, which is cheap).
pub fn undo_head(ctx: &Ctx) -> Action {
    undo_head_with(ctx, None)
}

/// Where HEAD is pushed: the list computed with the log when there is one, else the cheap upstream check.
fn head_pushed_or_upstream(ctx: &Ctx, head: &str, known: Option<Vec<String>>) -> Vec<String> {
    known.unwrap_or_else(|| history::pushed_to_upstream(&ctx.repo, head))
}

/// [`undo_head`] with the remote branches containing HEAD already known (`None` = check upstream).
pub fn undo_head_with(ctx: &Ctx, pushed: Option<Vec<String>>) -> Action {
    let Some(head) = ctx.repo.head_commit() else {
        return Action::Error("Nothing to undo: there are no commits yet".into());
    };
    let run = || Action::spawn("Undo commit", history::undo_last_commit);
    match head_pushed_or_upstream(ctx, &head, pushed) {
        remotes if !remotes.is_empty() => Action::popup(
            ConfirmPopup::new(
                "Undo pushed commit",
                format!(
                    "The last commit is already pushed to {}.\n\nUndoing it locally means the branch must be force-pushed later. Undo anyway?",
                    remotes.join(", ")
                ),
                move |_| run(),
            )
            .danger(),
        ),
        _ => run(),
    }
}

/// Reword a commit: HEAD in place (amend), older commits through interactive rebase. `pushed` lists the
/// remote branches containing HEAD when known.
pub fn edit_message(ctx: &Ctx, c: &Commit, is_head: bool, pushed: Option<Vec<String>>) -> Action {
    if !is_head {
        return rebase::open(ctx, &rebase_base(c));
    }
    let message = match ctx.repo.commit_message(&c.id) {
        Ok(m) => m,
        Err(e) => return Action::error(format!("{e:#}")),
    };
    let pushed = !head_pushed_or_upstream(ctx, &c.id, pushed).is_empty();
    let title = if pushed {
        "Edit commit message (already pushed: needs force push)"
    } else {
        "Edit commit message"
    };
    Action::popup(
        TextAreaPopup::new(title, &message, |text, _| {
            Action::spawn("Edit message", move |repo| {
                history::amend_head_message(repo, &text)
            })
        })
        .subject_limit(ctx.config.commit.subject_limit),
    )
}

/// Palette: reset the current branch to a typed revision.
pub fn reset_prompt(_ctx: &Ctx) -> Action {
    Action::popup(
        InputPopup::new("Reset current branch to", "HEAD~1", |rev, ctx| {
            match ctx.repo.rev_parse(rev.trim()) {
                Ok(id) => reset_menu(ctx, &id),
                Err(_) => Action::Error(format!("Unknown revision: {}", rev.trim())),
            }
        })
        .prompt("Revision, branch or tag"),
    )
}

/// Resolve whitespace-separated revisions to commits.
fn resolve_all(ctx: &Ctx, text: &str) -> Result<Vec<Commit>, String> {
    let revs: Vec<&str> = text.split_whitespace().collect();
    if revs.is_empty() {
        return Err("no revision given".into());
    }
    revs.iter()
        .map(|r| {
            ctx.repo
                .rev_parse(r)
                .and_then(|id| ctx.repo.commit_info(&id))
                .map_err(|_| format!("Unknown revision: {r}"))
        })
        .collect()
}

/// Palette: cherry-pick typed revisions (in the order given).
pub fn cherry_pick_prompt(_ctx: &Ctx) -> Action {
    Action::popup(
        InputPopup::new("Cherry-pick", "", |text, ctx| {
            match resolve_all(ctx, &text) {
                Ok(mut commits) => {
                    commits.reverse();
                    cherry_pick(commits)
                }
                Err(e) => Action::Error(e),
            }
        })
        .prompt("Revisions to apply onto the current branch, oldest first, separated by spaces"),
    )
}

/// Palette: revert typed revisions (in the order given).
pub fn revert_prompt(_ctx: &Ctx) -> Action {
    Action::popup(
        InputPopup::new("Revert", "", |text, ctx| match resolve_all(ctx, &text) {
            Ok(commits) => revert(commits),
            Err(e) => Action::Error(e),
        })
        .prompt("Revisions to revert, separated by spaces (merge commits are reverted against their first parent)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            short: id[..id.len().min(7)].into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "a".into(),
            email: "a@x".into(),
            time: 0,
            committer: "a".into(),
            committer_email: "a@x".into(),
            commit_time: 0,
            subject: "s".into(),
            refs: vec![],
        }
    }

    #[test]
    fn rebase_base_for_root_and_normal() {
        assert_eq!(rebase_base(&commit("abc", &[])), "--root");
        assert_eq!(rebase_base(&commit("abc", &["p"])), "abc^");
    }

    #[test]
    fn targets_prefer_marked() {
        let sel = Selection {
            commit: commit("s", &[]),
            marked: vec![],
            head: None,
            head_pushed: None,
            on_branch: None,
        };
        assert_eq!(sel.targets().len(), 1);
        let sel = Selection {
            commit: commit("s", &[]),
            marked: vec![commit("a", &[]), commit("b", &[])],
            head: None,
            head_pushed: None,
            on_branch: None,
        };
        assert_eq!(
            sel.targets()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(plural(1, "commit"), "1 commit");
        assert_eq!(plural(2, "commit"), "2 commits");
    }

    #[test]
    fn reachability_through_loaded_parents() {
        // m(merge) -> a, b ; a -> r ; b -> r ; x -> r (side branch, not an ancestor of m)
        let commits = vec![
            commit("x", &["r"]),
            commit("m", &["a", "b"]),
            commit("b", &["r"]),
            commit("a", &["r"]),
            commit("r", &[]),
        ];
        assert!(reaches(&commits, 1, 2), "second parent");
        assert!(reaches(&commits, 1, 4), "root through either parent");
        assert!(reaches(&commits, 1, 1));
        assert!(
            !reaches(&commits, 1, 0),
            "listed before: cannot be an ancestor"
        );
        assert!(!reaches(&commits, 2, 3), "sibling");
        assert!(!reaches(&commits, 1, 9));
    }

    #[test]
    fn previous_of_root_is_an_error() {
        assert!(matches!(
            compare_previous(&commit("r", &[])),
            Action::Error(_)
        ));
        assert!(matches!(
            compare_previous(&commit("r", &["p"])),
            Action::Popup(_)
        ));
    }
}
