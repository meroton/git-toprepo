#![allow(dead_code)]

use crate::config_loader::{
    ConfigLoader, ConfigLoaderTrait, LocalFileConfigLoader, LocalGitConfigLoader,
    RemoteGitConfigLoader,
};
use crate::git::CommitHash;
use crate::gitmodules::join_submodule_url;
use crate::repo::Repo;
use crate::util::{
    command_output_to_string, get_basename, iter_to_string, log_run_git, RawUrl, Url,
};
use anyhow::{bail, Context, Result};
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

//TODO: Create proper error enums instead of strings

/*
Toprepo, super repo har alla submoduler
Monorepo, utsorterat
 */
const DEFAULT_FETCH_ARGS: [&str; 3] = ["--prune", "--prune-tags", "--tags"];

const TOPREPO_CONFIG_DEFAULT_KEY: &str = "toprepo.config.default";
const TOPREPO_DEFAULT_URL: &str = ".";
const TOPREPO_DEFAULT_REF: &str = "refs/meta/git-toprepo";
const TOPREPO_DEFAULT_NAME: &str = "default";

#[derive(Debug)]
pub struct RepoConfig {
    /// Name of the storage directory and used for pattern matching.
    pub name: String,

    /// Flags if this repos should be expanded or not.
    pub enabled: bool,

    /// Exact matching against sub repos configs like .gitmodules.
    ///
    /// These URLs are not resolved any may be relative.
    pub raw_urls: Vec<RawUrl>,

    /// Absolute URL to git-fetch from.
    pub fetch_url: Url, //TODO: Borrow these from Config?

    /// extra options for git-fetch.
    pub fetch_args: Vec<String>,

    /// Absolute URL to git-push to.
    pub push_url: Url,
}

///////////////////////////////////////////////////////////////////////////////

const CONFIGMAP_UNSET: &str = "git_toprepo_ConfigDict_unset"; //Should this be replaced by None?

//https://git-scm.com/docs/git-config#_configuration_file
#[derive(Debug)]
pub struct ConfigMap {
    pub map: HashMap<String, Vec<String>>,
}

pub type Mapping = HashMap<String, ConfigMap>;

impl ConfigMap {
    pub fn new() -> ConfigMap {
        ConfigMap {
            map: HashMap::new(),
        }
    }

    pub fn list(&self) {
        for (key, values) in &self.map {
            for val in values.into_iter() {
                println!("{}={}", key, val);
            }
        }
    }

    pub fn join<'a, I: IntoIterator<Item = &'a ConfigMap>>(configs: I) -> ConfigMap {
        let mut ret = ConfigMap::new();

        for config in configs {
            for (key, values) in &config.map {
                ret.append(&key, values.clone());
            }
        }

        ret
    }

    /// Parse git config. This must be filtered through `git config --file - --list`.
    /// The on-disk format with comments cannot be parsed.
    /// It must be the porcelain `toprepo.role.default.repos=+ci-docker` syntax.
    pub fn parse(config_lines: &str) -> Result<ConfigMap> {
        let mut ret = ConfigMap::new();

        for line in config_lines.split("\n").filter(|s| !s.is_empty()) {
            if let Some(needle) = line.find("=") {
                let key = &line[..needle];
                let value = &line[needle + 1..line.len()];

                ret.push(key, value.to_string());
            } else {
                // TODO: ideally (optionally) print the entire corpus that was
                // parsed and its source.
                bail!("Could not parse '{}'", line)
            }
        }

        Ok(ret)
    }

    pub fn get(&self, key: &str) -> Option<&Vec<String>> {
        self.map.get(key)
    }

    // Allow hierarchical notation.
    pub fn get_subkey(&self, a: &str, b: &str) -> Option<&Vec<String>> {
        let key = format!("{}.{}", a, b);
        self.get(key.as_ref())
    }

    pub fn get_last(&self, key: &str) -> Option<&str> {
        Some(self.map.get(key)?.last()?.as_str())
    }

    pub fn remove(&mut self, key: &str) -> Option<Vec<String>> {
        self.map.remove(key)
    }

    pub fn remove_last(&mut self, key: &str) -> Option<String> {
        self.map.get_mut(key)?.pop()
    }

    /// Inserts default value if key doesn't exist in the map
    pub fn set_default(&mut self, key: &str, default: Vec<String>) -> &Vec<String> {
        self.map.entry(key.to_string()).or_insert(default)
    }

