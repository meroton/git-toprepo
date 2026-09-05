use crate::git::git_command;
use crate::log::CommandSpanExt as _;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use std::path::Path;
use std::path::PathBuf;

struct LfsFilterConfig {
    /// The subcommand sent to Git LFS.
    pub subcommand: &'static str,
    /// The git-config key.
    pub git_config_key: &'static str,
    /// Warning message in case of bad config.
    pub bad_config_message: &'static str,
}

const LFS_FILTERS: [LfsFilterConfig; 2] = [
    LfsFilterConfig {
        subcommand: "smudge",
        git_config_key: "filter.lfs.smudge",
        bad_config_message: "In an emulated monorepo this may fetch Git LFS objects from the wrong remote.",
    },
    LfsFilterConfig {
        subcommand: "filter-process",
        git_config_key: "filter.lfs.process",
        bad_config_message: "Git's long-running filter process can take precedence over filter.lfs.smudge,\
so this may bypass Git Toprepo aware Git LFS handling.",
    },
];

const PRE_PUSH_HOOK_NAME: &str = "pre-push";
const PRE_PUSH_BASE_HOOK_WITHOUT_LFS_CONTENT: &str = r#"#!/bin/sh
set -eu
/bin/sh "$0.toprepo" "$@" < /dev/null
"#;
const PRE_PUSH_BASE_HOOK_WITH_LFS_CONTENT: &str = r#"#!/bin/sh
set -eu
/bin/sh "$0.toprepo" "$@" < /dev/null
git lfs pre-push "$@"
"#;

const PRE_PUSH_TOPREPO_HOOK_CONTENT: &str = r#"#!/bin/sh
set -eu
# This is an optional hook to improve the error message when running 'git push' instead of 'git toprepo push'.
if test "${GIT_TOPREPO_ALLOW_PUSH:-0}" != "1"; then
    echo "ERROR: Please use 'git toprepo push' instead of 'git push'.

If you really want to push without Git Toprepo, use 'git push --no-verify' or 'export GIT_TOPREPO_ALLOW_PUSH=1'." >&2
    exit 1
fi
"#;

fn other_git_lfs_base_hook_content(name: &str) -> String {
    format!(
        r#"#!/bin/sh
set -eu
git lfs {name} "$@"
"#
    )
}

/// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
/// `Ok(false)` if partially successful. The progress is logged, both what files
/// are written and potential errors.
pub fn install_with_git_lfs(repo: &Path, force: bool) -> Result<bool> {
    let hooks_root_path = get_hooks_root_path(repo)?;

    let mut success = true;
    let mut handle_result = |result: Result<String>| match result {
        Ok(msg) => log::info!("{msg}"),
        Err(err) => {
            success = false;
            log::error!("{err:#}");
        }
    };

    handle_result(write_hook(
        &hooks_root_path.join(format!("{PRE_PUSH_HOOK_NAME}.toprepo")),
        PRE_PUSH_TOPREPO_HOOK_CONTENT,
        &[],
        force,
    ));
    handle_result(write_hook(
        &hooks_root_path.join(PRE_PUSH_HOOK_NAME),
        PRE_PUSH_BASE_HOOK_WITH_LFS_CONTENT,
        &[PRE_PUSH_BASE_HOOK_WITHOUT_LFS_CONTENT],
        force,
    ));
    for name in &["post-checkout", "post-commit", "post-merge"] {
        handle_result(write_hook(
            &hooks_root_path.join(name),
            &other_git_lfs_base_hook_content(name),
            &[],
            force,
        ));
    }

    for lfs_filter in &LFS_FILTERS {
        match lfs_filter_bypasses_toprepo(repo, lfs_filter.git_config_key, lfs_filter.subcommand)? {
            LfsFilterConfigState::Missing => {
                handle_result(install_git_lfs_filter(repo, lfs_filter));
            }
            LfsFilterConfigState::ThroughGitToprepo => {
                log::info!(
                    "Verified the git-config {} for Git LFS",
                    lfs_filter.git_config_key
                );
            }
            LfsFilterConfigState::Bad(values) => {
                if force {
                    handle_result(install_git_lfs_filter(repo, lfs_filter));
                } else {
                    for value in values {
                        handle_result(Err(anyhow::anyhow!(
                            "{} is configured as '{value}', which bypasses Git Toprepo. {}",
                            lfs_filter.git_config_key,
                            lfs_filter.bad_config_message,
                        )));
                    }
                }
            }
        }
    }
    Ok(success)
}

/// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
/// `Ok(false)` if partially successful. The progress is logged, both what files
/// are written and potential errors.
pub fn install_without_git_lfs(repo: &Path, force: bool) -> Result<bool> {
    let hooks_root_path = get_hooks_root_path(repo)?;

    let mut success = true;
    let mut handle_result = |result: Result<String>| match result {
        Ok(msg) => log::info!("{msg}"),
        Err(err) => {
            success = false;
            log::error!("{err:#}");
        }
    };

    handle_result(write_hook(
        &hooks_root_path.join(format!("{PRE_PUSH_HOOK_NAME}.toprepo")),
        PRE_PUSH_TOPREPO_HOOK_CONTENT,
        &[],
        force,
    ));
    handle_result(write_hook(
        &hooks_root_path.join(PRE_PUSH_HOOK_NAME),
        PRE_PUSH_BASE_HOOK_WITHOUT_LFS_CONTENT,
        &[PRE_PUSH_BASE_HOOK_WITH_LFS_CONTENT],
        force,
    ));
    for name in &["post-checkout", "post-commit", "post-merge"] {
        match remove_hook(
            &hooks_root_path.join(name),
            &[&other_git_lfs_base_hook_content(name)],
            force,
        ) {
            Ok(None) => {}
            Ok(Some(msg)) => handle_result(Ok(msg)),
            Err(err) => handle_result(Err(err)),
        }
    }

    for lfs_filter in &LFS_FILTERS {
        match lfs_filter_bypasses_toprepo(repo, lfs_filter.git_config_key, lfs_filter.subcommand)? {
            LfsFilterConfigState::Missing => {
                log::debug!(
                    "Verified absence of git-config {}",
                    lfs_filter.git_config_key
                );
            }
            LfsFilterConfigState::ThroughGitToprepo => {
                handle_result(remove_git_lfs_filter(repo, lfs_filter));
            }
            LfsFilterConfigState::Bad(values) => {
                if force {
                    handle_result(remove_git_lfs_filter(repo, lfs_filter));
                } else {
                    for value in values {
                        handle_result(Err(anyhow::anyhow!(
                            "{} is configured as '{value}', which bypasses Git Toprepo. {}",
                            lfs_filter.git_config_key,
                            lfs_filter.bad_config_message,
                        )));
                    }
                }
            }
        }
    }
    Ok(success)
}

/// If Git LFS is available, information about how to install the Git LFS hooks
/// is shown.
pub fn maybe_show_lfs_installation_instruction(repo: &Path) {
    if crate::lfs::ensure_git_lfs_available(repo).is_ok() {
        log::info!("Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.");
    }
}

