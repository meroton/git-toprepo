/** Command line argument definition using subcommands.
 *
 * See also https://jmmv.dev/2013/08/cli-design-putting-flags-to-good-use.html#bad-using-flags-to-select-subcommands.
 */
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bstr::BStr;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::GitPath;
use git_toprepo::git::repo_relative_path;
use git_toprepo::gitmodules::SubmoduleUrlExt as _;
use git_toprepo::repo_name::RepoName;
use git_toprepo::repo_name::SubRepoName;
use git_toprepo::util::UniqueContainer;
use std::ops::Deref as _;
use std::path::Path;
use std::path::PathBuf;

const ABOUT: &str = "git-submodule made easy with git-toprepo.

git-toprepo merges subrepositories into a common history, similar to git-subtree.\
";

#[derive(Parser, Debug)]
#[command(about = ABOUT)]
pub struct Cli {
    /// Run as if started in <path>.
    #[arg(name = "path", short = 'C')]
    pub working_directory: Option<PathBuf>,

    /// Optional "git" word to simplify pasting copied commands, for example:
    /// `git-toprepo git fetch ...`.
    #[arg(name = "git")]
    pub git: Option<GitEnum>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum GitEnum {
    Git,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a repository and the git-config, without fetching from the remote.
    Init(Init),
    /// Initialize a repository and fetch from the remote.
    Clone(Clone),
    Config(Config),
    Refilter(Refilter),
    Fetch(Fetch),
    /// Push commits to the respective remotes of each filtered submodule.
    Push(Push),

    #[command(subcommand)]
    Dump(Dump),

    /// Scaffolding code to start writing `.gitmodule` mapping code.
    /// This replaces the first field of every line on standard in
    /// with the submodule path.
    ///
    /// This can be used in interactive shell pipelines where the Gerrit project
    /// and the revision is known. To download review comments for commits in
    /// submodules, or to checkout out a commit in a submodule
    ///
    /// Note, that checking out submodules this way is only for regular repo
    /// checkouts. For a git-toprepo super repo purposeful checkout must be
    /// implemented.
    Replace(Replace),
    /// Print the version of the git-toprepo tool.
    #[clap(aliases = ["-V", "--version"])]
    Version,
}

#[derive(Args, Debug)]
pub struct Init {
    /// The repository to be configured as remote.
    pub repository: String,

    /// The name of a new directory to create the repository in. If no directory
    /// is given, the basename of the repository is used.
    pub directory: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct Clone {
    #[command(flatten)]
    pub init: Init,

    /// After fetching the top repository, skip fetching the submodules.
    #[clap(long)]
    pub minimal: bool,
}

#[derive(Args, Debug)]
pub struct Config {
    #[command(subcommand)]
    pub config_command: ConfigCommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommands {
    /// Prints the configuration location.
    Location,
    /// Show the configuration of the current repository.
    Show,
    /// Create a new configuration file based on direct submodules only and
    /// print it to stdout.
    Bootstrap,
    /// Reads a configuration and prints it in normalized form.
    Normalize(ConfigNormalize),
    /// Verifies that a given configuration can be loaded.
    Validate(ConfigValidate),
}

#[derive(Args, Debug, Clone)]
pub struct ConfigNormalize {
    /// The configuration file to normalize or - for stdin.
    #[arg(id = "file")]
    pub file: PathBuf,
}

#[derive(Args, Debug, Clone)]
pub struct ConfigValidate {
    /// The configuration file to validate or - for stdin.
    #[arg(id = "file")]
    pub file: PathBuf,
}

/// Dump internal states to stdout.
#[derive(Subcommand, Debug)]
pub enum Dump {
    /// Dump the repository import cache as JSON to stdout.
    ImportCache,
}

#[derive(Args, Debug)]
pub struct Refilter {
    /// Continue as much as possible after an error.
    #[arg(long)]
    pub keep_going: bool,

