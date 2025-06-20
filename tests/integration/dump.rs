use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

const GENERIC_CONFIG: &str = r#"
    [repo]
    [repo.foo.fetch]
    url = "ssh://generic/repo.git"
"#;

// TODO: Keep this test in mind when refactoring top-repo creations in the all
// code paths and subcommands. This is the first error that `toprepo-dump`
// encounters in a directory without git. But it is not unique to the dump
// subcommand. `toprepo-fetch` typically fails for a missing toprepo config file
// (or git-config for it) but at some point it could try to access the git
// information if really cajoled and would then have this error as well.
#[test]
fn test_dump_outside_git_repo() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_dump_outside_git_repo",
    );

    std::fs::write(temp_dir.join(".gittoprepo.toml"), GENERIC_CONFIG).unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        // An arbitrary subcommand that requires it to be initialized
        .arg("dump")
        .arg("import-cache")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            git_toprepo::repo::COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY,
        ));
}
