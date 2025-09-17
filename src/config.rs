use crate::git::CommitId;
use crate::git::git_command;
use crate::git::git_config_get;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::CommandSpanExt as _;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::CommandExtension as _;
use crate::util::OrderedHashSet;
use crate::util::find_current_worktree;
use crate::util::find_main_worktree;
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

// NB: We can't seem to curry this into a const function
// https://docs.rs/const-str/latest/const_str/index.html#const-context-only
// So we can either use a single variable for the full string
// and type in the namespace (no compile-time concatenation),
// or use a runtime function that just takes the key,
// which should too be a constant.
// So we can use either of the two use patterns:
//    * `TOPREPO_CONFIG_FILE_KEY`
//      benefits: tersest use sites
//    * `toprepo_config(CONFIG_FILE_KEY)`
//      benefits: does not repeat the namespace in multiple constant definitions.
pub const TOPREPO_CONFIG_NAMESPACE: &str = "toprepo";
pub fn toprepo_git_config(key: &str) -> String {
    format!("{TOPREPO_CONFIG_NAMESPACE}.{key}")
}
pub const TOPREPO_CONFIG_FILE_KEY: &str = "config";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
/// Static configuration for the tool itself.
/// Read from the configuration file.
pub struct GitTopRepoConfig {
    #[serde(skip)]
    pub checksum: String,
    #[serde(skip_serializing_if = "is_default")]
    pub fetch: GlobalFetchConfig,
    #[serde(rename = "repo")]
    pub subrepos: BTreeMap<SubRepoName, SubRepoConfig>,
}

pub enum ConfigLocation {
    /// Load a blob from the repo.
    RepoBlob { gitref: String, path: PathBuf },
    /// Load from the path relative to the main worktree root.
    // (The primary repository checkout).
    Local { path: PathBuf },
    /// Load from the path relative to the current worktree root.
    // A worktree links to the main worktree (repository) ,
    // but can be located anywhere on the filesystem.
    // https://git-scm.com/docs/git-worktree
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
                    .trace_command(crate::command_span!("git cat-file"))
                    .safe_output()?
                    .check_success_with_stderr()
                    .with_context(|| {
                        format!("Config file {} does not exist in {gitref}", path.display())
                    })?;
            }
            ConfigLocation::Local { path } => {
                // Check if the file exists in the main worktree.
                let main_worktree = find_main_worktree(repo_dir)?;
                if !main_worktree.join(path).exists() {
                    bail!("Config file {path:?} does not exist in the worktree")
                }
            }
            ConfigLocation::Worktree { path } => {
                // Check if the file exists in the current worktree.
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
            ConfigLocation::Local { path } => write!(f, "local:{}", path.display()),
            ConfigLocation::Worktree { path } => write!(f, "worktree:{}", path.display()),
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
            ConfigLocation::Local {
                path: PathBuf::from(path),
            }
        } else if let Some(path) = s.strip_prefix("worktree:") {
            ConfigLocation::Worktree {
                path: PathBuf::from(path),
            }
        } else {
            bail!("Invalid config location {s:?}, expected '(ref|local|worktree):...'");
        };
        Ok(ret)
    }
}

impl GitTopRepoConfig {
    /// Finds the location of the configuration to load.
    ///
    /// The location of the configuration file is set in the git-config of the
    /// super repository using
    ///    `git config --local toprepo.config <ref>:<git-repo-relative-path>`
    /// . This is initialized with `git-toprepo init` to
    /// `ref:refs/remotes/origin/HEAD:.gittoprepo.toml`, which is managed for
    /// the entire project by the maintainers.
    ///
    /// A developer can choose their own config file with a `worktree:` reference
    /// to a file relative to the current worktree.
    ///    `worktree:.gittoprepo.user.toml`,
    ///
    /// Overriding the location is not recommended.
    ///
    /// Returns the configuration and the location it was loaded from.
    pub fn find_configuration_location(repo_dir: &Path) -> Result<ConfigLocation> {
        // Load config file location.

        let key = &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY);
        let location = git_config_get(repo_dir, key)?.with_context(|| {
            format!("git-config '{key}' is missing. Is this an initialized git-toprepo?")
        })?;

