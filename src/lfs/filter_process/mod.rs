mod filter_command;
mod git_filter_process;
mod git_lfs_shard_provider;
mod git_packet;
mod packet_stream;
mod shard_provider;
mod sharding_filter_process;
mod sub_process_filter;
mod top_repo_git_filter_process;

use crate::git::FileLocationAndUrl;
use crate::git::GitPath;
use crate::git::git_command;
use crate::lfs::LfsRemote;
use crate::lfs::get_top_url;
use crate::log::CommandSpanExt as _;
use crate::repo::ConfiguredTopRepo;
use crate::util::CommandExtension as _;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice;
use git_packet::GitPacket;
use packet_stream::PacketStream;
use std::path::Path;
use top_repo_git_filter_process::TopRepoGitFilterProcess;

/// Forward the call to `git lfs filter-process --skip`, which can be done
/// without intercepting arguments and stdin/stdout.
pub fn run_lfs_filter_process_skip(repo: &Path) -> Result<()> {
    git_command(repo)
        .args(["lfs", "filter-process"])
        .arg("--skip")
        .trace_command(crate::command_span!("git lfs filter-process"))
        .safe_status()?
        .check_success()
        .context("Failed to run 'git lfs filter-process --skip'")?;
    Ok(())
}

/// For each `git filter-process` command, find out which repository `path`
/// belongs to and forward to `git lfs filter-process` within that context.
///
/// This implements the
/// https://git-scm.com/docs/gitattributes#_long_running_filter_process
/// for `command=smudge` and `command=clean`.
pub fn run_lfs_filter_process(configured_repo: &ConfiguredTopRepo) -> Result<()> {
    let lfs_remote =
        LfsRemote::from_repo_log_errors(&configured_repo.gix_repo, gix::remote::Direction::Fetch);
    let top_fetch_url = get_top_url(
        &configured_repo.gix_repo,
        lfs_remote.remote_name(),
        gix::remote::Direction::Fetch,
    )?;
    let worktree = configured_repo
        .gix_repo
        .workdir()
        .context("Missing worktree")?;
    let mut filter_processor =
        TopRepoGitFilterProcess::new(worktree.to_path_buf(), lfs_remote, &top_fetch_url)?;

    let stdin = &mut PacketStream::new(std::io::stdin());
    let stdout = &mut PacketStream::new(std::io::stdout());
    filter_process_handshake(stdin, stdout)?;
    let resolve_url = |pathname: &GitPath| -> Result<Option<gix::Url>> {
        let location = FileLocationAndUrl::resolve_in_worktree(
            worktree,
            pathname,
            &top_fetch_url,
            &configured_repo.config.subrepos,
            gix::remote::Direction::Fetch,
        )?;
        Ok((!location.subrepo_path.is_empty()).then_some(location.url))
    };
    run_lfs_filter_process_loop(&mut filter_processor, resolve_url, stdin, stdout)?;
    let exit_status = filter_processor.join()?;
    if !exit_status.success() {
        anyhow::bail!("'git lfs filter-process' exited with {exit_status}");
    }
    Ok(())
}

fn filter_process_handshake(
    stdin: &mut PacketStream<impl std::io::Read>,
    stdout: &mut PacketStream<impl std::io::Write>,
) -> Result<()> {
    try {
        let packet = stdin.read_packet_text()?;
        if !matches!(packet, GitPacket::Data(ref line) if line == "git-filter-client") {
            anyhow::bail!("Got {packet:?}");
        }
    }
    .context("Expected 'git-filter-client' from Git")?;
    try {
        let packet = stdin.read_packet_text()?;
        if !matches!(packet, GitPacket::Data(ref line) if line == "version=2") {
            anyhow::bail!("Got {packet:?}");
        }
    }
    .context("Expected 'version=2' from Git")?;
    try {
        let packet = stdin.read_packet_text()?;
        if !matches!(packet, GitPacket::Flush) {
            anyhow::bail!("Got {packet:?}");
        }
    }
    .context("Bad version header end from Git")?;

    stdout.write_packet("git-filter-server\n")?;
    stdout.write_packet("version=2\n")?;
    stdout.write_flush_packet()?;

    let mut supports_clean = false;
    let mut supports_smudge = false;
    let mut supports_delay = false;
    try {
        loop {
            match stdin.read_packet_text()? {
                GitPacket::Data(ref line)
                    if let Some(capability) = line.strip_prefix(b"capability=") =>
                {
                    if capability == b"clean" {
                        supports_clean = true;
                    } else if capability == b"smudge" {
                        supports_smudge = true;
                    } else if capability == b"delay" {
                        supports_delay = true;
                    } else {
                        log::debug!("Unknown capability: {:?}", capability);
                    }
                }
                GitPacket::Data(line) => anyhow::bail!("Got {:?}", line.to_str_lossy()),
                GitPacket::Flush => break,
                GitPacket::EndOfFile => anyhow::bail!("Unexpected end of file"),
            }
        }
    }
    .context("Negotiating capabilities with Git")?;
    if !supports_clean || !supports_smudge {
        anyhow::bail!(
            "Git filter client must support 'clean' and 'smudge', but requested clean={supports_clean}, smudge={supports_smudge}"
        );
    }

    stdout.write_packet("capability=clean\n")?;
    stdout.write_packet("capability=smudge\n")?;
    if supports_delay {
        stdout.write_packet("capability=delay\n")?;
    }
    stdout.write_flush_packet()?;
    Ok(())
}

