//! Diff viewer: a fullscreen popup ([`open`]) and an inline preview renderer ([`draw_diff`]).
//!
//! PUBLIC API (used by many features — keep these signatures stable):
//! - [`DiffSource`] and [`DiffSource::load`]
//! - [`open`] → pushes the fullscreen viewer
//! - [`draw_diff`] / [`diff_height`] → inline preview panes
//! - [`palette`] → "Compare with branch or revision…", "Compare two files…", "Show all local changes"...
//!
//! Layout math lives in [`model`], painting and caches in [`render`], syntax colors in
//! [`highlight`], the fullscreen popup in [`viewer`] and the comparison dialogs in [`compare`].

pub mod compare;
pub mod highlight;
pub mod model;
pub mod render;
pub mod viewer;

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::Action;
use crate::ctx::Ctx;
use crate::git::diff::{DiffOptions, FileDiff};
use crate::git::Repo;

pub use viewer::DiffViewer;

/// What to diff.
#[derive(Debug, Clone)]
pub enum DiffSource {
    /// Work tree vs index (plus untracked files listed in `paths`)
    Unstaged { paths: Vec<String> },
    /// Index vs HEAD
    Staged { paths: Vec<String> },
    /// Work tree vs HEAD (plus untracked files listed in `paths`)
    Local { paths: Vec<String> },
    /// One commit vs its first parent
    Commit { rev: String, paths: Vec<String> },
    /// `from` → `to`
    Range {
        from: String,
        to: String,
        paths: Vec<String>,
    },
    /// Revision vs work tree
    RevToWorktree { rev: String, paths: Vec<String> },
    /// Stash entry `stash@{index}` (including untracked files)
    Stash { index: usize },
    /// Two files on disk
    Files { left: PathBuf, right: PathBuf },
    /// Already-parsed diffs (shelf, patches, PRs...)
    Raw(Vec<FileDiff>),
}

/// Above this many paths, diffs are loaded unrestricted and filtered afterwards instead of passing
/// every path on the command line (Windows limits command lines to ~32k characters).
const MAX_ARG_PATHS: usize = 64;

impl DiffSource {
    pub fn load(&self, repo: &Repo, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        Ok(match self {
            DiffSource::Unstaged { paths } => {
                let d = with_paths(paths, |p| repo.diff_unstaged(p, opts))?;
                with_untracked(repo, d, paths)?
            }
            DiffSource::Staged { paths } => with_paths(paths, |p| repo.diff_staged(p, opts))?,
            DiffSource::Local { paths } => {
                let d = with_paths(paths, |p| repo.diff_local(p, opts))?;
                with_untracked(repo, d, paths)?
            }
            DiffSource::Commit { rev, paths } => {
                with_paths(paths, |p| repo.diff_commit(rev, p, opts))?
            }
            DiffSource::Range { from, to, paths } => {
                with_paths(paths, |p| repo.diff_range(from, to, p, opts))?
            }
            DiffSource::RevToWorktree { rev, paths } => {
                with_paths(paths, |p| repo.diff_rev_to_worktree(rev, p, opts))?
            }
            DiffSource::Stash { index } => load_stash(repo, *index, opts)?,
            DiffSource::Files { left, right } => {
                let mut d =
                    repo.diff_files(&left.to_string_lossy(), &right.to_string_lossy(), opts)?;
                // `--no-index` reports mangled absolute paths; show what the user picked.
                if let [one] = d.as_mut_slice() {
                    one.old_path = Some(crate::git::compare::display_path(repo, left));
                    one.new_path = Some(crate::git::compare::display_path(repo, right));
                }
                d
            }
            DiffSource::Raw(d) => d.clone(),
        })
    }

