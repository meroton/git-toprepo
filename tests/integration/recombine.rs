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
    insta::assert_snapshot!(
        log_graph,
        @r"
    *-.   N
    |\ \
    | | * 12
    | | |
    | | | A subpath/12.txt
    | | * 11
    | |/
    |/|
    | |
    | |   A subpath/11.txt
    | *   M
    |/|\
    | | * Resetting submodule subpath to 2903c2551c19
    | |/
    | |
    | |   D subpath/11.txt
    | |   D subpath/12.txt
    | * L
    | |
    | | A L.txt
    | | A subpath/12.txt
    | * K
    |/
    |
    |   A K.txt
    |   A subpath/11.txt
    *   J
    |\
    | * Ib
    | |
    | | A Ib.txt
    | | A subpath/9b.txt
    | * Hb
    | |
    | | A Hb.txt
    | | A subpath/8b.txt
    * | Ia
    | |
    | | A Ia.txt
    | | A subpath/9a.txt
    * | Ha
    |/
    |
    |   A Ha.txt
    |   A subpath/8a.txt
    *   G
    |\
    | * 6
    |/
    |
    |   A subpath/6.txt
    * F
    |
    | A F.txt
    *   E
    |\
    | * 4
    |/
    |
    |   A subpath/4.txt
    * D
    |
    | A D.txt
    | A subpath/3.txt
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
    );
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
    insta::assert_snapshot!(
        log_graph,
        @r"
    *-.   D6-release
    |\ \
    | | * x-release-5
    | |/
    |/|
    | |
    | |   A subpathx/x-release-5.txt
    * | C4-release
    | |
    | | A C4-release.txt
    | | A subpathx/x-release-4.txt
    | * B3-main
    |/|
    | * x-main-2
    |/
    |
    |   A subpathx/x-main-2.txt
    *   A1-main
    |\
    | * x-main-1
    |
    |   A x-main-1.txt
    * Initial empty commit
    "
    );
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
    #[rustfmt::skip] // Infinite indentation bug, rustfmt issue 4609.
    insta::assert_snapshot!(
        log_graph,
        @r"
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
    | | |   A subpathx/x-main-4.txt
    * | |   B3-main
    |\ \ \
    | * | | x-main-2
    |/ / /
    | | |
    | | |   A subpathx/x-main-2.txt
    | | * E6-release
    | |/
    | |
    | |   A E6-release.txt
    | * C6-release
    |/|
    | * x-release-5
    |/
    |
    |   A subpathx/x-release-5.txt
    *   A1-main
    |\
    | * x-main-1
    |
    |   A x-main-1.txt
    * Initial empty commit
    "
    );
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
    #[rustfmt::skip] // Infinite indentation bug, rustfmt issue 4609.
    insta::assert_snapshot!(
        log_graph,
        @r"
    *---.   D6-release
    |\ \ \
    | | | * y-release-5
    | |_|/
    |/| |
    | | |
    | | |   A subpathy/y-release-5.txt
    | | * x-release-5
    | |/
    |/|
    | |
    | |   A subpathx/x-release-5.txt
    * | C4-release
    | |
    | | A C4-release.txt
    | | A subpathx/x-release-4.txt
    | | A subpathy/y-release-4.txt
    | *   B3-main
    |/|\
    | | * y-main-2
    | |/
    |/|
    | |
    | |   A subpathy/y-main-2.txt
    | * x-main-2
    |/
    |
    |   A subpathx/x-main-2.txt
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
    );
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
    #[rustfmt::skip] // Infinite indentation bug, rustfmt issue 4609.
    insta::assert_snapshot!(
        log_graph,
        @r"
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
    "
    );
}

