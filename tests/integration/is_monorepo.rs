use assert_cmd::prelude::*;
use predicate::str::is_empty;
use predicates::prelude::*;
use std::process::Command;


#[test]
#[allow(non_snake_case)]
fn test__is_monorepo() {
    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_minimal_with_two_submodules.sh",
        )
        .unwrap(),
    );
    let toprepo = temp_dir.join("top");
    let monorepo = temp_dir.join("mono");
    crate::fixtures::toprepo::clone(&toprepo, &monorepo);

    let child_dir = monorepo.join("subx");
    /*
     * let regular_submodule = ...
     */

    for (wd, flags, ok) in [
        (Some(&toprepo), vec![], false),
        (None, vec!["-C", toprepo.to_str().unwrap()], false),
        (Some(&monorepo), vec![], true),
        (Some(&monorepo), vec!["-C", "."], true),
        (None, vec!["-C", monorepo.to_str().unwrap()], true),
        (Some(&child_dir), vec![], true),
        (Some(&child_dir), vec!["-C", "."], true),
        /*
         * Some(&subdirectory, vec![], false),
         * ...
         */
    ] {
        let mut args = flags;
        args.extend(vec!["is-monorepo"]);

        let mut command = Command::cargo_bin("git-toprepo").unwrap();
        let command = match wd {
            Some(wd) => {
                command.current_dir(wd)
            },
            None => {
                &mut command
            }
        };

        let assert = command
            .args(args)
            .assert()
            .stdout(is_empty())
            .stderr(is_empty());
        if ok {
            assert.success()
        } else {
                assert.failure()
        };
    }
}
