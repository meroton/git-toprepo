use crate::config::GitTopRepoConfig;
use crate::config::SubRepoConfig;
use crate::git::git_command;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::CommandSpanExt as _;
use crate::repo_name::RepoName;
use crate::util::CommandExtension;
use crate::util::SafeExitStatus;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use gix::remote::Direction;
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
            RepoName::Top => self.set_remote_as_top_repo(gix_repo)?,
            RepoName::SubRepo(sub_repo_name) => {
                let subrepo_config = config
                    .subrepos
                    .get(sub_repo_name)
                    .with_context(|| format!("Repo {repo_name} not found in config"))?;
                self.set_remote_from_subrepo_config(gix_repo, repo_name, subrepo_config)?;
            }
        };
        Ok(())
    }

    pub fn set_remote_as_top_repo(&mut self, gix_repo: &gix::Repository) -> Result<()> {
        self.remote = match gix_repo.remote_default_name(Direction::Fetch) {
            Some(name) => Some(name.to_str()?.to_string()),
            None => Some("origin".to_owned()),
        };
        Ok(())
    }

    fn set_remote_from_subrepo_config(
        &mut self,
        gix_repo: &gix::Repository,
        repo_name: &RepoName,
        subrepo_config: &SubRepoConfig,
    ) -> Result<()> {
        let fetch_config = subrepo_config.get_fetch_config_with_url();
        let subrepo_url = fetch_config.url.expect("with fetch url");
        let super_url = crate::git::get_default_remote_url(gix_repo)?;
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

    fn create_command(&self) -> std::process::Command {
        let mut cmd = git_command(&self.git_dir);
        cmd.args([
            "fetch",
            "--progress",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--no-write-fetch-head",
        ])
        .args(&self.args);
        if let Some(remote) = &self.remote {
            cmd.arg(remote);
        }
        cmd.args(&self.refspecs);
        cmd
    }

    pub fn fetch_with_progress_bar(self, pb: &indicatif::ProgressBar) -> Result<()> {
        pb.set_prefix(self.remote.as_deref().unwrap_or_default().to_owned());
        let (mut proc, _span_guard) = self
            .create_command()
            // TODO: Collect stdout (use a thread to avoid backpressure deadlock).
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .trace_command(crate::command_span!("git fetch"))
            .spawn()
            .context("Failed to spawn git-fetch")?;

        let permanent_stderr = crate::util::read_stderr_progress_status(
            proc.stderr.take().expect("piping stderr"),
            |line| pb.set_message(line),
        );
        let exit_status = SafeExitStatus::new(proc.wait().context("Failed to wait for git-fetch")?);
        if let Err(err) = exit_status.check_success() {
            let maybe_newline = if permanent_stderr.is_empty() {
                ""
            } else {
                "\n"
            };
            bail!(
                "git fetch {} failed: {err:#}{maybe_newline}{permanent_stderr}",
                self.remote.as_deref().unwrap_or("<default>")
            );
        }
        self.remove_fetch_head()?;
        Ok(())
    }

    pub fn fetch_on_terminal(self) -> Result<()> {
        self.create_command()
            .trace_command(crate::command_span!("git fetch"))
            .safe_output()
            .context("Failed to spawn git-fetch")?
            .check_success_with_stderr()
            .with_context(|| {
                format!(
                    "git fetch {} failed",
                    self.remote.as_deref().unwrap_or("<default>")
                )
            })
            .map(|_| ())?;
        self.remove_fetch_head()?;
        Ok(())
    }

    /// Remove the FETCH_HEAD file if it exists because the content should not
    /// be used without filtering into the monorepo.
    fn remove_fetch_head(&self) -> Result<()> {
        let fetch_head_path = self.git_dir.join("FETCH_HEAD");
        match std::fs::remove_file(&fetch_head_path) {
            Ok(_) => {
                // Successfully removed FETCH_HEAD.
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // FETCH_HEAD does not exist, which is fine.
                Ok(())
            }
            Err(err) => {
                // Some other error occurred while trying to remove FETCH_HEAD.
                Err(err).with_context(|| format!("Failed to remove {fetch_head_path:?}"))
            }
        }
    }
}
