use crate::git::git_command;
use crate::log::CommandSpanExt as _;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use std::path::Path;
use std::path::PathBuf;

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

/// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
/// `Ok(false)` if partially successful. The progress is logged, both what files
/// are written and potential errors.
pub fn install(repo: &Path, force: bool, with_git_lfs: bool) -> Result<bool> {
    if with_git_lfs {
        install_with_git_lfs(repo, force)
    } else {
        install_without_git_lfs(repo, force)
    }
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
    Ok(success)
}
