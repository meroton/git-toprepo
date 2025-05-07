use crate::git::git_command;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools;
use serde::Deserialize as _;
use serde::Serialize as _;
use serde_with::serde_as;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::string::ToString as _;

#[derive(Debug)]
pub struct FastExportCommit {
    pub branch: Option<FullName>,
    pub author_info: BString,
    pub committer_info: BString,
    pub encoding: Option<BString>,
    pub message: BString,
    pub file_changes: Vec<ChangedFile>,
    pub parents: Vec<gix::ObjectId>,
    pub original_id: gix::ObjectId,
}

#[derive(Debug)]
pub struct FastExportReset {
    pub branch: FullName,
    pub from: gix::ObjectId,
}

pub enum FastExportEntry {
    Commit(FastExportCommit),
    Reset(FastExportReset),
}

#[derive(Debug)]
pub struct FastImportCommit<'a> {
    pub branch: &'a FullNameRef,
    pub author_info: BString,
    pub committer_info: BString,
    pub encoding: Option<BString>,
    pub message: BString,
    pub file_changes: Vec<ChangedFile>,
    pub parents: Vec<ImportCommitRef>,
    pub original_id: Option<gix::ObjectId>,
}

impl FastExportCommit {
    pub fn get_committer_timestamp(&self) -> chrono::DateTime<chrono::Utc> {
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

    /// Creates a commit id but without the committer information.
    pub fn hash_without_committer(&self) -> Result<WithoutCommitterId> {
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update_bstring(&self.author_info);
        // Self::hash_bstring(&entry.committer_info);
        hasher.update_option_bstring(&self.encoding);
        hasher.update_bstring(&self.message);
        hasher.update_serde(&self.file_changes)?;
        hasher.update_serde(&self.parents)?;
        Ok(WithoutCommitterId(hasher.try_finalize()?))
    }
}

#[derive(Debug)]
pub enum ImportCommitRef {
    Mark(usize),
    CommitId(gix::ObjectId),
}

impl std::fmt::Display for ImportCommitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ImportCommitRef::Mark(mark) => write!(f, ":{mark}"),
            ImportCommitRef::CommitId(oid) => write!(f, "{oid}"),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize)]
pub struct ChangedFile {
    pub path: BString,
    pub change: FileChange,
}

#[derive(Debug, PartialEq, serde::Serialize)]
pub enum FileChange {
    Deleted,
    Modified { mode: BString, hash: BString },
}

#[derive(Debug)]
pub struct FastExportRepo {
    mark_oid_map: HashMap<BString, gix::ObjectId>,
    reader: BufReader<std::process::ChildStdout>,
    current_line: BString,
}

impl FastExportRepo {
    pub fn load_from_path_all_refs(repo_dir: &Path, logger: crate::log::Logger) -> Result<Self> {
        Self::load_from_path(repo_dir, Option::<Vec<&str>>::None, logger)
    }

