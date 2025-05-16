use crate::git::git_command;
use crate::git::git_config_get;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::CommandExtension as _;
use crate::util::is_default;
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
use std::collections::HashSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
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
    /// List of subrepos that are missing in the configuration and have
    /// automatically been added to `suprepos`.
    #[serde(skip)]
    pub missing_subrepos: HashSet<SubRepoName>,
    pub log: LogConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LogConfig {
    /// Warning messages that should be ignored and not displayed for the user.
    #[serde(default)]
    pub ignore_warnings: Vec<String>,
    /// Error messages that were displayed to the user.
    #[serde(skip_deserializing)]
    pub reported_errors: Vec<String>,
    /// Warning messages that were displayed to the user.
    #[serde(skip_deserializing)]
    pub reported_warnings: Vec<String>,
}

pub enum ConfigLocation {
    /// Load a blob from the repo.
    RepoBlob { gitref: String, path: PathBuf },
    /// Load from the path relative to the repository root.
    Worktree { path: PathBuf },
}

impl ConfigLocation {
    /// Check if the config file exists in the repository.
    pub fn validate_existence(&self, repo_dir: &Path) -> Result<()> {
        match self {
            ConfigLocation::RepoBlob { gitref, path } => {
                let location = format!("{gitref}:{}", path.display());
                // Check for existence.
                git_command(repo_dir)
                    .args(["cat-file", "-e", &location])
                    .safe_output()?
                    .check_success_with_stderr()
                    .with_context(|| {
                        format!("Config file {} does not exist in {gitref}", path.display())
                    })?;
            }
            ConfigLocation::Worktree { path } => {
                // Check if the file exists in the worktree.
                if !repo_dir.join(path).exists() {
                    bail!("Config file {path:?} does not exist in the worktree")
                }
            }
        };
        Ok(())
    }
}

impl Display for ConfigLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLocation::RepoBlob { gitref, path } => {
                write!(f, "repo:{gitref}:{}", path.display())
            }
            ConfigLocation::Worktree { path } => write!(f, "local:{}", path.display()),
        }
    }
}

