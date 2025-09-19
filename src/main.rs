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
use anyhow::bail;
use bstr::BStr;
use bstr::ByteSlice as _;
use clap::Parser;
use colored::Colorize;
use git_gr_lib::gerrit::Gerrit;
use git_gr_lib::gerrit::HTTPPasswordPolicy;
use git_gr_lib::query::QueryOptions;
use git_toprepo::config;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::repo::RepoHandle;
use git_toprepo::repo::TopRepo;
use git_toprepo::error::AlreadyAMonorepo;
use git_toprepo::git::GitModulesInfo;
use git_toprepo::git::git_command;
use git_toprepo::gitreview::parse_git_review;
use git_toprepo::loader::SubRepoLedger;
use git_toprepo::log::CommandSpanExt as _;
use git_toprepo::log::ErrorMode;
use git_toprepo::log::ErrorObserver;
use git_toprepo::repo;
use git_toprepo::repo::ConfiguredTopRepo;
use git_toprepo::repo_name::RepoName;
use git_toprepo::submitted_together::order_submitted_together;
use git_toprepo::submitted_together::split_by_supercommits;
use git_toprepo::util::CommandExtension as _;
use git_toprepo::error::NotAMonorepo;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools as _;
use std::collections::HashSet;
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

#[tracing::instrument(skip(configured_repo))]
fn clone_after_init(clone_args: &cli::Clone, configured_repo: &mut repo::ConfiguredTopRepo) -> Result<()> {
    fetch(
        &cli::Fetch {
            keep_going: false,
            job_count: std::num::NonZero::new(1).unwrap(),
            skip_filter: true,
            fetch_entire_topics_from_gerrit: false,
            remote: None,
            path: None,
            refspecs: None,
        },
        configured_repo,
    )?;
    let repo_dir = configured_repo.gix_repo.workdir().expect("ConfiguredTopRepo should have a working directory");
    verify_config_existence_after_clone(repo_dir)?;
    if !clone_args.minimal {
        configured_repo.reload_config()?;
        refilter(&clone_args.refilter, configured_repo)?;
    }
    git_command(configured_repo.gix_repo.workdir().expect("ConfiguredTopRepo should have a working directory"))
        .args(["checkout", "refs/remotes/origin/HEAD", "--"])
        .trace_command(git_toprepo::command_span!("git checkout"))
        .check_success_with_stderr()?;
    Ok(())
}

