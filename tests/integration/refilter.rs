use assert_cmd::Command;
use assert_cmd::assert::OutputAssertExt as _;
use bstr::ByteSlice as _;
use git_toprepo::git::git_command_for_testing;
use itertools::Itertools as _;
use predicates::prelude::PredicateBooleanExt as _;
use rstest::rstest;
use std::path::Path;

#[test]
fn init_and_refilter_example() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_readme_example.sh")
            .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
*-.   N
|\ \
| | * 12
| | |
| | | A sub/12.txt
| | * 11
| |/
|/|
| |
| |   A sub/11.txt
| *   M
|/|\
| | * Resetting submodule sub to 2903c2551c19
| |/
| |
| |   D sub/11.txt
| |   D sub/12.txt
| * L
| |
| | A L.txt
| | A sub/12.txt
| * K
|/
|
|   A K.txt
|   A sub/11.txt
*   J
|\
| * Ib
| |
| | A Ib.txt
| | A sub/9b.txt
| * Hb
| |
| | A Hb.txt
| | A sub/8b.txt
* | Ia
| |
| | A Ia.txt
| | A sub/9a.txt
* | Ha
|/
|
|   A Ha.txt
|   A sub/8a.txt
*   G
|\
| * 6
|/
|
|   A sub/6.txt
* F
|
| A F.txt
*   E
|\
| * 4
|/
|
|   A sub/4.txt
* D
|
| A D.txt
| A sub/3.txt
* C
|
| A C.txt
*   B
|\
| * 2
| |
| | A 2.txt
| * 1
|
|   A 1.txt
* A

  A .gittoprepo.toml
  A A.txt
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

#[test]
fn refilter_merge_with_one_submodule_a() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_a.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
*-.   D6-release
|\ \
| | * x-release-5
| |/
|/|
| |
| |   A subx/x-release-5.txt
* | C4-release
| |
| | A C4-release.txt
| | A subx/x-release-4.txt
| * B3-main
|/|
| * x-main-2
|/
|
|   A subx/x-main-2.txt
*   A1-main
|\
| * x-main-1
|
|   A x-main-1.txt
* Initial empty commit
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

#[test]
fn refilter_merge_with_one_submodule_b() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_b.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
*-----.   F8-release
|\ \ \ \
| | | | * x-release-7
| | |_|/|
| |/| |/
| |_|/|
|/| | |
| * | | D6-release
| | | |
| | | | A D6-release.txt
| | | * x-main-4
| |_|/
|/| |
| | |
| | |   A subx/x-main-4.txt
* | |   B3-main
|\ \ \
| * | | x-main-2
|/ / /
| | |
| | |   A subx/x-main-2.txt
| | * E6-release
| |/
| |
| |   A E6-release.txt
| * C6-release
|/|
| * x-release-5
|/
|
|   A subx/x-release-5.txt
*   A1-main
|\
| * x-main-1
|
|   A x-main-1.txt
* Initial empty commit
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

#[test]
fn refilter_merge_with_two_submodules() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
*---.   D6-release
|\ \ \
| | | * y-release-5
| |_|/
|/| |
| | |
| | |   A suby/y-release-5.txt
| | * x-release-5
| |/
|/|
| |
| |   A subx/x-release-5.txt
* | C4-release
| |
| | A C4-release.txt
| | A subx/x-release-4.txt
| | A suby/y-release-4.txt
| *   B3-main
|/|\
| | * y-main-2
| |/
|/|
| |
| |   A suby/y-main-2.txt
| * x-main-2
|/
|
|   A subx/x-main-2.txt
*-.   A1-main
|\ \
| | * y-main-1
| |
| |   A y-main-1.txt
| * x-main-1
|
|   A x-main-1.txt
* Initial empty commit
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

#[test]
fn refilter_submodule_removal() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_submodule_removal.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
*   E
|\
| * C
| |
| | M .gitmodules
| | R100 subx/1.txt C.txt
| | D subx/2.txt
| * B
| |
| | A B.txt
| | A subx/2.txt
* | D
|/
|
|   M .gitmodules
|   R100 subx/1.txt D.txt
*   A
|\
| * 1
|
|   A 1.txt
* Initial empty commit
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

#[test]
fn refilter_moved_submodule() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_moved_submodule.sh")
            .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    println!("{log_graph}");
    let expected_graph = r"
* E
|
| M .gitmodules
| R100 suby/1.txt E.txt
| R100 suby/2.txt subx/1.txt
| R100 suby/3.txt subx/2.txt
| A subx/3.txt
| A subz/1.txt
| A subz/2.txt
| A subz/3.txt
* D
|
| M .gitmodules
| R100 subx/1.txt D.txt
| D subx/2.txt
* C
|
| M .gitmodules
| A C.txt
| A subx/1.txt
| A subx/2.txt
| A suby/3.txt
* B
|
| M .gitmodules
| R100 subx/1.txt B.txt
| A suby/1.txt
| A suby/2.txt
*   A
|\
| * 1
|
|   A 1.txt
* Initial empty commit
"
    .strip_prefix("\n")
    .unwrap();
    assert_eq!(log_graph, expected_graph);
}

