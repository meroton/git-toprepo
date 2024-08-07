use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use itertools::Itertools;
use crate::config::Config;
use crate::git::GitModuleInfo;
use crate::repo::TopRepo;


pub type RawUrl = String;
pub type Url = String;


pub fn join_submodule_url(parent: &str, mut other: &str) -> String {
    if other.starts_with("./") || other.starts_with("../") || other == "." {
        let scheme_end = match parent.find("://") {
            Some(i) => i + 3,
            None => 0,
        };
        let (scheme, parent) = parent.split_at(scheme_end);
        let mut parent = parent.trim_end_matches("/").to_string();

        loop {
            if other.starts_with("/") {
                (_, other) = other.split_at(1);
            } else if other.starts_with("./") {
                (_, other) = other.split_at(2);
            } else if other.starts_with("../") {
                match parent.rfind("/") {
                    Some(i) => { parent.drain(..i); }

                    //Too many "../", move it from other to parent.
                    None => parent += "/..",
                }

                (_, other) = other.split_at(3);
            } else {
                break;
            }
        }

        return if other == "." || other.is_empty() {
            format!("{}{}", scheme, parent)
        } else {
            format!("{}{}/{}", scheme, parent, other)
        };
    }

    other.to_string()
}

pub fn log_run_git<'a, I>(
    repo: Option<&PathBuf>,
    args: I,
    dry_run: bool,
    log_command: bool,
    //kwargs: HashMap<(), ()> TODO?
) -> Option<io::Result<std::process::Output>>
where
    I: IntoIterator<Item=&'a str>,
{
    let mut command = Command::new("git");

    if let Some(repo) = repo {
        command.args(["-C", repo.to_str().unwrap()]);
    }

    command.args(args);

    if dry_run {
        println!("Would run {:?}", command);
        None
    } else {
        if log_command {
            println!("Running {:?}", command);
        }

        Some(command.output())
    }
}

pub fn remote_to_repo(remote: &str, mut git_modules: Vec<GitModuleInfo>, config: Config) ->
Option<(String, Option<GitModuleInfo>)> {
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
            Some((name, Some(gitmod)))
        }
        (name, None) => Some((name.to_string(), None))
    }
}

pub fn iter_to_string<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item=&'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}