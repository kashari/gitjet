use crate::config::Config;
use crate::git::Repo;
use crate::theme::Theme;

/// Shared, read-only context handed to every view and popup.
pub struct Ctx {
    pub repo: Repo,
    pub config: Config,
    pub theme: Theme,
}

impl Ctx {
    pub fn new(repo: Repo, config: Config) -> Self {
        let theme = Theme::from_name(&config.theme);
        Self {
            repo,
            config,
            theme,
        }
    }
}
