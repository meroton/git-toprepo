use anyhow::{anyhow, bail, Context, Result};
use bstr::ByteSlice;
use itertools::Itertools;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::Stdio;

use crate::util::git_command;

#[derive(Debug, Default)]
pub struct FastExportCommit {
    pub branch: Vec<u8>,
    pub author_info: Vec<u8>,
    pub committer_info: Vec<u8>,
    pub message: Vec<u8>,
    pub file_changes: Vec<ChangedFile>,
    pub parents: Vec<Vec<u8>>,
    pub original_id: Vec<u8>,
    pub encoding: Option<Vec<u8>>,
}

impl FastExportCommit {
    pub fn get_timestamp(&self) -> chrono::DateTime<chrono::Utc> {
        let substrings = self.committer_info.split_str(b" ").collect_vec();

        // For the default (`raw`) date format, the second to last word of the commit command should always be the timestamp.
        // The last word (UTC offset) does not affect the timestamp.
        // Example committer command: `committer C Name <c@no.domain> 1686121750 +0100`
        // https://git-scm.com/docs/git-fast-import#Documentation/git-fast-import.txt-coderawcode
        let timestamp: i64 = substrings
            .get(substrings.len() - 2)
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();

        chrono::DateTime::from_timestamp(timestamp, 0).unwrap()
    }
}

#[derive(Debug, PartialEq)]
pub enum ChangedFile {
    Deleted(FileDelete),
    Modified(FileModify),
}

