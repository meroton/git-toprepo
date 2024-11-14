#![allow(dead_code)]


mod cli;

use crate::cli::{Cli, Commands};

use git_toprepo::config;
use git_toprepo::config::{Config, ConfigMap, RepoConfig};
use git_toprepo::config_loader::{ConfigLoader,ConfigLoaderTrait,LocalGitConfigLoader,LocalFileConfigLoader};
use git_toprepo::git::get_gitmodules_info;
use git_toprepo::repo::{normalize, remote_to_repo, Repo, RepoFetcher, Submodule, TopRepo};


use std::fmt::{Display, Formatter};
use std::ops::Not;
use std::{env, io, panic};
use std::path::PathBuf;
use std::process::{Command, exit};
use std::collections::HashMap;

use clap::{Arg, Args, Parser, Subcommand};
use colored::Colorize;
use itertools::Itertools;
use anyhow::Result;
use lazycell::LazyCell;
use gix_config::File;
use bstr::{io::BufReadExt,BStr,BString,ByteSlice,ByteVec};

////////////////////////////////////////////////////////////////////////////////////////////////////

/// Replace references to Gerrit projects to the local file paths of submodules.
fn replace(args: &Cli, replace: &cli::Replace) -> Result<u16> {
    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    struct Mod {
        project: BString,
        path: BString,
    };
    let monorepo = Repo::from_str(&args.cwd)?;
    let main_project = monorepo.gerrit_project();
    let mut modules: Vec<Mod> = monorepo.submodules()?.into_iter()
        .map(|m| Mod{project: m.project, path: m.path}).collect();

    modules.push(Mod{
        project: main_project.into(),
        // TODO: What is the path to the repo? May be upwards.
        path: ".".into(),
    });

    if replace.dump {
        for module in modules {
            println!("{}: {}", module.project, module.path);
        }
        return Ok(0)
    }

    // TODO: This became really cluttered :(
    // In theory, we should also be able to do all the operations within the
    // Byte-string world, but that too was fraught with type conversions.
    let mut map: HashMap<String, String> = HashMap::new();
    for module in modules.into_iter() {
        map.insert(
            <Vec<u8> as Clone>::clone(&module.project).clone().into_string()?,
            <Vec<u8> as Clone>::clone(&module.path).clone().into_string()?,
        );
    }

    for result in std::io::stdin().lines() {
        let line = result?;
        let parts: Vec<&str> = line.split(" ").collect();
        // TODO: Return error and usage instructions here.
        assert!(parts.len() >= 1);

        let mut project = parts[0].to_owned();
        if !project.ends_with(".git") {
            project = format!("{}.git", project);
        }

        let replacement = &map.get(&project).expect(&format!("Could not find key: '{}'", &project));
        let replaced = line.replace(parts[0], replacement);
        println!("{}", replaced);
    }

    Ok(0)
}

fn fetch(args: &Cli, fetch_args: &cli::Fetch) -> Result<u16> {
    let monorepo = Repo::from_str(&args.cwd)?;

    let git_config = LocalGitConfigLoader::new(&monorepo).get_configmap().unwrap();
    let configmap = config::get_configmap(&monorepo, &git_config);

    let git_modules = get_gitmodules_info(
        configmap.extract_mapping("submodule")?,
        &monorepo.get_toprepo_fetch_url(),
    )?;

    let config = Config::new(configmap);
    println!("{}\n{:?}", "Config:".blue(), config);

    let toprepo = TopRepo::from_config(monorepo.get_toprepo_git_dir(), &config);
    let repo_fetcher = RepoFetcher::new(&monorepo);

    let (remote_name, git_module) = remote_to_repo(
        &fetch_args.remote, git_modules, &config,
    );
    let (repo_to_fetch, _) = match remote_name.as_str() {
        TopRepo::NAME => {
            todo!()
        }
        _ => {
            let git_module = git_module.expect(format!(
                "git module information is required for remote: '{}'", remote_name).as_str()
            );

            config.repos.into_iter().find_map(|subrepo_config| {
                if subrepo_config.name != remote_name {
                    return None;
                }

                let name = subrepo_config.name;
                let path = monorepo.get_subrepo_git_dir(&name);
                let repo_to_fetch = Repo::new(name, path);

                let subdir = git_module.path.to_str().unwrap().to_string();

                Some((repo_to_fetch, subdir))
            }).expect(format!(
                "Could not resolve the remote '{}'", fetch_args.remote
            ).as_str())
        }
    };

    todo!()
}

fn config(args: &Cli, c: &cli::Config) -> Result<u16> {
    if ! c.list {
        todo!();
    }

    let monorepo = Repo::from_str(&args.cwd)?;

    let git_config = LocalGitConfigLoader::new(&monorepo).get_configmap().unwrap();
    let configmap = config::get_configmap(&monorepo, &git_config);

    if c.list {
        configmap.list();
    }

    return Ok(0);
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

    let res = match args.command {
        Commands::Init(_) => todo!(),
        Commands::Config(ref config_args) => config(&args, config_args),
        Commands::Refilter => todo!(),
        Commands::Fetch(ref fetch_args) => fetch(&args, fetch_args),
        Commands::Push => todo!(),
        Commands::Replace(ref replace_args) => replace(&args, replace_args),
    };

    res.unwrap();


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
