#![allow(dead_code)]

use crate::git::CommitHash;
use crate::gitmodules::join_submodule_url;
use crate::repo::Repo;
use crate::util::{
    get_basename, git_command, git_config_get, iter_to_string, OutputExtension, RawUrl, Url,
};
use anyhow::{bail, Context, Result};
use chrono::TimeZone as _;
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

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
    repo: BTreeMap<String, SubrepoConfig>,
    repos: RepoListConfig,
}

impl GitTopRepoConfig {
    fn new() -> Self {
        GitTopRepoConfig::default()
    }

    pub fn get_subrepo_config(&mut self, repo_url: &str) -> &SubrepoConfig {
        let repo_name = get_basename(repo_url);
        if !self.repo.contains_key(&repo_name) {
            self.repo.insert(
                String::from(&repo_name),
                SubrepoConfig {
                    urls: vec![String::from(repo_url)],
                    ..Default::default()
                },
            );
        }
        self.repo.get(&repo_name).unwrap()
    }

    /// Loads the configuration from a repository.
    ///
    /// The location of the configuration file is set in the git-config
    /// of the super repository using
    /// `git config --local toprepo.config <ref>:<git-repo-relative-path>`
    /// which defaults to `refs/remotes/origin/HEAD:.gittoprepo.toml`.
    /// An empty `ref`, for example `:.gittoprepo.user.toml`, means that the
    /// file is read from the worktree instead of the commits.
    ///
    /// Overriding the location is only recommended for testing out a new
    /// config and debugging purpose.
    pub fn load_config_from_repo(repo_dir: &Path) -> Result<Self> {
        // Load config file location.
        let config_location = git_config_get(repo_dir, "toprepo.config")?;
        let (using_default_location, config_location) = match &config_location {
            Some(location) => (false, location.as_str()),
            None => (true, "refs/toprepo-super/HEAD:.gittoprepo.toml"),
        };

        if config_location.starts_with(":") {
            let config_path = repo_dir.join(&config_location[1..]);
            Self::load_config_from_file(&config_path).with_context(|| {
                format!(
                    "Loading from configured toprepo.config path in {}",
                    repo_dir.display()
                )
            })
        } else {
            || -> Result<GitTopRepoConfig> {
                // Read config file from the repository.
                let config_toml_output = git_command(&repo_dir)
                    .args(["show", &config_location])
                    .output()?;
                match config_toml_output.check_success_with_stderr() {
                    Ok(_) => {
                        let config_toml = String::from_utf8(config_toml_output.stdout)?.to_string();
                        Self::load_config_from_toml_string(&config_toml)
                    }
                    Err(e) => {
                        if using_default_location {
                            // If no config location has been specified, having the file is optional.
                            return Ok(GitTopRepoConfig::default());
                        } else {
                            bail!(e)
                        }
                    }
                }
            }()
            .with_context(|| format!("Loading config from {}", config_location))
        }
    }

    pub fn load_config_from_file(config_path: &Path) -> Result<GitTopRepoConfig> {
        || -> Result<GitTopRepoConfig> {
            let config_toml = std::fs::read_to_string(config_path)?;
            Self::load_config_from_toml_string(&config_toml)
        }()
        .context(format!(
            "Could not read config file {}",
            config_path.display()
        ))
    }

    pub fn load_config_from_toml_string(config_toml: &str) -> Result<GitTopRepoConfig> {
        let mut res: GitTopRepoConfig =
            toml::from_str(&config_toml).context("Could not parse TOML string")?;
        res.validate()?;
        Ok(res)
    }

    /// Validates that the configuration is sane.
    pub fn validate(&mut self) -> Result<()> {
        for (repo_name, subrepo_config) in self.repo.iter_mut() {
            subrepo_config
                .validate()
                .with_context(|| format!("Invalid subrepo configuration for {}", repo_name))?;
        }
        self.ensure_unique_urls()?;
        Ok(())
    }

