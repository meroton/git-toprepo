use crate::config::GetOrInsertOk;
use crate::config::GitTopRepoConfig;
use crate::config::SubRepoConfig;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git_fast_export_import::FastExportCommit;
use crate::git_fast_export_import::FastExportEntry;
use crate::git_fast_export_import::FastExportRepo;
use crate::git_fast_export_import::FileChange;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::InterruptedError;
use crate::log::InterruptedResult;
use crate::repo::ExportedFileEntry;
use crate::repo::RepoData;
use crate::repo::RepoStates;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::ThinSubmoduleContent;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::ui::ProgressStatus;
use crate::ui::ProgressTaskHandle;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use bstr::ByteSlice as _;
use gix::ObjectId;
use gix::refs::FullName;
use itertools::Itertools as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tracing::instrument;

/// A mapping of commit ids to their potential git-refs. This is useful when
/// printing log messages as they then can include the refs the user need to
/// look into.
type CommitToRefMap = HashMap<CommitId, Vec<FullName>>;

/// A mapping of submodule filter/expansion status.
/// Moved out of `GitTopRepoConfig` and duplicates some data.
/// This will be mutated during the filter/expansion algorithm
/// and the `monorepo` object will have implementation details tracked for
/// submodules in here.
// TODO: Split the shared `SubRepoConfig` to be specific to the two different
// usecases.
pub struct SubRepoLedger {
    pub subrepos: BTreeMap<SubRepoName, SubRepoConfig>,

    /// List of subrepos that are missing in the configuration and have
    /// automatically been added to `suprepos`.
    pub missing_subrepos: HashSet<SubRepoName>,
}

impl SubRepoLedger {
    /// Gets a `SubRepoConfig` based on a URL using exact matching. If an URL is
    /// missing, the user should add it to the `SubRepoConfig::urls` list.
    pub fn get_name_from_url(&self, url: &gix::Url) -> Result<Option<SubRepoName>> {
        let mut matches = self
            .subrepos
            .iter()
            .filter(|(_name, subrepo_config)| subrepo_config.urls.iter().any(|u| u == url));
        let Some(first_match) = matches.next() else {
            return Ok(None);
        };
        if let Some(second_match) = matches.next() {
            let names = [first_match, second_match]
                .into_iter()
                .chain(matches)
                .map(|(name, _)| name)
                .join(", ");
            bail!("Multiple remote candidates for {url}: {names}");
        }
        // Only a single match.
        let repo_name = first_match.0;
        if self.missing_subrepos.contains(repo_name) {
            Ok(None)
        } else {
            Ok(Some(repo_name.clone()))
        }
    }

    pub fn default_name_from_url(&self, repo_url: &gix::Url) -> Option<SubRepoName> {
        // TODO: UTF-8 validation.
        let mut name: &str = &repo_url.path.to_str_lossy();
        if name.ends_with(".git") {
            name = &name[..name.len() - 4];
        } else if name.ends_with("/") {
            name = &name[..name.len() - 1];
        }
        loop {
            if name.starts_with("../") {
                name = &name[3..];
            } else if name.starts_with("./") {
                name = &name[2..];
            } else if name.starts_with("/") {
                name = &name[1..];
            } else {
                break;
            }
        }
        let name = name.replace("/", "_");
        match RepoName::new(name) {
            RepoName::Top => None,
            RepoName::SubRepo(name) => Some(name),
        }
    }

    /// Get a repo name given a full url when doing an approximative matching,
    /// for example matching `ssh://foo/bar.git` with `https://foo/bar`.
    pub fn get_name_from_similar_full_url(
        &self,
        wanted_full_url: gix::Url,
        base_url: &gix::Url,
    ) -> Result<RepoName> {
        let wanted_url_str = wanted_full_url.to_string();
        let trimmed_wanted_full_url = wanted_full_url.trim_url_path();
        let mut matching_names = Vec::new();
        if trimmed_wanted_full_url.approx_equal(&base_url.clone().trim_url_path()) {
            matching_names.push(RepoName::Top);
        }
        for (submod_name, submod_config) in self.subrepos.iter() {
            if submod_config.urls.iter().any(|submod_url| {
                let full_submod_url = base_url.join(submod_url).trim_url_path();
                full_submod_url.approx_equal(&trimmed_wanted_full_url)
            }) {
                matching_names.push(RepoName::SubRepo(submod_name.clone()));
            }
        }
        matching_names.sort();
        let repo_name = match matching_names.as_slice() {
            [] => anyhow::bail!("No configured submodule URL matches {wanted_url_str:?}"),
            [repo_name] => repo_name.clone(),
            [_, ..] => anyhow::bail!(
                "URLs from multiple configured repos match: {}",
                matching_names
                    .iter()
                    .map(|name| name.to_string())
                    .join(", ")
            ),
        };
        Ok(repo_name)
    }

    /// Get a subrepo configuration without creating a new entry if missing.
    pub fn get_from_url(
        &self,
        repo_url: &gix::Url,
    ) -> Result<Option<(SubRepoName, &SubRepoConfig)>> {
        match self.get_name_from_url(repo_url)? {
            Some(repo_name) => {
                let subrepo_config = self.subrepos.get(&repo_name).expect("valid subrepo name");
                Ok(Some((repo_name, subrepo_config)))
            }
            None => Ok(None),
        }
    }


    /// Get a subrepo configuration or create a new entry if missing.
    pub fn get_or_insert_from_url<'a>(
        &'a mut self,
        repo_url: &gix::Url,
    ) -> Result<GetOrInsertOk<'a>> {
        let Some(repo_name) = self.get_name_from_url(repo_url)? else {
            let mut repo_name = self.default_name_from_url(repo_url).with_context(|| {
                format!(
                    "URL {repo_url} cannot be automatically converted to a valid repo name. \
                    Please create a manual config entry with the URL."
                )
            })?;
            // Instead of just self.subrepos.get(&repo_name), also check for
            // case insensitive repo name uniqueness. It's confusing for the
            // user to get multiple repos with the same name and not
            // realising that it's just the casing that is different.
            // Manually adding multiple entries with different casing is
            // allowed but not recommended.
            for existing_name in self.subrepos.keys() {
                if repo_name.to_lowercase() == existing_name.to_lowercase() {
                    repo_name = existing_name.clone();
                }
            }
            let urls = &mut self.subrepos.entry(repo_name.clone()).or_default().urls;
            if !urls.contains(repo_url) {
                urls.push(repo_url.clone());
            }
            return Ok(if self.missing_subrepos.insert(repo_name.clone()) {
                GetOrInsertOk::Missing(repo_name.clone())
            } else {
                GetOrInsertOk::MissingAgain(repo_name.clone())
            });
        };
        let subrepo_config = self
            .subrepos
            .get_mut(&repo_name)
            .expect("valid subrepo name");
        Ok(GetOrInsertOk::Found((repo_name, subrepo_config)))
    }
}

