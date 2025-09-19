use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt as _;
use bstr::ByteSlice as _;
use git_toprepo::git::git_command_for_testing;
use git_toprepo::util::NewlineTrimmer as _;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::prelude::predicate;

#[test]
fn test_log_only_fixable_missing_gitmodules_warnings() {
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
    let missing_gitmodules_rev = git_command_for_testing(&toprepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim_newline_suffix()
        .to_owned();
    // With another commit, the commit above is no longer fixable.
    git_command_for_testing(&toprepo)
        .args(["commit", "--allow-empty", "-m", "Still no .gitmodules"])
        .assert()
        .success();
    let still_missing_gitmodules_rev = git_command_for_testing(&toprepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim_newline_suffix()
        .to_owned();

    Command::cargo_bin("git-toprepo")
        .unwrap()
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
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN:").not());
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["refilter"])
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
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!("WARN: Commit {missing_gitmodules_rev} in top (refs/remotes/origin/first-missing-gitmodules): Cannot resolve submodule sub, .gitmodules is missing")))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning about missing .gitmodules, the tip of the branch that is fixable.
            stderr.matches(".gitmodules is missing").count() == 1
        }));
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["refilter"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!("WARN: Commit {missing_gitmodules_rev} in top (refs/remotes/origin/first-missing-gitmodules): Cannot resolve submodule sub, .gitmodules is missing")))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning about missing .gitmodules, the tip of the branch that is fixable.
            stderr.matches(".gitmodules is missing").count() == 1
        }));
}

#[test]
fn test_log_always_show_missing_submod_commit_warnings() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");

    // Make top.git/sub reference a non-existent commit.
    let original_sub_rev = git_command_for_testing(&subrepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim_newline_suffix()
        .to_owned();
    git_command_for_testing(&subrepo)
        .args(["commit", "--amend", "-m", "Different message"])
        .assert()
        .success();
    // Add another commit to toprepo which is still not pointing to the amended
    // commit in subrepo. Should warn in clone, fetch and refilter.
    git_command_for_testing(&toprepo)
        .args(["commit", "--allow-empty", "-m", "Still wrong pointer"])
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Missing commit in sub: {original_sub_rev}"
        )))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 1
        }));
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Missing commit in sub: {original_sub_rev}"
        )))
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 1
        }));
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["refilter"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "WARN: Missing commit in sub: {original_sub_rev}"
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
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["refilter"])
        .assert()
        .success()
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 0
        }));
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["fetch"])
        .assert()
        .success()
        .stderr(predicate::function(|stderr: &str| {
            // There should be only one warning, for the commit that is missing.
            stderr.matches("WARN:").count() == 0
        }));
}
