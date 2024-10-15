#![allow(unused)]

use std::{env, io};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use itertools::Itertools;
use lazycell::LazyCell;
use anyhow::{Context, Result};
use crate::config::{Config, RepoConfig};
use crate::git::{determine_git_dir, GitModuleInfo};
use crate::util::{iter_to_string, strip_suffix, Url};

const DEFAULT_FETCH_ARGS: [&str; 3] = ["--prune", "--prune-tags", "--tags"];

#[derive(Debug)]
pub struct Repo {
    name: String,
    pub path: PathBuf,
    git_dir: LazyCell<PathBuf>,
}

impl Repo {
    pub fn new(name: String, path: PathBuf) -> Repo {
        Repo {
            name,
            path,
            git_dir: LazyCell::new(),
        }
    }

    pub fn from_str(repo: &str) -> Result<Repo> {
        //PosixPath('/home/lack/Documents/Kod/RustRover/git-toprepo')
        let command = Command::new("git")
            .args(["-C", repo])
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .with_context(|| format!("Failed to parse repo path {}", repo))?;
        let path = strip_suffix(&String::from_utf8(command.stdout)?, "\n")
            .to_string();

        let cwd = env::current_dir().unwrap_or(PathBuf::new());
        let mut path = PathBuf::from(path);

        if path == cwd {
            path = PathBuf::from(".")
        }
        let path = path.strip_prefix(cwd)
            .map(|path| path.to_path_buf()).unwrap_or(path);

        Ok(Repo::new("mono repo".to_string(), path))
    }

    fn from_config(path: PathBuf, config: Config) -> Repo {
        todo!()
    }

    pub fn get_toprepo_fetch_url(&self) -> String {
        let fetch_url = self.get_url("remote.origin.url");
        let fetch_url = match fetch_url.as_deref() {
            Err(_) | Ok("file:///dev/null") => todo!(),
            Ok(url) => url,
        };

        let push_url = self.get_url("remote.top.pushUrl");
        if push_url.is_err() {
            todo!()
        }

        fetch_url.to_string()
    }

    fn get_url(&self, toprepo_fetchurl_key: &str) -> io::Result<String> {
        let command = Command::new("git")
            .args(["-C", self.path.to_str().unwrap()])
            .args(["config", toprepo_fetchurl_key])
            .output()
            .map(|cmd| String::from_utf8(cmd.stdout).unwrap())
            .map(|url| url.trim_end().to_string());

        command
    }

    pub fn get_toprepo_git_dir(&self) -> PathBuf {
        self.get_subrepo_git_dir(TopRepo::NAME)
    }

    pub fn get_subrepo_git_dir(&self, name: &str) -> PathBuf {
        if !self.git_dir.filled() {
            self.git_dir.fill(determine_git_dir(&self.path))
                .unwrap();
        }

        let git_dir = self.git_dir.borrow().unwrap().to_str().unwrap();
        PathBuf::from(
            format!("{}/repos/{}", git_dir, name)
        )
    }
}


#[derive(Debug)]
pub struct TopRepo {
    name: String,
    path: PathBuf,
    config: RepoConfig,
}

impl TopRepo {
    pub const NAME: &'static str = "top";

    pub fn new(path: PathBuf, fetch_url: &String, push_url: &String) -> TopRepo {
        let config = RepoConfig {
            name: TopRepo::NAME.to_string(),
            enabled: true,
            raw_urls: Vec::new(),
            fetch_url: fetch_url.clone(),
            fetch_args: iter_to_string(DEFAULT_FETCH_ARGS),
            push_url: push_url.clone(),
        };

        TopRepo {
            name: TopRepo::NAME.to_string(),
            path,
            config,
        }
    }
    pub fn from_config(repo: PathBuf, config: &Config) -> TopRepo {
        TopRepo::new(
            repo,
            &config.top_fetch_url,
            &config.top_push_url,
        )
    }
}


pub struct RepoFetcher<'a> {
    monorepo: &'a Repo,
}

impl RepoFetcher<'_> {
    pub fn new(monorepo: &Repo) -> RepoFetcher {
        RepoFetcher {
            monorepo
        }
    }

    fn fetch_repo(&self) {
        todo!()
    }
}


