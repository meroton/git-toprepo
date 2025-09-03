use crate::config::SubRepoConfig;
use crate::git::git_command;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::loader::SubRepoLedger;
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
use wait_timeout::ChildExt as _;

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

    pub(crate) fn set_remote_from_repo_name(
        &mut self,
        gix_repo: &gix::Repository,
        repo_name: &RepoName,
        ledger: &SubRepoLedger,
    ) -> Result<()> {
        match repo_name {
            RepoName::Top => self.set_remote_as_top_repo(gix_repo)?,
            RepoName::SubRepo(sub_repo_name) => {
                let subrepo_config = ledger
                    .subrepos
                    .get(sub_repo_name)
                    .with_context(|| format!("Repo {repo_name} not found in ledger"))?;
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
        self.args.push("--prune".to_owned());
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

    pub fn fetch_with_progress_bar(
        self,
        pb: &indicatif::ProgressBar,
        idle_timeouts: &Vec<Option<std::time::Duration>>,
    ) -> Result<()> {
        let remote_str = self.remote.as_deref().unwrap_or_default();
        pb.set_prefix(remote_str.to_owned());
        for idle_timeout in idle_timeouts {
            let _timeout_span_guard = tracing::debug_span!("idle-timeout", ?idle_timeout).entered();
            let (proc, _span_guard) = self
                .create_command()
                // TODO: 2025-09-22 Collect stdout (use a thread to avoid backpressure deadlock).
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .trace_command(crate::command_span!("git fetch"))
                .spawn()
                .context("Failed to spawn git-fetch")?;

            let Some((exit_status, permanent_stderr)) =
                Self::wait_for_fetch_with_progress_bar(pb, idle_timeout.as_ref(), proc, remote_str)
                    .with_context(|| format!("Running git-fetch {remote_str}"))?
            else {
                // Timeout, try again.
                let idle_timeout = idle_timeout.expect("Timeout is set");
                log::warn!(
                    "git fetch {remote_str} timed out, was silent {idle_timeout:?}, retrying"
                );
                continue;
            };
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
            return Ok(());
        }
        bail!("git fetch {remote_str} exceeded timeout retry limit");
    }

    fn wait_for_fetch_with_progress_bar(
        pb: &indicatif::ProgressBar,
        idle_timeout: Option<&std::time::Duration>,
        mut proc: std::process::Child,
        remote_str: &str,
    ) -> Result<Option<(SafeExitStatus, String)>> {
        let stderr_pipe = proc.stderr.take().expect("piping stderr");
        if let Some(idle_timeout) = idle_timeout {
            let proc_timeout = &std::sync::Mutex::new(std::time::Instant::now() + *idle_timeout);
            std::thread::scope(|scope| {
                // Collect stderr in a separate thread to be able to check for
                // timeout. Creating a thread should be cheaper than running
                // git-fetch, so no need to optimize.
                let parent_span = tracing::Span::current();
                let stderr_thread = std::thread::Builder::new()
                    .name(format!("git-fetch-stderr-{remote_str}"))
                    .spawn_scoped(scope, move || {
                        let _span_guard =
                            tracing::debug_span!(parent: &parent_span, "git-fetch stderr")
                                .entered();
                        crate::util::read_stderr_progress_status(stderr_pipe, |line| {
                            tracing::trace!(name: "stderr", line = ?line);
                            pb.set_message(line);
                            *proc_timeout.lock().unwrap() =
                                std::time::Instant::now() + *idle_timeout;
                        })
                    })
                    .expect("Failed to spawn git-fetch stderr thread");
                let Some(exit_status) = proc_wait_with_timeout(proc, proc_timeout)
                    .with_context(|| format!("Running git-fetch {remote_str}"))?
                else {
                    // Timeout.
                    return Ok(None);
                };
                let permanent_stderr = stderr_thread
                    .join()
                    .expect("Failed to join git-fetch stderr thread");
                Ok(Some((exit_status, permanent_stderr)))
            })
        } else {
            // No timeout, just wait.
            let permanent_stderr = crate::util::read_stderr_progress_status(stderr_pipe, |line| {
                tracing::trace!(name: "stderr", line = ?line);
                pb.set_message(line);
            });
            let exit_status = SafeExitStatus::new(
                proc.wait()
                    .with_context(|| format!("Failed to wait for git-fetch {remote_str}"))?,
            );
            Ok(Some((exit_status, permanent_stderr)))
        }
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

fn proc_wait_with_timeout(
    mut proc: std::process::Child,
    timeout: &std::sync::Mutex<std::time::Instant>,
) -> Result<Option<SafeExitStatus>> {
    loop {
        let duration_to_wait = timeout
            .lock()
            .unwrap()
            .saturating_duration_since(std::time::Instant::now());
        if duration_to_wait.is_zero() {
            // Timeout reached.
            proc.kill().expect("Failed to kill git-fetch after timeout");
            let status = SafeExitStatus::new(
                proc.wait()
                    .context("Failed to wait for process after kill")?,
            );
            if status.success() {
                // Finished before kill.
                return Ok(Some(status));
            }
            return Ok(None);
        }
        if let Some(status) = proc
            .wait_timeout(duration_to_wait)
            .context("Failed to wait for process with timeout")?
        {
            // Process exited.
            return Ok(Some(SafeExitStatus::new(status)));
        }
        // Timed out, check if the timeout has been updated.
    }
}