    fn ensure_unique_urls(&self) -> Result<()> {
        let mut found = HashMap::<String, String>::new();
        for (repo_name, v) in self.repo.iter() {
            for url in v.urls.iter() {
                match found.entry(url.to_string()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(repo_name.clone());
                    }
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        let existing_repo_name = entry.get();
                        bail!(
                            "URLs must be unique across all repos, found {} in {} and {}",
                            url,
                            existing_repo_name,
                            repo_name
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for GitTopRepoConfig {
    fn default() -> Self {
        GitTopRepoConfig {
            repo: BTreeMap::default(),
            repos: RepoListConfig {
                filter: repos_filter_default(),
            },
        }
    }
}

/// SubrepoConfig holds the configuration for a subrepo in the super repo. In
/// case `fetch.url` is empty, the first entry in `urls` is used. In case
/// `push.url` is empty, the value of `fetch.url` is used. A sane configuration
/// has either exactly one entry in `urls` or a fetch URL set.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SubrepoConfig {
    #[serde(default = "repo_since_default")]
    pub since: chrono::DateTime<chrono::Utc>,
    pub urls: Vec<String>,
    pub commits: CommitFilterConfig,
    pub fetch: FetchConfig,
    pub push: PushConfig,
}

impl SubrepoConfig {
    pub fn new(url: &str) -> Self {
        let mut repo_table = SubrepoConfig::default();
        repo_table.fetch.url = url.to_string();
        repo_table.push.url = url.to_string();
        repo_table
    }

    /// Validates that the configuration is sane.
    /// This will check that a fetch URL is set
    /// if `urls` does not contain exacly one entry.
    pub fn validate(&self) -> Result<()> {
        // Set fetch/push urls.
        if self.fetch.url.is_empty() && self.urls.len() != 1 {
            bail!("Either .fetch.url needs to be set or .urls must have exactly one element")
        }
        Ok(())
    }

    pub fn resolve_fetch_url(&self) -> &str {
        if self.fetch.url.is_empty() {
            assert!(self.validate().is_ok());
            &self.urls[0]
        } else {
            &self.fetch.url
        }
    }

    pub fn resolve_push_url(&self) -> &str {
        if self.push.url.is_empty() {
            self.resolve_fetch_url()
        } else {
            &self.push.url
        }
    }

    pub fn get_fetch_config_with_url(&self) -> FetchConfig {
        let mut fetch = self.fetch.clone();
        if fetch.url.is_empty() {
            fetch.url = self.resolve_fetch_url().to_owned();
        }
        fetch
    }

    pub fn get_push_config_with_url(&self) -> PushConfig {
        let mut push = self.push.clone();
        if push.url.is_empty() {
            push.url = self.resolve_push_url().to_owned();
        }
        push
    }
}

impl Default for SubrepoConfig {
    fn default() -> Self {
        SubrepoConfig {
            since: repo_since_default(),
            urls: Vec::default(),
            commits: CommitFilterConfig::default(),
            fetch: FetchConfig::default(),
            push: PushConfig::default(),
        }
    }
}

fn repo_since_default() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.timestamp_micros(0).unwrap()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CommitFilterConfig {
    pub missing: Vec<String>,
    pub override_parents: BTreeMap<String, Vec<String>>,
}

impl Default for CommitFilterConfig {
    fn default() -> Self {
        CommitFilterConfig {
            missing: Vec::new(),
            override_parents: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct FetchConfig {
    pub args: Vec<String>,
    #[serde(default = "fetch_prune_default")]
    pub prune: bool,
    pub refspecs: Vec<String>,
    pub url: String,
}

impl Default for FetchConfig {
    fn default() -> Self {
        FetchConfig {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PushConfig {
    pub args: Vec<String>,
    pub url: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        PushConfig {
            args: Vec::new(),
            url: String::new(),
        }
    }
}

/// Configuration for the list of subrepos to use.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RepoListConfig {
    /// Tells which sub repos to use. Each value starts with `+` or `-` followed
    /// by a regexp that should match a whole repository name. The expressions
    /// are matched in the order specified and the first matching regex decides
    /// whether the repo should be expanded or not.
    ///
    /// Defaults to `["+.*"]`.
    #[serde(default = "repos_filter_default")]
    pub filter: Vec<RepoListFilter>,
}

impl Default for RepoListConfig {
    fn default() -> Self {
        RepoListConfig {
            filter: repos_filter_default(),
        }
    }
}

fn repos_filter_default() -> Vec<RepoListFilter> {
    vec![RepoListFilter::new(true, Regex::new("^.*$").unwrap()).unwrap()]
}

#[derive(Debug, serde_with::DeserializeFromStr, serde_with::SerializeDisplay)]
pub struct RepoListFilter {
    pub wanted: bool,
    pattern: Regex,
}

impl RepoListFilter {
    pub fn new(wanted: bool, pattern: Regex) -> Result<Self> {
        if !pattern.as_str().starts_with('^') || !pattern.as_str().ends_with('$') {
            bail!("Pattern must start with ^ and end with $");
        }
        Ok(RepoListFilter { wanted, pattern })
    }
}

impl Display for RepoListFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let wanted_str = if self.wanted { "+" } else { "-" };
        let full_pattern_str = self.pattern.as_str();
        assert!(full_pattern_str.starts_with('^'));
        assert!(full_pattern_str.ends_with('$'));
        let short_pattern_str = &full_pattern_str[1..full_pattern_str.len() - 1];
        write!(f, "{}{}", wanted_str, short_pattern_str)
    }
}

impl std::str::FromStr for RepoListFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = match s.chars().next() {
            Some('+') => true,
            Some('-') => false,
            _ => return Err("Expected + or - prefix".to_string()),
        };
        let pattern = match Regex::new(&format!("^{}$", &s[1..])) {
            Ok(r) => r,
            Err(e) => return Err(format!("Invalid regex: {}", e)),
        };
        Ok(RepoListFilter { wanted, pattern })
    }
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

    #[test]
    fn test_config_repo_is_wanted() {
        assert!(Config::repo_is_wanted("Repo", &iter_to_string(["+Repo"])).unwrap());
        assert!(!Config::repo_is_wanted("Repo", &iter_to_string(["+Repo", "-Repo"])).unwrap());
        assert!(Config::repo_is_wanted("Repo", &iter_to_string(["+R"])).is_none());
        assert!(Config::repo_is_wanted("Repo", &iter_to_string(["-o"])).is_none());
        assert!(Config::repo_is_wanted("Repo", &iter_to_string(["-.*", "+Repo"])).unwrap());
        assert!(!Config::repo_is_wanted("Repo", &iter_to_string(["+.*", "-Repo"])).unwrap());
    }

    #[test]
    #[should_panic]
    fn test_create_config_from_invalid_ref() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = crate::util::commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["config", "toprepo.config", ":foobar.toml"])
            .envs(&env)
            .output()
            .unwrap();

        GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();
    }

    #[test]
    fn test_create_config_from_worktree() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = crate::util::commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .output()
            .unwrap();

        let mut tmp_file = std::fs::File::create(tmp_path.join("foobar.toml")).unwrap();

        writeln!(
            tmp_file,
            r#"[repo]
[repo.foo.fetch]
url = "ssh://bar/baz.git"
[repos]"#
        )
        .unwrap();

        git_command(&tmp_path)
            .args(["add", "foobar.toml"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["config", "toprepo.config", ":foobar.toml"])
            .envs(&env)
            .output()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();

        assert!(config.repo.contains_key("foo"));
        assert_eq!(
            config.repo.get("foo").unwrap().resolve_fetch_url(),
            "ssh://bar/baz.git"
        );
        assert_eq!(
            config.repo.get("foo").unwrap().resolve_push_url(),
            "ssh://bar/baz.git"
        );
        assert_eq!(config.repos.filter.len(), 1);
        assert_eq!(config.repos.filter[0].to_string(), "+.*");
    }

    #[test]
    fn test_create_config_from_empty_string() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = crate::util::commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "--allow-empty", "-m", "Initial commit"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["update-ref", "refs/toprepo-super/HEAD", "HEAD"])
            .envs(&env)
            .output()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();

        assert!(config.repo.is_empty());
        assert_eq!(config.repos.filter.len(), 1);
        assert_eq!(config.repos.filter[0].to_string(), "+.*");
    }

