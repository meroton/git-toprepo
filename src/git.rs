use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;
use itertools::Itertools;
use anyhow::{anyhow, Result};
use crate::config::ConfigMap;
use crate::config_loader::{
    ConfigLoaderTrait,
    ConfigLoader,
};
use crate::repo::Repo;
use crate::util::{CommitHash, join_submodule_url, RawUrl, Url};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GitModuleInfo {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub url: Url,
    pub raw_url: RawUrl,
}


 pub fn determine_git_dir(repo: &PathBuf) -> PathBuf {
    let mut code = format!(
        "str(\"{}\").encode(\"utf-8\")",
        repo.to_str().unwrap()
    );
    code = format!(
        "git_filter_repo_for_toprepo.GitUtils.determine_git_dir({})",
        code
    );
    code = format!(
        "print({})",
        code
    );

    let command = Command::new("python")
        // FIXME: Temporary bodge local to my file structure
        .env("PYTHONPATH", "../../../..")

        .arg("-c")
        .arg(format!("{}\n{}",
                     "import git_filter_repo_for_toprepo",
                     code,
        ))
        .output()
        .unwrap();

    if !command.stderr.is_empty() {
        if let Ok(err) = String::from_utf8(command.stderr) {
            std::panic!("{}", err);
        }
    }

    let path = String::from_utf8(command.stdout).unwrap();
    PathBuf::from(path)
}

 pub fn get_gitmodules_info(
    config_loader: ConfigLoader, parent_url: &str,
) -> Result<Vec<GitModuleInfo>> {
    // Parses the output from 'git config --list --file .gitmodules'.
    let submod_config_mapping: HashMap<String, ConfigMap> = config_loader.get_configmap()?
        .extract_mapping("submodule")?;

    let mut configs = Vec::new();
    let mut used = HashSet::new();

    for (name, configmap) in submod_config_mapping {
        let raw_url = configmap.get_singleton("url").unwrap().to_string();
        let resolved_url = join_submodule_url(parent_url, &raw_url);
        let path = configmap.get_singleton("path").unwrap();

        let submod_info = GitModuleInfo {
            name,
            path: PathBuf::from(path),
            branch: configmap.get_singleton("branch").map(|s| s.to_string()),
            url: resolved_url,
            raw_url,
        };

        if used.insert(path.to_owned()) {
            panic!("Duplicate submodule configs for '{}'", path);
        }
        println!("Submodule: {}", path);
        configs.push(submod_info);
    }

    Ok(configs)
}


#[derive(Debug)]
pub struct PushSplitter<'a> {
    repo: &'a Repo,
}

impl PushSplitter<'_> { //TODO: verify
pub fn new(repo: &Repo) -> PushSplitter {
        PushSplitter {
            repo
        }
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

    pub fn get_top_commit_subrepos(&self, top_commit_hash: CommitHash) -> HashMap<Vec<u8>, CommitHash> {
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
                    submod_hash.bytes().collect_vec(),
                );
            }
        }

        subrepo_map
    }
}
