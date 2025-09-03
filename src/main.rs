mod cli;

use crate::cli::Cli;
use crate::cli::Commands;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use bstr::BStr;
use bstr::ByteSlice as _;
use clap::Parser;
use colored::Colorize;
use git_gr_lib::gerrit::HTTPPasswordPolicy;
use git_gr_lib::git::Git;
use git_gr_lib::query::QueryOptions;
use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::config::TOPREPO_CONFIG_FILE_KEY;
use git_toprepo::config::toprepo_git_config;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::git_command;
use git_toprepo::git::git_config_get;
use git_toprepo::gitreview::parse_git_review;
use git_toprepo::log::CommandSpanExt as _;
use git_toprepo::log::ErrorMode;
use git_toprepo::log::ErrorObserver;
use git_toprepo::repo;
use git_toprepo::repo::MonoRepoProcessor;
use git_toprepo::repo_name::RepoName;
use git_toprepo::submitted_together::order_submitted_together;
use git_toprepo::util::CommandExtension as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools as _;
use std::env;
use std::fs::File;
use std::io::Read;
use std::num::NonZeroUsize;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::panic::resume_unwind;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, PartialEq)]
struct NotAMonorepo;

impl std::fmt::Display for NotAMonorepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NotAMonorepo")
    }
}
impl std::error::Error for NotAMonorepo {}

#[derive(Debug, PartialEq)]
struct ExitSilently;

impl std::fmt::Display for ExitSilently {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExitSilently")
    }
}
impl std::error::Error for ExitSilently {}

#[tracing::instrument]
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

    if !init_args.force && directory.is_dir() && directory.read_dir()?.next().is_some() {
        anyhow::bail!("Target directory {directory:?} is not empty");
    }
    git_toprepo::repo::TopRepo::create(&directory, url)?;
    log::info!("Initialized git-toprepo in {}", directory.display());
    Ok(directory)
}

#[tracing::instrument(skip(processor))]
fn clone_after_init(clone_args: &cli::Clone, processor: &mut MonoRepoProcessor) -> Result<()> {
    fetch(
        &cli::Fetch {
            keep_going: false,
            job_count: std::num::NonZero::new(1).unwrap(),
            skip_filter: true,
            remote: None,
            path: None,
            refspecs: None,
        },
        processor,
    )?;
    verify_config_existence_after_clone()?;
    if !clone_args.minimal {
        processor.reload_config()?;
        refilter(&clone_args.refilter, processor)?;
    }
    git_command(Path::new("."))
        .args(["checkout", "refs/remotes/origin/HEAD", "--"])
        .trace_command(git_toprepo::command_span!("git checkout"))
        .check_success_with_stderr()?;
    Ok(())
}

#[tracing::instrument(skip_all)]
fn verify_config_existence_after_clone() -> Result<()> {
    let repo_dir = git_toprepo::util::find_current_worktree(Path::new("."))?;
    let location = config::GitTopRepoConfig::find_configuration_location(&repo_dir)?;
    if location.validate_existence(&repo_dir).is_err() {
        // Fetch from the default remote to get all the direct submodules.
        let gix_repo =
            gix::open(&repo_dir).context("Could not open the newly cloned repository")?;
        log::error!("Config file .gittoprepo.toml does not exist in {location}");
        git_toprepo::fetch::RemoteFetcher::new(&gix_repo).fetch_on_terminal()?;
        log::info!(
            "Please run 'git-toprepo config bootstrap' to generate an initial .gittoprepo.toml."
        );
        bail!("Clone failed due to missing config file");
    }
    Ok(())
}

#[tracing::instrument]
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

