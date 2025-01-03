use anyhow::bail;
use bstr::ByteSlice;
use itertools::Itertools;
use std::collections::HashMap;
use std::path::{self, Path, PathBuf};
use std::process::Command;

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
            continue;
        }
        if p == ".." {
            stack.pop();
        } else {
            stack.push(p)
        }
    }

    stack.into_iter().map(|s| s.to_owned()).join("/")
}

pub trait CommandExtension {
    fn log_cmdline(&mut self) -> &mut Self;
    fn output_stdout_only(&mut self) -> std::io::Result<std::process::Output>;
}

impl CommandExtension for Command {
    fn log_cmdline(&mut self) -> &mut Self {
        // TODO: Escape and quote! String representations are always annoying.
        let args: Vec<&std::ffi::OsStr> = self.get_args().collect();
        let joined = args.into_iter().map(|s| s.to_str().unwrap()).join(" ");
        let display = format!("{} {}", self.get_program().to_str().unwrap(), joined);

        eprintln!("Running   {}", display);
        self
    }

    fn output_stdout_only(&mut self) -> std::io::Result<std::process::Output> {
        self.stderr(std::process::Stdio::inherit()).output()
    }
}

pub trait OutputExtension {
    fn check_success_with_stderr(&self) -> anyhow::Result<&Self>;
}

impl OutputExtension for std::process::Output {
    /// Checks that the command was successful and otherwise returns an error
    /// with the exit status together with the stderr content.
    fn check_success_with_stderr(&self) -> anyhow::Result<&Self> {
        // TODO: Print the command line as well?
        if !self.status.success() {
            if self.stderr.is_empty() {
                bail!("{}", self.status);
            } else {
                bail!(
                    "{}:\n{}",
                    self.status,
                    String::from_utf8_lossy(&self.stderr)
                );
            }
        }
        Ok(self)
    }
}

pub trait ExitStatusExtension {
    fn check_success(&self) -> anyhow::Result<&Self>;
}

impl ExitStatusExtension for std::process::ExitStatus {
    /// Checks that the command was successful and otherwise returns an error
    /// with the exit status.
    fn check_success(&self) -> anyhow::Result<&Self> {
        if !self.success() {
            bail!("{}", self);
        }
        Ok(self)
    }
}

pub fn git_global_command() -> Command {
    Command::new("git")
}

pub fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.args([std::ffi::OsStr::new("-C"), repo.as_os_str()]);
    command
}

/// Returns the value of a single entry git configuration key
/// or `None` if the key is not set.
pub fn git_config_get(repo: &Path, key: &str) -> anyhow::Result<Option<String>> {
    let res = git_command(repo).args(["config", key]).output()?;
    if res.status.code() == Some(1) {
        Ok(None)
    } else {
        res.check_success_with_stderr()?;
        Ok(Some(trim_newline_suffix(res.stdout.to_str()?).to_string()))
    }
}

/// Removes trailing LF or CRLF from a string.
///
/// # Examples
/// ```
/// use git_toprepo::util::trim_newline_suffix;
///
/// assert_eq!(trim_newline_suffix("foo"), "foo");
/// assert_eq!(trim_newline_suffix("foo\n"), "foo");
/// assert_eq!(trim_newline_suffix("foo\r\n"), "foo");
/// assert_eq!(trim_newline_suffix("foo\nbar\n"), "foo\nbar");
/// assert_eq!(trim_newline_suffix("foo\r\nbar\r\n"), "foo\r\nbar");
///
/// assert_eq!(trim_newline_suffix("foo\n\r"), "foo\n\r");
/// ```
pub fn trim_newline_suffix(s: &str) -> &str {
    if s.ends_with("\r\n") {
        &s[..s.len() - 2]
    } else if s.ends_with("\n") {
        &s[..s.len() - 1]
    } else {
        s
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

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&sub_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .output()
            .unwrap();

        commit(&sub_repo, &env, "1");
        commit(&sub_repo, &env, "2");
        commit(&top_repo, &env, "A");

        git_command(&top_repo)
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "../sub/", // TODO: Absolute or relative path?
                           // sub_repo.to_str().unwrap(),
            ])
            .envs(&env)
            .output()
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
    git_command(repo)
        .args(["commit", "--allow-empty", "-m", message])
        .envs(env)
        .output()
        .unwrap();

    // Returns commit hash as String.
    // TODO: Return Result<String> instead?
    trim_newline_suffix(
        git_command(&repo)
            .args(["rev-parse", "HEAD"])
            .envs(env)
            .output()
            .unwrap()
            .stdout
            .to_str()
            .unwrap(),
    )
    .to_string()
}

fn update_index_submodule(repo: &PathBuf, env: &HashMap<String, String>, commit: String) {
    git_command(repo)
        .args([
            "update-index",
            "--cacheinfo",
            &format!("160000,{},sub", commit),
        ])
        .envs(env)
        .output()
        .unwrap();
}

pub fn get_basename(name: &str) -> String {
    path::Path::new(name)
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_annotate_message() {
        // Don't fold the footer into the subject line, leave an empty line.
        assert_eq!(
            annotate_message("Subject line\n", "sub/dir", &commit_hash("123hash"),),
            "\
Subject line

^-- sub/dir 123hash
"
        );

        assert_eq!(
            annotate_message("Subject line, no LF", "sub/dir", &commit_hash("123hash"),),
            "\
Subject line, no LF

^-- sub/dir 123hash
"
        );

        assert_eq!(
            annotate_message("Double subject line\n", "sub/dir", &commit_hash("123hash"),),
            "\
Double subject line

^-- sub/dir 123hash
"
        );

        assert_eq!(
            annotate_message(
                "Subject line, extra LFs\n\n\n",
                "sub/dir",
                &commit_hash("123hash"),
            ),
            "\
Subject line, extra LFs

^-- sub/dir 123hash
",
        );

        assert_eq!(
            annotate_message(
                "Multi line\n\nmessage\n",
                "sub/dir",
                &commit_hash("123hash")
            ),
            "\
Multi line

message
^-- sub/dir 123hash
",
        );

        assert_eq!(
            annotate_message(
                "Multi line\n\nmessage, no LF",
                "sub/dir",
                &commit_hash("123hash"),
            ),
            "\
Multi line

message, no LF
^-- sub/dir 123hash
",
        );

        assert_eq!(
            annotate_message(
                "Multi line\n\nmessage, extra LFs\n\n\n",
                "sub/dir",
                &commit_hash("123hash"),
            ),
            "\
Multi line

message, extra LFs
^-- sub/dir 123hash
",
        )
    }
}