fn extract_log_graph(repo_path: &Path, extra_args: Vec<&str>) -> String {
    let log_command = git_command_for_testing(repo_path)
        .args(["log", "--graph", "--format=%s"])
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

#[rstest]
#[case::duplicated_key(&r#"
[submodule "subx"]
	path = subx
	url = ../subx/
# Duplicate the entry, same key.
# https://github.com/meroton/git-toprepo/issues/31
[submodule "subx"]
	path = suby
	url = ../suby/
"#[1..], r#"Missing path "subx" in .gitmodules\n"#)]
#[case::bad_syntax(&r#"
[submodule "subx"
"#[1..], "Failed to parse .gitmodules: Got an unexpected token on line 1 while trying to parse a section header: ")]
fn copes_with_bad_dot_gitmodules_content(
    #[case] gitmodules_content: &str,
    #[case] expected_warning: &str,
) {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");

    std::fs::write(toprepo.join(".gitmodules"), gitmodules_content).unwrap();
    git_command_for_testing(&toprepo)
        .args(["add", ".gitmodules"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Bad .gitmodules"])
        .assert()
        .success();

    let monorepo = temp_dir.join("mono");
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(
            predicates::str::is_match(format!(
                r#"\nWARN: Commit [0-9a-f]+ in top \(refs/[^)]+\): {expected_warning}"#
            ))
            .unwrap(),
        )
        .stderr(predicates::function::function(|stderr: &str| {
            stderr.matches("WARN:").count() == 1
        }));
}

/// git-submodule creates an empty directory for each submodule. In case a
/// submodule is substituted for an empty tree with git-toprepo, that directory
/// will disappear. User tooling might depend on the existance of that empty
/// directory, e.g. for each submodule mentioned in .gitmodules.
///
/// Verify that git-toprepo prints a warning to make the user aware of the
/// problem, so that a dummy file can be added to the submodule as a fix.
#[test]
fn warn_for_empty_submodule() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");
    let subxrepo = temp_dir.join("subx");
    let monorepo = temp_dir.join("mono");
    let monorepo2 = temp_dir.join("mono2");

    git_command_for_testing(&subxrepo)
        .args(["rm", "-rf", "."])
        .assert()
        .success();
    git_command_for_testing(&subxrepo)
        .args(["commit", "-m", "Remove all files"])
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(    predicates::str::is_match(r"\nWARN: Commit [0-9a-f]+ in subx \(refs/heads/main\): With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\. To avoid this problem, commit a file\.\n").unwrap());

    // Fix the warning.
    std::fs::write(subxrepo.join("file.txt"), "content\n").unwrap();
    git_command_for_testing(&subxrepo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    git_command_for_testing(&subxrepo)
        .args(["commit", "-m", "add a file"])
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .args(["clone", "-vv"])
        .arg(&toprepo)
        .arg(&monorepo2)
        .assert()
        .success()
        // Note that the trace message does not include any branch name.
        .stderr(    predicates::str::is_match(r"\nTRACE: Commit [0-9a-f]+ in subx: With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\. To avoid this problem, commit a file\.\n").unwrap())
        .stderr(predicates::str::contains("WARN:").not());
}

#[test]
fn refilter_prints_updates() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stdout(
            " * [new] e1f32c7      -> origin/HEAD
 * [new] e1f32c7      -> origin/main
",
        );
    git_command_for_testing(&monorepo)
        .args([
            "symbolic-ref",
            "refs/namespaces/top/refs/symbolic/good",
            "refs/namespaces/top/refs/heads/main",
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "symbolic-ref",
            "refs/namespaces/top/refs/symbolic/outside-top",
            "refs/heads/main",
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "-d",
            "refs/namespaces/top/refs/remotes/origin/main",
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "refs/namespaces/top/refs/remotes/origin/other",
            "d849346",
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "refs/namespaces/top/refs/tags/v1.0",
            "d849346",
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "refs/namespaces/top/refs/tags/v2.0",
            "d849346",
        ])
        .assert()
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("refilter")
        .arg("-v")
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN: Skipping symbolic ref refs/namespaces/top/refs/symbolic/outside-top that points outside the top repo, to refs/heads/main"))
        .stderr(predicates::function::function(|s: &str| s.matches("WARN:").count() == 1))
        .stdout(&"
 * [new] e1f32c7              -> origin/other
 * [new] link:refs/heads/main -> refs/symbolic/good
 * [new tag] e1f32c7          -> v1.0
 * [new tag] e1f32c7          -> v2.0
 = [up to date] e1f32c7       -> origin/HEAD
 - [deleted] e1f32c7          -> origin/main
"[1..]);

    // Symbolic refs are never pruned, so delete it manually.
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "-d",
            "--no-deref",
            "refs/namespaces/top/refs/symbolic/outside-top",
        ])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "--allow-empty", "-m", "Empty commit"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["branch", "other", "HEAD"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["reset", "HEAD~"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args([
            "commit",
            "--amend",
            "--allow-empty",
            "-m",
            "Different message",
        ])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["tag", "-m", "Version 1.0", "v1.0", "HEAD"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        // If this tag would not be nested, it would get the same hash as v1.0.
        .args(["tag", "-m", "Version 1.0", "v1.0-nested", "v1.0"])
        .assert()
        .stderr(predicates::str::contains("You have created a nested tag."))
        .success();
    Command::cargo_bin("git-toprepo")
        .unwrap()
        .current_dir(&monorepo)
        .arg("fetch")
        .assert()
        .success()
        .stderr(predicates::str::contains("WARN:").not())
        // This output has triggered most paths. Note that the symbolic links
        // are not possible to fetch, only to add manually to
        // `refs/namespaces/top/...` and refilter.
        .stdout(
            &"
 * [new] ce017aa                     -> origin/main
 * [new tag] 2998233                 -> v1.0-nested
 + [forced update] e1f32c7...ce017aa -> origin/HEAD
   e1f32c7..13e5daa                  -> origin/other
 t [updated tag] e1f32c7..adc9359    -> v1.0
 - [deleted tag] e1f32c7             -> v2.0
"[1..],
        );
}
