use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use itertools::Itertools as _;
use predicates::prelude::*;
use std::path::Path;

#[test]
fn assemble_golden() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_golden_commit_message.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg("-v")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\nDEBUG: Path /: Unknown commit message encoding \"bad-encoding\", assuming UTF-8\n",
        ))
        .stderr(predicate::str::contains(
            "\nDEBUG: Path /: Commit message decoding errors\n",
        ));
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    insta::assert_snapshot!(
        log_graph,
        @r"
    * commit 9ac2c1d3d376838ebdd6d2de96e287a66893d921
    | Author: author <author@example.com>
    | Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |     Update git submodules
    |
    |     Git-Toprepo-Ref: <top> 46a6323bebe7135d927f88e116b456451ebc48f7
    |
    *   commit 022ae7075428c2dca1950e360f2468b8e5bcdad2
    |\  Merge: 7e96b58 e39a6a4
    | | Author: author <author@example.com>
    | | Date:   Sat Jan 1 00:00:00 2000 +0000
    | |
    | |     all-3
    | |
    | |     Git-Toprepo-Ref: <top> e22d0797b2da3b26f2872254ad95e35f3b0539b0
    | |     Git-Toprepo-Ref: subpathy 0123456789012345678901234567890123456789 unknown submodule
    | |     Git-Toprepo-Ref: subpathz removed
    | |     Footer: X
    | |     Git-Toprepo-Ref: subpathx 3f148600685ecec54c88517c72213e8be62fa7d2
    | |
    | * commit e39a6a4b697cec6cf5485153e99c7c29e96d5519
    |/  Author: author <author@example.com>
    |   Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |       sub-2
    |
    |       Git-Toprepo-Ref: subpathx c05fdf47f83a6cbdcc4aefc66d14095b2d4a2175
    |
    |   A subpathx/sub-2.txt
    |
    * commit 7e96b58e1b27c3c35d6e01ae16016c8426d14421
    | Author: author <author@example.com>
    | Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |     Bad � encoding
    |
    |     Git-Toprepo-Ref: <top> 39f908a3cabd91f91a5450e9159f9ca241080af6
    |
    *   commit ee87bca2a6f840f1890d343bc8396d0a61790167
    |\  Merge: 607ebe2 264b7e1
    | | Author: author <author@example.com>
    | | Date:   Sat Jan 1 00:00:00 2000 +0000
    | |
    | |     Regress x and missing commit y
    | |
    | |     End with some extra empty lines that are trimmed.
    | |
    | |     Git-Toprepo-Ref: <top> 25f73dd213e262bddbaa6825507981ce6d540190
    | |
    | |     x-1
    | |
    | |     Git-Toprepo-Ref: subpathx 55653d7a847a2d66486230ecca4b8d56ddb0bbc6
    | |
    | |     Git-Toprepo-Ref: subpathy 0123456789012345678901234567890123456789 not found
    | |
    | * commit 264b7e1a1133d914d1c383914e1bdcfb8e15e198
    |/  Author: author <author@example.com>
    |   Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |       Resetting submodule subpathx to 55653d7a847a
    |
    |       The gitlinks of the parents to this commit references the commit:
    |       - 3f148600685ecec54c88517c72213e8be62fa7d2
    |       Regress the gitlink to the earlier commit
    |       55653d7a847a2d66486230ecca4b8d56ddb0bbc6:
    |
    |       x-1
    |
    |   D subpathx/all-3.txt
    |   D subpathx/sub-2.txt
    |
    * commit 607ebe2a59812ce2e86bac276cacc2cf8e0d4f0c
    | Author: author <author@example.com>
    | Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |     all-3
    |
    |     Git-Toprepo-Ref: <top> de226a6a2b6f0b7927f10179470578f87b3f98bd
    |     Footer: X
    |     Git-Toprepo-Ref: subpathx 3f148600685ecec54c88517c72213e8be62fa7d2
    |     Footer: Y
    |     Git-Toprepo-Ref: subpathy 92c35e7b31db5264abd4bf71b5cfe0a8d0c389e3
    |
    | A all-3.txt
    | A subpathx/all-3.txt
    | A subpathy/all-3.txt
    |
    * commit 77b3240962d925cb715b0f07ece01cee49f8123a
    | Author: author <author@example.com>
    | Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |     top-and-y-2
    |
    |     Git-Toprepo-Ref: <top> b1ba9b3d1873a1676df20362ed020fb827ca855e
    |     Git-Toprepo-Ref: subpathy 6b312c7ae87753d4d2ba7fed69831e373b30021e
    |
    |     sub-2
    |
    |     Git-Toprepo-Ref: subpathx c05fdf47f83a6cbdcc4aefc66d14095b2d4a2175
    |
    | A subpathx/sub-2.txt
    | A subpathy/top-and-y-2.txt
    | A top-and-y-2.txt
    |
    *-.   commit 56d52bb117ed37d8503600eb0ec6924315fd5630
    |\ \  Merge: 6f66116 55653d7 a789a5c
    | | | Author: author <author@example.com>
    | | | Date:   Sat Jan 1 00:00:00 2000 +0000
    | | |
    | | |     top-1
    | | |
    | | |     With: a footer
    | | |     Git-Toprepo-Ref: <top> 205b9a8189d496bd2f59b8c03052edef01dcb9da
    | | |
    | | |     x-1
    | | |
    | | |     Git-Toprepo-Ref: subpathx 55653d7a847a2d66486230ecca4b8d56ddb0bbc6
    | | |
    | | |     y-1
    | | |
    | | |     Git-Toprepo-Ref: subpathy a789a5ca1e2cb59b9afc71a0c73fcedcc3bf6dd2
    | | |
    | | |     Git-Toprepo-Ref: subpathz 0011223344556677889900112233445566778899 (submodule)
    | | |
    | | * commit a789a5ca1e2cb59b9afc71a0c73fcedcc3bf6dd2
    | |   Author: author <author@example.com>
    | |   Date:   Sat Jan 1 00:00:00 2000 +0000
    | |
    | |       y-1
    | |
    | |   A y-1.txt
    | |
    | * commit 55653d7a847a2d66486230ecca4b8d56ddb0bbc6
    |   Author: author <author@example.com>
    |   Date:   Sat Jan 1 00:00:00 2000 +0000
    |
    |       x-1
    |
    |   A x-1.txt
    |
    * commit 6f66116bf3ce5a27ea4726348e3283702839717c
      Author: author <author@example.com>
      Date:   Sat Jan 1 00:00:00 2000 +0000

          Initial empty commit
    "
    );
}

