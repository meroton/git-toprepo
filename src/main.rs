mod cli;

use crate::cli::Cli;
use crate::cli::Commands;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use bstr::BStr;
use bstr::ByteSlice as _;
use clap::Parser;
use colored::Colorize;
use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::git_command;
use git_toprepo::repo;
use git_toprepo::repo::MonoRepoProcessor;
use git_toprepo::repo_name::RepoName;
use git_toprepo::util::CommandExtension as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools as _;
use std::io::Read;
use std::num::NonZeroUsize;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

fn init(init_args: &cli::Init) -> Result<PathBuf> {
    let mut url = gix::url::Url::from_bytes(init_args.repository.as_bytes().as_bstr())?;
    // git-clone converts paths URLs to absolute paths.
    url.canonicalize(&std::env::current_dir()?)
        .with_context(|| format!("Failed to canonicalize URL {url}"))?;

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
    log::info!("Initialized git-toprepo in {}", directory.display());
    Ok(directory)
}

fn clone_after_init(clone_args: &cli::Clone, processor: &mut MonoRepoProcessor) -> Result<()> {
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
    )?;
    git_command(Path::new("."))
        .args(["checkout", "refs/remotes/origin/HEAD", "--"])
        .check_success_with_stderr()?;
    Ok(())
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

fn config(config_args: &cli::Config) -> Result<()> {
    let repo_dir = Path::new("");
    match &config_args.config_command {
        cli::ConfigCommands::Location => {
            let location = config::GitTopRepoConfig::find_configuration_location(repo_dir)?;
            if let Err(err) = location.validate_existence(repo_dir) {
                log::warn!("{err:#}");
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
    Ok(())
}

fn config_bootstrap() -> Result<GitTopRepoConfig> {
    let gix_repo = gix::open(PathBuf::from("."))
        .context(repo::COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY)?;

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

    let mut top_repo_cache = git_toprepo::repo::TopRepoCache::default();

    // Resolve borrowing issues.
    let gix_repo = gix_repo.clone();

    git_toprepo::log::get_global_logger().with_progress(|progress| {
        (|| -> Result<()> {
            let error_observer =
                git_toprepo::log::ErrorObserver::new(git_toprepo::log::ErrorMode::KeepGoing);
            let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                gix_repo,
                &mut top_repo_cache.repos,
                &mut config,
                progress.clone(),
                error_observer.clone(),
                threadpool::ThreadPool::new(1),
            )?;
            commit_loader.fetch_missing_commits = false;
            commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)?;
            commit_loader.join()?;
            error_observer.get_result(())
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
                log::warn!("Missing submodule {submod_path} in .gitmodules");
                continue;
            };
            let submod_url = match submod_url {
                Ok(submod_url) => submod_url,
                Err(err) => {
                    log::warn!("Invalid submodule URL for path {submod_path}: {err}");
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
                    log::warn!("Failed to load submodule {submod_path}: {err}");
                    continue;
                }
            }
        }
        Ok(())
    })?;
    // Skip printing the warnings in the initial configuration.
    // config.log = log_config;
    Ok(config)
}
/*
/// Replace references to Gerrit projects to the local file paths of submodules.
fn replace(args: &Cli, replace: &cli::Replace) -> Result<()> {
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
        return Ok(())
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

    Ok(())
}
*/

fn refilter(refilter_args: &cli::Refilter, processor: &mut MonoRepoProcessor) -> Result<()> {
    load_commits(
        refilter_args.jobs.into(),
        |commit_loader| {
            commit_loader.fetch_missing_commits = !refilter_args.no_fetch;
            commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)
        },
        processor,
    )?;
    processor.refilter_all_top_refs()
}

