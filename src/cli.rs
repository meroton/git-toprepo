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
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::GitPath;
use git_toprepo::git::repo_relative_path;
use git_toprepo::gitmodules::SubmoduleUrlExt as _;
use git_toprepo::loader::SubRepoLedger;
use git_toprepo::repo_name::RepoName;
use git_toprepo::repo_name::SubRepoName;
use git_toprepo::util::UniqueContainer;
use itertools::Itertools;
use std::ops::Deref as _;
use std::path::Path;
use std::path::PathBuf;

const ABOUT: &str = "\
git-toprepo - git-submodules made easy with a client-side monorepo

git-toprepo combines submodules into a common history, similar to git-subtree, \
and lets you work with an emulated monorepo locally \
while keeping the original submodule structure on the remote server.\
";

/// When using `global=true` the `display_order` of the arguments is mixed up and
/// they get interleaved. One alternative is to use a `display_order`, but the
/// user easily looses focus if all the global options are not separated from the
/// arguments of interest for a subcommand. Therefore, put global arguments under
/// a separate heading.
const GLOBAL_HELP_HEADING: &str = "Global options";

#[derive(Parser, Debug)]
#[command(about = ABOUT)]
pub struct Cli {
    /// Run as if started in <PATH>.
    #[arg(value_name = "PATH", short = 'C')]
    pub working_directory: Option<PathBuf>,

    #[clap(flatten)]
    pub log_level: LogLevelArg,

    #[arg(
        long = "no-progress",
        action = clap::ArgAction::SetFalse,
        help = "Hide scrolling progress bars",
        help_heading=GLOBAL_HELP_HEADING,
        global=true,
    )]
    pub show_progress: bool,

    #[command(subcommand)]
    pub command: GitAndCommands,
}

const LOG_LEVEL_DEFAULT: log::LevelFilter = log::LevelFilter::Info;

macro_rules! verbosity_default {
    () => {
        3
    };
}
macro_rules! verbosity_max {
    () => {
        5
    };
}
macro_rules! verbosity_doc {
    () => {
        concat!(
            "Set a specific log verbosity from 0 to ",
            verbosity_max!(),
            ".",
        )
    };
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct LogLevelArg {
    /// Increase log verbosity.
    #[arg(
        long = "verbose",
        short = 'v',
        help = "Increase log verbosity with -v or -vv, or ...",
        help_heading=GLOBAL_HELP_HEADING,
        global=true,
        action = clap::ArgAction::Count,
    )]
    verbose_increment: u8,

    #[doc = verbosity_doc!()]
    #[arg(
        long,
        value_name = "LEVEL",
        help = format!("... set {}", format!("{}", verbosity_doc!()).strip_prefix("Set ").unwrap().strip_suffix(".").unwrap()),
        help_heading=GLOBAL_HELP_HEADING,
        global=true,
        default_value = verbosity_default!().to_string(),
        value_parser = clap::builder::RangedU64ValueParser::<u8>::new().range(0..=verbosity_max!()),
    )]
    verbosity: u8,

    /// Use `-q` to hide all output to stderr.
    #[arg(
        long,
        short = 'q',
        help_heading=GLOBAL_HELP_HEADING,
        global=true,
    )]
    pub quiet: bool,
}

impl LogLevelArg {
    /// Get the log level based on the verbosity and quietness.
    pub fn value(&self) -> Result<log::LevelFilter> {
        let levels = log::LevelFilter::iter().collect_vec();
        debug_assert_eq!(levels.len(), verbosity_max!() + 1);
        debug_assert_eq!(levels.get(verbosity_default!()), Some(&LOG_LEVEL_DEFAULT));
        let verbosity = if self.quiet {
            0
        } else {
            (self.verbosity + self.verbose_increment) as usize
        };
        let Some(log_level) = levels.get(verbosity) else {
            anyhow::bail!(
                "Too high verbosity level {}, maximum is {}",
                verbosity,
                verbosity_max!(),
            );
        };
        Ok(*log_level)
    }
}

#[derive(Subcommand, Debug)]
pub enum GitAndCommands {
    /// Ignored word to simplify pasting copied commands into e.g.
    /// `git-toprepo git fetch ...`.
    #[command(subcommand)]
    Git(Commands),
    #[command(flatten)]
    Command(Commands),
}