/// Testing a regression from 2025-10-22.
#[test]
fn regression_20251023() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_expander_regression_20251023.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    let log_graph = extract_log_graph(&monorepo, vec!["HEAD", "--"]);
    insta::assert_snapshot!(
        log_graph,
        @r"
    *-.   F-X7
    |\ \
    | | * x6
    | * | x5
    |/ /
    * | E-X4
    * | D-X4
    * | C-X3
    |/
    *   B-X2
    |\
    | * x2
    | * x1
    * A
    "
    );
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
    #[rustfmt::skip] // Infinite indentation bug, rustfmt issue 4609.
    insta::assert_snapshot!(
        log_graph,
        @r"
    *   E
    |\
    | * C
    | |
    | | M .gitmodules
    | | R100 subpathx/1.txt C.txt
    | | D subpathx/2.txt
    | * B
    | |
    | | A B.txt
    | | A subpathx/2.txt
    * | D
    |/
    |
    |   M .gitmodules
    |   R100 subpathx/1.txt D.txt
    *   A
    |\
    | * 1
    |
    |   A 1.txt
    * Initial empty commit
    "
    );
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
    insta::assert_snapshot!(
        log_graph,
        @r"
    * E
    |
    | M .gitmodules
    | R100 subpathy/1.txt E.txt
    | R100 subpathy/2.txt subpathx/1.txt
    | R100 subpathy/3.txt subpathx/2.txt
    | A subpathx/3.txt
    | A subpathz/1.txt
    | A subpathz/2.txt
    | A subpathz/3.txt
    * D
    |
    | M .gitmodules
    | R100 subpathx/1.txt D.txt
    | D subpathx/2.txt
    * C
    |
    | M .gitmodules
    | A C.txt
    | A subpathx/1.txt
    | A subpathx/2.txt
    | A subpathy/3.txt
    * B
    |
    | M .gitmodules
    | R100 subpathx/1.txt B.txt
    | A subpathy/1.txt
    | A subpathy/2.txt
    *   A
    |\
    | * 1
    |
    |   A 1.txt
    * Initial empty commit
    "
    );
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
    insta::assert_snapshot!(
        log_graph,
        @r"
    * C-X3-Y2
    |
    | A C-X3-Y2.txt
    | A subpathx/x3-y2.txt
    * B-X2-Y1
    |
    | A B-X2-Y1.txt
    | A subpathx/.gitmodules
    | A subpathx/subpathy/y-1.txt
    | A subpathx/subpathy/y-2.txt
    | A subpathx/x2-y2.txt
    *   A1-X1
    |\
    | * x-1
    |
    |   A x-1.txt
    * init

      A .gittoprepo.toml
      A init.txt
    "
    );
    let ls_tree_command = git_command_for_testing(monorepo)
        .args(["ls-files"])
        .assert()
        .success();
    let ls_tree_stdout = ls_tree_command.get_output().stdout.to_str().unwrap();
    insta::assert_snapshot!(
        ls_tree_stdout,
        @r"
    .gitmodules
    .gittoprepo.toml
    A1-X1.txt
    B-X2-Y1.txt
    C-X3-Y2.txt
    init.txt
    subpathx/.gitmodules
    subpathx/subpathy/y-1.txt
    subpathx/subpathy/y-2.txt
    subpathx/x-1.txt
    subpathx/x2-y2.txt
    subpathx/x3-y2.txt
    "
    );
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
[submodule "same_name"]
	path = subpathx
	url = ../repox/
# Duplicate the entry, same key.
# https://github.com/meroton/git-toprepo/issues/31
[submodule "same_name"]
	path = subpathy
	url = ../repoy/
