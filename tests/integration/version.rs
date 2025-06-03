use assert_cmd::prelude::*;
use std::process::Command;

#[test]
fn test_toprepo_version() {
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout("git_toprepo 0.0.0~timestamp-git-hash\n")
        .stderr("");
}

#[test]
fn test_toprepo_dash_dash_version() {
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout("git_toprepo 0.0.0~timestamp-git-hash\n")
        .stderr("");
}

#[test]
fn test_toprepo_short_flag_version() {
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("-V")
        .assert()
        .success()
        .stdout("git_toprepo 0.0.0~timestamp-git-hash\n")
        .stderr("");
}
