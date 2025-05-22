mod fixtures;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_toprepo_init() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();

    let clone_name = "clone";

    let mut cmd = Command::cargo_bin("git-toprepo")?;
    cmd.current_dir(temp_dir).arg("init").arg(toprepo).arg(clone_name);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains(format!("Initialized git-toprepo in {}", clone_name)));
    Ok(())
}
