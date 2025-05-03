#![allow(dead_code)]

mod cli;

use crate::cli::Cli;
use crate::cli::Commands;
use anyhow::Context;
use anyhow::Result;
use bstr::ByteSlice as _;
use clap::Parser;
use colored::Colorize;
use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use std::collections::HashSet;
use std::io::Read;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::vec;

fn init(init_args: &cli::Init) -> Result<ExitCode> {
    let url = gix::url::Url::from_bytes(init_args.repository.as_bytes().as_bstr())?;
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
        toprepo.fetch_toprepo()?;
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
            if let Some(log) = search_log {
                eprint!("{}", log);
            }
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
            if let Some(log) = search_log {
                eprint!("{}", log);
            }
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

fn refilter(args: &cli::Refilter) -> Result<ExitCode> {
    fetch_and_refilter(
        &cli::Fetch {
            keep_going: args.keep_going,
            jobs: args.jobs,
            skip_filter: false,
            repo: None,
            super_or_submodule_remote: "origin".to_owned(),
            refspecs: None,
        },
        |commit_loader| {
            commit_loader.fetch_missing_commits = false;
            commit_loader.load_repo(git_toprepo::repo_name::RepoName::Top)
        },
    )
}
fn fetch(fetch_args: &cli::Fetch) -> Result<ExitCode> {
    fetch_and_refilter(fetch_args, |commit_loader| {
        commit_loader.load_after_fetch = !fetch_args.skip_filter;
        // TODO: refspecs
        commit_loader.fetch_repo(git_toprepo::repo_name::RepoName::Top, vec![None]);
        Ok(())
    })
}

fn fetch_and_refilter<F>(fetch_args: &cli::Fetch, commit_loader_setup: F) -> Result<ExitCode>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    if fetch_args.repo.is_some() {
        todo!("Implement repo argument");
    }
    if fetch_args.super_or_submodule_remote != "origin" {
        todo!("Implement super_or_submodule_remote argument");
    }
    if fetch_args.refspecs.is_some() {
        todo!("Implement refspecs argument");
    }

    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::from("."))?;
    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(fetch_args.keep_going);
    let mut config =
        git_toprepo::config::GitTopRepoConfig::load_config_from_repo(&toprepo.directory)?;
    let mut log_config = config.log.clone();

    let log_receiver =
        git_toprepo::log::LogReceiver::new_stderr(HashSet::new(), error_mode.clone());
    let mut top_repo_cache = git_toprepo::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
        toprepo.gix_repo.git_dir(),
        Some(&config.checksum),
        &log_receiver.get_logger(),
    )?
    .unpack()?;
    log_receiver.join().check()?;

    let mut result = git_toprepo::log::log_task_to_stderr(
        error_mode.clone(),
        &mut log_config,
        |logger, progress| {
            let gix_toprepo = toprepo.gix_repo.to_thread_local();
            (|| -> Result<()> {
                let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                    gix_toprepo.clone(),
                    &mut top_repo_cache.repos,
                    &mut config,
                    progress.clone(),
                    logger.clone(),
                    error_mode.interrupted(),
                    threadpool::ThreadPool::new(fetch_args.jobs.get() as usize),
                )?;
                commit_loader_setup(&mut commit_loader)?;
                commit_loader.join();
                Ok(())
            })()
            .context("Failed to fetch")?;

            if !fetch_args.skip_filter {
                toprepo
                    .refilter(&mut top_repo_cache, &config, logger, progress)
                    .map_err(|_| anyhow::anyhow!("Failed to filter"))?;
            }
            Ok(())
        },
    )
    .map(|_| ExitCode::SUCCESS);

    // Store some result files.
    if let Err(err) = git_toprepo::repo_cache_serde::SerdeTopRepoCache::pack(
        &top_repo_cache,
        config.checksum.clone(),
    )
    .store_to_git_dir(toprepo.gix_repo.git_dir())
    {
        if result.is_ok() {
            result = Err(err);
        }
    }
    const EFFECTIVE_TOPREPO_CONFIG: &str = "toprepo/last-effective-git-toprepo.toml";
    config.log = log_config;
    if let Err(err) =
        config.save_config_to_repo(&toprepo.gix_repo.git_dir().join(EFFECTIVE_TOPREPO_CONFIG))
    {
        if result.is_ok() {
            result = Err(err);
        }
    }
    result
}

fn dump(dump_args: &cli::Dump) -> Result<ExitCode> {
    match dump_args {
        cli::Dump::ImportCache => dump_import_cache(),
    }
}

fn dump_import_cache() -> Result<ExitCode> {
    let toprepo = gix::open("")?;

    let log_receiver = git_toprepo::log::LogReceiver::new(
        HashSet::new(),
        git_toprepo::log::ErrorMode::FailFast(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )),
        |msg| eprintln!("{}", msg),
    );
    let serde_repo_states = git_toprepo::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
        toprepo.git_dir(),
        None,
        &log_receiver.get_logger(),
    )?;
    serde_repo_states.dump_as_json(std::io::stdout())?;

    log_receiver.join().check()?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> Result<ExitCode> {
    // Make panic messages red.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic| {
        if let Some(payload) = panic.payload().downcast_ref::<&str>() {
            eprintln!("\n{}\n", payload.red());
        }
        if let Some(payload) = panic.payload().downcast_ref::<String>() {
            eprintln!("\n{}\n", payload.red());
        }
        default_hook(panic);
    }));

    let args = Cli::parse();
    if let Some(path) = &args.working_directory {
        std::env::set_current_dir(path)
            .with_context(|| format!("Failed to change working directory to {}", path.display()))?;
    }
    let res: ExitCode = match args.command {
        Commands::Init(ref init_args) => init(init_args)?,
        Commands::Config(ref config_args) => config(config_args)?,
        Commands::Refilter(ref refilter_args) => refilter(refilter_args)?,
        Commands::Fetch(ref fetch_args) => fetch(fetch_args)?,
        Commands::Push => todo!(),
        Commands::Dump(ref dump_args) => dump(dump_args)?,
        Commands::Replace(ref _replace_args) => todo!(), //replace(&args, replace_args)?,
    };
    Ok(res)
}
