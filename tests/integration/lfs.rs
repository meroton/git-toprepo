use super::fetch::RepoWithTwoSubmodules;
use bstr::ByteSlice as _;
use git_toprepo::gitmodules::SubmoduleUrlExt as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::prepend_path_env;
use itertools::Itertools;
use predicates::prelude::*;
use std::path::Path;
use std::path::PathBuf;

struct RepoWithInnerSubmodule {
    monorepo: PathBuf,
    #[expect(unused)]
    temp_dir_guard: git_toprepo_testtools::test_util::MaybePermanentTempDir,
}

impl RepoWithInnerSubmodule {
    fn new() -> Self {
        let temp_dir_guard = git_toprepo_testtools::test_util::maybe_keep_tempdir(
            gix_testtools::scripted_fixture_writable(
                "../integration/fixtures/make_minimal_with_inner_submodule.sh",
            )
            .unwrap(),
        );
        let temp_dir = temp_dir_guard.canonicalize().unwrap();
        let monorepo = temp_dir.join("mono");
        crate::fixtures::toprepo::clone(&temp_dir.join("top"), &monorepo);
        Self {
            monorepo,
            temp_dir_guard,
        }
    }
}

fn make_fake_git_lfs(dir: &Path, version_exit: i32) -> std::path::PathBuf {
    let path = dir.join("git-lfs");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "version" ]; then
  echo "git-lfs/FAKE"
  exit {version_exit}
fi
if [ "$1" = "fetch" ]; then
  remote=$2
  shift 2
  printf 'cwd=%s\n' "$(pwd)" >> "$GIT_TOPREPO_TEST_LFS_LOG"
  printf 'remote=%s\n' "$remote" >> "$GIT_TOPREPO_TEST_LFS_LOG"
  for arg in "$@"; do
    printf 'arg=%s\n' "$arg" >> "$GIT_TOPREPO_TEST_LFS_LOG"
  done
  exit "${{GIT_TOPREPO_TEST_LFS_EXIT:-0}}"
fi
echo "unexpected git-lfs command: $*" >&2
exit 64
"#
    );
    git_toprepo::util::create_executable(&path, script).unwrap();
    path
}

#[test]
fn install() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "install"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    crate::hooks::assert_hooks_with_git_lfs(&repo.monorepo);
}

#[test]
fn missing_git_lfs_fails_clearly() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    make_fake_git_lfs(bin_dir, 1);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/assets/model.bin"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Git LFS is required for 'git toprepo lfs fetch'",
        ))
        .stderr(predicate::str::contains("git lfs version"))
        .stderr(predicate::str::contains("Install Git LFS"));
}

#[test]
fn fetches_top_level_path_from_top_repo_url() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "video.mov"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&format!("cwd={}\n", repo.monorepo.display())),
        "{log}"
    );
    assert!(log.contains(&format!("remote={top_url}")), "{log}");
    assert!(log.contains("arg=--include=video.mov"), "{log}");
}

#[test]
fn fetches_subrepo_path_from_subrepo_url() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let submodule_url = git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get",
            "submodule.subpathx.url",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let expected_remote = gix::Url::from_bytes(top_url.as_bytes().as_bstr())
        .unwrap()
        .join(&gix::Url::from_bytes(submodule_url.as_bytes().as_bstr()).unwrap())
        .to_string();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/assets/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains(&format!("remote={expected_remote}")), "{log}");
    assert!(log.contains("arg=--include"), "{log}");
    assert!(
        log.contains("arg=--include=subpathx/assets/model.bin"),
        "{log}"
    );
}

#[test]
fn fetches_relative_path_from_subdirectory() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);
    std::fs::create_dir_all(repo.monorepo.join("subpathx")).unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(repo.monorepo.join("subpathx"))
        .args(["lfs", "fetch", "assets/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains("cwd="), "{log}");
    assert!(
        log.contains("arg=--include=subpathx/assets/model.bin"),
        "{log}"
    );
}

#[test]
fn fetches_multiple_paths_one_by_one() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/a.bin", "subpathy/b.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("cwd=").count(), 2, "{log}");
    assert!(log.contains("arg=--include=subpathx/a.bin"), "{log}");
    assert!(log.contains("arg=--include=subpathy/b.bin"), "{log}");
}

#[test]
fn rejects_unsupported_options() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--all", "subpathx/file.bin"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--all': unsupported for 'git toprepo lfs fetch'",
        ));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--stdin", "subpathx/file.bin"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--stdin': unsupported for 'git toprepo lfs fetch'",
        ));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "ipath", "subpathx/file.bin"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "error: invalid value 'ipath' for '--include <PATHS>': unsupported, 'git toprepo lfs fetch' supplies '--include' internally",
        ));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--json", "subpathx/file.bin"])
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--json': unsupported for 'git toprepo lfs fetch'",
        ));
}

#[test]
fn errors_on_unconfigured_subrepo() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--replace-all",
            "toprepo.config",
            "must:local:.gittoprepo.toml",
        ])
        .assert()
        .success();
    std::fs::write(
        repo.monorepo.join(".gittoprepo.toml"),
        "[repo.namey]\nurl = \"../repoy/\"\n",
    )
    .unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/file.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Cannot resolve LFS remote for 'subpathx/file.bin'",
        ))
        .stderr(predicate::str::contains("belongs to submodule 'subpathx'"))
        .stderr(predicate::str::contains(
            "not configured in .gittoprepo.toml",
        ));

    assert!(!std::fs::exists(&log_path).unwrap());
}

#[test]
fn uses_deepest_matching_submodule_path() {
    let repo = RepoWithInnerSubmodule::new();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let inner_url = git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get",
            "submodule.subpathx.url",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let expected_remote = gix::Url::from_bytes(top_url.as_bytes().as_bstr())
        .unwrap()
        .join(&gix::Url::from_bytes(inner_url.as_bytes().as_bstr()).unwrap())
        .to_string();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/subpathy/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains(&format!("remote={expected_remote}")), "{log}");
    assert!(
        log.contains("arg=--include=subpathx/subpathy/model.bin"),
        "{log}"
    );
}

#[test]
fn preserves_spaces_in_include_path() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/assets/big file.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .env("PATH", prepend_path_env(bin_dir))
        .assert()
        .success();

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.lines()
            .contains("arg=--include=subpathx/assets/big file.bin"),
        "{log}"
    );
}