enum TaskResult {
    RepoFetchDone {
        repo_name: RepoName,
        /// A reference to the UI progress line.
        progress_task: ProgressTaskHandle,
        /// The result of the fetch.
        result: Result<()>,
    },
    LoadCachedCommits {
        repo_name: RepoName,
        /// Commit ids to load and their potential git-refs. All ancestors should also be loaded.
        cached_commits_to_load: CommitToRefMap,
        /// A channel to send the result of the loading process.
        result_channel: oneshot::Sender<Result<()>>,
    },
    ImportCommit {
        repo_name: Arc<RepoName>,
        commit: FastExportCommit,
        tree_id: TreeId,
        /// git-refs for the commit to be imported.
        ref_names: Vec<FullName>,
    },
    LoadRepoDone {
        repo_name: RepoName,
        /// A reference to the UI progress line.
        progress_task: ProgressTaskHandle,
        /// The result of loading a repository, potentially interrupted.
        result: InterruptedResult<()>,
    },
}

#[derive(Debug)]
struct NeededCommit {
    /// Repository the needed commit is in.
    pub repo_name: SubRepoName,
    /// Needed submodule commit.
    pub commit_id: CommitId,
}

#[derive(Debug, Clone, Copy)]
struct CommitLogLevel {
    /// Used for a branch tip which can be fixed.
    pub is_tip: bool,
    /// Hide warnings that config-bootstrap should not log.
    // TODO: Can this be refactored somewhere else?
    pub log_missing_repo_configs: bool,
    /// The log level for messages that belong to a commit.
    pub level: log::Level,
}

pub struct CommitLoader<'a> {
    toprepo: &'a gix::Repository,
    repos: HashMap<RepoName, RepoFetcher>,
    /// Repositories that have been loaded from the cache.
    cached_repo_states: &'a mut RepoStates,
    // TODO: remove config.
    config: &'a GitTopRepoConfig,
    ledger: &'a mut SubRepoLedger,

    tx: std::sync::mpsc::Sender<TaskResult>,
    rx: std::sync::mpsc::Receiver<TaskResult>,
    event_queue: VecDeque<TaskResult>,

    import_progress: indicatif::ProgressBar,
    load_progress: ProgressStatus,
    fetch_progress: ProgressStatus,
    /// Signal to not start new work but to fail as fast as possible.
    error_observer: &'a crate::log::ErrorObserver,
    pub log_missing_config_warnings: bool,

    thread_pool: threadpool::ThreadPool,
    ongoing_jobs_in_threads: usize,

    /// Flag if the repository content should be loaded after fetch is done.
    pub load_after_fetch: bool,
    /// Flag if submodule commits that are missing should be fetched.
    pub fetch_missing_commits: bool,

    repos_to_load: VecDeque<RepoName>,
    repos_to_fetch: VecDeque<RepoName>,

    /// A cache of parsed `.gitmodules` files.
    dot_gitmodules_cache: DotGitModulesCache<'a>,
}