    /// Number of concurrent threads to load the repository.
    #[arg(long, default_value = "7")]
    pub jobs: std::num::NonZero<u16>,

    /// Skip fetching missing submodule commits.
    #[arg(long)]
    pub no_fetch: bool,
}

#[derive(Args, Debug)]
pub struct Fetch {
    /// Continue as much as possible after an error.
    #[arg(long)]
    pub keep_going: bool,

    /// Number of concurrent threads to perform git-fetch and the filtering.
    #[arg(long, name = "N", default_value = "7")]
    pub jobs: std::num::NonZero<u16>,

    /// Skip the filtering step after fetching the top repository.
    #[arg(long)]
    pub skip_filter: bool,

    /// A configured git-remote in the mono repository, a URL to a remote top
    /// repository, a URL to a remote submodule repository, a working directory
    /// relative path to a submodule in the repository. This argument will be
    /// used to resolve which URL to fetch from and which directory to filter
    /// into, unless `--path` overrides the directory.
    #[arg(name = "remote-ish", verbatim_doc_comment)]
    pub remote: Option<String>,

    /// The worktree path to filter into, relative to the working directory.
    /// This path is used to override the repo to filter into which is otherwise
    /// deduced from the `remote` argument.
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// A reference to fetch from the top repository or submodule. Refspec
    /// wildcards are not supported.
    #[arg(id = "ref", num_args=1.., value_parser = clap::builder::ValueParser::new(parse_refspec), verbatim_doc_comment)]
    pub refspecs: Option<Vec<(String, String)>>,
}

impl Fetch {
    pub fn resolve_remote_and_path(
        &self,
        repo: &gix::Repository,
        config: &GitTopRepoConfig,
    ) -> Result<ResolvedFetchParams> {
        FetchParamsResolver::new(repo, config)?
            .resolve_remote_and_path(self.remote.as_deref(), self.path.as_deref())
    }
}
pub struct ResolvedFetchParams {
    /// The repository to fetch from.
    pub repo: RepoName,
    /// The path to filter into.
    pub path: GitPath,
    /// The URL to fetch from.
    pub url: gix::Url,
}

struct FetchParamsResolver<'a> {
    /// The git repository to fetch from.
    repo: &'a gix::Repository,
    /// The git-toprepo configuration.
    config: &'a GitTopRepoConfig,
    worktree: PathBuf,
    gitmod_infos: GitModulesInfo,
}

impl<'a> FetchParamsResolver<'a> {
    pub fn new(repo: &'a gix::Repository, config: &'a GitTopRepoConfig) -> Result<Self> {
        let worktree = repo
            .workdir()
            .context("Worktree missing in git repository")?;
        let gitmod_infos = GitModulesInfo::parse_dot_gitmodules_in_repo(repo)?;
        Ok(Self {
            repo,
            config,
            worktree: worktree.to_owned(),
            gitmod_infos,
        })
    }
    /// Resolve the remote and path fields. The `--remote` can be either a
    /// git-remote name, a repository-relative path or a URL.
    pub fn resolve_remote_and_path(
        &self,
        remote: Option<&str>,
        override_path: Option<&Path>,
    ) -> Result<ResolvedFetchParams> {
        // Convert from working directory relative `Path` to worktree relative
        // `GitPath`.
        let override_path = match override_path {
            Some(path) => Some(repo_relative_path(&self.worktree, path)?),
            None => None,
        };

        let Some(remote) = remote else {
            if override_path.is_some() {
                bail!("Cannot use --path without specifying a 'remote-ish'");
            }
            return Ok(ResolvedFetchParams {
                repo: RepoName::Top,
                path: GitPath::default(),
                url: self.get_default_top_url()?,
            });
        };
        let remote_bstr = BStr::new(remote);
        if self.repo.remote_names().contains(remote_bstr) {
            return self.resolve_as_remote_name(remote, override_path);
        }
        // If not git-remote name, is it a worktree path?
        if !remote.contains("://")
            && let Some(ret) = self.try_resolve_as_worktree_path(remote, &override_path)?
        {
            return Ok(ret);
        }
        let url = gix::Url::from_bytes(remote_bstr)?;
        if let Some(ret) = self.try_resolve_as_remote_url(&url, &override_path)? {
            return Ok(ret);
        }
        // If not a git-remote URL, then it must be a submodule URL.
        self.resolve_as_submod_url(url, override_path)
    }