#[tracing::instrument(skip_all)]
fn verify_config_existence_after_clone(repo_dir: &Path) -> Result<()> {
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

#[tracing::instrument(skip(repo))]
// NB: This could also take an `Option<RepoHandle>` but we know it is a result from
// earlier, so this kind of smears the error handling between this function and
// the caller. But it allows us to give correct errors for each case.
//
// In the `Normalize` and `Validate` cases the callee error is entirely
// irrelevant and should not be contextualized.
// In `Bootstrap`, `Location` and `Show` the callee error is sufficient on its own
// so we just return it.
fn config(config_args: &cli::Config, repo: Result<RepoHandle>) -> Result<()> {
    match &config_args.config_command {
        cli::ConfigCommands::Location => {
            // TODO: Print something that can easily be selected and opened in
            // a terminal or VsCode or so. The "local:" prefix makes it harder.
            let repo = &ConfiguredTopRepo::try_from(repo)?.gix_repo;
            let repo_dir = repo.git_dir();
            let location = config::GitTopRepoConfig::find_configuration_location(repo_dir)?;
            if let Err(err) = location.validate_existence(repo_dir) {
                log::warn!("{err:#}");
            }
            println!("{location}");
        }
        cli::ConfigCommands::Show => {
            let repo = &ConfiguredTopRepo::try_from(repo)?.gix_repo;
            let repo_dir = repo.git_dir();
            let config = config::GitTopRepoConfig::load_config_from_repo(repo_dir)?;
            print!("{}", toml::to_string(&config)?);
        }
        cli::ConfigCommands::Bootstrap => {
            match repo {
                Ok(RepoHandle::Basic(repo, err)) => {
                    if let Some(ref error) = err {
                        log::trace!("Config loading error in basic repo during bootstrap: {error:#}");
                    }
                    let config = config_bootstrap(&repo)?;
                    print!("{}", toml::to_string(&config)?);
                },
                Ok(RepoHandle::Configured(_)) => return Err(AlreadyAMonorepo.into()),
                Err(e) => return Err(e),
            };
        },
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

fn config_bootstrap(repo: &TopRepo) -> Result<GitTopRepoConfig> {
    let default_remote_name = repo.gix_repo
        .remote_default_name(gix::remote::Direction::Fetch)
        .with_context(|| "Failed to get the default remote name")?;
    let bootstrap_ref = FullName::try_from(format!(
        "{}refs/remotes/{default_remote_name}/HEAD",
        RepoName::Top.to_ref_prefix()
    ))?;
    let head_commit = repo.gix_repo.find_reference(&bootstrap_ref)?.peel_to_commit()?;
    let dot_gitmodules_bytes = match head_commit.tree()?.find_entry(".gitmodules") {
        Some(entry) => &entry.object()?.data,
        None => &Vec::new(),
    };
    let gitmod_infos = GitModulesInfo::parse_dot_gitmodules_bytes(
        dot_gitmodules_bytes,
        PathBuf::from(".gitmodules"),
    )?;
    let config = GitTopRepoConfig::default();
    let mut top_repo_cache = git_toprepo::repo::TopRepoCache::default();

    let ledger = git_toprepo::log::get_global_logger().with_progress(|progress| {
        let mut ledger = SubRepoLedger{
            subrepos: config.subrepos.clone(),
            missing_subrepos: HashSet::new(),
        };
        ErrorObserver::run(ErrorMode::KeepGoing, |error_observer| {
            let mut commit_loader = git_toprepo::loader::CommitLoader::new(
                &repo.gix_repo,
                &mut top_repo_cache.repos,
                &config.fetch,
                &mut ledger,
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
            ledger.missing_subrepos.clear();
            match ledger
                .get_name_from_url(submod_url)
                .with_context(|| format!("Submodule {submod_path}"))
            {
                Ok(Some(name)) => {
                    ledger
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
        Ok(ledger)
    })?;
    // Skip printing the warnings in the initial configuration.
    // config.log = log_config;

    Ok(GitTopRepoConfig {
        checksum: config.checksum,
        fetch: config.fetch,
        subrepos: ledger.subrepos,
    })
}

/// Checkout topics from Gerrit.
fn checkout(_: &Cli, checkout: &cli::Checkout) -> Result<()> {
    if !checkout.dry_run {
        panic!("only --dry-run is supported.");
    }

    // TODO: Promote to a CLI argument,
    // and parse .gitreview for defaults instead of this!
    // It is in fact load bearing with the hacky git-gr overrides.

    let toprepo = repo::TopRepo::open(&PathBuf::from("."))?;
    // TODO: Do we want a helper function on toprepo itself for the path to the
    // source tree?
    let mut git_review_file = toprepo.gix_repo.path().parent().unwrap().to_owned();
    git_review_file.push(".gitreview");

    if !git_review_file.exists() {
        // TODO: rephrase and context.
        bail!("Could not read gitreview file");
    }
    let mut content: String = "".to_owned();
    File::open(git_review_file)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    let git_review = parse_git_review(&content)?;
    let http_host = git_review.host;
    let ssh_host = git_review.ssh_host;
    // TODO: git-gr: Why do we need to know the port? It is sufficient for ssh to know
    // it right? Refactor git-gr to omit ports.
    let port = git_review.port.unwrap_or(22);

    // let parsed_remote = git_gr_lib::gerrit_project::parse_remote_url(&checkout.remote).unwrap();
    // TODO: How should we ask for the username, or autodetect it?
    // It is often missing from the remote! We could rely on `.gitreview`.
    // let username_override = parsed_remote.username;

    let netrc = netrc::Netrc::new()?;

    let authenticator = netrc
        .hosts
        .get(&http_host)
        .context("Looking for Gerrit entry for '{&http_host}' in netrc file.")?;
    let username = authenticator.login.clone();

    let host = git_gr_lib::gerrit_project::GerritProject {
        host: git_gr_lib::gerrit_host::GerritHost {
            username: Some(username),
            host: ssh_host,
            http_host: Some(http_host),
            port: port as u16,
        },
        project: git_review.project,
    };

    let gerrit = Gerrit::new(
        host,
        // TODO: Now that we do parse the netrc ourselves we might as well pick
        // out the password and pass it on? To bypass even more setup code in
        // git-gr.
        HTTPPasswordPolicy::Netrc,
        /* cache: */ true,
        /* persist SSH: */ false, // No SSH calls are expected.
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
    let res = res
        .map_err(|e| anyhow::Error::from_boxed(e.into()))
        .context("Could not query Gerrit's REST API for changes submitted together")?;

    let res = order_submitted_together(res)?.chronological_order();
    let res = split_by_supercommits(res, &checkout.strategy)?;

    println!("# # Cherry-pick order:");
    let fetch_stem = "git toprepo fetch";
    for (index, topic) in res.0.into_iter().enumerate() {
        for supercommit in topic.into_iter() {
            for repo in supercommit.into_iter() {
                for commit in repo.into_iter().rev() {
                    let remote = format!("ssh://{}/{}.git", gerrit.host.host.host, commit.project);
                    let cherry_pick = "&& git cherry-pick --allow-empty refs/toprepo/fetch-head";
                    if let Some(subject) = commit.subject {
                        println!("# {subject}");
                    }
                    println!(
                        "{fetch_stem} {remote} {} {cherry_pick} # topic index: {index}",
                        commit.current_revision.unwrap()
                    );
                }
            }
        }
    }

    Ok(())
}

fn dump_modules(repo: repo::RepoHandle) -> Result<()> {
    /// The main repo is not technically a submodule.
    /// But it is very convenient to have transparent handling of the main
    /// project in code that iterates over projects provided by the users.
    struct Mod {
        project: String,
        path: git_toprepo::git::GitPath,
    }

    let (main_project, mut modules) = match repo {
        repo::RepoHandle::Configured(configured) => {
            let main_project = configured.gerrit_project().to_string();
            let modules: Vec<Mod> = configured
                .submodules()?
                .into_iter()
                .map(|(path, project)| Mod { project, path })
                .collect();
            (main_project, modules)
        }
        // NB: Don't care if there were errors loading the monorepo
        repo::RepoHandle::Basic(basic, _) => {
            let main_project = basic
                .gerrit_project()
                .context(git_toprepo::repo::LOADING_THE_MAIN_PROJECT_CONTEXT)?;
            let modules: Vec<Mod> = basic
                .submodules()?
                .into_iter()
                .map(|(path, project)| Mod { project, path })
                .collect();
            (main_project, modules)
        }
    };

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

#[tracing::instrument(skip(configured_repo))]
fn refilter(refilter_args: &cli::Refilter, configured_repo: &mut ConfiguredTopRepo) -> Result<()> {
    if !refilter_args.reuse_cache {
        configured_repo.top_repo_cache = repo::TopRepoCache::default();
    }
    ErrorObserver::run_keep_going(refilter_args.keep_going, |error_observer| {
        load_commits(
            refilter_args.job_count.into(),
            |commit_loader| {
                commit_loader.fetch_missing_commits = !refilter_args.no_fetch;
                commit_loader.load_repo(&git_toprepo::repo_name::RepoName::Top)
            },
            configured_repo,
            error_observer,
        )
    })?;
    configured_repo.config = GitTopRepoConfig {
        checksum: configured_repo.config.checksum.clone(),
        fetch: configured_repo.config.fetch.clone(),
        subrepos: configured_repo.ledger.subrepos.clone(),
    };
    git_toprepo::expander::refilter_all_top_refs(configured_repo)
}

#[tracing::instrument(skip(configured_repo))]
fn fetch(fetch_args: &cli::Fetch, configured_repo: &mut repo::ConfiguredTopRepo) -> Result<()> {
    if let Some(refspecs) = &fetch_args.refspecs {
        let resolved_args =
            cli::resolve_remote_and_path(fetch_args, &configured_repo.gix_repo, &configured_repo.ledger)?;
        let detailed_refspecs = detail_refspecs(refspecs, &resolved_args.repo, &resolved_args.url)?;
        let mut result =
            fetch_with_refspec(fetch_args, resolved_args, &detailed_refspecs, configured_repo);
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
            if let Err(err) = configured_repo
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
        fetch_with_default_refspecs(fetch_args, configured_repo)
    }
}

#[tracing::instrument(skip(configured_repo))]
fn fetch_with_default_refspecs(
    fetch_args: &cli::Fetch,
    configured_repo: &mut repo::ConfiguredTopRepo,
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
        configured_repo.reload_config()?;
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

#[tracing::instrument(skip(configured_repo))]
fn fetch_with_refspec(
    fetch_args: &cli::Fetch,
    resolved_args: cli::ResolvedFetchParams,
    detailed_refspecs: &Vec<DetailedFetchRefspec>,
    configured_repo: &mut ConfiguredTopRepo,
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
        configured_repo.reload_config()?;

        load_commits(
            fetch_args.job_count.into(),
            |commit_loader| commit_loader.load_repo(&resolved_args.repo),
            configured_repo,
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
                git_toprepo::expander::refilter_some_top_refspecs(configured_repo, top_refs)?;
            }
            RepoName::SubRepo(sub_repo_name) => {
                for refspec in detailed_refspecs {
                    // TODO: Reuse the git-fast-import process for all refspecs.
                    let dest_ref = refspec.destination.get_filtered_ref();
                    if let Err(err) = git_toprepo::expander::expand_submodule_ref_onto_head(
                        configured_repo,
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
    error_observer: &ErrorObserver,
) -> Result<()>
where
    F: FnOnce(&mut git_toprepo::loader::CommitLoader) -> Result<()>,
    {
        let mut commit_loader = git_toprepo::loader::CommitLoader::new(
            &configured_repo.gix_repo,
            &mut configured_repo.top_repo_cache.repos,
            &configured_repo.config.fetch,
            &mut configured_repo.ledger,
            configured_repo.progress.clone(),
            error_observer,
            threadpool::ThreadPool::new(job_count.get()),
        )?;
        commit_loader_setup(&mut commit_loader).with_context(|| "Failed to setup the commit loader")?;
        commit_loader.join()?;
        Ok(())
    }


#[tracing::instrument(skip(configured_repo))]
fn push(push_args: &cli::Push, configured_repo: &mut ConfiguredTopRepo) -> Result<()> {
    let base_url = match configured_repo
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

    let push_metadatas = git_toprepo::push::split_for_push(configured_repo, &base_url, local_rev)?;

    ErrorObserver::run_keep_going(!push_args.fail_fast, |error_observer| {
        let commit_pusher = git_toprepo::push::CommitPusher::new(
            configured_repo.gix_repo.clone(),
            configured_repo.progress.clone(),
            error_observer.clone(),
            push_args.job_count.into(),
        );
        commit_pusher.push(push_metadatas, &remote_ref, push_args.dry_run)
    })
}

fn dump(dump_args: &cli::Dump, repo: repo::RepoHandle) -> Result<()> {
    match dump_args {
        cli::Dump::ImportCache => dump_import_cache(repo),
        cli::Dump::GitModules => dump_modules(repo),
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::Host => dump_gerrit(choice, repo),
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::UserOverride => {
            dump_gerrit(choice, repo)
        }
        cli::Dump::Gerrit(choice) if choice == &cli::DumpGerrit::Project => dump_gerrit(choice, repo),
        cli::Dump::Gerrit(_) => unreachable!(),
    }
}

fn dump_gerrit(choice: &cli::DumpGerrit, repo: repo::RepoHandle) -> Result<()> {
    let git_dir = match repo {
        repo::RepoHandle::Configured(ref configured) => configured.gix_repo.git_dir(),
        // NB: Don't care if there were errors loading the monorepo
        repo::RepoHandle::Basic(ref basic, _) => basic.gix_repo.path(),
    };

    let mut git_review_file = git_dir.parent().unwrap().to_owned();
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

fn dump_import_cache(repo: repo::RepoHandle) -> Result<()> {
    let repo = &ConfiguredTopRepo::try_from(repo)?.gix_repo;
    let repo_dir = repo.git_dir();

    let serde_repo_states = git_toprepo::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
        repo_dir,
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

/// Session manager that provides setup/teardown lifecycle for operations
/// Replicates the MonoRepoProcessor::run() pattern without changing data structures
struct ConfiguredRepoSession;

impl ConfiguredRepoSession {
    /// Run an operation with automatic setup and teardown
    fn run<T, F>(toprepo: &mut ConfiguredTopRepo, f: F) -> Result<T>
    where
        F: FnOnce(&mut repo::ConfiguredTopRepo) -> Result<T>,
    {
        // Setup: Open configured repo (loads cache from disk)
        // let mut configured_repo = repo::TopRepo::open_configured(directory)?;
        toprepo.reload_config()?;
        
        // Run the operation
        let result = f(toprepo);
        
        // Teardown: Always save state (preserves partial work even on errors)
        let save_result = toprepo.save_state();
        
        // Return operation result, but if save failed and operation succeeded, return save error
        match (result, save_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(save_err)) => Err(save_err),
            (Err(op_err), _) => Err(op_err), // Operation error takes precedence
        }
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
    let gitrepo_root = git_toprepo::util::find_current_worktree(&current_dir);
    // NB: Flatten the `Result<Result<RepoHandle, Error>, Error>`
    //     note: see issue #70142 <https://github.com/rust-lang/rust/issues/70142> for more information
    //     help: add `#![feature(result_flattening)]` to the crate attributes to enable
    let repo = match gitrepo_root {
        Ok(repo_root) => repo::TopRepo::open_for_commands(Path::new(&repo_root)),
        Err(e) => Err(e),
    };

    /*
     * // TODO: `dump` would like to have the relative path to the root
     * // for it to calculate relative paths correctly.
     * let down_from_root = current_dir.strip_prefix(&toprepo_root);
     * let up_to_root = Path::new();
     */

    // TODO(terminology #172):
    // We can persist the tracing in the monorepo.
    // TODO: Maybe move the logger into the `ConfiguredTopRepo` as we only want to log
    // in that case.
    match (logger, &repo) {
        (Some(logger), Ok(RepoHandle::Configured(toprepo))) => {
            logger.write_to_git_dir(toprepo.gix_repo.git_dir())?;
        }
        _ => {}
    };


    match args.command {
        // Early exit commands
        Commands::Version => return print_version(),
        Commands::IsMonorepo => {
            // NB: There is no possible way to know if it is a monorepo a
            // priori: We currently only try to open it and if it works it
            // works. We should document when it is a monorepo and implement a
            // static `is_monorepo`: function.
            return match repo {
                Ok(RepoHandle::Configured(_)) => Ok(()),
                _ => Err(ExitSilently.into()),
            };
        }

        Commands::Init(init_args) => {
            init(&init_args).map(|_path| ())?;
            log::info!("The next step is to run 'git-toprepo fetch'.");
            return Ok(());
        }

        Commands::Config(config_args) => {
            return config(&config_args, repo);
        }
        Commands::Dump(dump_args) => dump(&dump_args, repo.map_err(|e| anyhow::Error::from(NotAMonorepo::new(e)))?),
        Commands::Clone(clone_args) => {
            // Two-stage initialization: init + clone_after_init
            let directory = init(&clone_args.init)?;
            let mut toprepo = TopRepo::open_configured(&directory)?;
            logger.map(|logger| logger.write_to_git_dir(toprepo.gix_repo.git_dir())).transpose()?;
            
            ConfiguredRepoSession::run(&mut toprepo, |configured| {
                clone_after_init(&clone_args, configured)
            })
        }
        Commands::Refilter(refilter_args) => {
            ConfiguredRepoSession::run(&mut ConfiguredTopRepo::try_from(repo)?, |configured| {
                refilter(&refilter_args, configured)
            })
        }
        Commands::Fetch(fetch_args) => {
            ConfiguredRepoSession::run(&mut ConfiguredTopRepo::try_from(repo)?, |configured| {
                fetch(&fetch_args, configured)
            })
        }
        Commands::Push(push_args) => {
            ConfiguredRepoSession::run(&mut ConfiguredTopRepo::try_from(repo)?, |configured| {
                push(&push_args, configured)
            })
        }

        // Experimental and scaffolding commands.
        Commands::Checkout(ref checkout_args) => checkout(&args, checkout_args),
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
    use git_toprepo::error::NotAMonorepo;

    #[test]
    fn test_main_outside_git_toprepo() {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_main_outside_git_toprepo",
        );
        let temp_dir_str = temp_dir.to_str().unwrap();
        let argv = vec!["git-toprepo", "-C", temp_dir_str, "config", "show"];
        let argv = argv.into_iter().map(|s| s.into());
        let err = main_impl(argv, None).unwrap_err();
        // NB: This is linked to     dump::test_dump_outside_git_repo
        // The error is now wrapped with context, so we need to find the root cause
        assert!(err.downcast_ref::<NotAMonorepo>().is_some());
    }

    #[test]
    fn test_main_in_uninitialized_git_toprepo() {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::new_with_prefix(
            "git_toprepo-test_main_in_uninitialized_git_toprepo",
        );
        let temp_dir_str = temp_dir.to_str().unwrap();
        let mut init_cmd = git_command(&temp_dir.to_path_buf().to_owned());
        let _ = init_cmd.arg("init").output();
        let argv = vec!["git-toprepo", "-C", temp_dir_str, "config", "show"];
        let argv = argv.into_iter().map(|s| s.into());
        let err = main_impl(argv, None).unwrap_err();
        assert!(err.downcast_ref::<NotAMonorepo>().is_some());

        // TODO: Should there be distinctly different error messages in a
        // unassembled gitrepo, or without a git repo entirely?
    }

    // TODO: Check that the formatting of the NotAMonorepo is visually appealing.

    /* TODO: We should also make sure that a running git-toprepo
     * inside a proper checked out submodule fails.
     * In there regular git should be used.
    #[test]
    fn test_main_inside_a_proper_submodule_to_a_git_toprepo() {
    }
    */
}
