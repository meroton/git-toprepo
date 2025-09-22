use assert_cmd::assert::OutputAssertExt as _;
use bstr::ByteSlice as _;
use git_toprepo::git::git_command_for_testing;
use itertools::Itertools as _;
use predicates::prelude::PredicateBooleanExt as _;
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

    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg("-v")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicates::str::contains(
            "\nDEBUG: Path /: Unknown commit message encoding \"bad-encoding\", assuming UTF-8\n",
        ))
        .stderr(predicates::str::contains(
            "\nDEBUG: Path /: Commit message decoding errors\n",
        ));
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = &r"
* commit ccd8a950e8a17a28dbe42bd2890df66fdefb67bf
| Author: author <author@example.com>
| Date:   Sat Jan 1 00:00:00 2000 +0000
|
|     Update git submodules
|
|     Git-Toprepo-Ref: <top> 378d47c8797e736a12b95dc668728f6ac135eadd
|
*   commit 423c7af4726bae5129cb0c1d6e6248dfb761f9d3
|\  Merge: 48891e2 f7d9969
| | Author: author <author@example.com>
| | Date:   Sat Jan 1 00:00:00 2000 +0000
| |
| |     all-3
| |
| |     Git-Toprepo-Ref: <top> c478b26854b0111c295fd19465e32bf6cca67b93
| |     Git-Toprepo-Ref: subx 044827c4cbf84f8d007c8ff08f777e28f1fd95f4
| |     Git-Toprepo-Ref: suby 0123456789012345678901234567890123456789 unknown submodule
| |     Git-Toprepo-Ref: subz removed
| |
| * commit f7d99697dd97f0dd930355f1e30c305ad6ebb7bc
|/  Author: author <author@example.com>
|   Date:   Sat Jan 1 00:00:00 2000 +0000
|
|       sub-2
|
|       Git-Toprepo-Ref: subx c05fdf47f83a6cbdcc4aefc66d14095b2d4a2175
|
|   A subx/sub-2.txt
|
* commit 48891e2056ae7143446d5c48f9cb183bd898ca8a
| Author: author <author@example.com>
| Date:   Sat Jan 1 00:00:00 2000 +0000
|
|     Bad � encoding
|
|     Git-Toprepo-Ref: <top> 29f046ec8531189e9018cc6bf035fc68259872cf
|
*   commit b2e110d72dee0efe8501ab327a88fffd4596ad44
|\  Merge: c2d3274 1f37b7f
| | Author: author <author@example.com>
| | Date:   Sat Jan 1 00:00:00 2000 +0000
| |
| |     Regress x and missing commit y
| |
| |     End with some extra empty lines that are trimmed.
| |
| |     Git-Toprepo-Ref: <top> de365093f72af23b6be8965c8d2b7027c135f45b
| |
| |     x-1
| |
| |     Git-Toprepo-Ref: subx 55653d7a847a2d66486230ecca4b8d56ddb0bbc6
| |
| |     Git-Toprepo-Ref: suby 0123456789012345678901234567890123456789 not found
| |
| * commit 1f37b7fea9825b232941e13a93f3b9c6bc28db0f
|/  Author: author <author@example.com>
|   Date:   Sat Jan 1 00:00:00 2000 +0000
|
|       Resetting submodule subx to 55653d7a847a
|
|       The gitlinks of the parents to this commit references the commit:
|       - 044827c4cbf84f8d007c8ff08f777e28f1fd95f4
|       Regress the gitlink to the earlier commit
|       55653d7a847a2d66486230ecca4b8d56ddb0bbc6:
|
|       x-1
|
|   D subx/all-3.txt
|   D subx/sub-2.txt
|
* commit c2d3274623b1ec972bee479d4f0f67056148c450
| Author: author <author@example.com>
| Date:   Sat Jan 1 00:00:00 2000 +0000
|
|     all-3
|
|     Git-Toprepo-Ref: <top> 6fe934e3e03f3ed77f604c99bcb3222de9cdebd9
|     Git-Toprepo-Ref: subx 044827c4cbf84f8d007c8ff08f777e28f1fd95f4
|     Git-Toprepo-Ref: suby 1e5d12ddd8d2b9e8c160a471a8943f6015f389a2
|
| A all-3.txt
| A subx/all-3.txt
| A suby/all-3.txt
|
* commit 770ff5b791ab77de92d661f8822b503e682fb3d1
| Author: author <author@example.com>
| Date:   Sat Jan 1 00:00:00 2000 +0000
|
|     top-and-y-2
|
|     Git-Toprepo-Ref: <top> 283be0edfca9a9b31a6af22f780cf28a62b2cf0e
|     Git-Toprepo-Ref: suby 6b312c7ae87753d4d2ba7fed69831e373b30021e
|
|     sub-2
|
|     Git-Toprepo-Ref: subx c05fdf47f83a6cbdcc4aefc66d14095b2d4a2175
|
| A subx/sub-2.txt
| A suby/top-and-y-2.txt
| A top-and-y-2.txt
|
*-.   commit 672a2d19bec267347917871d73d8cdbdce42ea49
|\ \  Merge: 6f66116 55653d7 a789a5c
| | | Author: author <author@example.com>
| | | Date:   Sat Jan 1 00:00:00 2000 +0000
| | |
| | |     top-1
| | |
| | |     With: a footer
| | |     Git-Toprepo-Ref: <top> 7a47e8fe2bfe67be68d088345f190c8d8b279eb8
| | |
| | |     x-1
| | |
| | |     Git-Toprepo-Ref: subx 55653d7a847a2d66486230ecca4b8d56ddb0bbc6
| | |
| | |     y-1
| | |
| | |     Git-Toprepo-Ref: suby a789a5ca1e2cb59b9afc71a0c73fcedcc3bf6dd2
| | |
| | |     Git-Toprepo-Ref: subz 0011223344556677889900112233445566778899 (submodule)
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
"[1..];
    pretty_assertions::assert_str_eq!(log_graph, expected_graph);
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
    let subxrepo = temp_dir.join("subx");
    let subyrepo = temp_dir.join("suby");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("suby/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subx/file.txt", "suby/file.txt"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["commit", "-m"])
        .arg(
            "Add files

Body text

With: a footer
Git-Toprepo-Ref: <top>
Topic: my-topic
Git-Toprepo-Ref: suby anything-random

subx subject

Git-Toprepo-Ref: subx
Topic: remove-this-line
subx-footer: keep-this-line
",
        )
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN").not());

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
        "Add files\n\nBody text\n\nWith: a footer\n"
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
    let subxrepo = temp_dir.join("subx");
    let subyrepo = temp_dir.join("suby");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    std::fs::write(monorepo.join("top.txt"), "top\n").unwrap();
    std::fs::write(monorepo.join("subx/file.txt"), "subx\n").unwrap();
    std::fs::write(monorepo.join("suby/file.txt"), "suby\n").unwrap();
    git_command_for_testing(&monorepo)
        .args(["add", "top.txt", "subx/file.txt", "suby/file.txt"])
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
Git-Toprepo-Ref: subx
",
        )
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN").not());

    assert_eq!(git_commit_message(&toprepo, "other"), "Add files\n");
    assert_eq!(git_commit_message(&subxrepo, "other"), "subx subject\n");
    assert_eq!(git_commit_message(&subyrepo, "other"), "Add files\n");

    // The same, but where the toprepo is missing a message.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m"])
        .arg(
            "suby subject

Topic: my-topic
Git-Toprepo-Ref: suby

subx subject

Git-Toprepo-Ref: subx
Topic: my-topic
",
        )
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
                "^ERROR: No commit message found for path <top> in mono commit [0-9a-f]+\n$",
            )
            .unwrap(),
        );

    // The same, but with a residual message in the toprepo.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m"])
        .arg(
            "suby subject

Git-Toprepo-Ref: suby
Topic: my-topic

Residual message

Topic: other-topic
",
        )
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN").not());

    assert_eq!(git_commit_message(&toprepo, "other"), "Residual message\n");
    assert_eq!(git_commit_message(&subxrepo, "other"), "Residual message\n");
    assert_eq!(git_commit_message(&subyrepo, "other"), "suby subject\n");

    // No message assigned to specific paths.
    git_command_for_testing(&monorepo)
        .args(["commit", "--amend", "-m", "Subject\n\nTopic: my-topic"])
        .assert()
        .success();
    assert_cmd::Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .args(["push", "origin", "--force", "HEAD:refs/heads/other"])
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN").not());

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
