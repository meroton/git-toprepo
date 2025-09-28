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
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::log::CommandSpanExt as _;
use git_toprepo::log::ErrorMode;
use git_toprepo::log::ErrorObserver;
use git_toprepo::repo::ConfiguredTopRepo;
use git_toprepo::repo::ImportCache;
use git_toprepo::repo_name::RepoName;
use git_toprepo::util::CommandExtension as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools as _;
use std::env;
use std::io::Read;
use std::num::NonZeroUsize;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::panic::resume_unwind;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

fn gix_discover_current_dir() -> Result<gix::Repository> {
    // Using working directory instead of "." to get better error messages.
    let current_dir = std::env::current_dir()?;
    git_toprepo::repo::gix_discover(&current_dir)
}

fn discover_configured_repo_current_dir() -> Result<ConfiguredTopRepo> {
    // Using working directory instead of "." to get better error messages.
    let current_dir = std::env::current_dir()?;
    ConfiguredTopRepo::discover(&current_dir)
}

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
    ConfiguredTopRepo::create(&directory, url)?;
    log::info!("Initialized git-toprepo in {}", directory.display());
    Ok(directory)
}

#[tracing::instrument(skip(configured_repo))]
fn clone_after_init(
    clone_args: &cli::Clone,
    configured_repo: &mut ConfiguredTopRepo,
) -> Result<()> {
    fetch(
        &cli::Fetch {
            keep_going: false,
            job_count: std::num::NonZero::new(1).unwrap(),
            skip_filter: true,
            remote: None,
            path: None,
            refspecs: None,
        },
        configured_repo,
    )?;
    let repo_dir = configured_repo
        .gix_repo
        .workdir()
        .expect("ConfiguredTopRepo should have a working directory");
    verify_config_existence_after_clone(repo_dir)?;
    if !clone_args.minimal {
        // Reload the config from disk to get the cloned configuration now
        // stored in the repository.
        configured_repo.reload_repo()?;
        refilter(&clone_args.refilter, configured_repo)?;
    }
    std::process::Command::new("git")
        .current_dir(
            configured_repo
                .gix_repo
                .workdir()
                .expect("ConfiguredTopRepo should have a working directory"),
        )
        .args(["checkout", "refs/remotes/origin/HEAD", "--"])
        .trace_command(git_toprepo::command_span!("git checkout"))
        .check_success_with_stderr()?;
    Ok(())
}

