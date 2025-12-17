use anyhow::Result;
use anyhow::anyhow;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitReview {
    pub host: String,
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
///         project: "dir/project.git".to_owned(),
///         host: "abc.gerrit.internal".to_owned(),
///         port: None,
///     }
/// );
/// let x = &(x.to_owned() + "port=22");
/// assert_eq!(
///     git_toprepo::gitreview::parse_git_review(x).unwrap(),
///     git_toprepo::gitreview::GitReview {
///         project: "dir/project.git".to_owned(),
///         host: "abc.gerrit.internal".to_owned(),
///         port: Some(22),
///     }
/// );
/// ```
pub fn parse_git_review(content: &str) -> Result<GitReview> {
    let mut port = None;
    let mut host = SENTINEL_VALUE.to_owned();
    let mut project = SENTINEL_VALUE.to_owned();

    for line in content.lines() {
        match line.trim().split_once("=") {
            Some(("host", h)) => host = h.to_owned(),
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
    Ok(GitReview {
        port,
        host,
        project,
    })
}
