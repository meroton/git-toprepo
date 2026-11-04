use super::git_filter_process::GitFilterProcess;
use super::packet_stream::PacketStream;
use super::shard_provider::ShardProvider;
use crate::git::GitPath;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;

pub(super) struct ShardingFilterProcess<T: ShardProvider> {
    pub(super) sub_filter_provider: T,
}

impl<T: ShardProvider> ShardingFilterProcess<T> {
    pub fn clean(
        &mut self,
        pathname: &GitPath,
        content_to_clean: &mut PacketStream<impl std::io::Read>,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        self.sub_filter_provider
            .get_filter(None)?
            .clean(pathname, content_to_clean, output)?;
        Ok(())
    }

    pub fn smudge(
        &mut self,
        url: Option<&gix::Url>,
        pathname: &GitPath,
        can_delay: bool,
        stdin: &mut PacketStream<impl std::io::Read>,
        stdout: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        let processor_shard = self.sub_filter_provider.get_filter(url).with_context(|| {
            format!(
                "Failed to initialize 'git lfs filter-process' for {}",
                url.map_or_else(|| "<top>".to_owned(), |u| u.to_string()),
            )
        })?;
        processor_shard
            .smudge(pathname, can_delay, stdin, stdout)
            .with_context(|| format!("Failed to smudge {}", pathname.to_str_lossy()))?;
        Ok(())
    }

    pub fn list_available_blobs(
        &mut self,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        let processed_all = self.sub_filter_provider.for_each(|sub_processor| {
            let status_line = sub_processor
                .list_available_blobs(|pathname_line| output.write_packet(pathname_line))?;
            if status_line != "status=success" {
                output.write_flush_packet()?;
                output.write_packet(status_line)?;
                output.write_flush_packet()?;
                return Ok(false);
            }
            Ok(true)
        })?;
        if processed_all {
            output.write_flush_packet()?;
            output.write_packet("status=success\n")?;
            output.write_flush_packet()?;
        }
        Ok(())
    }
}
