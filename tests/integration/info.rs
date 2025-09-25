use assert_cmd::prelude::*;
use git_toprepo::git::git_command_for_testing;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn outside_repo_should_fail() {
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
fn print_specific_value() {
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
fn print_in_monorepo_worktree() {
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
import-cache {import_cache}
main-worktree {monorepo}
version "#,
        git_dir = monorepo.join(".git/worktrees/worktree").to_string_lossy(),
        import_cache = monorepo
            .join(".git/worktrees/worktree/../../toprepo/import-cache.bincode")
            .to_string_lossy(),
        monorepo = monorepo.to_string_lossy(),
        worktree = worktree.to_string_lossy(),
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
import-cache {import_cache}
main-worktree {monorepo}
version "#,
        git_dir = monorepo.join(".git").to_string_lossy(),
        import_cache = monorepo
            .join(".git/toprepo/import-cache.bincode")
            .to_string_lossy(),
        monorepo = monorepo.to_string_lossy(),
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
fn print_in_basic_git_repo() {
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
import-cache {import_cache}
main-worktree {repo}
version "#,
        git_dir = temp_dir.join(".git").to_string_lossy(),
        import_cache = temp_dir
            .join(".git/toprepo/import-cache.bincode")
            .to_string_lossy(),
        repo = temp_dir.to_string_lossy(),
        subdir = subdir.to_string_lossy(),
    );
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&subdir)
        .args(["info"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(&expected_info[1..]))
        .stderr(
            "WARN: git-config 'toprepo.config' is missing. Is this an initialized git-toprepo?\n",
        );

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

#[test]
fn flag_is_monorepo() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let subdir = temp_dir.join("sub");
    std::fs::create_dir(&subdir).unwrap();

    // Without a git repository.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .args(["info", "--is-monorepo"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::starts_with(
            "ERROR: Could not find a git repository in ",
        ));
    // --is-monorepo and a value should fail.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .args(["info", "--is-monorepo", "cwd"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains(
            "error: the argument '--is-monorepo' cannot be used with '[VALUE]'\n",
        ));

    // In a basic git repository.
    git_command_for_testing(&temp_dir)
        .args(["init", "--initial-branch", "main"])
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .args(["info", "--is-monorepo"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(
            "WARN: git-config \'toprepo.config\' is missing. Is this an initialized git-toprepo?\n",
        );

    // In a git-toprepo emulated monorepo.
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let sub = monorepo.join("sub");
    std::fs::create_dir(&sub).unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["info", "--is-monorepo"])
        .assert()
        .success()
        .stdout("")
        .stderr("");
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&sub)
        .args(["info", "--is-monorepo"])
        .assert()
        .success()
        .stdout("")
        .stderr("");
}