        ConfigLocation::from_str(&location)
    }

    /// Loads the TOML configuration string without parsing it.
    pub fn load_config_toml(repo_dir: &Path, location: &ConfigLocation) -> Result<String> {
        || -> Result<String> {
            match location {
                ConfigLocation::RepoBlob { gitref, path } => Ok(git_command(repo_dir)
                    .args(["show", &format!("{gitref}:{}", path.display())])
                    .trace_command(crate::command_span!("git show"))
                    .check_success_with_stderr()?
                    .stdout
                    .to_str()?
                    .to_owned()),
                ConfigLocation::Local { path } => {
                    let main_worktree = find_main_worktree(repo_dir)?;
                    std::fs::read_to_string(main_worktree.join(path)).context("Reading config file")
                }
                ConfigLocation::Worktree { path } => {
                    let current_worktree = find_current_worktree(repo_dir)?;
                    std::fs::read_to_string(current_worktree.join(path))
                        .context("Reading config file")
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

    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_impl(path)
            .with_context(|| format!("Saving config to {}", path.display()))
    }

    fn save_impl(&self, path: &Path) -> Result<()> {
        if let Some(parent_dir) = path.parent() {
            std::fs::create_dir_all(parent_dir).context("Failed to create parent directory")?;
        }
        let config_toml = toml::to_string_pretty(self).context("Serializing config")?;
        std::fs::write(path, config_toml).context("Writing config file")?;
        Ok(())
    }

    /// Validates that the configuration is sane.
    pub fn validate(&self) -> Result<()> {
        for (repo_name, subrepo_config) in self.subrepos.iter() {
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

#[derive(Debug, PartialEq, Eq)]
pub enum GetOrInsertOk<'a> {
    /// The subrepo was found in the configuration.
    Found((SubRepoName, &'a mut SubRepoConfig)),
    /// The subrepo was not found in the configuration.
    Missing(SubRepoName),
    /// The subrepo was not found in the configuration, but `Missing` was reported previously.
    MissingAgain(SubRepoName),
}

/// `TopRepoConfig` holds the configuration for the toprepo itself. The content is
/// taken from the default git remote configuration.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TopRepoConfig {
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: gix::Url,
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub push_url: gix::Url,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalFetchConfig {
    /// Timeouts for git-fetch when there is no update on stderr. Each number
    /// timeout value represents one additional attempt. Zero means infinite, so
    /// no timeout.
    ///
    /// This is useful for patchy network connections where git-fetch sometimes
    /// hangs.
    ///
    /// Consider the example `[10, 30, 0]`. This first execution of git-fetch is
    /// killed after 10 seconds of no progress output to stderr. The second
    /// attempt is killed after 30 seconds. The third attempt has no timeout, so
    /// either git-fetch will fail after some time or the user has to abort
    /// git-toprepo with e.g. Ctrl+C.
    ///
    /// Default: `[]` means `[0]` (no timeout).
    #[serde(skip_serializing_if = "is_default")]
    pub idle_timeouts_secs: Vec<u32>,
}

impl GlobalFetchConfig {
    /// Returns the idle timeouts as `Duration`s with at least one element.
    /// `None` means no timeout.
    pub fn get_idle_timeouts(&self) -> Vec<Option<std::time::Duration>> {
        if self.idle_timeouts_secs.is_empty() {
            vec![None]
        } else {
            self.idle_timeouts_secs
                .iter()
                .map(|secs| {
                    if *secs == 0 {
                        None
                    } else {
                        Some(std::time::Duration::from_secs(*secs as u64))
                    }
                })
                .collect()
        }
    }
}

/// `SubRepoConfig` holds the configuration for a subrepo in the super repo. If
/// `fetch.url` is empty, the first entry in `urls` is used. If `push.url` is
/// empty, the value of `fetch.url` is used.
#[serde_as]
#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct SubRepoConfig {
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
    /// Commits that should not be expanded but rather kept as submodules. These
    /// don't need to be fetched from the remote.
    #[serde_as(
        serialize_as = "serde_with::IfIsHumanReadable<OrderedHashSet<serde_with::DisplayFromStr>>"
    )]
    pub skip_expanding: HashSet<CommitId>,
}

fn return_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl SubRepoConfig {
    /// Validates that the configuration is sane.
    /// This will check that a fetch URL is set
    /// if `urls` does not contain exactly one entry.
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct PushConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: Option<gix::Url>,
}

#[cfg(test)]
mod tests {
    use super::super::git::commit_env_for_testing;
    use super::*;
    use assert_cmd::assert::OutputAssertExt as _;

    const BAR_BAZ: &str = r#"
        [repo]
        [repo.foo.fetch]
        url = "ssh://bar/baz.git"
    "#;

    const BAR_BAZ_FETCH_URL: &str = "ssh://bar/baz.git";
    const BAR_BAZ_FETCH: &str = r#"url = "ssh://bar/baz.git""#;

    #[test]
    fn test_create_config_from_invalid_ref() {
        let tmp_path = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_create_config_from_invalid_ref",
        );
        let env = commit_env_for_testing();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .assert()
            .success();

        git_command(&tmp_path)
            .args([
                "config",
                &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
                "worktree:foobar.toml",
            ])
            .envs(&env)
            .assert()
            .success();

        let err: anyhow::Error = GitTopRepoConfig::load_config_from_repo(&tmp_path).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Loading worktree:foobar.toml: Reading config file: No such file or directory (os error 2)"
        );
    }

    #[test]
    fn test_create_config_from_worktree() {
        use std::io::Write;

        let tmp_path = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_create_config_from_worktree",
        );
        let env = commit_env_for_testing();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .assert()
            .success();

        let mut tmp_file = std::fs::File::create(tmp_path.join("foobar.toml")).unwrap();

        writeln!(tmp_file, "{BAR_BAZ}").unwrap();

        git_command(&tmp_path)
            .args(["add", "foobar.toml"])
            .envs(&env)
            .assert()
            .success();

        git_command(&tmp_path)
            .args([
                "config",
                &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
                "worktree:foobar.toml",
            ])
            .envs(&env)
            .assert()
            .success();

        let config = GitTopRepoConfig::load_config_from_repo(&tmp_path).unwrap();

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
        let tmp_path = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_missing_config",
        );
        let env = commit_env_for_testing();

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

        // Try a path in the repository.
        git_command(&tmp_path)
            .args([
                "config",
                &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
                "repo:HEAD:.gittoprepo.toml",
            ])
            .check_success_with_stderr()
            .unwrap();

        assert_eq!(
            format!(
                "{:#}",
                GitTopRepoConfig::load_config_from_repo(&tmp_path).unwrap_err()
            ),
            "Loading repo:HEAD:.gittoprepo.toml: exit status: 128:\n\
            fatal: path '.gittoprepo.toml' does not exist in 'HEAD'\n"
        );

        // Try the worktree.
        git_command(&tmp_path)
            .args([
                "config",
                &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
                "worktree:nonexisting.toml",
            ])
            .check_success_with_stderr()
            .unwrap();
        let err = GitTopRepoConfig::load_config_from_repo(&tmp_path).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Loading worktree:nonexisting.toml: Reading config file: No such file or directory (os error 2)"
        );
    }

    #[test]
    fn test_parse_fetch_url() {
        let table = BAR_BAZ_FETCH.parse::<toml::Table>();
        assert!(table.is_ok(), "{table:?}");
        let table = table.unwrap();

        let fetch: Result<FetchConfig, _> = serde_path_to_error::deserialize(table);
        assert!(fetch.is_ok(), "{fetch:?}");
        let fetch = fetch.unwrap();
        assert!(fetch.url.is_some(), "{fetch:?}");
        assert_eq!(
            fetch.url.unwrap(),
            gix::Url::from_bytes(BAR_BAZ_FETCH_URL.into()).unwrap()
        );
    }

    #[test]
    fn test_parse_config() {
        let table = BAR_BAZ.parse::<toml::Table>();
        assert!(table.is_ok(), "{table:?}");
        let table = table.unwrap();

        let res: Result<GitTopRepoConfig, _> = serde_path_to_error::deserialize(table);
        assert!(res.is_ok(), "{res:?}");
    }

    #[test]
    fn test_create_config_from_head() {
        // TODO: Move to integration tests.
        use std::io::Write;

        let tmp_path = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_create_config_from_head",
        );
        let env = commit_env_for_testing();

        git_command(&tmp_path)
            .args(["init"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        let mut tmp_file = std::fs::File::create(tmp_path.join(".gittoprepo.toml")).unwrap();
        writeln!(tmp_file, "{BAR_BAZ}").unwrap();

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
            .args([
                "update-ref",
                "refs/namespaces/top/refs/remotes/origin/HEAD",
                "HEAD",
            ])
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

        git_command(&tmp_path)
            .args([
                "config",
                &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY),
                "repo:refs/namespaces/top/refs/remotes/origin/HEAD:.gittoprepo.toml",
            ])
            .check_success_with_stderr()
            .unwrap();

        let config = GitTopRepoConfig::load_config_from_repo(&tmp_path).unwrap();

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

    // #[test]
    // fn test_get_repo_with_new_entry() -> Result<()> {
    //     let mut config = GitTopRepoConfig::parse_config_toml_string("")?;
    //
    //     assert_eq!(config.subrepos.len(), 0);
    //     assert_eq!(
    //         config
    //             .get_or_insert_from_url(&gix::Url::from_bytes(b"ssh://bar/baz.git".as_bstr())?)
    //             .unwrap(),
    //         GetOrInsertOk::Missing(SubRepoName::new("baz".to_owned()))
    //     );
    //     assert!(
    //         config
    //             .subrepos
    //             .contains_key(&SubRepoName::new("baz".to_owned()))
    //     );
    //     // Second time, it should still report an error.
    //     assert_eq!(
    //         config
    //             .get_or_insert_from_url(&gix::Url::from_bytes(b"ssh://bar/baz.git".as_bstr())?)
    //             .unwrap(),
    //         GetOrInsertOk::MissingAgain(SubRepoName::new("baz".to_owned()))
    //     );
    //     Ok(())
    // }

    // #[test]
    // fn test_get_repo_without_new_entry() -> Result<()> {
    //     let config = GitTopRepoConfig::parse_config_toml_string(
    //         r#"
    //             [repo.foo]
    //             urls = ["../bar/repo.git"]
    //         "#,
    //     );
    //     assert!(config.is_ok(), "{config:?}");
    //     let mut config = config.unwrap();
    //
    //     assert!(
    //         config
    //             .subrepos
    //             .contains_key(&SubRepoName::new("foo".to_owned()))
    //     );
    //     assert_eq!(
    //         config
    //             .get_or_insert_from_url(&gix::Url::from_bytes(
    //                 b"https://example.com/foo.git".as_bstr()
    //             )?)
    //             .unwrap(),
    //         GetOrInsertOk::Missing(SubRepoName::new("foo".to_owned()))
    //     );
    //     assert_eq!(config.subrepos.len(), 1);
    //     Ok(())
    // }

    #[test]
    fn test_config_with_duplicate_urls() {
        let err = GitTopRepoConfig::parse_config_toml_string(
            r#"
                [repo.foo]
                urls = ["ssh://bar/baz.git"]

                [repo.bar]
                urls = ["ssh://bar/baz.git"]
            "#,
        )
        .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "URLs must be unique across all repos, found ssh://bar/baz.git in bar and foo"
        );
    }
}