    pub fn push(&mut self, key: &str, value: String) {
        self.map
            .entry(key.to_string())
            .or_insert(Vec::new())
            .push(value);
    }

    pub fn append(&mut self, key: &str, mut values: Vec<String>) {
        if !self.map.contains_key(key) {
            self.map.insert(key.to_string(), values);
        } else {
            self.map.get_mut(key).unwrap().append(&mut values);
        }
    }

    /// Extracts for example submodule.<name>.<key>=<value>.
    /// All entries that don't contain the prefix are returned in the residual.
    pub fn extract_mapping(&self, prefix: &str) -> Result<Mapping> {
        let mut prefix = prefix.to_string();
        if !prefix.ends_with('.') {
            prefix.push('.');
        }

        let mut extracted = HashMap::new();

        for (key, values) in &self.map {
            if let Some(temp) = key.strip_prefix(&prefix) {
                if let Some((name, subkey)) = temp.split(".").next_tuple() {
                    extracted
                        .entry(name.to_string())
                        .or_insert(ConfigMap::new())
                        .append(subkey, values.clone());
                } else {
                    bail!("Illegal config {}", temp);
                }
            }
        }

        Ok(extracted)
    }

    pub fn get_singleton(&self, key: &str) -> Option<&str> {
        let mut values = self.get(key)?.iter().sorted();

        match values.len() {
            0 => panic!("The key {} should not exist without a value!", key),
            1 => Some(values.next().unwrap()),
            _ => {
                None
                //panic!("Conflicting values for {}: {}", key, values.join(", "));
                //Err(format!("Conflicting values for {}: {}", key, values.join(", ")))
            }
        }
    }
}

impl Display for ConfigMap {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let temp = self
            .map
            .iter()
            .map(|(key, values)| format!("{}: [{}]", key, values.join(", ")))
            .join(", ");

        write!(f, "ConfigMap {{ {} }}", temp)?;

        Ok(())
    }
}

///////////////////////////////////////////////////////////////////////////////

pub fn get_configmap(monorepo: &Repo, git_config: &ConfigMap) -> ConfigMap {
    let configloader = get_config_loader(&monorepo, &git_config).unwrap();
    let toprepo_config = match configloader {
        ConfigLoader::Local(c) => c.get_configmap(),
        ConfigLoader::Remote(c) => c.get_configmap(),
    }
    .unwrap();
    ConfigMap::join(vec![&git_config, &toprepo_config])
}

fn get_config_loader<'a>(
    monorepo: &'a Repo,
    git_config: &'a ConfigMap,
) -> Result<ConfigLoader<'a>> {
    // TODO: configmap::get_last
    let last_wins = |vals: Option<&Vec<String>>| vals.map(|v| v[v.len() - 1].clone());

    let loader_type = last_wins(git_config.get_subkey(TOPREPO_CONFIG_DEFAULT_KEY, "type"))
        .map(|s| s.to_lowercase());
    if loader_type.is_none() {
        return Ok(ConfigLoader::Remote(RemoteGitConfigLoader::new(
            TOPREPO_DEFAULT_URL.to_owned(),
            TOPREPO_DEFAULT_REF.to_owned(),
            &monorepo,
            // TODO: default value in the constructor?,
            format!("refs/toprepo/config/{}", TOPREPO_DEFAULT_NAME),
        )));
    }

    let loader_type = loader_type.unwrap();
    let config_loader = match loader_type.as_str() {
        "file" => {
            let mut file_path = monorepo.path.clone();
            file_path.push(PathBuf::from(
                git_config.get_last("file").expect("Missing file"),
            ));

            ConfigLoader::Local(LocalFileConfigLoader::new(file_path, false))
        }
        "git" => {
            // Load
            let raw_url = git_config.get_last("url").expect("Missing url");
            let reference = git_config.get_last("ref").expect("Missing ref");

            /*
             * .
             * refs/meta/git-toprepo
             * toprepo.config
             */

            // Translate.
            let parent_url = monorepo.get_toprepo_fetch_url();
            let url = join_submodule_url(&parent_url, raw_url);

            // Parse.
            ConfigLoader::Remote(RemoteGitConfigLoader::new(
                url,
                reference.to_string(),
                &monorepo,
                format!("refs/toprepo/config/{}", TOPREPO_DEFAULT_NAME),
            ))
        }
        _ => {
            bail!("Invalid toprepo.config.type {}!", loader_type);
        }
    };

    Ok(config_loader)
}

