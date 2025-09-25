use assert_cmd::prelude::*;
use bstr::ByteSlice;
use git_toprepo::git::git_command_for_testing;
use git_toprepo_testtools::test_util::MaybePermanentTempDir;
use predicate::str::contains;
use predicates::prelude::*;
use rstest::rstest;
use std::io::Write;
use std::path::PathBuf;
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

// The code should not attempt to read git-toprepo's cache if the directory is
// not a monorepo.
#[test]
fn cache_from_basic_repo_should_fail() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();

    git_command_for_testing(&temp_dir)
        .args(["init", "--quiet"])
        .assert()
        .success();

    let git_dir = temp_dir.join(".git");
    let cache_path = git_toprepo::repo_cache_serde::SerdeTopRepoCache::get_cache_path(&git_dir);

    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cache_path,
        "arbitrary content; we expect an error before reading this file",
    )
    .unwrap();

    // Look for a sane warning message.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&temp_dir)
        .arg("dump")
        .arg("import-cache")
        .assert()
        .code(1)
        .stderr("ERROR: git-config \'toprepo.config\' is missing. Is this an initialized git-toprepo?\n");
}

const EMPTY_CACHE_JSON: &str = r#"{
  "config_checksum": "config-checksum",
  "repos": {},
  "monorepo_commits": [],
  "top_to_mono_commit_map": {},
  "dedup": {
    "commits": {}
  }
}
"#;

#[rstest::fixture]
fn empty_cache_file() -> (MaybePermanentTempDir, PathBuf) {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let cache_path = temp_dir.join("cache-file");
    let cache = git_toprepo::repo::TopRepoCache::default();
    let serde_cache = git_toprepo::repo_cache_serde::SerdeTopRepoCache::pack(
        &cache,
        "config-checksum".to_owned(),
    );
    serde_cache.store(&cache_path).unwrap();
    (temp_dir, cache_path)
}

#[rstest]
fn external_cache_file_path(empty_cache_file: (MaybePermanentTempDir, PathBuf)) {
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .args(["dump", "import-cache"])
        .arg(&empty_cache_file.1)
        .assert()
        .success()
        .stdout(EMPTY_CACHE_JSON)
        .stderr("");
}

#[rstest]
fn external_cache_file_from_stdin(empty_cache_file: (MaybePermanentTempDir, PathBuf)) {
    // Test with stdin as input.
    let cache_bytes = std::fs::read(&empty_cache_file.1).unwrap();
    let mut child = Command::cargo_bin("git-toprepo")
        .unwrap()
        .args(["dump", "import-cache", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&cache_bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.to_str().unwrap(), EMPTY_CACHE_JSON);
    assert!(output.stderr.is_empty());
}

#[test]
fn external_cache_non_existing_file() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .args(["dump", "import-cache"])
        .arg(temp_dir.join("non-existing-file"))
        .assert()
        .failure()
        .stdout("");
}

#[test]
fn wrong_cache_prelude() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let git_dir = monorepo.join(".git");
    let cache_path = git_toprepo::repo_cache_serde::SerdeTopRepoCache::get_cache_path(&git_dir);

    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, "wrong-#cache-format").unwrap();

    // Look for a sane warning message.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("import-cache")
        .assert()
        .success()
        .stderr(predicates::str::is_match(
            "WARN: Discarding toprepo cache .* due to version mismatch, expected \"#cache-format-v2\"\n").unwrap()
        );
}