fn run_lfs_filter_process_loop<T: shard_provider::ShardProvider>(
    filter_process: &mut sharding_filter_process::ShardingFilterProcess<T>,
    mut url_resolver: impl FnMut(&GitPath) -> Result<Option<gix::Url>>,
    stdin: &mut PacketStream<impl std::io::Read>,
    stdout: &mut PacketStream<impl std::io::Write>,
) -> Result<()> {
    loop {
        let request = stdin.read_filter_process_request()?;
        match request {
            filter_command::FilterCommand::Clean { pathname } => {
                filter_process.clean(&pathname, stdin, stdout)?;
            }
            filter_command::FilterCommand::Smudge {
                pathname,
                can_delay,
            } => {
                let url = url_resolver(&pathname)?;
                filter_process.smudge(url.as_ref(), &pathname, can_delay, stdin, stdout)?;
            }
            filter_command::FilterCommand::ListAvailableBlobs => {
                filter_process.list_available_blobs(stdout)?;
            }
            filter_command::FilterCommand::EndOfFile => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lfs::filter_process::git_filter_process::GitFilterProcess;
    use crate::lfs::filter_process::shard_provider::ShardProvider;
    use crate::lfs::filter_process::sharding_filter_process::ShardingFilterProcess;
    use anyhow::Result;
    use rstest::rstest;
    use std::cell::RefCell;
    use std::io::Cursor;
    use std::rc::Rc;

    struct FakeFilter {
        name: &'static str,
        calls: Rc<RefCell<Vec<String>>>,
    }

    #[rstest]
    fn filter_process_handshake_negotiates_clean_and_smudge(
        #[values(true, false)] with_delay: bool,
    ) {
        let mut client = PacketStream::new(Vec::new());
        client.write_packet("git-filter-client\n").unwrap();
        client.write_packet("version=2\n").unwrap();
        client.write_flush_packet().unwrap();
        client.write_packet("capability=clean\n").unwrap();
        client.write_packet("capability=smudge\n").unwrap();
        if with_delay {
            client.write_packet("capability=delay\n").unwrap();
        }
        client.write_flush_packet().unwrap();

        let mut server_input = PacketStream::new(Cursor::new(client.into_inner()));
        let mut server_output = PacketStream::new(Vec::new());
        filter_process_handshake(&mut server_input, &mut server_output).unwrap();

        let mut expected = PacketStream::new(Vec::new());
        expected.write_packet("git-filter-server\n").unwrap();
        expected.write_packet("version=2\n").unwrap();
        expected.write_flush_packet().unwrap();
        expected.write_packet("capability=clean\n").unwrap();
        expected.write_packet("capability=smudge\n").unwrap();
        if with_delay {
            expected.write_packet("capability=delay\n").unwrap();
        }
        expected.write_flush_packet().unwrap();
        assert_eq!(server_output.into_inner(), expected.into_inner());
    }

    impl FakeFilter {
        fn record(&self, action: &str, pathname: &GitPath) {
            self.calls.borrow_mut().push(format!(
                "{}:{action}:{}",
                self.name,
                pathname.to_str_lossy()
            ));
        }

        fn consume_content(content: &mut PacketStream<impl std::io::Read>) -> Result<()> {
            content.forward_until_flush(&mut PacketStream::new(Vec::new()))
        }
    }

    impl GitFilterProcess for FakeFilter {
        fn clean(
            &mut self,
            pathname: &GitPath,
            content: &mut PacketStream<impl std::io::Read>,
            _output: &mut PacketStream<impl std::io::Write>,
        ) -> Result<()> {
            self.record("clean", pathname);
            Self::consume_content(content)
        }

        fn smudge(
            &mut self,
            pathname: &GitPath,
            _can_delay: bool,
            content: &mut PacketStream<impl std::io::Read>,
            _output: &mut PacketStream<impl std::io::Write>,
        ) -> Result<()> {
            self.record("smudge", pathname);
            Self::consume_content(content)
        }

        fn list_available_blobs(
            &mut self,
            _pathname_line_fn: impl FnMut(Vec<u8>) -> Result<()>,
        ) -> Result<String> {
            self.calls.borrow_mut().push(format!("{}:list", self.name));
            Ok("status=success".to_owned())
        }
    }

    struct FakeShardProvider {
        top: FakeFilter,
        subrepo: FakeFilter,
    }

    impl FakeShardProvider {
        fn new(calls: Rc<RefCell<Vec<String>>>) -> Self {
            Self {
                top: FakeFilter {
                    name: "top",
                    calls: calls.clone(),
                },
                subrepo: FakeFilter {
                    name: "subrepo",
                    calls,
                },
            }
        }
    }

    impl ShardProvider for FakeShardProvider {
        type Filter = FakeFilter;

        fn get_filter(&mut self, url: Option<&gix::Url>) -> Result<&mut Self::Filter> {
            Ok(if url.is_some() {
                &mut self.subrepo
            } else {
                &mut self.top
            })
        }

        fn for_each(
            &mut self,
            mut f: impl FnMut(&mut Self::Filter) -> Result<bool>,
        ) -> Result<bool> {
            if !f(&mut self.top)? {
                return Ok(false);
            }
            f(&mut self.subrepo)
        }
    }

    #[test]
    fn sharded_filter_routes_clean_and_smudge_and_lists_all_shards() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut filter = ShardingFilterProcess {
            sub_filter_provider: FakeShardProvider::new(calls.clone()),
        };
        let pathname = GitPath::from("file");
        let url = gix::Url::from_bytes(bstr::BStr::new(b"https://example.com/subrepo")).unwrap();
        let mut clean_content = PacketStream::new(Cursor::new(b"00000000".to_vec()));
        let mut smudge_content = PacketStream::new(Cursor::new(b"00000000".to_vec()));
        let mut output = PacketStream::new(Vec::new());

        filter
            .clean(&pathname, &mut clean_content, &mut output)
            .unwrap();
        filter
            .smudge(
                Some(&url),
                &pathname,
                false,
                &mut smudge_content,
                &mut output,
            )
            .unwrap();
        filter.list_available_blobs(&mut output).unwrap();

        assert_eq!(
            *calls.borrow(),
            [
                "top:clean:file",
                "subrepo:smudge:file",
                "top:list",
                "subrepo:list"
            ]
        );
    }

    #[test]
    fn request_loop_resolves_smudge_url_and_dispatches_commands() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut filter = ShardingFilterProcess {
            sub_filter_provider: FakeShardProvider::new(calls.clone()),
        };
        let url = gix::Url::from_bytes(bstr::BStr::new(b"https://example.com/subrepo")).unwrap();
        let mut input = Vec::new();
        macro_rules! packet {
            ($data:expr) => {{
                let data = $data.as_bytes();
                use std::io::Write as _;
                write!(&mut input, "{:04x}", data.len() + 4).unwrap();
                input.extend_from_slice(data);
            }};
        }
        packet!("command=clean\n");
        packet!("pathname=top-file\n");
        input.extend_from_slice(b"0000");
        packet!("blob");
        input.extend_from_slice(b"0000");
        packet!("command=smudge\n");
        packet!("pathname=sub-file\n");
        input.extend_from_slice(b"0000");
        packet!("blob");
        input.extend_from_slice(b"0000");

        let mut stdin = PacketStream::new(Cursor::new(input));
        let mut stdout = PacketStream::new(Vec::new());
        let mut resolved = Vec::new();
        run_lfs_filter_process_loop(
            &mut filter,
            |pathname| {
                resolved.push(pathname.to_str_lossy().to_string());
                Ok(Some(url.clone()))
            },
            &mut stdin,
            &mut stdout,
        )
        .unwrap();

        assert_eq!(resolved, ["sub-file"]);
        assert_eq!(
            *calls.borrow(),
            ["top:clean:top-file", "subrepo:smudge:sub-file"]
        );
    }
}
