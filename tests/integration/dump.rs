use assert_cmd::prelude::*;
use git_toprepo::git::git_command;
use predicate::str::contains;
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
        .stderr(contains(
            git_toprepo::repo::COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY,
        ));
}

#[test]
fn test_dump_git_modules() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_a.sh",
        )
        .unwrap(),
    );

    let project = "main/project";
    let temp_dir = temp_dir.path().join("top");

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        // An arbitrary subcommand that requires it to be initialized
        .arg("dump")
        .arg("git-modules")
        .assert()
        .failure()
        .stderr(contains("Loading the main repo Gerrit project"));

    Command::new("git")
        .current_dir(&temp_dir)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(format!("ssh://gerrit.example/{project}.git"))
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        // An arbitrary subcommand that requires it to be initialized
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout(contains(project));
}

#[test]
fn test_wrong_cache_prelude() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_wrong_cache_prelude",
    );

    git_command(&temp_dir)
        .args(["init", "--quiet"])
        .assert()
        .success();
    let git_dir = temp_dir.join(".git");
    let cache_path = git_toprepo::repo_cache_serde::SerdeTopRepoCache::get_cache_path(&git_dir);
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, "wrong-#cache-format").unwrap();

    // Look for a sance warning message.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("dump")
        .arg("import-cache")
        .assert()
        .success()
        .stderr(predicates::str::is_match(
            "WARN: Discarding toprepo cache .* due to version mismatch, expected \"#cache-format-v2\"\n").unwrap()
        );
}
