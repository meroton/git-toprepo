use anyhow::{anyhow, Result};
use itertools::Itertools;
use std::collections::HashMap;
use std::hash::Hash;
use std::io::BufRead;
use std::process::Command;
use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub struct Repo {
    pub path: PathBuf,
}

#[derive(
    PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct CommitHash(String);

impl From<Vec<u8>> for CommitHash {
    fn from(bytes: Vec<u8>) -> Self {
        let s = match std::str::from_utf8(&bytes) {
            Ok(v) => v,
            Err(e) => panic!("Invalid UTF-8 bytes: {}", e),
        };
        CommitHash(s.to_owned())
    }
}

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let CommitHash(s) = self;
        write!(f, "{}", s)
    }
}

#[allow(unused)]
pub fn determine_git_dir(repo: &PathBuf) -> PathBuf {
    let command = Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .output()
        .unwrap();

    if !command.stderr.is_empty() {
        if let Ok(err) = String::from_utf8(command.stderr) {
            std::panic!("{}", err);
        }
    }

    let path = String::from_utf8(command.stdout).unwrap();
    PathBuf::from(path.trim())
}

#[derive(Debug)]
pub struct PushSplitter<'a> {
    repo: &'a Repo,
}

impl PushSplitter<'_> {
    //TODO: verify
    pub fn new(repo: &Repo) -> PushSplitter {
        PushSplitter { repo }
    }

    pub fn _trim_push_commit_message(mono_message: &str) -> Result<&str> {
        let mut trimmed_message = mono_message;

        if let Some(i) = mono_message.rfind("\n^-- ") {
            trimmed_message = &mono_message[..=i];
        }

        if trimmed_message.contains("\n^-- ") {
            Err(anyhow!(
                "'^-- ' was found in the following commit message. \
                It looks like a commit that already exists upstream. {}",
                mono_message
            ))
        } else {
            Ok(trimmed_message)
        }
    }

    #[allow(unused)]
    pub fn get_top_commit_subrepos(
        &self,
        top_commit_hash: CommitHash,
    ) -> HashMap<Vec<u8>, CommitHash> {
        let top_commit_hash = ""; //TODO
        let ls_tree_subrepo_stdout = Command::new("git")
            .args(["-C", self.repo.path.to_str().unwrap()])
            .args(["ls-tree", "-r", top_commit_hash, "--"])
            .output()
            .unwrap()
            .stdout;

        let mut subrepo_map = HashMap::new();
        for line in ls_tree_subrepo_stdout.lines() {
            let line = line.unwrap();
            let submodule_mode_and_type_prefix = "160000 commit ";

            if line.starts_with(submodule_mode_and_type_prefix) {
                let hash_and_path = &line[submodule_mode_and_type_prefix.len()..];
                let (submod_hash, subdir) = hash_and_path.split_once("\t").unwrap();
                subrepo_map.insert(
                    subdir.bytes().collect_vec(),
                    submod_hash.bytes().collect_vec().into(),
                );
            }
        }

        subrepo_map
    }
}