#[derive(Debug, PartialEq)]
pub struct FileModify {
    pub path: Vec<u8>,
    pub mode: Vec<u8>,
    pub hash: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub struct FileDelete {
    pub path: Vec<u8>,
}

#[derive(Debug)]
pub struct FastExportRepo {
    mark_oid_map: HashMap<Vec<u8>, Vec<u8>>,
    reader: BufReader<std::process::ChildStdout>,
    current_line: Vec<u8>,
}

impl FastExportRepo {
    pub fn load_from_path(repo_dir: &Path, refs: Option<Vec<&str>>) -> Result<Self> {
        let mut cmd = git_command(&repo_dir);
        cmd.args([
            "fast-export",
            "--no-data",
            "--use-done-feature",
            "--show-original-ids",
            "--reference-excluded-parents",
            "--signed-tags=strip",
        ]);
        match refs {
            Some(refs) => {
                cmd.arg("--").args(refs);
            }
            None => {
                cmd.arg("--all").arg("--");
            }
        }
        let stdout = cmd
            .stdout(Stdio::piped())
            .spawn()?
            // Just release the process. git-fast-export will terminate when
            // stdout is closed.
            .stdout
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Could not capture standard output.",
                )
            })?;

        Ok(FastExportRepo {
            mark_oid_map: HashMap::new(),
            reader: BufReader::new(stdout),
            current_line: Vec::new(),
        })
    }

    /// Reads the next line from git-fast-export output and stores it in
    /// `current_line` without the trailing newline.
    fn advance_line_or_eof(&mut self) -> Result<usize> {
        self.current_line.clear();
        let bytes = self.reader.read_until(b'\n', &mut self.current_line)?;
        if bytes == 0 {
            // EOF
        } else if (bytes == 1 && self.current_line[0] == b'\n')
            || (bytes >= 2
                // Check that the LF is not part of a multi-byte character.
                && self.current_line[bytes - 1] == b'\n'
                && self.current_line[bytes - 2].is_ascii())
        {
            self.current_line.truncate(bytes - 1);
        } else {
            bail!(
                "Expected newline at the end of the line, found {:?}",
                self.current_line
            );
        }
        Ok(bytes)
    }

    /// Same as `advance_line`, but returns an `Err` if the end of the file is reached.
    fn must_advance_line(&mut self) -> Result<()> {
        let bytes = self.advance_line_or_eof()?;
        if bytes == 0 {
            bail!(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected end of file",
            ));
        }
        Ok(())
    }

    fn read_bytes(&mut self, buf: &mut Vec<u8>, bytes: usize) -> Result<()> {
        *buf = vec![0u8; bytes];
        self.reader.read_exact(buf)?;
        Ok(())
    }

    /// Reads a 'commit' command according to documentation in git-fast-import.
    fn read_commit(&mut self) -> Result<FastExportCommit> {
        let mut commit = FastExportCommit::default();
        commit.branch = self
            .current_line
            .strip_prefix(b"commit ")
            .ok_or(anyhow!("Expected 'commit' line"))?
            .into();
        self.must_advance_line()?;

        // TODO: Convert to a GitFastExportMark struct.
        let opt_mark = self
            .current_line
            .strip_prefix(b"mark :")
            .map(|mark| mark.into());
        if opt_mark.is_some() {
            self.must_advance_line()?;
        }

        // TODO: Convert to a CommitHash struct.
        commit.original_id = self
            .current_line
            .strip_prefix(b"original-oid ")
            .ok_or(anyhow!("Expected 'original-oid' line"))?
            .into();
        if let Some(mark) = opt_mark {
            self.mark_oid_map.insert(mark, commit.original_id.clone());
        }
        self.must_advance_line()?;

        let author_info_opt = self.current_line.strip_prefix(b"author ").map(|a| a.into());
        if author_info_opt.is_some() {
            self.must_advance_line()?;
        }
        commit.committer_info = self
            .current_line
            .strip_prefix(b"committer ")
            .ok_or(anyhow!("Expected 'committer' line"))?
            .into();
        self.must_advance_line()?;
        commit.author_info = author_info_opt.unwrap_or_else(|| commit.committer_info.clone());

        commit.encoding = self
            .current_line
            .strip_prefix(b"encoding ")
            .map(|e| e.into());
        if commit.encoding.is_some() {
            self.must_advance_line()?;
        }

        let msg_byte_count = self
            .current_line
            .strip_prefix(b"data ")
            .ok_or(anyhow!("Expected 'data' line"))?
            .to_str()?
            .parse::<usize>()
            .context("Could not parse commit message size")?;
        self.read_bytes(&mut commit.message, msg_byte_count)
            .context("Error reading commit message")?;
        self.must_advance_line()?;
        if self.current_line.is_empty() {
            // Optional newline after commit message detected.
            self.must_advance_line()?;
        }
        if self.current_line.is_empty() {
            // Optional additional newline if there is nothing more.
            self.must_advance_line()?;
        }

        if let Some(first_parent) = self.current_line.strip_prefix(b"from ") {
            let parent_hash = if first_parent.get(0) == Some(&b':') {
                let mark = &first_parent[1..];
                self.mark_oid_map
                    .get(mark)
                    .ok_or(anyhow!(
                        "Could not find parent mark {}",
                        first_parent.to_str_lossy()
                    ))?
                    .clone()
            } else {
                first_parent.into()
            };
            commit.parents.push(parent_hash);
            self.must_advance_line()?;
        } else if self.current_line.starts_with(b"merge ") {
            bail!("'merge' line without 'from' line is not supported");
        }
        while self.current_line.starts_with(b"merge ") {
            let parent = self.current_line.strip_prefix(b"merge ").unwrap();
            let parent_hash = if parent.get(0) == Some(&b':') {
                let mark = &parent[1..];
                self.mark_oid_map
                    .get(mark)
                    .ok_or(anyhow!(
                        "Could not find parent mark {}",
                        parent.to_str_lossy()
                    ))?
                    .clone()
            } else {
                parent.into()
            };
            commit.parents.push(parent_hash);
            self.must_advance_line()?;
        }

        loop {
            let changed_file = if let Some(change_data) = self.current_line.strip_prefix(b"M ") {
                // filemodify: 'M' SP <mode> SP <dataref> SP <path> LF
                let (mode, hash, path) =
                    change_data
                        .splitn_str(3, b" ")
                        .collect_tuple()
                        .ok_or(anyhow!(
                            "Expected 3 parts in filemodify line {:?}",
                            self.current_line
                        ))?;
                ChangedFile::Modified(FileModify {
                    path: path.to_vec(),
                    mode: mode.to_vec(),
                    hash: hash.to_vec(),
                })
            } else if let Some(change_data) = self.current_line.strip_prefix(b"D ") {
                // filedelete: 'D' SP <path> LF
                ChangedFile::Deleted(FileDelete {
                    path: change_data.to_vec(),
                })
            } else if let Some(_change_data) = self.current_line.strip_prefix(b"C ") {
                // filecopy: 'C' SP <path> SP <path> LF
                // Should not happen when git-fast-export is called without -C.
                bail!("filecopy line is not supported");
            } else if let Some(_change_data) = self.current_line.strip_prefix(b"R ") {
                // filerename: 'R' SP <path> SP <path> LF
                // Should not happen when git-fast-export is called without -R.
                bail!("filerename line is not supported");
            } else if self.current_line == b"deleteall" {
                // filedeleteall: 'deleteall' LF
                bail!("filedeleteall line is not supported");
            } else if let Some(_change_data) = self.current_line.strip_prefix(b"N ") {
                // notemodify: 'N' SP <dataref> SP <commit-ish> LF
                bail!("notemodify line is not supported");
            } else {
                break;
            };
            commit.file_changes.push(changed_file);
            self.must_advance_line()?;
        }
        return Ok(commit);
    }
}

