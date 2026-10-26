use super::git_filter_process::GitFilterProcess;
use super::git_packet::GitPacket;
use super::packet_stream::PacketStream;
use crate::git::GitPath;
use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use bstr::ByteVec as _;
use itertools::Itertools as _;
use std::collections::HashSet;

pub(super) struct SubProcessFilter<TIn: std::io::Write, TOut: std::io::Read> {
    pub supports_delay: bool,
    pub stdin: PacketStream<TIn>,
    pub stdout: PacketStream<TOut>,
}

impl<TIn: std::io::Write, TOut: std::io::Read> SubProcessFilter<TIn, TOut> {
    pub fn new(stdin: PacketStream<TIn>, stdout: PacketStream<TOut>) -> Result<Self> {
        let mut ret = Self {
            supports_delay: false,
            stdin,
            stdout,
        };
        ret.version_handshake()
            .context("Handshake with 'git lfs filter-process' failed")?;
        ret.negotiate_capabilities()
            .context("Negotiating capabilities with 'git lfs filter-process' failed")?;
        Ok(ret)
    }

    pub fn into_inner(self) -> (PacketStream<TIn>, PacketStream<TOut>) {
        (self.stdin, self.stdout)
    }

    pub fn half_close(self) -> PacketStream<TOut> {
        let (stdin, stdout) = self.into_inner();
        drop(stdin);
        stdout
    }

    fn version_handshake(&mut self) -> Result<()> {
        self.stdin.write_packet("git-filter-client\n")?;
        self.stdin.write_packet("version=2\n")?;
        self.stdin.write_flush_packet()?;

        try {
            let packet = self.stdout.read_packet_text()?;
            if !matches!(packet, GitPacket::Data(ref line) if line == "git-filter-server") {
                anyhow::bail!("Got {packet:?}");
            }
        }
        .context("Expected 'git-filter-server'")?;
        try {
            let packet = self.stdout.read_packet_text()?;
            if !matches!(packet, GitPacket::Data(ref line) if line == "version=2") {
                anyhow::bail!("Got {packet:?}");
            }
        }
        .context("Expected 'version=2'")?;
        try {
            let packet = self.stdout.read_packet_text()?;
            if !matches!(packet, GitPacket::Flush) {
                anyhow::bail!("Got {packet:?}");
            }
        }
        .context("Bad version end")?;
        Ok(())
    }

    fn negotiate_capabilities(&mut self) -> Result<()> {
        self.stdin.write_packet("capability=clean\n")?;
        self.stdin.write_packet("capability=smudge\n")?;
        self.stdin.write_packet("capability=delay\n")?;
        self.stdin.write_flush_packet()?;
        let mut supported_capabilities = HashSet::new();
        loop {
            let packet = self.stdout.read_packet_text()?;
            match packet {
                GitPacket::Data(line) => try {
                    let cap = line
                        .strip_prefix(b"capability=")
                        .context("Missing 'capability=' prefix")?;
                    let cap = String::from_utf8(cap.to_vec())
                        .map_err(|err| anyhow::anyhow!("{err:#}"))?;
                    supported_capabilities.insert(cap);
                }
                .with_context(|| format!("Bad capability line {:?}", line.to_str_lossy()))?,
                GitPacket::Flush => break,
                GitPacket::EndOfFile => anyhow::bail!("Unexpected end of file"),
            }
        }
        for needed_cap in ["clean", "smudge"] {
            if !supported_capabilities.contains(needed_cap) {
                anyhow::bail!(
                    "Git LFS filter process must support '{needed_cap}' but only supports {:?}",
                    supported_capabilities.iter().sorted().collect_vec(),
                );
            }
        }
        self.supports_delay = supported_capabilities.contains("delay");
        Ok(())
    }
}

impl<TIn: std::io::Write, TOut: std::io::Read> GitFilterProcess for SubProcessFilter<TIn, TOut> {
    fn clean(
        &mut self,
        pathname: &GitPath,
        content_to_clean: &mut PacketStream<impl std::io::Read>,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        self.stdin.write_packet("command=clean\n")?;
        let mut packet = Vec::new();
        packet.extend_from_slice(b"pathname=");
        packet.extend_from_slice(pathname.as_slice());
        packet.extend_from_slice(b"\n");
        self.stdin.write_packet(&packet)?;
        self.stdin.write_flush_packet()?;
        content_to_clean
            .forward_until_flush(&mut self.stdin)
            .context("Forwarding clean request content")?;
        self.stdout
            .forward_until_flush(output)
            .context("Forwarding clean response header")?;
        self.stdout
            .forward_until_flush(output)
            .context("Forwarding clean response content")?;
        self.stdout
            .forward_until_flush(output)
            .context("Forwarding clean response status update")?;
        Ok(())
    }

