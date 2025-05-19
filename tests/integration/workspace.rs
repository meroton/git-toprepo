mod fixtures;

#[test]
fn test_workspace_search() {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();

    std::env::set_current_dir(std::path::Path::new(&toprepo)).unwrap();
    let workspace = git_toprepo::util::find_working_directory(None).unwrap();
    assert_eq!(workspace, toprepo);

    let subrepo = toprepo.join("sub").to_path_buf();
    std::env::set_current_dir(std::path::Path::new(&subrepo)).unwrap();
    let workspace = git_toprepo::util::find_working_directory(None).unwrap();
    assert_eq!(workspace, toprepo);
}
