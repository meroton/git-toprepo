use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::git_rev_parse;
use itertools::Itertools as _;
use predicates::prelude::*;
use rstest::rstest;
use std::path::Path;

struct RepoWithTwoSubmodules {
    pub toprepo: std::path::PathBuf,
    pub monorepo: std::path::PathBuf,
    pub subx_repo: std::path::PathBuf,

    /// Keep during the lifetime of the struct to let the directory exist.
    #[expect(unused)]
    temp_dir_guard: git_toprepo_testtools::test_util::MaybePermanentTempDir,
}

impl RepoWithTwoSubmodules {
    pub fn new_minimal_with_two_submodules() -> Self {
        let temp_dir_guard = git_toprepo_testtools::test_util::maybe_keep_tempdir(
            gix_testtools::scripted_fixture_writable(
                "../integration/fixtures/make_minimal_with_two_submodules.sh",
            )
            .unwrap(),
        );
        // macOS needs canonicalize() to resolve the /var symlink into /private/var.
        let temp_dir = temp_dir_guard.canonicalize().unwrap();
        // For relative path dirs to be tested, and distinguish between
        // mono/../repox and mono/../top/../repox, use a subdir for mono repo.
        let toprepo = temp_dir.join("top");
        let monorepo = temp_dir.join("outermono/innermono");
        let subx_repo = temp_dir.join("repox");
        std::fs::create_dir(monorepo.parent().unwrap()).unwrap();
        crate::fixtures::toprepo::clone(&toprepo, &monorepo);
        std::fs::create_dir(monorepo.join("subdir_part_of_top")).unwrap();

        git_command_for_testing(&toprepo)
            .args(["checkout", "-b", "foo"])
            .assert()
            .success();
        git_command_for_testing(&toprepo)
            .args(["commit", "--allow-empty", "-m", "Empty test commit in top"])
            .assert()
            .success();
        git_command_for_testing(&subx_repo)
            .args(["checkout", "-b", "subfoo"])
            .assert()
            .success();
        git_command_for_testing(&subx_repo)
            .args(["commit", "--allow-empty", "-m", "Empty test commit in subx"])
            .assert()
            .success();
        // Make sure suby cannot be fetched, as it is not needed.
        let suby_repo = temp_dir.join("repoy");
        assert!(suby_repo.is_dir());
        std::fs::remove_dir_all(&suby_repo).unwrap();

        Self {
            toprepo,
            monorepo,
            subx_repo: temp_dir.join("repox"),
            temp_dir_guard,
        }
    }
}

#[test]
fn print_fetch_duration() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );

    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "INFO: git fetch <top> completed in ",
        ))
        .stderr(predicate::str::is_match("INFO: git fetch .*repox/ completed in ").unwrap())
        .stderr(predicate::str::is_match("INFO: git fetch .*repoy/ completed in ").unwrap());
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("fetch")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "INFO: git fetch <top> completed in ",
        ))
        .stderr(predicate::str::contains("repox").not())
        .stderr(predicate::str::contains("repoy").not());
}

