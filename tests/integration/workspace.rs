use anyhow::Result;

#[test]
fn test_workspace_search() -> Result<()> {
    let temp_dir = tempfile::TempDir::with_prefix("git-toprepo").unwrap();
    let temp_dir = temp_dir.path();
    let toprepo =
        crate::fixtures::toprepo::GitTopRepoExample::new(temp_dir.to_path_buf()).init_server_top();

    let workspace = git_toprepo::util::find_working_directory(&toprepo)?;
    assert_eq!(workspace, toprepo);

    let subrepo = toprepo.join("sub").to_path_buf();
    let workspace = git_toprepo::util::find_working_directory(&subrepo)?;
    assert_eq!(workspace, toprepo);
    Ok(())
}
