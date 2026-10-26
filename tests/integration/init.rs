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
            "Initialized a Git Toprepo in {clone_name}",
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
            "Initialized a Git Toprepo in {clone_name}",
        )));
}

#[test]
fn force_into_non_empty_dir_with_repo_name() {
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
        .stderr(format!(
            "ERROR: Target directory \"{clone_name}\" is not empty\n"
        ));
    assert!(!clone_repo.join(".git").is_dir());

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&temp_dir)
        .arg("init")
        .arg("--force")
        .arg(&toprepo)
        .arg(clone_name)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized a Git Toprepo in {clone_name}",
        )));
    assert!(clone_repo.join(".git").is_dir());
}

#[test]
fn force_into_non_empty_dir_without_repo_name() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");

    let clone_repo = temp_dir.join("dest");
    std::fs::create_dir(&clone_repo).unwrap();
    std::fs::write(clone_repo.join(".some-hidden-file"), "hello").unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&clone_repo)
        .arg("init")
        .arg(&toprepo)
        .assert()
        .code(1)
        .stderr("ERROR: Target directory \".\" is not empty\n");
    assert!(!clone_repo.join(".git").is_dir());

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&clone_repo)
        .arg("init")
        .arg("--force")
        .arg(&toprepo)
        .assert()
        .success()
        .stderr(predicate::str::contains("Initialized a Git Toprepo in ."));
    assert!(clone_repo.join(".git").is_dir());
}