#[derive(Debug)]
pub struct Config {
    pub missing_commits: HashMap<String, HashSet<CommitHash>>, // TODO What data type is a commit hash?
    pub top_fetch_url: String,
    pub top_push_url: String,
    pub repos: Vec<RepoConfig>,
}

impl Config {
    pub fn new(mut configmap: ConfigMap) -> Config {
        let repo_configmaps = configmap
            .extract_mapping("toprepo.repo.")
            .expect("Could not create config");

        // Resolve the role.
        configmap.set_default("toprepo.role.default.repos", vec!["+.*".to_string()]);
        let role = configmap.get_last("toprepo.role").unwrap_or("default");

        let wanted_repos_role = format!("toprepo.role.{}.repos", role);
        configmap.set_default(&wanted_repos_role, vec![]);
        let wanted_repos_patterns = configmap.remove(&wanted_repos_role).unwrap();

        let top_fetch_url = match configmap.remove_last("remote.origin.url").as_deref() {
            None | Some("file:///dev/null") => configmap
                .remove_last("toprepo.top.fetchurl")
                .expect("Config remote.origin.url is not set"),
            Some(url) => url.to_string(),
        };
        let top_push_url = match configmap.remove_last("remote.top.pushurl") {
            None => configmap
                .remove_last("toprepo.top.pushurl")
                .expect(&"Config remote.top.pushurl is not set"),
            Some(url) => url,
        };
        let repo_configs = Config::parse_repo_configs(
            repo_configmaps,
            wanted_repos_patterns,
            &top_fetch_url,
            &top_push_url,
        );

        // Find configured missing commits.
        let mut missing_commits = HashMap::new();
        let missing_commits_prefix = "toprepo.missing-commits.rev-";
        for (key, values) in configmap.map {
            if let Some(commit_hash) = key.strip_prefix(missing_commits_prefix) {
                let commit_hash: CommitHash = commit_hash.bytes().collect_vec().into();

                for raw_url in values {
                    missing_commits
                        .entry(raw_url)
                        .or_insert(HashSet::new())
                        .insert(commit_hash.clone());
                }
            }
        }

        Config {
            missing_commits,
            top_fetch_url: top_fetch_url.to_string(),
            top_push_url: top_push_url.to_string(),
            repos: repo_configs,
        }
    }

    fn parse_repo_configs(
        repo_configmaps: HashMap<String, ConfigMap>,
        wanted_repos_patterns: Vec<String>,
        parent_fetch_url: &str,
        parent_push_url: &str,
    ) -> Vec<RepoConfig> {
        let mut repo_configs = Vec::new();

        for (repo_name, repo_configmap) in repo_configmaps {
            repo_configs.push(Config::parse_repo_config(
                repo_name,
                repo_configmap,
                &wanted_repos_patterns,
                parent_fetch_url,
                parent_push_url,
            ))
        }

        repo_configs
    }

    fn parse_repo_config(
        name: String,
        mut repo_configmap: ConfigMap,
        wanted_repos_patterns: &Vec<String>,
        parent_fetch_url: &str,
        parent_push_url: &str,
    ) -> RepoConfig {
        // TODO: name check?
        if PathBuf::from(&name).components().count() != 1 {
            panic!("Subdirectories not allowed in repo name: {}", name);
        }

        let enabled = Config::repo_is_wanted(&name, wanted_repos_patterns)
            .expect(format!("Could not determine if repo {} is wanted or not", name).as_str());

        let mut raw_urls = repo_configmap
            .remove("urls")
            .expect(format!("toprepo.repo.{}.urls is unspecified", name).as_str());

        let raw_fetch_url = match repo_configmap.get_last("fetchurl") {
            None => {
                if raw_urls.len() != 1 {
                    panic!(
                        "Missing toprepo.repo.{}.fetchUrl and multiple \
                    toprepo.repo.{}.urls gives an ambiguous default",
                        name, name
                    )
                }

                raw_urls.pop().unwrap()
            }
            Some(url) => url.to_string(),
        };
        let fetch_url = join_submodule_url(parent_fetch_url, &raw_fetch_url);

        let raw_push_url = repo_configmap.get_last("pushurl").unwrap_or(&raw_fetch_url);
        let push_url = join_submodule_url(parent_push_url, raw_push_url);

        let fetch_args = repo_configmap
            .remove("fetchargs")
            .unwrap_or(iter_to_string(DEFAULT_FETCH_ARGS));

        RepoConfig {
            name,
            enabled,
            raw_urls,
            fetch_url,
            fetch_args,
            push_url,
        }
    }

