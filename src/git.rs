use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use crate::config::ConfigMap;
use crate::config_loader::{
    ConfigLoaderTrait,
    ConfigLoader,
};
use crate::util::{join_submodule_url, RawUrl, Url};

#[derive(Clone, Eq, PartialEq)]
pub struct GitModuleInfo {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub url: Url,
    pub raw_url: RawUrl,
}


pub(crate) fn determine_git_dir(repo: &PathBuf) -> PathBuf {
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

pub(crate) fn get_gitmodules_info(
    config_loader: ConfigLoader, parent_url: &str,
) -> Vec<GitModuleInfo> {
    // Parses the output from 'git config --list --file .gitmodules'.
    let submod_config_mapping: HashMap<String, ConfigMap> = config_loader.get_configmap().extract_mapping("submodule");

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

    configs
}