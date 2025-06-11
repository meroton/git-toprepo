use anyhow::Context as _;
use assert_cmd::prelude::*;
use bstr::ByteSlice as _;
use git_toprepo::git::git_command;
use git_toprepo::util::CommandExtension as _;
use predicates::prelude::*;
use std::collections::HashMap;
use std::process::Command;

#[test]
fn test_toprepo_clone() {
    let base_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
        "git_toprepo-test_toprepo_clone",
    );
    let from_path = &base_dir.path().join("from");
    std::fs::create_dir(from_path).unwrap();
    let to_path = &base_dir.path().join("to");
    std::fs::create_dir(to_path).unwrap();

    // TODO: Can this use the deterministic environment setup?
    // Or are these particular values important?
    let env = HashMap::from([
        ("GIT_AUTHOR_NAME", "A Name"),
        ("GIT_AUTHOR_EMAIL", "a@no.example"),
        ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
        ("GIT_COMMITTER_NAME", "C Name"),
        ("GIT_COMMITTER_EMAIL", "c@no.example"),
        ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
    ]);

    git_command(from_path)
        .args(["init", "--quiet", "--initial-branch", "main"])
        .envs(&env)
        .assert()
        .success();
    git_command(from_path)
        .args(["commit", "--allow-empty", "--quiet"])
        .args(["-m", "Initial commit"])
        .envs(&env)
        .assert()
        .success();
    std::fs::write(from_path.join(".gittoprepo.toml"), "").unwrap();
    git_command(from_path)
        .args(["add", ".gittoprepo.toml"])
        .envs(&env)
        .assert()
        .success();
    git_command(from_path)
        .args(["commit", "--quiet"])
        .args(["-m", "Config file"])
        .envs(&env)
        .assert()
        .success();
    git_command(from_path)
        .args(["tag", "mytag"])
        .envs(&env)
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(from_path)
        .arg(to_path)
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Initialized git-toprepo in {}",
            to_path.display()
        )))
        .stderr(predicate::str::contains(
            "Expanding the toprepo to a monorepo...",
        ));

    let to_gix_repo = gix::open(to_path)
        .with_context(|| format!("Failed to open gix repository {}", to_path.display()))
        .unwrap();

    let ref_pairs = vec![
        ("HEAD", "refs/namespaces/top/refs/remotes/origin/HEAD"),
        ("main", "refs/namespaces/top/refs/remotes/origin/main"),
        ("mytag", "refs/namespaces/top/refs/tags/mytag"),
    ];
    for (orig_ref, top_ref) in ref_pairs {
        let orig_rev = git_command(from_path)
            .args(["rev-parse", "--verify", orig_ref])
            .output_stdout_only()
            .unwrap()
            .check_success_with_stderr()
            .with_context(|| format!("orig {orig_ref}"))
            .unwrap()
            .stdout
            .to_owned();
        let top_rev = git_command(to_gix_repo.git_dir())
            .args(["rev-parse", "--verify", top_ref])
            .output_stdout_only()
            .unwrap()
            .check_success_with_stderr()
            .with_context(|| format!("top {top_ref}"))
            .unwrap()
            .stdout
            .to_owned();
        assert_eq!(
            orig_rev.to_str().unwrap(),
            top_rev.to_str().unwrap(),
            "ref {orig_ref} mismatch",
        );
    }
}
