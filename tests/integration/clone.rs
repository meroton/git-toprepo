use anyhow::Context as _;
use bstr::ByteSlice as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use git_toprepo_testtools::test_util::git_rev_parse;
use predicates::prelude::*;

#[test]
fn toprepo_clone() {
    let base_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let from_path = &base_dir.path().join("from");
    std::fs::create_dir(from_path).unwrap();
    let to_path = &base_dir.path().join("to");
    std::fs::create_dir(to_path).unwrap();

    git_command_for_testing(from_path)
        .args(["init", "--quiet", "--initial-branch", "main"])
        .assert()
        .success();
    git_command_for_testing(from_path)
        .args(["commit", "--allow-empty", "--quiet"])
        .args(["-m", "Initial commit"])
        .assert()
        .success();
    std::fs::write(from_path.join(".gittoprepo.toml"), "").unwrap();
    git_command_for_testing(from_path)
        .args(["add", ".gittoprepo.toml"])
        .assert()
        .success();
    git_command_for_testing(from_path)
        .args(["commit", "--quiet"])
        .args(["-m", "Config file"])
        .assert()
        .success();
    git_command_for_testing(from_path)
        .args(["tag", "mytag"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
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
        let orig_rev = git_command_for_testing(from_path)
            .args(["rev-parse", "--verify", orig_ref])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let top_rev = git_command_for_testing(to_gix_repo.git_dir())
            .args(["rev-parse", "--verify", top_ref])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            orig_rev.to_str().unwrap(),
            top_rev.to_str().unwrap(),
            "ref {orig_ref} mismatch",
        );
    }
}

#[ignore]
#[test]
fn clone_and_bootstrap() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    git_command_for_testing(&toprepo)
        .args(["rm", ".gittoprepo.toml"])
        .assert()
        .success();
    git_command_for_testing(&toprepo)
        .args(["commit", "-m", "Remove .gittoprepo.toml"])
        .assert()
        .success();

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::ends_with(
            "WARN: Config file .gittoprepo.toml does not exist in refs/namespaces/top/refs/remotes/origin/HEAD: \
            exit status: 128: fatal: path '.gittoprepo.toml' does not exist in 'refs/namespaces/top/refs/remotes/origin/HEAD'\n\
            ERROR: Config file .gittoprepo.toml does not exist in should:repo:refs/namespaces/top/refs/remotes/origin/HEAD:.gittoprepo.toml\n\
            INFO: Please run 'git-toprepo config bootstrap > .gittoprepo.user.toml' to generate an initial config and \
            'git-toprepo recombine' to use it.\n\
            ERROR: Clone failed due to missing config file\n"
        ));
    let config_path = monorepo.join(".gittoprepo.user.toml");
    assert!(!config_path.exists());
    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .args(["config", "bootstrap"])
        .assert()
        .success()
        .stderr(
            predicate::str::is_match("^INFO: Finished importing commits in [^\n]*\n$").unwrap(),
        );
    let bootstrap_config = &cmd.get_output().stdout;
    insta::assert_snapshot!(bootstrap_config.to_str().unwrap(), @r#"
    [repo.repox]
    urls = ["../repox/"]
    missing_commits = []

    [repo.repoy]
    urls = ["../repoy/"]
    missing_commits = []
    "#);
    std::fs::write(config_path, bootstrap_config).unwrap();

    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&monorepo)
        .arg("recombine")
        .assert()
        .success();

    insta::assert_snapshot!(
        cmd.get_output().stdout.to_str().unwrap(),
        @r"
    * [new] 787fec1      -> origin/HEAD
    * [new] 787fec1      -> origin/main
    "
    );
}

#[test]
fn double_clone_should_fail() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .code(1)
        .stderr(predicate::eq(format!(
            "ERROR: Target directory {monorepo:?} is not empty\n"
        )));

    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg("--force")
        .arg(&toprepo)
        .arg(&monorepo)
        .assert()
        .success();
}

#[test]
fn checkout_should_not_add_branch_tracking() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");

    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    assert_eq!(
        git_rev_parse(&monorepo, "refs/namespaces/top/refs/remotes/origin/main"),
        git_rev_parse(&toprepo, "refs/heads/main")
    );

    git_command_for_testing(&monorepo)
        .args(["checkout", "main"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("error: pathspec 'main' did not match any file(s) known to git\n");

    git_command_for_testing(&monorepo)
        .args(["checkout", "-b", "main", "origin/main"])
        .assert()
        .success();
    assert_ne!(
        git_rev_parse(&monorepo, "refs/heads/main"),
        git_rev_parse(&toprepo, "refs/heads/main")
    );
    assert_eq!(
        git_rev_parse(&monorepo, "refs/heads/main"),
        git_rev_parse(&monorepo, "refs/remotes/origin/main")
    );
    git_command_for_testing(&monorepo)
        .args(["config", "--list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch.main.").not());
    git_command_for_testing(&monorepo)
        .args(["pull"])
        .assert()
        .code(1);

    // Make sure it is due to our config and not by luck.
    let monorepo = temp_dir.join("mono2");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);
    git_command_for_testing(&monorepo)
        .args(["config", "--unset-all", "checkout.guess"])
        .assert()
        .success();
    git_command_for_testing(&monorepo)
        .args(["checkout", "main"])
        .assert()
        .success();
    // monorepo:main = toprepo:main is NOT wanted.
    assert_ne!(
        git_rev_parse(&monorepo, "refs/heads/main"),
        git_rev_parse(&monorepo, "refs/remotes/origin/main")
    );
}