    pub fn load_from_path<I, S>(
        repo_dir: &Path,
        refs: Option<I>,
        logger: crate::log::Logger,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let logger = logger.with_context("git-fast-export");
        let mut cmd = git_command(repo_dir);
        cmd.args([
            "fast-export",
            "--no-data",
            "--use-done-feature",
            "--show-original-ids",
            "--reference-excluded-parents",
            "--signed-tags=strip",
            "--reencode=no",
            "--tag-of-filtered-object=drop",
        ]);
        match refs {
            Some(refs) => {
                cmd.arg("--").args(refs);
            }
            None => {
                cmd.arg("--all").arg("--");
            }
        }
        let mut process = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Just release the process. git-fast-export will terminate when
        // stdout is closed.
        let stdout = process.stdout.take().context("Could not capture stdout")?;
        let mut stderr_reader =
            BufReader::new(process.stderr.take().context("Could not capture stderr")?);
        std::thread::Builder::new()
            .name("git-fast-export-stderr".into())
            .spawn(move || {
                for line in crate::util::ReadLossyCrOrLfLines::new(&mut stderr_reader) {
                    logger.warning(line.trim_end().to_owned());
                }
            })
            .expect("failed to spawn thread");

        Ok(FastExportRepo {
            mark_oid_map: HashMap::new(),
            reader: BufReader::new(stdout),
            current_line: BString::new(vec![]),
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
        let branch = self
            .current_line
            .strip_prefix(b"commit ")
            .ok_or_else(|| anyhow!("Expected 'commit' line"))?
            .as_bstr();
        let maybe_branch = match FullName::try_from(branch) {
            Ok(branch) => Some(branch),
            Err(_err) => {
                gix::ObjectId::from_hex(branch)?;
                None
            }
        };
        let mut commit = FastExportCommit {
            branch: maybe_branch,
            author_info: BString::new(vec![]),
            committer_info: BString::new(vec![]),
            encoding: None,
            message: BString::new(vec![]),
            file_changes: Vec::new(),
            parents: Vec::new(),
            original_id: gix::ObjectId::empty_blob(gix::hash::Kind::Sha1),
        };
        self.must_advance_line()?;

        // TODO: Convert to a GitFastExportMark struct.
        let opt_mark = self
            .current_line
            .strip_prefix(b"mark :")
            .map(|mark| mark.into());
        if opt_mark.is_some() {
            self.must_advance_line()?;
        }

        commit.original_id = gix::ObjectId::from_hex(
            self.current_line
                .strip_prefix(b"original-oid ")
                .ok_or_else(|| anyhow!("Expected 'original-oid' line"))?,
        )?;
        if let Some(mark) = opt_mark {
            self.mark_oid_map.insert(mark, commit.original_id);
        }
        self.must_advance_line()?;

        let author_info_opt = self.current_line.strip_prefix(b"author ").map(|a| a.into());
        if author_info_opt.is_some() {
            self.must_advance_line()?;
        }
        commit.committer_info = self
            .current_line
            .strip_prefix(b"committer ")
            .ok_or_else(|| anyhow!("Expected 'committer' line"))?
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
            .ok_or_else(|| anyhow!("Expected 'data' line"))?
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

        if let Some(first_parent) = self.current_line.strip_prefix(b"from ").map(BStr::new) {
            let parent_hash = if first_parent.first() == Some(&b':') {
                let mark = &first_parent[1..];
                *self
                    .mark_oid_map
                    .get(mark)
                    .ok_or_else(|| anyhow!("Could not find parent mark {first_parent}"))?
            } else {
                gix::ObjectId::from_hex(first_parent)
                    .with_context(|| format!("Bad commit hash {first_parent}"))?
            };
            commit.parents.push(parent_hash);
            self.must_advance_line()?;
        } else if self.current_line.starts_with(b"merge ") {
            bail!("'merge' line without 'from' line is not supported");
        }
        while self.current_line.starts_with(b"merge ") {
            let parent = BStr::new(self.current_line.strip_prefix(b"merge ").unwrap());
            let parent_hash = if parent.first() == Some(&b':') {
                let mark = &parent[1..];
                *self
                    .mark_oid_map
                    .get(mark)
                    .ok_or_else(|| anyhow!("Could not find parent mark {parent}"))?
            } else {
                gix::ObjectId::from_hex(parent)
                    .with_context(|| format!("Bad commit hash {parent}"))?
            };
            commit.parents.push(parent_hash);
            self.must_advance_line()?;
        }

        loop {
            let changed_file = if let Some(change_data) = self.current_line.strip_prefix(b"M ") {
                // filemodify: 'M' SP <mode> SP <dataref> SP <path> LF
                let (mode, hash, path) = change_data
                    .splitn_str(3, b" ")
                    .collect_tuple()
                    .ok_or_else(|| {
                        anyhow!(
                            "Expected 3 parts in filemodify line {:?}",
                            self.current_line
                        )
                    })?;
                ChangedFile {
                    path: path.into(),
                    change: FileChange::Modified {
                        mode: mode.into(),
                        hash: hash.into(),
                    },
                }
            } else if let Some(change_data) = self.current_line.strip_prefix(b"D ") {
                // filedelete: 'D' SP <path> LF
                ChangedFile {
                    path: change_data.into(),
                    change: FileChange::Deleted,
                }
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
        Ok(commit)
    }

    fn read_reset(&mut self) -> Result<Option<FastExportReset>> {
        let branch: BString = self
            .current_line
            .strip_prefix(b"reset ")
            .ok_or_else(|| anyhow!("Expected 'reset' line"))?
            .into();
        self.must_advance_line()?;
        let from = match self.current_line.strip_prefix(b"from ") {
            Some(from) => from.as_bstr(),
            None => {
                // No optional 'from' line, reset to an empty branch which is
                // uninteresting.
                return Ok(None);
            }
        };
        let from = match from.strip_prefix(b":") {
            Some(mark) => *self
                .mark_oid_map
                .get(mark)
                .ok_or_else(|| anyhow!("Mark {from} not seen before"))?,
            None => gix::ObjectId::from_hex(from)?,
        };
        let branch = FullName::try_from(branch)?;
        self.must_advance_line()?;
        Ok(Some(FastExportReset { branch, from }))
    }
}

impl Iterator for FastExportRepo {
    type Item = Result<FastExportEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_line.is_empty() {
                // noop
            } else if self.current_line == b"done" {
                break;
            } else if self.current_line == b"feature done" {
                // noop
            } else if self.current_line.starts_with(b"commit ") {
                let commit = self.read_commit().with_context(|| {
                    format!(
                        "Error parsing git-fast-export commit line {:?}",
                        self.current_line
                    )
                });
                return Some(commit.map(FastExportEntry::Commit));
            } else if self.current_line.starts_with(b"reset ") {
                let reset = self.read_reset().with_context(|| {
                    format!(
                        "Error parsing git-fast-export reset line {:?}",
                        self.current_line
                    )
                });
                match reset {
                    Ok(Some(reset)) => return Some(Ok(FastExportEntry::Reset(reset))),
                    Ok(None) => continue, // Uninteresting reset command, continue.
                    Err(err) => return Some(Err(err)),
                };
            } else {
                return Some(Err(anyhow!(
                    "Unexpected git-fast-export command: {}",
                    self.current_line
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

pub struct FastImportRepo {
    process: std::process::Child,
    stdin_writer: Option<BufWriter<std::process::ChildStdin>>,
    stdout_reader: BufReader<std::process::ChildStdout>,

    /// The last mark that was imported, which is also the number of commits
    /// imported.
    last_mark: usize,
    /// `marks` is 1-indexed, so `marks[0]` holds the object id for `mark=1`.
    marks: Vec<gix::ObjectId>,
    oid_to_mark: HashMap<gix::ObjectId, usize>,
}

impl FastImportRepo {
    pub fn new(repo_dir: &Path, logger: crate::log::Logger) -> Result<Self> {
        let logger = logger.with_context("git-fast-import");
        // If the upstream repository has been force updated, then this import
        // should also force update.
        //
        // TODO: Add --force and --quiet as parameters?
        let mut process = git_command(repo_dir)
            .args(["fast-import", "--done", "--force", "--quiet"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin_writer = BufWriter::new(
            process
                .stdin
                .take()
                .context("Could not connect standard input.")?,
        );
        let stdout_reader =
            BufReader::new(process.stdout.take().context("Could not capture stdout")?);
        let mut stderr_reader =
            BufReader::new(process.stderr.take().context("Could not capture stderr")?);
        std::thread::Builder::new()
            .name("git-fast-import-stderr".into())
            .spawn(move || {
                for line in crate::util::ReadLossyCrOrLfLines::new(&mut stderr_reader) {
                    logger.warning(line.trim_end().to_owned());
                }
            })
            .expect("failed to spawn thread");
        stdin_writer.write_all(b"feature done\n")?;
        Ok(FastImportRepo {
            process,
            stdin_writer: Some(stdin_writer),
            stdout_reader,
            last_mark: 0,
            marks: Vec::new(),
            oid_to_mark: HashMap::new(),
        })
    }

    pub fn wait(mut self) -> Result<Vec<gix::ObjectId>> {
        let mut stdin_writer = self.stdin_writer.take().unwrap();
        stdin_writer.write_all(b"done\n")?;
        drop(stdin_writer); // Close stdin.
        let marks_count = self.last_mark;
        while self.marks.len() < marks_count {
            self.read_mark()?;
        }
        drop(self.stdout_reader);
        self.process.wait()?;
        Ok(self.marks)
    }

    /// Don't accumulate a too big buffer in stdout. On the other hand, don't
    /// wait for git-fast-import to finish all the importing to be done and wait
    /// for the next command. Default pipe size is 64kiB, so up to 64 entries
    /// backlog is fine, then far more than half the pipe buffer is left.
    fn peel_stdout(&mut self) -> Result<()> {
        const MAX_BACKLOG: usize = 64;
        while self.marks.len() + MAX_BACKLOG < self.last_mark {
            self.read_mark()?;
        }
        Ok(())
    }

    fn read_mark(&mut self) -> Result<()> {
        let mut line = BString::new(vec![]);
        if self.stdout_reader.read_until(b'\n', &mut line)? == 0 {
            bail!("Unexpected EOF while reading mark");
        }
        let oid_hex = BStr::new(
            line.strip_suffix(b"\n")
                .ok_or_else(|| anyhow!("No LF at the end of the {line}"))?,
        );
        let oid = gix::ObjectId::from_hex(oid_hex).with_context(|| {
            format!(
                "Bad hash {oid_hex} for mark :{} from git-fast-import",
                self.marks.len()
            )
        })?;
        self.marks.push(oid);
        Ok(())
    }

    pub fn get_object_id(&mut self, mark: usize) -> Result<gix::ObjectId> {
        if mark == 0 || mark > self.last_mark {
            bail!("Mark {mark} not exported yet");
        }
        let idx = mark - 1;
        while self.marks.len() <= idx {
            self.read_mark()?;
        }
        Ok(self.marks[idx])
    }

    pub fn write_commit(&mut self, commit: &FastImportCommit<'_>) -> Result<usize> {
        self.peel_stdout()?;

        self.last_mark += 1;
        let mark = self.last_mark;
        if let Some(oid) = commit.original_id {
            self.oid_to_mark.insert(oid, mark);
        }

        let out = self.stdin_writer.as_mut().unwrap();

        if commit.parents.is_empty() {
            out.write_all(b"reset ")?;
            out.write_all(commit.branch.as_bstr())?;
            out.write_all(b"\n")?;
        }

        out.write_all(b"commit ")?;
        out.write_all(commit.branch.as_bstr())?;
        out.write_all(b"\n")?;

        writeln!(out, "mark :{mark}")?;

        out.write_all(b"author ")?;
        out.write_all(&commit.author_info)?;
        out.write_all(b"\n")?;

        out.write_all(b"committer ")?;
        out.write_all(&commit.committer_info)?;
        out.write_all(b"\n")?;

        if let Some(encoding) = &commit.encoding {
            out.write_all(b"encoding ")?;
            out.write_all(encoding)?;
            out.write_all(b"\n")?;
        }

        writeln!(out, "data {}", commit.message.len())?;
        out.write_all(&commit.message)?;

        for (idx, parent) in commit.parents.iter().enumerate() {
            out.write_all(if idx == 0 { b"from " } else { b"merge " })?;
            out.write_all(&Self::format_commit_ref(&self.oid_to_mark, parent))?;
            out.write_all(b"\n")?;
        }

        for changed_file in &commit.file_changes {
            match &changed_file.change {
                FileChange::Modified { mode, hash } => {
                    out.write_all(b"M ")?;
                    out.write_all(mode)?;
                    out.write_all(b" ")?;
                    out.write_all(hash)?;
                    out.write_all(b" ")?;
                    out.write_all(&changed_file.path)?;
                }
                FileChange::Deleted => {
                    out.write_all(b"D ")?;
                    out.write_all(&changed_file.path)?;
                }
            }
            out.write_all(b"\n")?;
        }
        out.write_all(b"\n")?;
        writeln!(out, "get-mark :{mark}")?;
        out.flush()?;
        Ok(mark)
    }

    pub fn write_reset(&mut self, branch: &FullNameRef, revision: &ImportCommitRef) -> Result<()> {
        self.peel_stdout()?;

        let out = self.stdin_writer.as_mut().unwrap();
        out.write_all(b"reset ")?;
        out.write_all(branch.as_bstr())?;
        out.write_all(b"\nfrom ")?;
        out.write_all(&Self::format_commit_ref(&self.oid_to_mark, revision))?;
        out.write_all(b"\n")?;
        // No need to flush.
        Ok(())
    }

    fn format_commit_ref(
        oid_to_mark: &HashMap<gix::ObjectId, usize>,
        commit_ref: &ImportCommitRef,
    ) -> BString {
        match commit_ref {
            ImportCommitRef::Mark(mark) => format!(":{mark}").into(),
            ImportCommitRef::CommitId(oid) => match oid_to_mark.get(oid) {
                Some(mark) => format!(":{mark}").into(),
                None => oid.to_hex().to_string().into(),
            },
        }
    }
}

#[serde_as]
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct WithoutCommitterId(
    #[serde_as(serialize_as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    gix::ObjectId,
);

impl serde_with::SerializeAs<WithoutCommitterId> for WithoutCommitterId {
    fn serialize_as<S>(source: &WithoutCommitterId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        source.serialize(serializer)
    }
}

impl<'de> serde_with::DeserializeAs<'de, WithoutCommitterId> for WithoutCommitterId {
    fn deserialize_as<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::deserialize(deserializer)
    }
}

impl std::fmt::Display for WithoutCommitterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for WithoutCommitterId {
    type Err = <gix::ObjectId as std::str::FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let oid = gix::ObjectId::from_str(s)?;
        Ok(Self(oid))
    }
}

/// `FastExportImportDedupCache` deduplicates entries that would be exported that
/// have different committer but otherwise are exactly the same.
#[serde_as]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FastExportImportDedupCache {
    /// Maps from a hash of a commit, apart from the committer, to the latest
    /// imported or exported commit id.
    #[serde_as(
        serialize_as = "serde_with::IfIsHumanReadable<HashMap<serde_with::DisplayFromStr, serde_with::DisplayFromStr>>"
    )]
    commits: HashMap<WithoutCommitterId, gix::ObjectId>,
}

trait HasherExt {
    fn update_bstring(&mut self, value: &BString);
    fn update_usize(&mut self, value: usize);
    fn update_option_bstring(&mut self, value: &Option<BString>);
    fn update_serde<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize;
}

impl HasherExt for gix::hash::Hasher {
    fn update_bstring(&mut self, value: &BString) {
        self.update_usize(value.len());
        self.update(value);
    }

    fn update_usize(&mut self, value: usize) {
        // Native endianess is enough as the cache is not meant to be cross-platform.
        self.update(&value.to_ne_bytes());
    }

    fn update_option_bstring(&mut self, value: &Option<BString>) {
        match value {
            Some(inner) => {
                self.update(&[1]);
                self.update_bstring(inner);
            }
            None => self.update(&[0]),
        }
    }

    fn update_serde<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let encoded_value = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
        self.update_usize(encoded_value.len());
        self.update(&encoded_value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitId;
    use crate::git::GitPath;
    use crate::util::CommandExtension as _;
    use bstr::ByteSlice;
    use std::borrow::Borrow as _;
    use std::path::Path;
    use std::path::PathBuf;

    /// Copied from `tests/fixtures/toprepo.rs`.
    fn commit_env() -> HashMap<String, String> {
        HashMap::from(
            [
                ("GIT_AUTHOR_NAME", "A Name"),
                ("GIT_AUTHOR_EMAIL", "a@no.domain"),
                ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
                ("GIT_COMMITTER_NAME", "C Name"),
                ("GIT_COMMITTER_EMAIL", "c@no.domain"),
                ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
            ]
            .map(|(k, v)| (k.into(), v.into())),
        )
    }

    /// Copied from `tests/fixtures/toprepo.rs`.
    fn commit(repo: &Path, env: &HashMap<String, String>, message: &str) -> CommitId {
        git_command(repo)
            .args(["commit", "--allow-empty", "-m", message])
            .envs(env)
            .check_success_with_stderr()
            .unwrap();

        // Returns commit hash as String.
        // TODO: Return Result<String> instead?
        let output = git_command(repo)
            .args(["rev-parse", "HEAD"])
            .envs(env)
            .check_success_with_stderr()
            .unwrap();

        let commit_id_hex = crate::util::trim_newline_suffix(output.stdout.to_str().unwrap());
        CommitId::from_hex(commit_id_hex.as_bytes()).unwrap()
    }

    /// The example repository is from `tests/fixtures/toprepo.rs`. Sets up the
    /// repo structure and returns the top repo path.
    fn setup_example_repo(path: &Path) -> PathBuf {
        let env = commit_env();
        let top_repo = path.join("top").to_path_buf();
        let sub_repo = path.join("sub").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&sub_repo).unwrap();

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&sub_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        commit(&sub_repo, &env, "1");
        commit(&sub_repo, &env, "2");
        commit(&top_repo, &env, "A");

        git_command(&top_repo)
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "../sub/", // TODO: Absolute or relative path?
                           // sub_repo.to_str().unwrap(),
            ])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        commit(&top_repo, &env, "B");
        commit(&top_repo, &env, "C");
        let sub_rev_3 = commit(&sub_repo, &env, "3");
        crate::git::git_update_submodule_in_index(
            &top_repo,
            &GitPath::new("sub".into()),
            &sub_rev_3,
        )
        .unwrap();

        commit(&top_repo, &env, "D");
        top_repo
    }

    #[test]
    fn test_parse_fast_export_output() {
        let tmp_dir = tempfile::tempdir().unwrap();
        // Debug with tmp_dir.into_path() which persists the directory.
        let example_repo = setup_example_repo(tmp_dir.path());

        let (log_accumulator, logger, _interrupted) = crate::log::LogAccumulator::new_fail_fast();
        let mut repo =
            FastExportRepo::load_from_path_all_refs(example_repo.as_path(), logger).unwrap();
        let commit_a = match repo.next().unwrap().unwrap() {
            FastExportEntry::Commit(c) => c,
            _ => panic!("Expected FastExportCommit"),
        };
        let commit_d = match repo.nth(2).unwrap().unwrap() {
            FastExportEntry::Commit(c) => c,
            _ => panic!("Expected FastExportCommit"),
        };
        log_accumulator.join_no_warnings().unwrap();

        assert_eq!(
            commit_a.branch,
            Some(FullName::try_from(b"refs/heads/main".as_bstr()).unwrap())
        );
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
            commit_a.original_id.to_hex().to_string(),
            "6fc12aa7d6d06400a70bb522244bb184e3678416",
        );
        assert_eq!(commit_a.encoding, None);

        assert_eq!(
            commit_d.branch,
            Some(FullName::try_from(b"refs/heads/main".as_bstr()).unwrap())
        );
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
            &ChangedFile {
                path: BString::from(b"sub"),
                change: FileChange::Modified {
                    mode: BString::from(b"160000"),
                    hash: BString::from(b"eeb85c77b614a7ec060f6df5825c9a5c10414307"),
                }
            }
        );
        assert_eq!(
            commit_d.parents,
            vec![gix::ObjectId::from_hex(b"ec67a8703750336a938bef740115009b6310892f").unwrap()]
        );
        assert_eq!(
            commit_d.original_id,
            gix::ObjectId::from_hex(b"9f781a9707757573b16ee5946ab147e4e66857bc").unwrap(),
        );
        assert_eq!(commit_d.encoding, None);
    }

    #[test]
    fn test_fast_import() {
        let temp_dir = tempfile::tempdir().unwrap();
        let from_repo_path = setup_example_repo(&temp_dir.path().join("from"));

        let to_repo_path = temp_dir.path().join("to");
        std::fs::create_dir(&to_repo_path).unwrap();
        git_command(&to_repo_path)
            .args(["init"])
            .check_success_with_stderr()
            .unwrap();
        std::fs::copy(
            from_repo_path.join(".gitmodules"),
            to_repo_path.join(".gitmodules"),
        )
        .unwrap();
        git_command(&to_repo_path)
            .args(["add", ".gitmodules"])
            .check_success_with_stderr()
            .unwrap();

        let (log_accumulator, logger, _interrupted) = crate::log::LogAccumulator::new_fail_fast();

        let fast_export_repo =
            FastExportRepo::load_from_path_all_refs(from_repo_path.as_path(), logger.clone())
                .unwrap();

        let mut fast_import_repo = FastImportRepo::new(to_repo_path.as_path(), logger).unwrap();
        for export_entry in fast_export_repo {
            let export_entry = export_entry.unwrap();
            match export_entry {
                FastExportEntry::Commit(export_commit) => {
                    let import_commit = FastImportCommit {
                        branch: export_commit.branch.as_ref().unwrap().borrow(),
                        author_info: export_commit.author_info,
                        committer_info: export_commit.committer_info,
                        encoding: export_commit.encoding,
                        message: export_commit.message,
                        file_changes: export_commit.file_changes,
                        parents: export_commit
                            .parents
                            .iter()
                            .map(|parent_id| ImportCommitRef::CommitId(*parent_id))
                            .collect(),
                        original_id: Some(export_commit.original_id),
                    };
                    fast_import_repo.write_commit(&import_commit).unwrap();
                }
                FastExportEntry::Reset(export_reset) => {
                    fast_import_repo
                        .write_reset(
                            export_reset.branch.borrow(),
                            &ImportCommitRef::CommitId(export_reset.from),
                        )
                        .unwrap();
                }
            }
        }
        fast_import_repo.wait().unwrap();

        log_accumulator.join_no_warnings().unwrap();

        let from_ref = git_command(&from_repo_path)
            .args(["rev-parse", "refs/heads/main"])
            .check_success_with_stderr()
            .unwrap()
            .stdout
            .to_str()
            .unwrap()
            .to_string();

        let to_ref = git_command(&to_repo_path)
            .args(["rev-parse", "refs/heads/main"])
            .check_success_with_stderr()
            .unwrap()
            .stdout
            .to_str()
            .unwrap()
            .to_string();

        assert_eq!(from_ref, to_ref);
    }
}