    /// Get the default git-remote URL in the mono repository.
    fn get_default_top_url(&self) -> Result<gix::Url> {
        let url = self
            .repo
            .find_default_remote(gix::remote::Direction::Fetch)
            .context("Default git-remote not found")?
            .context("Bad default git-remote")?
            .url(gix::remote::Direction::Fetch)
            .context("Missing fetch URL for the default git-remote")?
            .clone();
        Ok(url)
    }

    fn get_submodule_from_path(&self, submod_path: &GitPath) -> Result<(SubRepoName, gix::Url)> {
        let submod_url = self.get_dot_gitmodules_url(submod_path)?;
        let (name, _config) = self
            .config
            .get_from_url(&submod_url)?
            .with_context(|| format!("Missing git-toprepo configuration for URL {submod_url}"))?;
        Ok((name, submod_url))
    }

    fn get_dot_gitmodules_url(&self, path: &GitPath) -> Result<gix::Url> {
        let submod_url = self
            .gitmod_infos
            .submodules
            .get(path)
            .with_context(|| format!("{path} is not a submodule"))?;
        let submod_url = match submod_url {
            Ok(submod_url) => submod_url.clone(),
            Err(err) => {
                anyhow::bail!(format!("Bad URL for {path} in .gitmodules: {err}"));
            }
        };
        Ok(submod_url)
    }

    fn resolve_as_remote_name(
        &self,
        remote: &str,
        override_path: Option<GitPath>,
    ) -> Result<ResolvedFetchParams> {
        // If a git-remote exists, use it as the top repo URL.
        let top_url = self
            .repo
            .find_fetch_remote(Some(remote.into()))
            .with_context(|| format!("Remote {remote} not found"))?
            .url(gix::remote::Direction::Fetch)
            .with_context(|| format!("Missing fetch URL for remote {remote}"))?
            .clone();
        let override_path = override_path.unwrap_or_default();
        if override_path.is_empty() {
            // --path is unset or empty, then a git-remote name means
            // fetching to the top reporitory.
            return Ok(ResolvedFetchParams {
                repo: RepoName::Top,
                path: GitPath::default(),
                url: top_url,
            });
        }
        // --path is set to a submodule path.
        let (submod_name, submod_url) = self.get_submodule_from_path(&override_path)?;
        let url = top_url.join(&submod_url);
        Ok(ResolvedFetchParams {
            repo: RepoName::from(submod_name),
            path: override_path,
            url,
        })
    }

    fn try_resolve_as_worktree_path(
        &self,
        remote: &str,
        override_path: &Option<GitPath>,
    ) -> Result<Option<ResolvedFetchParams>> {
        if override_path.is_some() {
            anyhow::bail!(
                "Cannot use --path when specifying a worktree relative path (submodule path) as 'remote-ish'"
            );
        }
        if let Ok(repo_rel_path) = repo_relative_path(&self.worktree, Path::new(&remote)) {
            // The path is relative to the worktree.
            let (submod_name, submod_url) = self
                .get_submodule_from_path(&repo_rel_path)
                .with_context(|| format!("Submodule {repo_rel_path} not found in config"))?;
            let full_url = self.get_default_top_url()?.join(&submod_url);
            return Ok(Some(ResolvedFetchParams {
                repo: RepoName::from(submod_name),
                path: repo_rel_path,
                url: full_url,
            }));
        }
        // Not a worktree path, so must be a URL.
        Ok(None)
    }