fn fetch(fetch_args: &cli::Fetch, processor: &mut MonoRepoProcessor) -> Result<()> {
    if let Some(refspecs) = &fetch_args.refspecs {
        let repo = processor.gix_repo.to_thread_local();
        let resolved_args = fetch_args.resolve_remote_and_path(&repo, &processor.config)?;
        let detailed_refspecs = detail_refspecs(refspecs, &resolved_args.repo, &resolved_args.url)?;
        let mut result =
            fetch_with_refspec(fetch_args, resolved_args, &detailed_refspecs, processor);
        // Delete temporary refs.
        let mut ref_edits = Vec::new();
        for refspec in detailed_refspecs {
            ref_edits.push(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Delete {
                    expected: gix::refs::transaction::PreviousValue::Any,
                    log: gix::refs::transaction::RefLog::AndReference,
                },
                name: refspec.unfiltered_ref,
                deref: false,
            });
            match refspec.destination {
                FetchDestinationRef::Normal(_normal_ref) => {}
                FetchDestinationRef::FetchHead { filtered_ref, .. } => {
                    // Special case for FETCH_HEAD.
                    ref_edits.push(gix::refs::transaction::RefEdit {
                        change: gix::refs::transaction::Change::Delete {
                            expected: gix::refs::transaction::PreviousValue::Any,
                            log: gix::refs::transaction::RefLog::AndReference,
                        },
                        name: filtered_ref,
                        deref: false,
                    });
                }
            }
        }
        if !ref_edits.is_empty() {
            let committer = gix::actor::SignatureRef {
                name: "git-toprepo".as_bytes().as_bstr(),
                email: BStr::new(""),
                time: &gix::date::Time::now_local_or_utc().format(gix::date::time::Format::Raw),
            };
            if let Err(err) = repo
                .edit_references_as(ref_edits, Some(committer))
                .context("Failed to update all the mono references")
                && result.is_ok()
            {
                result = Err(err);
            }
        }
        result
    } else {
        fetch_with_default_refspecs(fetch_args, processor)
    }
}

fn fetch_with_default_refspecs(
    fetch_args: &cli::Fetch,
    processor: &mut MonoRepoProcessor,
) -> Result<()> {
    let repo = processor.gix_repo.to_thread_local();
    let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(&repo);

    // Fetch without a refspec.
    if fetch_args.path.is_some() {
        anyhow::bail!("Cannot use --path without specifying a refspec");
    }
    let remote_names = repo.remote_names();
    if let Some(remote) = &fetch_args.remote
        && !remote_names.contains(BStr::new(remote))
    {
        let remote_names_str = remote_names
            .iter()
            .map(|name| format!("{:?}", name.to_str_lossy()))
            .join(", ");
        anyhow::bail!(
            "Failed to fetch: \
            The git-remote {remote:?} was not found among {remote_names_str}.\n\
            When no refspecs are provided, a name among `git remote -v` must be specified."
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
        )?;
    }
    Ok(())
}

#[derive(Debug)]
enum FetchDestinationRef {
    Normal(FullName),
    /// Special case for `FETCH_HEAD` ref. The lines in `.git/FETCH_HEAD` file looks like
    /// `123abc<TAB>not-for-merge<TAB>branch 'algo' of https://example.com/repo.git`.
    FetchHead {
        /// Name of the temporary filter result.
        filtered_ref: FullName,
        /// Description to add to `.git/FETCH_HEAD`.
        description_suffix: String,
    },
}

impl FetchDestinationRef {
    pub fn get_filtered_ref(&self) -> &FullNameRef {
        match self {
            FetchDestinationRef::Normal(refname) => refname.as_ref(),
            FetchDestinationRef::FetchHead { filtered_ref, .. } => filtered_ref.as_ref(),
        }
    }
}

#[derive(Debug)]
struct DetailedFetchRefspec {
    /// Whether the refspec sets force-fetch (starts with `+`).
    #[allow(unused)] // Currently unused.
    force: bool,
    remote_ref: String,
    unfiltered_ref: FullName,
    destination: FetchDestinationRef,
}

fn detail_refspecs(
    refspecs: &[(String, String)],
    repo_name: &RepoName,
    url: &gix::Url,
) -> Result<Vec<DetailedFetchRefspec>> {
    let ref_prefix = repo_name.to_ref_prefix();
    refspecs
        .iter()
        .enumerate()
        .map(|(idx, (remote_ref, local_ref))| {
            let (remote_ref, force) = remote_ref
                .strip_prefix('+')
                .map(|stripped| (stripped, true))
                .unwrap_or((remote_ref, false));
            if local_ref == "FETCH_HEAD" {
                // This is a special case for FETCH_HEAD.
                let filtered_ref = FullName::try_from(format!("refs/fetch-heads/{idx}")).unwrap();
                let unfiltered_ref =
                    FullName::try_from(format!("{ref_prefix}{filtered_ref}")).unwrap();
                Ok(DetailedFetchRefspec {
                    force,
                    remote_ref: remote_ref.to_owned(),
                    unfiltered_ref,
                    destination: FetchDestinationRef::FetchHead {
                        filtered_ref,
                        description_suffix: format!("\t\t{remote_ref} of {url}"),
                    },
                })
            } else {
                let local_ref = FullName::try_from(local_ref.as_bytes().as_bstr())
                    .with_context(|| format!("Bad local ref {local_ref}"))?;
                let unfiltered_ref =
                    FullName::try_from(format!("{ref_prefix}{local_ref}")).unwrap();
                Ok(DetailedFetchRefspec {
                    force,
                    remote_ref: remote_ref.to_owned(),
                    unfiltered_ref,
                    destination: FetchDestinationRef::Normal(local_ref),
                })
            }
        })
        .collect::<Result<Vec<_>>>()
}

