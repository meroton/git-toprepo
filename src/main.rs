mod cli;
mod config;

use crate::cli::{Cli, Commands};
use crate::config::ConfigMap;

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::ops::Not;
use std::path::PathBuf;
use std::process::Command;

use clap::{Arg, Args, Parser, Subcommand};
use itertools::Itertools;


//THe repo class seems unnecessary, as the only thing
// it does is sanitize a file path
#[derive(Debug)]
struct MonoRepo {
    path: PathBuf,
    name: String,
}

#[allow(dead_code)]
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

    fn get_toprepo_fetch_url(&self) { todo!() }
}

fn fetch(args: Cli) {
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


    let mut a = ConfigMap::new();
    a.push("lorem.ipsum.abc", _vec_to_string(vec!["a", "b", "c"]));
    a.push("lorem.ipsum.123", _vec_to_string(vec!["1", "2"]));

    println!("{}", a);

    a.push("lorem.ipsum.123", _vec_to_string(vec!["3", "2"]));
    a.push("lorem.dolor.sit", _vec_to_string(vec!["amet", "consectetur"]));

    println!("{}", a);

    let temp = a.extract_mapping("lorem");

    println!("{:?}", temp);

    let (b, c) = temp.iter().next_tuple().unwrap();

    println!("{:?}", b);
    println!("{:?}", c);
}

fn _vec_to_string(vec: Vec<&str>) -> Vec<String> {
    vec.iter().map(|s| s.to_string()).collect()
}