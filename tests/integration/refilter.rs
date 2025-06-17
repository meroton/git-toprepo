use bstr::ByteSlice as _;
use git_toprepo::git::git_command;
use git_toprepo::util::CommandExtension as _;
use itertools::Itertools as _;
use std::path::Path;

#[test]
fn test_init_and_refilter_example() {
    let temp_dir =
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_readme_example.sh")
            .unwrap();
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
fn test_refilter_merge_with_one_submodule_a() {
    let temp_dir = gix_testtools::scripted_fixture_writable(
        "../integration/fixtures/make_merge_with_one_submodule_a.sh",
    )
    .unwrap();
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
fn test_refilter_merge_with_one_submodule_b() {
    let temp_dir = gix_testtools::scripted_fixture_writable(
        "../integration/fixtures/make_merge_with_one_submodule_b.sh",
    )
    .unwrap();
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
fn test_refilter_merge_with_two_submodules() {
    let temp_dir = gix_testtools::scripted_fixture_writable(
        "../integration/fixtures/make_merge_with_two_submodules.sh",
    )
    .unwrap();
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
fn test_refilter_submodule_removal() {
    let temp_dir = gix_testtools::scripted_fixture_writable(
        "../integration/fixtures/make_submodule_removal.sh",
    )
    .unwrap();
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
fn test_refilter_moved_submodule() {
    let temp_dir =
        gix_testtools::scripted_fixture_writable("../integration/fixtures/make_moved_submodule.sh")
            .unwrap();
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
    let log_output = git_command(repo_path)
        .args(["log", "--graph", "--format=%s"])
        .args(extra_args)
        .check_success_with_stderr()
        .unwrap();
    let log_graph = log_output.stdout.to_str().unwrap();
    // Replace TAB and trailing spaces.

    log_graph
        .split('\n')
        .map(str::trim_end)
        .join("\n")
        .replace('\t', " ")
}