pub fn remote_to_repo(remote: &str, mut git_modules: Vec<GitModuleInfo>, config: &Config) ->
(String, Option<GitModuleInfo>) {
    // Map a remote or URL to a repository.
    //
    // A repo can be specified by subrepo path inside the toprepo or
    // as a full or partial URL.

    // Map a full or partial URL or path to one or more repos.
    let mut remote_to_name = HashMap::from_iter([
        ("origin", vec![(TopRepo::NAME, None)]),
        (".", vec![(TopRepo::NAME, None)]),
        ("", vec![(TopRepo::NAME, None)]),
    ]);

    fn add_url<'a>(
        remote_to_name: &mut HashMap<&'a str, Vec<(&'a str, Option<&'a GitModuleInfo>)>>,
        url: &'a str,
        name: &'a str,
        gitmod: Option<&'a GitModuleInfo>,
    ) {
        let entry = (name, gitmod);
        remote_to_name.entry(url).or_insert(Vec::new()).push(entry);

        // Also match partial URLs.
        // Example: ssh://user@github.com:22/foo/bar.git
        let url = url.strip_suffix(".git").unwrap_or(url);
        remote_to_name.entry(url).or_insert(Vec::new()).push(entry);

        if let Some(i) = url.find("://") {
            let (_, url) = url.split_at(i);
            remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
        }
        if let Some(i) = url.find("@") {
            let (_, url) = url.split_at(i);
            remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
        }
        if !url.starts_with(".") {
            if let Some(i) = url.find("/") {
                let (_, url) = url.split_at(i);
                remote_to_name.entry(url).or_insert(Vec::new()).push(entry);
            }
        }
    }

    add_url(&mut remote_to_name, &config.top_fetch_url, TopRepo::NAME, None);
    add_url(&mut remote_to_name, &config.top_push_url, TopRepo::NAME, None);

    for module in &git_modules {
        for cfg in &config.repos {
            if cfg.raw_urls.contains(&module.raw_url) {
                // Add URLs from .gitmodules.
                add_url(&mut remote_to_name, &module.url, &cfg.name, Some(&module));
                add_url(&mut remote_to_name, &module.raw_url, &cfg.name, Some(&module));

                remote_to_name.get_mut(module.name.as_str()).unwrap()
                    .push((&cfg.name, Some(&module)));
                remote_to_name.get_mut(module.path.to_str().unwrap()).unwrap()
                    .push((&cfg.name, Some(&module)));

                // Add URLs from the toprepo config.
                add_url(&mut remote_to_name, &cfg.fetch_url, &cfg.name, Some(&module));
                add_url(&mut remote_to_name, &cfg.push_url, &cfg.name, Some(&module));
                for raw_url in &cfg.raw_urls {
                    add_url(&mut remote_to_name, &raw_url, &cfg.name, Some(&module));
                }
            }
        }
    }

    //Now, try to find our repo.
    let full_remote = remote;
    let mut remote = remote;
    remote = remote.strip_suffix("/").unwrap_or(remote);
    remote = remote.strip_suffix(".git").unwrap_or(remote);
    let mut entries = remote_to_name.get(remote);

    if entries.is_none() && remote.contains("://") {
        (_, remote) = remote.split_once("://").unwrap();
        entries = remote_to_name.get(remote);
    }
    if entries.is_none() && remote.contains("@") {
        (_, remote) = remote.split_once("@").unwrap();
        entries = remote_to_name.get(remote);
    }
    if entries.is_none() && remote.contains("/") && !remote.starts_with(".") {
        (_, remote) = remote.split_once("/").unwrap();
        entries = remote_to_name.get(remote);
    }
    let entries = entries.expect(format!("Could not resolve '{}'", full_remote).as_str());

    if entries.len() > 1 {
        let names = entries.into_iter().map(|(name, _)| *name).join(", ");
        panic!("Multiple remote candidates: [{}]", names)
    }

    match entries[0] {
        (name, Some(gitmod)) => {
            let i = git_modules.iter().position(|module| module == gitmod);
            let name = name.to_string();
            let gitmod = git_modules.swap_remove(i.unwrap());
            (name, Some(gitmod))
        }
        (name, None) => (name.to_string(), None)
    }
}

pub fn repository_name(repository: Url) -> String {
    let mut name: String = repository;

    // Handle relative paths.
    name = name.replace("../", "");
    name = name.replace("./", "");

    // Remove scheme.
    if let Some(i) = name.find("://") {
        name = name.split_off(i + 3);

        // Remove the domain name.
        if let Some(i) = name.find("/") {
            name = name.split_off(i + 1);
        }
    }

    //Annoying with double slash.
    name = name.replace("//", "/");
    name = name.trim_start_matches("/")
        .trim_end_matches("/")
        .to_string();

    if let Some(temp) = name.strip_suffix(".git") {
        name = temp.to_string();
    }

    name = name.replace("/", "-");
    name = name.replace("\\", "-");
    name = name.replace(":", "-");

    name
}
