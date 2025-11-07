use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use predicates::prelude::*;

#[test]
fn local_config_resolution() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_worktree.sh",
        )
        .unwrap(),
    );
    let worktree = temp_dir.join("worktree");
    git_command_for_testing(temp_dir.join("mono"))
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(worktree)
        .arg("fetch")
        .assert()
        .success();
}

#[test]
fn fetch() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let worktree = temp_dir.join("worktree");
    git_command_for_testing(temp_dir.join("mono"))
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&worktree)
        .args(["fetch", "origin", "HEAD"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(r"^ \* \[new\] [0-9a-f]+\s+-> refs/fetch-heads/0\n$").unwrap(),
        )
        .stderr(predicate::str::contains(
            "INFO: Updated ../mono/.git/worktrees/worktree/FETCH_HEAD\n",
        ));
    // The main worktree should be missing FETCH_HEAD.
    assert!(!std::fs::exists(monorepo.join(".git/FETCH_HEAD")).unwrap());
    assert!(std::fs::exists(monorepo.join(".git/worktrees/worktree/FETCH_HEAD")).unwrap());
    std::fs::remove_file(monorepo.join(".git/worktrees/worktree/FETCH_HEAD")).unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch", "origin", "HEAD"])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(r"^ \* \[new\] [0-9a-f]+\s+-> refs/fetch-heads/0\n$").unwrap(),
        )
        .stderr(predicate::str::contains("INFO: Updated .git/FETCH_HEAD\n"));
    // Only the main worktree should have FETCH_HEAD.
    assert!(std::fs::exists(monorepo.join(".git/FETCH_HEAD")).unwrap());
    assert!(!std::fs::exists(monorepo.join(".git/worktrees/worktree/FETCH_HEAD")).unwrap());
}

#[test]
fn recombine() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let worktree = temp_dir.join("worktree");
    git_command_for_testing(temp_dir.join("mono"))
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&worktree)
        .args(["recombine"])
        .assert()
        .success()
        .stdout("");
    assert!(std::fs::exists(monorepo.join(".git/toprepo/mono-refs-ok-to-remove")).unwrap());
    assert!(!std::fs::exists(monorepo.join(".git/worktrees/worktree/toprepo")).unwrap());
}

#[test]
fn push() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let worktree = temp_dir.join("worktree");
    git_command_for_testing(temp_dir.join("mono"))
        .args(["worktree", "add", "../worktree"])
        .assert()
        .success();

    git_command_for_testing(&worktree)
        .args([
            "commit",
            "--amend",
            "-m",
            "Message in worktree\n\nTopic: work",
        ])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&worktree)
        .args(["push", "--dry-run", "origin", "HEAD:refs/dry/run"])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repox/ -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repoy/ -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*top -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(predicate::function(|s: &str| {
            s.matches("INFO: Would run git push").count() == 3
        }));
}
