use crate::util::CommandExtension as _;
use crate::util::trim_newline_suffix;
use anyhow::Context;
use anyhow::Result;
use bstr::BString;
use bstr::ByteSlice as _;
use serde_with::serde_as;
use std::collections::HashMap;
use std::fmt::Display;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub type CommitId = gix::ObjectId;
pub type TreeId = gix::ObjectId;
pub type BlobId = gix::ObjectId;

#[serde_as]
#[derive(
    Default,
    Debug,
    Clone,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct GitPath(
    /// The serialized human readable form is a string, so non-UTF8 will panic.
    // TODO: Maybe '::<hex>' as paths don't contain (or starts with) ':'.
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    BString,
);

impl GitPath {
    pub const fn new(path: BString) -> Self {
        Self(path)
    }

    /// Joins two paths together.
    ///
    /// ```
    /// use git_toprepo::git::GitPath;
    /// use bstr::ByteSlice as _;
    /// let empty_path = GitPath::new(b"".into());
    /// let foo = GitPath::new(b"foo".into());
    /// let bar = GitPath::new(b"bar".into());
    /// assert_eq!(foo.join(&bar), GitPath::new(b"foo/bar".into()));
    /// assert_eq!(foo.join(&empty_path), GitPath::new(b"foo".into()));
    /// assert_eq!(empty_path.join(&bar), GitPath::new(b"bar".into()));
    /// ```
    pub fn join(&self, other: &GitPath) -> Self {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() {
            self.clone()
        } else {
            let mut path = Vec::with_capacity(self.0.len() + 1 + other.0.len());
            path.extend_from_slice(&self.0);
            path.push(b'/');
            path.extend_from_slice(&other.0);
            Self(path.into())
        }
    }

    /// Removes a prefix from a path.
    ///
    /// ```
    /// use git_toprepo::git::GitPath;
    /// use bstr::ByteSlice as _;
    /// let empty_path = GitPath::new(b"".into());
    /// let foo_bar = GitPath::new(b"foo/bar".into());
    /// let foo = GitPath::new(b"foo".into());
    /// let bar = GitPath::new(b"bar".into());
    /// assert_eq!(foo_bar.relative_to(&foo), Some(GitPath::new(b"bar".into())));
    /// assert_eq!(foo_bar.relative_to(&foo_bar), Some(GitPath::new(b"".into())));
    /// assert_eq!(foo_bar.relative_to(&empty_path), Some(GitPath::new(b"foo/bar".into())));
    /// assert_eq!(empty_path.relative_to(&bar), None);
    /// assert_eq!(foo_bar.relative_to(&bar), None);
    /// ```
    pub fn relative_to(&self, other: &Self) -> Option<Self> {
        if other.0.is_empty() {
            // The other path is empty, return self.
            return Some(self.clone());
        } else if self.0.starts_with(&other.0) {
            if self.0.len() == other.0.len() {
                // The paths are equal.
                return Some(Self(BString::new(vec![])));
            }
            if self.0[other.0.len()] == b'/' {
                let relative_path = &self.0[other.0.len() + 1..];
                return Some(Self(relative_path.into()));
            }
        }
        None
    }
}

