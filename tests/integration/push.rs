use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::git_rev_parse;
use itertools::Itertools as _;
use predicates::prelude::*;

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

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
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

    std::fs::write(monorepo.join("subpath/file.txt"), "text\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "subpath/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "To {}/\n",
            toprepo.join("../repo").canonicalize().unwrap().display()
        )))
        .stderr(predicate::str::is_match(r"\n \* \[new branch\]\s+[0-9a-f]+ -> foo\n").unwrap());

    git_command_for_testing(toprepo.join("../repo"))
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

    let revision = git_rev_parse(&monorepo, "HEAD");
    cargo_bin_git_toprepo_for_testing()
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
    cargo_bin_git_toprepo_for_testing()
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
        (&monorepo.join("subpath"), vec![]),
        // `-C .` should trivially give the same result.
        (&monorepo, vec!["-C", "subpath"]),
        (&monorepo.join("subpath"), vec!["-C", "."]),
    ] {
        cargo_bin_git_toprepo_for_testing()
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

    // Not a full rev.
    let rev = &git_rev_parse(&monorepo, "HEAD")[..13];
    cargo_bin_git_toprepo_for_testing()
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
    let subrepo = temp_dir.join("repo");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpath/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "subpath/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "--jobs=1", "HEAD:refs/heads/foo"])
        .assert()
        .success()
        // First execute one push, then the other.
        .stderr(
            predicate::str::is_match(
                "\
INFO: Running git push .*/repo/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
INFO: Stderr from git push .*/repo/? -o topic=my-topic [0-9a-f]+:refs/heads/foo
remote: GIT_PUSH_OPTION_0=topic=my-topic\\s*
remote: pre-receive hook sleeping\\s*
remote: pre-receive hook continues\\s*
To .*/repo/?
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
    let subrepo = temp_dir.join("repo");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpath/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "subpath/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
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
fn original_submodule_commit_as_parent() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    git_command_for_testing(&monorepo)
        .args([
            "commit",
            "--amend",
            "-m",
            "Message in worktree\n\nTopic: work",
        ])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "--jobs=1",
            "--dry-run",
            "origin",
            "HEAD:refs/dry/run",
        ])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repox/ -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repoy/ -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*top -o topic=work [0-9a-f]+:refs/dry/run\n",
            )
            .unwrap(),
        )
        .stderr(predicate::function(|s: &str| {
            s.matches("INFO: Would run git push").count() == 3
        }));
}

#[test]
fn topic_removed_from_commit_message() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let subrepo = temp_dir.join("repo");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("file.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpath/file.txt"), "submodule\n").unwrap();

    git_command_for_testing(&monorepo)
        .args(["add", "file.txt", "subpath/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
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

    cargo_bin_git_toprepo_for_testing()
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
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files"])
        .assert()
        .success();
    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .code(1);
    insta::assert_snapshot!(
        cmd.get_output().stderr.to_str().unwrap(),
                @r#"ERROR: Multiple submodules are modified in commit 068408a74b19b6d4c260af0cc109d2c0c475f7d5 "Add files", but no topic was provided. Please push to a Gerrit refspec like refs/for/branch/topic-name or amend the commit message to add a 'Topic: something-descriptive' footer line."#
    );
}

/// When pushing to a Gerrit-style refspec like `refs/for/master/TOPIC`, the topic
/// is extracted from the refspec path for validation. No `-o topic=` is emitted
/// since Gerrit already reads the topic from the refspec.
#[test]
fn topic_extracted_from_gerrit_refspec() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    // No Topic: footer — the topic comes from the refspec, no -o needed.
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "--jobs=1",
            "--dry-run",
            "origin",
            "HEAD:refs/for/master/my-topic",
        ])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repox/ [0-9a-f]+:refs/for/master/my-topic\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repoy/ [0-9a-f]+:refs/for/master/my-topic\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*top [0-9a-f]+:refs/for/master/my-topic\n",
            )
            .unwrap(),
        )
        .stderr(predicate::str::contains("-o topic=").not());
}

/// When pushing with Gerrit `%topic=` syntax, the topic is extracted for validation
/// and the `%topic=...` suffix is preserved in the refspec. No `-o topic=` is emitted
/// since Gerrit reads the topic from the `%topic=` in the refspec.
#[test]
fn topic_extracted_from_gerrit_push_option_in_refspec() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    // No Topic: footer — the topic comes from %topic= in the refspec, no -o needed.
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files"])
        .assert()
        .success();

    // The %topic= suffix is preserved in the refspec; Gerrit reads the topic from it.
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "--jobs=1",
            "--dry-run",
            "origin",
            "HEAD:refs/for/master%topic=pct-topic",
        ])
        .assert()
        .success()
        .stdout("")
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repox/ [0-9a-f]+:refs/for/master%topic=pct-topic\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*repoy/ [0-9a-f]+:refs/for/master%topic=pct-topic\n",
            )
            .unwrap(),
        )
        .stderr(
            predicate::str::is_match(
                "INFO: Would run git push .*top [0-9a-f]+:refs/for/master%topic=pct-topic\n",
            )
            .unwrap(),
        )
        .stderr(predicate::str::contains("-o topic=").not());
}

