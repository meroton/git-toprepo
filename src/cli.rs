use clap::{Args, Parser, Subcommand};
use std::env;
use std::string::ToString;

///TODO: Program description
#[derive(Parser, Debug)]
#[command(version)]
pub struct Cli {
    #[arg(default_value_t = get_cwd())]
    pub cwd: String,

    #[command(subcommand)]
    pub command: Commands,

}

#[derive(Subcommand, Debug)]
#[command(version)]
pub enum Commands {
    Init(Init),
    Config,
    Refilter,
    Fetch(Fetch),
    Push,
}

#[derive(Args, Debug)]
pub struct Init {
    repository: String,

    directory: String,
}

#[derive(Args, Debug)]
pub struct Fetch {
    #[arg(long)]
    skip_filter: bool,

    #[arg(default_value_t = String::from("origin"))]
    pub remote: String,

    #[arg(id="ref")]
    reference: Option<String>,
}


fn get_cwd() -> String {
    env::current_dir().unwrap().to_str().unwrap().to_string()
}