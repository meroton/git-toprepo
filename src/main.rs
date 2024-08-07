#![allow(dead_code)]


mod cli;
mod config;
mod util;
mod config_loader;
mod repo;
mod git;

use crate::cli::{Cli, Commands, Fetch};
use crate::config::{Config, ConfigAccumulator, ConfigMap, RepoConfig};

use std::fmt::{Display, Formatter};
use std::ops::Not;
use std::{env, io, panic};
use std::path::PathBuf;
use std::process::Command;

use clap::{Arg, Args, Parser, Subcommand};
use colored::Colorize;
use itertools::Itertools;
use lazycell::LazyCell;
use crate::config_loader::LocalFileConfigLoader;
use crate::git::get_gitmodules_info;
use crate::repo::{Repo, RepoFetcher, TopRepo};
use crate::util::{iter_to_string, join_submodule_url, remote_to_repo};


////////////////////////////////////////////////////////////////////////////////////////////////////

fn fetch(args: &Cli, fetch_args: &Fetch) -> u16 {
    let monorepo = Repo::new(&args.cwd);
    println!("Monorepo path: {:?}", monorepo.path);

    let config_accumulator = ConfigAccumulator::new(&monorepo, true);
    let configmap = config_accumulator.load_main_config();

    if let Err(err) = configmap {
        panic!("{}", err);
    }
    let configmap = configmap.unwrap();
    println!("{}", "Congifmap".blue());
    for (key, values) in &configmap.map {
        println!("{}: {:?}", key, values);
    }

    let config = Config::new(configmap);
    println!("{}\n{:?}", "Config:".blue(), config);

    let toprepo = TopRepo::from_config(monorepo.get_toprepo_dir(), &config);
    let repo_fetcher = RepoFetcher::new(&monorepo);

    let git_modules = get_gitmodules_info(
        LocalFileConfigLoader::new(monorepo.path.join(".gitmodules"), true).into(),
        &monorepo.get_toprepo_fetch_url()
    );

    let maybe = remote_to_repo(&fetch_args.remote, git_modules, config);

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
        Commands::Fetch(ref fetch_args) => { fetch(&args, fetch_args); }
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
