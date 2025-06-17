use assert_cmd::prelude::*;
use predicates::prelude::predicate;
use std::process::Command;

#[test]
fn test_toprepo_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}

#[test]
fn test_toprepo_dash_dash_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}

#[test]
fn test_toprepo_short_flag_version() {
    let validate_stdout = predicate::str::is_match("^git-toprepo .*~.*-.*\n$").unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("-V")
        .assert()
        .success()
        .stdout(validate_stdout)
        .stderr("");
}
