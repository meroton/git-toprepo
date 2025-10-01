use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::MaybePermanentTempDir;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use predicate::str::contains;
use predicates::prelude::*;
use rstest::rstest;
use std::path::PathBuf;

const EXPECTED_IMPORT_CACHE_VERSION: &str = "#cache-format-v3";

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
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout("main/project.git .\nmain/subx subx\n");
    // Test a subdirectory which is not a submodule.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(monorepo.join("subdir"))
        .arg("dump")
        .arg("git-modules")
        .assert()
        .success()
        .stdout("main/project.git .\nmain/subx subx\n");
    // Test a subdirectory which is not an integrated submodule.
    cargo_bin_git_toprepo_for_testing()
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
    cargo_bin_git_toprepo_for_testing()
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

    let gix_repo = gix::open(&temp_dir).unwrap();
    let cache_path = git_toprepo::import_cache_serde::SerdeImportCache::get_cache_path(&gix_repo);

    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cache_path,
        "arbitrary content; we expect an error before reading this file",
    )
    .unwrap();

    // Look for a sane warning message.
    cargo_bin_git_toprepo_for_testing()
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
    let cache = git_toprepo::repo::ImportCache::default();
    let serde_cache = git_toprepo::import_cache_serde::SerdeImportCache::pack(
        &cache,
        "config-checksum".to_owned(),
    );
    serde_cache.store(&cache_path).unwrap();
    (temp_dir, cache_path)
}

#[rstest]
fn external_cache_file_path(empty_cache_file: (MaybePermanentTempDir, PathBuf)) {
    cargo_bin_git_toprepo_for_testing()
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
    cargo_bin_git_toprepo_for_testing()
        .args(["dump", "import-cache", "-"])
        .pipe_stdin(empty_cache_file.1)
        .unwrap()
        .assert()
        .success()
        .stdout(EMPTY_CACHE_JSON)
        .stderr("");
}

#[test]
fn external_cache_non_existing_file() {
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    cargo_bin_git_toprepo_for_testing()
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

    let gix_repo = gix::open(&monorepo).unwrap();
    let cache_path = git_toprepo::import_cache_serde::SerdeImportCache::get_cache_path(&gix_repo);

    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, "wrong-#cache-format").unwrap();

    // Look for a sane warning message.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("import-cache")
        .assert()
        .success()
        .stderr(predicate::str::is_match(
            format!("WARN: Discarding import cache .* due to version mismatch, expected \"{EXPECTED_IMPORT_CACHE_VERSION}\"\n")).unwrap()
        );
}

/// Check if the cache version need to be updated.
///
/// NOTE: If the fixture needs updating, update the cache format number as well!
#[test]
fn cache_version_change_detection() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let gix_repo = gix::open(&monorepo).unwrap();
    let cache_path = git_toprepo::import_cache_serde::SerdeImportCache::get_cache_path(&gix_repo);
    let cache_bytes = std::fs::read(&cache_path).unwrap();
    assert_eq!(
        std::str::from_utf8(cache_bytes.get(0..16).unwrap()).unwrap(),
        EXPECTED_IMPORT_CACHE_VERSION
    );

    // Check that unpacking works.
    git_toprepo::import_cache_serde::SerdeImportCache::load_from_git_dir(
        &gix_repo,
        Some("6c10545879319d948b2bcb241d61c0c31bd86a485b423d1a2cb40eb56ffe3a56"),
    )
    .unwrap()
    .unpack()
    .unwrap();

    // Check the JSON dump for changes. Using the JSON dump because it is
    // stable, the bincode variant depends on HashMap ordering.
    let cmd_assert = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("dump")
        .arg("import-cache")
        .assert()
        .success()
        .stderr("");
    let example_import_cache_json = cmd_assert.get_output().stdout.to_str().unwrap();
    // Update this source file to not forget updating the expected version
    // string.
    let snapshot_path = std::path::Path::new(
        "tests/integration/snapshots/integration__dump__readme-example-import-cache-json.snap",
    );
    let snapshot_content =
        std::fs::read_to_string(snapshot_path).unwrap_or_else(|_err| "---\n---\n".to_owned());
    let snapshot_content = snapshot_content[4..].split_once("---\n").unwrap().1;
    if example_import_cache_json != snapshot_content {
        let old_code = std::fs::read_to_string(file!()).unwrap();
        let new_code = old_code.replace(
            EXPECTED_IMPORT_CACHE_VERSION,
            "#cache-format-vNNN - PLEASE UPDATE THIS NUMBER",
        );
        assert_ne!(
            old_code,
            new_code,
            "Expected {} to contain the version string",
            file!()
        );
        std::fs::write(file!(), new_code).unwrap();
    }
    // Check the output content in the end.
    insta::assert_snapshot!(
        "readme-example-import-cache-json",
        example_import_cache_json
    );
}
