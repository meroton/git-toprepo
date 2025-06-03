mod cli;

use crate::cli::Cli;
use crate::cli::Commands;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use bstr::ByteSlice as _;
use clap::Parser;
use colored::Colorize;
use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::git_command;
use git_toprepo::log::Logger;
use git_toprepo::repo::MonoRepoProcessor;
use git_toprepo::repo_name::RepoName;
use git_toprepo::util::CommandExtension as _;
use gix::refs::FullName;
use itertools::Itertools as _;
use std::io::Read;
use std::num::NonZeroUsize;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

fn init(init_args: &cli::Init) -> Result<PathBuf> {
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

    git_toprepo::repo::TopRepo::create(&directory, url)?;
    eprintln!("Initialized git-toprepo in {}", directory.display());
    Ok(directory)
}

fn clone_after_init(
    clone_args: &cli::Clone,
    processor: &mut MonoRepoProcessor,
    logger: &Logger,
) -> Result<ExitCode> {
    fetch(
        &cli::Fetch {
            keep_going: false,
            jobs: std::num::NonZero::new(1).unwrap(),
            skip_filter: clone_args.minimal,
            remote: None,
            path: None,
            refspecs: None,
        },
        processor,
        logger,
    )?;
    git_command(Path::new("."))
        .args(["checkout", "refs/remotes/origin/HEAD"])
        .check_success_with_stderr()?;
    Ok(ExitCode::SUCCESS)
}

fn load_config_from_file(file: &Path) -> Result<GitTopRepoConfig> {
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
}

