use crate::git::CommitHash;
use anyhow::{bail, Context};
use itertools::Itertools;
use std::collections::HashMap;
use std::io;
use std::path;
use std::path::PathBuf;
use std::process::Command;

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
    repo: Option<&PathBuf>, // Should this be a &Path?
    args: I,
    env: Option<&HashMap<String, String>>,
    check: bool,
    log_command: bool,
    //kwargs: HashMap<(), ()> TODO?
) -> anyhow::Result<std::process::Output>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut command = Command::new("git");

    if let Some(repo) = repo {
        command.args(["-C", repo.to_str().unwrap()]);
    }

    command.args(args);

     if let Some(e) = env {
        command.envs(e);
    }

    // TODO: Escape and quote! String representations are always annoying.
    let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
    let joined = args.into_iter().map(|s| s.to_str().unwrap()).join(" ");
    let display = format!("{} {}", command.get_program().to_str().unwrap(), joined);

        if log_command {
            eprintln!("Running   {}", display);
            Some(command.output());
        }


    let command_result = command.output();
    if let Ok(output) = &command_result {
        if check && !output.status.success() {
            bail!(output.status.to_string());
        }
    }
    command_result.context("Non-zero result")
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
    I: IntoIterator<Item = &'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}

pub fn commit_hash(hash: &str) -> CommitHash {
    hash.bytes().collect_vec().into()
}

pub fn commit_env() -> HashMap<String, String> {
    let mut hashmap = HashMap::new();
    let env = [
        ("GIT_AUTHOR_NAME", "A Name"),
        ("GIT_AUTHOR_EMAIL", "a@no.domain"),
        ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
        ("GIT_COMMITTER_NAME", "C Name"),
        ("GIT_COMMITTER_EMAIL", "c@no.domain"),
        ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
    ];
    env.map(|(k, v)| hashmap.insert(k.to_string(), v.to_string()));

    hashmap
}

#[derive(Debug)]
/// A struct for example repo structures. The example repo consists of repos `top` and `sub`, with `sub` being a submodule in `top`. The commit history is shown below:
/// ```text
/// top  A---B---C---D-------E---F---G---H
///          |       |       |       |
/// sub  1---2-------3---4---5---6---7---8
/// ```
///
/// # Examples
///
/// ```rust
/// // The crate `tempfile` is used here to create temporary directories for testing
/// use tempfile::tempdir;
/// use git_toprepo::util::GitTopRepoExample;
///
/// let tmp_dir = tempdir().unwrap();
/// let tmp_path = tmp_dir.path().to_path_buf();
///
/// // Use this instead for a persistent directory:
/// // let tmp_path = tmp_dir.into_path();
///
/// let repo = GitTopRepoExample::new(&tmp_path);
/// let top_repo_path = repo.init_server_top();
/// assert!(top_repo_path.exists());
/// ```
pub struct GitTopRepoExample {
    pub tmp_path: PathBuf,
    // TODO: store top/sub paths?
}

impl GitTopRepoExample {
    pub fn new(tmp_path: &PathBuf) -> GitTopRepoExample {
        GitTopRepoExample {
            tmp_path: tmp_path.to_path_buf(),
        }
    }

    pub fn init_server_top(&self) -> PathBuf {
        //! Sets up the repo structure and returns the top repo path.
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let sub_repo = self.tmp_path.join("sub").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&sub_repo).unwrap();

        log_run_git(
            Some(&top_repo),
            ["init", "--quiet", "--initial-branch", "main"],
            Some(&env),
            false,
            false,
        )
        .unwrap();

        log_run_git(
            Some(&sub_repo),
            ["init", "--quiet", "--initial-branch", "main"],
            Some(&env),
            false,
            false,
        )
        .unwrap();

        commit(&sub_repo, &env, "1");
        commit(&sub_repo, &env, "2");
        commit(&top_repo, &env, "A");

        log_run_git(
            Some(&top_repo),
            [
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "../sub/", // TODO: Absolute or relative path?
                           // sub_repo.to_str().unwrap(),
            ],
            Some(&env),
            false,
            false,
        )
        .unwrap();
        commit(&top_repo, &env, "B");
        commit(&top_repo, &env, "C");
        let sub_rev_3 = commit(&sub_repo, &env, "3");
        update_index_submodule(&top_repo, &env, sub_rev_3);

        commit(&top_repo, &env, "D");
        commit(&sub_repo, &env, "4");
        let sub_rev_5 = commit(&sub_repo, &env, "5");
        update_index_submodule(&top_repo, &env, sub_rev_5);

        commit(&top_repo, &env, "E");
        commit(&top_repo, &env, "F");
        commit(&sub_repo, &env, "6");
        let sub_rev_7 = commit(&sub_repo, &env, "7");
        update_index_submodule(&top_repo, &env, sub_rev_7);

        commit(&top_repo, &env, "G");
        commit(&top_repo, &env, "H");
        commit(&sub_repo, &env, "8");

        top_repo
    }
}

fn commit(repo: &PathBuf, env: &HashMap<String, String>, message: &str) -> String {
    log_run_git(
        Some(&repo),
        ["commit", "--allow-empty", "-m", message],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    // Returns commit hash as String.
    // TODO: Return Result<String> instead?
    command_output_to_string(
        log_run_git(Some(&repo), ["rev-parse", "HEAD"], Some(&env), false, false).unwrap(),
    )
    .trim()
    .to_string()
}

fn update_index_submodule(repo: &PathBuf, env: &HashMap<String, String>, commit: String) {
    log_run_git(
        Some(repo),
        [
            "update-index",
            "--cacheinfo",
            &format!("160000,{},sub", commit),
        ],
        Some(&env),
        false,
        false,
    )
    .unwrap();
}

pub fn command_output_to_string(
    command_output: std::process::Output,
) -> String {
    String::from_utf8(command_output.stdout)
        .unwrap()
        .trim()
        .to_string()
}

pub fn get_basename(name: &str) -> String {
    path::Path::new(name)
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}