    fn try_resolve_as_remote_url(
        &self,
        url: &gix::Url,
        override_path: &Option<GitPath>,
    ) -> Result<Option<ResolvedFetchParams>> {
        for remote_name in self.repo.remote_names() {
            let gix_remote = self.repo.find_remote(remote_name.deref())?;
            let Some(fetch_url) = gix_remote.url(gix::remote::Direction::Fetch) else {
                continue;
            };
            if fetch_url.approx_equal(url) {
                // It is one of the git-remotes, i.e. a top repository URL.
                if override_path.is_some() {
                    anyhow::bail!(
                        "Cannot use --path when specifying a git-remote top repo URL as 'remote-ish'"
                    );
                }
                return Ok(Some(ResolvedFetchParams {
                    repo: RepoName::Top,
                    path: GitPath::default(),
                    url: url.clone(),
                }));
            }
        }
        Ok(None)
    }

    fn resolve_as_submod_url(
        &self,
        url: gix::Url,
        override_path: Option<GitPath>,
    ) -> Result<ResolvedFetchParams> {
        if let Some(override_path) = override_path {
            let (submod_name, _submod_url) = self.get_submodule_from_path(&override_path)?;
            return Ok(ResolvedFetchParams {
                repo: RepoName::from(submod_name),
                path: override_path,
                url,
            });
        }
        let base_url = self.get_default_top_url()?;
        let name = self
            .config
            .get_name_from_similar_full_url(url.clone(), &base_url)?;
        let RepoName::SubRepo(submod_name) = &name else {
            unreachable!("Already checked that top URLs are not matching");
        };
        let submod_config = self.config.subrepos.get(submod_name).unwrap();
        let mut matching_submod_path = UniqueContainer::new();
        for (submod_path, submod_url) in &self.gitmod_infos.submodules {
            let Ok(submod_url) = submod_url else { continue };
            if submod_config.urls.iter().any(|url| url == submod_url) {
                matching_submod_path.insert(submod_path);
            }
        }
        let submod_path = match matching_submod_path {
            UniqueContainer::Empty => {
                anyhow::bail!("No entry in .gitmodules matches repo {name} and {url}")
            }
            UniqueContainer::Single(submod_path) => submod_path,
            UniqueContainer::Multiple => {
                anyhow::bail!("Multiple entries in .gitmodules matches repo {name} and {url}")
            }
        };
        Ok(ResolvedFetchParams {
            repo: name,
            path: submod_path.clone(),
            url,
        })
    }
}

#[derive(Args, Debug)]
pub struct Push {
    /// Print the push commands to stdout but do not execute them.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Stop pushing on the first error.
    #[arg(long)]
    pub fail_fast: bool,

    /// A configured git remote in the mono repository or a URL of the top
    /// repository to push to. Submodules are calculated relative this remote.
    #[arg(name = "top-remote", verbatim_doc_comment)]
    pub top_remote: String,

    /// A reference to push from the top repository. Refspec wildcards are not
    /// supported.
    #[arg(id = "refspec", required=true, num_args=1.., value_parser = clap::builder::ValueParser::new(parse_refspec), verbatim_doc_comment)]
    pub refspecs: Vec<(String, String)>,
}

fn parse_refspec(refspec: &str) -> Result<(String, String), std::io::Error> {
    if let Some((lhs, rhs)) = refspec.split_once(':') {
        if rhs.contains(':') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid refspec",
            ));
        }
        let mut rhs = rhs.to_owned();
        if !rhs.starts_with("refs/") {
            rhs = format!("refs/heads/{rhs}");
        }
        Ok((lhs.to_owned(), rhs))
    } else {
        // Automatically force override FETCH_HEAD when set by default.
        Ok((format!("+{refspec}"), "FETCH_HEAD".to_owned()))
    }
}

#[derive(Args, Debug)]
pub struct Replace {
    #[arg(long)]
    /// Dump the project to submodule mapping
    ///    <project>: <module path>
    pub dump: bool,
}
