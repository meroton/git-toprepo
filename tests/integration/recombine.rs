use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::git_rev_parse;
use git_toprepo_testtools::test_util::git_update_submodule_in_index;
use itertools::Itertools as _;
use predicates::prelude::*;
use rstest::rstest;
use std::path::Path;

#[test]
fn init_and_recombine_example() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_readme_example.sh")
            .unwrap(),
    );
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
fn merge_with_one_submodule_a() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_a.sh",
        )
        .unwrap(),
    );
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
fn merge_with_one_submodule_b() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_one_submodule_b.sh",
        )
        .unwrap(),
    );
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
fn merge_with_two_submodules() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_merge_with_two_submodules.sh",
        )
        .unwrap(),
    );
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

/// Testing a regression from 2025-10-22.
#[test]
fn regression_20251022() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_expander_regression_20251022.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["HEAD", "--"]);
    insta::assert_snapshot!(log_graph, @r"
    *   H11
    |\
    | * 10
    |/|
    | * 7
    * |   G9
    |\ \
    | * | 4
    * | | F8
    | |/
    |/|
    * |   E6
    |\ \
    | |/
    |/|
    | *   D5
    | |\
    | | * 3
    | |/
    * / C3
    |/
    *   B2
    |\
    | * 2
    | * 1
    * A
    ");
}

#[test]
fn submodule_removal() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_submodule_removal.sh",
        )
        .unwrap(),
    );
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
fn moved_submodule() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_moved_submodule.sh")
            .unwrap(),
    );
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

#[test]
fn inner_submodule() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_inner_submodule.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["--name-status", "HEAD", "--"]);
    insta::assert_snapshot!(log_graph, @r"
    * C-X3-Y2
    |
    | A C-X3-Y2.txt
    | A subx/x3-y2.txt
    * B-X2-Y1
    |
    | A B-X2-Y1.txt
    | A subx/.gitmodules
    | A subx/suby/y-1.txt
    | A subx/suby/y-2.txt
    | A subx/x2-y2.txt
    *   A1-X1
    |\
    | * x-1
    |
    |   A x-1.txt
    * init

      A .gittoprepo.toml
      A init.txt
    ");
    let ls_tree_command = git_command_for_testing(monorepo)
        .args(["ls-files"])
        .assert()
        .success();
    let ls_tree_stdout = ls_tree_command.get_output().stdout.to_str().unwrap();
    insta::assert_snapshot!(ls_tree_stdout, @r"
    .gitmodules
    .gittoprepo.toml
    A1-X1.txt
    B-X2-Y1.txt
    C-X3-Y2.txt
    init.txt
    subx/.gitmodules
    subx/suby/y-1.txt
    subx/suby/y-2.txt
    subx/x-1.txt
    subx/x2-y2.txt
    subx/x3-y2.txt
    ");
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
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(
            predicate::str::is_match(format!(
                r#"\nWARN: Commit [0-9a-f]+ in top \(refs/[^)]+\): {expected_warning}"#
            ))
            .unwrap(),
        )
        .stderr(predicate::function(|stderr: &str| {
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
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::is_match(
            "\\nWARN: Commit [0-9a-f]+ in subx \\(refs/heads/main\\): \
            With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\\. \
            To avoid this problem, commit a file\\.\\n").unwrap(),
        );

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
    cargo_bin_git_toprepo_for_testing()
        .args(["clone", "-vv"])
        .arg(&toprepo)
        .arg(&monorepo2)
        .assert()
        .success()
        // Note that the trace message does not include any branch name.
        .stderr(    predicate::str::is_match(r"\nTRACE: Commit [0-9a-f]+ in subx: With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\. To avoid this problem, commit a file\.\n").unwrap())
        .stderr(predicate::str::contains("WARN:").not());
}

/// Check that warnings are printed for improper use of the `missing_commits`
/// option.
#[test]
fn config_missing_commits() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let subyrepo = temp_dir.join("suby");
    let monorepo = temp_dir.join("mono");
    let monorepo2 = temp_dir.join("mono2");

    let suby_missing_rev = git_rev_parse(&subyrepo, "HEAD");

    std::fs::write(
        toprepo.join(".gittoprepo.toml"),
        &format!(
            r#"
[repo.subx]
urls = ["../subx/"]
missing_commits = [
    # Non-existing commit.
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
]
[repo.suby]
urls = ["../suby/"]
missing_commits = [
    # Commit that exists.
    "{suby_missing_rev}",
    # Non-existing commit.
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
]
"#
        )[1..],
    )
    .unwrap();
    git_command_for_testing(&toprepo)
        .args(["commit", "--all", "-m", "Update .gittoprepo.toml"])
        .assert()
        .success();
    let expected_warnings = "\
        WARN: Commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in subx is \
        configured as missing but was never referenced from any repo\n";
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        // suby should not be loaded at all, because the only referenced commit
        // is marked as missing.
        .stderr(predicate::str::contains(expected_warnings))
        .stderr(
            predicate::function(|stderr: &str| stderr.matches("WARN:").count() == 1)
                .name("exactly 1 warning"),
        );
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["recombine", "--use-cache"])
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains(expected_warnings))
        .stderr(
            predicate::function(|stderr: &str| stderr.matches("WARN:").count() == 1)
                .name("exactly 1 warning"),
        );

    // Update subx to somthing that does not exist.
    git_update_submodule_in_index(&toprepo, "subx", "cccccccccccccccccccccccccccccccccccccccc");
    // Add a commit to suby so that it is loaded.
    git_command_for_testing(&subyrepo)
        .args(["commit", "--allow-empty", "-m", "Second empty commit"])
        .assert()
        .success();
    git_update_submodule_in_index(&toprepo, "suby", &git_rev_parse(&subyrepo, "HEAD"));
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Update subx and suby"])
        .assert()
        .success();
    let some_expected_warnings = format!(
        "WARN: Commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in subx is \
        configured as missing but was never referenced from any repo\n\
        WARN: Commit {suby_missing_rev} in suby exists \
        but is configured as missing\n\
        WARN: Commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb in suby is \
        configured as missing but was never referenced from any repo\n\
        ",
    );
    let missing_commit_subx_c_warning = "\nWARN: Commit cccccccccccccccccccccccccccccccccccccccc in subx is missing, referenced from top\n";
    cargo_bin_git_toprepo_for_testing()
        .args(["clone"])
        .arg(&toprepo)
        .arg(&monorepo2)
        .assert()
        .success()
        .stderr(predicate::str::contains(missing_commit_subx_c_warning))
        .stderr(predicate::str::contains(&some_expected_warnings))
        .stderr(
            predicate::function(|stderr: &str| stderr.matches("WARN:").count() == 4)
                .name("exactly 4 warnings"),
        );
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo2)
        .args(["recombine", "--use-cache"])
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains(missing_commit_subx_c_warning))
        .stderr(predicate::str::contains(&some_expected_warnings))
        .stderr(
            predicate::function(|stderr: &str| stderr.matches("WARN:").count() == 4)
                .name("exactly 4 warnings"),
        );
}

