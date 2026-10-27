//! The tabs, in display order, plus cross-feature entry points (see each module's `pub fn`s).

pub mod branches;
pub mod changes;
pub mod conflicts;
pub mod files;
pub mod local_history;
pub mod log;
pub mod prs;
pub mod push;
pub mod rebase;
pub mod repository;
pub mod shelf;

use super::{PaletteItem, View};
use crate::ctx::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Log,
    Branches,
    Shelf,
    Files,
    Conflicts,
    PullRequests,
    LocalHistory,
    Repository,
}

impl Tab {
    pub const ALL: [Tab; 9] = [
        Tab::Changes,
        Tab::Log,
        Tab::Branches,
        Tab::Shelf,
        Tab::Files,
        Tab::Conflicts,
        Tab::PullRequests,
        Tab::LocalHistory,
        Tab::Repository,
    ];

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// One view per [`Tab`], in the same order.
pub fn build(ctx: &Ctx) -> Vec<Box<dyn View>> {
    vec![
        Box::new(changes::ChangesView::new(ctx)),
        Box::new(log::LogView::new(ctx)),
        Box::new(branches::BranchesView::new(ctx)),
        Box::new(shelf::ShelfView::new(ctx)),
        Box::new(files::FilesView::new(ctx)),
        Box::new(conflicts::ConflictsView::new(ctx)),
        Box::new(prs::PrsView::new(ctx)),
        Box::new(local_history::LocalHistoryView::new(ctx)),
        Box::new(repository::RepositoryView::new(ctx)),
    ]
}

/// Every feature's palette entries.
pub fn palette() -> Vec<PaletteItem> {
    let mut v = Vec::new();
    v.extend(changes::palette());
    v.extend(log::palette());
    v.extend(branches::palette());
    v.extend(push::palette());
    v.extend(rebase::palette());
    v.extend(shelf::palette());
    v.extend(files::palette());
    v.extend(conflicts::palette());
    v.extend(prs::palette());
    v.extend(local_history::palette());
    v.extend(repository::palette());
    v.extend(crate::ui::diff_view::palette());
    v
}