    pub fn repo_is_wanted(name: &str, wanted_repos_patterns: &Vec<String>) -> Option<bool> {
        // The rev is required to maintain the behaviour from python.
        // The last pattern has priority.
        for pattern in wanted_repos_patterns.iter().rev() {
            if !pattern.starts_with(&['+', '-']) {
                panic!(
                    "Invalid wanted repo config {} for {}, \
                should start with '+' or '-' followed by a regex.",
                    pattern, name
                );
            }

            let wanted = pattern.starts_with('+');

            // Force whole string to match
            let pattern = format!("^{}$", &pattern[1..]);

            // Returns True if it matches with a '+' and false if it matches with a '-'.
            if Regex::new(&pattern).unwrap().is_match(name) {
                return Some(wanted);
            }
        }

        None
    }

    fn raw_url_to_repos(&self) -> HashMap<&str, Vec<&RepoConfig>> {
        let mut raw_url_to_repos = HashMap::new();
        for repo_config in &self.repos {
            for raw_url in &repo_config.raw_urls {
                raw_url_to_repos
                    .entry(raw_url.as_str())
                    .or_insert(Vec::new())
                    .push(repo_config);
            }
        }
        raw_url_to_repos
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct GitTopRepoConfig {
    pub repo: HashMap<String, RepoTable>,
    pub repos: ReposTable,
}

impl GitTopRepoConfig {
    fn new() -> Self {
        GitTopRepoConfig::default()
    }

    fn init(&mut self, repo_dir: Option<&PathBuf>) -> Result<()> {
        self.load_config(repo_dir)?;
        self.ensure_unique_urls()?;
        Ok(())
    }

    pub fn get_repo_config(&mut self, repo_url: &str) -> &RepoTable {
        let repo_name = get_basename(repo_url);
        if !self.repo.contains_key(&repo_name) {
            self.repo.insert(
                String::from(&repo_name),
                RepoTable {
                    urls: vec![String::from(repo_url)],
                    ..Default::default()
                },
            );
            self.init(None).unwrap();
        }
        self.repo.get(&repo_name).unwrap()
    }

    fn load_config(&mut self, repo_dir: Option<&PathBuf>) -> Result<()> {
        if let Some(_) = repo_dir {
            // Load config file location
            let config_location = command_output_to_string(log_run_git(
                repo_dir,
                ["config", "toprepo.config"],
                None,
                false,
                false,
            )?);

            // Read config file
            let config_toml: String;
            if config_location.is_empty() {
                // No config file location was specified, reading from default location
                // Will be an empty string if the file does not exist
                config_toml = command_output_to_string(log_run_git(
                    repo_dir,
                    ["show", "refs/toprepo-super/HEAD:.gittoprepo.toml"],
                    None,
                    false,
                    false,
                )?);
            } else {
                // Read config file from provided location
                // Will panic if the file does not exist
                config_toml = command_output_to_string(
                    log_run_git(repo_dir, ["show", &config_location], None, true, false)
                        .context(format!("Invalid config file location {}", config_location))?,
                );
            }

            // Load config from config file output (default config if empty output)
            let config: GitTopRepoConfig =
                toml::from_str(&config_toml).context("Could not parse TOML string")?;

            self.repo = config.repo;
            self.repos = config.repos;
        }

        // Set fetch/push urls
        for (_, v) in self.repo.iter_mut() {
            if v.fetch.url.is_empty() {
                if v.urls.len() == 1 {
                    v.fetch.url = String::from(v.urls.first().unwrap());
                } else {
                    bail!("Toprepo requires a submodule fetch url")
                }
            }

            if v.push.url.is_empty() {
                v.push.url = String::from(&v.fetch.url);
            }
        }
        Ok(())
    }

    fn ensure_unique_urls(&self) -> Result<()> {
        let mut set = HashSet::<String>::new();
        for (_, v) in self.repo.iter() {
            for url in v.urls.iter() {
                if set.contains(url) {
                    bail!("URLs must be unique across all repos");
                } else {
                    set.insert(String::from(url));
                }
            }
        }
        Ok(())
    }
}

impl Default for GitTopRepoConfig {
    fn default() -> Self {
        GitTopRepoConfig {
            repo: HashMap::default(),
            repos: ReposTable {
                filter: repos_filter_default(),
            },
        }
    }
}

impl FromStr for GitTopRepoConfig {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut repo_config: GitTopRepoConfig =
            toml::from_str(&value).context("Could not parse TOML string")?;
        repo_config
            .init(None)
            .context("Failed to initialize repo config from string")?;
        Ok(repo_config)
    }
}

impl TryFrom<&Path> for GitTopRepoConfig {
    type Error = anyhow::Error;