#[test]
fn print_updates() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    let top_head_rev = "9c6fda5";
    let mono_head_rev = "9ddc65e";

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stdout(format!(
            " * [new] {mono_head_rev}      -> origin/HEAD
 * [new] {mono_head_rev}      -> origin/main
"
        ));
    git_command_for_testing(&toprepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(top_head_rev));
    git_command_for_testing(&monorepo)
        .args(["rev-parse", "HEAD"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(mono_head_rev));

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
            top_head_rev,
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "refs/namespaces/top/refs/tags/v1.0",
            top_head_rev,
        ])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args([
            "update-ref",
            "refs/namespaces/top/refs/tags/v2.0",
            top_head_rev,
        ])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("recombine")
        .arg("-v")
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN: Skipping symbolic ref refs/namespaces/top/refs/symbolic/outside-top that points outside the top repo, to refs/heads/main"))
        .stderr(predicate::function(|s: &str| s.matches("WARN:").count() == 1))
        .stdout(format!(" * [new] {mono_head_rev}              -> origin/other
 * [new] link:refs/heads/main -> refs/symbolic/good
 * [new tag] {mono_head_rev}          -> v1.0
 * [new tag] {mono_head_rev}          -> v2.0
 = [up to date] {mono_head_rev}       -> origin/HEAD
 - [deleted] {mono_head_rev}          -> origin/main
"));

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
        .stderr(predicate::str::contains("You have created a nested tag."))
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("fetch")
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN:").not())
        // This output has triggered most paths. Note that the symbolic links
        // are not possible to fetch, only to add manually to
        // `refs/namespaces/top/...` and recombine.
        .stdout(
            &"
 * [new] cfa5366                     -> origin/main
 * [new tag] 8bd46b9                 -> v1.0-nested
 + [forced update] 9ddc65e...cfa5366 -> origin/HEAD
   9ddc65e..fbfac05                  -> origin/other
 t [updated tag] 9ddc65e..5feaf42    -> v1.0
 - [deleted tag] 9ddc65e             -> v2.0
"[1..],
        );
}