#[test]
fn topic_priority_across_commits() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    // Commit A: no Topic footer -> uses refspec %topic=messaging
    std::fs::write(monorepo.join("top.txt"), "A\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "A\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "A\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Commit A"])
        .assert()
        .success();

    // Commit B: Topic: web -> footer wins over refspec %topic=messaging
    std::fs::write(monorepo.join("top.txt"), "B\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "B\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "B\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Commit B\n\nTopic: web"])
        .assert()
        .success();

    // Commit C: Topic: rpc -> footer wins over refspec %topic=messaging
    std::fs::write(monorepo.join("top.txt"), "C\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "C\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "C\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Commit C\n\nTopic: rpc"])
        .assert()
        .success();

    // Commit D: no Topic footer -> uses refspec %topic=messaging
    std::fs::write(monorepo.join("top.txt"), "D\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "D\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "D\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Commit D"])
        .assert()
        .success();

    let result = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "--jobs=1",
            "--dry-run",
            "origin",
            "HEAD:refs/for/master%topic=messaging",
        ])
        .assert()
        .success();

    let stderr = &result.get_output().stderr;
    let stderr_str = String::from_utf8_lossy(stderr);

    // 4 commits × 3 repos = 12 pushes total.
    // A and D have no Topic footer -> no -o topic= (topic from refspec %topic=messaging).
    // B has Topic: web -> -o topic=web (3 pushes).
    // C has Topic: rpc -> -o topic=rpc (3 pushes).
    let count_topic = |output: &str, topic: &str| {
        let pattern = format!("-o topic={topic}");
        output.matches(&pattern).count()
    };
    assert_eq!(
        count_topic(&stderr_str, "messaging"),
        0,
        "expected 0 pushes with -o topic=messaging (refspec topic not repeated as -o):\n{stderr_str}"
    );
    assert_eq!(
        count_topic(&stderr_str, "web"),
        3,
        "expected 3 pushes with topic=web (commit B, 3 repos):\n{stderr_str}"
    );
    assert_eq!(
        count_topic(&stderr_str, "rpc"),
        3,
        "expected 3 pushes with topic=rpc (commit C, 3 repos):\n{stderr_str}"
    );
}

#[test]
fn override_url_in_config() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");
    let repox = temp_dir.join("repox");
    let repoy = temp_dir.join("repoy");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: my-topic"])
        .assert()
        .success();
    // Push top to repox and subx to repoy.
    std::fs::write(
        monorepo.join(".gittoprepo.user.toml"),
        r#"
[repo.namex]
url = "../repox/"
push.url = "../repoy/"
[repo.namey]
url = "../repoy/"
push.url = "../non-existing/"
"#,
    )
    .unwrap();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("recombine")
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("push")
        .arg(&repox)
        .arg("HEAD:refs/heads/other")
        .assert()
        .success();

    git_command_for_testing(&repox)
        .args(["show", "refs/heads/other:top.txt"])
        .assert()
        .success()
        .stdout("top\n");
    git_command_for_testing(&repoy)
        .args(["show", "refs/heads/other:file.txt"])
        .assert()
        .success()
        .stdout("subx\n");
}

/// Regression test where pushing a commit that modifies multiple submodules did
/// only keep ancestry for the alphabetically last submodule processed.
#[test]
fn keep_commit_ancestry_for_all_repos() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");
    let subx = temp_dir.join("repox");
    let suby = temp_dir.join("repoy");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files\n\nTopic: 1"])
        .assert()
        .success();
    std::fs::write(monorepo.join("top.txt"), "top2\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx2\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby2\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add files2\n\nTopic: 2\n"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success();

    git_command_for_testing(&toprepo)
        .args(["log", "--oneline", "--format=%s", "-n3", "refs/heads/other"])
        .assert()
        .success()
        .stdout("Add files2\nAdd files\nA1-main\n");
    git_command_for_testing(&subx)
        .args(["log", "--oneline", "--format=%s", "-n3", "refs/heads/other"])
        .assert()
        .success()
        .stdout("Add files2\nAdd files\nx-main-1\n");
    git_command_for_testing(&suby)
        .args(["log", "--oneline", "--format=%s", "-n3", "refs/heads/other"])
        .assert()
        .success()
        .stdout("Add files2\nAdd files\ny-main-1\n");
}

#[test]
fn force_push() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success();

    // --force
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m", "Force"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::is_match(
                r"\n ! \[rejected\] *[0-9a-f]+ -> other \(non-fast-forward\)\n",
            )
            .unwrap(),
        );
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicate::str::contains(" -> other (forced update)"));
}

#[test]
fn push_option_single() {
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

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "origin",
            "--push-option",
            "review=my-review-id",
            "HEAD:refs/heads/foo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_0=review=my-review-id",
        ))
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_1=topic=my-topic",
        ));
}

#[test]
fn push_option_short_form() {
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
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "origin",
            "-o",
            "description=my-desc",
            "HEAD:refs/heads/foo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_0=description=my-desc",
        ));

    git_command_for_testing(&toprepo)
        .args(["show", "refs/heads/foo:file.txt"])
        .assert()
        .success()
        .stdout("top\n");
}

#[test]
fn push_option_multiple() {
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

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "origin",
            "-o",
            "key1=val1",
            "-o",
            "key2=val2",
            "HEAD:refs/heads/foo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_0=key1=val1",
        ))
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_1=key2=val2",
        ))
        .stderr(predicate::str::contains(
            "\nremote: GIT_PUSH_OPTION_2=topic=my-topic",
        ));
}

#[test]
fn push_option_dry_run() {
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
        .args(["commit", "-m", "Add file"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args([
            "push",
            "origin",
            "--dry-run",
            "--push-option",
            "my-opt=my-val",
            "HEAD:refs/heads/foo",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_match(
            "INFO: Would run git push .* --push-option my-opt=my-val [0-9a-f]+:refs/heads/foo\n",
        )
        .unwrap());
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
    cargo_bin_git_toprepo_for_testing()
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
