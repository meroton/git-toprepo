#![allow(dead_code)]

mod cli;

use git_toprepo::config::{self, GitTopRepoConfig};

use crate::cli::{Cli, Commands};
use anyhow::{Context, Result};
use bstr::ByteSlice;
use clap::Parser;
use colored::Colorize;
use std::io::Read;
use std::panic;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn init(init_args: &cli::Init) -> Result<ExitCode> {
    let url = gix_url::Url::from_bytes(init_args.repository.as_bytes().as_bstr())?;
    // TODO: Should url.path be canonicalized for scheme=File like git does?
    let directory = match &init_args.directory {
        Some(dir) => dir.clone(),
        None => {
            let url_path = url.path.to_str()?;
            let name = Path::new(url_path)
                .file_stem()
                .context("URL path contains no basename")?;
            PathBuf::from(name)
        }
    };

    let toprepo = git_toprepo::repo::TopRepo::create(directory, url)?;
    eprintln!("Initialized git-toprepo in {}", toprepo.directory.display());

    if init_args.fetch {
        eprintln!("Fetching from {}", toprepo.url);
        toprepo.fetch()?;
    }
    Ok(ExitCode::SUCCESS)
}

fn config(config_args: &cli::Config) -> Result<ExitCode> {
    let load_config_from_file = |file: &Path| -> Result<GitTopRepoConfig> {
        if file == PathBuf::from("-") {
            || -> Result<GitTopRepoConfig> {
                let mut toml_string = String::new();
                std::io::stdin().read_to_string(&mut toml_string)?;
                config::GitTopRepoConfig::parse_config_toml_string(&toml_string)
            }()
            .context("Loading from stdin")
        } else {
            || -> Result<GitTopRepoConfig> {
                let toml_string = std::fs::read_to_string(file)?;
                config::GitTopRepoConfig::parse_config_toml_string(&toml_string)
            }()
            .with_context(|| format!("Loading config file {}", file.display()))
        }
    };
    match &config_args.config_command {
        cli::ConfigCommands::Location(args) => {
            let mut search_log = match args.verbose {
                true => Some(String::new()),
                false => None,
            };
            let location = config::GitTopRepoConfig::find_configuration_location(
                Path::new(""),
                search_log.as_mut(),
            )?;
            match search_log {
                Some(log) => eprint!("{}", log),
                None => (),
            };
            println!("{}", location);
        }
        cli::ConfigCommands::Show(args) => {
            let mut search_log = match args.verbose {
                true => Some(String::new()),
                false => None,
            };
            let config = config::GitTopRepoConfig::load_config_from_repo_with_log(
                Path::new(""),
                search_log.as_mut(),
            )?;
            match search_log {
                Some(log) => eprint!("{}", log),
                None => (),
            };
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Normalize(args) => {
            let config = load_config_from_file(args.file.as_path())?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Validate(args) => {
            let _ = load_config_from_file(args.file.as_path())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
/*
/// Replace references to Gerrit projects to the local file paths of submodules.
fn replace(args: &Cli, replace: &cli::Replace) -> Result<ExitCode> {
    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    struct Mod {
        project: BString,
        path: BString,
    }
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
        return Ok(ExitCode::SUCCESS)
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

        let project = parts[0].to_owned();

        let replacement = &map.get(&project).expect(&format!("Could not find key: '{}'", &project));
        let replaced = line.replace(parts[0], replacement);
        println!("{}", replaced);
    }

    Ok(ExitCode::SUCCESS)
}
*/

fn refilter() -> Result<ExitCode> {
    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::new())?;
    todo!("Implement refilter");
}

fn fetch(_fetch_args: &cli::Fetch) -> Result<ExitCode> {
    //let monorepo = Repo::from_str(&args.cwd)?;
    todo!("Implement fetch");
}

fn main() -> Result<ExitCode> {
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
    match args.working_directory {
        Some(ref path) => std::env::set_current_dir(path)?,
        None => (),
    };
    let res: ExitCode = match args.command {
        Commands::Init(ref init_args) => init(init_args)?,
        Commands::Config(ref config_args) => config(config_args)?,
        Commands::Refilter => refilter()?,
        Commands::Fetch(ref fetch_args) => fetch(fetch_args)?,
        Commands::Push => todo!(),
        Commands::Replace(ref _replace_args) => todo!(), //replace(&args, replace_args)?,
    };
    Ok(res)
}
