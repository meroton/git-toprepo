#![allow(dead_code)]

use crate::git::CommitHash;
use crate::util::{
    get_basename, git_command, git_config_get, trim_newline_suffix, OutputExtension,
};
use anyhow::{bail, Context, Result};
use bstr::ByteSlice;
use chrono::TimeZone as _;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct GitTopRepoConfig {
    repo: BTreeMap<String, SubrepoConfig>,
    repos: RepoListConfig,
}

pub enum ConfigLocation {
    /// Load a blob from the repo.
    RepoBlob(String),
    /// Load from the path relative to the repository root.
    Worktree(PathBuf),
    /// Default empty configuration should be loaded.
    None,
}

impl Display for ConfigLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLocation::RepoBlob(blob) => write!(f, "blob: {}", blob),
            ConfigLocation::Worktree(path) => write!(f, "worktree file: {}", path.display()),
            ConfigLocation::None => write!(f, "<default-empty>"),
        }
    }
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

    /// Finds the location of the configuration to load.
    ///
    /// The location of the configuration file is set in the git-config of the
    /// super repository using `git config --local toprepo.config
    /// <ref>:<git-repo-relative-path>` which defaults to
    /// `refs/remotes/origin/HEAD:.gittoprepo.toml`. An empty `ref`, for example
    /// `:.gittoprepo.user.toml`, means that the file is read from the worktree
    /// instead of the commits.
    ///
    /// Overriding the location is only recommended for testing out a new config
    /// and debugging purpose.
    ///
    /// The `search_log` will be filled with information about how the config
    /// was found.
    ///
    /// Returns the configuration and the location it was loaded from.
    pub fn find_configuration_location(
        repo_dir: &Path,
        mut search_log: Option<&mut String>,
    ) -> Result<ConfigLocation> {
        // Load config file location.
        const GIT_CONFIG_KEY: &str = "toprepo.config";
        const DEFAULT_LOCATION: &str = "refs/toprepo-super/HEAD:.gittoprepo.toml";

        let user_location = git_config_get(repo_dir, GIT_CONFIG_KEY)?;
        let (using_default_location, location) = match user_location.as_deref().unwrap_or("") {
            "" => (true, DEFAULT_LOCATION),
            s => (false, s),
        };
        // Log the search.
        if let Some(ref mut search_log) = search_log {
            writeln!(
                search_log,
                "git config {}: {}",
                GIT_CONFIG_KEY,
                user_location.as_deref().unwrap_or("<unset>")
            )?;
            if using_default_location {
                writeln!(search_log, "Using default location: {}", DEFAULT_LOCATION)?;
            }
        }

        let parsed_location = if location.starts_with(":") {
            ConfigLocation::Worktree(PathBuf::from_str(&location[1..])?)
        } else {
            // R¨ead config file from the repository.
            let config_toml_output = git_command(&repo_dir)
                .args(["cat-file", "-e", location])
                .output()?;
            match config_toml_output.check_success_with_stderr() {
                Ok(_) => {
                    // The config file blob exists in the repo.
                    ConfigLocation::RepoBlob(location.to_owned())
                }
                Err(e) => {
                    // The config file does not exist in the commit.
                    if using_default_location {
                        // If no config location has been specified, having the file is optional.
                        if let Some(ref mut search_log) = search_log {
                            writeln!(
                                search_log,
                                "'git cat-file -e' reported {}",
                                trim_newline_suffix(&config_toml_output.stderr.to_str_lossy())
                            )?;
                            writeln!(search_log, "Falling back to default configuration")?;
                        }
                        ConfigLocation::None
                    } else {
                        bail!(
                            "Config file {} does not exist in the commit: {}",
                            location,
                            e
                        )
                    }
                }
            }
        };
        Ok(parsed_location)
    }

    /// Loads the TOML configuration string without parsing it.
    pub fn load_config_toml(repo_dir: &Path, location: &ConfigLocation) -> Result<String> {
        || -> Result<String> {
            match location {
                ConfigLocation::RepoBlob(object) => Ok(git_command(&repo_dir)
                    .args(["show", object])
                    .output()?
                    .check_success_with_stderr()?
                    .stdout
                    .to_str()?
                    .to_owned()),
                ConfigLocation::Worktree(path) => {
                    std::fs::read_to_string(repo_dir.join(path)).context("Reading config file")
                }
                ConfigLocation::None => Ok(String::new()),
            }
        }()
        .with_context(|| format!("Loading {}", location))
    }

    /// Parse a TOML configuration string.
    pub fn parse_config_toml_string(config_toml: &str) -> Result<Self> {
        let mut res: Self = toml::from_str(&config_toml).context("Could not parse TOML string")?;
        res.validate()?;
        Ok(res)
    }

    pub fn load_config_from_repo_with_log(
        repo_dir: &Path,
        search_log: Option<&mut String>,
    ) -> Result<Self> {
        let location = Self::find_configuration_location(repo_dir, search_log)?;
        let config_toml = Self::load_config_toml(repo_dir, &location)?;
        Self::parse_config_toml_string(&config_toml)
            .with_context(|| format!("Parsing {}", &location))
    }

    pub fn load_config_from_repo(repo_dir: &Path) -> Result<Self> {
        Self::load_config_from_repo_with_log(repo_dir, None)
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
    pub missing: Vec<CommitHash>,
    pub missing_while_filtering: Vec<CommitHash>,
}

impl Default for CommitFilterConfig {
    fn default() -> Self {
        CommitFilterConfig {
            missing: Vec::new(),
            missing_while_filtering: Vec::new(),
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

        let err = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap_err();
        print!("{:?}", err);
        assert_eq!(
            format!("{:?}", err),
            "Loading worktree file: foobar.toml

Caused by:
    0: Reading config file
    1: No such file or directory (os error 2)"
        );
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
        let mut config = GitTopRepoConfig::parse_config_toml_string("").unwrap();

        config.get_subrepo_config("ssh://bar/baz.git");

        assert!(config.repo.contains_key("baz"));
        assert_eq!(config.repos.filter.len(), 1);
        assert_eq!(config.repos.filter[0].to_string(), "+.*");
    }

    #[test]
    fn test_get_repo_without_new_entry() {
        let mut config = GitTopRepoConfig::parse_config_toml_string(
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
        let err = GitTopRepoConfig::parse_config_toml_string(
            r#"[repo.foo]
        urls = ["ssh://bar/baz.git"]

        [repo.bar]
        urls = ["ssh://bar/baz.git"]

        [repos]"#,
        )
        .unwrap_err();
        assert_eq!(
            format!("{:?}", err),
            "URLs must be unique across all repos, found ssh://bar/baz.git in bar and foo"
        );
    }
}
