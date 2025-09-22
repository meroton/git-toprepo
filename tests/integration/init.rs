use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn into_non_existing_dir() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("init")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {clone_name}",
        )));
}

#[test]
fn into_empty_dir() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    std::fs::create_dir(temp_dir.join(clone_name)).unwrap();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("init")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {clone_name}",
        )));
}

#[test]
fn force_into_non_empty_dir() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    let clone_repo = temp_dir.join(clone_name);
    std::fs::create_dir(&clone_repo).unwrap();
    std::fs::write(clone_repo.join(".some-hidden-file"), "hello").unwrap();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("init")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .code(1)
        .stderr(predicate::eq(format!(
            "ERROR: Target directory {clone_name:?} is not empty\n"
        )));

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(temp_dir)
        .arg("init")
        .arg("--force")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {clone_name}",
        )));
}