impl Iterator for FastExportRepo {
    type Item = Result<FastExportCommit>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_line.is_empty() {
                // noop
            } else if self.current_line == b"done" {
                break;
            } else if self.current_line == b"feature done" {
                // noop
            } else if self.current_line.starts_with(b"commit ") {
                return Some(self.read_commit().with_context(|| {
                    format!(
                        "Error parsing git-fast-export commit line {:?}",
                        self.current_line.to_str_lossy()
                    )
                }));
            } else if self.current_line.starts_with(b"reset ") {
                // Unintresting command.
            } else {
                return Some(Err(anyhow!(
                    "Unexpected git-fast-export command: {}",
                    self.current_line.to_str_lossy()
                )));
            }
            match self.advance_line_or_eof() {
                // EOF
                Ok(0) => break,
                // Normal line.
                Ok(_) => {}
                // Error, stop immediately.
                Err(err) => return Some(Err(err)),
            };
        }
        None
    }
}

pub fn export_to_fast_import<W, I>(buf: &mut W, commit_container: &mut I) -> Result<()>
where
    W: Write,
    I: Iterator<Item = Result<FastExportCommit>>,
{
    let mut mark_counter: usize = 1;
    let mut oid_mark_map: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    buf.write_all(b"feature done\n")?;

    for commit in commit_container.by_ref() {
        let commit = commit?;
        if commit.parents.is_empty() {
            buf.write_all(b"reset ")?;
            buf.write_all(&commit.branch)?;
            buf.write_all(b"\n")?;
        }

        buf.write_all(b"commit ")?;
        buf.write_all(&commit.branch)?;
        buf.write_all(b"\n")?;

        let mark_as_bytes = mark_counter.to_string().as_bytes().to_vec();
        buf.write_all(b"mark :")?;
        buf.write_all(&mark_as_bytes)?;
        buf.write_all(b"\n")?;
        oid_mark_map.insert(commit.original_id.clone(), mark_as_bytes.clone());
        mark_counter += 1;

        buf.write_all(b"author ")?;
        buf.write_all(&commit.author_info)?;
        buf.write_all(b"\n")?;

        buf.write_all(b"committer ")?;
        buf.write_all(&commit.committer_info)?;
        buf.write_all(b"\n")?;

        buf.write_all(b"data ")?;
        buf.write_all(&commit.message.len().to_string().as_bytes())?;
        buf.write_all(b"\n")?;
        buf.write_all(&commit.message)?;

        if !commit.parents.is_empty() {
            buf.write_all(b"from :")?;
            buf.write_all(&oid_mark_map.get(commit.parents.first().unwrap()).unwrap())?;
            buf.write_all(b"\n")?;

            for parent in commit.parents[1..].iter() {
                buf.write_all(b"merge :")?;
                buf.write_all(&oid_mark_map.get(parent).unwrap())?;
                buf.write_all(b"\n")?;
            }
        }

        for changed_file in commit.file_changes {
            match changed_file {
                ChangedFile::Modified(changed_file) => {
                    buf.write_all(b"M ")?;
                    buf.write_all(&changed_file.mode)?;
                    buf.write_all(b" ")?;
                    buf.write_all(&changed_file.hash)?;
                    buf.write_all(b" ")?;
                    buf.write_all(&changed_file.path)?;
                }
                ChangedFile::Deleted(changed_file) => {
                    buf.write_all(b"D ")?;
                    buf.write_all(&changed_file.path)?;
                }
            }
            buf.write_all(b"\n")?;
        }
        buf.write_all(b"\n")?;
    }
    buf.write_all(b"done\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fast_export_output() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();

        let example_repo = crate::util::GitTopRepoExample::new(&tmp_path);
        let example_top_repo = example_repo.init_server_top();

        let mut repo = FastExportRepo::load_from_path(example_top_repo.as_path(), None).unwrap();
        let commit_a = repo.next().unwrap().unwrap();
        let commit_d = repo.nth(2).unwrap().unwrap();

        assert_eq!(commit_a.branch, b"refs/heads/main");
        assert_eq!(
            commit_a.author_info,
            b"A Name <a@no.domain> 1672625045 +0100"
        );
        assert_eq!(
            commit_a.committer_info,
            b"C Name <c@no.domain> 1686121750 +0100"
        );
        assert_eq!(commit_a.message, b"A\n");
        assert!(commit_a.file_changes.is_empty());
        assert!(commit_a.parents.is_empty());
        assert_eq!(
            commit_a.original_id,
            b"6fc12aa7d6d06400a70bb522244bb184e3678416"
        );
        assert_eq!(commit_a.encoding, None);

        assert_eq!(commit_d.branch, b"refs/heads/main");
        assert_eq!(
            commit_d.author_info,
            b"A Name <a@no.domain> 1672625045 +0100"
        );
        assert_eq!(
            commit_d.committer_info,
            b"C Name <c@no.domain> 1686121750 +0100"
        );
        assert_eq!(commit_d.message, b"D\n");
        assert_eq!(
            // CommitInfo and DiffStatus implements PartialEq trait for this test,
            // might exclude PartialEq and this test if it's not needed elsewhere.
            commit_d.file_changes.first().unwrap(),
            &ChangedFile::Modified(FileModify {
                path: Vec::from(b"sub"),
                mode: Vec::from(b"160000"),
                hash: Vec::from(b"eeb85c77b614a7ec060f6df5825c9a5c10414307"),
            })
        );
        assert_eq!(
            commit_d.parents,
            Vec::from([b"ec67a8703750336a938bef740115009b6310892f"])
        );
        assert_eq!(
            commit_d.original_id,
            b"9f781a9707757573b16ee5946ab147e4e66857bc"
        );
        assert_eq!(commit_d.encoding, None);
    }

    #[test]
    fn test_fast_import() {
        use crate::util::git_command;
        use crate::util::GitTopRepoExample;
        use std::io::Write;
        use tempfile::tempdir;

        let temp_dir = tempdir().unwrap();
        let from_repo_path =
            GitTopRepoExample::new(&temp_dir.path().join("from")).init_server_top();
        let to_repo_path = temp_dir.path().join("to");
        std::fs::create_dir(&to_repo_path).unwrap();

        git_command(&to_repo_path).args(["init"]).output().unwrap();
        std::fs::copy(
            from_repo_path.as_path().join(".gitmodules"),
            to_repo_path.as_path().join(".gitmodules"),
        )
        .unwrap();
        git_command(&to_repo_path)
            .args(["add", ".gitmodules"])
            .output()
            .unwrap();

        let mut fast_export_repo =
            FastExportRepo::load_from_path(from_repo_path.as_path(), None).unwrap();

        let mut fast_import_input: Vec<u8> = Vec::new();
        export_to_fast_import(&mut fast_import_input, &mut fast_export_repo).unwrap();

        let mut child = git_command(&to_repo_path)
            .args(["fast-import", "--done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Could not start fast-import process");

        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            stdin
                .write_all(&fast_import_input)
                .expect("Could not write to stdin");
        });

        child.wait().expect("Could not read stdin");

        let from_ref = git_command(&from_repo_path)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap()
            .stdout
            .to_str()
            .unwrap()
            .to_string();

        let to_ref = git_command(&to_repo_path)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap()
            .stdout
            .to_str()
            .unwrap()
            .to_string();

        assert_eq!(from_ref, to_ref);
    }
}