fn fetch_with_refspec(
    fetch_args: &cli::Fetch,
    resolved_args: cli::ResolvedFetchParams,
    detailed_refspecs: &Vec<DetailedFetchRefspec>,
    processor: &mut MonoRepoProcessor,
) -> Result<()> {
    let repo = processor.gix_repo.to_thread_local();
    let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(&repo);

    fetcher.remote = Some(
        resolved_args
            .url
            .to_bstring()
            .to_str()
            .context("Bad UTF-8 defualt remote URL")?
            .to_owned(),
    );
    // TODO: Should the + force be possible to remove?
    fetcher.refspecs = detailed_refspecs
        .iter()
        .map(|refspec| format!("+{}:{}", refspec.remote_ref, refspec.unfiltered_ref))
        .collect_vec();
    fetcher.fetch_on_terminal()?;
    // Stop early?
    if fetch_args.skip_filter {
        return Ok(());
    }
    processor.reload_config()?;

    load_commits(
        fetch_args.jobs.into(),
        |commit_loader| commit_loader.load_repo(&resolved_args.repo),
        processor,
    )?;

    match &resolved_args.repo {
        RepoName::Top => {
            let top_refs = detailed_refspecs
                .iter()
                .map(|refspec| &refspec.unfiltered_ref);
            processor.refilter_some_top_refspecs(top_refs)?;
        }
        RepoName::SubRepo(sub_repo_name) => {
            for refspec in detailed_refspecs {
                if processor.error_observer.should_interrupt() {
                    bail!("Aborting due to previous errors");
                }
                // TODO: Reuse the git-fast-import process for all refspecs.
                let dest_ref = refspec.destination.get_filtered_ref();
                if let Err(err) = processor.expand_submodule_ref_onto_head(
                    refspec.unfiltered_ref.as_ref(),
                    sub_repo_name,
                    &resolved_args.path,
                    dest_ref,
                ) {
                    log::error!("Failed to expand {}: {err:#}", refspec.remote_ref);
                }
            }
        }
    }

    // Update .git/FETCH_HEAD.
    let mut fetch_head_lines = Vec::new();
    for refspec in detailed_refspecs {
        match &refspec.destination {
            FetchDestinationRef::Normal(_normal_ref) => {
                // Normal ref is written by git-fast-import.
            }
            FetchDestinationRef::FetchHead {
                filtered_ref,
                description_suffix,
            } => {
                // Special case for FETCH_HEAD.
                let mut r = repo
                    .find_reference(filtered_ref.as_ref())
                    .with_context(|| {
                        format!("Failed to find filtered ref {}", filtered_ref.as_bstr())
                    })?;
                let r = r.follow_to_object().with_context(|| {
                    format!(
                        "Failed to follow filtered ref {} to commit or tag",
                        filtered_ref.as_bstr()
                    )
                })?;
                let mono_object_id = r
                    .object()
                    .with_context(|| {
                        format!(
                            "Failed to get the object id for filtered ref {}",
                            filtered_ref.as_bstr()
                        )
                    })?
                    .id;
                fetch_head_lines.push(format!("{}{description_suffix}\n", mono_object_id.to_hex()));
            }
        }
    }
    if !fetch_head_lines.is_empty() {
        // Update .git/FETCH_HEAD.
        let fetch_head_path = repo.git_dir().join("FETCH_HEAD");
        std::fs::write(&fetch_head_path, fetch_head_lines.join(""))?;
    }
    Ok(())
}

fn load_commits<F>(
    job_count: NonZeroUsize,
    commit_loader_setup: F,
    processor: &mut MonoRepoProcessor,
) -> Result<()>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    let mut commit_loader = git_toprepo::loader::CommitLoader::new(
        processor.gix_repo.to_thread_local(),
        &mut processor.top_repo_cache.repos,
        &mut processor.config,
        processor.progress.clone(),
        processor.error_observer.clone(),
        threadpool::ThreadPool::new(job_count.get()),
    )?;
    commit_loader_setup(&mut commit_loader).with_context(|| "Failed to setup the commit loader")?;
    commit_loader.join()?;
    if processor.error_observer.has_got_errors() {
        anyhow::bail!("Failed to load commits, see previous errors");
    }
    Ok(())
}

