#![allow(dead_code)]

use enum_dispatch::enum_dispatch;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use anyhow::{Context, Result};
use crate::config::ConfigMap;
use crate::repo::Repo;
use crate::util::{commit_env, log_run_git};


#[enum_dispatch]
pub trait ConfigLoaderTrait {
    fn git_config_list(self) -> String;
    fn get_configmap(self) -> Result<ConfigMap>
    where
        Self: Sized,
    {
        ConfigMap::parse(&self.git_config_list())
            .context("Could not load config.")
    }
}

pub enum ConfigLoader<'a> {
    Local(LocalFileConfigLoader),
    Remote(RemoteGitConfigLoader<'a>),
}

pub struct LocalGitConfigLoader<'a> {
    repo: &'a Repo, // <---- This reference causes lifetime voodoo.
}

pub struct LocalFileConfigLoader {
    filename: PathBuf,
    allow_missing: bool,
}

pub struct RemoteGitConfigLoader<'a> {
    url: String,
    remote_ref: String,
    local_repo: &'a Repo, // <---- This reference causes lifetime voodoo.
    local_ref: String,
}

////////////////////////////////////////////////////////////////////////////////////////////////////

fn parse_config_file(config_file: &str) -> String {
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

impl LocalGitConfigLoader<'_> {
    pub fn new(repo: &Repo) -> LocalGitConfigLoader {
        LocalGitConfigLoader {
            repo
        }
    }
}

impl ConfigLoaderTrait for LocalGitConfigLoader<'_> {
    fn git_config_list(self) -> String {
        let command = Command::new("git")
            .args(["-C", self.repo.path.to_str().unwrap().trim()])
            .args(["config", "--list"])
            .output()
            .unwrap();

        let ret = String::from_utf8(command.stdout)
            .expect("Could not load Local git configuration.");
        ret
    }
}

impl LocalFileConfigLoader {
    pub fn new(filename: PathBuf, allow_missing: bool) -> LocalFileConfigLoader {
        LocalFileConfigLoader {
            filename,
            allow_missing,
        }
    }
}

impl ConfigLoaderTrait for LocalFileConfigLoader {
    fn git_config_list(self) -> String {
        let mut content = String::new();
        if self.filename.exists() || !self.allow_missing {
            File::open(self.filename.clone())
                .expect(&format!("File '{}' does not exist!", self.filename.to_str().unwrap()))
                .read_to_string(&mut content).unwrap();
        }

        parse_config_file(&content)
    }
}


impl RemoteGitConfigLoader<'_> {
    pub fn new(
        url: String,
        remote_ref: String,
        local_repo: &Repo,
        local_ref: String,
    ) -> RemoteGitConfigLoader {
        RemoteGitConfigLoader {
            url,
            remote_ref,
            local_repo,
            local_ref,
        }
    }

    fn fetch_remote_config(&self) {
        log_run_git(
            Some(&self.local_repo.path),
            [
                "fetch",
                "--quiet",
                &self.url,
                &format!("+{}:{}", self.remote_ref, &self.local_ref),
            ],
            None,
            false,
            true,
        );
    }
}

impl ConfigLoaderTrait for RemoteGitConfigLoader<'_> {
    // First fetch:
    //   Running "git" "-C" "." "fetch" "--quiet" "ssh://csp-gerrit-ssh.volvocars.net/csp/hp/super" "refs/meta/git-toprepo:refs/toprepo/config/default"
    // The show and parse:
    //   git show refs/toprepo/config/default:toprepo.config \
    //   | git config --file - --list
    fn git_config_list(self) -> String {
        /*
        let completed = log_run_git(
            Some(&self.local_repo.path),
            [
                "show",
                "--quiet",
                &format!("{}:{}", self.local_ref, "toprepo.config"),
            ],
            false,
            true,
        );

        let ok =  completed.unwrap().unwrap();
        */

        /*
        let filtered = log_run_git(
            None,
            [
                "config",
                "--file=-",
                "--list",
            ],
            false,
            true,
        );
        */

        // TODO: Unify this with the `log_run_git` code.
        let raw_config = Command::new("git")
            .args([
                  "show",
                  "--quiet",
                  &format!("{}:{}", self.local_ref, "toprepo.config"),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("Could not show git-toprepo config ref.")
            ;

        let porcelain = Command::new("git")
            .args(["config", "--file=-", "--list"])
            .stdin(raw_config.stdout.unwrap())
            .output()
            .expect("Could not parse git-config syntax")
            ;

        let s = match String::from_utf8(porcelain.stdout) {
            Ok(s) => s,
            Err(e) => {
                panic!("{:?}", e);
            }
        };

        s
    }
}