"#[1..], r#"Missing path "subpathx" in .gitmodules\n"#)]
#[case::bad_syntax(&r#"
[submodule "same_name"
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
                r#"\nWARN: Top commit [0-9a-f]+ \(refs/remotes/origin/(HEAD|main)\): {expected_warning}"#
            ))
            .unwrap(),
        )
        .stderr(predicate::function(|stderr: &str| {
            // The warning exists for both refs/remotes/origin/{HEAD,main}.
            stderr.matches("WARN:").count() == 2
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
    let subxrepo = temp_dir.join("repox");
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
    let subx_head_rev = git_rev_parse(&subxrepo, "HEAD");
    git_update_submodule_in_index(&toprepo, "subpathx", &subx_head_rev);
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Remove all files in subx"])
        .assert()
        .success();
    let msg_pattern = |gitref: &str| {
        format!(
            "\\nWARN: Top commit [0-9a-f]+ \\({gitref}\\): \
            Submodule commit [0-9a-f]+ at subpathx \\(namex\\): \
            With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\\. \
            To avoid this problem, commit a file\\.\\n"
        )
    };
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .stderr(predicate::str::is_match(msg_pattern("refs/remotes/origin/HEAD")).unwrap())
        .stderr(predicate::str::is_match(msg_pattern("refs/remotes/origin/main")).unwrap())
        .stderr(predicate::function(|stderr: &str| {
            // The warning exists for both refs/remotes/origin/{HEAD,main}.
            stderr.matches("WARN:").count() == 2
        }));

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
    let subx_head_rev = git_rev_parse(&subxrepo, "HEAD");
    git_update_submodule_in_index(&toprepo, "subpathx", &subx_head_rev);
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Add a file in subx"])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .args(["clone", "-vv"])
        .arg(&toprepo)
        .arg(&monorepo2)
        .assert()
        .success()
        // Note that the trace message does not include any branch name.
        .stderr(predicate::str::is_match(
            "\\nTRACE: Commit [0-9a-f]+ in namex: \
             With git-submodule, this empty commit results in a directory that is empty, but with git-toprepo it will disappear\\. \
             To avoid this problem, commit a file\\.\\n"
        ).unwrap())
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
    let subyrepo = temp_dir.join("repoy");
    let monorepo = temp_dir.join("mono");
    let monorepo2 = temp_dir.join("mono2");

    let suby_missing_rev = git_rev_parse(&subyrepo, "HEAD");

    std::fs::write(
        toprepo.join(".gittoprepo.toml"),
        &format!(
            r#"
[repo.namex]
urls = ["../repox/"]
missing_commits = [
    # Non-existing commit.
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
]
[repo.namey]
urls = ["../repoy/"]
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
        WARN: Commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in namex is \
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
    git_update_submodule_in_index(
        &toprepo,
        "subpathx",
        "cccccccccccccccccccccccccccccccccccccccc",
    );
    // Add a commit to suby so that it is loaded.
    git_command_for_testing(&subyrepo)
        .args(["commit", "--allow-empty", "-m", "Second empty commit"])
        .assert()
        .success();
    git_update_submodule_in_index(&toprepo, "subpathy", &git_rev_parse(&subyrepo, "HEAD"));
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Update subx and suby"])
        .assert()
        .success();
    let some_expected_warnings = format!(
        "WARN: Commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in namex is \
        configured as missing but was never referenced from any repo\n\
        WARN: Commit {suby_missing_rev} in namey exists \
        but is configured as missing\n\
        WARN: Commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb in namey is \
        configured as missing but was never referenced from any repo\n\
        ",
    );
    let missing_commit_subx_c_warning = "\nWARN: Commit cccccccccccccccccccccccccccccccccccccccc in namex is missing, referenced from top\n";
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

    let top_head_rev = &git_rev_parse(&toprepo, "HEAD")[..7];
    insta::assert_snapshot!(
        top_head_rev,
        @"e1644da",
    );

    let stdout = cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .to_owned();

    let mono_head_rev = &git_rev_parse(&monorepo, "HEAD")[..7];
    insta::assert_snapshot!(
        mono_head_rev,
        @"db59d86",
        // All commit hashes will differ if mono_head_rev is wrong
    );

    assert_eq!(
        stdout,
        // TODO: Please find a way to dedent this and make it look better.
        format!(
            " * [new] {mono_head_rev}      -> origin/HEAD
 * [new] {mono_head_rev}      -> origin/main
"
        ),
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
    let stdout = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("recombine")
        .arg("-v")
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN: Skipping symbolic ref refs/namespaces/top/refs/symbolic/outside-top that points outside the top repo, to refs/heads/main"))
        .stderr(predicate::function(|s: &str| s.matches("WARN:").count() == 1))
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .to_owned();

    insta::assert_snapshot!(
        stdout,
        @r"
    * [new] db59d86              -> origin/other
    * [new] link:refs/heads/main -> refs/symbolic/good
    * [new tag] db59d86          -> v1.0
    * [new tag] db59d86          -> v2.0
    = [up to date] db59d86       -> origin/HEAD
    - [deleted] db59d86          -> origin/main
    ");

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
    let fetch_stdout = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("fetch")
        .assert()
        .success()
        .stderr(predicate::str::contains("WARN:").not())
        // This output has triggered most paths. Note that the symbolic links
        // are not possible to fetch, only to add manually to
        // `refs/namespaces/top/...` and recombine.
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .to_owned();
    insta::assert_snapshot!(
        fetch_stdout,
        @r"
    * [new] a6afcc9                     -> origin/main
    * [new tag] 9f086bb                 -> v1.0-nested
    + [forced update] db59d86...a6afcc9 -> origin/HEAD
      db59d86..77dbca5                  -> origin/other
    t [updated tag] db59d86..d51577c    -> v1.0
    - [deleted tag] db59d86             -> v2.0
    ",
    );
}
