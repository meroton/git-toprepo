use crate::git::git_command;
use crate::git::git_config_get;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::CommandExtension as _;
use crate::util::is_default;
use crate::util::trim_newline_suffix;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use serde_with::serde_as;
use sha2::Digest as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct GitTopRepoConfig {
    #[serde(skip)]
    pub checksum: String,
    #[serde(rename = "repo")]
    pub subrepos: BTreeMap<SubRepoName, SubrepoConfig>,
    pub log: LogConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LogConfig {
    /// Warning messages that should be ignored and not displayed for the user.
    #[serde(default)]
    pub ignored_warnings: Vec<String>,
    /// Warning messages that were displayed to the user.
    #[serde(skip_deserializing)]
    pub reported_warnings: Vec<String>,
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
            ConfigLocation::RepoBlob(blob) => write!(f, "blob: {blob}"),
            ConfigLocation::Worktree(path) => write!(f, "worktree file: {}", path.display()),
            ConfigLocation::None => write!(f, "<default-empty>"),
        }
    }
}

impl GitTopRepoConfig {
    /// Gets a `SubrepoConfig` based on a URL using exact matching. If an URL is
    /// missing, the user should add it to the `SubrepoConfig::urls` list.
    pub fn get_name_from_url(&self, url: &gix::Url) -> Result<Option<SubRepoName>> {
        let matches: Vec<_> = self
            .subrepos
            .iter()
            .filter(|(_name, subrepo_config)| subrepo_config.urls.iter().any(|u| u == url))
            .collect();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(Some(matches[0].0.clone())),
            _ => {
                let names = matches.into_iter().map(|(name, _)| name).join(", ");
                bail!("Multiple remote candidates for {url}: {names}");
            }
        }
    }

    pub fn default_name_from_url(&self, repo_url: &gix::Url) -> Option<SubRepoName> {
        // TODO: UTF-8 validation.
        let mut name: &str = &repo_url.path.to_str_lossy();
        if name.ends_with(".git") {
            name = &name[..name.len() - 4];
        } else if name.ends_with("/") {
            name = &name[..name.len() - 1];
        }
        loop {
            if name.starts_with("../") {
                name = &name[3..];
            } else if name.starts_with("./") {
                name = &name[2..];
            } else if name.starts_with("/") {
                name = &name[1..];
            } else {
                break;
            }
        }
        let name = name.replace("/", "_");
        match RepoName::new(name) {
            RepoName::Top => None,
            RepoName::SubRepo(name) => Some(name),
        }
    }

    /// Get a subrepo configuration without creating a new entry if missing.
    pub fn get_from_url(
        &self,
        repo_url: &gix::Url,
    ) -> Result<Option<(SubRepoName, &SubrepoConfig)>> {
        match self.get_name_from_url(repo_url)? {
            Some(repo_name) => {
                let subrepo_config = self.subrepos.get(&repo_name).expect("valid subrepo name");
                Ok(Some((repo_name, subrepo_config)))
            }
            None => Ok(None),
        }
    }

    /// Get a subrepo configuration or create a new entry if missing.
    pub fn get_or_insert_from_url(
        &mut self,
        repo_url: &gix::Url,
    ) -> Result<(SubRepoName, &SubrepoConfig)> {
        let (repo_name, subrepo_config) = match self.get_name_from_url(repo_url)? {
            Some(name) => {
                let subrepo_config = self.subrepos.get_mut(&name).expect("valid subrepo name");
                (name, subrepo_config)
            }
            None => {
                let repo_name = self.default_name_from_url(repo_url).with_context(|| {
                    format!(
                        "URL {repo_url} cannot be automatically converted to a valid repo name. \
                        Please create a manual config entry with the URL."
                    )
                })?;
                // Instead of just self.subrepos.get(&repo_name), also check for
                // case insensitive repo name uniqueness. It's confusing for the
                // user to get multiple repos with the same name and not
                // realising that it's just the casing that is different.
                // Manually adding multiple entries with different casing is
                // allowed but not recommended.
                for (existing_name, subrepo_config) in &self.subrepos {
                    if repo_name.to_lowercase() == existing_name.to_lowercase() {
                        // TODO: Improve error message with how to write such a config.
                        let existing_url = subrepo_config.resolve_fetch_url();
                        bail!(
                            "URL {repo_url} would duplicate repo name {existing_name:?} with URL {existing_url}. \
                            Please create a manual config entry with both URLs."
                        );
                    }
                }
                let subrepo_config =
                    self.subrepos
                        .entry(repo_name.clone())
                        .or_insert(SubrepoConfig {
                            urls: vec![repo_url.clone()],
                            ..Default::default()
                        });
                (repo_name, subrepo_config)
            }
        };
        Ok((repo_name, subrepo_config))
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
        const DEFAULT_LOCATION: &str = "refs/namespaces/top/HEAD:.gittoprepo.toml";

        #[cfg(test)]
        assert_eq!(
            DEFAULT_LOCATION,
            format!(
                "{}HEAD:.gittoprepo.toml",
                crate::repo_name::RepoName::Top.to_ref_prefix()
            )
        );

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
                writeln!(search_log, "Using default location: {DEFAULT_LOCATION}")?;
            }
        }

        let parsed_location = if let Some(path) = location.strip_prefix(':') {
            ConfigLocation::Worktree(PathBuf::from_str(path)?)
        } else {
            // Read config file from the repository.
            let config_toml_output = git_command(repo_dir)
                .args(["cat-file", "-e", location])
                .safe_output()?;
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
                ConfigLocation::RepoBlob(object) => Ok(git_command(repo_dir)
                    .args(["show", object])
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
        .with_context(|| format!("Loading {location}"))
    }

    /// Parse a TOML configuration string.
    pub fn parse_config_toml_string(config_toml: &str) -> Result<Self> {
        let mut config: Self =
            toml::from_str(config_toml).context("Could not parse TOML string")?;
        let checksum = sha2::Sha256::digest(config_toml.as_bytes());
        config.checksum = hex::encode(checksum);
        config.validate()?;
        Ok(config)
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

    pub fn save_config_to_repo(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(
            path.parent()
                .with_context(|| format!("Bad config path {}", path.display()))?,
        )?;
        let config_toml = toml::to_string_pretty(self).context("Serializing config")?;
        std::fs::write(path, config_toml).context("Writing config file")?;
        Ok(())
    }

    /// Validates that the configuration is sane.
    pub fn validate(&mut self) -> Result<()> {
        for (repo_name, subrepo_config) in self.subrepos.iter_mut() {
            // Validate each subrepo config.
            subrepo_config
                .validate()
                .with_context(|| format!("Invalid subrepo configuration for {repo_name}"))?;
        }
        self.ensure_unique_urls()?;
        Ok(())
    }

    fn ensure_unique_urls(&self) -> Result<()> {
        let mut found = HashMap::<String, SubRepoName>::new();
        for (repo_name, v) in self.subrepos.iter() {
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

    pub fn is_enabled(&self, repo_name: &RepoName) -> bool {
        match repo_name {
            RepoName::Top => true,
            RepoName::SubRepo(sub_repo_name) => self
                .subrepos
                .get(sub_repo_name)
                .is_none_or(|repo_config| repo_config.enabled),
        }
    }
}

/// `ToprepoConfig`` holds the configuration for the toprepo itself. The content is
/// taken from the default git remote configuration.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToprepoConfig {
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: gix::Url,
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub push_url: gix::Url,
}

/// `SubrepoConfig`` holds the configuration for a subrepo in the super repo. If
/// `fetch.url` is empty, the first entry in `urls` is used. If `push.url` is
/// empty, the value of `fetch.url` is used.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SubrepoConfig {
    #[serde_as(as = "Vec<crate::util::SerdeGixUrl>")]
    pub urls: Vec<gix::Url>,
    #[serde(skip_serializing_if = "is_default")]
    pub fetch: FetchConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub push: PushConfig,
    #[serde(default = "return_true")]
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn return_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl SubrepoConfig {
    /// Validates that the configuration is sane.
    /// This will check that a fetch URL is set
    /// if `urls` does not contain exacly one entry.
    pub fn validate(&self) -> Result<()> {
        // Set fetch/push urls.
        if self.fetch.url.is_none() && self.urls.len() != 1 {
            bail!("Either .fetch.url needs to be set or .urls must have exactly one element")
        }
        Ok(())
    }

    pub fn resolve_fetch_url(&self) -> &gix::Url {
        match &self.fetch.url {
            Some(url) => url,
            None => {
                assert!(self.validate().is_ok());
                &self.urls[0]
            }
        }
    }

    pub fn resolve_push_url(&self) -> &gix::Url {
        match &self.push.url {
            Some(url) => url,
            None => self.resolve_fetch_url(),
        }
    }

    pub fn get_fetch_config_with_url(&self) -> FetchConfig {
        let mut fetch = self.fetch.clone();
        if fetch.url.is_none() {
            fetch.url = Some(self.resolve_fetch_url().to_owned());
        }
        fetch
    }

    pub fn get_push_config_with_url(&self) -> PushConfig {
        let mut push = self.push.clone();
        if push.url.is_none() {
            push.url = Some(self.resolve_push_url().to_owned());
        }
        push
    }
}

impl Default for SubrepoConfig {
    fn default() -> Self {
        SubrepoConfig {
            urls: Vec::new(),
            fetch: Default::default(),
            push: Default::default(),
            enabled: true,
        }
    }
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct FetchConfig {
    #[serde(default = "fetch_prune_default")]
    #[serde(skip_serializing_if = "eq_fetch_prune_default")]
    pub prune: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub depth: i32,
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: Option<gix::Url>,
}

impl Default for FetchConfig {
    fn default() -> Self {
        FetchConfig {
            prune: fetch_prune_default(),
            depth: 0,
            url: None,
        }
    }
}

fn fetch_prune_default() -> bool {
    true
}

fn eq_fetch_prune_default(value: &bool) -> bool {
    *value == fetch_prune_default()
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PushConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: Option<gix::Url>,
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::commit_env;
    use super::*;

    #[test]
    fn test_create_config_from_invalid_ref() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["config", "toprepo.config", ":foobar.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let err: anyhow::Error =
            GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Loading worktree file: foobar.toml\
            : Reading config file\
            : No such file or directory (os error 2)"
        );
    }

    #[test]
    fn test_create_config_from_worktree() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
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
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["config", "toprepo.config", ":foobar.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();

        let foo_name = SubRepoName::new("foo".to_owned());
        assert!(config.subrepos.contains_key(&foo_name));
        assert_eq!(
            config
                .subrepos
                .get(&foo_name)
                .unwrap()
                .resolve_fetch_url()
                .to_bstring(),
            b"ssh://bar/baz.git".as_bstr()
        );
        assert_eq!(
            config
                .subrepos
                .get(&foo_name)
                .unwrap()
                .resolve_push_url()
                .to_bstring(),
            b"ssh://bar/baz.git".as_bstr()
        );
    }

    #[test]
    fn test_create_config_from_empty_string() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path: PathBuf = tmp_dir.path().to_path_buf();
        let env = commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "--allow-empty", "-m", "Initial commit"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["update-ref", "refs/namespaces/top/HEAD", "HEAD"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let mut search_log = String::new();
        let config = GitTopRepoConfig::load_config_from_repo_with_log(
            tmp_path.as_path(),
            Some(&mut search_log),
        )
        .unwrap();
        assert_eq!(
            search_log,
            "git config toprepo.config: <unset>\n\
            Using default location: refs/namespaces/top/HEAD:.gittoprepo.toml\n\
            'git cat-file -e' reported fatal: path '.gittoprepo.toml' does not exist in 'refs/namespaces/top/HEAD'\n\
            Falling back to default configuration\n"
        );

        assert!(config.subrepos.is_empty());
    }

    #[test]
    fn test_create_config_from_head() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().to_path_buf();
        let env = commit_env();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
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
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "-m", "Initial commit"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["update-ref", "refs/namespaces/top/HEAD", "HEAD"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["rm", ".gittoprepo.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&tmp_path)
            .args(["commit", "-m", "Remove .gittoprepo.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path.as_path()).unwrap();

        let foo_name = SubRepoName::new("foo".to_owned());
        assert!(config.subrepos.contains_key(&foo_name));
        assert_eq!(
            config
                .subrepos
                .get(&foo_name)
                .unwrap()
                .resolve_fetch_url()
                .to_bstring(),
            b"ssh://bar/baz.git".as_bstr()
        );
        assert_eq!(
            config
                .subrepos
                .get(&foo_name)
                .unwrap()
                .resolve_push_url()
                .to_bstring(),
            b"ssh://bar/baz.git".as_bstr()
        );
    }

    #[test]
    fn test_get_repo_with_new_entry() -> Result<()> {
        let mut config = GitTopRepoConfig::parse_config_toml_string("")?;

        assert_eq!(config.subrepos.len(), 0);
        config.get_or_insert_from_url(&gix::Url::from_bytes(b"ssh://bar/baz.git".as_bstr())?)?;
        assert!(
            config
                .subrepos
                .contains_key(&SubRepoName::new("baz".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn test_get_repo_without_new_entry() -> Result<()> {
        let mut config = GitTopRepoConfig::parse_config_toml_string(
            r#"[repo.foo]
        urls = ["../bar/repo.git"]

        [repos]"#,
        )?;

        assert!(
            config
                .subrepos
                .contains_key(&SubRepoName::new("foo".to_owned()))
        );
        assert!(
            config
                .get_or_insert_from_url(&gix::Url::from_bytes(
                    b"https://example.com/foo.git".as_bstr()
                )?)
                .is_err()
        );
        assert_eq!(config.subrepos.len(), 1);
        Ok(())
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
            format!("{err:#}"),
            "URLs must be unique across all repos, found ssh://bar/baz.git in bar and foo"
        );
    }
}