impl GitAndCommands {
    /// Get the inner command.
    pub fn actual(&self) -> &Commands {
        match self {
            GitAndCommands::Git(cmd) => cmd,
            GitAndCommands::Command(cmd) => cmd,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a repository and the git-config, without fetching from the remote.
    Init(Init),
    /// Initialize a repository and fetch from the remote.
    Clone(Clone),
    /// Manage the git-toprepo configuration.
    #[command(subcommand)]
    Config(Config),
    /// Rerun the history combination and submodule content expansion.
    Recombine(Recombine),
    /// Fetch commits from the top repository and expand submodules.
    Fetch(Fetch),
    /// Push commits to the respective remotes of each filtered submodule.
    Push(Push),

    /// Show information about git-toprepo in the current repository.
    Info(Info),
    #[command(subcommand)]
    Dump(Dump),

    /// Print the version of the git-toprepo tool.
    #[command(aliases = ["-V", "--version"])]
    Version,
}

#[derive(Args, Debug)]
pub struct Init {
    /// The repository to be configured as remote.
    pub repository: String,

    /// The name of a new directory to create the repository in. If no directory
    /// is given, the basename of the repository is used.
    pub directory: Option<PathBuf>,

    /// Initialize even if the target directory is not empty.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct Clone {
    #[command(flatten)]
    pub init: Init,

    #[command(flatten)]
    pub recombine: Recombine,

    /// After fetching the top repository, skip fetching the submodules.
    #[arg(long)]
    pub minimal: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Config {
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

// Inspired by https://stackoverflow.com/questions/72588743/can-you-use-a-const-value-in-docs-in-rust.
macro_rules! info_exit_code_false {
    () => {
        3
    };
}
macro_rules! info_is_emulated_monorepo_doc {
    () => {
        concat!(
            "Exit with code ",
            info_exit_code_false!(),
            " if the repository is not initialized by git-toprepo."
        )
    };
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct Info {
    #[arg(value_enum)]
    pub value: Option<InfoValue>,

    // Make clap detect the docs.
    #[doc = info_is_emulated_monorepo_doc!()]
    #[arg(long, help = info_is_emulated_monorepo_doc!().trim_end_matches('.'))]
    pub is_emulated_monorepo: bool,
}

impl Info {
    /// The exit code for `git-toprepo info --<flag>` when the answer is "false".
    pub const EXIT_CODE_FALSE: u8 = info_exit_code_false!();
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
#[value(rename_all = "kebab-case")]
pub enum InfoValue {
    /// The location of the configuration file.
    ConfigLocation,
    /// The current git-worktree path.
    CurrentWorktree,
    /// The .git directory path for the current worktree.
    GitDir,
    /// The path to the import cache file.
    ImportCache,
    /// The main worktree path, which might be the current worktree.
    MainWorktree,
    /// The version of git-toprepo.
    Version,
}

impl InfoValue {
    pub const ALL_VARIANTS: [InfoValue; 6] = [
        InfoValue::ConfigLocation,
        InfoValue::CurrentWorktree,
        InfoValue::GitDir,
        InfoValue::ImportCache,
        InfoValue::MainWorktree,
        InfoValue::Version,
    ];
}

impl std::fmt::Display for InfoValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            InfoValue::ConfigLocation => "config-location",
            InfoValue::CurrentWorktree => "current-worktree",
            InfoValue::GitDir => "git-dir",
            InfoValue::ImportCache => "import-cache",
            InfoValue::MainWorktree => "main-worktree",
            InfoValue::Version => "version",
        };
        write!(f, "{s}")
    }
}

/// Experimental feature: dump internal states to stdout.
/// Do not script against these.
// If you want to use these in your own tools and pipeline please file a feature
// request issue so we can guarantee a stable API for your use-case.
#[derive(Subcommand, Debug)]
pub enum Dump {
    /// Prints the current working directory, after resolving the `-C` argument.
    Cwd,
    /// Dump the repository import cache as JSON to stdout.
    ImportCache(DumpImportCache),
    /// Dump the git submodule path mappings to remote projects.
    // TODO: 2025-09-22 Take individual modules and paths as arguments?
    // TODO: 2025-09-22 Take a full path for a file and convert it to its remote repo, or
    // url path?
    GitModules,
}

#[derive(Args, Debug)]
pub struct DumpImportCache {
    /// The cache file to dump, - for stdin or unset to auto detect.
    #[arg(id = "file")]
    pub file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct Recombine {
    /// Continue as much as possible after an error.
    #[arg(long)]
    pub keep_going: bool,

    /// Number of concurrent threads to load the repository.
    #[arg(long("jobs"), value_name = "N", default_value = "7")]
    pub job_count: std::num::NonZero<u16>,

    /// Skip fetching missing submodule commits.
    #[arg(long)]
    pub no_fetch: bool,

    /// Reuse information from the cache and skip combining the repositories
    /// from scratch.
    #[arg(long)]
    pub use_cache: bool,
}

#[derive(Args, Debug)]
pub struct Fetch {
    /// Continue as much as possible after an error.
    #[arg(long)]
    pub keep_going: bool,

    /// Number of concurrent threads to perform git-fetch and the filtering.
    #[arg(long("jobs"), value_name = "N", default_value = "7")]
    pub job_count: std::num::NonZero<u16>,

    /// Skip the combining history step after fetching the top repository.
    #[arg(long)]
    pub skip_combine: bool,

    /// A configured git-remote in the mono repository, a URL to a remote top
    /// repository or a URL to a remote submodule repository. This argument will
    /// be used to resolve which URL to fetch from and which directory to filter
    /// into, unless `--path` overrides the directory.
    #[arg(value_name = "REMOTE-ISH")]
    pub remote: Option<String>,

    /// The worktree path to filter into, relative to the working directory.
    /// This path is used to override the repo to filter into which is otherwise
    /// deduced from the `remote` argument.
    #[arg(long, value_name = "SUBMOD")]
    pub path: Option<PathBuf>,

    /// A reference to fetch from the top repository or submodule. Refspec
    /// wildcards are not supported.
    #[arg(value_name = "REF", num_args=1.., value_parser = clap::builder::ValueParser::new(parse_refspec))]
    pub refspecs: Option<Vec<(String, String)>>,
}

pub fn resolve_remote_and_path(
    args: &Fetch,
    repo: &gix::Repository,
    ledger: &SubRepoLedger,
) -> Result<ResolvedFetchParams> {
    FetchParamsResolver::new(repo, ledger)?
        .resolve_remote_and_path(args.remote.as_deref(), args.path.as_deref())
}

/// Internal structure when the `Fetch` arguments have been parsed given config and `.gitmodules`.
#[derive(Debug)]
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
    /// Expansion ledger of subrepos.
    ledger: &'a SubRepoLedger,
    worktree: PathBuf,
    /// Cache computation of the infos. They are not expected to be mutated
    /// during execution. If they are we will not pick up the changes.
    gitmod_infos: GitModulesInfo,
}

impl<'a> FetchParamsResolver<'a> {
    pub fn new(repo: &'a gix::Repository, ledger: &'a SubRepoLedger) -> Result<Self> {
        let worktree = repo
            .workdir()
            .context("Worktree missing in git repository")?;
        let gitmod_infos = GitModulesInfo::parse_dot_gitmodules_in_repo(repo)?;
        Ok(Self {
            repo,
            ledger,
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
        let mut url = gix::Url::from_bytes(remote_bstr)?;
        if url.scheme == gix::url::Scheme::File
            && let Ok(cwd) = std::env::current_dir()
            && let Some(cwd_str) = cwd.to_str()
            && let Ok(cwd_url) = gix::Url::from_bytes(cwd_str.into())
        {
            // If the remote is a local directory, the matching from url to the
            // git-toprepo configured submodule need the path to be resolved to
            // an absolute path. Matching ../../../path/to/repo doesn't work.
            url = cwd_url.join(&url);
        }
        // TODO: 2025-09-22 If we refactor the repo view to contain a list of all
        // *projects* including super itself. We do not need tiered access here.
        // #unified-git-config.
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
            .ledger
            .get_existing_config_from_url(&submod_url)?
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
            .ledger
            .get_name_from_similar_full_url(url.clone(), &base_url)?;
        let RepoName::SubRepo(submod_name) = &name else {
            unreachable!("Already checked that top URLs are not matching");
        };
        let submod_config = self.ledger.subrepos.get(submod_name).unwrap();
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

    /// Number of concurrent threads to load the repository.
    #[arg(long("jobs"), value_name = "N", default_value = "7")]
    pub job_count: std::num::NonZero<u16>,

    /// Forward `--force` to `git push`. `--force-with-lease` is unsupported.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// A configured git remote in the mono repository or a URL of the top
    /// repository to push to. Submodules are calculated relative this remote.
    #[arg(value_name = "TOP-REMOTE")]
    pub top_remote: String,

    /// A reference to push from the top repository. Refspec wildcards are not
    /// supported.
    #[arg(value_name = "REFSPEC", required=true, num_args=1.., value_parser = clap::builder::ValueParser::new(parse_refspec))]
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
        let remote_ref = refspec.strip_prefix('+').unwrap_or(refspec);
        Ok((format!("+{remote_ref}"), "FETCH_HEAD".to_owned()))
    }
}
