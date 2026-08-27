use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::prepend_path_env;
use predicates::prelude::*;
use std::path::Path;

const GIT_LFS_HOOKS: [&str; 4] = ["pre-push", "post-checkout", "post-commit", "post-merge"];

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
}

fn assert_hooks_with_git_lfs(repo: &Path) {
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
}

/// Check that the auto detection of Git LFS works and installs the Git LFS hooks.
#[test]
fn write_hooks_with_git_lfs_installed() {
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
        .arg("git-hooks")
        .arg("install")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success();
    assert_hooks_with_git_lfs(&repo);
}

/// Check that the auto detection of Git LFS works and skips the Git LFS hooks.
#[test]
fn write_hooks_without_git_lfs_installed() {
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
        .arg("git-hooks")
        .arg("install")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success();
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
        .args(["git-hooks", "install", "--git-lfs=no"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Written .*pre-push\\.toprepo
INFO: Written .*pre-push
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["git-hooks", "install", "--git-lfs=yes"])
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
$",
            )
            .unwrap(),
        );
    assert_hooks_with_git_lfs(&repo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["git-hooks", "install", "--git-lfs=no"])
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
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["git-hooks", "install", "--git-lfs=no"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Verified .*pre-push
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["git-hooks", "install", "--git-lfs=yes"])
        .assert()
        .success();
    assert_hooks_with_git_lfs(&repo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo)
        .args(["git-hooks", "install", "--git-lfs=yes"])
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
$",
            )
            .unwrap(),
        );
    assert_hooks_with_git_lfs(&repo);
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
        .args(["git-hooks", "install", "--git-lfs=no"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Written .*pre-push\\.toprepo
INFO: Written .*pre-push
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
        .args(["git-hooks", "install", "--git-lfs=no"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
ERROR: Failed to create .*pre-push: File exists.*
ERROR: Unexpected content, won\'t delete .*post-commit
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
        .args(["git-hooks", "install", "--git-lfs=no", "--force"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Written .*pre-push
INFO: Removed .*post-commit
$",
            )
            .unwrap(),
        );
    assert_hooks_without_git_lfs(&repo);
}
