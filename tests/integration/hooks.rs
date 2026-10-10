use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::prepend_path_env;
use predicates::prelude::*;
use std::path::Path;

const GIT_LFS_HOOKS: [&str; 4] = ["pre-push", "post-checkout", "post-commit", "post-merge"];
const FILTER_LFS_SMUDGE: &str = "filter.lfs.smudge";
const FILTER_LFS_PROCESS: &str = "filter.lfs.process";

fn assert_hooks_without_git_lfs(repo: &Path) {
    assert!(repo.join(".git/hooks/pre-push").try_exists().unwrap());
    assert!(
        repo.join(".git/hooks/pre-push.toprepo")
            .try_exists()
            .unwrap()
    );
    assert!(
        std::fs::read_to_string(repo.join(".git/hooks/pre-push"))
            .unwrap()
            .contains("$0.toprepo")
    );
    for name in GIT_LFS_HOOKS {
        if name == "pre-push" {
            continue;
        }
        assert!(
            !repo.join(".git/hooks").join(name).try_exists().unwrap(),
            "Unexpected hook exists {name} hook"
        );
    }

    git_command_for_testing(repo)
        .args(["config", FILTER_LFS_SMUDGE])
        .assert()
        .code(1)
        .stdout("")
        .stderr("");
    git_command_for_testing(repo)
        .args(["config", FILTER_LFS_PROCESS])
        .assert()
        .code(1)
        .stdout("")
        .stderr("");
}

fn assert_hooks_with_git_lfs(repo: &Path) {
    assert_hooks_with_git_lfs_impl(repo, "");
}

fn assert_hooks_with_git_lfs_skip_smudge(repo: &Path) {
    assert_hooks_with_git_lfs_impl(repo, " --skip");
}

fn assert_hooks_with_git_lfs_impl(repo: &Path, extra_filter_args: &str) {
    assert!(repo.join(".git/hooks/pre-push").try_exists().unwrap());
    assert!(
        repo.join(".git/hooks/pre-push.toprepo")
            .try_exists()
            .unwrap()
    );
    assert!(
        std::fs::read_to_string(repo.join(".git/hooks/pre-push"))
            .unwrap()
            .contains("$0.toprepo")
    );
    for name in GIT_LFS_HOOKS {
        assert!(
            std::fs::read_to_string(repo.join(".git/hooks").join(name))
                .unwrap()
                .contains(&format!("git lfs {name}")),
            "Git LFS not called in {name} hook"
        );
    }

    git_command_for_testing(repo)
        .args(["config", FILTER_LFS_SMUDGE])
        .assert()
        .success()
        .stdout(format!("git toprepo lfs smudge{extra_filter_args}\n"))
        .stderr("");
    git_command_for_testing(repo)
        .args(["config", FILTER_LFS_PROCESS])
        .assert()
        .success()
        .stdout(format!(
            "git toprepo lfs filter-process{extra_filter_args}\n"
        ))
        .stderr("");
}

/// Check the installation of hooks with Git LFS.
#[test]
fn install_with_git_lfs() {
    let temp_dir =
        git_toprepo_testtools::test_util::maybe_keep_tempdir(tempfile::TempDir::new().unwrap());

    let repo = temp_dir.join("repo");
    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .arg(&repo)
        .assert()
        .success();

    let bin_dir = temp_dir.join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    git_toprepo::util::create_executable(bin_dir.join("git-lfs"), "#!/bin/sh\nexit 1").unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .arg("hooks")
        .arg("install")
        .arg("--git-lfs")
        .arg("--git-lfs-skip-smudge")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("Git LFS").not());
    assert_hooks_with_git_lfs_skip_smudge(&repo);
}

#[test]
fn install_bad_flag_combination() {
    let cmd = cargo_bin_git_toprepo_for_testing()
        .arg("hooks")
        .arg("install")
        .arg("--git-lfs-skip-smudge")
        .assert()
        .code(2)
        .stdout("");
    insta::assert_snapshot!(
        cmd.get_output().stderr.to_str().unwrap(),
        @"
    error: the following required arguments were not provided:
      --git-lfs

    Usage: git-toprepo hooks install --git-lfs --git-lfs-skip-smudge

    For more information, try '--help'.
    ",
    );
}

/// If Git LFS exists but the hooks are not being installed, an informative
/// message should be printed.
#[test]
fn install_without_git_lfs() {
    let temp_dir =
        git_toprepo_testtools::test_util::maybe_keep_tempdir(tempfile::TempDir::new().unwrap());

    let repo = temp_dir.join("repo");
    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .arg(&repo)
        .assert()
        .success();

    let bin_dir = temp_dir.join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    git_toprepo::util::create_executable(bin_dir.join("git-lfs"), "#!/bin/sh\nexit 0").unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .arg("hooks")
        .arg("install")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::contains("\nINFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.\n")
        );
    assert_hooks_without_git_lfs(&repo);

    // Git LFS not available.
    git_toprepo::util::write_executable(bin_dir.join("git-lfs"), "#!/bin/sh\nexit 1").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .arg("hooks")
        .arg("install")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("Git LFS").not());
    assert_hooks_without_git_lfs(&repo);
}

#[test]
fn overwrite_hooks_alternating_git_lfs() {
    let temp_dir =
        git_toprepo_testtools::test_util::maybe_keep_tempdir(tempfile::TempDir::new().unwrap());

    let repo = temp_dir.join("repo");
    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .arg(&repo)
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Written .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install", "--git-lfs"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Written .*post-checkout
INFO: Written .*post-commit
INFO: Written .*post-merge
WARN: Git Toprepo does not support automatic download of Git LFS objects. Please install hooks with --git-lfs-skip-smudge.
INFO: Written git-config filter.lfs.smudge
INFO: Written git-config filter.lfs.process
$",
            )
            .unwrap(),
        );
    assert_hooks_with_git_lfs(&repo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Removed .*post-checkout
INFO: Removed .*post-commit
INFO: Removed .*post-merge
INFO: Unset git-config filter.lfs.smudge
INFO: Unset git-config filter.lfs.process
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Verified .*pre-push
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install", "--git-lfs"])
        .assert()
        .success();
    assert_hooks_with_git_lfs(&repo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install", "--git-lfs", "--git-lfs-skip-smudge"])
        .assert()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Verified .*pre-push
INFO: Verified .*post-checkout
INFO: Verified .*post-commit
INFO: Verified .*post-merge
INFO: Rewritten git-config filter.lfs.smudge
INFO: Rewritten git-config filter.lfs.process
$",
            )
            .unwrap(),
        );
    assert_hooks_with_git_lfs_skip_smudge(&repo);
}

#[test]
fn overwrite_unexpected_content() {
    let temp_dir =
        git_toprepo_testtools::test_util::maybe_keep_tempdir(tempfile::TempDir::new().unwrap());

    let repo = temp_dir.join("repo");
    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .arg(&repo)
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Written .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);

    // Fail overwriting without force.
    std::fs::write(repo.join(".git/hooks/pre-push"), "Hello").unwrap();
    std::fs::write(repo.join(".git/hooks/post-commit"), "World").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
ERROR: Failed to create .*pre-push: File exists.*
ERROR: Unexpected content, won\'t delete .*post-commit
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/hooks/pre-push")).unwrap(),
        "Hello"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/hooks/post-commit")).unwrap(),
        "World"
    );

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["hooks", "install", "--force"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Removed .*post-commit
INFO: Git LFS is available. Add the Git LFS hooks using 'git toprepo lfs install'.
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);
}