fn push(push_args: &cli::Push, processor: &mut MonoRepoProcessor) -> Result<()> {
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

    processor.push(
        &base_url,
        local_rev,
        &FullName::try_from(remote_ref.clone())?,
        push_args.dry_run,
    )
}

fn dump(dump_args: &cli::Dump) -> Result<()> {
    match dump_args {
        cli::Dump::ImportCache => dump_import_cache(),
    }
}

fn dump_import_cache() -> Result<()> {
    let toprepo = repo::TopRepo::open(&PathBuf::from("."))?;

    let serde_repo_states = git_toprepo::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
        toprepo.gix_repo.git_dir(),
        None,
    )?;
    serde_repo_states.dump_as_json(std::io::stdout())?;
    Ok(())
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

/// Executes `main_fn` with a signal handler that listens for termination
/// signals. When `main_fn` returns or when a termination signal is received,
/// the signal handler is stopped and `shutdown_fn` is called.
fn with_termination_signal_handler<T>(
    main_fn: impl FnOnce() -> Result<T>,
    shutdown_fn: impl FnOnce() + Send,
) -> Result<T> {
    std::thread::scope(|s| {
        let mut signals = signal_hook::iterator::Signals::new(signal_hook::consts::TERM_SIGNALS)
            .context("Failed to register signal handlers")?;
        let signal_handler = signals.handle();
        let signal_handler_clone = signal_handler.clone();
        let thread = std::thread::Builder::new()
            .name("signal-handler".to_owned())
            .spawn_scoped(s, move || {
                let mut signal_iter = signals.forever().peekable();
                if let Some(signal) = signal_iter.peek() {
                    let signal_str = signal_hook::low_level::signal_name(*signal)
                        .map(|name| name.to_owned())
                        .unwrap_or_else(|| signal.to_string());
                    tracing::info!("Received termination signal {signal_str}");
                }
                // Stop listening for signals and run the shutdown function.
                signal_handler_clone.close();
                shutdown_fn();
                // Reraise all signals to ensure they are handled in the default manner.
                for signal in signal_iter {
                    signal_hook::low_level::emulate_default_handler(signal)
                        .expect("emulate default signal handler in the signal handler thread");
                }
            })?;
        // Run the main function.
        let result = main_fn();
        // Shutdown the signal handler.
        signal_handler.close();
        thread.join().unwrap();
        result
    })
}

fn main_impl<I>(argv: I, logger: Option<&git_toprepo::log::GlobalLogger>) -> Result<()>
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
        Commands::Init(init_args) => return init(init_args).map(|_path| ()),
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
        Commands::Version => return print_version(),
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

    // Now when the working directory is set, we can persist the tracing.
    if let Some(logger) = logger {
        logger.write_to_git_dir(gix::open(".")?.git_dir())?;
    }

    git_toprepo::repo::MonoRepoProcessor::run(Path::new("."), error_mode, |processor| {
        match args.command {
            Commands::Init(_) => unreachable!("init already processed"),
            Commands::Clone(clone_args) => clone_after_init(&clone_args, processor),
            Commands::Config(_) => unreachable!("config already processed"),
            Commands::Refilter(refilter_args) => refilter(&refilter_args, processor),
            Commands::Fetch(fetch_args) => fetch(&fetch_args, processor),
            Commands::Push(push_args) => push(&push_args, processor),
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

    let global_logger = git_toprepo::log::init();

    with_termination_signal_handler(
        || match main_impl(std::env::args_os(), Some(global_logger)) {
            Ok(_) => Ok(ExitCode::SUCCESS),
            Err(err) => {
                log::error!("{err:#}");
                Ok(ExitCode::FAILURE)
            }
        },
        || global_logger.finalize(),
    )
    .unwrap_or_else(|err| {
        eprintln!("{}: {err:#}", "ERROR".red().bold());
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_outside_git_toprepo() {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_main_outside_git_toprepo",
        );
        let temp_dir_str = temp_dir.to_str().unwrap();
        let argv = vec!["git-toprepo", "-C", temp_dir_str, "config", "show"];
        let argv = argv.into_iter().map(|s| s.into());
        assert_eq!(
            format!("{:#}", main_impl(argv, None).unwrap_err()),
            "git-config 'toprepo.config' is missing. Is this an initialized git-toprepo?"
        );
    }
}
