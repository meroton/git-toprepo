use super::packet_stream::PacketStream;
use super::shard_provider::ShardProvider;
use super::sub_process_filter::SubProcessFilter;
use crate::git::git_command;
use crate::lfs;
use crate::lfs::filter_process::git_packet::GitPacket;
use anyhow::Context as _;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;

pub(super) type GitLfsSubProcess =
    SubProcessFilter<std::process::ChildStdin, std::process::ChildStdout>;

pub(super) struct GitLfsShardProvider {
    repo: PathBuf,
    lfs_remote: lfs::LfsRemote,
    top_child: std::process::Child,
    top_processor: GitLfsSubProcess,
    sub_processors: HashMap<gix::Url, (std::process::Child, GitLfsSubProcess)>,
}

impl GitLfsShardProvider {
    pub fn new(
        repo: PathBuf,
        lfs_remote: lfs::LfsRemote,
        top_fetch_url: &gix::Url,
    ) -> Result<Self> {
        let (top_child, top_processor) = Self::run(&repo, &lfs_remote, top_fetch_url)?;
        Ok(Self {
            repo,
            lfs_remote,
            top_child,
            top_processor,
            sub_processors: HashMap::new(),
        })
    }

    fn run(
        repo: &Path,
        lfs_remote: &lfs::LfsRemote,
        url: &gix::Url,
    ) -> Result<(std::process::Child, GitLfsSubProcess)> {
        let mut cmd = git_command(repo);
        lfs_remote.override_remote_in_lfs_command(&mut cmd, &url.to_string());
        let mut child = cmd
            .args(["lfs", "filter-process"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn 'git lfs filter-process'")?;
        let stdin = PacketStream::new(child.stdin.take().context("Failed to take stdin")?);
        let stdout = PacketStream::new(child.stdout.take().context("Failed to take stdout")?);
        Ok((child, SubProcessFilter::new(stdin, stdout)?))
    }

    /// Terminates all child processes and returns the exit status of the first failing process, if any.
    pub fn join(self) -> Result<ExitStatus> {
        let mut exit_status = Self::join_one(self.top_child, self.top_processor);
        for (child, processor) in self.sub_processors.into_values() {
            let sub_exit_status = Self::join_one(child, processor);
            if let Ok(status) = &exit_status
                && status.success()
            {
                exit_status = sub_exit_status;
            }
        }
        exit_status
    }

    /// Terminates one child process and returns its exit status.
    fn join_one(mut child: std::process::Child, processor: GitLfsSubProcess) -> Result<ExitStatus> {
        let exit_status = try {
            // Close stdin and read until EOF from stdout.
            let mut processor_output = processor.half_close();
            loop {
                if let GitPacket::EndOfFile = processor_output
                    .read_packet_text()
                    .context("Draining child filter process stdout")?
                {
                    break;
                }
            }
            child.wait().context("Waiting for child process")?
        };
        if exit_status.is_err() {
            let _ = child
                .kill()
                .map_err(|err| log::error!("Faild to kill child filter process: {err:#}"));
        }
        exit_status
    }
}

impl ShardProvider for GitLfsShardProvider {
    type Filter = GitLfsSubProcess;

    fn get_filter(&mut self, url: Option<&gix::Url>) -> Result<&mut Self::Filter> {
        if let Some(url) = url {
            if !self.sub_processors.contains_key(url) {
                let (child, processor) = Self::run(&self.repo, &self.lfs_remote, url)?;
                self.sub_processors.insert(url.clone(), (child, processor));
            }
            Ok(&mut self.sub_processors.get_mut(url).unwrap().1)
        } else {
            Ok(&mut self.top_processor)
        }
    }

    fn for_each(&mut self, mut f: impl FnMut(&mut Self::Filter) -> Result<bool>) -> Result<bool> {
        if !f(&mut self.top_processor)? {
            return Ok(false);
        }
        for (_, processor) in &mut self.sub_processors.values_mut() {
            if !f(processor)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
