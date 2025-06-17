use assert_cmd::prelude::*;
use git_toprepo::git::commit_env_for_testing;
use predicates::prelude::*;
use rstest::rstest;
use std::process::Command;

#[test]
fn test_fetch_only_needed_commits() {
    let temp_dir = gix_testtools::scripted_fixture_writable(
        "../integration/fixtures/make_minimal_with_two_submodules.sh",
    )
    .unwrap();
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    const RANDOM_SHA1: &str = "0123456789abcdef0123456789abcdef01234567";
    Command::new("git")
        .current_dir(&toprepo)
        .args([
            "update-index",
            "--cacheinfo",
            &format!("160000,{RANDOM_SHA1},subx"),
        ])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&toprepo)
        .args(["commit", "-m", "Update submodule subx"])
        .envs(commit_env_for_testing())
        .assert()
        .success();
    // Make sure suby cannot be fetched, as it is not needed.
    let suby_repo = temp_dir.join("suby");
    assert!(suby_repo.is_dir());
    std::fs::remove_dir_all(&suby_repo).unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARNING: Missing commit in subx: {RANDOM_SHA1}\n"
        )));

    // Check the filter result.
    Command::new("git")
        .current_dir(&monorepo)
        .args(["ls-tree", "-r", "origin/main"])
        .assert()
        .success()
        .stdout(
            "\
100644 blob 73bf371d38ac93f7592bdee317c8ea53fead1c8c\t.gitmodules
100644 blob ed6ed9e7ce37c1f13f718aeaf54c522610a994c2\t.gittoprepo.toml
100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\tA1-main.txt
160000 commit 0123456789abcdef0123456789abcdef01234567\tsubx
100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\tsuby/y-main-1.txt
",
        );

    // After updating suby, fetch should fail as the suby remote is missing.
    Command::new("git")
        .current_dir(&toprepo)
        .args([
            "update-index",
            "--cacheinfo",
            &format!("160000,{RANDOM_SHA1},suby"),
        ])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&toprepo)
        .args(["commit", "-m", "Update submodule suby"])
        .envs(commit_env_for_testing())
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::is_match(
                "ERROR: Fetching suby: git fetch .*/suby/ failed: exit status: 128",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match("fatal: '.*' does not appear to be a git repository").unwrap(),
        )
        .stderr(predicate::str::contains(
            "fatal: Could not read from remote repository.",
        ));

    // Check the filter result, suby should not be updated as fetching failed.
    Command::new("git")
        .current_dir(&monorepo)
        .args(["ls-tree", "-r", "origin/main"])
        .assert()
        .success()
        .stdout(
            "\
100644 blob 73bf371d38ac93f7592bdee317c8ea53fead1c8c\t.gitmodules
100644 blob ed6ed9e7ce37c1f13f718aeaf54c522610a994c2\t.gittoprepo.toml
100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\tA1-main.txt
160000 commit 0123456789abcdef0123456789abcdef01234567\tsubx
100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\tsuby/y-main-1.txt
",
        );
}

// TODO: Using #[allow(unused)] because the members will probably be used in the
// near future.
struct RepoWithTwoSubmodules {
    pub toprepo: std::path::PathBuf,
    pub monorepo: std::path::PathBuf,
    #[allow(unused)]
    pub subx_repo: std::path::PathBuf,
    #[allow(unused)]
    pub suby_repo: std::path::PathBuf,

    #[allow(unused)]
    temp_dir: git_toprepo_testtools::test_util::MaybePermanentTempDir,
}

impl RepoWithTwoSubmodules {
    pub fn new_minimal_with_two_submodules() -> Self {
        let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
            gix_testtools::scripted_fixture_writable(
                "../integration/fixtures/make_minimal_with_two_submodules.sh",
            )
            .unwrap(),
        );
        let toprepo = temp_dir.join("top");
        let monorepo = temp_dir.join("mono");
        crate::fixtures::toprepo::clone(&toprepo, &monorepo);
        std::fs::create_dir(monorepo.join("subdir_part_of_top")).unwrap();

        Command::new("git")
            .current_dir(&toprepo)
            .args(["checkout", "-b", "foo"])
            .assert()
            .success();
        Command::new("git")
            .current_dir(&toprepo)
            .args(["commit", "--allow-empty", "-m", "Empty test commit in top"])
            .envs(commit_env_for_testing())
            .assert()
            .success();
        // Make sure suby cannot be fetched, as it is not needed.
        let suby_repo = temp_dir.join("suby");
        assert!(suby_repo.is_dir());
        std::fs::remove_dir_all(&suby_repo).unwrap();

        Self {
            toprepo,
            monorepo,
            subx_repo: temp_dir.join("subx"),
            suby_repo: temp_dir.join("suby"),
            temp_dir,
        }
    }
}

#[rstest]
#[case::no_remote(None)]
#[case::origin(Some("origin"))]
fn test_fetch_no_refspec_success(#[case] remote: Option<&str>) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let mut cmd = Command::cargo_bin("git-toprepo").unwrap();
    cmd.current_dir(&repo.monorepo).arg("fetch");
    if let Some(remote) = remote {
        cmd.arg(remote);
    }
    cmd.assert().success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "origin/foo"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
}

#[rstest]
#[case::local_root_dir(".")]
#[case::local_subdir("subdir_part_of_top")]
fn test_fetch_no_refspec_fail(#[case] remote: &str) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch", remote])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(format!(
            "ERROR: Failed to fetch: The git-remote {remote:?} was not found among \"origin\".\n\
                When no refspecs are provided, a name among `git remote -v` must be specified.\n",
        )));
}

