use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use predicates::prelude::*;

#[test]
fn into_non_existing_dir() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
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
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    std::fs::create_dir(temp_dir.join(clone_name)).unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
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
    let toprepo = temp_dir.join("top");

    let clone_name = "my-clone";
    let clone_repo = temp_dir.join(clone_name);
    std::fs::create_dir(&clone_repo).unwrap();
    std::fs::write(clone_repo.join(".some-hidden-file"), "hello").unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("init")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .code(1)
        .stderr(predicate::eq(format!(
            "ERROR: Target directory {clone_name:?} is not empty\n"
        )));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
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
