use std::ffi::OsStr;
use std::ffi::OsString;
use std::ops::Deref;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Like [`git_toprepo::git::git_command`] but also sets environment variables
/// for deterministic testing.
pub fn git_command_for_testing(repo: impl AsRef<std::ffi::OsStr>) -> assert_cmd::Command {
    // Inspired by gix-testtools v0.16.1 configure_command().
    let mut command = assert_cmd::Command::new("git");
    command.args([std::ffi::OsStr::new("-C"), repo.as_ref()]);
    apply_git_env(&mut command);
    command
}

/// Like [`git_toprepo::git::git_command`] but also sets environment variables
/// for deterministic testing.
pub fn cargo_bin_git_toprepo_for_testing() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("git-toprepo").unwrap()
}

fn apply_git_env(command: &mut assert_cmd::Command) {
    // Inspired by gix-testtools v0.16.1 configure_command().
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_NAME", "A Name")
        .env("GIT_AUTHOR_EMAIL", "a@no.example")
        .env("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00")
        .env("GIT_COMMITTER_NAME", "C Name")
        .env("GIT_COMMITTER_EMAIL", "c@no.example")
        .env("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00")
        .env("GIT_CONFIG_COUNT", "0");
}

pub enum MaybePermanentTempDir {
    Keep(std::path::PathBuf),
    Discard(tempfile::TempDir),
}

impl MaybePermanentTempDir {
    /// Creates a new temporary directory, that may be kept permanently.
    ///
    /// The thread name is used because rust test sets the thread name to the
    /// test name, so why not use that for test purpose.
    ///
    /// See also [`maybe_keep_tempdir`].
    pub fn create() -> Self {
        let prefix = if let Some(name) = std::thread::current().name() {
            &format!("git_toprepo-{}", name.replace("::", "-"))
        } else {
            "git_toprepo"
        };
        let tempdir = tempfile::TempDir::with_prefix(prefix)
            .expect("successful temporary directory creation");
        maybe_keep_tempdir(tempdir)
    }

    pub fn path(&self) -> &std::path::Path {
        match self {
            MaybePermanentTempDir::Keep(path) => path,
            MaybePermanentTempDir::Discard(tempdir) => tempdir.path(),
        }
    }
}

impl Deref for MaybePermanentTempDir {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

impl AsRef<std::path::Path> for MaybePermanentTempDir {
    fn as_ref(&self) -> &std::path::Path {
        self.path()
    }
}

impl AsRef<OsStr> for MaybePermanentTempDir {
    fn as_ref(&self) -> &OsStr {
        self.path().as_os_str()
    }
}

impl From<tempfile::TempDir> for MaybePermanentTempDir {
    /// Persist a temporary directory to disk if the environment variable
    /// `GIT_TOPREPO_KEEP_TEMP_DIR` is set to `1`,
    ///
    /// # Examples
    ///
    /// See the unit tests.
    fn from(tempdir: tempfile::TempDir) -> Self {
        let keep_var = std::env::var_os("GIT_TOPREPO_KEEP_TEMP_DIR");
        maybe_keep_tempdir_impl(tempdir, keep_var)
    }
}

/// Persist a temporary directory to disk if the environment variable
/// `GIT_TOPREPO_KEEP_TEMP_DIR` is set to `1`.
///
/// To find the temporary directory, run one test at a time or make it fail and find
/// the path on stderr.
///
/// # Examples
///
/// See the unit tests.
pub fn maybe_keep_tempdir(tempdir: tempfile::TempDir) -> MaybePermanentTempDir {
    MaybePermanentTempDir::from(tempdir)
}

pub(crate) fn maybe_keep_tempdir_impl(
    tempdir: tempfile::TempDir,
    keep_var: Option<OsString>,
) -> MaybePermanentTempDir {
    if keep_var == Some("1".into()) {
        let tempdir = tempdir.keep();
        eprintln!("Keeping temporary directory: {}", tempdir.display());
        MaybePermanentTempDir::Keep(tempdir)
    } else {
        MaybePermanentTempDir::Discard(tempdir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Test that the temporary directory is discarded by default.
    #[rstest]
    #[case::env_unset(None)]
    #[case::env_set_to_0(Some("0".into()))]
    #[case::env_set_to_false(Some("false".into()))]
    #[case::env_set_to_no(Some("no".into()))]
    #[case::env_set_to_off(Some("off".into()))]
    #[case::env_set_to_true(Some("true".into()))]
    #[case::env_set_to_yes(Some("yes".into()))]
    #[case::env_set_to_on(Some("on".into()))]
    fn discard_tempdir(#[case] env_var: Option<OsString>) {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp_dir = maybe_keep_tempdir_impl(tmp_dir, env_var);
        let tmp_path = tmp_dir.path().to_owned();

        // Delete the temporary directory.
        drop(tmp_dir);
        assert!(!tmp_path.exists());
    }

    /// Test that the temporary directory can be kept.
    #[test]
    fn keep_tempdir() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let tmp_dir = maybe_keep_tempdir_impl(tmp_dir, Some("1".into()));
        let tmp_path = tmp_dir.path().to_owned();

        // Keep the temporary directory.
        drop(tmp_dir);
        assert!(tmp_path.is_dir());
        std::fs::remove_dir_all(tmp_path).unwrap();
    }
}