#[tracing::instrument]
fn config(config_args: &cli::Config, monorepo_root: &Option<PathBuf>) -> Result<()> {
    match &config_args.config_command {
        cli::ConfigCommands::Location => {
            let repo_dir = monorepo_root.as_ref().ok_or(NotAMonorepo)?;
            let location = config::GitTopRepoConfig::find_configuration_location(repo_dir)?;
            if let Err(err) = location.validate_existence(repo_dir) {
                log::warn!("{err:#}");
            }
            println!("{location}");
        }
        cli::ConfigCommands::Show => {
            let repo_dir = monorepo_root.as_ref().ok_or(NotAMonorepo)?;
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
    let default_remote_name = gix_repo
        .remote_default_name(gix::remote::Direction::Fetch)
        .with_context(|| "Failed to get the default remote name")?;
    let bootstrap_ref = FullName::try_from(format!(
        "{}refs/remotes/{default_remote_name}/HEAD",
        RepoName::Top.to_ref_prefix()
    ))?;
    let head_commit = gix_repo.find_reference(&bootstrap_ref)?.peel_to_commit()?;
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

    git_toprepo::log::get_global_logger().with_progress(|progress| {
        ErrorObserver::run(ErrorMode::KeepGoing, |error_observer| {
            let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                &gix_repo,
                &mut top_repo_cache.repos,
                &mut config,
                progress.clone(),
                error_observer,
                threadpool::ThreadPool::new(1),
            )?;
            commit_loader.fetch_missing_commits = false;
            // No point in spamming with warnings when a configuration is missing anyway.
            commit_loader.log_missing_config_warnings = false;
            commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)?;
            commit_loader.join()
        })
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

/// Checkout topics from Gerrit.
fn checkout(_: &Cli, checkout: &cli::Checkout) -> Result<()> {
    if !checkout.dry_run {
        panic!("only --dry-run is supported.");
    }

    // TODO: Promote to a CLI argument,
    // and parse .gitreview for defaults instead of this!
    // It is in fact load bearing with the hacky git-gr overrides.
    let mut http_server_override = None;

    let toprepo = gix::open("")?;
    let mut git_review_file = toprepo.path().to_owned();
    git_review_file.push(".gitreview");

    if git_review_file.exists() {
        let mut content: String = "".to_owned();
        File::open(git_review_file)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let git_review = parse_git_review(&content)?;
        http_server_override = Some(git_review.host);
    }

    let git = Git::new();
    let parsed_remote = git_gr_lib::gerrit_project::parse_remote_url(&checkout.remote).unwrap();
    let username_override = parsed_remote.username;
    let gerrit = git.gerrit(
        None,
        username_override,
        http_server_override,
        HTTPPasswordPolicy::Netrc,
        /* cache: */ true,
        /* persist ssh: */ false,
    );

    // TODO: Is this a full conversion to anyhow errors?
    // It seems that we lose some of the miette context.
    // Notably, where is the inner error?:
    //     Err(  × Override: None
    //     ╰─▶ Could not determine git remote username
    //     )
    //
    // If this fails without the required override we just see:
    //     called `Result::unwrap()` on an `Err` value: Failed to parse Gerrit configuration from Git remotes. Tried to parse these remotes:
    //     • file:///dev/null
    //     • ssh://csp-gerrit-ssh.volvocars.net/csp/hp/super
    // which to its credit shows the remotes it tried
    // but not the inner error.
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
        _ => {
            return Err(anyhow!(
                "Could not parse change {:?} for its change number",
                checkout.change
            ));
        }
    };

    let res = gerrit.query(QueryOptions::new(change).current_patch_set().dependencies());
    let res = match res {
        Ok(r) => r,
        Err(error) => {
            let box_dyn = Box::<dyn std::error::Error + Send + Sync>::from(error);
            return Err(anyhow!(box_dyn.to_string()));
        }
    };
    let triplet_id = res.changes[0].triplet_id();
    let res = gerrit.get_submitted_together(&triplet_id);

    let res = order_submitted_together(res.unwrap())?;

    println!("Cherry-pick order:");
    let fetch_stem = "git toprepo fetch";
    for (index, atomic) in res.into_iter().rev().enumerate() {
        for repo in atomic.into_iter() {
            for commit in repo.into_iter() {
                let remote = format!("ssh://{}/{}.git", gerrit.ssh_host(), commit.project);
                let cherry_pick = "&& git cherry-pick FETCH_HEAD";
                println!(
                    "{fetch_stem} {remote} {} {cherry_pick} # topic index: {index}",
                    commit.current_revision.unwrap()
                );
            }
        }
    }

    Ok(())
}

fn dump_modules(monorepo_root: &Option<PathBuf>) -> Result<()> {
    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    struct Mod {
        project: String,
        path: git_toprepo::git::GitPath,
    }
    let monorepo_root = monorepo_root.as_ref().ok_or(NotAMonorepo)?;
    let toprepo = repo::TopRepo::open(monorepo_root)?;

    let main_project = toprepo
        .gerrit_project()
        .context(git_toprepo::repo::LOADING_THE_MAIN_PROJECT_CONTEXT)?;
    let mut modules: Vec<Mod> = toprepo
        .submodules()?
        .into_iter()
        .map(|(path, project)| Mod { project, path })
        .collect();

    modules.push(Mod {
        project: main_project,
        // TODO: What is the path to the repo? May be upwards.
        path: ".".into(),
    });

    for module in modules {
        println!("{} {}", module.project, module.path);
    }

    Ok(())
}

#[tracing::instrument(skip(processor))]
fn refilter(refilter_args: &cli::Refilter, processor: &mut MonoRepoProcessor) -> Result<()> {
    if !refilter_args.reuse_cache {
        *processor.top_repo_cache = repo::TopRepoCache::default();
    }
    ErrorObserver::run_keep_going(refilter_args.keep_going, |error_observer| {
        load_commits(
            refilter_args.job_count.into(),
            |commit_loader| {
                commit_loader.fetch_missing_commits = !refilter_args.no_fetch;
                commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)
            },
            processor,
            error_observer,
        )
    })?;
    git_toprepo::expander::refilter_all_top_refs(processor)
}

#[tracing::instrument(skip(processor))]
fn fetch(fetch_args: &cli::Fetch, processor: &mut MonoRepoProcessor) -> Result<()> {
    if let Some(refspecs) = &fetch_args.refspecs {
        let resolved_args =
            cli::resolve_remote_and_path(fetch_args, processor.gix_repo, processor.config)?;
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
            if let Err(err) = processor
                .gix_repo
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

#[tracing::instrument(skip(processor))]
fn fetch_with_default_refspecs(
    fetch_args: &cli::Fetch,
    processor: &mut MonoRepoProcessor,
) -> Result<()> {
    let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(processor.gix_repo);

    // Fetch without a refspec.
    if fetch_args.path.is_some() {
        anyhow::bail!("Cannot use --path without specifying a refspec");
    }
    let remote_names = processor.gix_repo.remote_names();
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
    fetcher.args.push("--prune".to_owned());
    fetcher.fetch_on_terminal()?;

    if !fetch_args.skip_filter {
        processor.reload_config()?;
        refilter(
            &cli::Refilter {
                keep_going: fetch_args.keep_going,
                job_count: fetch_args.job_count,
                no_fetch: false,
                reuse_cache: true,
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

#[tracing::instrument(skip(processor))]
fn fetch_with_refspec(
    fetch_args: &cli::Fetch,
    resolved_args: cli::ResolvedFetchParams,
    detailed_refspecs: &Vec<DetailedFetchRefspec>,
    processor: &mut MonoRepoProcessor,
) -> Result<()> {
    ErrorObserver::run_keep_going(fetch_args.keep_going, |error_observer| {
        let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(processor.gix_repo);

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
            fetch_args.job_count.into(),
            |commit_loader| commit_loader.load_repo(&resolved_args.repo),
            processor,
            error_observer,
        )?;

        match &resolved_args.repo {
            RepoName::Top => {
                let top_refs = detailed_refspecs
                    .iter()
                    .map(|refspec| &refspec.unfiltered_ref);
                git_toprepo::expander::refilter_some_top_refspecs(processor, top_refs)?;
            }
            RepoName::SubRepo(sub_repo_name) => {
                for refspec in detailed_refspecs {
                    // TODO: Reuse the git-fast-import process for all refspecs.
                    let dest_ref = refspec.destination.get_filtered_ref();
                    if let Err(err) = git_toprepo::expander::expand_submodule_ref_onto_head(
                        processor,
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
                    let mut r = processor
                        .gix_repo
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
                    fetch_head_lines
                        .push(format!("{}{description_suffix}\n", mono_object_id.to_hex()));
                }
            }
        }
        if !fetch_head_lines.is_empty() {
            // Update .git/FETCH_HEAD.
            let fetch_head_path = processor.gix_repo.git_dir().join("FETCH_HEAD");
            std::fs::write(&fetch_head_path, fetch_head_lines.join(""))?;
        }
        Ok(())
    })
}

#[tracing::instrument(skip_all, fields(job_count = %job_count))]
fn load_commits<F>(
    job_count: NonZeroUsize,
    commit_loader_setup: F,
    processor: &mut MonoRepoProcessor,
    error_observer: &ErrorObserver,
) -> Result<()>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    let mut commit_loader = git_toprepo::loader::CommitLoader::new(
        processor.gix_repo,
        &mut processor.top_repo_cache.repos,
        processor.config,
        processor.progress.clone(),
        error_observer,
        threadpool::ThreadPool::new(job_count.get()),
    )?;
    commit_loader_setup(&mut commit_loader).with_context(|| "Failed to setup the commit loader")?;
    commit_loader.join()?;
    Ok(())
}

fn is_monorepo(path: &Path) -> Result<bool> {
    let key = &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY);
    let maybe = git_config_get(path, key)?;
    Ok(maybe.is_some())
}

#[tracing::instrument(skip(processor))]
fn push(push_args: &cli::Push, processor: &mut MonoRepoProcessor) -> Result<()> {
    let base_url = match processor
        .gix_repo
        .try_find_remote(push_args.top_remote.as_bytes())
    {
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
    let remote_ref = FullName::try_from(remote_ref.as_bytes().as_bstr())
        .with_context(|| format!("Bad remote ref {remote_ref}"))?;
    let local_rev = local_ref;

    let push_metadatas = git_toprepo::push::split_for_push(processor, &base_url, local_rev)?;

    ErrorObserver::run_keep_going(!push_args.fail_fast, |error_observer| {
        let commit_pusher = git_toprepo::push::CommitPusher::new(
            processor.gix_repo.clone(),
            processor.progress.clone(),
            error_observer.clone(),
            push_args.job_count.into(),
        );
        commit_pusher.push(push_metadatas, &remote_ref, push_args.dry_run)
    })
}

#[tracing::instrument]
fn dump(dump_args: &cli::Dump, monorepo_root: &Option<PathBuf>) -> Result<()> {
    match dump_args {
        cli::Dump::ImportCache => dump_import_cache(),
        cli::Dump::GitModules => dump_modules(monorepo_root),
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::Host => dump_gerrit(choice),
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::UserOverride => {
            dump_gerrit(choice)
        }
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::Project => dump_gerrit(choice),
        cli::Dump::Gerrit(_) => unreachable!(),
    }
}

fn dump_gerrit(choice: &cli::DumpGerrit) -> Result<()> {
    let toprepo = repo::TopRepo::open(&PathBuf::from("."))?;

    let mut git_review_file = toprepo.gix_repo.path().parent().unwrap().to_owned();
    git_review_file.push(".gitreview");

    let mut content: String = "".to_owned();
    std::fs::File::open(&git_review_file)
        .context(format!("{}", git_review_file.display()))?
        .read_to_string(&mut content)
        .unwrap();
    let git_review = parse_git_review(&content)?;

    let cons = match choice {
        cli::DumpGerrit::Host => git_review.host,
        cli::DumpGerrit::Project => git_review.project,
        cli::DumpGerrit::UserOverride => bail!("No user override is specified."),
    };
    println!("{cons}");
    Ok(())
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

#[tracing::instrument]
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
        let parent_span = tracing::Span::current();
        let thread = std::thread::Builder::new()
            .name("signal-handler".to_owned())
            .spawn_scoped(s, move || {
                let mut signal_iter = signals.forever().peekable();
                tracing::debug_span!(parent: parent_span.clone(), "signal_handler_watch").in_scope(
                    || {
                        if let Some(signal) = signal_iter.peek() {
                            let signal_str = signal_hook::low_level::signal_name(*signal)
                                .map(|name| name.to_owned())
                                .unwrap_or(signal.to_string());
                            tracing::info!("Received termination signal {signal_str}");
                        }
                        // Stop listening for signals and run the shutdown function.
                        signal_handler_clone.close();
                    },
                );
                tracing::debug_span!(parent: parent_span.clone(), "signal_handler_shutdown")
                    .in_scope(shutdown_fn);
                tracing::debug_span!(parent: parent_span, "signal_handler_reraise").in_scope(
                    || {
                        // Reraise all signals to ensure they are handled in the default manner.
                        for signal in signal_iter {
                            signal_hook::low_level::emulate_default_handler(signal).expect(
                                "emulate default signal handler in the signal handler thread",
                            );
                        }
                    },
                );
            })?;
        // Run the main function.
        let result_or_panic = catch_unwind(AssertUnwindSafe(main_fn));
        // Shutdown the signal handler.
        signal_handler.close();
        // Now it is safe to join the thread or panic.
        let result = match result_or_panic {
            Ok(result) => result,
            Err(e) => resume_unwind(e),
        };
        thread.join().unwrap();
        result
    })
}

fn main_impl<I>(argv: I, logger: Option<&git_toprepo::log::GlobalLogger>) -> Result<()>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let args = Cli::parse_from(argv);

    if let Some(logger) = logger {
        match args.log_level.value() {
            Ok(level) => logger.set_stderr_log_level(level),
            Err(err) => {
                log::error!("{err:#}");
                let usage_exit_code = 2;
                std::process::exit(usage_exit_code);
            }
        }
    }

    if let Some(path) = &args.working_directory {
        std::env::set_current_dir(path)?;
    }

    let current_dir = std::env::current_dir()?;
    // First find whether we are in a git repo at all.
    // It is used as the anchor to later detect if it is a monorepo.
    let repo_root = git_toprepo::util::find_current_worktree(&current_dir);
    // TODO(nils): it is `mut` only for the two-stage `init`.
    //             If we refactor the first stage to fulfill `is_monorepo`
    //             we could probably refactor away the mutation.
    let mut monorepo_root = if let Ok(repo_root) = &repo_root {
        match is_monorepo(repo_root)? {
            true => Some(repo_root.clone()),
            false => None,
        }
    } else {
        None
    };
    /*
     * // TODO: `dump` would like to have the relative path to the root
     * // for it to calculate relative paths correctly.
     * let down_from_root = current_dir.strip_prefix(&toprepo_root);
     * let up_to_root = Path::new();
     */

    // First run subcommands that can run with a mis- or unconfigured repo.
    match &args.command {
        Commands::Init(init_args) => {
            init(init_args).map(|_path| ())?;
            log::info!("The next step is to run 'git-toprepo fetch'.");
            return Ok(());
        }
        Commands::Clone(cli::Clone {
            init: init_args,
            refilter: _,
            minimal: _,
        }) => {
            let directory = init(init_args)?;
            std::env::set_current_dir(&directory).with_context(|| {
                format!(
                    "Failed to change working directory to {}",
                    directory.display()
                )
            })?;
            // NB: We have not set the marker in the initial clone
            // But this command is meant to create it.
            // So it is safe to proceed with the processor `clone_after_init`.
            monorepo_root = Some(directory);
        }
        Commands::Config(config_args) => return config(config_args, &monorepo_root),
        // TODO: Dump can run with a mis- or unconfigured repo.
        //       But it would also be good if it did run on a configured repo
        //       and could dump more information. Like the remotes of each
        //       module. We can probably find it through `gix::Repository`,
        //       but the main toprepo operations do it through `GitTopRepoConfig`
        //       which is not available without the processor.
        //
        //       It strikes me as odd to have two completely different access
        //       paths for the same information. And makes maintaining these
        //       commands harder. I would prefer to have a richer representation
        //       around the repo. And have it optionally contain more specific
        //       toprepo configs, than to maintain them in two different data
        //       structures without a less clear (shared) provenance.
        //
        //       Especially since some algorithms only operate on submodules
        //       but others operate on all projects (including super)
        //       using the same API. So to avoid placing an early exit that
        //       checks for the main module before each submodule lookup
        //       it would be better to have a datastructure that treat them as
        //       interchangeable.
        //
        //       $ git-toprepo fetch ssh://gerrit.example/super 1046c7139f113ca82ccff86722707e089debf919
        //       ERROR: No configured submodule URL matches "ssh://gerrit.example/super"
        Commands::Dump(dump_args) => return dump(dump_args, &monorepo_root),
        Commands::Version => return print_version(),
        Commands::IsMonorepo => {
            return match monorepo_root.is_some() {
                true => Ok(()),
                false => Err(ExitSilently.into()),
            };
        }
        _ => {}
    }

    let toprepo_root = monorepo_root.as_ref().ok_or(NotAMonorepo)?;

    // Now when the working directory is set, we can persist the tracing.
    if let Some(logger) = logger {
        logger.write_to_git_dir(gix::open(toprepo_root)?.git_dir())?;
    }

    git_toprepo::repo::MonoRepoProcessor::run(Path::new(&toprepo_root), |processor| {
        match args.command {
            // TODO: Why does the config belong to the processor?
            // would it not make more sense in a richer repo representation,
            // we do have the regular submodule info available in the gix::Repository
            // that is customarily used for the toprepo itself.
            // But that only contains the local paths, no remote information.

            // Main toprepo operations.
            Commands::Init(_) => unreachable!("init is already processed."),
            Commands::Clone(clone_args) => clone_after_init(&clone_args, processor),
            Commands::Config(_) => unreachable!("config is already processed."),
            Commands::Refilter(refilter_args) => refilter(&refilter_args, processor),
            Commands::Fetch(fetch_args) => fetch(&fetch_args, processor),
            Commands::Push(push_args) => push(&push_args, processor),
            Commands::IsMonorepo => unreachable!("is-monorepo is already handled."),
            // User friendly introspection into the tool itself.
            Commands::Dump(_) => unreachable!("dump is already processed."),
            Commands::Version => unreachable!("version is already processed."),

            // Experimental and scaffolding commands.
            Commands::Checkout(ref checkout_args) => checkout(&args, checkout_args),
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
                // NB: downcast of error is discouraged
                // and we need to do it more we should look into more structured
                // custom errors for our code base, like the `thiserror` crate.
                // https://www.reddit.com/r/learnrust/comments/zovj2x/how_to_match_on_underlying_error_when_using_anyhow/
                if let Some(ExitSilently) = err.downcast_ref() {
                    return Ok(ExitCode::FAILURE);
                }
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
        let err = main_impl(argv, None).unwrap_err();
        assert!(err.downcast_ref() == Some(&NotAMonorepo));
    }

    #[test]
    fn test_main_in_uninitialized_git_toprepo() {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_main_outside_git_toprepo",
        );
        let temp_dir_str = temp_dir.to_str().unwrap();
        let mut init_cmd = git_command(&temp_dir.to_path_buf().to_owned());
        let _ = init_cmd.arg("init").output();
        let argv = vec!["git-toprepo", "-C", temp_dir_str, "config", "show"];
        let argv = argv.into_iter().map(|s| s.into());
        let err = main_impl(argv, None).unwrap_err();
        assert!(err.downcast_ref() == Some(&NotAMonorepo));
    }

    // TODO: Check that the formatting of the NotAMonorepo is visually appealing.

    /* TODO: We should also make sure that a running git-toprepo
     * inside a proper checked out submodule fails.
     * In there regular git should be used.
    #[test]
    fn test_main_insdie_a_proper_submodule_to_a_git_toprepo() {
    }
    */
}
