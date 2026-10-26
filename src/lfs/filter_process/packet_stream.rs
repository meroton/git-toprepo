use super::filter_command::FilterCommand;
use super::git_packet::GitPacket;
use anyhow::Context as _;
use anyhow::Result;
use bstr::BString;
use bstr::ByteSlice as _;
use bstr::ByteVec as _;
use std::collections::HashMap;

pub(super) struct PacketStream<T> {
    inner: T,
}

impl<T> PacketStream<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    #[cfg(test)]
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: std::io::Read> PacketStream<T> {
    pub fn read_packet(&mut self) -> Result<GitPacket<Vec<u8>>> {
        let mut pkt_size_hex = [0u8; 4];
        let n = self.inner.read(&mut pkt_size_hex)?;
        if n == 0 {
            return Ok(GitPacket::EndOfFile);
        }
        self.inner
            .read_exact(&mut pkt_size_hex[n..])
            .context("Reading packet size")?;
        let pkt_size = u32::from_str_radix(std::str::from_utf8(&pkt_size_hex)?, 16)?;
        if pkt_size == 0 {
            return Ok(GitPacket::Flush);
        }
        if pkt_size <= 4 {
            anyhow::bail!("Invalid packet size: {}", pkt_size);
        }
        let content_size = (pkt_size - 4) as usize;
        let mut content = vec![0u8; content_size];
        self.inner.read_exact(&mut content)?;
        Ok(GitPacket::Data(content))
    }

    pub fn read_packet_text(&mut self) -> Result<GitPacket<BString>> {
        Ok(match self.read_packet()? {
            GitPacket::Data(mut content) => {
                content.pop_if(|c| *c == b'\n');
                GitPacket::Data(BString::from(content))
            }
            GitPacket::Flush => GitPacket::Flush,
            GitPacket::EndOfFile => GitPacket::EndOfFile,
        })
    }

    fn read_request(&mut self) -> Result<Option<HashMap<String, BString>>> {
        let mut request = HashMap::new();
        loop {
            match self.read_packet_text()? {
                GitPacket::EndOfFile => {
                    if !request.is_empty() {
                        anyhow::bail!("Unexpected end of file");
                    }
                    return Ok(None);
                }
                GitPacket::Flush => return Ok(Some(request)),
                GitPacket::Data(mut line) => {
                    let idx = line
                        .find_byte(b'=')
                        .with_context(|| format!("Invalid request line {line:?}"))?;
                    let value = BString::from(line.split_off(idx + 1));
                    line.truncate(idx);
                    let key = line
                        .to_vec()
                        .into_string()
                        .with_context(|| format!("Bad key {:?}", line.to_str_lossy()))?;
                    match request.entry(key) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(value);
                        }
                        std::collections::hash_map::Entry::Occupied(entry) => {
                            anyhow::bail!(
                                "Duplicate key {:?} in request: {value:?} and {:?}",
                                entry.key(),
                                entry.get()
                            );
                        }
                    }
                }
            }
        }
    }

    pub fn read_filter_process_request(&mut self) -> Result<FilterCommand> {
        let Some(mut request_map) = self.read_request()? else {
            return Ok(FilterCommand::EndOfFile);
        };
        let command = request_map
            .remove("command")
            .with_context(|| format!("Missing command in request: {request_map:?}"))?;
        let request = match command.to_str().unwrap_or_default() {
            "clean" => FilterCommand::Clean {
                pathname: request_map
                    .remove("pathname")
                    .with_context(|| format!("Missing pathname in request: {request_map:?}"))?
                    .into(),
            },
            "smudge" => {
                let can_delay_str = request_map.remove("can-delay");
                let can_delay = match can_delay_str {
                    Some(v) if v == b"1" => true,
                    Some(v) if v == b"0" => false,
                    None => false,
                    _ => anyhow::bail!("Invalid can-delay value: {can_delay_str:?}"),
                };
                FilterCommand::Smudge {
                    pathname: request_map
                        .remove("pathname")
                        .with_context(|| format!("Missing pathname in request: {request_map:?}"))?
                        .into(),
                    can_delay,
                }
            }
            "list_available_blobs" => FilterCommand::ListAvailableBlobs,
            _ => anyhow::bail!("Unknown command: {command:?}"),
        };
        Ok(request)
    }

    pub fn forward_until_flush(
        &mut self,
        writer: &mut PacketStream<impl std::io::Write>,
    ) -> Result<()> {
        loop {
            let packet = self.read_packet()?;
            match packet {
                GitPacket::Data(data) => writer.write_packet(data)?,
                GitPacket::Flush => {
                    writer.write_flush_packet()?;
                    break;
                }
                GitPacket::EndOfFile => anyhow::bail!("Unexpected end of file"),
            }
        }
        Ok(())
    }
}

impl<T: std::io::Write> PacketStream<T> {
    pub fn write_packet(&mut self, data: impl AsRef<[u8]>) -> Result<()> {
        let data = data.as_ref();
        let pkt_size = (data.len() + 4) as u32;
        write!(self.inner, "{:04x}", pkt_size)?;
        self.inner.write_all(data)?;
        Ok(())
    }

    pub fn write_flush_packet(&mut self) -> Result<()> {
        write!(self.inner, "0000")?;
        self.inner.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn packet_stream_writes_data_and_flush_packets() {
        let mut stream = PacketStream::new(Vec::new());
        stream.write_packet(b"hello\n").unwrap();
        stream.write_flush_packet().unwrap();
        assert_eq!(stream.inner, b"000ahello\n0000");
    }

    #[test]
    fn packet_stream_reads_data_flush_and_eof() {
        let mut stream = PacketStream::new(Cursor::new(b"000ahello\n0000".to_vec()));
        assert!(matches!(
            stream.read_packet().unwrap(),
            GitPacket::Data(data) if data == b"hello\n"
        ));
        assert!(matches!(stream.read_packet().unwrap(), GitPacket::Flush));
        assert!(matches!(
            stream.read_packet().unwrap(),
            GitPacket::EndOfFile
        ));
    }

    #[test]
    fn packet_stream_read_packet_text_removes_one_trailing_newline() {
        let mut stream = PacketStream::new(Cursor::new(b"000ahello\n".to_vec()));
        assert!(matches!(
            stream.read_packet_text().unwrap(),
            GitPacket::Data(data) if data == "hello"
        ));
    }

    #[test]
    fn packet_stream_rejects_invalid_and_truncated_packets() {
        for input in [b"0003".as_slice(), b"00", b"0008hi"] {
            let mut stream = PacketStream::new(Cursor::new(input.to_vec()));
            assert!(stream.read_packet().is_err(), "accepted packet {input:?}");
        }
    }

    #[test]
    fn forward_until_flush_copies_packets_through_flush() {
        let mut reader = PacketStream::new(Cursor::new(b"0005a0006bc00000005d".to_vec()));
        let mut writer = PacketStream::new(Vec::new());
        reader.forward_until_flush(&mut writer).unwrap();
        assert_eq!(writer.inner, b"0005a0006bc0000");
        assert!(matches!(
            reader.read_packet().unwrap(),
            GitPacket::Data(data) if data == b"d"
        ));
    }

    #[test]
    fn forward_until_flush_rejects_eof_before_flush() {
        let mut reader = PacketStream::new(Cursor::new(b"0005a".to_vec()));
        let mut writer = PacketStream::new(Vec::new());
        assert!(reader.forward_until_flush(&mut writer).is_err());
        assert_eq!(writer.inner, b"0005a");
    }
}
