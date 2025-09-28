use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;

#[test]
fn local_config_resolution_in_worktree() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let worktree = temp_dir.join("worktree");
    git_command_for_testing(temp_dir.join("mono"))
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(worktree)
        .arg("fetch")
        .assert()
        .success();
}