    fn smudge(
        &mut self,
        pathname: &GitPath,
        can_delay: bool,
        content_to_smudge: &mut PacketStream<impl std::io::Read>,
        output: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        self.stdin.write_packet("command=smudge\n")?;
        let mut packet = Vec::new();
        packet.extend_from_slice(b"pathname=");
        packet.extend_from_slice(pathname.as_slice());
        packet.extend_from_slice(b"\n");
        self.stdin.write_packet(&packet)?;
        if self.supports_delay && can_delay {
            self.stdin.write_packet(b"can-delay=1\n")?;
        }
        self.stdin.write_flush_packet()?;
        content_to_smudge
            .forward_until_flush(&mut self.stdin)
            .context("Forwarding smudge request content")?;

        // Intercept the smudge response header to check for a delayed status.
        let mut delayed = false;
        try {
            loop {
                match self.stdout.read_packet()? {
                    GitPacket::Data(data) => {
                        if data.trim_ascii_end() == b"status=delayed" {
                            delayed = true;
                        }
                        output.write_packet(data)?;
                    }
                    GitPacket::Flush => {
                        output.write_flush_packet()?;
                        break;
                    }
                    GitPacket::EndOfFile => anyhow::bail!("Unexpected end of file"),
                }
            }
        }
        .context("Forwarding smudge response header")?;
        if delayed {
            // Git does not expect any content or trailing status for delayed
            // responses, the blob is fetched later with a new smudge request.
            return Ok(());
        }
        self.stdout
            .forward_until_flush(output)
            .context("Forwarding smudge response content")?;
        self.stdout
            .forward_until_flush(output)
            .context("Forwarding smudge response status update")?;
        Ok(())
    }