fn config(config_args: &cli::Config) -> Result<ExitCode> {
    let repo_dir = Path::new("");
    match &config_args.config_command {
        cli::ConfigCommands::Location => {
            let location = config::GitTopRepoConfig::find_configuration_location(repo_dir)?;
            if let Err(err) = location.validate_existence(repo_dir) {
                git_toprepo::log::eprint_warning(&format!("{err:#}"));
            }
            println!("{location}");
        }
        cli::ConfigCommands::Show => {
            let config = config::GitTopRepoConfig::load_config_from_repo(repo_dir)?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Bootstrap => {
            let config = config_bootstrap()?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Normalize(args) => {
            let config = load_config_from_file(args.file.as_path())?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Validate(validation) => {
            let _config = load_config_from_file(validation.file.as_path())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn config_bootstrap() -> Result<GitTopRepoConfig> {
    let gix_repo = gix::open(PathBuf::from("."))?;
    let head_commit = gix_repo
        .find_reference(&FullName::try_from(RepoName::Top.to_ref_prefix() + "HEAD")?)?
        .peel_to_commit()?;
    let dot_gitmodules_bytes = match head_commit.tree()?.find_entry(".gitmodules") {
        Some(entry) => &entry.object()?.data,
        None => &Vec::new(),
    };
    let gitmod_infos = GitModulesInfo::parse_dot_gitmodules_bytes(
        dot_gitmodules_bytes,
        PathBuf::from(".gitmodules"),
    )?;
    let mut config = GitTopRepoConfig::default();

    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(true);
    let mut log_config = config.log.clone();
    let mut top_repo_cache = git_toprepo::repo::TopRepoCache::default();

    // Resolve borrowing issues.
    let gix_repo = gix_repo.clone();

    git_toprepo::log::log_task_to_stderr(
        error_mode.clone(),
        &mut log_config,
        |logger, progress| {
            (|| -> Result<()> {
                let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                    gix_repo,
                    &mut top_repo_cache.repos,
                    &mut config,
                    progress.clone(),
                    logger.clone(),
                    error_mode.interrupted(),
                    threadpool::ThreadPool::new(1),
                )?;
                commit_loader.fetch_missing_commits = false;
                commit_loader.load_repo(git_toprepo::repo_name::RepoName::Top)?;
                commit_loader.join();
                Ok(())
            })()
            .context("Failed to load the top repo")?;

            // Go through submodules at HEAD and enable them in the config.
            let top_repo_data = top_repo_cache
                .repos
                .get(&RepoName::Top)
                .expect("top repo has been loaded");
            let thin_head_commit = top_repo_data
                .thin_commits
                .get(&head_commit.id)
                .with_context(|| {
                    format!("Missing the HEAD commit {} in the top repo", head_commit.id)
                })?;
            for submod_path in &*thin_head_commit.submodule_paths {
                let Some(submod_url) = gitmod_infos.submodules.get(submod_path) else {
                    logger.warning(format!("Missing submodule {submod_path} in .gitmodules"));
                    continue;
                };
                let submod_url = match submod_url {
                    Ok(submod_url) => submod_url,
                    Err(err) => {
                        logger.warning(format!(
                            "Invalid submodule URL for path {submod_path}: {err}"
                        ));
                        continue;
                    }
                };
                // TODO: Refactor to not use missing_subrepos.clear() for
                // accessing the submodule configs.
                config.missing_subrepos.clear();
                match config
                    .get_name_from_url(submod_url)
                    .with_context(|| format!("Submodule {submod_path}"))
                {
                    Ok(Some(name)) => {
                        config
                            .subrepos
                            .get_mut(&name)
                            .expect("valid subrepo name")
                            .enabled = true
                    }
                    Ok(None) => unreachable!("Submodule {submod_path} should be in the config"),
                    Err(err) => {
                        logger.warning(format!("Failed to load submodule {submod_path}: {err}"));
                        continue;
                    }
                }
            }
            Ok(())
        },
    )?;
    // Skip printing the warnings in the initial configuration.
    // config.log = log_config;
    Ok(config)
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

fn refilter(
    refilter_args: &cli::Refilter,
    processor: &mut MonoRepoProcessor,
    logger: &Logger,
) -> Result<ExitCode> {
    load_commits(
        refilter_args.jobs.into(),
        |commit_loader| {
            commit_loader.fetch_missing_commits = !refilter_args.no_fetch;
            commit_loader.load_repo(git_toprepo::repo_name::RepoName::Top)
        },
        processor,
        logger,
    )?;
    let top_refs = processor
        .gix_repo
        .to_thread_local()
        .references()?
        .prefixed(RepoName::Top.to_ref_prefix().as_bytes())?
        .map(|r| r.map_err(|err| anyhow::anyhow!("Bad ref: {err}")))
        .map_ok(|r| r.detach().name)
        .collect::<Result<Vec<_>>>()?;
    processor
        .expand_toprepo_refs(&top_refs, logger)
        .map(|_| ExitCode::SUCCESS)
}

fn fetch(
    fetch_args: &cli::Fetch,
    processor: &mut MonoRepoProcessor,
    logger: &Logger,
) -> Result<ExitCode> {
    let repo = processor.gix_repo.to_thread_local();
    let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(&repo);

    let Some(refspecs) = &fetch_args.refspecs else {
        // Fetch without a refspec.
        if fetch_args.path.is_some() {
            anyhow::bail!("Cannot use --path without specifying a refspec");
        }
        if let Some(remote) = &fetch_args.remote
            && repo.remote_names().contains(BStr::new(remote))
        {
            anyhow::bail!(
                "The git-remote name {remote} does not exist and no refspecs were provided."
            );
        }
        fetcher.remote = fetch_args.remote.clone();
        fetcher.fetch_on_terminal()?;

        if !fetch_args.skip_filter {
            processor.reload_config()?;
            refilter(
                &cli::Refilter {
                    keep_going: fetch_args.keep_going,
                    jobs: fetch_args.jobs,
                    no_fetch: false,
                },
                processor,
                logger,
            )?;
        }
        return Ok(ExitCode::SUCCESS);
    };

    // Fetch a specific refspec.
    let resolved_args = fetch_args.resolve_remote_and_path(&repo, &processor.config)?;
    fetcher.remote = Some(
        resolved_args
            .url
            .to_bstring()
            .to_str()
            .context("Bad UTF-8 defualt remote URL")?
            .to_owned(),
    );
    let ref_prefix = resolved_args.repo.to_ref_prefix();
    fetcher.refspecs = refspecs
        .iter()
        .map(|(remote_ref, mono_ref)| format!("{remote_ref}:{ref_prefix}{mono_ref}"))
        .collect_vec();
    fetcher.fetch_on_terminal()?;
    // Stop early?
    if fetch_args.skip_filter {
        return Ok(ExitCode::SUCCESS);
    }
    processor.reload_config()?;

    load_commits(
        fetch_args.jobs.into(),
        |commit_loader| commit_loader.load_repo(resolved_args.repo.clone()),
        processor,
        logger,
    )?;
    if processor
        .interrupted
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Ok(ExitCode::FAILURE);
    }

    match &resolved_args.repo {
        RepoName::Top => {
            let top_refs: Vec<FullName> = refspecs
                .iter()
                .map(|(_, mono_ref)| {
                    FullName::try_from(format!("{ref_prefix}{mono_ref}"))
                        .with_context(|| format!("Bad URL {ref_prefix}{mono_ref}"))
                })
                .collect::<Result<Vec<_>>>()?;
            processor.expand_toprepo_refs(&top_refs, logger)?;
        }
        RepoName::SubRepo(sub_repo_name) => {
            for (_, mono_ref) in refspecs {
                if processor
                    .interrupted
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    break;
                }
                let submod_ref = format!("{ref_prefix}{mono_ref}");
                let submod_ref = FullName::try_from(submod_ref)?;
                let mono_ref = FullName::try_from(mono_ref.clone())?;
                match processor.expand_submodule_ref_onto_head(
                    submod_ref.as_ref(),
                    sub_repo_name,
                    &resolved_args.path,
                    mono_ref.as_ref(),
                    logger,
                ) {
                    Ok(()) => {}
                    Err(err) => {
                        logger.error(format!("Failed to expand {submod_ref}: {err:#}"));
                    }
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load_commits<F>(
    job_count: NonZeroUsize,
    commit_loader_setup: F,
    processor: &mut MonoRepoProcessor,
    logger: &Logger,
) -> Result<()>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    let mut commit_loader = git_toprepo::loader::CommitLoader::new(
        processor.gix_repo.to_thread_local(),
        &mut processor.top_repo_cache.repos,
        &mut processor.config,
        processor.progress.clone(),
        logger.clone(),
        processor.interrupted.clone(),
        threadpool::ThreadPool::new(job_count.get()),
    )?;
    commit_loader_setup(&mut commit_loader).with_context(|| "Failed to setup the commit loader")?;
    commit_loader.join();
    Ok(())
}

fn push(
    push_args: &cli::Push,
    processor: &mut MonoRepoProcessor,
    logger: &Logger,
) -> Result<ExitCode> {
    let repo = processor.gix_repo.to_thread_local();
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
    let refspecs = push_args.refspecs.as_slice();
    let [(local_ref, remote_ref)] = refspecs else {
        unimplemented!("Handle multiple refspecs");
    };
    // TODO: This assumes a single ref in the refspec. What about patterns?
    let local_rev = local_ref;

    processor
        .push(
            &base_url,
            local_rev,
            &FullName::try_from(remote_ref.clone())?,
            push_args.dry_run,
            logger,
        )
        .map(|_| ExitCode::SUCCESS)
}

fn dump(dump_args: &cli::Dump) -> Result<ExitCode> {
    match dump_args {
        cli::Dump::ImportCache => dump_import_cache(),
    }
}

fn dump_import_cache() -> Result<ExitCode> {
    let toprepo = gix::open("")?;

    let serde_repo_states = git_toprepo::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
        toprepo.git_dir(),
        None,
        git_toprepo::log::eprint_warning,
    )?;
    serde_repo_states.dump_as_json(std::io::stdout())?;
    Ok(ExitCode::SUCCESS)
}

/// Print the version of the git-toprepo to stdout.
fn print_version() -> Result<()> {
    println!(
        "{} {}~{}-{}",
        env!("CARGO_PKG_NAME"),
        option_env!("BUILD_SCM_TAG").unwrap_or("0.0.0"),
        option_env!("BUILD_SCM_TIMESTAMP").unwrap_or("timestamp"),
        option_env!("BUILD_SCM_REVISION").unwrap_or("git-hash"),
    );
    Ok(())
}

fn main_impl<I>(argv: I) -> Result<ExitCode>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let args = Cli::parse_from(argv);
    if let Some(path) = &args.working_directory {
        std::env::set_current_dir(path)
            .with_context(|| format!("Failed to change working directory to {}", path.display()))?;
    }

    // First run subcommands that can run with a mis- or unconfigured repo.
    match &args.command {
        Commands::Init(init_args) => return init(init_args).map(|_| ExitCode::SUCCESS),
        Commands::Clone(cli::Clone {
            init: init_args,
            minimal: _,
        }) => {
            let directory = init(init_args)?;
            std::env::set_current_dir(&directory).with_context(|| {
                format!(
                    "Failed to change working directory to {}",
                    directory.display()
                )
            })?;
        }
        Commands::Config(config_args) => return config(config_args),
        Commands::Dump(dump_args) => return dump(dump_args),
        Commands::Version => return print_version().map(|_| ExitCode::SUCCESS),
        _ => {
            if args.working_directory.is_none() {
                let current_dir = std::env::current_dir()?;
                let working_directory = git_toprepo::util::find_working_directory(&current_dir)?;
                std::env::set_current_dir(&working_directory).with_context(|| {
                    format!(
                        "Failed to change working directory to {}",
                        &working_directory.display()
                    )
                })?;
            }
        }
    }
    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(match &args.command {
        Commands::Refilter(refilter_args) => refilter_args.keep_going,
        Commands::Fetch(fetch_args) => fetch_args.keep_going,
        _ => false,
    });
    git_toprepo::repo::MonoRepoProcessor::run(Path::new("."), error_mode, |processor, logger| {
        match args.command {
            Commands::Init(_) => unreachable!("init already processed"),
            Commands::Clone(clone_args) => clone_after_init(&clone_args, processor, logger),
            Commands::Config(_) => unreachable!("config already processed"),
            Commands::Refilter(refilter_args) => refilter(&refilter_args, processor, logger),
            Commands::Fetch(fetch_args) => fetch(&fetch_args, processor, logger),
            Commands::Push(push_args) => push(&push_args, processor, logger),
            Commands::Dump(_) => unreachable!("dump already processed"),
            Commands::Replace(_replace_args) => todo!(), //replace(&args, replace_args)?,
            Commands::Version => unreachable!("version already processed"),
        }
    })
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

    match main_impl(std::env::args_os()) {
        Ok(exit_code) => exit_code,
        Err(err) => {
            eprintln!("{}: {:#}", "ERROR".red().bold(), err);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_outside_git_toprepo() {
        let temp_dir = tempfile::TempDir::with_prefix("git-toprepo-").unwrap();
        // Debug with &temp_dir.into_path() to persist the path.
        let temp_dir = temp_dir.path();
        let temp_dir_str = temp_dir.to_str().unwrap();
        let argv = vec!["git-toprepo", "-C", temp_dir_str, "config", "show"];
        let argv = argv.into_iter().map(|s| s.into());
        assert_eq!(
            format!("{:#}", main_impl(argv).unwrap_err()),
            "git-config 'toprepo.config' is missing. Is this an initialized git-toprepo?"
        );
    }
}