#[test]
fn download_only_for_needed_commits() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );

    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    git_command_for_testing(&toprepo)
        .args([
            "update-index",
            "--cacheinfo",
            "160000,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,subpathx",
        ])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Update submodule subx"])
        .assert()
        .success();
    // Make sure suby cannot be fetched, as it is not needed.
    let suby_repo = temp_dir.join("repoy");
    assert!(suby_repo.is_dir());
    std::fs::remove_dir_all(&suby_repo).unwrap();

    // Success because suby wasn't needed to be fetched.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "WARN: Commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in namex is missing, referenced from top\n"
        ));

    // Check the filter result.
    let first_lstree = git_command_for_testing(&monorepo)
        .args(["ls-tree", "-r", "origin/main"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .to_owned();
    insta::assert_snapshot!(
        first_lstree,
        @r"
    100644 blob 5488142f0fb986fa257ab2704c5e744f04c63ddd	.gitmodules
    100644 blob a947b37238208308b7108a266d9466aa976977fb	.gittoprepo.toml
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	A1-main.txt
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	init.txt
    160000 commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa	subpathx
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	subpathy/y-main-1.txt
    "
    );

    // After updating suby, fetch should fail as the suby remote is missing.
    git_command_for_testing(&toprepo)
        .args([
            "update-index",
            "--cacheinfo",
            "160000,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,subpathy",
        ])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Update submodule suby"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::is_match(
                "ERROR: Fetching namey: git fetch .*/repoy/ failed: exit status: 128",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match("fatal: '.*' does not appear to be a git repository").unwrap(),
        )
        .stderr(predicate::str::contains(
            "fatal: Could not read from remote repository.",
        ));

    // Check the filter result, nothing should not be updated as fetching
    // failed, even if suby was bumped in the fetched toprepo main branch.
    let second_lstree = git_command_for_testing(&monorepo)
        .args(["ls-tree", "-r", "origin/main"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .to_owned();
    insta::assert_snapshot!(
        second_lstree,
        @r"
    100644 blob 5488142f0fb986fa257ab2704c5e744f04c63ddd	.gitmodules
    100644 blob a947b37238208308b7108a266d9466aa976977fb	.gittoprepo.toml
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	A1-main.txt
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	init.txt
    160000 commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa	subpathx
    100644 blob e69de29bb2d1d6434b8b29ae775ad8c2e48c5391	subpathy/y-main-1.txt
    ");

    assert_eq!(
        first_lstree, second_lstree,
        "ls-tree listings should be the same."
    )
}

#[rstest]
#[case::no_remote(None)]
#[case::origin(Some("origin"))]
fn origin_without_refspec_arg(#[case] remote: Option<&str>) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let mut cmd = cargo_bin_git_toprepo_for_testing();
    cmd.current_dir(&repo.monorepo).arg("fetch");
    if let Some(remote) = remote {
        cmd.arg(remote);
    }
    cmd.assert().success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "origin/foo"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
}

#[rstest]
#[case::no_remote(None)]
#[case::origin(Some("origin"))]
fn without_refspec_arg_prunes_refs(#[case] remote: Option<&str>) {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let mut cmd = cargo_bin_git_toprepo_for_testing();
    cmd.current_dir(&repo.monorepo).arg("fetch");
    if let Some(remote) = remote {
        cmd.arg(remote);
    }
    cmd.assert().success();
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo"));

    // Delete and prune foo.
    git_command_for_testing(&repo.toprepo)
        .args(["checkout", "--detach"])
        .assert()
        .success();
    git_command_for_testing(&repo.toprepo)
        .args(["update-ref", "-d", "refs/heads/foo"])
        .assert()
        .success();
    let mut cmd = cargo_bin_git_toprepo_for_testing();
    cmd.current_dir(&repo.monorepo).arg("fetch");
    if let Some(remote) = remote {
        cmd.arg(remote);
    }
    cmd.assert().success();
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo").not());
}

/// It is not possible to fetch a refspec without a remote.
#[test]
fn refspec_arg_without_remote_fails() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch", "refs/heads/foo"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "ERROR: Failed to fetch: The git-remote \"refs/heads/foo\" was not found among \"origin\".\n\
            When no refspecs are provided, a name among `git remote -v` must be specified.\n",
        ));
}

#[test]
fn info_fetch_head() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch", "origin", "refs/heads/foo"])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Check that no extra temporary refs are available.
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/namespaces/namex/refs/heads/main\n",
                    ".* refs/namespaces/namey/refs/heads/main\n",
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

/// This regression test ensures that fetching twice does not remove the refs.
#[test]
fn two_times_should_keep_refs() {
    let expected_show_ref_output = predicate::str::is_match(
        [
            ".* refs/namespaces/namex/refs/heads/main\n",
            ".* refs/namespaces/namey/refs/heads/main\n",
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
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch"])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(expected_show_ref_output.clone());

    // Update main branch in the top repo.
    git_command_for_testing(&repo.toprepo)
        .args(["checkout", "main"])
        .assert()
        .success();
    git_command_for_testing(&repo.toprepo)
        .args(["commit", "--allow-empty", "-m", "Commit A main branch"])
        .assert()
        .success();

    // Fetch again, should not remove refs/remotes/origin/main.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch"])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(expected_show_ref_output.clone());

    // Update main branch in the top repo.
    git_command_for_testing(&repo.toprepo)
        .args(["commit", "--allow-empty", "-m", "Commit B main branch"])
        .assert()
        .success();

    // Fetch again, but with a refspec.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch", "origin", "refs/heads/main"])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(expected_show_ref_output);
}

#[test]
fn top_with_refspec_arg_success() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch", "origin", "refs/heads/foo:refs/heads/bar"])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Check that no extra temporary refs are available.
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/namespaces/namex/refs/heads/main\n",
                    ".* refs/namespaces/namey/refs/heads/main\n",
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

#[test]
fn with_refspec_arg_absolute_submod_remote_path_from_root() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    submod_with_refspec_arg_success(&repo, &repo.monorepo, &repo.subx_repo);
}

#[test]
fn with_refspec_arg_absolute_submod_remote_path_from_subdir() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    submod_with_refspec_arg_success(&repo, &repo.monorepo.join("other"), &repo.subx_repo);
}

