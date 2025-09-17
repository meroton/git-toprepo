pub mod config;
pub mod expander;
pub mod fetch;
pub mod git;
pub mod git_fast_export_import;
pub mod git_fast_export_import_dedup;
pub mod gitmodules;
pub mod gitreview;
pub mod loader;
pub mod log;
pub mod push;
pub mod repo;
pub mod repo_cache_serde;
pub mod repo_name;
pub mod submitted_together;
pub mod ui;
pub mod util;

/// Error indicating that the current directory is not a configured git-toprepo
/// TODO(terminology pr#172): This will be renamed when terminology is finalized
#[derive(Debug, PartialEq)]
pub struct NotAMonorepo;

impl std::fmt::Display for NotAMonorepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NotAMonorepo")
    }
}

impl std::error::Error for NotAMonorepo {}

/// Checks if a directory contains a configured git-toprepo
/// This is the canonical detection logic used throughout the codebase
pub fn is_monorepo(path: &std::path::Path) -> anyhow::Result<bool> {
    let key = &config::toprepo_git_config(config::TOPREPO_CONFIG_FILE_KEY);
    let maybe = git::git_config_get(path, key)?;
    Ok(maybe.is_some())
}
