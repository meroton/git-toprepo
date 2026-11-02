use crate::git::git_command;
use crate::log::CommandSpanExt as _;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use std::path::Path;
use std::path::PathBuf;

struct LfsFilter {
    /// The subcommand sent to Git LFS.
    pub subcommand: &'static str,
    /// Additional arguments to pass after optional`--skip`.
    pub args_after_skip: &'static str,
    /// The git-config key.
    pub git_config_key: &'static str,
    /// Warning message in case of bad config.
    pub bad_config_message: &'static str,
}

const LFS_FILTERS: &[LfsFilter] = &[
    // The filter.lfs.clean already works with the basic Git LFS hook, so it
    // does not need to be intercepted. For completeness, and in case it changes
    // in the future in Git LFS, so install the clean filter as well.
    LfsFilter {
        subcommand: "clean",
        args_after_skip: " -- %f",
        git_config_key: "filter.lfs.clean",
        // The filter.lfs.clean already works and does not need to be
        // intercepted, so no warning message is needed.
        bad_config_message: "",
    },
    LfsFilter {
        subcommand: "smudge",
        args_after_skip: " -- %f",
        git_config_key: "filter.lfs.smudge",
        bad_config_message: "In an emulated monorepo this may fetch Git LFS objects from the wrong remote.",
    },
    LfsFilter {
        subcommand: "filter-process",
        args_after_skip: "",
        git_config_key: "filter.lfs.process",
        bad_config_message: "Git's long-running filter process can take precedence over filter.lfs.smudge, \
so this may bypass Git Toprepo aware Git LFS handling.",
    },
];

struct Hook {
    pub name: &'static str,
    pub content_with_lfs: Option<&'static str>,
    pub content_without_lfs: Option<&'static str>,
}

const HOOKS: &[Hook] = &[
    Hook {
        name: "pre-push.toprepo",
        content_with_lfs: Some(include_str!("hooks/pre-push.toprepo")),
        content_without_lfs: Some(include_str!("hooks/pre-push.toprepo")),
    },
    Hook {
        name: "pre-push",
        content_with_lfs: Some(include_str!("hooks/with-lfs/pre-push")),
        content_without_lfs: Some(include_str!("hooks/without-lfs/pre-push")),
    },
    Hook {
        name: "post-checkout",
        content_with_lfs: Some(include_str!("hooks/with-lfs/post-checkout")),
        content_without_lfs: None,
    },
    Hook {
        name: "post-commit",
        content_with_lfs: Some(include_str!("hooks/with-lfs/post-commit")),
        content_without_lfs: None,
    },
    Hook {
        name: "post-merge",
        content_with_lfs: Some(include_str!("hooks/with-lfs/post-merge")),
        content_without_lfs: None,
    },
];

impl LfsFilter {
    /// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
    /// `Ok(false)` if partially successful, i.e. only some of the hooks could be
    /// installed.
    ///
    /// The progress is logged, both what files are written and potential errors.
    pub fn install(&self, repo: &Path, skip_download: bool, action: &str) -> Result<String> {
        let value = &format!(
            "git toprepo lfs {}{}{}",
            self.subcommand,
            if skip_download { " --skip" } else { "" },
            self.args_after_skip,
        );
        git_command(repo)
            .args(["config", self.git_config_key, value])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .with_context(|| {
                format!(
                    "Failed to set git-config {} to '{value}'",
                    self.git_config_key
                )
            })?;
        Ok(format!("{action} git-config {}", self.git_config_key))
    }

    pub fn remove(&self, repo: &Path) -> Result<String> {
        git_command(repo)
            .args(["config", "--unset", self.git_config_key])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .with_context(|| format!("Failed to unset git-config {}", self.git_config_key))?;
        match lfs_filter_bypasses_toprepo(repo, self.git_config_key, self.subcommand)? {
            LfsFilterConfigState::Missing => {
                Ok(format!("Unset git-config {}", self.git_config_key))
            }
            LfsFilterConfigState::ThroughGitToprepo => anyhow::bail!(
                "git-config {} still exists after unsetting it",
                self.git_config_key
            ),
            LfsFilterConfigState::Bad(values) => anyhow::bail!(
                "git-config {} got bad values {values:#?} after unsetting it",
                self.git_config_key
            ),
        }
    }
}

impl Hook {
    /// Checks if given content is acceptable to overwrite, basically checking
    /// if the content had been written by Git Toprepo itself before.
    pub fn content_can_be_overwritten(&self, content: &str) -> bool {
        self.content_with_lfs == Some(content) || self.content_without_lfs == Some(content)
    }