    /// Labels for the two sides ("HEAD", "Working tree", a short hash...), shown above each pane.
    pub fn side_labels(&self) -> (String, String) {
        let s = |x: &str| short_rev(x);
        match self {
            DiffSource::Unstaged { .. } => ("Index".into(), "Working tree".into()),
            DiffSource::Staged { .. } => ("HEAD".into(), "Index".into()),
            DiffSource::Local { .. } => ("HEAD".into(), "Working tree".into()),
            DiffSource::Commit { rev, .. } => (format!("{}^", s(rev)), s(rev)),
            DiffSource::Range { from, to, .. } => (s(from), s(to)),
            DiffSource::RevToWorktree { rev, .. } => (s(rev), "Working tree".into()),
            DiffSource::Stash { index } => ("Stash base".into(), format!("stash@{{{index}}}")),
            DiffSource::Files { left, right } => (
                left.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                right
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            DiffSource::Raw(_) => ("Before".into(), "After".into()),
        }
    }

    /// The revision holding the "after" side, when it is not the work tree (for blame).
    ///
    /// `None` means the work tree. For [`DiffSource::Staged`] that is the closest annotatable
    /// version of the index: `git blame` takes revisions, and staged lines show as not committed.
    pub fn new_side_rev(&self) -> Option<String> {
        match self {
            DiffSource::Commit { rev, .. } => Some(rev.clone()),
            DiffSource::Range { to, .. } => Some(to.clone()),
            DiffSource::Stash { index } => Some(format!("stash@{{{index}}}")),
            _ => None,
        }
    }

    /// The revision holding the "before" side (for blaming deleted files).
    ///
    /// For [`DiffSource::Unstaged`] the before side is the index, which matches HEAD for a file
    /// that was deleted without staging.
    pub fn old_side_rev(&self) -> Option<String> {
        match self {
            DiffSource::Commit { rev, .. } => Some(format!("{rev}^")),
            DiffSource::Range { from, .. } => Some(from.clone()),
            DiffSource::RevToWorktree { rev, .. } => Some(rev.clone()),
            DiffSource::Stash { index } => Some(format!("stash@{{{index}}}^")),
            DiffSource::Unstaged { .. } | DiffSource::Staged { .. } | DiffSource::Local { .. } => {
                Some("HEAD".into())
            }
            DiffSource::Files { .. } | DiffSource::Raw(_) => None,
        }
    }
}

/// Abbreviate full hashes to 8 characters; leave names alone.
pub fn short_rev(rev: &str) -> String {
    if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        rev[..8].to_string()
    } else {
        rev.to_string()
    }
}

fn load_stash(repo: &Repo, index: usize, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
    let s = format!("stash@{{{index}}}");
    let extra = opts.args();
    let mut args: Vec<&str> = vec!["stash", "show", "-p", "--include-untracked"];
    args.extend(extra.iter().map(String::as_str));
    args.push(&s);
    match repo.run(&args) {
        Ok(out) => Ok(crate::git::diff::parse_unified(&out)),
        Err(_) => {
            // Older git without --include-untracked support for show.
            let mut args: Vec<&str> = vec!["stash", "show", "-p"];
            args.extend(extra.iter().map(String::as_str));
            args.push(&s);
            Ok(crate::git::diff::parse_unified(&repo.run(&args)?))
        }
    }
}

