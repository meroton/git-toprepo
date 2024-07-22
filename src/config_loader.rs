#![allow(dead_code)]

use enum_dispatch::enum_dispatch;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use clap::command;
use colored::Colorize;
use crate::config::ConfigMap;
use crate::Repo;


#[enum_dispatch]
pub trait ConfigLoaderTrait {
    fn fetch_remote_config(&self);
    fn git_config_list(self) -> String;
    fn get_configmap(self) -> Result<ConfigMap, String>
    where
        Self: Sized,
    {
        ConfigMap::parse(&self.git_config_list())
    }
}

// Similar to an abstract class, removes the need for Box<dyn &ConfigLoaderTrait>
#[enum_dispatch(ConfigLoaderTrait)]
pub enum ConfigLoader<'a> {
    MultiConfigLoader(MultiConfigLoader<'a>),
    LocalGitConfigLoader(LocalGitConfigLoader<'a>),
    StaticContentConfigLoader,
    LocalFileConfigLoader,
    GitRemoteConfigLoader(GitRemoteConfigLoader<'a>),
}


pub struct MultiConfigLoader<'a> {
    config_loaders: Vec<ConfigLoader<'a>>,
}

pub struct LocalGitConfigLoader<'a> {
    repo: &'a Repo, // <---- This reference causes lifetime voodoo.
}

pub struct StaticContentConfigLoader {
    content: String,
}

pub struct LocalFileConfigLoader {
    filename: PathBuf,
    allow_missing: bool,
}

pub struct GitRemoteConfigLoader<'a> {
    url: String,
    remote_ref: String,
    filename: PathBuf,
    local_repo: &'a Repo, // <---- This reference causes lifetime voodoo.
    local_ref: String,
}

////////////////////////////////////////////////////////////////////////////////////////////////////

impl MultiConfigLoader<'_> {
    pub(crate) fn new(config_loaders: Vec<ConfigLoader>) -> MultiConfigLoader {
        MultiConfigLoader {
            config_loaders
        }
    }
}

impl ConfigLoaderTrait for MultiConfigLoader<'_> {
    fn fetch_remote_config(&self) {
        for config_loader in &self.config_loaders {
            config_loader.fetch_remote_config();
        }
    }

    fn git_config_list(self) -> String {
        let mut config_list = String::new();

        // Joins all ConfigLoaders in reverse order.
        for config_loader in self.config_loaders {
            let mut part: String = config_loader.git_config_list();
            if !part.is_empty() && !part.ends_with('\n') {
                part.push('\n');
            }

            part.push_str(&config_list);
            config_list = part;
        }

        config_list
    }
}


impl LocalGitConfigLoader<'_> {
    pub(crate) fn new(repo: &Repo) -> LocalGitConfigLoader {
        LocalGitConfigLoader {
            repo
        }
    }
}

impl ConfigLoaderTrait for LocalGitConfigLoader<'_> {
    fn fetch_remote_config(&self) {}
    fn git_config_list(self) -> String {
        let command = Command::new("git")
            .args(["-C", self.repo.path.to_str().unwrap().trim()])
            .args(["config", "--list"])
            .output()
            .unwrap();

        let ret = String::from_utf8(command.stdout)
            .expect("Could not load Local git configuration.");
        println!("\n{}\n{}", "Local Config".blue(), ret);
        ret
    }
}


fn parse_config_file(config_file: &str) -> String {
    println!("raw: {}", config_file);

    let mut command = Command::new("git")
        .arg("config")
        .args(["--file", "-"])
        .arg("--list")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let stdin = command.stdin.as_mut().unwrap();
    stdin.write_all(config_file.as_bytes()).unwrap();

    let output = command.wait_with_output()
        .expect("Command failed");

    String::from_utf8(output.stdout).unwrap()
}

impl StaticContentConfigLoader {
    pub(crate) fn new(content: String) -> StaticContentConfigLoader {
        StaticContentConfigLoader {
            content
        }
    }
}

impl ConfigLoaderTrait for StaticContentConfigLoader {
    fn fetch_remote_config(&self) { () }
    fn git_config_list(self) -> String {
        parse_config_file(self.content.as_str())
    }
}


impl LocalFileConfigLoader {
    pub(crate) fn new(filename: PathBuf, allow_missing: bool) -> LocalFileConfigLoader {
        LocalFileConfigLoader {
            filename,
            allow_missing,
        }
    }
}

impl ConfigLoaderTrait for LocalFileConfigLoader {
    fn fetch_remote_config(&self) { todo!() }
    fn git_config_list(self) -> String {
        todo!();
        match fs::read_to_string(&self.filename) {
            Ok(config) => config,
            Err(_) if self.allow_missing => String::new(),
            Err(err) => panic!("Failed to read config from {}", self.filename.to_str().unwrap()),
        }
    }
}


impl GitRemoteConfigLoader<'_> {
    pub(crate) fn new(
        url: String,
        remote_ref: String,
        filename: PathBuf,
        local_repo: &Repo,
        local_ref: String,
    ) -> GitRemoteConfigLoader {
        GitRemoteConfigLoader {
            url,
            remote_ref,
            filename,
            local_repo,
            local_ref,
        }
    }
}

impl ConfigLoaderTrait for GitRemoteConfigLoader<'_> {
    fn fetch_remote_config(&self) { todo!() }
    fn git_config_list(self) -> String { todo!() }
}