    /// Writes `content` as an executable file. If the file already exists, it
    /// will only be overwritten if the content is deemed safe to overwrite, see
    /// `content_can_be_overwritten`, or if the `force` flag is set.
    ///
    /// Returns a human readable message about the action taken.
    fn write(&self, hooks_root: &Path, content: &str, force: bool) -> Result<String> {
        let path = &hooks_root.join(self.name);
        let mut allow_overwrite = force;
        let existing_content = std::fs::read_to_string(path);
        if let Ok(existing_content) = &existing_content
            && crate::util::is_executable(path)
        {
            if existing_content == content {
                return Ok(format!("Verified {}", path.display()));
            }
            if !allow_overwrite && self.content_can_be_overwritten(existing_content) {
                allow_overwrite = true;
            }
        }
        // Write or overwrite.
        std::fs::create_dir_all(hooks_root)?;
        if allow_overwrite {
            crate::util::overwrite_executable(path, content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        } else {
            crate::util::create_executable(path, content)
                .with_context(|| format!("Failed to create {}", path.display()))?;
        }
        Ok(format!("Written {}", path.display()))
    }

    /// Removes a file if the content matches the expected content.
    fn remove(&self, hooks_root: &Path, force: bool) -> Result<Option<String>> {
        let path = &hooks_root.join(self.name);
        if path.try_exists()? {
            if !force {
                let existing_content = std::fs::read_to_string(path)?;
                if !self.content_can_be_overwritten(&existing_content) {
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
}

/// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
/// `Ok(false)` if partially successful, i.e. only some of the hooks could be
/// installed.
///
/// The progress is logged, both what files are written and potential errors.
pub fn install_with_git_lfs(repo: &Path, force: bool, skip_download: bool) -> Result<bool> {
    let hooks_root_path = get_hooks_root_path(repo)?;

    let mut success = true;
    let mut handle_result = |result: Result<String>| match result {
        Ok(msg) => log::info!("{msg}"),
        Err(err) => {
            success = false;
            log::error!("{err:#}");
        }
    };

    for hook in HOOKS {
        if let Some(content) = hook.content_with_lfs {
            handle_result(hook.write(&hooks_root_path, content, force));
        } else {
            match hook.remove(&hooks_root_path, force) {
                Ok(Some(msg)) => handle_result(Ok(msg)),
                Ok(None) => {}
                Err(err) => handle_result(Err(err)),
            }
        }
    }
    if !skip_download {
        log::warn!(
            "Git Toprepo does not support automatic download of Git LFS objects. Please install hooks with --git-lfs-skip-smudge."
        );
    }
    for lfs_filter in LFS_FILTERS {
        match lfs_filter_bypasses_toprepo(repo, lfs_filter.git_config_key, lfs_filter.subcommand)? {
            LfsFilterConfigState::Missing => {
                handle_result(lfs_filter.install(repo, skip_download, "Written"));
            }
            LfsFilterConfigState::ThroughGitToprepo => {
                handle_result(lfs_filter.install(repo, skip_download, "Rewritten"));
            }
            LfsFilterConfigState::Bad(values) => {
                if force {
                    handle_result(lfs_filter.install(repo, skip_download, "Written"));
                } else {
                    for value in values {
                        handle_result(Err(anyhow::anyhow!(
                            "{} is configured as '{value}', which bypasses Git Toprepo.{}{}",
                            lfs_filter.git_config_key,
                            if lfs_filter.bad_config_message.is_empty() {
                                ""
                            } else {
                                " "
                            },
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
/// `Ok(false)` if partially successful, i.e. only some of the hooks could be
/// installed.
///
/// The progress is logged, both what files are written and potential errors.
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

    for hook in HOOKS {
        if let Some(content) = hook.content_without_lfs {
            handle_result(hook.write(&hooks_root_path, content, force));
        } else {
            match hook.remove(&hooks_root_path, force) {
                Ok(Some(msg)) => handle_result(Ok(msg)),
                Ok(None) => {}
                Err(err) => handle_result(Err(err)),
            }
        }
    }
    for lfs_filter in LFS_FILTERS {
        match lfs_filter_bypasses_toprepo(repo, lfs_filter.git_config_key, lfs_filter.subcommand)? {
            LfsFilterConfigState::Missing => {
                log::debug!(
                    "Verified absence of git-config {}",
                    lfs_filter.git_config_key
                );
            }
            LfsFilterConfigState::ThroughGitToprepo => {
                handle_result(lfs_filter.remove(repo));
            }
            LfsFilterConfigState::Bad(values) => {
                if force {
                    handle_result(lfs_filter.remove(repo));
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
        log::info!(
            "Git LFS is available. Add the Git LFS hooks using 'git toprepo hooks install --git-lfs'."
        );
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
