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
mod refilter;
#[cfg(test)]
mod version;
#[cfg(test)]
mod worktree;

#[cfg(test)]
mod main {
    use assert_cmd::assert::OutputAssertExt as _;
    use git_toprepo::git::git_command_for_testing;
    use rstest::rstest;

    #[rstest]
    #[case::default(".", &[])]
    #[case::pwd_sub("sub", &[])]
    #[case::c_sub(".", &["-C", "sub"])]
    #[case::pwd_sub_c_dotdot("sub", &["-C", ".."])]
    fn commands_in_uninitialized_repo_fails(
        #[case] pwd_sub_dir: &str,
        #[case] dash_c: &[&str],
        #[values(
            "dump import-cache",
            "config show",
            "fetch origin",
            "refilter",
            "push origin main"
        )]
        command: &str,
    ) {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
        std::fs::create_dir(temp_dir.join("sub")).unwrap();
        let expected_stderr = "ERROR: git-config \'toprepo.config\' is missing. Is this an initialized git-toprepo?\n";

        git_command_for_testing(&temp_dir)
            .args(["init", "--quiet"])
            .assert()
            .success();
        std::fs::create_dir_all(temp_dir.join("sub")).unwrap();

        assert_cmd::Command::cargo_bin("git-toprepo")
            .unwrap()
            .current_dir(temp_dir.join(pwd_sub_dir))
            .args(dash_c)
            .args(command.split(' '))
            .assert()
            .failure()
            .stderr(expected_stderr);
    }

    #[rstest]
    #[case::default(".", &[], "")]
    #[case::pwd_sub("sub", &[], "sub")]
    #[case::c_sub(".", &["-C", "sub"], "sub")]
    #[case::pwd_sub_c_dotdot("sub", &["-C", ".."], "")]
    fn commands_outside_git_repos_fail(
        #[case] pwd_sub_dir: &str,
        #[case] dash_c: &[&str],
        #[case] final_dir: &str,
        #[values(
            "dump git-modules",
            "config show",
            "fetch origin",
            "refilter",
            "push origin main"
        )]
        command: &str,
    ) {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
        std::fs::create_dir_all(temp_dir.join("sub")).unwrap();
        let expected_stderr = predicates::str::is_match(format!(
            "^ERROR: Could not find a git repository in '{}' or in any of its parents\n$",
            temp_dir.join(final_dir).canonicalize().unwrap().display()
        ))
        .unwrap();

        assert_cmd::Command::cargo_bin("git-toprepo")
            .unwrap()
            .current_dir(temp_dir.join(pwd_sub_dir))
            .args(dash_c)
            .args(command.split(' '))
            .assert()
            .failure()
            .stderr(expected_stderr.clone());
    }
}