    #[test]
    fn test_create_config_from_head() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = crate::util::commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .output()
            .unwrap();

        let mut tmp_file = std::fs::File::create(tmp_path.join(".gittoprepo.toml")).unwrap();

        writeln!(
            tmp_file,
            r#"[repo]
[repo.foo.fetch]
url = "ssh://bar/baz.git"
[repos]"#
        )
        .unwrap();

        git_command(&tmp_path)
            .args(["add", ".gittoprepo.toml"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "-m", "Initial commit"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["update-ref", "refs/toprepo-super/HEAD", "HEAD"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["rm", ".gittoprepo.toml"])
            .envs(&env)
            .output()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "-m", "Remove .gittoprepo.toml"])
            .envs(&env)
            .output()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();

        assert!(config.repo.contains_key("foo"));
        assert_eq!(
            config.repo.get("foo").unwrap().resolve_fetch_url(),
            "ssh://bar/baz.git"
        );
        assert_eq!(
            config.repo.get("foo").unwrap().resolve_push_url(),
            "ssh://bar/baz.git"
        );
        assert_eq!(config.repos.filter.len(), 1);
        assert_eq!(config.repos.filter[0].to_string(), "+.*");
    }

    #[test]
    fn test_get_repo_with_new_entry() {
        let mut config = GitTopRepoConfig::load_config_from_toml_string("").unwrap();

        config.get_subrepo_config("ssh://bar/baz.git");

        assert!(config.repo.contains_key("baz"));
        assert_eq!(config.repos.filter.len(), 1);
        assert_eq!(config.repos.filter[0].to_string(), "+.*");
    }

    #[test]
    fn test_get_repo_without_new_entry() {
        let mut config = GitTopRepoConfig::load_config_from_toml_string(
            r#"[repo.foo]
        urls = ["../bar/repo.git"]

        [repos]"#,
        )
        .unwrap();

        config.get_subrepo_config("foo");

        assert_eq!(config.repo.len(), 1);
    }

    #[test]
    fn test_config_with_duplicate_urls() {
        let err = GitTopRepoConfig::load_config_from_toml_string(
            r#"[repo.foo]
        urls = ["ssh://bar/baz.git"]

        [repo.bar]
        urls = ["ssh://bar/baz.git"]

        [repos]"#,
        )
        .unwrap_err();
        assert_eq!(
            err.root_cause().to_string(),
            "URLs must be unique across all repos, found ssh://bar/baz.git in bar and foo"
        );
    }
}