#[tracing::instrument(skip_all)]
fn verify_config_existence_after_clone(repo_dir: &Path) -> Result<()> {
    let gix_repo = gix::open(repo_dir).context("Could not open the newly cloned repository")?;
    let location = GitTopRepoConfig::find_configuration_location(&gix_repo)?;
    if location.validate_existence(&gix_repo).is_err() {
        // Fetch from the default remote to get all the direct submodules.
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
    if file == "-" {
        || -> Result<GitTopRepoConfig> {
            let mut toml_string = String::new();
            std::io::stdin().read_to_string(&mut toml_string)?;
            GitTopRepoConfig::parse_config_toml_string(&toml_string)
        }()
        .context("Loading from stdin")
    } else {
        || -> Result<GitTopRepoConfig> {
            let toml_string = std::fs::read_to_string(file)?;
            GitTopRepoConfig::parse_config_toml_string(&toml_string)
        }()
        .with_context(|| format!("Loading config file {}", file.display()))
    }
}

#[tracing::instrument]
fn config(config_args: &cli::Config) -> Result<()> {
    match &config_args {
        cli::Config::Location => {
            // TODO: 2025-09-24 Print something that can easily be selected and
            // opened in a terminal or VsCode or so. The "local:" prefix makes
            // it harder.
            let repo = gix_discover_current_dir()?;
            let location = GitTopRepoConfig::find_configuration_location(&repo)?;
            if let Err(err) = location.validate_existence(&repo) {
                log::warn!("{err:#}");
            }
            println!("{location}");
        }
        cli::Config::Show => {
            let repo = discover_configured_repo_current_dir()?;
            print!("{}", toml::to_string(&repo.config)?);
        }
        cli::Config::Bootstrap => {
            let repo = gix_discover_current_dir()?;
            let config = config_bootstrap(&repo)?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::Config::Normalize(args) => {
            let config = load_config_from_file(args.file.as_path())?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::Config::Validate(validation) => {
            let _config = load_config_from_file(validation.file.as_path())?;
        }
    }
    Ok(())
}

fn config_bootstrap(repo: &gix::Repository) -> Result<GitTopRepoConfig> {
    let mut repo = ConfiguredTopRepo::new_empty(repo.clone());
    let default_remote_name = repo
        .gix_repo
        .remote_default_name(gix::remote::Direction::Fetch)
        .with_context(|| "Failed to get the default remote name")?;
    let bootstrap_ref = FullName::try_from(format!(
        "{}refs/remotes/{default_remote_name}/HEAD",
        RepoName::Top.to_ref_prefix()
    ))?;
    let head_commit = repo
        .gix_repo
        .find_reference(&bootstrap_ref)?
        .peel_to_commit()?;
    let head_commit_id = head_commit.id;
    let gitmod_infos = match head_commit.tree()?.find_entry(".gitmodules") {
        Some(entry) => GitModulesInfo::parse_dot_gitmodules_bytes(
            &entry.object()?.data,
            PathBuf::from(".gitmodules"),
        )?,
        None => GitModulesInfo::default(),
    };
    drop(head_commit);
    git_toprepo::log::get_global_logger().with_progress(|progress| {
        ErrorObserver::run(ErrorMode::KeepGoing, |error_observer| {
            let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                &mut repo,
                &progress,
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
        let top_repo_data = repo
            .import_cache
            .repos
            .get(&RepoName::Top)
            .expect("top repo has been loaded");
        let thin_head_commit = top_repo_data
            .thin_commits
            .get(&head_commit_id)
            .with_context(|| {
                format!("Missing the HEAD commit {} in the top repo", head_commit_id)
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
            // TODO: 2025-09-22 Refactor to not use missing_subrepos.clear() for
            // accessing the submodule configs.
            repo.ledger.missing_subrepos.clear();
            match repo
                .ledger
                .get_name_from_url(submod_url)
                .with_context(|| format!("Submodule {submod_path}"))
            {
                Ok(Some(name)) => {
                    repo.ledger
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
        Ok(GitTopRepoConfig {
            checksum: repo.config.checksum,
            fetch: repo.config.fetch,
            subrepos: repo.ledger.subrepos,
        })
    })
}

#[tracing::instrument(skip(configured_repo))]
fn refilter(refilter_args: &cli::Refilter, configured_repo: &mut ConfiguredTopRepo) -> Result<()> {
    if !refilter_args.reuse_cache {
        configured_repo.import_cache = ImportCache::default();
    }
    git_toprepo::log::get_global_logger().with_progress(|progress| {
        ErrorObserver::run_keep_going(refilter_args.keep_going, |error_observer| {
            load_commits(
                refilter_args.job_count.into(),
                |commit_loader| {
                    commit_loader.fetch_missing_commits = !refilter_args.no_fetch;
                    commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)
                },
                configured_repo,
                &progress,
                error_observer,
            )
        })?;
        configured_repo.config = GitTopRepoConfig {
            checksum: configured_repo.config.checksum.clone(),
            fetch: configured_repo.config.fetch.clone(),
            subrepos: configured_repo.ledger.subrepos.clone(),
        };
        git_toprepo::expander::refilter_all_top_refs(configured_repo, &progress)
    })
}

#[tracing::instrument(skip(configured_repo))]
fn fetch(fetch_args: &cli::Fetch, configured_repo: &mut ConfiguredTopRepo) -> Result<()> {
    if let Some(refspecs) = &fetch_args.refspecs {
        let resolved_args = cli::resolve_remote_and_path(
            fetch_args,
            &configured_repo.gix_repo,
            &configured_repo.ledger,
        )?;
        let detailed_refspecs = detail_refspecs(refspecs, &resolved_args.repo, &resolved_args.url)?;
        git_toprepo::log::get_global_logger().with_progress(|progress| {
            fetch_with_refspec(
                fetch_args,
                resolved_args,
                &detailed_refspecs,
                configured_repo,
                &progress,
            )
        })?;
        // Delete temporary refs, but only on success. Keep the refs on failure
        // to be able to debug.
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
            configured_repo
                .gix_repo
                .edit_references_as(ref_edits, Some(committer))
                .context("Failed to delete temporary fetch-head references")?;
        }
        Ok(())
    } else {
        fetch_with_default_refspecs(fetch_args, configured_repo)
    }
}

#[tracing::instrument(skip(configured_repo))]
fn fetch_with_default_refspecs(
    fetch_args: &cli::Fetch,
    configured_repo: &mut ConfiguredTopRepo,
) -> Result<()> {
    let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(&configured_repo.gix_repo);

    // Fetch without a refspec.
    if fetch_args.path.is_some() {
        anyhow::bail!("Cannot use --path without specifying a refspec");
    }
    let remote_names = configured_repo.gix_repo.remote_names();
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
        // Reload the config from disk to get any changes fetched into the repository.
        configured_repo.reload_repo()?;
        refilter(
            &cli::Refilter {
                keep_going: fetch_args.keep_going,
                job_count: fetch_args.job_count,
                no_fetch: false,
                reuse_cache: true,
            },
            configured_repo,
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
    // TODO: 2025-09-22 Implement force fetch with + refspec.
    #[allow(unused)]
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

#[tracing::instrument(skip(configured_repo))]
fn fetch_with_refspec(
    fetch_args: &cli::Fetch,
    resolved_args: cli::ResolvedFetchParams,
    detailed_refspecs: &Vec<DetailedFetchRefspec>,
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
) -> Result<()> {
    ErrorObserver::run_keep_going(fetch_args.keep_going, |error_observer| {
        let mut fetcher = git_toprepo::fetch::RemoteFetcher::new(&configured_repo.gix_repo);

        fetcher.remote = Some(
            resolved_args
                .url
                .to_bstring()
                .to_str()
                .context("Bad UTF-8 defualt remote URL")?
                .to_owned(),
        );
        // TODO: 2025-09-22 Should the + force be possible to remove?
        fetcher.refspecs = detailed_refspecs
            .iter()
            .map(|refspec| format!("+{}:{}", refspec.remote_ref, refspec.unfiltered_ref))
            .collect_vec();
        fetcher.fetch_on_terminal()?;
        // Stop early?
        if fetch_args.skip_filter {
            return Ok(());
        }
        configured_repo.reload_repo()?;

        load_commits(
            fetch_args.job_count.into(),
            |commit_loader| commit_loader.load_repo(&resolved_args.repo),
            configured_repo,
            progress,
            error_observer,
        )?;
        configured_repo.config = GitTopRepoConfig {
            checksum: configured_repo.config.checksum.clone(),
            fetch: configured_repo.config.fetch.clone(),
            subrepos: configured_repo.ledger.subrepos.clone(),
        };

        match &resolved_args.repo {
            RepoName::Top => {
                let top_refs = detailed_refspecs
                    .iter()
                    .map(|refspec| &refspec.unfiltered_ref);
                git_toprepo::expander::refilter_some_top_refspecs(
                    configured_repo,
                    progress,
                    top_refs,
                )?;
            }
            RepoName::SubRepo(sub_repo_name) => {
                for refspec in detailed_refspecs {
                    // TODO: 2025-09-22 Reuse the git-fast-import process for all refspecs.
                    let dest_ref = refspec.destination.get_filtered_ref();
                    if let Err(err) = git_toprepo::expander::expand_submodule_ref_onto_head(
                        configured_repo,
                        progress,
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
                    let mut r = configured_repo
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
            let fetch_head_path = configured_repo.gix_repo.git_dir().join("FETCH_HEAD");
            std::fs::write(&fetch_head_path, fetch_head_lines.join(""))?;
        }
        Ok(())
    })
}

#[tracing::instrument(skip_all, fields(job_count = %job_count))]
fn load_commits<F>(
    job_count: NonZeroUsize,
    commit_loader_setup: F,
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
    error_observer: &ErrorObserver,
) -> Result<()>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
{
    let mut commit_loader = git_toprepo::loader::CommitLoader::new(
        configured_repo,
        progress,
        error_observer,
        threadpool::ThreadPool::new(job_count.get()),
    )?;
    commit_loader_setup(&mut commit_loader).with_context(|| "Failed to setup the commit loader")?;
    commit_loader.join()?;
    Ok(())
}

#[tracing::instrument(skip(configured_repo))]
fn push(push_args: &cli::Push, configured_repo: &mut ConfiguredTopRepo) -> Result<()> {
    let mut extra_args = Vec::new();
    if push_args.force {
        extra_args.push("--force".to_owned());
    }

    let base_url = match configured_repo
        .gix_repo
        .try_find_remote(push_args.top_remote.as_bytes())
    {
        Some(Ok(remote)) => remote
            // TODO: 2025-09-22 Support push URL config.
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
    // TODO: 2025-09-22 This assumes a single ref in the refspec. What about patterns?
    let remote_ref = FullName::try_from(remote_ref.as_bytes().as_bstr())
        .with_context(|| format!("Bad remote ref {remote_ref}"))?;
    let local_rev = local_ref;

    git_toprepo::log::get_global_logger().with_progress(|progress| {
        let push_metadatas =
            git_toprepo::push::split_for_push(configured_repo, &progress, &base_url, local_rev)?;
        ErrorObserver::run_keep_going(!push_args.fail_fast, |error_observer| {
            let commit_pusher = git_toprepo::push::CommitPusher::new(
                configured_repo.gix_repo.clone(),
                progress.clone(),
                error_observer.clone(),
                push_args.job_count.into(),
            );
            commit_pusher.push(push_metadatas, &remote_ref, &extra_args, push_args.dry_run)
        })
    })
}

#[tracing::instrument]
fn print_info(info_args: &cli::Info) -> Result<ExitCode> {
    let repo = gix_discover_current_dir()?;
    let config_location_str_result = GitTopRepoConfig::find_configuration_location_str(&repo);

    if info_args.is_monorepo {
        // Handle the case where the repository is a monorepo.
        if let Err(err) = config_location_str_result {
            log::warn!("{err}");
            Ok(ExitCode::from(cli::Info::EXIT_CODE_FALSE))
        } else {
            Ok(ExitCode::SUCCESS)
        }
    } else {
        let keys = info_args
            .value
            .map_or(Vec::from(cli::InfoValue::ALL_VARIANTS), |v| vec![v]);
        let mut keys_and_values = Vec::new();
        for key in keys {
            let value = match key {
                cli::InfoValue::ConfigLocation => match &config_location_str_result {
                    Ok(s) => s.clone(),
                    Err(err) => {
                        log::warn!("{err}");
                        String::new()
                    }
                },
                cli::InfoValue::CurrentWorktree => match repo.workdir() {
                    Some(path) => path.to_string_lossy().to_string(),
                    None => "<bare repository>".to_string(),
                },
                cli::InfoValue::Cwd => env::current_dir()?.to_string_lossy().to_string(),
                cli::InfoValue::GitDir => repo.git_dir().to_string_lossy().to_string(),
                cli::InfoValue::ImportCache => {
                    let cache_path =
                        git_toprepo::import_cache_serde::SerdeImportCache::get_cache_path(&repo);
                    cache_path.to_string_lossy().to_string()
                }
                cli::InfoValue::MainWorktree => {
                    match git_toprepo::util::find_main_worktree_path(&repo) {
                        Ok(path) => path.to_string_lossy().to_string(),
                        Err(err) => {
                            log::warn!("No main worktree: {err}");
                            String::new()
                        }
                    }
                }
                cli::InfoValue::Version => get_version(),
            };
            keys_and_values.push((key.to_string(), value));
        }
        if info_args.value.is_none() {
            for (key, value) in keys_and_values {
                println!("{key} {value}");
            }
        } else {
            // Should only be one value.
            debug_assert_eq!(keys_and_values.len(), 1);
            for (_key, value) in keys_and_values {
                println!("{value}");
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

#[tracing::instrument]
fn dump(dump_args: &cli::Dump) -> Result<()> {
    match dump_args {
        cli::Dump::ImportCache(args) => dump_import_cache(args),
        cli::Dump::GitModules => dump_git_modules(),
    }
}

fn dump_import_cache(args: &cli::DumpImportCache) -> Result<()> {
    let serde_repo_states = if let Some(cache_path) = &args.file {
        let reader: &mut dyn std::io::Read = if cache_path == "-" {
            &mut std::io::stdin()
        } else {
            &mut std::fs::File::open(cache_path)?
        };
        git_toprepo::import_cache_serde::SerdeImportCache::load_from_reader(
            cache_path, reader, None,
        )?
    } else {
        let repo = gix_discover_current_dir()?;
        // Demand a configured repository to ensure we not just fall back to empty
        // cache content when not even inside a git-toprepo emulated monorepo.
        let _ = GitTopRepoConfig::find_configuration_location(&repo)?;
        git_toprepo::import_cache_serde::SerdeImportCache::load_from_git_dir(&repo, None)?
    };
    serde_repo_states.dump_as_json(std::io::stdout())?;
    println!();
    Ok(())
}

fn dump_git_modules() -> Result<()> {
    let repo = gix_discover_current_dir()?;

    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    #[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
    struct Mod {
        project: String,
        path: git_toprepo::git::GitPath,
    }

    let main_project = git_toprepo::repo::resolve_gerrit_project(&repo)
        .context("Loading the main repo Gerrit project")?;
    let mut modules: Vec<Mod> = git_toprepo::repo::get_submodules(&repo)?
        .into_iter()
        .map(|(path, project)| Mod { project, path })
        .collect();
    modules.push(Mod {
        project: main_project,
        // TODO: What is the path to the repo? May be upwards.
        path: ".".into(),
    });
    modules.sort();

    for module in modules {
        println!("{} {}", module.project, module.path);
    }
    Ok(())
}

#[tracing::instrument]
/// Creates a human readable version string for git-toprepo.
fn get_version() -> String {
    format!(
        "{} {}~{}-{}",
        env!("CARGO_PKG_NAME"),
        option_env!("BUILD_SCM_TAG").unwrap_or("0.0.0"),
        option_env!("BUILD_SCM_TIMESTAMP").unwrap_or("timestamp"),
        option_env!("BUILD_SCM_REVISION").unwrap_or("git-hash"),
    )
}

/// Run an operation with automatic setup and teardown lifecycle for operations.
fn run_session<T, F>(logger: Option<&git_toprepo::log::GlobalLogger>, f: F) -> Result<T>
where
    F: FnOnce(&mut ConfiguredTopRepo) -> Result<T>,
{
    let mut repo = discover_configured_repo_current_dir()?;

    if let Some(logger) = logger {
        logger.write_to_git_dir(repo.gix_repo.git_dir())?;
    }

    // Run the operation.
    let result = f(&mut repo);

    // Teardown: Always save state (preserves partial work even on errors).
    let save_result = repo.save_state();

    // Return operation result, but if save failed and operation succeeded, return save error
    match (result, save_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(save_err)) => Err(save_err),
        (Err(op_err), _) => Err(op_err), // Operation error takes precedence
    }
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
                                .unwrap_or_else(|| signal.to_string());
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

fn main_impl<I>(argv: I, logger: Option<&git_toprepo::log::GlobalLogger>) -> Result<ExitCode>
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
        std::env::set_current_dir(path)
            .with_context(|| format!("Failed to change working directory to {}", path.display()))?;
    }

    match args.command {
        Commands::Init(init_args) => {
            init(&init_args).map(|_path| ())?;
            log::info!("The next step is to run 'git-toprepo fetch'.");
            Ok(ExitCode::SUCCESS)
        }
        Commands::Config(config_args) => config(&config_args).map(|()| ExitCode::SUCCESS),
        Commands::Dump(dump_args) => dump(&dump_args).map(|()| ExitCode::SUCCESS),
        Commands::Clone(clone_args) => {
            // Two-stage initialization: init + clone_after_init
            let directory = init(&clone_args.init)?;
            std::env::set_current_dir(&directory).with_context(|| {
                format!(
                    "Failed to change working directory to {}",
                    directory.display()
                )
            })?;
            run_session(logger, |configured| {
                clone_after_init(&clone_args, configured)
            })
            .map(|()| ExitCode::SUCCESS)
        }
        Commands::Refilter(refilter_args) => {
            run_session(logger, |configured| refilter(&refilter_args, configured))
                .map(|()| ExitCode::SUCCESS)
        }
        Commands::Fetch(fetch_args) => {
            run_session(logger, |configured| fetch(&fetch_args, configured))
                .map(|()| ExitCode::SUCCESS)
        }
        Commands::Push(push_args) => run_session(logger, |configured| push(&push_args, configured))
            .map(|()| ExitCode::SUCCESS),

        Commands::Info(info_args) => print_info(&info_args),
        Commands::Version => {
            println!("git-toprepo {}", get_version());
            Ok(ExitCode::SUCCESS)
        }
    }
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
        || {
            main_impl(std::env::args_os(), Some(global_logger)).or_else(|err| {
                log::error!("{err:#}");
                Ok(ExitCode::FAILURE)
            })
        },
        || global_logger.finalize(),
    )
    .unwrap_or_else(|err| {
        eprintln!("{}: {err:#}", "ERROR".red().bold());
        ExitCode::FAILURE
    })
}
