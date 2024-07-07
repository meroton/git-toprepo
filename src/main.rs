mod cli;
mod config;
mod util;

use crate::cli::{Cli, Commands};
use crate::config::{Config, ConfigAccumulator, ConfigMap};

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::ops::Not;
use std::path::PathBuf;
use std::process::Command;

use clap::{Arg, Args, Parser, Subcommand};
use itertools::Itertools;
use url::Url;


//THe repo class seems unnecessary, as the only thing
// it does is sanitize a file path
#[derive(Debug)]
struct Repo {
    path: PathBuf,
    name: String,
}

#[allow(dead_code)]
impl Repo {
    fn new(repo: String) -> Repo {
        let command = Command::new("git")
            .args(["-C", repo.as_str()])
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .unwrap();

        let path = PathBuf::from(
            String::from_utf8(command.stdout).unwrap()
        );

        Repo {
            path,
            name: "mono repo".to_string(),
        }
    }

    fn from_config(path: PathBuf, config: Config) -> Repo {
        todo!()
    }

    fn get_toprepo_fetch_url(&self) -> Option<Url> { todo!() }
}

fn fetch(args: Cli) -> u16 {
    let monorepo = Repo::new(args.cwd);
    println!("Monorepo path: {:?}", monorepo.path);

    let config_accumulator = ConfigAccumulator::new(&monorepo, true);
    let configmap = config_accumulator.load_main_config();

    if let Err(err) = configmap {
        panic!("{}", err);
        return 1
    }
    let configmap = configmap.unwrap();
    println!("{}", configmap);

    let config = Config::new(configmap);

    todo!()
}


fn main() {
    let args = Cli::parse();
    println!("{:?}", args);

    match args.command {
        Commands::Init(_) => {}
        Commands::Config => {}
        Commands::Refilter => {}
        Commands::Fetch(_) => { fetch(args); }
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