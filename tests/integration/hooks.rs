use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::prepend_path_env;
use predicates::prelude::*;
use std::path::Path;

fn assert_hooks_ok(repo: &Path) {
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
}

#[test]
fn install_fails_outside_monorepo() {
    let temp_dir =
        git_toprepo_testtools::test_util::maybe_keep_tempdir(tempfile::TempDir::new().unwrap());

    let normal_repo = temp_dir.join("repo");
    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .arg(&normal_repo)
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&normal_repo)
        .arg("hooks")
        .arg("install")
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            "ERROR: git-config 'toprepo.config' is missing. Is this an initialized Git Toprepo?\n",
        );
}

#[test]
fn write_hooks() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");

    let bin_dir = temp_dir.join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    git_toprepo::util::create_executable(bin_dir.join("git-lfs"), "#!/bin/sh\nexit 1").unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("hooks")
        .arg("install")
        .env("PATH", prepend_path_env(&bin_dir))
        .assert()
        .success();
    assert_hooks_ok(&monorepo);
}

#[test]
fn overwrite_hooks() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["hooks", "install"])
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
    assert_hooks_ok(&monorepo);

    // Try installing twice.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["hooks", "install"])
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
    assert_hooks_ok(&monorepo);
}

#[test]
fn overwrite_unexpected_content() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["hooks", "install"])
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
    assert_hooks_ok(&monorepo);

    // Fail overwriting without force.
    std::fs::write(monorepo.join(".git/hooks/pre-push"), "Hello").unwrap();
    std::fs::write(monorepo.join(".git/hooks/post-commit"), "World").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["hooks", "install"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
ERROR: Failed to create .*pre-push: File exists.*
$",
            )
            .unwrap(),
        );
    assert_eq!(
        std::fs::read_to_string(monorepo.join(".git/hooks/pre-push")).unwrap(),
        "Hello"
    );
    assert_eq!(
        std::fs::read_to_string(monorepo.join(".git/hooks/post-commit")).unwrap(),
        "World"
    );

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["hooks", "install", "--force"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^\
INFO: Verified .*pre-push\\.toprepo
INFO: Written .*pre-push
$",
            )
            .unwrap(),
        );
    assert_hooks_ok(&monorepo);
}
