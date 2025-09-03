use anyhow::Result;
use anyhow::anyhow;
use serde::Deserialize;

/// TODO: Create a new config format for Gerrit entirely.
/// backwards compatible with gitreview so projects can symlink the files.

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitReview {
    pub host: String,
    // ssh host is often the same as the host but can be different.
    pub ssh_host: String,
    pub project: String,
    pub port: Option<i32>,
}

const SENTINEL_VALUE: &str = "git-toprepo::gitreview::SENTINEL_VALUE::PARSER_ERROR";

/// Parse `.gitreview` file content.
/// ```
/// let x = "[gerrit]
///    host=abc.gerrit.internal
///    project=dir/project.git
/// ";
/// assert_eq!(
///     git_toprepo::gitreview::parse_git_review(x).unwrap(),
///     git_toprepo::gitreview::GitReview {
///          project: "dir/project.git".to_owned(),
///          host: "abc.gerrit.internal".to_owned(),
///          ssh_host: "abc.gerrit.internal".to_owned(),
///          port: None,
///     }
/// );
///
/// let x = &(x.to_owned() + "port=22");
/// assert_eq!(
///     git_toprepo::gitreview::parse_git_review(x).unwrap(),
///     git_toprepo::gitreview::GitReview {
///          project: "dir/project.git".to_owned(),
///          host: "abc.gerrit.internal".to_owned(),
///          ssh_host: "abc.gerrit.internal".to_owned(),
///          port: Some(22),
///     }
/// );
///
/// let x = "[gerrit]
///    host=ABC.gerrit.internal
///    ssh_host=OTHER.gerrit.internal
///    project=dir/project.git
/// ";
/// assert_eq!(
///     git_toprepo::gitreview::parse_git_review(x).unwrap(),
///     git_toprepo::gitreview::GitReview {
///          project: "dir/project.git".to_owned(),
///          host: "ABC.gerrit.internal".to_owned(),
///          ssh_host: "OTHER.gerrit.internal".to_owned(),
///          port: None,
///     }
/// );
/// ```
pub fn parse_git_review(content: &str) -> Result<GitReview> {
    let mut port = None;
    let mut host = SENTINEL_VALUE.to_owned();
    let mut ssh_host = None;
    let mut project = SENTINEL_VALUE.to_owned();

    for line in content.lines() {
        match line.trim().split_once("=") {
            Some(("host", h)) => host = h.to_owned(),
            Some(("ssh_host", h)) => ssh_host = Some(h.to_owned()),
            Some(("project", proj)) => project = proj.to_owned(),
            Some(("port", p)) => port = Some(p.parse::<i32>()?),
            x => {
                if line != "[gerrit]" {
                    // TODO: Upscope: enrich this with the file we parsed.
                    return Err(anyhow!(
                        "Could not parse line {:?} in .gitreview: {:?}, error on {:?}",
                        line,
                        content,
                        x,
                    ));
                }
            }
        }
    }

    assert_ne!(host, SENTINEL_VALUE);
    assert_ne!(project, SENTINEL_VALUE);
    let ssh_host = ssh_host.get_or_insert(host.clone()).to_string();
    Ok(GitReview {
        port,
        project,
        host,
        ssh_host,
    })
}