impl<'a> CommitLoader<'a> {
    pub fn new(
        toprepo: &'a gix::Repository,
        cached_repo_states: &'a mut RepoStates,
        config: &'a GitTopRepoConfig,
        ledger: &'a mut SubRepoLedger,
        progress: indicatif::MultiProgress,
        error_observer: &'a crate::log::ErrorObserver,
        thread_pool: threadpool::ThreadPool,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<TaskResult>();
        let import_progress = progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::with_template("{elapsed:>4} {prefix:.cyan}: {pos}")
                        .unwrap(),
                )
                .with_prefix("Importing commits"),
        );
        // Make sure that the elapsed time is updated continuously.
        import_progress.enable_steady_tick(std::time::Duration::from_millis(1000));

        let style = indicatif::ProgressStyle::with_template(
            "     {prefix:.cyan} [{bar:24}] {pos}/{len}{wide_msg}",
        )
        .unwrap()
        .progress_chars("=> ");
        let load_progress = ProgressStatus::new(
            progress.clone(),
            progress.add(
                indicatif::ProgressBar::no_length()
                    .with_style(style.clone())
                    .with_prefix("Loading "),
            ),
        );
        let fetch_progress = ProgressStatus::new(
            progress.clone(),
            progress.add(
                indicatif::ProgressBar::no_length()
                    .with_style(style)
                    .with_prefix("Fetching"),
            ),
        );
        Ok(Self {
            toprepo,
            repos: HashMap::new(),
            cached_repo_states,
            config,
            ledger,
            tx,
            rx,
            event_queue: VecDeque::new(),
            import_progress,
            load_progress,
            fetch_progress,
            error_observer,
            log_missing_config_warnings: true,
            thread_pool,
            ongoing_jobs_in_threads: 0,
            load_after_fetch: true,
            fetch_missing_commits: true,
            repos_to_fetch: VecDeque::new(),
            repos_to_load: VecDeque::new(),
            dot_gitmodules_cache: DotGitModulesCache {
                repo: toprepo,
                cache: HashMap::new(),
            },
        })
    }

    /// Enqueue fetching of a repo with specific refspecs. Using `None` as
    /// refspec will fetch all heads and tags.
    pub fn fetch_repo(&mut self, repo_name: RepoName) -> Result<()> {
        let result = self.fetch_repo_impl(repo_name);
        self.error_observer.maybe_consume(result)
    }

    fn fetch_repo_impl(&mut self, repo_name: RepoName) -> Result<()> {
        let repo_fetcher = self.get_or_create_repo_fetcher(&repo_name)?;
        if !repo_fetcher.enabled {
            log::warn!("Repo {repo_name} is disabled in the configuration, will not fetch");
            return Ok(());
        }
        if repo_fetcher.fetch_state == RepoFetcherState::Idle {
            repo_fetcher.fetch_state = RepoFetcherState::Queued;
            self.repos_to_fetch.push_back(repo_name);
        }
        Ok(())
    }

    /// Enqueue loading commits from a repo.
    pub fn load_repo(&mut self, repo_name: &RepoName) -> Result<()> {
        let repo_fetcher = self.get_or_create_repo_fetcher(repo_name)?;
        if !repo_fetcher.enabled {
            log::warn!("Repo {repo_name} is disabled in the configuration, will not load commits");
            return Ok(());
        }
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet | LoadRepoState::Done => {
                repo_fetcher.loading = LoadRepoState::LoadingThenDone;
                self.repos_to_load.push_back(repo_name.clone());
            }
            LoadRepoState::LoadingThenQueueAgain => (),
            // If loading and this call is because a fetch has finished, or some
            // other external update of the repo, all refs might not have been
            // accounted for. Need to load again when done.
            LoadRepoState::LoadingThenDone => {
                repo_fetcher.loading = LoadRepoState::LoadingThenQueueAgain;
            }
        }
        Ok(())
    }

    /// Waits for all ongoing tasks to finish.
    pub fn join(mut self) -> Result<()> {
        let result = (|| {
            while !self.error_observer.should_interrupt() {
                if !self.process_one_event()? {
                    return Ok(());
                }
            }
            Ok(())
        })();
        self.thread_pool.join();
        self.cached_repo_states.clear();
        self.cached_repo_states.extend(
            self.repos
                .into_iter()
                .map(|(repo_name, repo_fetcher)| (repo_name, repo_fetcher.repo_data)),
        );
        log::info!(
            "Finished importing commits in {:.2?}",
            self.import_progress.elapsed()
        );
        result
    }

    /// Receives one event and processes it. Returns true if there are more
    /// events to be expected.
    pub fn process_one_event(&mut self) -> Result<bool> {
        self.fetch_progress
            .set_queue_size(self.repos_to_fetch.len());
        self.load_progress.set_queue_size(self.repos_to_load.len());

        // Start work if possible.
        if self.ongoing_jobs_in_threads < self.thread_pool.max_count() {
            if let Some(repo_name) = self.repos_to_fetch.pop_front() {
                match self.start_fetch_repo_job(repo_name.clone()) {
                    Ok(()) => {
                        self.ongoing_jobs_in_threads += 1;
                    }
                    Err(err) => {
                        // Fail unless keep-going.
                        self.error_observer.maybe_consume(Err(err))?;
                    }
                }
                return Ok(true);
            }
            if let Some(repo_name) = self.repos_to_load.pop_front() {
                self.start_load_repo_job(repo_name.clone());
                self.ongoing_jobs_in_threads += 1;
                return Ok(true);
            }
        }

        // Receive messages.
        while let Ok(msg) = self.rx.try_recv() {
            self.event_queue.push_back(msg);
        }
        let msg = match self.event_queue.pop_front() {
            Some(msg) => msg,
            None => {
                if self.ongoing_jobs_in_threads == 0 {
                    // No more work to do and no more messages. Shutdown.
                    return Ok(false);
                }
                // Blocking message fetching.
                loop {
                    match self.rx.recv_timeout(std::time::Duration::from_secs(1)) {
                        Ok(msg) => break msg,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            // The sender has been dropped, no more events will come.
                            unreachable!();
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if self.thread_pool.panic_count() != 0 {
                                return Err(anyhow::anyhow!(
                                    "A worker thread has panicked, aborting"
                                ));
                            }
                        }
                    };
                }
            }
        };

        // Process one messages.
        match msg {
            TaskResult::RepoFetchDone {
                repo_name,
                progress_task,
                result,
            } => {
                self.ongoing_jobs_in_threads -= 1;
                drop(progress_task); // Remove the progress bar from the UI.
                match result {
                    Ok(()) => {
                        self.finish_fetch_repo_job(&repo_name);
                    }
                    Err(err) => {
                        self.error_observer.maybe_consume(Err(err))?;
                    }
                }
            }
            TaskResult::LoadCachedCommits {
                repo_name,
                cached_commits_to_load,
                result_channel,
            } => {
                let result = match self.load_cached_commits(&repo_name, cached_commits_to_load) {
                    Ok(()) => Ok(()),
                    Err(InterruptedError::Interrupted) => {
                        // The caller can continue processing if they want.
                        return Ok(true);
                    }
                    Err(InterruptedError::Normal(err)) => Err(err),
                };
                result_channel
                    .send(result)
                    .expect("result from loading cached commits has not been set yet");
            }
            TaskResult::ImportCommit {
                repo_name,
                commit,
                tree_id,
                ref_names,
            } => {
                match self.import_commit(&repo_name, commit, tree_id, &ref_names) {
                    Ok(()) => self.import_progress.inc(1),
                    Err(InterruptedError::Interrupted) => {
                        // The caller can continue processing if they want.
                        return Ok(true);
                    }
                    Err(InterruptedError::Normal(err)) => {
                        self.error_observer.maybe_consume(Err(err))?;
                    }
                }
            }
            TaskResult::LoadRepoDone {
                repo_name,
                progress_task,
                result,
            } => {
                self.ongoing_jobs_in_threads -= 1;
                drop(progress_task); // Remove the progress bar from the UI.
                match result {
                    Ok(()) => {
                        let result = self.finish_load_repo_job(&repo_name);
                        self.error_observer.maybe_consume(result)?;
                    }
                    Err(InterruptedError::Interrupted) => {
                        // This doesn't mean there are no more events to process.
                    }
                    Err(InterruptedError::Normal(err)) => {
                        self.error_observer.maybe_consume(Err(err))?;
                    }
                }
            }
        }
        Ok(true)
    }

    fn start_fetch_repo_job(&mut self, repo_name: RepoName) -> Result<()> {
        let context = format!("Fetching {repo_name}");
        let _log_scope_guard = crate::log::scope(context.clone());

        let repo_fetcher = self.repos.get_mut(&repo_name).unwrap();
        assert_eq!(repo_fetcher.fetch_state, RepoFetcherState::Queued);
        repo_fetcher.fetch_state = RepoFetcherState::InProgress;

        let mut fetcher = crate::fetch::RemoteFetcher::new(self.toprepo);
        fetcher.set_remote_from_repo_name(self.toprepo, &repo_name, self.ledger)?;

        let pb_url = indicatif::ProgressBar::hidden()
            .with_style(
                indicatif::ProgressStyle::with_template("{elapsed:>4} {prefix:.cyan} {msg}")
                    .unwrap(),
            )
            .with_prefix("git fetch")
            .with_message(fetcher.remote.clone().unwrap_or_else(|| "<top>".to_owned()));
        let pb_status = indicatif::ProgressBar::hidden()
            .with_style(indicatif::ProgressStyle::with_template("     {msg}").unwrap());
        // Now when it is added, we can start calling tick to print. Don't print
        // before it is added as multiple ProgressBars will trash eachother.
        pb_url.enable_steady_tick(std::time::Duration::from_millis(1000));

        let progress_task = self
            .fetch_progress
            .start(repo_name.to_string(), vec![pb_url, pb_status.clone()]);

        let tx = self.tx.clone();
        let error_observer = self.error_observer.clone();
        let log_context = crate::log::current_scope();
        let parent_span = tracing::Span::current();
        let idle_timeouts = self.config.fetch.get_idle_timeouts();
        self.thread_pool.execute(move || {
            let _log_scope_guard = crate::log::scope(log_context);
            let _span_guard =
                tracing::info_span!(parent: parent_span, "Fetching", "repo" = %repo_name).entered();
            let result = fetcher
                .fetch_with_progress_bar(&pb_status, &idle_timeouts)
                .with_context(|| format!("Fetching {repo_name}"));
            // Sending might fail on interrupt.
            if let Err(err) = tx.send(TaskResult::RepoFetchDone {
                repo_name,
                progress_task,
                result,
            }) {
                assert!(
                    error_observer.should_interrupt(),
                    "The receiver should only close early when interrupted: {err:?}"
                );
            }
        });
        Ok(())
    }

    fn finish_fetch_repo_job(&mut self, repo_name: &RepoName) {
        let repo_fetcher = self.repos.get_mut(repo_name).unwrap();
        assert_eq!(repo_fetcher.fetch_state, RepoFetcherState::InProgress);
        repo_fetcher.fetch_state = RepoFetcherState::Done;

        // Load the fetched data.
        if self.load_after_fetch {
            self.load_repo(repo_name)
                .expect("configuration exists for repo");
        }
    }

    /// Loads basic information, i.e. `ThinCommit` information, about all
    /// reachable commits, from `refs/namespaces/{repo_name}/*`, and their
    /// referenced submodules and stores them in `storage`. Commits that are
    /// already in `storage` are skipped.
    fn start_load_repo_job(&self, repo_name: RepoName) {
        let context = format!("Loading commits in {repo_name}");
        let _log_scope_guard = crate::log::scope(context.clone());

        // Use the main thread when getting the refs to make use of the gix cached
        let existing_commits: HashSet<_> = self
            .repos
            .get(&repo_name)
            .unwrap()
            .repo_data
            .thin_commits
            .keys()
            .cloned()
            .collect();
        let cached_commits: HashSet<_> = self
            .cached_repo_states
            .get(&repo_name)
            .map(|repo_data| repo_data.thin_commits.keys().cloned().collect())
            .unwrap_or_default();

        let progress_task = self.load_progress.start(repo_name.to_string(), Vec::new());

        let toprepo = self.toprepo.clone();
        let tx = self.tx.clone();
        let error_observer = self.error_observer.clone();
        let log_context = crate::log::current_scope();
        let parent_span = tracing::Span::current();
        self.thread_pool.execute(move || {
            let _log_scope_guard = crate::log::scope(log_context);
            let _span_guard =
                tracing::info_span!(parent: parent_span, "Loading", "repo" = %repo_name).entered();
            let single_repo_loader = SingleRepoLoader {
                toprepo: &toprepo,
                repo_name: &repo_name,
                error_observer: &error_observer,
            };

            struct LoadRepoCallback {
                tx: std::sync::mpsc::Sender<TaskResult>,
                repo_name: Arc<RepoName>,
            }
            impl SingleLoadRepoCallback for LoadRepoCallback {
                fn load_cached_commits(
                    &self,
                    cached_commits_to_load: CommitToRefMap,
                ) -> InterruptedResult<()> {
                    let (result_tx, result_rx) = oneshot::channel();
                    self.tx
                        .send(TaskResult::LoadCachedCommits {
                            repo_name: (*self.repo_name).clone(),
                            cached_commits_to_load,
                            result_channel: result_tx,
                        })
                        // The receiver only closes to fail fast.
                        .map_err(|_| InterruptedError::Interrupted)?;
                    result_rx
                        .recv()
                        // The sender only closes to fail fast.
                        .map_err(|_| InterruptedError::Interrupted)?
                        .map_err(InterruptedError::Normal)
                }

                fn import_commit(
                    &self,
                    commit: FastExportCommit,
                    tree_id: TreeId,
                    ref_names: &[FullName],
                ) -> InterruptedResult<()> {
                    self.tx
                        .send(TaskResult::ImportCommit {
                            repo_name: self.repo_name.clone(),
                            commit,
                            tree_id,
                            ref_names: Vec::from(ref_names),
                        })
                        // The receiver only closes to fail fast.
                        .map_err(|_| InterruptedError::Interrupted)?;
                    Ok(())
                }
            }
            let callback = LoadRepoCallback {
                tx: tx.clone(),
                repo_name: Arc::new(repo_name.clone()),
            };

            let result =
                single_repo_loader.load_repo(&existing_commits, &cached_commits, &callback);
            // Send only fails if the receiver has been interrupted.
            if let Err(err) = tx.send(TaskResult::LoadRepoDone {
                repo_name,
                progress_task,
                result,
            }) {
                assert!(
                    error_observer.should_interrupt(),
                    "The receiver should only close early when interrupted: {err:?}"
                );
            }
        });
    }

    fn finish_load_repo_job(&mut self, repo_name: &RepoName) -> Result<()> {
        let repo_fetcher = self.repos.get_mut(repo_name).unwrap();
        // Load again?
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet => unreachable!(),
            LoadRepoState::LoadingThenQueueAgain => {
                self.repos_to_load.push_back(repo_name.clone());
                repo_fetcher.loading = LoadRepoState::LoadingThenDone;
            }
            LoadRepoState::LoadingThenDone => {
                repo_fetcher.loading = LoadRepoState::Done;
                repo_fetcher.needed_commits.retain(|commit_id| {
                    // Keep the commits that are not yet loaded.
                    !repo_fetcher.repo_data.thin_commits.contains_key(commit_id)
                });
                if self.fetch_missing_commits && !repo_fetcher.needed_commits.is_empty() {
                    if repo_fetcher.fetching_default_refspec_done() {
                        let missing_commits = std::mem::take(&mut repo_fetcher.needed_commits);
                        Self::add_missing_commits(
                            repo_name,
                            missing_commits,
                            &mut repo_fetcher.missing_commits,
                        );
                    } else {
                        self.fetch_repo(repo_name.clone())?;
                    }
                }
            }
            LoadRepoState::Done => unreachable!(),
        }
        Ok(())
    }

    fn add_missing_commits(
        repo_name: &RepoName,
        missing_commits: HashSet<CommitId>,
        all_missing_commits: &mut HashSet<CommitId>,
    ) {
        // Already logged when loading the repositories.
        for commit_id in &missing_commits {
            log::warn!("Missing commit in {repo_name}: {commit_id}");
        }
        all_missing_commits.extend(missing_commits);
    }

    fn load_cached_commits(
        &mut self,
        repo_name: &RepoName,
        cached_commits_to_load: CommitToRefMap,
    ) -> InterruptedResult<()> {
        // Load from cache, should be quick.
        let Some(cached_repo) = self.cached_repo_states.remove(repo_name) else {
            return Ok(());
        };
        let repo_fetcher = self
            .repos
            .get_mut(repo_name)
            .expect("repo_fetcher already exists");
        if repo_fetcher.repo_data.url != cached_repo.url {
            return Err(anyhow::anyhow!(
                "Cached URL was {} instead of {}",
                cached_repo.url,
                repo_fetcher.repo_data.url,
            )
            .into());
        }
        let reverse_dedup_cache = cached_repo
            .dedup_cache
            .into_iter()
            .map(|(commit_id, thin_commit_id)| (thin_commit_id, commit_id))
            .collect::<HashMap<_, _>>();

        let mut needed_commits = Vec::new();
        let mut todo = cached_commits_to_load
            .keys()
            .map(|commit_id| {
                cached_repo
                    .thin_commits
                    .get(commit_id)
                    .expect("commit_id is in cache")
                    .clone()
            })
            .collect_vec();
        // Make sure to load the commits with smallest depth first because there
        // is no existence check when pushing processing commits, only when
        // pushing into the todo stack.
        todo.sort_by_key(|c| std::cmp::Reverse(c.depth));
        let result = (|| {
            while let Some(thin_commit) = todo.last().cloned() {
                // Process parents first.
                let mut missing_parents = false;
                for parent in &thin_commit.parents {
                    if repo_fetcher
                        .repo_data
                        .thin_commits
                        .contains_key(&parent.commit_id)
                    {
                        continue;
                    }
                    missing_parents = true;
                    todo.push(parent.clone());
                }
                if missing_parents {
                    continue;
                }
                // All parents have been process previously.
                let thin_commit = todo.pop().expect("at least one element exists");
                let mut context = format!("Commit {} in {}", thin_commit.commit_id, repo_name);
                let commit_log_level = if let Some(ref_names) =
                    cached_commits_to_load.get(&thin_commit.commit_id)
                    && !ref_names.is_empty()
                {
                    context += &format!(" ({})", ref_names.iter().sorted().join(", "));
                    CommitLogLevel {
                        is_tip: true,
                        log_missing_repo_configs: self.log_missing_config_warnings,
                        level: log::Level::Warn,
                    }
                } else {
                    CommitLogLevel {
                        is_tip: false,
                        log_missing_repo_configs: self.log_missing_config_warnings,
                        level: log::Level::Trace,
                    }
                };

                let _log_scope_guard = crate::log::scope(context.clone());
                Self::verify_cached_commit(
                    &repo_fetcher.repo_data,
                    thin_commit.as_ref(),
                    self.ledger,
                    &mut self.dot_gitmodules_cache,
                    commit_log_level,
                )
                .context(context)?;
                // Load all submodule commits as well as that would be done if not
                // using the cache.
                for bump in thin_commit.submodule_bumps.values() {
                    if let ThinSubmodule::AddedOrModified(bump) = bump
                        && let Some(submod_repo_name) = &bump.repo_name
                    {
                        let subconfig = self
                            .config
                            .subrepos
                            .get(submod_repo_name)
                            .expect("subrepo name exists");
                        if !subconfig.skip_expanding.contains(&bump.commit_id) {
                            needed_commits.push(NeededCommit {
                                repo_name: submod_repo_name.clone(),
                                commit_id: bump.commit_id,
                            });
                        }
                    }
                }
                // TODO: The without_committer_id for thin_commit might not be
                // loaded if the duplicate is not loaded. Should
                // without_committer_id be part of thin_commit instead?
                if let Some(without_committer_id) = reverse_dedup_cache.get(&thin_commit.commit_id)
                {
                    repo_fetcher
                        .repo_data
                        .dedup_cache
                        .insert(without_committer_id.clone(), thin_commit.commit_id);
                }
                repo_fetcher
                    .repo_data
                    .thin_commits
                    .insert(thin_commit.commit_id, thin_commit);
            }
            Ok(())
        })();
        self.load_progress.inc_num_cached_done();
        self.error_observer.maybe_consume(result)?;
        // Load all submodule commits that are needed so far.
        for needed_commit in needed_commits {
            let result = self.ensure_commit_available(needed_commit);
            self.error_observer.maybe_consume(result)?;
        }
        Ok(())
    }

    fn import_commit(
        &mut self,
        repo_name: &RepoName,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
        ref_names: &[FullName],
    ) -> InterruptedResult<()> {
        let mut context = format!("Commit {} in {}", exported_commit.original_id, repo_name);
        if !ref_names.is_empty() {
            let ref_names_str = ref_names.iter().sorted().join(", ");
            context += &format!(" ({ref_names_str})");
        }
        let is_tip = !ref_names.is_empty();

        let _log_scope_guard = crate::log::scope(context.clone());
        let hash_without_committer = exported_commit.hash_without_committer()?;
        let repo_data = &mut self.repos.get_mut(repo_name).unwrap().repo_data;
        let (thin_commit, updated_submodule_commits) = Self::export_thin_commit(
            repo_data,
            exported_commit,
            tree_id,
            self.ledger,
            &mut self.dot_gitmodules_cache,
            CommitLogLevel {
                is_tip,
                log_missing_repo_configs: self.log_missing_config_warnings,
                level: if is_tip {
                    log::Level::Warn
                } else {
                    log::Level::Trace
                },
            },
        )
        .context(context)
        .map_err(InterruptedError::Normal)?;
        // Always overwrite the cache entry with the newest commit with
        // the same key. It makes most sense to reuse the newest commit
        // available.
        repo_data
            .dedup_cache
            .insert(hash_without_committer, thin_commit.commit_id);
        // Insert it into the storage.
        repo_data
            .thin_commits
            .entry(thin_commit.commit_id)
            .or_insert(thin_commit);

        // Any of the submodule updates that need to be fetched?
        for needed_commit in updated_submodule_commits {
            let result = self.ensure_commit_available(needed_commit);
            self.error_observer.maybe_consume(result)?;
        }
        Ok(())
    }

    fn verify_cached_commit(
        repo_storage: &RepoData,
        commit: &ThinCommit,
        ledger: &mut SubRepoLedger,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        commit_log_level: CommitLogLevel,
    ) -> Result<()> {
        let submodule_paths_to_check = if commit_log_level.is_tip
            || commit
                .parents
                .first()
                .is_none_or(|first_parent| commit.dot_gitmodules != first_parent.dot_gitmodules)
        {
            &commit
                .submodule_paths
                .iter()
                .map(|path| {
                    (
                        path.clone(),
                        commit
                            .get_submodule(path)
                            .expect("submodule exists")
                            .clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        } else {
            &commit.submodule_bumps
        };
        for (path, bump) in submodule_paths_to_check {
            match bump {
                ThinSubmodule::AddedOrModified(cached_thin_submod) => {
                    let submod_repo_name = Self::get_submod_repo_name(
                        commit.dot_gitmodules,
                        commit.parents.first(),
                        path,
                        &repo_storage.url,
                        ledger,
                        dot_gitmodules_cache,
                        commit_log_level,
                    );
                    if submod_repo_name != cached_thin_submod.repo_name {
                        anyhow::bail!(
                            "Submodule {path} was cached as repo {:?} but is now {:?}",
                            cached_thin_submod.repo_name,
                            submod_repo_name
                        );
                    }
                }
                ThinSubmodule::Removed => (),
            }
        }
        Ok(())
    }

    fn get_or_create_repo_fetcher(
        &'_ mut self,
        repo_name: &RepoName,
    ) -> Result<&'_ mut RepoFetcher> {
        if !self.repos.contains_key(repo_name) {
            self.create_repo_fetcher(repo_name)?;
        }
        Ok(self
            .repos
            .get_mut(repo_name)
            .expect("just added repo fetcher"))
    }

    /// Creates a `RepoFetcher` for the given repository name. If `repo_name`
    /// doesn't exist in the configuration, the creation will fail.
    fn create_repo_fetcher(&mut self, repo_name: &RepoName) -> Result<()> {
        let (enabled, url) = match &repo_name {
            RepoName::Top => (true, crate::util::EMPTY_GIX_URL.clone()),
            RepoName::SubRepo(submod_repo_name) => {
                // Check if the submodule is configured.
                let submod_contig = self
                    .config
                    .subrepos
                    .get(submod_repo_name)
                    .with_context(|| format!("Repo {repo_name} not found in config"))?;
                let fetch_url = submod_contig.resolve_fetch_url();
                (submod_contig.enabled, fetch_url.clone())
            }
        };
        let repo_fetcher = RepoFetcher::new(enabled, url);
        self.repos.insert(repo_name.clone(), repo_fetcher);
        Ok(())
    }

    fn ensure_commit_available(&mut self, needed_commit: NeededCommit) -> Result<()> {
        let repo_name: RepoName = RepoName::SubRepo(needed_commit.repo_name);
        let commit_id = needed_commit.commit_id;
        // Already loaded?
        if !self.repos.contains_key(&repo_name) {
            self.create_repo_fetcher(&repo_name)
                .expect("repo_name has been found in the configuration");
        }
        let repo_fetcher = self
            .repos
            .get_mut(&repo_name)
            .expect("repo_fetch just inserted");
        if repo_fetcher.repo_data.thin_commits.contains_key(&commit_id)
            || repo_fetcher.missing_commits.contains(&commit_id)
        {
            return Ok(());
        }
        if !repo_fetcher.enabled {
            return Ok(());
        }
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet => {
                repo_fetcher.needed_commits.insert(commit_id);
                self.load_repo(&repo_name)
                    .expect("configuration exists for repo");
            }
            LoadRepoState::LoadingThenQueueAgain | LoadRepoState::LoadingThenDone => {
                repo_fetcher.needed_commits.insert(commit_id);
            }
            LoadRepoState::Done => {
                if repo_fetcher.fetching_default_refspec_done() {
                    let mut missing_commits = HashSet::new();
                    missing_commits.insert(commit_id);
                    Self::add_missing_commits(
                        &repo_name,
                        missing_commits,
                        &mut repo_fetcher.missing_commits,
                    );
                } else {
                    repo_fetcher.needed_commits.insert(commit_id);
                    if self.fetch_missing_commits {
                        self.fetch_repo(repo_name)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Converts a `FastExportCommit` to a `ThinCommit`.
    fn export_thin_commit(
        repo_storage: &RepoData,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
        ledger: &mut SubRepoLedger,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        commit_log_level: CommitLogLevel,
    ) -> Result<(Rc<ThinCommit>, Vec<NeededCommit>)> {
        let commit_id: CommitId = exported_commit.original_id;
        let thin_parents = exported_commit
            .parents
            .iter()
            .map(|parent_id| {
                repo_storage
                    .thin_commits
                    .get(parent_id)
                    .with_context(|| {
                        format!("BUG: Parent {parent_id} of {commit_id} not yet parsed")
                    })
                    .cloned()
            })
            .collect::<Result<Vec<_>>>()?;
        let mut submodule_bumps = BTreeMap::new();

        // Check for an updated .gitmodules file.
        let old_dot_gitmodules = thin_parents
            .first()
            .and_then(|first_parent| first_parent.dot_gitmodules);
        let dot_gitmodules =
            Self::get_dot_gitmodules_update(&exported_commit)?.unwrap_or(old_dot_gitmodules); // None means no update of .gitmodules.

        // Look for submodule updates.
        let parent_submodule_paths = match thin_parents.first() {
            Some(first_parent) => &first_parent.submodule_paths,
            None => &HashSet::new(),
        };
        let mut new_submodule_commits = Vec::new();
        for fc in exported_commit.file_changes {
            // TODO: Implement borrow between BStr and GitPath to delay
            // construction of a GitPath.
            let path = GitPath::new(fc.path);
            match fc.change {
                FileChange::Modified { mode, hash } => {
                    if mode == b"160000" {
                        // 160000 means submodule
                        let submod_commit_id: CommitId = ObjectId::from_hex(&hash)?;
                        let submod_repo_name = Self::get_submod_repo_name(
                            dot_gitmodules,
                            thin_parents.first(),
                            &path,
                            &repo_storage.url,
                            ledger,
                            dot_gitmodules_cache,
                            commit_log_level,
                        );
                        if let Some(submod_repo_name) = &submod_repo_name {
                            new_submodule_commits.push(NeededCommit {
                                repo_name: submod_repo_name.clone(),
                                commit_id: submod_commit_id,
                            });
                        }
                        submodule_bumps.insert(
                            path,
                            ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
                                repo_name: submod_repo_name,
                                commit_id: submod_commit_id,
                            }),
                        );
                    } else if parent_submodule_paths.contains(&path) {
                        // It might be a submodule that changed to another
                        // type, e.g. tree or file. Remove it.
                        submodule_bumps.insert(path, ThinSubmodule::Removed);
                    }
                }
                FileChange::Deleted => {
                    if parent_submodule_paths.contains(&path) {
                        submodule_bumps.insert(path, ThinSubmodule::Removed);
                    }
                }
            }
        }
        // If the .gitmodules file was updated, the submodule URLs might have
        // changed. Update which repository each submodule points to.
        //
        // Do this after removing deleted submodules as those entries are likely
        // also gone from .gitmodules.
        //
        // Also do it to get logs for fixable commits, i.e. tips of branches.
        if dot_gitmodules != old_dot_gitmodules || commit_log_level.is_tip {
            // Loop through all submodules to see if any have changed, sorted to
            // get deterministic sorting.
            for path in parent_submodule_paths.iter().sorted() {
                let std::collections::btree_map::Entry::Vacant(entry) =
                    submodule_bumps.entry(path.clone())
                else {
                    // The submodule was updated in this commit and already got
                    // the correct .gitmodules information.
                    continue;
                };
                match thin_parents
                    .first()
                    .expect("parent which added submodule exists")
                    .get_submodule(path)
                    .expect("listed submodule path is a submodule")
                {
                    ThinSubmodule::AddedOrModified(thin_submod) => {
                        let new_repo_name = Self::get_submod_repo_name(
                            dot_gitmodules,
                            thin_parents.first(),
                            path,
                            &repo_storage.url,
                            ledger,
                            dot_gitmodules_cache,
                            commit_log_level,
                        );
                        if new_repo_name != thin_submod.repo_name {
                            // Insert an entry that this submodule has been updated.
                            entry.insert(ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
                                repo_name: new_repo_name,
                                commit_id: thin_submod.commit_id,
                            }));
                        }
                    }
                    ThinSubmodule::Removed => {}
                }
            }
        }
        let thin_commit = ThinCommit::new_rc(
            commit_id,
            tree_id,
            thin_parents,
            dot_gitmodules,
            submodule_bumps,
        );
        Ok((thin_commit, new_submodule_commits))
    }

    /// Returns `Ok(Some(...))` if a `.gitmodules` update was found and
    /// `Ok(None)` if there is no update.
    fn get_dot_gitmodules_update(
        exported_commit: &FastExportCommit,
    ) -> Result<Option<Option<ExportedFileEntry>>> {
        // Assume just a single entry for .gitmodules.
        const GITMODULES_FILE_REMOVED: Option<Option<ExportedFileEntry>> = Some(None);
        for fc in &exported_commit.file_changes {
            if fc.path == b".gitmodules" {
                match &fc.change {
                    FileChange::Modified { mode, hash } => {
                        let dot_gitmodules =
                            ObjectId::from_hex(hash).context("Bad blob id for .gitmodules")?;
                        // Cannot do proper error reporting for bad modes here,
                        // as that is a fixable warning.
                        let mode_str = std::str::from_utf8(mode)
                            .with_context(|| format!("Bad .gitmodules mode {mode:?}"))?;
                        let mode_u32 = u32::from_str_radix(mode_str, 8).with_context(|| {
                            format!("Failed to parse mode {mode_str} for .gitmodules")
                        })?;
                        return Ok(Some(Some(ExportedFileEntry {
                            mode: mode_u32,
                            id: dot_gitmodules,
                        })));
                    }
                    FileChange::Deleted => {
                        return Ok(GITMODULES_FILE_REMOVED);
                    }
                }
            }
        }
        // No .gitmodules update found.
        Ok(None)
    }

    /// Finds the `SubRepoName` for a submodule and logs warnings for potential problems.
    fn get_submod_repo_name(
        dot_gitmodules: Option<ExportedFileEntry>,
        first_parent: Option<&Rc<ThinCommit>>,
        path: &GitPath,
        base_url: &gix::Url,
        ledger: &mut SubRepoLedger,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        commit_log_level: CommitLogLevel,
    ) -> Option<SubRepoName> {
        let do_log = || {
            commit_log_level.is_tip || {
                // Only log if .gitmodules has changed or the submodule was just added.
                let submodule_just_added = first_parent
                    .is_none_or(|first_parent| !first_parent.submodule_paths.contains(path));
                // A missing parent means that .gitmodules has changed.
                let dot_gitmodules_updated = first_parent
                    .is_none_or(|first_parent| dot_gitmodules != first_parent.dot_gitmodules);
                dot_gitmodules_updated || submodule_just_added
            }
        };

        // Parse .gitmodules.
        let Some(dot_gitmodules) = dot_gitmodules else {
            if do_log() {
                log::log!(
                    commit_log_level.level,
                    "Cannot resolve submodule {path}, .gitmodules is missing"
                );
            }
            return None;
        };

        let gitmodules_info = match dot_gitmodules_cache.get_from_blob_id(dot_gitmodules) {
            Ok(gitmodules_info) => gitmodules_info,
            Err(err) => {
                if do_log() {
                    log::log!(commit_log_level.level, "{err:#}");
                }
                return None;
            }
        };

        let submod_url = match gitmodules_info.submodules.get(path) {
            Some(Ok(url)) => url,
            Some(Err(err)) => {
                if do_log() {
                    log::log!(commit_log_level.level, "{err:#}");
                }
                return None;
            }
            None => {
                if do_log() {
                    log::log!(commit_log_level.level, "Missing {path} in .gitmodules");
                }
                return None;
            }
        };
        let full_url = base_url.join(submod_url);
        let name = match ledger
            .get_or_insert_from_url(&full_url)
            .map_err(|err| {
                if do_log() {
                    log::error!("{err:#}");
                }
            })
            .ok()?
        {
            crate::config::GetOrInsertOk::Found((name, _)) => name,
            crate::config::GetOrInsertOk::Missing(_) => {
                if commit_log_level.log_missing_repo_configs && do_log() {
                    log::warn!("URL {full_url} is missing in the git-toprepo configuration");
                }
                return None;
            }
            crate::config::GetOrInsertOk::MissingAgain(_) => return None,
        };
        Some(name)
    }
}

struct SingleRepoLoader<'a> {
    toprepo: &'a gix::Repository,
    repo_name: &'a RepoName,
    error_observer: &'a crate::log::ErrorObserver,
}

