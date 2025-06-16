use assert_cmd::Command;
use std::path::Path;

/// Sets up an example repo structure. The example repo consists of the
/// subdirectories `top` and `sub`, with `sub` being a submodule in `top`. The
/// commit history is shown below:
/// ```text
/// top  A---B---C---D-------E---F---G----Ha--Ia-----J---------------N
///          |       |       |       |\    |   |   / | \      \     /|
///          |       |       |       | Hb--------Ib  |  K---L--M(10) |
///          |       |       |       |  |  |   |  |  |  |   |        |
/// sub  1---2-------3---4---5---6---7----8a--9a----10-------------13
///                                   \ |         | /   |   |      /
///                                    8b--------9b-----11--12----/
/// ```
/// The commit N is pointing to commit 11 in the submodule, which is a bad merge
/// because even if N keeps the submodule K was pointing to, the submodule
/// pointer goes backwards in relation to M.
///
/// # Examples
///
/// ```rust
/// // See [`git_toprepo_testtools::test_util::maybe_keep_tempdir`] how to set
/// // the environment variable to persist the temporary directory.
/// let tmp_path = readme_example_tempdir();
/// let top_repo_path = tmp_path.join("top");
/// assert!(!top_repo_path.exists());
/// ```
pub fn readme_example_tempdir() -> git_toprepo_testtools::test_util::MaybePermanentTempDir {
    git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_readme_example.sh")
            .unwrap(),
    )
}

pub fn clone(toprepo: &Path, monorepo: &Path) {
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(toprepo)
        .arg(monorepo)
        .assert()
        .success();
}
