mod cli;
mod config;
mod util;
mod config_loader;

use crate::cli::{Cli, Commands};
use crate::config::{Config, ConfigAccumulator, ConfigMap};

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::ops::Not;
use std::{env, io, panic};
use std::panic::PanicInfo;
use std::path::PathBuf;
use std::process::Command;

use clap::{Arg, Args, Parser, Subcommand};
use colored::Colorize;
use itertools::Itertools;
use url::Url;
use crate::util::iter_to_string;


//THe repo class seems unnecessary, as the only thing
// it does is sanitize a file path
#[derive(Debug)]
struct Repo {
    name: String,
    path: PathBuf,
}

#[allow(dead_code)]
impl Repo {
    fn new(repo: String) -> Repo {
        println!("Repo: {}", repo);

        //PosixPath('/home/lack/Documents/Kod/RustRover/git-toprepo')
        let command = Command::new("git")
            .args(["-C", repo.as_str()])
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .expect(format!("Failed to parse repo path {}", repo).as_str());
        println!("stdout: {:?}", command.stdout);
        let path = String::from_utf8(command.stdout).unwrap()
            .strip_suffix("\n").unwrap().to_string();

        let cwd = env::current_dir().unwrap_or(PathBuf::new());
        let mut path = PathBuf::from(path);

        if path == cwd {
            path = PathBuf::from(".")
        }
        if let Ok(relative) = path.strip_prefix(cwd) {
            path = relative.to_path_buf();
        }

        println!("Path: {:?}", path);

        Repo {
            name: "mono repo".to_string(),
            path,
        }
    }

    fn from_config(path: PathBuf, config: Config) -> Repo {
        todo!()
    }

    fn get_toprepo_fetch_url(&self) -> Option<&str> { todo!() }

    fn get_toprepo_dir(&self) -> PathBuf { todo!() }

    fn get_subrepo_dir(&self, name: &str) -> PathBuf { todo!() }
}

fn fetch(args: Cli) -> u16 {
    let monorepo = Repo::new(args.cwd);
    println!("Monorepo path: {:?}", monorepo.path);

    let config_accumulator = ConfigAccumulator::new(&monorepo, true);
    let configmap = config_accumulator.load_main_config();

    if let Err(err) = configmap {
        panic!("{}", err);
    }
    let configmap = configmap.unwrap();
    println!("{}", configmap);

    let config = Config::new(configmap);
    //let toprepo = Repo::new(monorepo.ge(), config);
    todo!()
}


fn main() {
    // Make panic messages red.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic| {
        if let Some(payload) = panic.payload().downcast_ref::<&str>() {
            println!("\n{}\n", payload.red());
        }
        if let Some(payload) = panic.payload().downcast_ref::<String>() {
            println!("\n{}\n", payload.red());
        }
        default_hook(panic);
    }));

    let args = Cli::parse();
    println!("{:?}", args);

    match args.command {
        Commands::Init(_) => {}
        Commands::Config => {}
        Commands::Refilter => {}
        Commands::Fetch(_) => { fetch(args); }
        Commands::Push => {}
    }


//    ////////////////////////////////////////////////////////////////////////////////////////////////
//
//    let mut a = ConfigMap::new();
//    a.push("lorem.ipsum.abc", iter_to_string(["a", "b", "c"]));
//    a.push("lorem.ipsum.123", iter_to_string(["1", "2"]));
//
//    println!("{}", a);
//
//    a.push("lorem.ipsum.123", iter_to_string(["3", "2"]));
//    a.push("lorem.dolor.sit", iter_to_string(["amet", "consectetur"]));
//
//    println!("{}", a);
//
//    let temp = a.extract_mapping("lorem");
//
//    println!("{:?}", temp);
//
//    let (b, c) = temp.iter().next_tuple().unwrap();
//
//    println!("{:?}", b);
//    println!("{:?}", c);
}
