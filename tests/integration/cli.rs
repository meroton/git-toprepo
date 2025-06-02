use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_toprepo_init() {
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
