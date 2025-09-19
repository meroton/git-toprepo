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
// TODO: Create a specific error message for no git.
//       Though it is mostly important that it is a NotAMonorepo error.
//       Whether it is git or not is actually secondary here.
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
            "NotAMonorepo",
        ));
}

#[test]
fn test_dump_git_modules() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let project = "top";
    let child_dir = monorepo.join("subx");

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout(contains(project));

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&child_dir)
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout(contains(project));

    // TODO: Make sure the inner and outer call have different paths.
    //       Recent refactorings have partially solved
    //           dump modules only works in the root · Issue #163 · meroton/git-toprepo
    //           https://github.com/meroton/git-toprepo/issues/163
    //       It can now run in sub directories and gives the root-relative paths
    //       for the module. We'd rather have it have relative paths from the current
    //       working directory.
    /*
     * assert!(outer != inner);
     */
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

    // Look for a sane warning message.
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
