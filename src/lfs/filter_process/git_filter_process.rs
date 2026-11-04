use super::packet_stream::PacketStream;
use crate::git::GitPath;
use anyhow::Result;

/// The Git long running filter process interface, to avoid indivitual
/// subprocess invocations for each clean and smudge.
///
/// See https://git-scm.com/docs/gitattributes#_long_running_filter_process
pub(super) trait GitFilterProcess {
    fn clean(
        &mut self,
        pathname: &GitPath,
        content_to_clean: &mut PacketStream<impl std::io::Read>,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()>;

    fn smudge(
        &mut self,
        pathname: &GitPath,
        can_delay: bool,
        content_to_smudge: &mut PacketStream<impl std::io::Read>,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()>;

    /// If delay is not supported, this method is a no-op.
    fn list_available_blobs(
        &mut self,
        pathname_line_fn: impl FnMut(Vec<u8>) -> Result<()>,
    ) -> Result<String>;
}
