use assert_cmd::prelude::*;
use git_toprepo::git::commit_env_for_testing;
use git_toprepo::git::git_command;
use git_toprepo::util::NewlineTrimmer as _;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_push_empty_commit_should_fail() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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
fn test_push_duplicate_branch() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/new-branch"])
        .assert()
        .success();

    // It is enough to push to the top repository, as the submodules are not
    // changed and their commits are already present but potentially under a
    // different ref.
    Command::new("git")
        .current_dir(&toprepo)
        .args([
            "diff",
            "--exit-code",
            "refs/heads/main",
            "refs/heads/new-branch",
        ])
        .assert()
        .success();
}

#[test]
fn test_push_top() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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
            toprepo.canonicalize().unwrap().display()
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
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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
            "To {}/\n",
            toprepo.join("../sub").canonicalize().unwrap().display()
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
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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

    let cmd = Command::new("git")
        .current_dir(&monorepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success();
    let out = cmd.get_output();
    let revision = String::from_utf8(out.to_owned().stdout).unwrap();
    let revision = revision.trim_newline_suffix();

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
            toprepo.canonicalize().unwrap().display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());
}

#[test]
fn test_push_from_sub_directory() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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
            toprepo.canonicalize().unwrap().display()
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
fn test_push_shortrev() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
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

    let cmd = Command::new("git")
        .current_dir(&monorepo)
        .args(["rev-parse", "--short", "HEAD"])
        .envs(commit_env_for_testing())
        .assert()
        .success();
    let output = cmd.get_output();
    let rev = String::from_utf8(output.to_owned().stdout).unwrap();
    let rev = rev.trim();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", format!("{rev}:refs/heads/foo").as_str()])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}\n",
            toprepo.canonicalize().unwrap().display()
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
fn test_push_top_and_submodule_in_series() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "--jobs=1", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        // First execute one push, then the other.
        .stderr(
            predicate::str::is_match(
                "\
INFO: Running git push .*/sub/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Stderr from git push .*/sub/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: prereceive hook sleeping\\s*
remote: prereceive hook continues\\s*
To .*/sub/?
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
INFO: Running git push .*/top/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Stderr from git push .*/top/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: prereceive hook sleeping\\s*
remote: prereceive hook continues\\s*
To .*/top/?
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
",
            )
            .unwrap(),
        );

    Command::new("git")
        .current_dir(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("top\n");
    Command::new("git")
        .current_dir(&subrepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("submodule\n");
}

#[test]
fn test_push_top_and_submodule_in_parallel() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        // Both pushes should have started in parallel before printing the
        // "pre-receive hook continues" lines.
        .stderr(
            predicate::str::is_match(
                "\
INFO: Running git push .* -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Running git push .* -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Stderr from git push .* -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: prereceive hook sleeping\\s*
remote: prereceive hook continues\\s*
To .*
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
INFO: Stderr from git push .* -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: prereceive hook sleeping\\s*
remote: prereceive hook continues\\s*
To .*
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
",
            )
            .unwrap(),
        );

    Command::new("git")
        .current_dir(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("top\n");
    Command::new("git")
        .current_dir(&subrepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("submodule\n");
}

#[test]
fn test_push_topic_removed_from_commit_message() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success();

    // Check for missing topic and a single LF at the end.
    Command::new("git")
        .current_dir(&toprepo)
        .args(["cat-file", "-p", "refs/heads/foo"])
        .assert()
        .success()
        .stdout(predicate::str::ends_with("\n\nAdd files\n"));
    Command::new("git")
        .current_dir(&subrepo)
        .args(["cat-file", "-p", "refs/heads/foo"])
        .assert()
        .success()
        .stdout(predicate::str::ends_with("\n\nAdd files\n"));
}

#[test]
fn test_push_topic_is_used_as_push_option() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();

    Command::new("git")
        .current_dir(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    Command::new("git")
        .current_dir(&monorepo)
        .args(["commit", "-m", "Add file\n\nTopic: my-topic"])
        .envs(commit_env_for_testing())
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        // Both pushes should have started in parallel before printing the
        // "pre-receive hook continues" lines.
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_0=topic=my-topic",
        ));
}

#[test]
fn test_force_push() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("suby/file.txt"), "suby\n").unwrap();
    git_command(&monorepo)
        .args(["add", "top.txt"])
        .assert()
        .success();
    git_command(&monorepo)
        .args(["commit", "-m", "Add file"])
        .envs(commit_env_for_testing())
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success();

    // --force
    git_command(&monorepo)
        .args(["commit", "--amend", "-m", "Force"])
        .envs(commit_env_for_testing())
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .code(1)
        .stderr(
            predicates::str::is_match(
                r"\n ! \[rejected\] *[0-9a-f]+ -> other \(non-fast-forward\)\n",
            )
            .unwrap(),
        );
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicates::str::contains(" -> other (forced update)"));
}