impl Deref for GitPath {
    type Target = BString;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for GitPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<S> From<S> for GitPath
where
    S: Into<BString>,
{
    fn from(path: S) -> Self {
        Self(path.into())
    }
}

/// Returns the default ("origin") remote URL for a repository.
pub fn get_default_remote_url(repo: &gix::Repository) -> Result<gix::Url> {
    Ok(repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .context("Missing default git-remote")?
        .context("Error getting default git-remote")?
        .url(gix::remote::Direction::Fetch)
        .context("Missing default git-remote fetch url")?
        .to_owned())
}

#[derive(Default)]
pub struct GitModulesInfo {
    pub submodules: HashMap<GitPath, Result<gix::Url>>,
}

impl GitModulesInfo {
    /// Parses a `.gitmodules` file in a git repository.
    pub fn parse_dot_gitmodules_in_repo(repo: &gix::Repository) -> Result<Self> {
        let Some(workdir) = repo.workdir() else {
            anyhow::bail!("Repository {} has no workdir", repo.common_dir().display());
        };
        let path = workdir.join(".gitmodules");
        let bytes = std::fs::read(&path)
            .or_else(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    // The file does not exist, return empty info.
                    Ok(Vec::new())
                } else {
                    Err(err)
                }
            })
            .with_context(|| {
                format!(
                    "Failed to read .gitmodules file in repository {}",
                    repo.common_dir().display()
                )
            })?;
        let config = gix::submodule::File::from_bytes(&bytes, path, &Default::default())
            .context("Failed to parse .gitmodules")?;
        let mut info = GitModulesInfo::default();
        for name in config.names() {
            // Skip misconfigured paths, they might not even be used.
            let Ok(path) = config.path(name) else {
                continue;
            };
            let url = config.url(name).map_err(anyhow::Error::new);
            info.submodules.insert(GitPath::new(path.into_owned()), url);
        }
        Ok(info)
    }

    /// Parses the `.gitmodules` content.
    ///
    /// The `path` argument is used for error reporting only.
    pub fn parse_dot_gitmodules_bytes(bytes: &[u8], path: PathBuf) -> Result<Self> {
        let config = gix::submodule::File::from_bytes(bytes, Some(path), &Default::default())
            .context("Failed to parse .gitmodules")?;
        let mut info = GitModulesInfo::default();
        for name in config.names() {
            // Skip misconfigured paths, they might not even be used.
            let Ok(path) = config.path(name) else {
                continue;
            };
            let url = config.url(name).map_err(anyhow::Error::new);
            info.submodules.insert(GitPath::new(path.into_owned()), url);
        }
        Ok(info)
    }

    /// Returns the submodule path of the submodule that contains the given path
    /// or None if the path is not in any submodule.
    ///
    /// Note that the .gitmodules file might contain entries of inner submodules
    /// but there is no reason for it to contain entries for paths that are not
    /// submodules. Therefore, the shortest matching path is selected.
    pub fn get_containing_submodule(
        &'_ self,
        path: &GitPath,
    ) -> Option<(&'_ GitPath, &'_ Result<gix::Url>)> {
        let mut best_prefix_len = usize::MAX;
        let mut ret = None;
        for (submodule_path, url) in &self.submodules {
            let prefix_len = submodule_path.len();
            if prefix_len < best_prefix_len
                && path.starts_with(submodule_path)
                && path.get(prefix_len) == Some(&b'/')
            {
                ret = Some((submodule_path, url));
                best_prefix_len = prefix_len;
            }
        }
        ret
    }
}

/// Run git without repository context.
pub fn git_global_command() -> Command {
    Command::new("git")
}

pub fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.args([std::ffi::OsStr::new("-C"), repo.as_os_str()]);
    command
}

/// Returns the value of a single entry git configuration key
/// or `None` if the key is not set.
pub fn git_config_get(repo: &Path, key: &str) -> anyhow::Result<Option<String>> {
    let output = git_command(repo).args(["config", key]).safe_output()?;
    if output.status.code() == Some(1) {
        Ok(None)
    } else {
        output.check_success_with_stderr()?;
        Ok(Some(
            trim_newline_suffix(output.stdout.to_str()?).to_string(),
        ))
    }
}

/// Sets the submodule pointer without checking out the submodule.
pub fn git_update_submodule_in_index(repo: &Path, path: &GitPath, commit: &CommitId) -> Result<()> {
    git_command(repo)
        .args([
            "update-index",
            "--cacheinfo",
            &format!("160000,{commit},{path}"),
        ])
        .check_success_with_stderr()
        .with_context(|| format!("Failed to set submodule {path}={commit} in {repo:?}"))
        .map(|_| ())
}

