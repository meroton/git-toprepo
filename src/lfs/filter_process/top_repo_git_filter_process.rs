use super::git_lfs_shard_provider::GitLfsShardProvider;
use super::sharding_filter_process::ShardingFilterProcess;
use crate::lfs;
use anyhow::Result;
use std::path::PathBuf;
use std::process::ExitStatus;

pub(super) type TopRepoGitFilterProcess = ShardingFilterProcess<GitLfsShardProvider>;

impl TopRepoGitFilterProcess {
    pub fn new(
        repo: PathBuf,
        lfs_remote: lfs::LfsRemote,
        top_fetch_url: &gix::Url,
    ) -> Result<Self> {
        Ok(Self {
            sub_filter_provider: GitLfsShardProvider::new(repo, lfs_remote, top_fetch_url)?,
        })
    }

    pub fn join(self) -> Result<ExitStatus> {
        self.sub_filter_provider.join()
    }
}
