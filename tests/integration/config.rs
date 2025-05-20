use assert_cmd::prelude::*;
use git_toprepo::config::GIT_CONFIG_KEY;
use git_toprepo::git::commit_env_for_testing;
use git_toprepo::git::git_command;
use git_toprepo::util::CommandExtension as _;
use predicates::prelude::*;
use std::io::Write;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

#[test]
fn test_validate_external_file_in_corrupt_repository() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    // Debug with &temp_dir.into_path() to persist the path.
    let temp_dir = temp_dir.path();

    // TODO: Set NO_COLOR here.
    let deterministic = commit_env_for_testing();

    let invalid_toml = "invalid.t.o.m.l";
    let mut invalid_tomlfile = std::fs::File::create(temp_dir.join(invalid_toml))?;
    writeln!(invalid_tomlfile, "nonesuch configuration. BEEP BOOP")?;

    let incorrect_config = "incorrect.toml";
    let mut incorrect_tomlfile = std::fs::File::create(temp_dir.join(incorrect_config))?;
    writeln!(incorrect_tomlfile, "[Wrong.Key]")?;

    let okay_config = "okay.toml";
    let mut okay_file = std::fs::File::create(temp_dir.join(okay_config))?;
    writeln!(okay_file, "{GENERIC_CONFIG}")?;

    git_command(temp_dir)
        .args(["init"])
        .envs(&deterministic)
        .check_success_with_stderr()?;

    git_command(temp_dir)
        .args(["config", GIT_CONFIG_KEY, &format!("local:{invalid_toml}")])
        .envs(&deterministic)
        .check_success_with_stderr()?;

    // NB: We do not need to initialize the history for this test.

    {
        let mut cmd = Command::cargo_bin("git-toprepo")?;
        cmd.current_dir(temp_dir).arg("config").arg("show");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "ERROR: Parsing local:{invalid_toml}: Could not parse TOML string",
            )));
    }

    // TODO: Rephrase the namespace in the error message. It looks ugly.
    // TODO: Verify that a TOML-parse-error exit code is used.

    {
        let mut cmd = Command::cargo_bin("git-toprepo")?;
        cmd.current_dir(temp_dir)
            .arg("config")
            .arg("validate")
            .arg(invalid_toml);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "ERROR: Loading config file {invalid_toml}: Could not parse TOML string",
            )))
            .stderr(predicate::str::contains("expected `.`, `=`"));
    }

    // TODO: Verify that a TOML-parse-error exit code is used.

    {
        let mut cmd = Command::cargo_bin("git-toprepo")?;
        cmd.current_dir(temp_dir)
            .arg("config")
            .arg("validate")
            .arg(incorrect_config);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "ERROR: Loading config file {incorrect_config}: Could not parse TOML string",
            )))
            .stderr(predicate::str::contains(
                "unknown field `Wrong`, expected `repo` or `log`",
            ));
    }

    {
        let mut cmd = Command::cargo_bin("git-toprepo")?;
        cmd.current_dir(temp_dir)
            .arg("config")
            .arg("validate")
            .arg(okay_config);
        cmd.assert().success();
    }

    Ok(())
}
