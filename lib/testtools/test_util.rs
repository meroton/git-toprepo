use std::ffi::OsStr;
use std::ffi::OsString;
use std::ops::Deref;

pub enum MaybePermanentTempDir {
    Keep(std::path::PathBuf),
    Discard(tempfile::TempDir),
}

impl MaybePermanentTempDir {
    /// Creates a new temporary directory, that may be kept permanently,
    /// with a certain prefix.
    ///
    /// See also [`maybe_keep_tempdir`].
    pub fn new_with_prefix(prefix: &str) -> Self {
        let tempdir = tempfile::TempDir::with_prefix(format!("{prefix}-"))
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
/// `GIT_TOPREPO_KEEP_TEMP_DIR` is set to `1`,
///
/// # Examples
///
/// See the unit tests.
// TODO: Maybe we should also require a name.
// When reusing fixtures between multiple integration tests we don't have a good
// way to see which is which on the filesystem.
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
