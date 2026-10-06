use crate::git::git_command;
use crate::log::CommandSpanExt as _;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use std::path::Path;
use std::path::PathBuf;

const TOPREPO_HOOKS: [(&str, &str); 2] = [
    ("pre-push.toprepo", include_str!("hooks/pre-push.toprepo")),
    ("pre-push", include_str!("hooks/pre-push")),
];
// Currently, there is no "expected content" for the LFS hooks. This simply asks
// the user to remove the hooks manually or use `--force`.
const ADDITIONAL_LFS_HOOKS: [(&str, &[&str]); 3] = [
    ("post-checkout", &[]),
    ("post-commit", &[]),
    ("post-merge", &[]),
];

/// Writes `content` as an executable file. If the file already exists, it will
/// only be overwritten if the content matches one of the acceptable contents or
/// if the `force` flag is set.
///
/// Returns a human readable message about the action taken.
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
        crate::util::overwrite_executable(path, content)
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

/// Writes `.git/hooks/*` scripts. Returns `Ok(true)` if successful and
/// `Ok(false)` if partially successful, i.e. only some of the hooks could be
/// installed.
///
/// The progress is logged, both what files are written and potential errors.
pub fn install(repo: &Path, force: bool) -> Result<bool> {
    let hooks_root_path = get_hooks_root_path(repo)?;

    let mut success = true;
    let mut handle_result = |result: Result<String>| match result {
        Ok(msg) => log::info!("{msg}"),
        Err(err) => {
            success = false;
            log::error!("{err:#}");
        }
    };

    for (name, content) in TOPREPO_HOOKS {
        handle_result(write_hook(&hooks_root_path.join(name), content, &[], force));
    }
    for (name, expected_content) in ADDITIONAL_LFS_HOOKS {
        match remove_hook(&hooks_root_path.join(name), expected_content, force) {
            Ok(None) => {}
            Ok(Some(msg)) => handle_result(Ok(msg)),
            Err(err) => handle_result(Err(err)),
        }
    }
    Ok(success)
}
