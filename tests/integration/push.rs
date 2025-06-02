use assert_cmd::prelude::*;
use git_toprepo::git::commit_env_for_testing;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_push_empty_commit_should_fail() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "--allow-empty", "-m", "Empty commit"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:main"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::is_match(
                "ERROR: Pushing empty commits like [0-9a-f]+ is not supported\n",
            )
            .unwrap(),
        );
}

#[test]
fn test_push_top() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add file"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}\n",
            toprepo.display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());

    Command::new("git")
        .current_dir(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}

#[test]
fn test_push_submodule() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("sub/file.txt"), "text\n").unwrap();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "sub/file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add file"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}\n",
            toprepo.parent().unwrap().join("sub/").display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());

    Command::new("git")
        .current_dir(toprepo.join("../sub"))
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}

#[test]
fn test_push_revision() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add file"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    let out = Command::new("git")
        .current_dir(&monorepo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let revision = String::from(std::str::from_utf8(&out.stdout).unwrap());
    let revision = git_toprepo::util::trim_newline_suffix(&revision);

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("push")
        .arg("origin")
        .arg(format!("{revision}:refs/heads/foo"))
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}\n",
            toprepo.display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());
}

#[test]
fn test_push_from_sub_directory() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add file"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        // Don't push from the worktree root.
        .current_dir(monorepo.join("sub"))
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}\n",
            toprepo.display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());

    Command::new("git")
        .current_dir(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}
