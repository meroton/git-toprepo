/** Command line argument definition using subcommands.
 *
 * See also https://jmmv.dev/2013/08/cli-design-putting-flags-to-good-use.html#bad-using-flags-to-select-subcommands.
 */
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

const ABOUT: &str = "git-submodule made easy with git-toprepo.

git-toprepo merges subrepositories into a common history, similar to git-subtree.\
";

#[derive(Parser, Debug)]
#[command(version, about = ABOUT)]
pub struct Cli {
    /// Run as if started in <path> as current working directory.
    #[arg(name = "path", short = 'C')]
    pub working_directory: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
#[command(version)]
pub enum Commands {
    Init(Init),
    Config(Config),
    Refilter, // Unimplemented
    Fetch(Fetch),
    Push, // Unimplemented

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
    repository: String,

    directory: String,
}

#[derive(Args, Debug)]
pub struct Config {
    #[command(subcommand)]
    pub config_command: ConfigCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Prints the configuration location.
    Location(ConfigLocation),
    /// Show the configuration of the current repository.
    Show(ConfigShow),
    /// Reads a configuration and prints it in normalized form.
    Normalize(ConfigNormalize),
    /// Verifies that a given configuration can be loaded.
    Validate(ConfigValidate),
}

#[derive(Args, Debug)]
pub struct ConfigLocation {
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

#[derive(Args, Debug)]
pub struct ConfigShow {
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
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

#[derive(Args, Debug)]
pub struct Fetch {
    #[arg(long)]
    skip_filter: bool,

    #[arg(default_value_t = String::from("origin"))]
    pub remote: String,

    #[arg(id = "ref")]
    reference: Option<String>,
}

#[derive(Args, Debug)]
pub struct Replace {
    #[arg(long)]
    /// Dump the project to submodule mapping
    ///    <project>: <module path>
    pub dump: bool,
}
