//! Repository switcher (superproject, worktrees, submodules, nested and recent repositories) and the
//! Clone / Init dialogs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::admin;
use crate::ui::popups::{InputPopup, MenuItem, MenuPopup};

/// How deep the work tree is scanned for nested repositories.
const NESTED_DEPTH: usize = 3;

/// One switcher entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub kind: &'static str,
    pub label: String,
    pub path: PathBuf,
}

/// `~/…` for paths in the home directory, for compact display.
pub(super) fn display_path(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
        }
    }
    p.display().to_string()
}

fn dir_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Everything the switcher offers, deduplicated, excluding the current repository.
pub(super) fn candidates(ctx: &Ctx) -> Vec<Candidate> {
    let repo = &ctx.repo;
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(sp) = admin::superproject(repo) {
        out.push(Candidate {
            kind: "Parent",
            label: dir_name(&sp),
            path: sp,
        });
    }
    for w in admin::list_worktrees(repo).unwrap_or_default() {
        if w.bare || w.prunable.is_some() {
            continue;
        }
        let what = match (&w.branch, w.is_main) {
            (Some(b), true) => format!("{} [{b}] (main)", dir_name(&w.path)),
            (Some(b), false) => format!("{} [{b}]", dir_name(&w.path)),
            (None, _) => format!("{} (detached)", dir_name(&w.path)),
        };
        out.push(Candidate {
            kind: "Worktree",
            label: what,
            path: w.path,
        });
    }
    for s in admin::list_submodules(repo).unwrap_or_default() {
        let abs = repo.path(&s.path);
        if abs.join(".git").exists() {
            out.push(Candidate {
                kind: "Submodule",
                label: s.path.clone(),
                path: abs,
            });
        }
    }
    for p in admin::find_nested_repos(&repo.root, NESTED_DEPTH) {
        let rel = p
            .strip_prefix(&repo.root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| p.display().to_string());
        out.push(Candidate {
            kind: "Nested",
            label: rel,
            path: p,
        });
    }
    for p in admin::recent_repos() {
        out.push(Candidate {
            kind: "Recent",
            label: dir_name(&p),
            path: p,
        });
    }
    dedupe(out, &repo.root)
}

/// Drop duplicates (first occurrence wins) and the current repository.
fn dedupe(list: Vec<Candidate>, current: &Path) -> Vec<Candidate> {
    let key = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    seen.insert(key(current));
    list.into_iter()
        .filter(|c| seen.insert(key(&c.path)))
        .collect()
}

/// The filterable switcher popup.
pub(super) fn open(ctx: &Ctx) -> Action {
    let mut items: Vec<MenuItem> = candidates(ctx)
        .into_iter()
        .map(|c| {
            let label = format!("{:<10} {}  {}", c.kind, c.label, display_path(&c.path));
            let path = c.path;
            MenuItem::new(label, "", move |_| Action::SwitchRepo(path))
        })
        .collect();
    if items.is_empty() {
        items.push(
            MenuItem::new(
                "No worktrees, submodules, nested or recent repositories",
                "",
                |_| Action::None,
            )
            .enabled(false),
        );
    }
    items.push(MenuItem::new("Clone repository…", "", clone_dialog));
    items.push(MenuItem::new("Init repository…", "", init_dialog));
    let title = format!("Switch repository (current: {})", ctx.repo.name());
    Action::popup(MenuPopup::new(title, items).filterable())
}

/// Directory next to the current repository, the default location for clones and new repositories.
fn sibling_base(ctx: &Ctx) -> PathBuf {
    ctx.repo
        .root
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.repo.root.clone())
}

/// "Clone repository…": URL → target directory (default: sibling folder named after the repo).
pub(super) fn clone_dialog(_ctx: &Ctx) -> Action {
    Action::popup(
        InputPopup::new("Clone repository", "", |url, ctx| {
            let url = url.trim().to_string();
            let base = sibling_base(ctx);
            let default = base.join(crate::util::repo_name_from_url(&url));
            Action::popup(
                InputPopup::new("Clone into", &default.to_string_lossy(), move |dir, _| {
                    let target = admin::resolve_dir(&dir, &base);
                    let open = target.clone();
                    Action::spawn_then(
                        format!("Clone {}", crate::util::repo_name_from_url(&url)),
                        move |repo| admin::clone_repo(repo, &url, &target),
                        move |res| match res {
                            Ok(_) => Action::SwitchRepo(open),
                            Err(_) => Action::None,
                        },
                    )
                })
                .prompt("Target directory (must not exist or be empty)"),
            )
        })
        .prompt("Repository URL (https, ssh or a local path)"),
    )
}

/// "Init repository…": directory → `git init`, then open it.
pub(super) fn init_dialog(ctx: &Ctx) -> Action {
    let base = sibling_base(ctx);
    let initial = format!("{}{}", base.display(), std::path::MAIN_SEPARATOR);
    Action::popup(
        InputPopup::new("Init repository", &initial, move |dir, ctx| {
            let target = admin::resolve_dir(&dir, &base);
            match admin::init_repo(&ctx.repo, &target) {
                Ok(()) => Action::Batch(vec![
                    Action::Toast(format!("Initialized {}", target.display())),
                    Action::SwitchRepo(target),
                ]),
                Err(e) => Action::error(format!("{e:#}")),
            }
        })
        .prompt("Directory for the new repository (created if missing)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(kind: &'static str, p: &Path) -> Candidate {
        Candidate {
            kind,
            label: String::new(),
            path: p.to_path_buf(),
        }
    }

    #[test]
    fn dedupe_keeps_first_and_skips_current() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let cur = tempfile::tempdir().unwrap();
        let list = vec![
            c("Worktree", a.path()),
            c("Recent", cur.path()),
            c("Recent", a.path()),
            c("Nested", b.path()),
        ];
        let out = dedupe(list, cur.path());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, "Worktree");
        assert_eq!(out[1].kind, "Nested");
    }

    #[test]
    fn display_path_shortens_home() {
        if let Some(home) = dirs::home_dir() {
            let p = home.join("code").join("x");
            assert!(display_path(&p).starts_with('~'));
        }
        assert_eq!(dir_name(Path::new("/a/b c")), "b c");
    }
}