/*
#[derive(Debug)]
pub struct PushSplitter<'a> {
    repo: &'a Repo,
}

impl PushSplitter<'_> {
    //TODO: verify
    pub fn new(repo: &Repo) -> PushSplitter {
        PushSplitter { repo }
    }

    pub fn _trim_push_commit_message(mono_message: &str) -> Result<&str> {
        let mut trimmed_message = mono_message;

        if let Some(i) = mono_message.rfind("\n^-- ") {
            trimmed_message = &mono_message[..=i];
        }

        if trimmed_message.contains("\n^-- ") {
            Err(anyhow!(
                "'^-- ' was found in the following commit message. \
                It looks like a commit that already exists upstream. {}",
                mono_message
            ))
        } else {
            Ok(trimmed_message)
        }
    }

    #[allow(unused)]
    pub fn get_top_commit_subrepos(
        &self,
        top_commit_hash: CommitHash,
    ) -> HashMap<Vec<u8>, CommitHash> {
        let top_commit_hash = ""; //TODO
        let ls_tree_subrepo_stdout = Command::new("git")
            .args(["-C", self.repo.path.to_str().unwrap()])
            .args(["ls-tree", "-r", top_commit_hash, "--"])
            .safe_output()
            .unwrap()
            .stdout;

        let mut subrepo_map = HashMap::new();
        for line in ls_tree_subrepo_stdout.lines() {
            let line = line.unwrap();
            let submodule_mode_and_type_prefix = "160000 commit ";

            if line.starts_with(submodule_mode_and_type_prefix) {
                let hash_and_path = &line[submodule_mode_and_type_prefix.len()..];
                let (submod_hash, subdir) = hash_and_path.split_once("\t").unwrap();
                subrepo_map.insert(
                    subdir.bytes().collect_vec(),
                    submod_hash.bytes().collect_vec().into(),
                );
            }
        }

        subrepo_map
    }
}
*/

/// Walks through the history from the tips until commits that are already
/// exported are found. Those commits can be used as negative filter for
/// which commits to export.
pub fn get_first_known_commits<F, I>(
    repo: &gix::Repository,
    start_commit_ids: I,
    mut exists_filter: F,
    pb: &indicatif::ProgressBar,
) -> Result<(Vec<CommitId>, usize)>
where
    F: FnMut(CommitId) -> bool,
    I: Iterator<Item = CommitId>,
{
    let mut start_commit_ids = start_commit_ids.peekable();
    if start_commit_ids.peek().is_none() {
        // No commits to walk.
        return Ok((Vec::new(), 0));
    }

    pb.unset_length();
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{elapsed:>4} {msg} {pos}")
            .unwrap(),
    );
    pb.set_message("Looking for new commits to expand");

    let walk = repo.rev_walk(start_commit_ids);
    // TODO: The commit graph cannot be reused. Until fixed upstream,
    // use the default behaviour of reloading it for each walk.
    // walk.with_commit_graph(cache);
    let mut stop_commit_ids: Vec<gix::ObjectId> = Vec::new();
    let mut unknown_commit_count: usize = 0;
    for info in walk.selected(|commit_id| {
        if exists_filter(commit_id.to_owned()) {
            stop_commit_ids.push(commit_id.to_owned());
            // Skip the parents of this commit.
            false
        } else {
            pb.inc(1);
            unknown_commit_count += 1;
            // Dig deeper.
            true
        }
    })? {
        // Discard the output, check for errors.
        info.context("Looking for commits to process")?;
    }
    Ok((stop_commit_ids, unknown_commit_count))
}

/// Resolve a working directory relative path to a path relative to the git
/// worktree.
pub fn repo_relative_path(worktree: &Path, cwd_relpath: &Path) -> Result<GitPath> {
    let worktree = worktree.canonicalize()?;
    let wanted_path = cwd_relpath.canonicalize()?;
    let worktree_path = wanted_path
        .strip_prefix(worktree)
        .context("Path is not relative to the worktree")?;
    Ok(GitPath::new(
        worktree_path.as_os_str().as_encoded_bytes().into(),
    ))
}

/// Scaffolding code to create deterministic commits.
pub fn commit_env_for_testing() -> HashMap<String, String> {
    HashMap::from(
        [
            ("GIT_AUTHOR_NAME", "A Name"),
            ("GIT_AUTHOR_EMAIL", "a@no.domain"),
            ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
            ("GIT_COMMITTER_NAME", "C Name"),
            ("GIT_COMMITTER_EMAIL", "c@no.domain"),
            ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string())),
    )
}
