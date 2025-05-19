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
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::GitPath;
use git_toprepo::gitmodules::SubmoduleUrlExt as _;
use git_toprepo::loader::FetchParams;
use git_toprepo::repo_name::RepoName;
use gix::refs::FullName;
use itertools::Itertools;
use std::collections::HashSet;
use std::io::Read;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

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
                eprint!("{log}");
            }
            println!("{location}");
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
                eprint!("{log}");
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
            top_or_submodule_remote: "".to_owned(),
            refspecs: None,
        },
        |commit_loader| {
            commit_loader.fetch_missing_commits = !args.no_fetch;
            commit_loader.load_repo(git_toprepo::repo_name::RepoName::Top)
        },
    )
}

fn fetch(fetch_args: &cli::Fetch) -> Result<ExitCode> {
    fetch_and_refilter(fetch_args, |commit_loader| {
        commit_loader.load_after_fetch = !fetch_args.skip_filter;
        Ok(())
    })
}

fn fetch_and_refilter<F>(fetch_args: &cli::Fetch, commit_loader_setup: F) -> Result<ExitCode>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::from("."))?;
    let repo = toprepo.gix_repo.to_thread_local();
    let mut config =
        git_toprepo::config::GitTopRepoConfig::load_config_from_repo(&toprepo.directory)?;

    let base_url = git_toprepo::git::get_default_remote_url(&repo)?;

    let (fetch_repo_name, abs_sub_path, fetch_params) = if fetch_args
        .top_or_submodule_remote
        .is_empty()
        && fetch_args.repo.is_none()
    {
        if fetch_args.refspecs.is_some() {
            anyhow::bail!("Refspecs are not supported unless a remote is specified");
        }
        (RepoName::Top, GitPath::new(b"".into()), None)
    } else {
        let (repo_name, submod_path, fetch_url_str) = if fetch_args.top_or_submodule_remote
            == "origin"
        {
            if let Some(repo_name) = &fetch_args.repo {
                if repo_name != &RepoName::Top {
                    anyhow::bail!(
                        "Expected the 'top' repository, not {repo_name}, for the remote 'origin'"
                    );
                }
            }
            (
                RepoName::Top,
                GitPath::new(b"".into()),
                fetch_args.top_or_submodule_remote.clone(),
            )
        } else {
            let fetch_arg_url = base_url.join(&gix::url::Url::from_bytes(
                fetch_args.top_or_submodule_remote.as_bytes().as_bstr(),
            )?);
            let fetch_url_str = fetch_arg_url.to_string();
            let trimmed_fetch_url = fetch_arg_url.trim_url_path();
            let mut matching_submod_names = HashSet::new();
            let mut matching_submod_path = GitPath::new("".into());
            if trimmed_fetch_url.approx_equal(&base_url.clone().trim_url_path()) {
                matching_submod_names.insert(RepoName::Top);
            }
            let gitmod_infos = GitModulesInfo::parse_dot_gitmodules_in_repo(&repo)?;
            for (submod_path, submod_url) in gitmod_infos.submodules {
                let Ok(submod_url) = submod_url else {
                    continue;
                };
                let full_url = base_url.join(&submod_url).trim_url_path();
                if full_url.approx_equal(&trimmed_fetch_url) {
                    let (name, submod_config) = config.get_or_insert_from_url(&submod_url)?;
                    if !submod_config.enabled {
                        anyhow::bail!("Submodule {name} is disabled in the configuration");
                    }
                    matching_submod_names.insert(RepoName::SubRepo(name));
                    matching_submod_path = submod_path;
                }
            }
            let matching_submod_names = matching_submod_names.into_iter().sorted().collect_vec();
            let repo_name = match matching_submod_names.as_slice() {
                [] => anyhow::bail!(
                    "No submodule matches {}",
                    fetch_args.top_or_submodule_remote
                ),
                [submod_name] => submod_name.clone(),
                [_, ..] => anyhow::bail!(
                    "Multiple submodules match: {}",
                    matching_submod_names
                        .iter()
                        .map(|name| name.to_string())
                        .join(", ")
                ),
            };
            (repo_name, matching_submod_path, fetch_url_str)
        };
        if let Some(refspecs) = &fetch_args.refspecs {
            if refspecs.len() != 1 {
                todo!("Handle multiple refspecs");
            }
            (
                repo_name,
                submod_path,
                Some(FetchParams::Custom {
                    remote: fetch_url_str,
                    refspec: refspecs[0].clone(),
                }),
            )
        } else {
            match repo_name {
                RepoName::Top => (
                    RepoName::Top,
                    GitPath::new(b"".into()),
                    Some(FetchParams::Default),
                ),
                RepoName::SubRepo(_) => {
                    anyhow::bail!("Refspecs are required for submodules");
                }
            }
        }
    };

    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(fetch_args.keep_going);
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
                if let Some(fetch_params) = &fetch_params {
                    commit_loader.fetch_repo(fetch_repo_name.clone(), fetch_params.clone());
                }
                commit_loader.join();
                Ok(())
            })()
            .context("Failed to fetch")?;

            if !fetch_args.skip_filter
                && !error_mode
                    .interrupted()
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                // TODO: This is so ugly.
                if fetch_args.top_or_submodule_remote.is_empty() && fetch_args.repo.is_none() {
                    // Doing refilter, not fetch.
                    top_repo_cache.monorepo_commit_ids.clear();
                    top_repo_cache.monorepo_commits.clear();
                    top_repo_cache.top_to_mono_map.clear();
                }
                match (&fetch_repo_name, &fetch_params) {
                    (
                        RepoName::Top,
                        Some(FetchParams::Custom {
                            remote: _,
                            refspec: (_from_remote_ref, to_local_ref),
                        }),
                    ) => {
                        let top_local_ref =
                            format!("{}{}", &RepoName::Top.to_ref_prefix(), to_local_ref);
                        let top_local_ref = FullName::try_from(top_local_ref)?;
                        toprepo.expand_toprepo_refs(
                            &vec![top_local_ref],
                            &mut top_repo_cache,
                            &config,
                            logger,
                            progress,
                        )?;
                    }
                    (RepoName::Top, None) | (RepoName::Top, Some(FetchParams::Default)) => {
                        toprepo
                            .refilter(&mut top_repo_cache, &config, logger, progress)
                            .map_err(|_| anyhow::anyhow!("Failed to filter"))?;
                    }
                    (RepoName::SubRepo(_sub_repo_name), Some(FetchParams::Default)) => {
                        unreachable!("Submodule fetch requires a refspec");
                    }
                    (
                        RepoName::SubRepo(sub_repo_name),
                        Some(FetchParams::Custom {
                            remote: _,
                            refspec: (_from_remote_ref, to_local_ref),
                        }),
                    ) => {
                        let submod_local_ref =
                            format!("{}{}", &fetch_repo_name.to_ref_prefix(), to_local_ref);
                        let submod_local_ref = FullName::try_from(submod_local_ref)?;
                        let to_local_ref = FullName::try_from(to_local_ref.clone())?;
                        toprepo.expand_submodule_ref_onto_head(
                            submod_local_ref.as_ref(),
                            sub_repo_name,
                            &abs_sub_path,
                            to_local_ref.as_ref(),
                            &mut top_repo_cache,
                            &config,
                            logger,
                            progress,
                        )?;
                    }
                    (RepoName::SubRepo(_sub_repo_name), None) => {
                        unreachable!("Submodule fetch requires a refspec");
                    }
                }
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