    fn try_from(repo_dir: &Path) -> Result<Self> {
        let mut top_repo_config = GitTopRepoConfig::new();
        top_repo_config
            .init(Some(&repo_dir.to_path_buf()))
            .context("Failed to initialize repo config from path")?;
        Ok(top_repo_config)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RepoTable {
    // TODO: String or Datetime?
    // pub since: toml::value::Datetime,
    #[serde(default = "repo_since_default")]
    pub since: String,
    pub urls: Vec<String>,
    pub commits: CommitsTable,
    pub fetch: FetchTable,
    pub push: PushTable,
}

impl RepoTable {
    pub fn new(url: &str) -> Self {
        let mut repo_table = RepoTable::default();
        repo_table.fetch.url = url.to_string();
        repo_table.push.url = url.to_string();
        repo_table
    }
}

impl Default for RepoTable {
    fn default() -> Self {
        RepoTable {
            since: repo_since_default(),
            urls: Vec::default(),
            commits: CommitsTable::default(),
            fetch: FetchTable::default(),
            push: PushTable::default(),
        }
    }
}

fn repo_since_default() -> String {
    "1970-01-01T00:00:00Z".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CommitsTable {
    pub missing: Vec<String>,
    pub override_parents: HashMap<String, Vec<String>>,
}

impl Default for CommitsTable {
    fn default() -> Self {
        CommitsTable {
            missing: Vec::new(),
            override_parents: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct FetchTable {
    pub args: Vec<String>,
    #[serde(default = "fetch_prune_default")]
    pub prune: bool,
    pub refspecs: Vec<String>,
    pub url: String,
}

impl Default for FetchTable {
    fn default() -> Self {
        FetchTable {
            args: Vec::new(),
            prune: fetch_prune_default(),
            refspecs: Vec::new(),
            url: String::new(),
        }
    }
}

fn fetch_prune_default() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PushTable {
    pub args: Vec<String>,
    pub url: String,
}

impl Default for PushTable {
    fn default() -> Self {
        PushTable {
            args: Vec::new(),
            url: String::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ReposTable {
    #[serde(default = "repos_filter_default")]
    pub filter: Vec<String>,
}

impl Default for ReposTable {
    fn default() -> Self {
        ReposTable {
            filter: repos_filter_default(),
        }
    }
}

fn repos_filter_default() -> Vec<String> {
    vec!["+.*".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_config() {
        let expected = |s: &str| Some(vec![s.to_owned()]);

        let cm = ConfigMap::parse("foo.bar=bazz").unwrap();
        let foo = cm.get("foo.bar");
        assert_eq!(foo, expected("bazz").as_ref());

        let cm = ConfigMap::parse("toprepo.repo.foo.fetchargs=--depth=1000000000").unwrap();
        let fetchargs = cm.get("toprepo.repo.foo.fetchargs");
        assert_eq!(fetchargs, expected("--depth=1000000000").as_ref());
    }

    #[test]
    fn parse_on_disk_representation() {
        let on_disk = "[toprepo.role.default]
        # Ignore all old repositories.
        repos = -.*";
        let cm = ConfigMap::parse(on_disk);
        assert!(cm.is_err());
    }

    #[test]
    fn join_config_maps() {
        let expected = |s: &str| Some(vec![s.to_owned()]);

        let a = ConfigMap::parse("a=1").unwrap();
        let b = ConfigMap::parse("b=2").unwrap();
        let joined = ConfigMap::join(vec![&a, &b]);

        assert_eq!(joined.get("a"), expected("1").as_ref());
        assert_eq!(joined.get("b"), expected("2").as_ref());
    }
}
