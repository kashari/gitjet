//! User configuration, read from `<config dir>/gitjet/config.toml`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// "dark" or "light"
    pub theme: String,
    /// Enable mouse support (click tabs, scroll)
    pub mouse: bool,
    /// Editor command; falls back to $VISUAL / $EDITOR / notepad / vi
    pub editor: Option<String>,
    pub diff: DiffConfig,
    pub commit: CommitConfig,
    pub log: LogConfig,
    pub local_history: LocalHistoryConfig,
    pub update: UpdateConfig,
    pub hosting: HostingConfig,
    /// Set when the config file exists but could not be parsed.
    #[serde(skip)]
    pub load_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffConfig {
    pub side_by_side: bool,
    pub context_lines: u32,
    pub ignore_whitespace: bool,
    pub word_diff: bool,
    pub syntax_highlight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommitConfig {
    /// Add a Signed-off-by trailer by default
    pub sign_off: bool,
    /// GPG/SSH-sign commits by default (`git commit -S`)
    pub gpg_sign: bool,
    /// Subject length after which the commit dialog warns
    pub subject_limit: usize,
    /// Commands run before each commit ; a non-zero exit blocks the commit
    pub checks: Vec<CommitCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommitCheck {
    pub name: String,
    /// Run through the platform shell in the repository root
    pub command: String,
    /// Enabled by default in the commit dialog
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Commits loaded per page in the log
    pub page_size: usize,
    /// Show all branches (like `--all`) instead of only HEAD
    pub all_branches: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalHistoryConfig {
    pub enabled: bool,
    /// Seconds between automatic snapshots of changed files
    pub interval_secs: u64,
    /// Snapshots older than this are pruned
    pub keep_days: u64,
    /// Files larger than this are not snapshotted
    pub max_file_kb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// "rebase" or "merge" — how "Update project" and Pull integrate upstream changes
    pub method: String,
    /// Fetch with --prune
    pub prune: bool,
}

/// Code hosting integration (pull requests). Works with any provider; tokens are never stored here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HostingConfig {
    /// Look up API tokens through git's credential helper (`git credential fill`)
    pub use_git_credentials: bool,
    /// Self-hosted servers whose kind can't be guessed from the host name
    pub hosts: Vec<HostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostEntry {
    /// Host name as it appears in the remote URL, e.g. "git.example.com"
    pub host: String,
    /// "github", "gitlab", "bitbucket", "bitbucket-server", "azure", "gitea" (also Forgejo) or "none"
    pub kind: String,
    /// API base URL when it isn't the provider's default for that host
    #[serde(default)]
    pub api_url: Option<String>,
}

impl Default for HostingConfig {
    fn default() -> Self {
        Self {
            use_git_credentials: true,
            hosts: Vec::new(),
        }
    }
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            mouse: true,
            editor: None,
            diff: DiffConfig::default(),
            commit: CommitConfig::default(),
            log: LogConfig::default(),
            local_history: LocalHistoryConfig::default(),
            update: UpdateConfig::default(),
            hosting: HostingConfig::default(),
            load_error: None,
        }
    }
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            side_by_side: false,
            context_lines: 3,
            ignore_whitespace: false,
            word_diff: true,
            syntax_highlight: true,
        }
    }
}

impl Default for CommitConfig {
    fn default() -> Self {
        Self {
            sign_off: false,
            gpg_sign: false,
            subject_limit: 72,
            checks: Vec::new(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            page_size: 500,
            all_branches: true,
        }
    }
}

impl Default for LocalHistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 60,
            keep_days: 14,
            max_file_kb: 1024,
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            method: "rebase".into(),
            prune: true,
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitjet")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Load the config; a missing file yields defaults, a broken one yields defaults plus `load_error`.
    pub fn load() -> Config {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => c,
                Err(e) => Config {
                    load_error: Some(format!("{}: {e}", path.display())),
                    ..Config::default()
                },
            },
            Err(_) => Config::default(),
        }
    }

    pub fn write_default() -> Result<PathBuf> {
        let path = Self::path();
        if !path.exists() {
            std::fs::create_dir_all(Self::dir())?;
            let text = toml::to_string_pretty(&Config::default())?;
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips() {
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.commit.subject_limit, 72);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c: Config = toml::from_str("theme = \"light\"\n[diff]\nside_by_side = true\n").unwrap();
        assert_eq!(c.theme, "light");
        assert!(c.diff.side_by_side);
        assert_eq!(c.diff.context_lines, 3);
    }
}