fn push(push_args: &cli::Push) -> Result<ExitCode> {
    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::from("."))?;
    let repo = toprepo.gix_repo.to_thread_local();
    let mut config =
        git_toprepo::config::GitTopRepoConfig::load_config_from_repo(&toprepo.directory)?;
    let base_url = match repo.try_find_remote(push_args.top_remote.as_bytes()) {
        Some(Ok(remote)) => remote
            // TODO: Support push URL config.
            .url(gix::remote::Direction::Fetch)
            .with_context(|| format!("Missing push URL for {}", push_args.top_remote))?
            .clone(),
        None => gix::Url::from_bytes(bstr::BStr::new(push_args.top_remote.as_bytes()))
            .with_context(|| format!("Invalid remote URL {}", push_args.top_remote))?,
        Some(Err(err)) => {
            anyhow::bail!("Failed to resolve remote {}: {}", push_args.top_remote, err);
        }
    };
    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(!push_args.fail_fast);
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

    let [(local_ref, remote_ref)] = push_args.refspecs.as_slice() else {
        unimplemented!("Handle multiple refspecs");
    };

    let mut result = git_toprepo::log::log_task_to_stderr(
        error_mode.clone(),
        &mut log_config,
        |logger, progress| {
            toprepo.push(
                &base_url,
                &FullName::try_from(local_ref.clone())?,
                &FullName::try_from(remote_ref.clone())?,
                &mut top_repo_cache,
                &mut config,
                push_args.dry_run,
                logger,
                progress,
            )
        },
    );

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
    result.map(|_| ExitCode::SUCCESS)
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
        |msg| eprintln!("{msg}"),
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

fn main_impl() -> Result<ExitCode> {
    let args = Cli::parse();

    let working_directory = &git_toprepo::util::find_working_directory(args.working_directory)?;
    std::env::set_current_dir(working_directory).with_context(|| {
        format!(
            "Failed to change working directory to {}",
            working_directory.display()
        )
    })?;

    let res: ExitCode = match args.command {
        Commands::Init(ref init_args) => init(init_args)?,
        Commands::Config(ref config_args) => config(config_args)?,
        Commands::Refilter(ref refilter_args) => refilter(refilter_args)?,
        Commands::Fetch(ref fetch_args) => fetch(fetch_args)?,
        Commands::Push(ref push_args) => push(push_args)?,
        Commands::Dump(ref dump_args) => dump(dump_args)?,
        Commands::Replace(ref _replace_args) => todo!(), //replace(&args, replace_args)?,
    };
    Ok(res)
}

fn main() -> ExitCode {
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

    match main_impl() {
        Ok(exit_code) => exit_code,
        Err(err) => {
            eprintln!("{}: {:#}", "ERROR".red().bold(), err);
            ExitCode::FAILURE
        }
    }
}
