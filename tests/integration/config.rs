use assert_cmd::prelude::*;
use git_toprepo::config::GIT_CONFIG_KEY;
use git_toprepo::git::commit_env_for_testing;
use git_toprepo::git::git_command;
use git_toprepo::util::CommandExtension as _;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

#[test]
fn test_validate_external_file_in_corrupt_repository() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_validate_external_file_in_corrupt_repository",
    );

    // TODO: Set NO_COLOR here.
    let deterministic = commit_env_for_testing();

    let invalid_toml = "invalid.t.o.m.l";
    std::fs::write(
        temp_dir.join(invalid_toml),
        "nonesuch configuration. BEEP BOOP",
    )
    .unwrap();

    let incorrect_config = "incorrect.toml";
    std::fs::write(temp_dir.join(incorrect_config), "[Wrong.Key]").unwrap();

    let okay_config = "okay.toml";
    std::fs::write(temp_dir.join(okay_config), GENERIC_CONFIG).unwrap();

    git_command(&temp_dir)
        .args(["init"])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();

    git_command(&temp_dir)
        .args([
            "config",
            GIT_CONFIG_KEY,
            &format!("worktree:{invalid_toml}"),
        ])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();

    // NB: We do not need to initialize the history for this test.

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("show")
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "ERROR: Parsing worktree:{invalid_toml}: Could not parse TOML string",
        )));

    // TODO: Rephrase the namespace in the error message. It looks ugly.
    // TODO: Verify that a TOML-parse-error exit code is used.

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(invalid_toml)
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "ERROR: Loading config file {invalid_toml}: Could not parse TOML string",
        )))
        .stderr(predicate::str::contains("expected `.`, `=`"));

    // TODO: Verify that a TOML-parse-error exit code is used.

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(incorrect_config)
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "ERROR: Loading config file {incorrect_config}: Could not parse TOML string",
        )))
        .stderr(predicate::str::contains(
            "unknown field `Wrong`, expected `repo` or `log`",
        ));

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(okay_config)
        .assert()
        .success();
}

#[test]
fn test_config_commands_use_correct_working_directory() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_config_commands_use_correct_working_directory",
    );

    let okay_config = "okay.toml";
    std::fs::write(temp_dir.join(okay_config), GENERIC_CONFIG).unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(okay_config)
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("-C")
        .arg(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(okay_config)
        .assert()
        .success();

    // Try to run from a subdirectory inside a git repo.
    Command::new("git")
        .current_dir(&temp_dir)
        .arg("init")
        .assert()
        .success();
    let subdir = temp_dir.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&subdir)
        .arg("config")
        .arg("validate")
        .arg(Path::new("..").join(okay_config))
        .assert()
        .success();
}
