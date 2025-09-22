use assert_cmd::prelude::*;
use git_toprepo::git::git_command_for_testing;
use git_toprepo::util::NewlineTrimmer as _;
use itertools::Itertools as _;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn empty_commit_should_fail() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    git_command_for_testing(&monorepo)
        .args(["commit", "--allow-empty", "-m", "Empty commit"])
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
fn duplicate_branch() {
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
    git_command_for_testing(&toprepo)
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
fn root_commit() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
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

    git_command_for_testing(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}

#[test]
fn submodule_commit() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("sub/file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "sub/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
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

    git_command_for_testing(toprepo.join("../sub"))
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}

#[test]
fn revision_as_push_arg() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    let cmd = git_command_for_testing(&monorepo)
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
fn inside_subdirectories() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    // Initial push to seed the remote. This makes sure all the other pushes
    // have the same behavior as pushing is idempotent.
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

    // Push again, this is the reference behavior and should be repeated in subdirectories.
    for (wd, flags) in [
        (&monorepo, vec![]),
        (&monorepo.join("sub"), vec![]),
        // `-C .` should trivially give the same result.
        (&monorepo, vec!["-C", "sub"]),
        (&monorepo.join("sub"), vec!["-C", "."]),
    ] {
        Command::cargo_bin("git-toprepo")
            .unwrap()
            .current_dir(wd)
            .args(flags)
            .args(["push", "origin", "HEAD:refs/heads/foo"])
            .assert()
            .success()
            .stderr(predicate::str::contains("Everything up-to-date"));
    }
}

#[test]
fn shortrev_as_push_arg() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    let cmd = git_command_for_testing(&monorepo)
        .args(["rev-parse", "--short", "HEAD"])
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

    git_command_for_testing(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("text\n");
}

#[test]
fn root_and_submodule_commits_in_series() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
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
remote: pre-receive hook sleeping\\s*
remote: pre-receive hook continues\\s*
To .*/sub/?
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
INFO: Running git push .*/top/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Stderr from git push .*/top/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: pre-receive hook sleeping\\s*
remote: pre-receive hook continues\\s*
To .*/top/?
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
",
            )
            .unwrap(),
        );

    git_command_for_testing(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("top\n");
    git_command_for_testing(&subrepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("submodule\n");
}

#[test]
fn root_and_submodule_commits_in_parallel() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
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
remote: pre-receive hook sleeping\\s*
remote: pre-receive hook continues\\s*
To .*
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
INFO: Stderr from git push .* -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: pre-receive hook sleeping\\s*
remote: pre-receive hook continues\\s*
To .*
 \\* \\[new branch\\]\\s+[0-9a-f]+ -> foo
",
            )
            .unwrap(),
        );

    git_command_for_testing(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("top\n");
    git_command_for_testing(&subrepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("submodule\n");
}

#[test]
fn topic_removed_from_commit_message() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("sub");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("sub/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "sub/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success();

    // Check for missing topic and a single LF at the end.
    git_command_for_testing(&toprepo)
        .args(["cat-file", "-p", "refs/heads/foo"])
        .assert()
        .success()
        .stdout(predicate::str::ends_with("\n\nAdd files\n"));
    git_command_for_testing(&subrepo)
        .args(["cat-file", "-p", "refs/heads/foo"])
        .assert()
        .success()
        .stdout(predicate::str::ends_with("\n\nAdd files\n"));
}

#[test]
fn topic_is_used_as_push_option() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file\n\nTopic: my-topic"])
        .assert()
        .success();

    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_0=topic=my-topic",
        ));
}

#[test]
fn topic_is_required_for_multi_repo_push() {
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
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subx/file.txt", "suby/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files"])
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .code(1)
        .stderr(predicates::str::is_match(r"^ERROR: Multiple submodules are modified in commit [0-9a-f]+, but no topic was provided. Please amend the commit message to add a 'Topic: something-descriptive' footer line.\n$").unwrap());
}

#[test]
fn force_push() {
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
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success();

    // --force
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m", "Force"])
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

/// The following push error message from a Gerrit server should be ignored:
/// ```text
/// ! [remote rejected] HEAD -> refs/for/something (no new changes)
/// ```
#[test]
fn ignore_gerrit_refusing_no_new_change() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let failing_git_push_dir =
        std::path::absolute("tests/integration/fixtures/git-push-gerrit-refuse-no-change").unwrap();

    let old_path_env = std::env::var_os("PATH").unwrap_or_default();
    let new_paths = [failing_git_push_dir]
        .into_iter()
        .chain(std::env::split_paths(&old_path_env))
        .collect_vec();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/gerrit/fail-no-new-change"])
        .env("OLD_PATH", &old_path_env)
        .env("PATH", std::env::join_paths(new_paths).unwrap())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\n ! [remote rejected] HEAD -> refs/for/something (no new changes)\n",
        ));
}
