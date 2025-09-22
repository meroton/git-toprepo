use assert_cmd::prelude::*;
use git_toprepo::config::TOPREPO_CONFIG_FILE_KEY;
use git_toprepo::config::toprepo_git_config;
use git_toprepo::git::git_command_for_testing;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

#[test]
fn validate_external_file_in_corrupt_repository() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

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

    git_command_for_testing(&temp_dir)
        .args(["init"])
        .assert()
        .success();

    git_command_for_testing(&temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            &format!("worktree:{invalid_toml}"),
        ])
        .assert()
        .success();

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

    // TODO: 2025-09-22 Rephrase the namespace in the error message. It looks ugly.
    // TODO: 2025-09-22 Verify that a TOML-parse-error exit code is used.

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
        .stderr(predicate::str::contains("key with no value, expected `=`"));

    // TODO: 2025-09-22 Verify that a TOML-parse-error exit code is used.

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
            "unknown field `Wrong`, expected `fetch` or `repo`",
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
fn validate_use_correct_working_directory() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

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
    git_command_for_testing(&temp_dir)
        .args(["init"])
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

#[test]
fn bootstrap() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    git_command_for_testing(&toprepo)
        .args(["rm", ".gittoprepo.toml"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Remove toprepo config"])
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("git-toprepo config bootstrap"));

    const EXPECTED_BOOTSTRAP_CONFIG: &str = "\
[repo.sub]
urls = [\"../sub/\"]
skip_expanding = []
";
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["config", "bootstrap"])
        .assert()
        .success()
        .stdout(EXPECTED_BOOTSTRAP_CONFIG)
        .stderr(predicate::str::contains("ERROR:").not())
        .stderr(predicate::str::contains("WARN:").not());
}
