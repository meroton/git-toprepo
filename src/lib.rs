pub mod config;
pub mod error;
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


/// Checks if a directory contains a configured git-toprepo
/// This is the canonical detection logic used throughout the codebase
pub fn is_monorepo(path: &std::path::Path) -> anyhow::Result<bool> {
    Ok(crate::repo::TopRepo::open_configured(path).is_ok())
}

// TODO: We found a path when testing with an incomplete setup that hits this
// but the code won't work anyway.
// Maybe with the recent refactorings we can instead try to open it and if it
// works it works?
//
//  The code below is sufficient for `is-monorepo` to return yes.
//  But one can't load the config nor other data structures for the monorepo.
/*
      git_command(temp_dir)
        .args(["init"])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();

    git_command(temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            &format!("local:{toprepo}"),
        ])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();
*/
// According to issue #21 this would be sufficient, but we have seen counter
// examples.
/*
 * pub fn is_monorepo(path: &std::path::Path) -> anyhow::Result<bool> {
 *     let key = &config::toprepo_git_config(config::TOPREPO_CONFIG_FILE_KEY);
 *     let maybe = git::git_config_get(path, key)?;
 *     Ok(maybe.is_some())
 * }
 */
