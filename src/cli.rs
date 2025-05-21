/** Command line argument definition using subcommands.
 *
 * See also https://jmmv.dev/2013/08/cli-design-putting-flags-to-good-use.html#bad-using-flags-to-select-subcommands.
 */
use clap::ArgAction;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use std::path::PathBuf;
use std::str::FromStr as _;

const ABOUT: &str = "git-submodule made easy with git-toprepo.

git-toprepo merges subrepositories into a common history, similar to git-subtree.\
";

#[derive(Parser, Debug)]
#[command(version, about = ABOUT)]
pub struct Cli {
    /// Run as if started in <path> as current working directory.
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
#[command(version)]
pub enum Commands {
    Init(Init),
    Config(Config),
    Refilter(Refilter),
    Fetch(Fetch),
    /// Push commits that are not part of any `refs/remotes/origin/*` to a
    /// remote.
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
}

#[derive(Args, Debug)]
pub struct Init {
    /// Skip the initial fetch of the top repository. This means the the
    /// default configuration will not be fetched either.
    #[clap(long = "no-fetch", action = ArgAction::SetFalse)]
    pub fetch: bool,

    /// The remote repository to clone from.
    pub repository: String,

    /// The name of a new directory to clone into. If no directory is given, the
    /// basename of the repository is used.
    pub directory: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct Config {
    #[command(subcommand)]
    pub config_command: ConfigCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Prints the configuration location.
    Location,
    /// Show the configuration of the current repository.
    Show,
    /// Reads a configuration and prints it in normalized form.
    Normalize(ConfigNormalize),
    /// Verifies that a given configuration can be loaded.
    Validate(ConfigValidate),
}

#[derive(Args, Debug)]
pub struct ConfigNormalize {
    /// The configuration file to normalize or - for stdin.
    #[arg(id = "file")]
    pub file: PathBuf,
}

#[derive(Args, Debug)]
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
    pub jobs: std::num::NonZero<u32>,

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
    pub jobs: std::num::NonZero<u32>,

    /// Skip the filtering step after fetching the top repository.
    #[arg(long)]
    pub skip_filter: bool,

    /// The repository to fetch to, either the top repository or a submodule.
    #[arg(long, name = "repo", value_parser = clap::builder::ValueParser::new(parse_repo_name))]
    pub repo: Option<git_toprepo::repo_name::RepoName>,

    /// A configured git remote in the mono repository or a URL to fetch from.
    /// If a URL is specified, it will be resolved into either the top
    /// repository or one of the submodules. Submodules are calculated relative
    /// this remote.
    #[arg(name = "top-remote-or-submodule-url", default_value_t = String::from("origin"), verbatim_doc_comment)]
    pub top_or_submodule_remote: String,

    /// A reference to fetch from the top repository or submodule. Refspec
    /// wildcards are not supported.
    #[arg(id = "ref", num_args=1.., value_parser = clap::builder::ValueParser::new(parse_refspec), verbatim_doc_comment)]
    pub refspecs: Option<Vec<(String, String)>>,
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

fn parse_repo_name(repo_name: &str) -> Result<git_toprepo::repo_name::RepoName, std::io::Error> {
    git_toprepo::repo_name::RepoName::from_str(repo_name).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid repository name")
    })
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
        Ok((refspec.to_owned(), "FETCH_HEAD".to_owned()))
    }
}

#[derive(Args, Debug)]
pub struct Replace {
    #[arg(long)]
    /// Dump the project to submodule mapping
    ///    <project>: <module path>
    pub dump: bool,
}
