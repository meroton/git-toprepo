use std::io;
use std::path::PathBuf;
use std::process::Command;
use itertools::Itertools;
use crate::git::CommitHash;

pub type RawUrl = String;
pub type Url = String;

/// Normalize a path in the abstract, without filesystem accesses.
///
/// This is not guaranteed to give correct paths,
/// Notably, it will be incorrect in the presence of mounts or symlinks.
/// But if the paths are known to be free of links,
/// this is faster than `realpath(3)` et al.
///
/// ```
/// assert_eq!(git_toprepo::util::normalize("A/b/../C"), "A/C");
/// assert_eq!(git_toprepo::util::normalize("B/D"), "B/D");
/// assert_eq!(git_toprepo::util::normalize("E//./F"), "E/F");
/// ```
pub fn normalize(p: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    let parts = p.split("/");
    for p in parts {
        if p == "" || p == "." {
            continue
        }
        if p == ".." {
            stack.pop();
        } else {
            stack.push(p)
        }
    }

    stack.into_iter().map(|s| s.to_owned()).join("/")
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

    // TODO: Escape and quote! String representations are always annoying.
    let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
    let joined = args.into_iter().map(|s| s.to_str().unwrap()).join(" ");
    let display = format!("{} {}", command.get_program().to_str().unwrap(), joined);

    if dry_run {
        eprintln!("Would run   {}", display);
        None
    } else {
        if log_command {
            eprintln!("Running   {}", display);
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