#[test]
fn with_refspec_arg_relative_submod_remote_path_from_root() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    submod_with_refspec_arg_success(&repo, &repo.monorepo, Path::new("../../repox"));
}

#[test]
fn with_refspec_arg_relative_submod_remote_path_from_subdir() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    submod_with_refspec_arg_success(
        &repo,
        &repo.monorepo.join("other"),
        Path::new("../../../repox"),
    );
    // Check that outermono/innermono/other/../repox fails, even if ../repox is
    // part of the .gittoprepo.toml config, because it is not a local path that
    // can be fetched from.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(repo.monorepo.join("other"))
        .args(["fetch", "../repox", "refs/heads/subfoo:refs/heads/bar"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "^ERROR: No configured submodule URL matches \".*/outermono/innermono/repox\"\\n$",
            )
            .unwrap(),
        );
}

fn submod_with_refspec_arg_success(repo: &RepoWithTwoSubmodules, cwd: &Path, remote: &Path) {
    std::fs::create_dir_all(cwd).unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(cwd)
        .args([
            "fetch",
            remote.to_str().unwrap(),
            "refs/heads/subfoo:refs/heads/bar",
        ])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in subx\n");
    // Check that no extra temporary refs are available.
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/namespaces/namex/refs/heads/main\n",
                    ".* refs/namespaces/namey/refs/heads/main\n",
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

/// Verify the behaviour for fetching a submod commit that was merged to the
/// submodule before the submodule was added to the toprepo. This means that the
/// fetched commit has no corresponding monocommit.
#[test]
fn submod_commit_merged_before_monorepo() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("repo");
    let monorepo = temp_dir.join("mono");
    let commit_merged_before_monorepo = git_rev_parse(&subrepo, "HEAD~10");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "fetch",
            "../repo",
            &commit_merged_before_monorepo,
        ])
        .assert()
        .code(1)
        .stderr(predicates::str::contains(format!(
            "\nERROR: Commit {commit_merged_before_monorepo} cannot be expanded onto any monocommit in the history of HEAD. \
            Running \'git toprepo recombine\' may help unless the commit is older than the submodule in the super repository. \
            You may still be able to \'git cherry-pick -Xsubtree=subpath {commit_merged_before_monorepo}\'.\n"
        )));
    // Because expansion failed, refs/namespaces/name/refs/fetch-heads/0 will be available.
    git_command_for_testing(&monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    &format!(
                        "{commit_merged_before_monorepo} refs/namespaces/name/refs/fetch-heads/0\n"
                    ),
                    ".* refs/namespaces/name/refs/heads/main\n",
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

#[test]
fn force_with_refspec_arg_not_implemented_yet() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args([
            "fetch",
            "origin",
            "refs/heads/foo:refs/heads/bar",
            "refs/heads/foo",
        ])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Empty test commit in top\n");
    // Amend so that force is needed.
    git_command_for_testing(&repo.toprepo)
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
    // TODO: 2025-09-22 Not implemented yet.
    // cargo_bin_git_toprepo_for_testing()
    //     .current_dir(&repo.monorepo)
    //     .args(["fetch", "origin", "refs/heads/foo:refs/heads/bar"])
    //     .assert()
    //     .failure();
    // cargo_bin_git_toprepo_for_testing()
    //     .current_dir(&repo.monorepo)
    //     .args(["fetch", "origin", "refs/heads/foo"])
    //     .assert()
    //     .failure();
    // git-fetch with force should succeed.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args([
            "fetch",
            "origin",
            "+refs/heads/foo:refs/heads/bar",
            "+refs/heads/foo",
        ])
        .assert()
        .success();
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "refs/heads/bar", "--"])
        .assert()
        .success()
        .stdout("Updated test commit\n");
    git_command_for_testing(&repo.monorepo)
        .args(["show", "--format=%s", "--quiet", "FETCH_HEAD", "--"])
        .assert()
        .success()
        .stdout("Updated test commit\n");
    // Check that no extra temporary refs are available.
    git_command_for_testing(&repo.monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                [
                    ".* refs/heads/bar\n",
                    ".* refs/namespaces/namex/refs/heads/main\n",
                    ".* refs/namespaces/namey/refs/heads/main\n",
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

/// Test `git fetch` does not time out while printing progress messages.
fn no_timeout_with_progress_checker(cmd: assert_cmd::assert::Assert) {
    cmd.success()
        .stderr(predicate::str::contains("WARN: Fetching namex:").not());
}

/// Test `git fetch` does not time out while printing progress messages.
fn idle_progress_with_successful_retry_checker(cmd: assert_cmd::assert::Assert) {
    cmd.success().stderr(
        predicate::str::is_match(
            "WARN: Fetching namex: git fetch .* timed out, was silent 1s, retrying",
        )
        .unwrap(),
    );
}

/// Test `git-toprepo fetch` fails if there are too many timeouts.
fn too_many_timeouts_checker(cmd: assert_cmd::assert::Assert) {
    cmd.code(1)
        .stderr(
            predicate::str::is_match(
                "\
WARN: Fetching namex: git fetch .* timed out, was silent 1s, retrying
WARN: Fetching namex: git fetch .* timed out, was silent 1s, retrying
",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "ERROR: Fetching namex: git fetch .* exceeded timeout retry limit",
            )
            .unwrap(),
        )
        // No INFO message about successful fetch.
        .stderr(
            predicate::str::is_match("INFO: git fetch .*repox")
                .unwrap()
                .not(),
        );
}

#[rstest]
#[case::does_not_timeout_with_progress("[3]", no_timeout_with_progress_checker)]
#[case::idle_progress("[1, 3]", idle_progress_with_successful_retry_checker)]
#[case::exceeds_retries("[1, 1]", too_many_timeouts_checker)]
fn timeout(
    #[case] idle_timeouts: &str,
    #[case] command_checker: impl Fn(assert_cmd::assert::Assert),
) {
    use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;

    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--replace-all",
            "toprepo.config",
            "must:local:.gittoprepo.toml",
        ])
        .assert()
        .success();
    let toprepo_config_path = repo.monorepo.join(".gittoprepo.toml");
    let old_config_content = std::fs::read_to_string(&toprepo_config_path).unwrap();
    let new_config_content =
        format!("fetch.idle_timeouts_secs = {idle_timeouts}\n{old_config_content}");
    std::fs::write(&toprepo_config_path, &new_config_content).unwrap();
    let slow_upload_pack_dir =
        std::path::absolute("tests/integration/fixtures/git-upload-pack-slow").unwrap();

    // Force a submodule fetch.
    git_command_for_testing(&repo.toprepo)
        .args([
            "update-index",
            "--cacheinfo",
            "160000,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,subpathx",
        ])
        .assert()
        .success();
    git_command_for_testing(&repo.toprepo)
        .args(["commit", "-m", "Update submodule subx"])
        .assert()
        .success();
    git_command_for_testing(&repo.subx_repo)
        .args(["commit", "--allow-empty", "-m", "Something to fetch"])
        .assert()
        .success();
    let old_path_env = std::env::var_os("PATH").unwrap_or_default();
    let new_paths = [slow_upload_pack_dir]
        .into_iter()
        .chain(std::env::split_paths(&old_path_env))
        .collect_vec();
    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["fetch"])
        .env("OLD_PATH", &old_path_env)
        .env("PATH", std::env::join_paths(new_paths).unwrap())
        .assert();
    command_checker(cmd);
}

#[test]
fn unaffected_by_dot_gitmodules_recurse_true() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");

    std::fs::write(
        toprepo.join(".gitmodules"),
        &r#"
[submodule "subx"]
	path = subx
	url = ../repox/
    recurse = true
[submodule "suby"]
	path = suby
	url = ../repoy/
"#[1..],
    )
    .unwrap();
    git_command_for_testing(&toprepo)
        .args(["add", ".gitmodules"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Set recurse=true in .gitmodules"])
        .assert()
        .success();

    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success();

    assert!(std::fs::exists(toprepo.join(".git/modules")).unwrap());
    assert!(!std::fs::exists(monorepo.join(".git/modules")).unwrap());

    git_command_for_testing(&monorepo)
        .args(["show-ref"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(
                "^\
[0-9a-f]+ refs/namespaces/namex/refs/heads/main
[0-9a-f]+ refs/namespaces/namey/refs/heads/main
[0-9a-f]+ refs/namespaces/top/refs/remotes/origin/HEAD
[0-9a-f]+ refs/namespaces/top/refs/remotes/origin/main
[0-9a-f]+ refs/remotes/origin/HEAD
[0-9a-f]+ refs/remotes/origin/main
",
            )
            .unwrap(),
        );
}
