use git_toprepo::config::TOPREPO_CONFIG_FILE_KEY;
use git_toprepo::config::toprepo_git_config;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use predicates::prelude::*;
use std::path::Path;
use bstr::ByteSlice as _;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

#[test]
fn create_config_from_invalid_ref() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

    git_command_for_testing(&temp_dir)
        .args(["init"])
        .assert()
        .success();

    git_command_for_testing(&temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            "should:worktree:foo.toml",
        ])
        .assert()
        .success();
    git_command_for_testing(&temp_dir)
        .args([
            "config",
            "--add",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            "may:local:bar.toml",
        ])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("show")
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            "WARN: Config file \"foo.toml\" does not exist in the worktree\n\
            ERROR: No configuration exists, looked at may:local:bar.toml, should:worktree:foo.toml\n",
        );
}

#[test]
fn missing_config() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

    git_command_for_testing(&temp_dir)
        .args(["init"])
        .assert()
        .success();

    git_command_for_testing(&temp_dir)
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
        .assert()
        .success();

    // Try a path in the repository.
    git_command_for_testing(&temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            "unreachable",
        ])
        .assert()
        .success();
    git_command_for_testing(&temp_dir)
        .args([
            "config",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            "must:repo:HEAD:.gittoprepo.toml",
        ])
        .assert()
        .success();
    git_command_for_testing(&temp_dir)
        .args([
            "config",
            "--add",
            &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
            "should:worktree:nonexisting.toml",
        ])
        .assert()
        .success();

    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("location")
        .assert()
        .code(1)
        .stdout("");

    let stderr = cmd.get_output().stderr.to_str().unwrap();
    let errors = vec![
        // Modern git.
        "WARN: Config file \"nonexisting.toml\" does not exist in the worktree\n\
        ERROR: Config file .gittoprepo.toml does not exist in HEAD: exit status: 128: \
        fatal: path '.gittoprepo.toml' does not exist in 'HEAD'\n\
        ERROR: None of the configured git-toprepo locations did exist\n",
        // Git version 2.34.1
        "WARN: Config file \"nonexisting.toml\" does not exist in the worktree\n\
        ERROR: Config file .gittoprepo.toml does not exist in HEAD: exit status: 128: \
        fatal: Not a valid object name HEAD:.gittoprepo.toml\n\
        ERROR: None of the configured git-toprepo locations did exist\n",
    ];
    if !(errors.contains(&stderr)) {
        eprintln!(r#"Unexpected error: "{stderr}", must be either from {errors:?}"#);
        assert!(false);
    }


}

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
            &format!("should:worktree:{invalid_toml}"),
        ])
        .assert()
        .success();

    // NB: We do not need to initialize the history for this test.

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("show")
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "Parsing worktree:{invalid_toml}: Could not parse TOML string"
        )));

    // TODO: 2025-09-22 Rephrase the namespace in the error message. It looks ugly.
    // TODO: 2025-09-22 Verify that a TOML-parse-error exit code is used.

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("config")
        .arg("validate")
        .arg(okay_config)
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
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
    let subdir = temp_dir.join("repodir");
    std::fs::create_dir(&subdir).unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&subdir)
        .arg("config")
        .arg("validate")
        .arg(Path::new("..").join(okay_config))
        .assert()
        .success();
}

#[test]
fn bootstrap_after_clone() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    let expected_boostrap_config = &r#"
[repo.repo]
urls = ["../repo/"]
missing_commits = []
"#[1..];

    git_command_for_testing(&toprepo)
        .args(["rm", ".gittoprepo.toml"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Remove toprepo config"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("git-toprepo config bootstrap"));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["config", "bootstrap"])
        .assert()
        .success()
        .stdout(expected_boostrap_config)
        .stderr(predicate::str::contains("ERROR:").not())
        .stderr(predicate::str::contains("WARN:").not());
}

#[test]
fn bootstrap_on_existing() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let expected_boostrap_config: &str = &r#"
[repo.repox]
urls = ["../repox/"]
missing_commits = []

[repo.repoy]
urls = ["../repoy/"]
missing_commits = []
"#[1..];

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["config", "bootstrap"])
        .assert()
        .success()
        .stdout(expected_boostrap_config)
        .stderr(predicate::str::contains("ERROR:").not())
        .stderr(predicate::str::contains("WARN:").not());
}

#[test]
/// The URLs in the `.gitmodules` file at HEAD are assumed to be the current
/// ones. Set them as `fetch.url` in the config.
fn bootstrap_multiple_urls_in_history() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    std::fs::write(
        toprepo.join(".gitmodules"),
        r#"
[submodule "repox"]
	path = subpathx
	url = https://other.example/repox
[submodule "repoy"]
	path = subpathy
	url = https://other.example/repoy.git
"#,
    )
    .unwrap();
    git_command_for_testing(&toprepo)
        .args(["add", ".gitmodules"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "New urls"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["rm", "subpathy"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Remove suby"])
        .assert()
        .success();

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    // subx has an URL in HEAD:.gitmodules, suby does not and therfore becomes
    // disabled.
    let expected_boostrap_config = &r#"
[repo.repox]
urls = ["../repox/", "https://other.example/repox"]
missing_commits = []

[repo.repox.fetch]
url = "https://other.example/repox"

[repo.repoy]
urls = ["../repoy/", "https://other.example/repoy.git"]
enabled = false
missing_commits = []
"#[1..];

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["config", "bootstrap"])
        .assert()
        .success()
        .stdout(expected_boostrap_config)
        .stderr(predicate::str::contains("ERROR:").not())
        .stderr(predicate::str::contains("WARN:").not());
}
