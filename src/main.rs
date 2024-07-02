mod cli;

use std::path::PathBuf;
use std::process::Command;
use crate::cli::{Cli, Commands};

use clap::{Arg, Args, Parser, Subcommand};


//THe repo class seems unnecessary, as the only thing
// it does is sanitize a file path
struct MonoRepo {
    path: PathBuf,
    name: String,
}

impl MonoRepo {
    fn new(repo: String) -> MonoRepo {
        let command = Command::new("git")
            .args(["-C", repo.as_str()])
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .unwrap();

        let path = PathBuf::from(
            String::from_utf8(command.stdout).unwrap()
        );

        MonoRepo {
            path,
            name: "mono repo".to_string(),
        }
    }

    fn get_toprepo_fetch_url(self) { todo!() }
}


fn fetch(args: Cli) {
    println!("Fetch!");
    let monorepo = MonoRepo::new(args.cwd);

    println!("{:?}", monorepo.path)
}


fn main() {
    let args = Cli::parse();
    println!("{:?}", args);

    match args.command {
        Commands::Init(_) => {}
        Commands::Config => {}
        Commands::Refilter => {}
        Commands::Fetch(_) => { fetch(args) }
        Commands::Push => {}
    }
}
