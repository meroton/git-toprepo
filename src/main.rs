#![allow(dead_code)]

/// TODO(nils): better error handling for service authentication/login flow
/// missing .netrc file?
/// should give instructions for the required manual steps.
/// or generate a new netrc file following Albin's suggestions with
/// XDG_RUNTIME_DIR.

mod cli;

use crate::cli::Cli;
use crate::cli::Commands;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use bstr::ByteSlice as _;
use bstr::BString;
use bstr::ByteVec;

use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::gitreview::parse_git_review;
use git_toprepo::submitted_together::order_submitted_together;

use git_gr_lib::git::Git;
use git_gr_lib::query::QueryOptions;
use git_gr_lib::gerrit::HTTPPasswordPolicy;

use clap::Parser;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::collections::HashMap;
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

/// Checkout topics from Gerrit.
fn checkout(_: &Cli, checkout: &cli::Checkout) -> Result<ExitCode> {
    if ! checkout.dry_run{
        assert!(false, "only --dry-run is supported.");
    }

    // TODO(nils): path?
    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::from("."))?;

    let mut git_review_file = toprepo.directory.clone();
    git_review_file.push(".gitreview");

    // TODO: Promote to a CLI argument,
    // and parse .gitreview for defaults instead of this!
    // It is in fact load bearing with the hacky git-gr overrides.
    let mut http_server_override = None;

    if git_review_file.exists() {
        let mut content: String = "".to_owned();
        File::open(git_review_file)
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        let git_review = parse_git_review(&content)?;
        http_server_override = Some(git_review.host);
    }

    // let parsed_remote = git_gr_lib::gerrit_project::parse_remote_url(&checkout.remote).unwrap();
    // TODO: How should we ask for the username, or autodetect it?
    // It is often missing from the remote! We could rely on `.gitreview`.
    // let username_override = parsed_remote.username;

    // DEBUG: make it work:
    let username_override = Some("nwirekli".to_owned());

    assert!(username_override.is_some(), "Username must be overridden, git-gr can't find it");
    assert!(http_server_override.is_some(), "http server must be overridden, git-gr can't find it");
    let git = Git::new();
    let gerrit = git.gerrit(
        /* gerrit remote name */ None,
        username_override,
        http_server_override,
        HTTPPasswordPolicy::Netrc,
        /* cache: */ true,
        /* persist ssh: */ false,
    );

    let mut gerrit = match gerrit {
        Ok(g) => g,
        Err(error) => {
            let box_dyn = Box::<dyn std::error::Error + Send + Sync>::from(error);
            return Err(anyhow!(box_dyn.to_string()));
        }
    };

    // refs/changes/32/261932/5
    // 1    2       3  ^^^^^^ 5
    let change = match checkout.change.split("/").collect::<Vec<&str>>()[..] {
        [_, _, _, c, _] => c.to_owned(),
        _ => { return Err(anyhow!("Could not parse change {:?} for its change number", checkout.change)) }
    };

    let res = gerrit.query(
        QueryOptions::new(change)
        .current_patch_set()
        .dependencies()
    );
    let res = match res {
        Ok(r) => r,
        Err(error) => {
            let box_dyn = Box::<dyn std::error::Error + Send + Sync>::from(error);
            return Err(anyhow!(box_dyn.to_string()));
        }
    };

    let triplet_id = res.changes[0].triplet_id();
    let res = gerrit.get_submitted_together(&triplet_id);
    let res = res.map_err(|e| anyhow::Error::from_boxed(e.into()))
        .context("Could not query Gerrit's REST API for changes submitted together")?;

    let res = order_submitted_together(res)?;

    println!("# # Cherry-pick order:");
    let fetch_stem = "git toprepo fetch";
    for (index, atomic) in res.into_iter().rev().enumerate() {
        for repo in atomic.into_iter() {
            for commit in repo.into_iter().rev() {
                let remote = format!("ssh://{}/{}.git", gerrit.ssh_host(), commit.project);
                let cherry_pick = "&& git cherry-pick refs/toprepo/fetch-head";
                if let Some(subject) = commit.subject {
                    println!("# {subject}");
                }
                println!("{fetch_stem} {remote} {} {cherry_pick} # topic index: {index}", commit.current_revision.unwrap());
            }
        }
    }

    Ok(0.into())
}

/// Replace references to Gerrit projects to the local file paths of submodules.
fn replace(_: &Cli, replace: &cli::Replace) -> Result<ExitCode> {
    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    struct Mod {
        // TODO: use regular String
        project: BString,
        // TODO: use regular PathBuf
        path: BString,
    }
    // TODO(nils): path?
    let toprepo = git_toprepo::repo::TopRepo::open(PathBuf::from("."))?;
    let main_project = toprepo.gerrit_project();
    let mut modules: Vec<Mod> = toprepo.submodules()?.subprojects.into_iter()
        .map(|(path, project)| Mod{project: project.into(), path: path.0}).collect();

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

fn refilter(args: &cli::Refilter) -> Result<ExitCode> {
    fetch_and_refilter(
        &cli::Fetch {
            fetch_submodules: false,
            keep_going: args.keep_going,
            jobs: args.jobs,
            skip_filter: false,
            repo: None,
            super_or_submodule_remote: "origin".to_owned(),
            refspecs: None,
        },
        |commit_loader| {
            commit_loader.fetch_missing_commits = false;
            commit_loader.load_all_repos()
        },
    )
}
fn fetch(fetch_args: &cli::Fetch) -> Result<ExitCode> {
    fetch_and_refilter(fetch_args, |commit_loader| {
        commit_loader.load_after_fetch = !fetch_args.skip_filter;
        // TODO: refspecs
        commit_loader.fetch_repo(git_toprepo::repo::RepoName::Top, vec![None]);
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
    let config = git_toprepo::config::GitTopRepoConfig::load_config_from_repo(&toprepo.directory)?;
    let error_mode = git_toprepo::log::ErrorMode::from_keep_going_flag(fetch_args.keep_going);
    let submodule_names = config.subrepos.keys().cloned().collect::<Vec<String>>();
    let (repo_states, config) =
        git_toprepo::log::log_task_to_stderr(error_mode.clone(), |logger, progress| {
            let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                toprepo.gix_repo.to_thread_local(),
                config,
                progress,
                logger,
                error_mode.interrupted(),
                threadpool::ThreadPool::new(fetch_args.jobs.get() as usize),
            );
            commit_loader_setup(&mut commit_loader)?;
            if fetch_args.fetch_submodules {
                for subrepo_name in submodule_names {
                    commit_loader.fetch_repo(
                        git_toprepo::repo::RepoName::SubRepo(git_toprepo::repo::SubRepoName::new(
                            subrepo_name,
                        )),
                        vec![None],
                    );
                }
            }
            commit_loader.join();
            Ok(commit_loader.into_result())
        })
        .context("Failed to fetch")?;

    if fetch_args.skip_filter {
        return Ok(ExitCode::SUCCESS);
    }
    let storage = git_toprepo::repo::TopRepoCache {
        repos: repo_states,
        monorepo_commits: HashMap::new(),
        expanded_commits: HashMap::new(),
    };
    git_toprepo::log::log_task_to_stderr(error_mode, |logger, progress| {
        toprepo.refilter(storage, &config, logger.clone(), progress)
    })
    .map_err(|_| anyhow::anyhow!("Failed to filter"))?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> Result<ExitCode> {

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
        Commands::Replace(ref replace_args) => replace(&args, replace_args)?,
        Commands::Checkout(ref checkout_args) => checkout(&args, checkout_args)?,
    };
    Ok(res)
}