fn extract_log_graph(repo_path: &Path, extra_args: Vec<&str>) -> String {
    let log_command = git_command_for_testing(repo_path)
        .args(["log", "--graph"])
        .args(extra_args)
        .assert()
        .success();
    let log_graph = log_command.get_output().stdout.to_str().unwrap();
    // Replace TAB and trailing spaces.
    log_graph
        .split('\n')
        .map(str::trim_end)
        .join("\n")
        .replace('\t', " ")
}

#[test]
fn split_example() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");
    let subxrepo = temp_dir.join("repox");
    let subyrepo = temp_dir.join("repoy");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m"])
        .arg(
            "Add files

Body text

Topic: my-topic
With: a footer
Git-Toprepo-Ref: <top>
Topic: my-topic
Git-Toprepo-Ref: subpathy something-random

subx subject

Git-Toprepo-Ref: subpathx
Topic: remove-this-line
subx-footer: keep-this-line
",
        )
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN").not());

    assert_eq!(
        git_commit_message(&toprepo, "other"),
        "Add files\n\nBody text\n\nWith: a footer\n"
    );
    assert_eq!(
        git_commit_message(&subxrepo, "other"),
        "subx subject\n\nsubx-footer: keep-this-line\n"
    );
    assert_eq!(
        git_commit_message(&subyrepo, "other"),
        "Add files\n\nBody text\n"
    );
}

#[test]
fn split_where_one_repo_is_missing() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let monorepo = temp_dir.join("mono");
    let toprepo = temp_dir.join("top");
    let subxrepo = temp_dir.join("repox");
    let subyrepo = temp_dir.join("repoy");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subpathx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("subpathy/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subpathx/file.txt", "subpathy/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m"])
        .arg(
            "Add files

Git-Toprepo-Ref: <top>
Topic: my-topic

subx subject

Topic: my-topic
Git-Toprepo-Ref: subpathx
",
        )
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN").not());

    assert_eq!(git_commit_message(&toprepo, "other"), "Add files\n");
    assert_eq!(git_commit_message(&subxrepo, "other"), "subx subject\n");
    assert_eq!(git_commit_message(&subyrepo, "other"), "Add files\n");

    // The same, but where the toprepo is missing a message.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m"])
        .arg(
            "suby subject

Topic: my-topic
Git-Toprepo-Ref: subpathy

subx subject

Git-Toprepo-Ref: subpathx
Topic: my-topic
",
        )
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::is_match(
                "^ERROR: No commit message found for path <top> in mono commit [0-9a-f]+\n$",
            )
            .unwrap(),
        );

    // The same, but with a residual message in the toprepo.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m"])
        .arg(
            "suby subject

Git-Toprepo-Ref: subpathy
Topic: my-topic

Residual message

Topic: other-topic
",
        )
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN").not());

    assert_eq!(git_commit_message(&toprepo, "other"), "Residual message\n");
    assert_eq!(git_commit_message(&subxrepo, "other"), "Residual message\n");
    assert_eq!(git_commit_message(&subyrepo, "other"), "suby subject\n");

    // No message assigned to specific paths.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m", "Subject\n\nTopic: my-topic"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN").not());

    assert_eq!(git_commit_message(&toprepo, "other"), "Subject\n");
    assert_eq!(git_commit_message(&subxrepo, "other"), "Subject\n");
    assert_eq!(git_commit_message(&subyrepo, "other"), "Subject\n");
}

fn git_commit_message(repo_path: &Path, revision: &str) -> String {
    let show_command = git_command_for_testing(repo_path)
        .args(["cat-file", "-p", revision])
        .assert()
        .success();
    let stdout = show_command.get_output().stdout.to_str().unwrap();
    stdout.split_once("\n\n").unwrap().1.to_owned()
}
