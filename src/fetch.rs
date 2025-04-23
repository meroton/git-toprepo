use crate::config::GitTopRepoConfig;
use crate::config::SubrepoConfig;
use crate::git::git_command;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::repo_name::RepoName;
use crate::util::SafeExitStatus;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use gix::remote::Direction;
use std::ops::Deref;
use std::path::PathBuf;
use std::str::FromStr as _;

pub struct RemoteFetcher {
    pub git_dir: PathBuf,
    pub args: Vec<String>,
    pub remote: Option<String>,
    pub refspecs: Vec<String>,
}

impl RemoteFetcher {
    pub fn new(repo: &gix::Repository) -> Self {
        Self {
            git_dir: repo.git_dir().to_owned(),
            args: Vec::new(),
            remote: None,
            refspecs: Vec::new(),
        }
    }

    pub fn set_remote_from_repo_name(
        &mut self,
        gix_repo: &gix::Repository,
        repo_name: &RepoName,
        config: &GitTopRepoConfig,
    ) -> Result<()> {
        match repo_name {
            RepoName::Top => {
                self.remote = match gix_repo.remote_default_name(Direction::Fetch) {
                    Some(name) => Some(name.to_str()?.to_string()),
                    None => Some("origin".to_owned()),
                };
            }
            RepoName::SubRepo(sub_repo_name) => {
                let subrepo_config = config
                    .subrepos
                    .get(sub_repo_name.deref())
                    .with_context(|| format!("Repo {repo_name} not found in config"))?;
                self.set_remote_from_subrepo_config(gix_repo, repo_name, subrepo_config)?;
            }
        };
        Ok(())
    }

    fn set_remote_from_subrepo_config(
        &mut self,
        gix_repo: &gix::Repository,
        repo_name: &RepoName,
        subrepo_config: &SubrepoConfig,
    ) -> Result<()> {
        let fetch_config = subrepo_config.get_fetch_config_with_url();
        let subrepo_url = fetch_config.url.expect("with fetch url");
        let super_url = gix_repo
            .find_default_remote(Direction::Fetch)
            .context("Missing default git-remote")?
            .context("Error getting default git-remote")?
            .url(Direction::Fetch)
            .context("Missing default git-remote fetch url")?
            .to_owned();
        self.remote = Some(
            super_url
                .join(&subrepo_url)
                .to_bstring()
                .to_str()
                .context("Bad UTF-8 defualt remote URL")?
                .to_owned(),
        );
        let ref_namespace_prefix = repo_name.to_ref_prefix();
        self.args = vec![format!("--negotiation-tip={ref_namespace_prefix}*")];
        if fetch_config.depth != 0 {
            self.args.push(format!("--depth={}", fetch_config.depth));
        }
        if fetch_config.prune {
            self.args.push("--prune".to_owned());
        }
        self.refspecs = vec![
            format!("+refs/heads/*:{ref_namespace_prefix}refs/heads/*"),
            format!("+refs/tags/*:{ref_namespace_prefix}refs/tags/*"),
            // Don't specify HEAD as git-fetch fails if it is missing because of
            // not being wildcard pattern. HEAD is just a symbolic ref anyway,
            // so there is no gain in fetching it if it points to e.g.
            // refs/heads/main anyway.
            // format!("+HEAD:{ref_namespace_prefix}HEAD"),
        ];
        Ok(())
    }

    pub fn set_remote_from_str(
        &mut self,
        gix_repo: &gix::Repository,
        name_or_url: &str,
        config: &GitTopRepoConfig,
    ) -> Result<()> {
        // Ignore any errors in the remote name.
        match gix_repo
            .try_find_remote(name_or_url)
            .and_then(|remote| remote.ok())
        {
            Some(remote) => {
                self.remote = Some(
                    remote
                        .url(Direction::Fetch)
                        .with_context(|| format!("Fetch URL for {name_or_url} is missing"))?
                        .to_string(),
                );
            }
            None => {
                // Not the super repo, try to find the subrepo.
                let url = gix::Url::from_bytes(name_or_url.into()).context("Invalid fetch URL")?;
                match config.get_from_url(&url)? {
                    Some((repo_name, subrepo_config)) => {
                        let repo_name = RepoName::from_str(&repo_name)
                            .map_err(|_| anyhow::anyhow!("Bad repo name {repo_name:#}"))?;
                        self.set_remote_from_subrepo_config(gix_repo, &repo_name, subrepo_config)?;
                    }
                    None => {
                        // TODO: Give command line or config suggestions to the user.
                        anyhow::bail!("No remote found for {name_or_url}");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn fetch(self, pb: &indicatif::ProgressBar) -> Result<()> {
        let remote = self.remote.context("No fetch remote set")?;
        pb.set_prefix(remote.clone());
        pb.set_message("Starting git-fetch");

        let mut cmd: std::process::Command = git_command(&self.git_dir);
        cmd.args([
            "fetch",
            "--progress",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--no-write-fetch-head",
        ])
        .args(self.args)
        .arg(&remote)
        .args(self.refspecs);

        let mut proc = cmd
            // TODO: Collect stdout (use a thread to avoid backpressure deadlock).
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| "Failed to spawn git-fetch".to_string())?;

        let last_paragraph = crate::util::read_stderr_progress_status(
            proc.stderr.take().expect("piping stderr"),
            |line| pb.set_message(line),
        );
        let exit_status = SafeExitStatus::new(proc.wait().context("Failed to wait for git-fetch")?);
        if let Err(err) = exit_status.check_success() {
            let maybe_newline = if last_paragraph.is_empty() { "" } else { "\n" };
            bail!("git fetch {remote} failed: {err:#}{maybe_newline}{last_paragraph}");
        }
        Ok(())
    }
}