trait SingleLoadRepoCallback {
    fn load_cached_commits(&self, cached_commits_to_load: CommitToRefMap) -> InterruptedResult<()>;

    fn import_commit(
        &self,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
        ref_names: &[FullName],
    ) -> InterruptedResult<()>;
}

impl SingleRepoLoader<'_> {
    #[instrument(skip_all, fields(repo_name = %self.repo_name))]
    pub fn load_repo(
        &self,
        existing_commits: &HashSet<CommitId>,
        cached_commits: &HashSet<CommitId>,
        callback: &impl SingleLoadRepoCallback,
    ) -> InterruptedResult<()> {
        let (tips, active_tips_map) = self.get_tips().context("Failed to resolve refs")?;
        let (mut refs_arg, cached_commit_ids_to_load, mut unknown_commit_count) = self
            .get_refs_to_load_arg(&tips, existing_commits, cached_commits)
            .context("Failed to find refs to load")?;
        // Map from commit id to ref names of branch tips for that commit.
        let cached_commits_to_load = cached_commit_ids_to_load
            .into_iter()
            .map(|commit_id| {
                (
                    commit_id,
                    active_tips_map.get(&commit_id).cloned().unwrap_or_default(),
                )
            })
            .collect::<HashMap<_, _>>();
        if !cached_commits_to_load.is_empty()
            && let Err(err) = callback.load_cached_commits(cached_commits_to_load)
        {
            let err = match err {
                InterruptedError::Interrupted => return Err(InterruptedError::Interrupted),
                InterruptedError::Normal(err) => err,
            };
            // Loading the cache failed, try again without the cache. Some of
            // the commits might have been loaded, but this kind of failure
            // should be rare so it is not worth updating existing_commits.
            log::warn!("Discarding cache for {}: {err:#}", self.repo_name);
            let cached_commit_ids_to_load;
            (refs_arg, cached_commit_ids_to_load, unknown_commit_count) = self
                .get_refs_to_load_arg(&tips, existing_commits, &HashSet::new())
                .context("Failed to find refs to load")
                .map_err(InterruptedError::Normal)?;
            assert!(cached_commit_ids_to_load.is_empty());
        }
        if !refs_arg.is_empty() {
            match self.load_from_refs(&active_tips_map, refs_arg, unknown_commit_count, callback) {
                Ok(()) => Ok(()),
                Err(InterruptedError::Interrupted) => Err(InterruptedError::Interrupted),
                Err(InterruptedError::Normal(err)) => {
                    Err(err.context("Failed to load commits").into())
                }
            }?
        }
        Ok(())
    }

    #[instrument(
        name = "get_tips",
        skip_all,
        fields(repo_name = %self.repo_name)
    )]
    fn get_tips(&self) -> Result<(Vec<CommitId>, CommitToRefMap)> {
        let ref_prefix = self.repo_name.to_ref_prefix();
        let mut tips = Vec::new();
        let mut active_tips_map: CommitToRefMap = HashMap::new();
        for r in self
            .toprepo
            .references()?
            .prefixed(BStr::new(ref_prefix.as_bytes()))?
        {
            let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            let Some(object_id) = r.try_id() else {
                // Skip symbolic refs, there is no more information in them
                // to load. Can they might even point to something outside
                // the namespace?
                continue;
            };
            let resolved_object = object_id.object()?.peel_tags_to_end()?;
            let commit_id: CommitId = match resolved_object.kind {
                gix::object::Kind::Commit => resolved_object.id,
                gix::object::Kind::Tag => unreachable!("Tags already peeled"),
                gix::object::Kind::Blob => {
                    log::warn!(
                        "Ignoring {} which points to a blob {object_id}, not to a commit",
                        r.name().as_bstr(),
                    );
                    continue;
                }
                gix::object::Kind::Tree => {
                    log::warn!(
                        "Ignoring {} which points to a tree {object_id}, not to a commit",
                        r.name().as_bstr(),
                    );
                    continue;
                }
            };
            let ref_suffix = r
                .name()
                .as_bstr()
                .strip_prefix(ref_prefix.as_bytes())
                .expect("ref has prefix");
            tips.push(commit_id);
            // Only remotes can be expected to be updated, not refs/tags/,
            // refs/notes/, refs/pull/ etc. which are ignored.
            //
            // Also allowing refs/heads/ in case someone puts their remote branches there.
            if ref_suffix.starts_with("refs/remotes/".as_bytes())
                || ref_suffix.starts_with("refs/heads/".as_bytes())
            {
                let ref_suffix_name = FullName::try_from(ref_suffix.as_bstr())
                    .expect("The ref suffix should be a valid full name");
                active_tips_map
                    .entry(commit_id)
                    .or_default()
                    .push(ref_suffix_name);
            }
        }
        Ok((tips, active_tips_map))
    }

    /// Creates ref listing arguments to give to git, on the form of
    /// `<start_rev> ^<stop_rev>`. An empty list means that there is nothing to
    /// load.
    #[instrument(
        name = "get_refs_to_load",
        skip_all,
        fields(repo_name = %self.repo_name)
    )]
    fn get_refs_to_load_arg(
        &self,
        tips: &Vec<CommitId>,
        existing_commits: &HashSet<CommitId>,
        cached_commits: &HashSet<CommitId>,
    ) -> Result<(Vec<String>, Vec<CommitId>, usize)> {
        let mut unknown_tips: Vec<CommitId> = Vec::new();
        let mut visited_cached_commits = Vec::new();
        for commit_id in tips {
            if existing_commits.contains(commit_id) {
                continue;
            }
            if cached_commits.contains(commit_id) {
                visited_cached_commits.push(*commit_id);
                continue;
            }
            unknown_tips.push(*commit_id);
        }
        if unknown_tips.is_empty() {
            return Ok((vec![], visited_cached_commits, 0));
        }

        let start_refs = unknown_tips
            .iter()
            .map(|id| id.to_hex().to_string())
            .collect::<Vec<_>>();
        let (stop_commit_ids, unknown_commit_count) = crate::git::get_first_known_commits(
            self.toprepo,
            unknown_tips.into_iter(),
            |commit_id| {
                if existing_commits.contains(&commit_id) {
                    return true;
                }
                if cached_commits.contains(&commit_id) {
                    visited_cached_commits.push(commit_id);
                    return true;
                }
                false
            },
        )?;
        let stop_refs = stop_commit_ids.iter().map(|id| format!("^{}", id.to_hex()));

        let refs_arg = start_refs.into_iter().chain(stop_refs).collect();
        Ok((refs_arg, visited_cached_commits, unknown_commit_count))
    }

    #[instrument(
        skip_all,
        fields(repo_name = %self.repo_name)
    )]
    fn load_from_refs(
        &self,
        tips: &CommitToRefMap,
        refs_arg: Vec<String>,
        _unknown_commit_count: usize, // TODO: Remove.
        callback: &impl SingleLoadRepoCallback,
    ) -> InterruptedResult<()> {
        // TODO: The super repository will get an empty URL, which is exactly
        // what is wanted. Does the rest of the code handle that?
        let toprepo_git_dir = self.toprepo.git_dir();
        for export_entry in FastExportRepo::load_from_path(toprepo_git_dir, Some(refs_arg))? {
            if self.error_observer.should_interrupt() {
                break;
            }
            match export_entry? {
                FastExportEntry::Commit(exported_commit) => {
                    let tree_id = self
                        .toprepo
                        .find_commit(exported_commit.original_id)
                        .with_context(|| {
                            format!("Exported commit {} not found", exported_commit.original_id)
                        })?
                        .tree_id()
                        .with_context(|| {
                            format!("Missing tree id in commit {}", exported_commit.original_id)
                        })?
                        .detach();
                    let empty_vec = Vec::new();
                    let ref_names = tips.get(&exported_commit.original_id).unwrap_or(&empty_vec);
                    callback.import_commit(exported_commit, tree_id, ref_names)?;
                }
                FastExportEntry::Reset(_exported_reset) => {
                    // Not used.
                }
            }
        }
        Ok(())
    }
}

