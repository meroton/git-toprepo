use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use predicates::prelude::*;

#[test]
fn toprepo_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .arg("version")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}

#[test]
fn toprepo_dash_dash_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .arg("--version")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}

#[test]
fn toprepo_short_flag_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    cargo_bin_git_toprepo_for_testing()
        .arg("-V")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}
