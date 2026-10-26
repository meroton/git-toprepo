use super::git_filter_process::GitFilterProcess;
use anyhow::Result;

pub(super) trait ShardProvider {
    type Filter: GitFilterProcess;

    fn get_filter(&mut self, url: Option<&gix::Url>) -> Result<&mut Self::Filter>;
    /// The iteration continues while `f` returns `Ok(true)`.
    fn for_each(&mut self, f: impl FnMut(&mut Self::Filter) -> Result<bool>) -> Result<bool>;
}
