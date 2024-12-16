use anyhow::Result;
use bstr::ByteSlice;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::Stdio,
};

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

#[derive(Debug, PartialEq)]
pub struct ChangedFile {
    pub file: Vec<u8>,
    pub mode: Option<Vec<u8>>,
    pub hash: Option<Vec<u8>>,
    pub status: DiffStatus,
}

#[derive(Debug, PartialEq)]
pub enum DiffStatus {
    Deleted,
    Modified,
}

#[derive(Debug)]
pub struct FastExportRepo {
    old_ids: HashMap<Vec<u8>, Vec<u8>>,
    reader: BufReader<std::process::ChildStdout>,
    current_line: Vec<u8>,
}

impl FastExportRepo {
    pub fn load_from_path(repo_dir: &Path) -> Result<Self> {
        let stdout = std::process::Command::new("git")
            .args([
                "-C",
                repo_dir.to_str().unwrap(),
                "fast-export",
                "--all", // Parameters needed to delimit which revs to include
                "--no-data",
                "--use-done-feature",
                "--show-original-ids",
                "--reference-excluded-parents",
                "--signed-tags=strip",
            ])
            .stdout(Stdio::piped())
            .spawn()?
            .stdout
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Could not capture standard output.",
                )
            })?;

        Ok(FastExportRepo {
            old_ids: HashMap::new(),
            reader: BufReader::new(stdout),
            current_line: Vec::new(),
        })
    }
    
    fn advance_line(&mut self) -> Result<usize> {
        self.current_line.clear();
        let bytes = self.reader.read_until(b'\n', &mut self.current_line)?;
        self.current_line = self.current_line.trim().to_vec();
        Ok(bytes)
    }

    fn read_bytes(&mut self, buf: &mut Vec<u8>, bytes: usize) -> Result<()> {
        let mut tmp_buf = vec![0u8; bytes];
        self.reader.read_exact(&mut tmp_buf)?;
        *buf = tmp_buf;
        Ok(())
    }

    fn starts_with(&self, pat: &[u8]) -> bool {
        self.current_line.starts_with_str(pat)
    }

    fn read_commit(&mut self) -> FastExportCommit {
        let mut commit = FastExportCommit::default();
        commit.branch = self
            .current_line
            .split_once_str(b"commit ")
            .unwrap()
            .1
            .into();

        self.advance_line().unwrap();

        let mut opt_mark: Option<Vec<u8>> = None;
        if self.starts_with(b"mark") {
            opt_mark = Some(
                self.current_line
                    .split_once_str(b"mark :")
                    .unwrap()
                    .1
                    .into(),
            );
            self.advance_line().unwrap();
        }

        if self.starts_with(b"original-oid") {
            let original_id: Vec<u8> = self
                .current_line
                .split_once_str(b"original-oid ")
                .unwrap()
                .1
                .into();

            commit.original_id = original_id.clone();

            self.advance_line().unwrap();

            if let Some(mark) = opt_mark {
                self.old_ids.insert(mark, original_id.clone());
            }
        }

        if self.starts_with(b"author") {
            commit.author_info = self.current_line.clone().into();
            self.advance_line().unwrap();
        }

        commit.committer_info = self.current_line.clone().into();
        self.advance_line().unwrap();

        if commit.author_info.is_empty() {
            commit.author_info = commit.committer_info.clone();
        }

        if self.current_line.starts_with(b"encoding") {
            commit.encoding = Some(self.current_line.clone().into());
            self.advance_line().unwrap();
        }

        let chars: usize = self
            .current_line
            .split_once_str(b"data ")
            .unwrap()
            .1
            .to_str()
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        self.read_bytes(&mut commit.message, chars).unwrap();
        self.advance_line().unwrap();

        if self.current_line == b"\n" {
            // Unsure where the potential newline comes from (and if we need to care about it),
            // but this is checked in git-filter-repo:
            // https://github.com/newren/git-filter-repo/blob/main/git-filter-repo#L1196
            self.advance_line().unwrap();
        }

        if self.starts_with(b"from") {
            let baseref = self.current_line.split_once_str(b"from ").unwrap().1;
            if baseref.starts_with_str(b":") {
                let key = baseref.split_at(1).1.to_vec();
                commit.parents.push(self.old_ids.get(&key).unwrap().clone());
            } else if baseref.len() == 40 {
                commit.parents.push(baseref.to_vec());
            }
            self.advance_line().unwrap();
        }

        if self.starts_with(b"merge") {
            let baseref = self.current_line.split_once_str(b"merge ").unwrap().1;
            if baseref.starts_with_str(":") {
                let key = baseref.split_at(1).1.to_vec();
                commit.parents.push(self.old_ids.get(&key).unwrap().clone());
            } else if baseref.len() == 40 {
                commit.parents.push(baseref.to_vec());
            }
            self.advance_line().unwrap();
        }

        let mut line_components: Vec<&[u8]> = self.current_line.splitn_str(4, b" ").collect();
        while let Some(&change_type) = line_components.get(0) {
            if self.current_line.is_empty() {
                break;
            } else if change_type == b"M" {
                let mode = line_components.get(1).unwrap().to_vec();
                let hash = line_components.get(2).unwrap().to_vec();
                let path = line_components.get(3).unwrap().trim().to_vec();

                commit.file_changes.push(ChangedFile {
                    file: path,
                    mode: Some(mode),
                    hash: Some(hash),
                    status: DiffStatus::Modified,
                });
            } else if change_type == b"D" {
                let path = line_components.get(1).unwrap().trim().to_vec();
                commit.file_changes.push(ChangedFile {
                    file: path,
                    mode: None,
                    hash: None,
                    status: DiffStatus::Deleted,
                });
            }
            self.advance_line().unwrap();
            line_components = self.current_line.splitn_str(4, b" ").collect();
        }
        return commit;
    }
}

impl Iterator for FastExportRepo {
    type Item = FastExportCommit;

    fn next(&mut self) -> Option<Self::Item> {
        while self.advance_line().unwrap() > 0 {
            if self.starts_with(b"commit") {
                return Some(self.read_commit());
            }
        }
        None
    }
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

        let mut repo = FastExportRepo::load_from_path(example_top_repo.as_path()).unwrap();
        let commit_a = repo.next().unwrap();
        let commit_d = repo.nth(2).unwrap();

        assert_eq!(commit_a.branch, b"refs/heads/main");
        assert_eq!(
            commit_a.author_info,
            b"author A Name <a@no.domain> 1672625045 +0100"
        );
        assert_eq!(
            commit_a.committer_info,
            b"committer C Name <c@no.domain> 1686121750 +0100"
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
            b"author A Name <a@no.domain> 1672625045 +0100"
        );
        assert_eq!(
            commit_d.committer_info,
            b"committer C Name <c@no.domain> 1686121750 +0100"
        );
        assert_eq!(commit_d.message, b"D\n");
        assert_eq!(
            // CommitInfo and DiffStatus implements PartialEq trait for this test,
            // might exclude PartialEq and this test if it's not needed elsewhere.
            commit_d.file_changes.first().unwrap(),
            &ChangedFile {
                file: Vec::from(b"sub"),
                mode: Some(Vec::from(b"160000")),
                hash: Some(Vec::from(b"eeb85c77b614a7ec060f6df5825c9a5c10414307")),
                status: DiffStatus::Modified
            }
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
}