/// The current loading state when running `git fast-export`.`
#[derive(Clone)]
enum LoadRepoState {
    /// `git fast-export` has not been run at all. There might be commits that
    /// we need to load.
    NotLoadedYet,
    /// The repository was updated while loading, so load again to get
    /// potentially new commits.
    LoadingThenQueueAgain,
    /// `git fast-export` is currently running and no concurrent update to the
    /// repository has happened.
    LoadingThenDone,
    /// `git fast-export` has been run.
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FetchParams {
    /// Fetch the configured URL with default refspec.
    Default,
    /// Fetch the specified URL or remote with specified refspec. The refspec is
    /// a tuple of `(from, to)` where the `to` ref is specified without
    /// namespace prefix.
    Custom {
        remote: String,
        refspec: (String, String),
    },
}

struct RepoFetcher {
    /// Indicates if the repository should be loaded at all.
    enabled: bool,
    repo_data: RepoData,
    /// Commits that need to be loaded or even fetched.
    needed_commits: HashSet<CommitId>,

    loading: LoadRepoState,
    /// Confirmed missing commits.
    missing_commits: HashSet<CommitId>,

    /// What be fetched.
    fetch_state: RepoFetcherState,
}

#[derive(Debug, PartialEq)]
enum RepoFetcherState {
    /// Not requested to be fetched yet.
    Idle,
    /// Queued for fetching.
    Queued,
    /// Currently fetching.
    InProgress,
    /// Fetching done.
    Done,
}

impl RepoFetcher {
    pub fn new(enabled: bool, url: gix::Url) -> Self {
        Self {
            enabled,
            repo_data: RepoData::new(url),
            needed_commits: HashSet::new(),
            loading: LoadRepoState::NotLoadedYet,
            missing_commits: HashSet::new(),
            fetch_state: RepoFetcherState::Idle,
        }
    }

