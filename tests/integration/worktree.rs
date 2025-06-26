use assert_cmd::prelude::*;
use std::process::Command;

#[test]
fn test_local_config_resolution_in_worktree() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );

    let temp_dir = temp_dir.path();
    let worktree = temp_dir.join("worktree");

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(worktree)
        .arg("fetch")
        .assert()
        .success();
}
