use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use itertools::Itertools;
use crate::config::Config;
use crate::git::{CommitHash,GitModuleInfo};
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
                    Some(i) => { parent.drain(i..); }

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

// TODO: Allow pipe to standard in?
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

pub fn strip_suffix<'a>(string: &'a str, suffix: &str) -> &'a str {
    if string.ends_with(suffix) {
        string.strip_suffix(suffix).unwrap()
    } else {
        string
    }
}

pub fn annotate_message(message: &str, subdir: &str, orig_commit_hash: &CommitHash) -> String {
    let mut res = message.trim_end_matches("\n").to_string() + "\n";
    if !res.contains("\n\n") {
        // Single-line message. Only a subject.
        res.push_str("\n")
    }

    format!("{}^-- {} {}\n", res, subdir, orig_commit_hash)
}

pub fn iter_to_string<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item=&'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}

pub fn commit_hash(hash: &str) -> CommitHash {
    hash.bytes().collect_vec().into()
}
