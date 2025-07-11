#[test]
fn test_workspace_search() {
    let temp_dir = crate::fixtures::toprepo::readme_example_tempdir();
    let temp_dir = temp_dir.path();
    let toprepo = temp_dir.join("top");

    let workspace = git_toprepo::util::find_current_worktree(&toprepo).unwrap();
    assert_eq!(workspace, toprepo);

    let subrepo = toprepo.join("sub").to_path_buf();
    let workspace = git_toprepo::util::find_current_worktree(&subrepo).unwrap();
    assert_eq!(workspace, toprepo);
}
