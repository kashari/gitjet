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
    pub fn new(repo: Repo, mut config: Config) -> Self {
        let (theme, problems) = Theme::from_config(&config);
        if config.load_error.is_none() && !problems.is_empty() {
            config.load_error = Some(problems.join("; "));
        }
        Self {
            repo,
            config,
            theme,
        }
    }
}
