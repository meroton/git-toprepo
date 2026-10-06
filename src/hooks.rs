use crate::git::git_command;
use crate::log::CommandSpanExt as _;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use std::path::Path;
use std::path::PathBuf;

struct Hook {
    pub name: &'static str,
    pub content: &'static str,
}

const HOOKS: &[Hook] = &[
    Hook {
        name: "pre-push.toprepo",
        content: include_str!("hooks/pre-push.toprepo"),
    },
    Hook {
        name: "pre-push",
        content: include_str!("hooks/pre-push"),
    },
];

impl Hook {
    /// Checks if given content is acceptable to overwrite.
    pub fn content_can_be_overwritten(&self, content: &str) -> bool {
        self.content == content
    }

    /// Writes `content` as an executable file. If the file already exists, it will
    /// only be overwritten if the content matches one of the acceptable contents or
    /// if the `force` flag is set.
    ///
    /// Returns a human readable message about the action taken.
    fn write(&self, hooks_root: &Path, force: bool) -> Result<String> {
        let path = &hooks_root.join(self.name);
        let mut allow_overwrite = force;
        let existing_content = std::fs::read_to_string(path);
        if let Ok(existing_content) = &existing_content
            && crate::util::is_executable(path)
        {
            if existing_content == self.content {
                return Ok(format!("Verified {}", path.display()));
            }
            if !allow_overwrite && self.content_can_be_overwritten(existing_content) {
                allow_overwrite = true;
            }
        }
        // Write or overwrite.
        if allow_overwrite {
            crate::util::overwrite_executable(path, self.content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        } else {
            crate::util::create_executable(path, self.content)
                .with_context(|| format!("Failed to create {}", path.display()))?;
        }
        Ok(format!("Written {}", path.display()))
    }
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

    for hook in HOOKS {
        handle_result(hook.write(&hooks_root_path, force));
    }
    Ok(success)
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
