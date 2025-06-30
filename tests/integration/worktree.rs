use assert_cmd::prelude::*;
use std::process::Command;


#[test]
fn test_local_config_resolution_in_worktree() {
    let base_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_local_config_resolution_fails_in_worktree",
    );
    let from_path = &base_dir.path().join("from");
    std::fs::create_dir(from_path).unwrap();

    git_toprepo::git::git_command(from_path)
        .args(["init", "--quiet", "--initial-branch", "main"])
        .assert()
        .success();

    std::fs::write(from_path.join(".gittoprepo.toml"), "").unwrap();

    git_toprepo::git::git_command(from_path)
        .args(["config", "toprepo.config", "local:.gittoprepo.toml"])
        .assert()
        .success();

    let worktree_path = &base_dir.path().join("worktree");
    git_toprepo::git::git_command(from_path)
        .args(["worktree", "add", worktree_path.to_str().unwrap()])
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(worktree_path)
        .arg("fetch")
        .assert()
        .success();
}
