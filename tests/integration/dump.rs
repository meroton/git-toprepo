use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

#[test]
fn test_dump_outside_git_repo() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-dump-").unwrap();
    // Debug with &temp_dir.into_path() to persist the path.
    // TODO: Parameterize all integrations tests to keep their temporary files.
    // Possibly with an environment variable?
    let temp_dir = temp_dir.path();

    std::fs::write(temp_dir.join(".gittoprepo.toml"), GENERIC_CONFIG).unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        // An arbitrary subcommand that requires it to be initialized
        .arg("dump")
        .arg("import-cache")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            git_toprepo::repo::COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY,
        ));
}
