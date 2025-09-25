use assert_cmd::prelude::*;
use git_toprepo::git::git_command_for_testing;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn info_outside_repo_should_fail() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("info")
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains(
            "ERROR: Could not find a git repository",
        ));
}

#[test]
fn info_specific_value() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir.join("mono"))
        .args(["info", "config-location"])
        .assert()
        .success()
        .stdout("local:.gittoprepo.toml\n")
        .stderr("");
}

#[test]
fn info_in_monorepo_worktree() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let worktree = temp_dir.join("worktree");

    git_command_for_testing(&monorepo)
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    let expected_info = format!(
        r#"
config-location local:.gittoprepo.toml
current-worktree {worktree}
cwd {worktree}
git-dir {git_dir}
main-worktree {monorepo}
version "#,
        worktree = worktree.to_string_lossy(),
        git_dir = monorepo.join(".git/worktrees/worktree").to_string_lossy(),
        monorepo = monorepo.to_string_lossy(),
    );
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&worktree)
        .arg("info")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(&expected_info[1..]))
        .stderr("");

    let expected_info = format!(
        r#"
config-location local:.gittoprepo.toml
current-worktree {monorepo}
cwd {monorepo}
git-dir {git_dir}
main-worktree {monorepo}
version "#,
        monorepo = monorepo.to_string_lossy(),
        git_dir = monorepo.join(".git").to_string_lossy(),
    );
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("info")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(&expected_info[1..]))
        .stderr("");
}

#[test]
fn info_basic_git_repo() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    git_command_for_testing(&temp_dir)
        .args(["init", "--initial-branch", "main"])
        .assert()
        .success();
    let subdir = temp_dir.join("sub");
    std::fs::create_dir(&subdir).unwrap();

    let space = " ";
    let expected_info = format!(
        r#"
config-location{space}
current-worktree {repo}
cwd {subdir}
git-dir {git_dir}
main-worktree {repo}
version "#,
        repo = temp_dir.to_string_lossy(),
        git_dir = temp_dir.join(".git").to_string_lossy(),
        subdir = subdir.to_string_lossy()
    );
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&subdir)
        .args(["info"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(&expected_info[1..]))
        .stderr("WARN: No main worktree: git-config 'toprepo.config' is missing. Is this an initialized git-toprepo?\n");

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("-C")
        .arg(&subdir)
        .args(["info", "cwd"])
        .assert()
        .success()
        .stdout(subdir.to_string_lossy().to_string() + "\n")
        .stderr("");
}