fn write_hook(
    path: &Path,
    content: &str,
    content_acceptable_to_overwrite: &[&str],
    force: bool,
) -> Result<String> {
    let wanted_content = content;
    let mut allow_overwrite = force;
    let existing_content = std::fs::read_to_string(path);
    if let Ok(existing_content) = &existing_content
        && crate::util::is_executable(path)
    {
        if existing_content == wanted_content {
            return Ok(format!("Verified {}", path.display()));
        }
        if !allow_overwrite && content_acceptable_to_overwrite.contains(&existing_content.as_str())
        {
            allow_overwrite = true;
        }
    }
    // Write or overwrite.
    if allow_overwrite {
        crate::util::write_executable(path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
    } else {
        crate::util::create_executable(path, content)
            .with_context(|| format!("Failed to create {}", path.display()))?;
    }
    Ok(format!("Written {}", path.display()))
}

/// Removes a file if the content matches the expected content.
fn remove_hook(
    path: &Path,
    content_acceptable_to_remove: &[&str],
    force: bool,
) -> Result<Option<String>> {
    if path.try_exists()? {
        if !force {
            let existing_content = std::fs::read_to_string(path)?;
            if !content_acceptable_to_remove.contains(&existing_content.as_str()) {
                anyhow::bail!("Unexpected content, won't delete {}", path.display());
            }
        }
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        Ok(Some(format!("Removed {}", path.display())))
    } else {
        log::debug!("Verified absence of {}", path.display());
        Ok(None)
    }
}

fn get_hooks_root_path(repo: &Path) -> Result<PathBuf> {
    Ok(Path::new(
        git_command(repo)
            .args(["rev-parse", "--path-format=absolute", "--git-path", "hooks"])
            .trace_command(crate::command_span!("git rev-parse --git-path hooks"))
            .safe_output()?
            .check_success_with_stderr()
            .context("Failed to rev-parse .git/hooks directory")?
            .stdout
            .to_str()?
            .trim_newline_suffix(),
    )
    .to_owned())
}

fn install_git_lfs_filter(repo: &Path, lfs_filter: &LfsFilterConfig) -> Result<String> {
    let value = &format!("git toprepo lfs {}", lfs_filter.subcommand);
    git_command(repo)
        .args(["config", lfs_filter.git_config_key, value])
        .trace_command(crate::command_span!("git config"))
        .safe_status()?
        .check_success()
        .with_context(|| {
            format!(
                "Failed to set git-config {} to {value}",
                lfs_filter.git_config_key
            )
        })?;
    Ok(format!("Written git-config {}", lfs_filter.git_config_key))
}

fn remove_git_lfs_filter(repo: &Path, lfs_filter: &LfsFilterConfig) -> Result<String> {
    git_command(repo)
        .args(["config", "--unset", lfs_filter.git_config_key])
        .trace_command(crate::command_span!("git config"))
        .safe_status()?
        .check_success()
        .with_context(|| format!("Failed to unset git-config {}", lfs_filter.git_config_key))?;
    match lfs_filter_bypasses_toprepo(repo, lfs_filter.git_config_key, lfs_filter.subcommand)? {
        LfsFilterConfigState::Missing => {
            Ok(format!("Unset git-config {}", lfs_filter.git_config_key))
        }
        LfsFilterConfigState::ThroughGitToprepo => anyhow::bail!(
            "git-config {} still exists after unsetting it",
            lfs_filter.git_config_key
        ),
        LfsFilterConfigState::Bad(values) => anyhow::bail!(
            "git-config {} got bad values {values:#?} after unsetting it",
            lfs_filter.git_config_key
        ),
    }
}

/// The state of the Git configuration for `smudge` and `filter-process` filters.
#[derive(PartialEq)]
pub enum LfsFilterConfigState {
    /// No configuration found.
    Missing,
    /// Correctly setup through Git Toprepo.
    ThroughGitToprepo,
    /// The bad configuration values that bypasses Git Toprepo. There might
    /// still be configuration values that go through Git Toprepo, but that
    /// mixed case is ignored.
    Bad(Vec<String>),
}

pub fn warn_if_lfs_filters_bypass_toprepo(
    repo: &gix::Repository,
    print_usage_help: bool,
) -> Result<()> {
    const USAGE_HELP_MSG: &str =
        "\nUse 'git toprepo hooks install --git-lfs=yes' to properly add Git LFS support.";
    for lfs_filter in LFS_FILTERS {
        match lfs_filter_bypasses_toprepo(
            repo.git_dir(),
            lfs_filter.git_config_key,
            lfs_filter.subcommand,
        )? {
            LfsFilterConfigState::Missing => {}
            LfsFilterConfigState::ThroughGitToprepo => {}
            LfsFilterConfigState::Bad(values) => {
                for value in values {
                    log::warn!(
                        "{} is configured as '{value}', which bypasses Git Toprepo. {}{}",
                        lfs_filter.git_config_key,
                        lfs_filter.bad_config_message,
                        if print_usage_help { USAGE_HELP_MSG } else { "" },
                    );
                }
            }
        }
    }
    Ok(())
}

fn lfs_filter_bypasses_toprepo(
    repo: &Path,
    key: &str,
    subcommand: &str,
) -> Result<LfsFilterConfigState> {
    let mut result = LfsFilterConfigState::Missing;
    for value in crate::git::git_config_get_all(repo, key)? {
        if is_lfs_filter_through_toprepo(&value, subcommand) {
            if result == LfsFilterConfigState::Missing {
                result = LfsFilterConfigState::ThroughGitToprepo;
            }
        } else {
            if let LfsFilterConfigState::Bad(values) = &mut result {
                values.push(value);
            } else {
                result = LfsFilterConfigState::Bad(vec![value]);
            }
        }
    }
    Ok(result)
}

pub(crate) fn is_lfs_filter_through_toprepo(command: &str, subcommand: &str) -> bool {
    let mut tokens: Vec<&str> = command.split_whitespace().collect();
    if let Some(first_token) = tokens.first() {
        let command_name = Path::new(first_token)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(first_token)
            .to_owned();
        // Replace a full path with the command name.
        tokens[0] = &command_name;
        if tokens.starts_with(&["git-toprepo", "lfs", subcommand]) {
            return true;
        }
        if tokens.starts_with(&["git", "toprepo", "lfs", subcommand]) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_git_toprepo_wrapper() {
        assert!(is_lfs_filter_through_toprepo(
            "git toprepo lfs filter-process",
            "filter-process"
        ));
        assert!(is_lfs_filter_through_toprepo(
            "git-toprepo lfs smudge -- %f",
            "smudge"
        ));
        assert!(is_lfs_filter_through_toprepo(
            "/some/path/git-toprepo lfs smudge -- %f",
            "smudge"
        ));
    }

    #[test]
    fn rejects_plain_git_lfs() {
        assert!(!is_lfs_filter_through_toprepo(
            "git lfs filter-process",
            "filter-process"
        ));
        assert!(!is_lfs_filter_through_toprepo(
            "git-lfs smudge -- %f",
            "smudge"
        ));
    }
}
