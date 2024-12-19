use anyhow::Result;
use bstr::{ByteSlice, ByteVec};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::Stdio,
};

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
// TODO: Change name to FastExportParser
pub struct FastExportRepo {
    mark_oid_map: HashMap<Vec<u8>, Vec<u8>>,
    reader: BufReader<std::process::ChildStdout>,
    current_line: Vec<u8>,
}

// TODO: Add access function to get committer date
impl FastExportRepo {
    pub fn load_from_path(repo_dir: &Path) -> Result<Self> {
        let stdout = git_command(&repo_dir)
            .args([
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
            mark_oid_map: HashMap::new(),
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
                self.mark_oid_map.insert(mark, original_id.clone());
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

        // TODO: Handle more than 1 parent
        if self.starts_with(b"from") {
            let baseref = self.current_line.split_once_str(b"from ").unwrap().1;
            if baseref.starts_with_str(b":") {
                let key = baseref.split_at(1).1.to_vec();
                commit
                    .parents
                    .push(self.mark_oid_map.get(&key).unwrap().clone());
            } else if baseref.len() == 40 {
                commit.parents.push(baseref.to_vec());
            }
            self.advance_line().unwrap();
        }

        while self.starts_with(b"merge") {
            let baseref = self.current_line.split_once_str(b"merge ").unwrap().1;
            if baseref.starts_with_str(":") {
                let key = baseref.split_at(1).1.to_vec();
                commit
                    .parents
                    .push(self.mark_oid_map.get(&key).unwrap().clone());
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

pub fn export_to_fast_import<I>(commit_container: &mut I) -> Vec<u8>
where
    I: Iterator<Item = FastExportCommit>,
{
    let mut mark_counter: usize = 1;
    let mut out = Vec::<u8>::new();
    let mut oid_mark_map: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    out.push_str(b"feature done\n");

    for commit in commit_container.by_ref() {
        if commit.parents.is_empty() {
            out.push_str(b"reset ");
            out.push_str(&commit.branch);
            out.push_byte(b'\n');
        }

        out.push_str(b"commit ");
        out.push_str(&commit.branch);
        out.push_byte(b'\n');

        let mark_as_bytes = mark_counter.to_string().as_bytes().to_vec();
        out.push_str(b"mark :");
        out.push_str(&mark_as_bytes);
        out.push_byte(b'\n');
        oid_mark_map.insert(commit.original_id.clone(), mark_as_bytes.clone());
        mark_counter += 1;

        out.push_str(b"original-oid ");
        out.push_str(&commit.original_id);
        out.push_byte(b'\n');

        out.push_str(&commit.author_info);
        out.push_byte(b'\n');

        out.push_str(&commit.committer_info);
        out.push_byte(b'\n');

        out.push_str(b"data ");
        out.push_str(&commit.message.len().to_string().as_bytes());
        out.push_byte(b'\n');
        out.push_str(&commit.message);

        if !commit.parents.is_empty() {
            out.push_str(b"from :");
            out.push_str(&oid_mark_map.get(commit.parents.first().unwrap()).unwrap());
            out.push_byte(b'\n');

            for parent in commit.parents[1..].iter() {
                out.push_str(b"merge :");
                out.push_str(&oid_mark_map.get(parent).unwrap());
                out.push_byte(b'\n');
            }
        }

        for changed_file in commit.file_changes {
            match changed_file.status {
                DiffStatus::Modified => {
                    out.push_str(b"M ");
                    out.push_str(&changed_file.mode.unwrap());
                    out.push_byte(b' ');
                    out.push_str(&changed_file.hash.unwrap());
                    out.push_byte(b' ');
                }
                DiffStatus::Deleted => {
                    out.push_str(b"D ");
                }
            }
            out.push_str(&changed_file.file);
            out.push_byte(b'\n');
        }

        out.push_byte(b'\n');
    }
    out.push_str(b"done\n");

    out
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
            FastExportRepo::load_from_path(from_repo_path.as_path()).unwrap();

        let fast_import_input = export_to_fast_import(&mut fast_export_repo);

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
