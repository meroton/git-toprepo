use std::{env, io};
use std::path::PathBuf;
use std::process::Command;
use lazycell::LazyCell;
use crate::config::{Config, RepoConfig};
use crate::git::determine_git_dir;
use crate::util::iter_to_string;

const DEFAULT_FETCH_ARGS: [&str; 3] = ["--prune", "--prune-tags", "--tags"];

#[derive(Debug)]
pub(crate) struct Repo {
    name: String,
    pub(crate) path: PathBuf,
    git_dir: LazyCell<PathBuf>,
}

impl Repo {
    pub(crate) fn new(repo: &str) -> Repo {
        println!("Repo: {}", repo);

        //PosixPath('/home/lack/Documents/Kod/RustRover/git-toprepo')
        let command = Command::new("git")
            .args(["-C", repo])
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .expect(format!("Failed to parse repo path {}", repo).as_str());
        println!("stdout: {:?}", command.stdout);
        let path = String::from_utf8(command.stdout).unwrap()
            .strip_suffix("\n").unwrap().to_string();

        let cwd = env::current_dir().unwrap_or(PathBuf::new());
        let mut path = PathBuf::from(path);

        if path == cwd {
            path = PathBuf::from(".")
        }
        if let Ok(relative) = path.strip_prefix(cwd) {
            path = relative.to_path_buf();
        }

        println!("Path: {:?}", path);

        Repo {
            name: "mono repo".to_string(),
            path,
            git_dir: LazyCell::new(),
        }
    }

    fn from_config(path: PathBuf, config: Config) -> Repo {
        todo!()
    }

    pub(crate) fn get_toprepo_fetch_url(&self) -> String {
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

        //println!("{:?}", command);

        command
    }

    pub(crate) fn get_toprepo_dir(&self) -> PathBuf {
        self.get_subrepo_dir(TopRepo::NAME)
    }

    fn get_subrepo_dir(&self, name: &str) -> PathBuf {
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
pub(crate) struct TopRepo {
    name: String,
    path: PathBuf,
    config: RepoConfig,
}

impl TopRepo {
    pub(crate) const NAME: &'static str = "top";

    fn new(path: PathBuf, fetch_url: &String, push_url: &String) -> TopRepo {
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
    pub(crate) fn from_config(repo: PathBuf, config: &Config) -> TopRepo {
        TopRepo::new(
            repo,
            &config.top_fetch_url,
            &config.top_push_url,
        )
    }
}


pub(crate) struct RepoFetcher<'a> {
    monorepo: &'a Repo
}

impl RepoFetcher<'_> {

    pub(crate) fn new(monorepo: &Repo) -> RepoFetcher {
        RepoFetcher {
            monorepo
        }
    }

    fn fetch_repo(&self) {
        todo!()
    }
}