/// It is not possible to fetch a refspec without a remote.
#[test]
fn test_fetch_refspec_no_remote() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch", "refs/heads/foo"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "ERROR: Failed to fetch: The git-remote \"refs/heads/foo\" was not found among \"origin\".\n\
            When no refspecs are provided, a name among `git remote -v` must be specified.\n",
        ));
}

#[rstest]
#[case::origin("origin")]
#[case::local_root_dir(".")]
fn test_fetch_to_fetch_head_success(#[case] remote: &str) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch", remote, "refs/heads/foo"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Check that no extra temporary refs are available.
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/namespaces/subx/refs/heads/main\n",
                    ".* refs/namespaces/suby/refs/heads/main\n",
                    ".* refs/namespaces/top/refs/remotes/origin/HEAD\n",
                    ".* refs/namespaces/top/refs/remotes/origin/main\n",
                    ".* refs/remotes/origin/HEAD\n",
                    ".* refs/remotes/origin/main\n",
                ]
                .join(""),
            )
            .unwrap(),
        );
}

#[rstest]
#[case::local_subdir("subdir_part_of_top")]
fn test_fetch_to_fetch_head_fail(#[case] remote: &str) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo").unwrap()
    .current_dir(&repo.monorepo).args(["fetch", remote, "refs/heads/foo"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            format!(
                "ERROR: Submodule {remote} not found in config: subdir_part_of_top is not a submodule\n",
            ),
        ));
}

/// This regression test ensures that fetching twice does not remove the refs.
#[test]
fn test_fetch_twice_should_keep_refs() {
    let expected_show_ref_output = predicate::str::is_match(
        [
            ".* refs/namespaces/subx/refs/heads/main\n",
            ".* refs/namespaces/suby/refs/heads/main\n",
            ".* refs/namespaces/top/refs/remotes/origin/HEAD\n",
            ".* refs/namespaces/top/refs/remotes/origin/foo\n",
            ".* refs/namespaces/top/refs/remotes/origin/main\n",
            ".* refs/remotes/origin/HEAD\n",
            ".* refs/remotes/origin/foo\n",
            ".* refs/remotes/origin/main\n",
        ]
        .join(""),
    )
    .unwrap();

    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(expected_show_ref_output.clone());

    // Update main branch in the top repo.
    Command::new("git")
        .current_dir(&repo.toprepo)
        .args(["checkout", "main"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.toprepo)
        .envs(commit_env_for_testing())
        .args([
            "commit",
            "--allow-empty",
            "-m",
            "Emptry commit on main branch",
        ])
        .assert()
        .success();

    // Fetch again, should not remove refs/remotes/origin/main.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(expected_show_ref_output);
}

#[rstest]
#[case::origin("origin")]
#[case::local_root_dir(".")]
fn test_fetch_refspec_success(#[case] remote: &str) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args(["fetch", remote, "refs/heads/foo:refs/heads/bar"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Check that no extra temporary refs are available.
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/namespaces/subx/refs/heads/main\n",
                    ".* refs/namespaces/suby/refs/heads/main\n",
                    ".* refs/namespaces/top/refs/remotes/origin/HEAD\n",
                    ".* refs/namespaces/top/refs/remotes/origin/main\n",
                    ".* refs/remotes/origin/HEAD\n",
                    ".* refs/remotes/origin/main\n",
                ]
                .join(""),
            )
            .unwrap(),
        );
}

#[rstest]
#[case::local_subdir("subdir_part_of_top")]
fn test_fetch_refspec_fail(#[case] remote: &str) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo").unwrap()
    .current_dir(&repo.monorepo).args(["fetch", remote, "refs/heads/foo:refs/heads/bar"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            format!(
                "ERROR: Submodule {remote} not found in config: subdir_part_of_top is not a submodule\n",
            ),
        ));
}

#[test]
fn test_fetch_force_refspec_not_implemented_yet() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args([
            "fetch",
            "origin",
            "refs/heads/foo:refs/heads/bar",
            "refs/heads/foo",
        ])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Amend so that force is needed.
    Command::new("git")
        .current_dir(&repo.toprepo)
        .envs(commit_env_for_testing())
        .args([
            "commit",
            "--amend",
            "--allow-empty",
            "-m",
            "Updated test commit",
        ])
        .assert()
        .success();
    // git-fetch without force should fail.
    // TODO: Not implemented yet.
    // Command::cargo_bin("git-toprepo")
    //     .unwrap()
    //     .current_dir(&repo.monorepo)
    //     .args(["fetch", "origin", "refs/heads/foo:refs/heads/bar"])
    //     .assert()
    //     .failure();
    // Command::cargo_bin("git-toprepo")
    //     .unwrap()
    //     .current_dir(&repo.monorepo)
    //     .args(["fetch", "origin", "refs/heads/foo"])
    //     .assert()
    //     .failure();
    // git-fetch with force should succeed.
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&repo.monorepo)
        .args([
            "fetch",
            "origin",
            "+refs/heads/foo:refs/heads/bar",
            "+refs/heads/foo",
        ])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Updated test commit\n");
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Updated test commit\n");
    // Check that no extra temporary refs are available.
    Command::new("git")
        .current_dir(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/heads/bar\n",
                    ".* refs/namespaces/subx/refs/heads/main\n",
                    ".* refs/namespaces/suby/refs/heads/main\n",
                    ".* refs/namespaces/top/refs/remotes/origin/HEAD\n",
                    ".* refs/namespaces/top/refs/remotes/origin/main\n",
                    ".* refs/remotes/origin/HEAD\n",
                    ".* refs/remotes/origin/main\n",
                ]
                .join(""),
            )
            .unwrap(),
        );
}