    fn list_available_blobs(
        &mut self,
        mut pathname_line_fn: impl FnMut(Vec<u8>) -> Result<()>,
    ) -> Result<String> {
        if !self.supports_delay {
            // Nothing to report, nothing should have been delayed.
            return Ok("status=success".to_string());
        }
        self.stdin.write_packet("command=list_available_blobs\n")?;
        self.stdin.write_flush_packet()?;
        loop {
            let packet = self.stdout.read_packet()?;
            match packet {
                GitPacket::Data(data) => pathname_line_fn(data)?,
                GitPacket::Flush => break,
                GitPacket::EndOfFile => anyhow::bail!("Unexpected end of file"),
            }
        }
        let packet = self.stdout.read_packet_text()?;
        let GitPacket::Data(status_line) = packet else {
            anyhow::bail!("Bad status line: {packet:?}");
        };
        let status_line = status_line
            .to_vec()
            .into_string()
            .with_context(|| format!("Status line {status_line:?}"))?;
        let packet = self.stdout.read_packet()?;
        if let GitPacket::Flush = packet {
        } else {
            anyhow::bail!("Expected flush packet after status line: {packet:?}");
        }
        Ok(status_line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bstr::ByteSlice;
    use rstest::rstest;
    use std::io::Cursor;

    const fn handshake_to_child() -> &'static str {
        "0016git-filter-client\n\
000eversion=2\n\
0000\
0015capability=clean\n\
0016capability=smudge\n\
0015capability=delay\n\
0000"
    }

    fn handshake_from_child(capabilities: &[&str]) -> Vec<u8> {
        let mut stream = PacketStream::new(Vec::new());
        stream.write_packet("git-filter-server\n").unwrap();
        stream.write_packet("version=2\n").unwrap();
        stream.write_flush_packet().unwrap();
        for capability in capabilities {
            stream
                .write_packet(format!("capability={capability}\n"))
                .unwrap();
        }
        stream.write_flush_packet().unwrap();
        stream.into_inner()
    }

    #[rstest]
    #[case::can_delay(true, &["clean", "smudge", "delay"])]
    #[case::no_delay(false, &["clean", "smudge"])]
    fn negotiates_handshake_and_required_capabilities(
        #[case] can_delay: bool,
        #[case] capabilities: &[&str],
    ) {
        let expected_child_request = handshake_to_child();
        let process = SubProcessFilter::new(
            PacketStream::new(Vec::new()),
            PacketStream::new(Cursor::new(handshake_from_child(capabilities))),
        )
        .unwrap();
        assert_eq!(process.supports_delay, can_delay);
        let (actual_child_request_stream, _) = process.into_inner();
        let actual_child_request = actual_child_request_stream.into_inner();

        assert_eq!(
            actual_child_request.to_str().unwrap(),
            expected_child_request,
        );
    }

    #[rstest]
    fn rejects_handshake_limited_capability(
        #[values(
            &["smudge"],
            &["delay", "smudge"],
            &["clean"],
            &["clean", "delay"],
        )]
        capabilities: &[&str],
    ) {
        let result = SubProcessFilter::new(
            PacketStream::new(Vec::new()),
            PacketStream::new(Cursor::new(handshake_from_child(capabilities))),
        );
        assert!(result.is_err());
        let Err(err) = result else { unreachable!() };
        let err_str = format!("{err:#}");
        assert!(
            err_str.starts_with("Negotiating capabilities with 'git lfs filter-process' failed: Git LFS filter process must support "),
            "err_str={err_str}"
        );
        assert!(
            err_str.contains(&format!("{capabilities:?}")),
            "err_str={err_str}, capabilities={capabilities:?}"
        );
    }

    #[test]
    fn clean_forwards_request_content_and_response_packets() {
        let mut response_from_child = PacketStream::new(handshake_from_child(&["clean", "smudge"]));
        response_from_child
            .write_packet("status=success\n")
            .unwrap();
        response_from_child.write_flush_packet().unwrap();
        response_from_child.write_packet("cleaned\n").unwrap();
        response_from_child.write_flush_packet().unwrap();
        response_from_child
            .write_packet("status=success\n")
            .unwrap();
        response_from_child.write_flush_packet().unwrap();
        let mut process = SubProcessFilter::new(
            PacketStream::new(Vec::new()),
            PacketStream::new(Cursor::new(response_from_child.into_inner())),
        )
        .unwrap();

        let mut content = PacketStream::new(Cursor::new(b"0008blob0000".to_vec()));
        let mut to_caller = PacketStream::new(Vec::new());
        process
            .clean(&GitPath::from("file"), &mut content, &mut to_caller)
            .unwrap();
        let actual_child_request = process.stdin.into_inner();
        let actual_caller_response = to_caller.into_inner();

        let expected_child_request = format!(
            "{}\
0012command=clean\n\
0012pathname=file\n\
0000\
0008blob\
0000",
            handshake_to_child()
        );
        assert_eq!(
            actual_child_request.to_str().unwrap(),
            expected_child_request
        );

        let expected_caller_response = "\
0013status=success\n\
0000\
000ccleaned\n\
0000\
0013status=success\n\
0000";
        assert_eq!(
            actual_caller_response.to_str().unwrap(),
            expected_caller_response
        );
    }

    #[test]
    fn lists_available_blobs_when_delay_is_supported() {
        let mut child_response =
            PacketStream::new(handshake_from_child(&["clean", "smudge", "delay"]));
        child_response.write_packet("file\n").unwrap();
        child_response.write_flush_packet().unwrap();
        child_response.write_packet("status=success\n").unwrap();
        child_response.write_flush_packet().unwrap();
        let mut process = SubProcessFilter::new(
            PacketStream::new(Vec::new()),
            PacketStream::new(Cursor::new(child_response.into_inner())),
        )
        .unwrap();
        let mut paths = Vec::new();

        let status = process
            .list_available_blobs(|path| {
                paths.push(path);
                Ok(())
            })
            .unwrap();

        let actual_child_request = process.stdin.into_inner();
        assert_eq!(paths, [b"file\n".to_vec()]);
        assert_eq!(status, "status=success");

        let expected_child_request = "\
0016git-filter-client\n\
000eversion=2\n\
0000\
0015capability=clean\n\
0016capability=smudge\n\
0015capability=delay\n\
0000\
0021command=list_available_blobs\n\
0000";
        assert_eq!(
            actual_child_request.to_str().unwrap(),
            expected_child_request
        );
    }

    #[test]
    fn lists_available_blobs_noop_when_delay_is_unsupported() {
        let child_response = PacketStream::new(handshake_from_child(&["clean", "smudge"]));
        let mut process = SubProcessFilter::new(
            PacketStream::new(Vec::new()),
            PacketStream::new(Cursor::new(child_response.into_inner())),
        )
        .unwrap();

        let status = process
            .list_available_blobs(|_path| {
                unreachable!("Should not be called when delay is unsupported");
            })
            .unwrap();
        assert_eq!(status, "status=success");
    }
}
