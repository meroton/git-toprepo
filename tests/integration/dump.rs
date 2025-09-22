use assert_cmd::prelude::*;
use git_toprepo::git::git_command_for_testing;
use predicate::str::contains;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn dump_git_modules() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_a.sh",
        )
        .unwrap(),
    );

    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::create_dir(monorepo.join("subdir")).unwrap();

    // Only update the fetch url, which is not possible with one call to
    // git-remote.
    git_command_for_testing(&monorepo)
        .arg("config")
        .arg("remote.origin.url")
        .arg("ssh://gerrit.example/main/project.git")
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout("main/project.git .\nmain/subx subx\n");
    // Test a subdirectory which is not a submodule.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(monorepo.join("subdir"))
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout("main/project.git .\nmain/subx subx\n");
    // Test a subdirectory which is not an integrated submodule.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(monorepo.join("subx"))
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout("main/project.git .\nmain/subx subx\n");

    // Without any remote, dumping git-modules will fail.
    git_command_for_testing(&monorepo)
        .arg("remote")
        .arg("remove")
        .arg("origin")
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("git-modules")
        .assert()
        .failure()
        .stderr(contains("Loading the main repo Gerrit project"));
}

#[test]
fn wrong_cache_prelude() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .assert()
        .success();
    git_command_for_testing(&temp_dir)
        .args(["config", "toprepo.config", "worktree:.gittoprepo.toml"])
        .assert()
        .success();
    std::fs::write(temp_dir.join(".gittoprepo.toml"), "").unwrap();

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
