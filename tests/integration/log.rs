use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::git_rev_parse;
use predicates::prelude::*;

#[test]
fn only_fixable_missing_gitmodules_warnings() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    // Remove .gitmodules.
    git_command_for_testing(&toprepo)
        .args(["rm", ".gitmodules"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "No .gitmodules"])
        .assert()
        .success();
    let missing_gitmodules_rev = git_rev_parse(&toprepo, "HEAD");
    // With another commit, the commit above is no longer fixable.
    git_command_for_testing(&toprepo)
        .args(["commit", "--allow-empty", "-m", "Still no .gitmodules"])
        .assert()
        .success();
    let still_missing_gitmodules_rev = git_rev_parse(&toprepo, "HEAD");

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!("WARN: Commit {still_missing_gitmodules_rev} in top (refs/remotes/origin/HEAD, refs/remotes/origin/main): Cannot resolve submodule sub, .gitmodules is missing")))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning about missing .gitmodules, the tip of the branch that is fixable.
            stderr.matches(".gitmodules is missing").count() == 1
        }));

    // Tag the problematic revision. Tags should never be updated, so no point to give a warning.
    git_command_for_testing(&toprepo)
        .args(["tag", "bad"])
        .assert()
        .success();

    // Fix the problem, so no warning from the main branch either.
    git_command_for_testing(&toprepo)
        .args(["checkout", "HEAD~2", ".gitmodules"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Restore .gitmodules"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN:").not());
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["recombine"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN:").not());

    // Adding a branch to missing_gitmodules_rev makes it fixable again.
    git_command_for_testing(&toprepo)
        .args([
            "branch",
            "first-missing-gitmodules",
            &missing_gitmodules_rev,
        ])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!("WARN: Commit {missing_gitmodules_rev} in top (refs/remotes/origin/first-missing-gitmodules): Cannot resolve submodule sub, .gitmodules is missing")))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning about missing .gitmodules, the tip of the branch that is fixable.
            stderr.matches(".gitmodules is missing").count() == 1
        }));
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["recombine"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!("WARN: Commit {missing_gitmodules_rev} in top (refs/remotes/origin/first-missing-gitmodules): Cannot resolve submodule sub, .gitmodules is missing")))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning about missing .gitmodules, the tip of the branch that is fixable.
            stderr.matches(".gitmodules is missing").count() == 1
        }));
}

#[test]
fn always_show_missing_submod_commit_warnings() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");

    // Make top.git/sub reference a non-existent commit.
    let original_sub_rev = git_rev_parse(&subrepo, "HEAD");
    git_command_for_testing(&subrepo)
        .args(["commit", "--amend", "-m", "Different message"])
        .assert()
        .success();
    // Add another commit to toprepo which is still not pointing to the amended
    // commit in subrepo. Should warn in clone, fetch and recombine.
    git_command_for_testing(&toprepo)
        .args(["commit", "--allow-empty", "-m", "Still wrong pointer"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Commit {original_sub_rev} in sub is missing, referenced from top"
        )))
        .stderr(
            predicate::function(|stderr: &str| {
                // There should be only one warning, for the commit that is missing.
                stderr.matches("WARN:").count() == 1
            })
            .name("exactly 1 warning"),
        );
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Commit {original_sub_rev} in sub is missing, referenced from top"
        )))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 1
        }));
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["recombine", "--use-cache"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Commit {original_sub_rev} in sub is missing, referenced from top"
        )))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 1
        }));

    // Reference it so that warnings are removed.
    git_command_for_testing(&subrepo)
        .args(["tag", "original-commit", &original_sub_rev])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["recombine"])
        .assert()
        .success()
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 0
        }));
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 0
        }));
}
