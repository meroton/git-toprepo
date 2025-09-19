use assert_cmd::prelude::*;
use git_toprepo::config::COULD_NOT_PARSE_TOML_STRING;
use git_toprepo::config::TOPREPO_CONFIG_FILE_KEY;
use git_toprepo::config::toprepo_git_config;
use git_toprepo::git::commit_env_for_testing;
use git_toprepo::git::git_command;
use git_toprepo::util::CommandExtension;
use predicates::prelude::*;
use std::path::Path;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

const GIT_REVIEW_CONFIG: &str = r#"[gerrit]
    host=gerrit.server.example
    project=path/to/repository.git
    port=2222"#;

#[test]
fn test_parse_gitreview() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-gerrit-").unwrap();
    // Debug with &temp_dir.into_path() to persist the path.
    // TODO: Parameterize all integrations tests to keep their temporary files.
    // Possibly with an environment variable?
    let temp_dir = temp_dir.path();

    let gitreview = ".gitreview";
    std::fs::write(temp_dir.join(gitreview), GIT_REVIEW_CONFIG).unwrap();

    let toprepo = ".gittoprepo.toml";
    std::fs::write(temp_dir.join(toprepo), GENERIC_CONFIG).unwrap();

    let deterministic = commit_env_for_testing();
    git_command(temp_dir)
        .args(["init"])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();

    git_command(temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            &format!("local:{toprepo}"),
        ])
        .envs(&deterministic)
        .check_success_with_stderr()
        .unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("dump")
        .arg("gerrit")
        .arg("host")
        .assert()
        .success()
        .stdout(predicate::str::contains("gerrit.server.example"));

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("dump")
        .arg("gerrit")
        .arg("project")
        .assert()
        .success()
        .stdout(predicate::str::contains("path/to/repository.git"));
}

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
        .assert()
        .success();

    git_command(&temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            &format!("worktree:{invalid_toml}"),
        ])
        .envs(&deterministic)
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
        .stderr(predicate::str::contains(
            COULD_NOT_PARSE_TOML_STRING,
        ))
        .stderr(predicate::str::contains(format!(
            "ERROR: Parsing worktree:{invalid_toml}:",
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

// TODO: Create a bootstrap test inside a monorepo,
// it should give an error then.

#[test]
fn test_config_bootstrap() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    const EXPECTED_BOOTSTRAP_CONFIG: &str = "\
[repo.sub]
urls = [\"../sub/\"]
skip_expanding = []
";

    Command::new("git")
        .current_dir(&toprepo)
        .args(["rm", ".gittoprepo.toml"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&toprepo)
        .args(["commit", "-m", "Remove toprepo config"])
        .envs(commit_env_for_testing())
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
