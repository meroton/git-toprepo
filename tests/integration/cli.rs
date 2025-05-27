use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_toprepo_init() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();

    let clone_name = "clone";

    let mut cmd = Command::cargo_bin("git-toprepo")?;
    cmd.current_dir(temp_dir)
        .arg("init")
        .arg(toprepo)
        .arg(clone_name);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {clone_name}",
        )));
    Ok(())
}

#[test]
fn test_push_revision() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let clone_name = "clone";
    let clone_path = temp_dir.join(clone_name);

    let mut cmd = Command::cargo_bin("git-toprepo")?;
    cmd.current_dir(temp_dir)
        .arg("init")
        .arg(toprepo)
        .arg(clone_name);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {clone_name}",
        )));

    Command::new("git")
        .current_dir(&clone_path)
        .args(["commit", "--allow-empty", "-m", "empty commit"])
        .assert()
        .success();

    let out = Command::new("git")
        .current_dir(&clone_path)
        .args(["rev-parse", "HEAD"])
        .output()?;

    let revision = String::from(std::str::from_utf8(&out.stdout)?);
    let revision = revision.trim();

    let mut cmd = Command::cargo_bin("git-toprepo")?;
    cmd.current_dir(temp_dir.join(clone_name))
        .arg("push")
        .arg("origin")
        .arg(format!("{revision}:master"));
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("push"));
    Ok(())
}
