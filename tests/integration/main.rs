mod fixtures;

#[cfg(test)]
mod clone;
#[cfg(test)]
mod commit_message;
#[cfg(test)]
mod config;
#[cfg(test)]
mod dump;
#[cfg(test)]
mod fetch;
#[cfg(test)]
mod info;
#[cfg(test)]
mod init;
#[cfg(test)]
mod log;
#[cfg(test)]
mod push;
#[cfg(test)]
mod recombine;
#[cfg(test)]
mod version;
#[cfg(test)]
mod worktree;

#[cfg(test)]
mod main {
    use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
    use git_toprepo_testtools::test_util::git_command_for_testing;
    use predicates::prelude::*;
    use rstest::rstest;

    #[rstest]
    #[case::default(".", &[])]
    #[case::pwd_sub("repo", &[])]
    #[case::c_sub(".", &["-C", "repo"])]
    #[case::pwd_sub_c_dotdot("repo", &["-C", ".."])]
    fn commands_in_uninitialized_repo_fails(
        #[case] pwd_sub_dir: &str,
        #[case] dash_c: &[&str],
        #[values(
            "dump import-cache",
            "config show",
            "fetch origin",
            "recombine",
            "push origin main"
        )]
        command: &str,
    ) {
        use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;

        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
        std::fs::create_dir(temp_dir.join("repo")).unwrap();
        let expected_stderr = "ERROR: git-config \'toprepo.config\' is missing. Is this an initialized git-toprepo?\n";

        git_command_for_testing(&temp_dir)
            .args(["init", "--quiet"])
            .assert()
            .success();
        std::fs::create_dir_all(temp_dir.join("repo")).unwrap();

        cargo_bin_git_toprepo_for_testing()
            .current_dir(temp_dir.join(pwd_sub_dir))
            .args(dash_c)
            .args(command.split(' '))
            .assert()
            .failure()
            .stderr(expected_stderr);
    }

    #[rstest]
    #[case::default(".", &[], "")]
    #[case::pwd_sub("repo", &[], "repo")]
    #[case::c_sub(".", &["-C", "repo"], "repo")]
    #[case::pwd_sub_c_dotdot("repo", &["-C", ".."], "")]
    fn commands_outside_git_repos_fail(
        #[case] pwd_sub_dir: &str,
        #[case] dash_c: &[&str],
        #[case] final_dir: &str,
        #[values(
            "dump git-modules",
            "config show",
            "fetch origin",
            "recombine",
            "push origin main"
        )]
        command: &str,
    ) {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
        std::fs::create_dir_all(temp_dir.join("repo")).unwrap();
        let expected_stderr = predicate::str::is_match(format!(
            "^ERROR: Could not find a git repository in '{}' or in any of its parents\n$",
            temp_dir.join(final_dir).canonicalize().unwrap().display()
        ))
        .unwrap();

        cargo_bin_git_toprepo_for_testing()
            .current_dir(temp_dir.join(pwd_sub_dir))
            .args(dash_c)
            .args(command.split(' '))
            .assert()
            .failure()
            .stderr(expected_stderr.clone());
    }

    /// Verify the verbosity options ordering.
    #[test]
    fn verbosity_help_text() {
        cargo_bin_git_toprepo_for_testing()
            .arg("help")
            .assert()
            .success()
            .stdout(predicate::str::contains(
                r#"
  help       Print this message or the help of the given subcommand(s)

Options:
  -C <PATH>      Run as if started in <PATH>
  -h, --help     Print help

Global options:
  -v, --verbose...         Increase log verbosity with -v or -vv, or ...
      --verbosity <LEVEL>  ... set a specific log verbosity from 0 to 5 [default: 3]
  -q, --quiet              Use `-q` to hide all output to stderr
      --no-progress        Hide scrolling progress bars
"#,
            ))
            .stderr("");
    }

    /// Verify the verbosity options ordering.
    #[test]
    fn verbosity_help_text_in_subcommand() {
        cargo_bin_git_toprepo_for_testing()
            .args(["info", "-h"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                r#"
Options:
      --is-emulated-monorepo  Exit with code 3 if the repository is not initialized by git-toprepo
  -h, --help                  Print help (see more with '--help')

Global options:
  -v, --verbose...         Increase log verbosity with -v or -vv, or ...
      --verbosity <LEVEL>  ... set a specific log verbosity from 0 to 5 [default: 3]
  -q, --quiet              Use `-q` to hide all output to stderr
      --no-progress        Hide scrolling progress bars
"#,
            ))
            .stderr("");
    }
}