/// True if repo-relative `path` is one of `paths` or inside one of them (directory pathspecs).
fn path_selected(path: &str, exact: &HashSet<&str>, paths: &[String]) -> bool {
    exact.contains(path)
        || paths.iter().any(|p| {
            let dir = p.trim_end_matches('/');
            !dir.is_empty()
                && path
                    .strip_prefix(dir)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// Run `load` restricted to `paths`, avoiding over-long command lines for big path lists.
fn with_paths(
    paths: &[String],
    load: impl Fn(&[String]) -> Result<Vec<FileDiff>>,
) -> Result<Vec<FileDiff>> {
    if paths.len() <= MAX_ARG_PATHS {
        return load(paths);
    }
    let exact: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut all = load(&[])?;
    all.retain(|d| {
        [d.new_path.as_deref(), d.old_path.as_deref()]
            .into_iter()
            .flatten()
            .any(|p| path_selected(p, &exact, paths))
    });
    Ok(all)
}

/// Append synthetic diffs for untracked files among `paths`.
fn with_untracked(
    repo: &Repo,
    mut diffs: Vec<FileDiff>,
    paths: &[String],
) -> Result<Vec<FileDiff>> {
    if paths.is_empty() {
        return Ok(diffs);
    }
    let exact: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let present: HashSet<String> = diffs.iter().map(|d| d.path().to_string()).collect();
    let st = repo.status()?;
    for e in st.entries.iter().filter(|e| e.is_untracked()) {
        if path_selected(&e.path, &exact, paths) && !present.contains(&e.path) {
            // Files deleted between status and read are simply skipped.
            if let Ok(d) = repo.untracked_diff(&e.path) {
                diffs.push(d);
            }
        }
    }
    Ok(diffs)
}

/// Open the fullscreen diff viewer.
pub fn open(title: impl Into<String>, source: DiffSource) -> Action {
    Action::popup(DiffViewer::new(title.into(), source))
}

/// Number of rendered lines for `diffs` in the inline preview (for scroll clamping).
pub fn diff_height(diffs: &[FileDiff]) -> usize {
    model::unified_row_count(diffs)
}

/// Render `diffs` (unified) into `area`, starting `scroll` lines down. No border is drawn.
/// Word-level and syntax highlighting follow the user's config; results are cached by content,
/// so calling this every frame is cheap.
pub fn draw_diff(f: &mut Frame, area: Rect, diffs: &[FileDiff], scroll: usize, ctx: &Ctx) {
    render::draw_inline(f, area, diffs, scroll, ctx);
}

/// Palette entries for comparisons (compare with branch/revision, compare two files...).
pub fn palette() -> Vec<crate::ui::PaletteItem> {
    compare::palette()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::ChangeKind;
    use crate::git::testutil::*;

    #[test]
    fn loads_local_changes_with_untracked() {
        let (_d, repo) = temp_repo();
        commit_files(&repo, &[("a.txt", "1\n")], "one");
        write(&repo, "a.txt", "2\n");
        write(&repo, "new dir/u é.txt", "x\r\ny\r\n");
        let o = DiffOptions::default();
        let paths = vec!["a.txt".to_string(), "new dir/u é.txt".to_string()];
        let d = DiffSource::Local { paths }.load(&repo, &o).unwrap();
        assert_eq!(d.len(), 2);
        let u = d.iter().find(|f| f.path() == "new dir/u é.txt").unwrap();
        assert_eq!(u.kind, ChangeKind::Added);
        assert_eq!(u.hunks[0].lines[0].content, "x");
        // Directory pathspecs pick up untracked files inside.
        let d = DiffSource::Unstaged {
            paths: vec!["new dir".into()],
        }
        .load(&repo, &o)
        .unwrap();
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn many_paths_are_filtered_not_passed() {
        let (_d, repo) = temp_repo();
        let files: Vec<(String, String)> = (0..80)
            .map(|i| (format!("f{i}.txt"), format!("{i}\n")))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        commit_files(&repo, &refs, "many");
        for (p, _) in &files {
            write(&repo, p, "changed\n");
        }
        let mut paths: Vec<String> = files.iter().map(|(p, _)| p.clone()).take(70).collect();
        paths.push("missing.txt".into());
        let d = DiffSource::Unstaged { paths }
            .load(&repo, &DiffOptions::default())
            .unwrap();
        assert_eq!(d.len(), 70);
    }

    #[test]
    fn loads_files_and_ranges() {
        let (_d, repo) = temp_repo();
        let c1 = commit_files(&repo, &[("a.txt", "1\n")], "one");
        let c2 = commit_files(&repo, &[("a.txt", "2\n")], "two");
        let o = DiffOptions::default();
        let r = DiffSource::Range {
            from: c1.clone(),
            to: c2.clone(),
            paths: vec![],
        };
        assert_eq!(r.load(&repo, &o).unwrap().len(), 1);
        assert_eq!(r.side_labels().0, c1[..8]);
        let c = DiffSource::Commit {
            rev: c2.clone(),
            paths: vec![],
        };
        assert_eq!(c.load(&repo, &o).unwrap().len(), 1);
        assert_eq!(c.old_side_rev().unwrap(), format!("{c2}^"));
        let u = DiffSource::Unstaged { paths: vec![] };
        assert_eq!(u.old_side_rev().as_deref(), Some("HEAD"));
        assert_eq!(u.new_side_rev(), None);
        let st = DiffSource::Stash { index: 1 };
        assert_eq!(st.new_side_rev().as_deref(), Some("stash@{1}"));
        assert_eq!(st.old_side_rev().as_deref(), Some("stash@{1}^"));
        write(&repo, "l.txt", "a\n");
        write(&repo, "r.txt", "b\n");
        let f = DiffSource::Files {
            left: repo.path("l.txt"),
            right: repo.path("r.txt"),
        };
        let d = f.load(&repo, &o).unwrap();
        assert_eq!(d[0].old_path.as_deref(), Some("l.txt"));
        assert_eq!(d[0].new_path.as_deref(), Some("r.txt"));
        write(&repo, "r.txt", "a\n");
        assert!(f.load(&repo, &o).unwrap().is_empty());
    }

    #[test]
    fn height_matches_rows() {
        let d = crate::git::diff::parse_unified(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/i b/i\nBinary files a/i and b/i differ\n",
        );
        assert_eq!(diff_height(&d), model::unified_rows(&d, 0..2, true).len());
        assert_eq!(diff_height(&d), 4 + 2);
    }

    #[test]
    fn short_revs() {
        assert_eq!(short_rev("main"), "main");
        assert_eq!(short_rev(&"a".repeat(40)), "aaaaaaaa");
    }

    #[test]
    fn palette_has_comparisons() {
        let names: Vec<&str> = palette().iter().map(|p| p.name).collect();
        assert!(names.contains(&"Compare with branch or revision…"));
        assert!(names.contains(&"Compare two files…"));
        assert!(names.contains(&"Show all local changes"));
        assert!(names.contains(&"Show staged changes"));
    }
}