    fn fetching_default_refspec_done(&self) -> bool {
        self.fetch_state == RepoFetcherState::Done
    }
}

/// `DotGitModulesCache` is a caching storage of parsed `.gitmodules` content
/// that is read directly from blobs in a git repository. file by given a blob `id`.
struct DotGitModulesCache<'a> {
    repo: &'a gix::Repository,
    cache: HashMap<BlobId, Result<GitModulesInfo>>,
}

impl DotGitModulesCache<'_> {
    /// Parse the `.gitmodules` file given by the `BlobId` and return the map
    /// from path to url.
    pub fn get_from_blob_id(&mut self, entry: ExportedFileEntry) -> Result<&GitModulesInfo> {
        if entry.mode != 0o100644 && entry.mode != 0o100755 {
            anyhow::bail!("Bad mode {:o} for .gitmodules", entry.mode);
        }
        self.cache
            .entry(entry.id)
            .or_insert_with(|| Self::get_from_blob_id_impl(self.repo, entry.id))
            .as_ref()
            // anyhow::Error doesn't implement clone, simply format it to create a copy.
            .map_err(|err| anyhow::anyhow!("{err:#}"))
    }

    fn get_from_blob_id_impl(repo: &gix::Repository, id: BlobId) -> Result<GitModulesInfo> {
        let bytes = repo
            .find_blob(id)
            .with_context(|| format!("Failed to read .gitmodules file, blob {id}"))?
            .take_data();
        GitModulesInfo::parse_dot_gitmodules_bytes(&bytes, PathBuf::from(id.to_hex().to_string()))
    }
}