impl FromStr for ConfigLocation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let ret = if let Some(residual) = s.strip_prefix("repo:") {
            let Some((gitref, path)) = residual.split_once(':') else {
                bail!(
                    "Invalid repo config location {s:?}, expected 'repo:<ref>:<path>', e.g. 'repo:refs/remotes/origin/HEAD:.gittoprepo.toml'"
                )
            };
            ConfigLocation::RepoBlob {
                gitref: gitref.to_owned(),
                path: PathBuf::from(path),
            }
        } else if let Some(path) = s.strip_prefix("local:") {
            ConfigLocation::Worktree {
                path: PathBuf::from(path),
            }
        } else {
            bail!("Invalid config location {s:?}, expected '(ref|local):...'");
        };
        Ok(ret)
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
            1 => {
                let repo_name = matches[0].0;
                if self.missing_subrepos.contains(repo_name) {
                    Ok(None)
                } else {
                    Ok(Some(repo_name.clone()))
                }
            }
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
        let Some(repo_name) = self.get_name_from_url(repo_url)? else {
            let mut repo_name = self.default_name_from_url(repo_url).with_context(|| {
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
            for existing_name in self.subrepos.keys() {
                if repo_name.to_lowercase() == existing_name.to_lowercase() {
                    repo_name = existing_name.clone();
                }
            }
            self.subrepos
                .entry(repo_name.clone())
                .or_default()
                .urls
                .push(repo_url.clone());
            self.missing_subrepos.insert(repo_name.clone());
            bail!("URL {repo_url} is missing in the git-toprepo configuration");
        };
        let subrepo_config = self
            .subrepos
            .get_mut(&repo_name)
            .expect("valid subrepo name");
        Ok((repo_name, subrepo_config))
    }

    /// Finds the location of the configuration to load.
    ///
    /// The location of the configuration file is set in the git-config of the
    /// super repository using
    ///    `git config --local toprepo.config <ref>:<git-repo-relative-path>`
    /// . This is initialized with `git-toprepo init` to
    /// `ref:refs/remotes/origin/HEAD:.gittoprepo.toml`, which is managed for
    /// the entire project by the maintainers.
    /// A developer can choose their own config file with a `local:` reference
    /// to a file on disk.
    ///    `local:.gittoprepo.user.toml`,
    ///
    /// Overriding the location is not recommended.
    ///
    /// Returns the configuration and the location it was loaded from.
    pub fn find_configuration_location(repo_dir: &Path) -> Result<ConfigLocation> {
        // Load config file location.
        const GIT_CONFIG_KEY: &str = "toprepo.config";

        let location = git_config_get(repo_dir, GIT_CONFIG_KEY)?.with_context(|| {
            format!("git-config '{GIT_CONFIG_KEY}' is missing. Is this an initialized git-toprepo?")
        })?;

        ConfigLocation::from_str(&location)
    }

    /// Loads the TOML configuration string without parsing it.
    pub fn load_config_toml(repo_dir: &Path, location: &ConfigLocation) -> Result<String> {
        || -> Result<String> {
            match location {
                ConfigLocation::RepoBlob { gitref, path } => Ok(git_command(repo_dir)
                    .args(["show", &format!("{gitref}:{}", path.display())])
                    .check_success_with_stderr()?
                    .stdout
                    .to_str()?
                    .to_owned()),
                ConfigLocation::Worktree { path } => {
                    std::fs::read_to_string(repo_dir.join(path)).context("Reading config file")
                }
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

    pub fn load_config_from_repo(repo_dir: &Path) -> Result<Self> {
        let location = Self::find_configuration_location(repo_dir)?;
        let config_toml = Self::load_config_toml(repo_dir, &location)?;
        Self::parse_config_toml_string(&config_toml)
            .with_context(|| format!("Parsing {}", &location))
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
#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SubrepoConfig {
    #[serde_as(as = "Vec<crate::util::SerdeGixUrl>")]
    pub urls: Vec<gix::Url>,
    #[serde(skip_serializing_if = "is_default")]
    pub fetch: FetchConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub push: PushConfig,
    /// If `false`, the subrepo is not enabled. This is useful to avoid fetching
    /// old repository. Default is `true`.
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
        if self.fetch.url.is_some() {
            // Ok, explicit fetch URL.
        } else if self.urls.len() == 1 {
            // Ok, only one URL.
        } else if !self.enabled && self.urls.len() > 1 {
            // Doesn't matter if self.fetch.url is not specified when disabled.
        } else {
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
        let tmp_path = tmp_dir.path();
        let env = commit_env();

        git_command(tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["config", "toprepo.config", "local:foobar.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let err: anyhow::Error = GitTopRepoConfig::load_config_from_repo(tmp_path).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Loading local:foobar.toml: Reading config file: No such file or directory (os error 2)"
        );
    }

    #[test]
    fn test_create_config_from_worktree() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path();
        let env = commit_env();

        git_command(tmp_path)
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

        git_command(tmp_path)
            .args(["add", "foobar.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["config", "toprepo.config", "local:foobar.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path).unwrap();

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
    fn test_missing_config() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path();
        let env = commit_env();

        git_command(tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["commit", "--allow-empty", "-m", "Initial commit"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["update-ref", "refs/namespaces/top/HEAD", "HEAD"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Try a path in the repository.
        git_command(tmp_path)
            .args(["config", "toprepo.config", "repo:HEAD:.gittoprepo.toml"])
            .check_success_with_stderr()
            .unwrap();

        assert_eq!(
            format!(
                "{:#}",
                GitTopRepoConfig::load_config_from_repo(tmp_path).unwrap_err()
            ),
            "Loading repo:HEAD:.gittoprepo.toml: exit status: 128:\n\
            fatal: path '.gittoprepo.toml' does not exist in 'HEAD'\n"
        );

        // Try the worktree.
        git_command(tmp_path)
            .args(["config", "toprepo.config", "local:nonexisting.toml"])
            .check_success_with_stderr()
            .unwrap();
        let err = GitTopRepoConfig::load_config_from_repo(tmp_path).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Loading local:nonexisting.toml: Reading config file: No such file or directory (os error 2)"
        );
    }

    #[test]
    fn test_create_config_from_head() {
        use std::io::Write;

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path();
        let env = commit_env();

        git_command(tmp_path)
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

        git_command(tmp_path)
            .args(["add", ".gittoprepo.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["commit", "-m", "Initial commit"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["update-ref", "refs/namespaces/top/HEAD", "HEAD"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["rm", ".gittoprepo.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args(["commit", "-m", "Remove .gittoprepo.toml"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(tmp_path)
            .args([
                "config",
                "toprepo.config",
                "repo:refs/namespaces/top/HEAD:.gittoprepo.toml",
            ])
            .check_success_with_stderr()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(tmp_path).unwrap();

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
        assert!(
            config
                .get_or_insert_from_url(&gix::Url::from_bytes(b"ssh://bar/baz.git".as_bstr())?)
                .is_err()
        );
        assert!(
            config
                .subrepos
                .contains_key(&SubRepoName::new("baz".to_owned()))
        );
        // Second time, it should still report an error.
        assert!(
            config
                .get_or_insert_from_url(&gix::Url::from_bytes(b"ssh://bar/baz.git".as_bstr())?)
                .is_err()
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
