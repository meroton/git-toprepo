use crate::config::GitTopRepoConfig;
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
use crate::log::Logger;
use crate::repo::RepoData;
use crate::repo::RepoStates;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::ThinSubmoduleContent;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use itertools::Itertools as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

enum TaskResult {
    RepoFetchDone(RepoName, Result<()>),
    LoadCachedCommits(RepoName, Vec<CommitId>, oneshot::Sender<Result<()>>),
    ImportCommit(Arc<RepoName>, FastExportCommit, TreeId),
    LoadRepoDone(RepoName, InterruptedResult<()>),
}

#[derive(Debug)]
struct NeededCommit {
    pub repo_name: SubRepoName,
    pub commit_id: CommitId,
}

pub struct CommitLoader<'a> {
    toprepo: gix::Repository,
    repos: HashMap<RepoName, RepoFetcher>,
    /// Repositories that have been loaded from the cache.
    cached_repo_states: &'a mut RepoStates,
    config: &'a mut GitTopRepoConfig,

    tx: std::sync::mpsc::Sender<TaskResult>,
    rx: std::sync::mpsc::Receiver<TaskResult>,
    event_queue: VecDeque<TaskResult>,

    pub progress: indicatif::MultiProgress,
    import_progress: indicatif::ProgressBar,
    load_progress: ProgressStatus,
    fetch_progress: ProgressStatus,
    logger: Logger,
    /// Signal to not start new work but to fail as fast as possible.
    error_observer: crate::log::ErrorObserver,

    thread_pool: threadpool::ThreadPool,
    ongoing_jobs_in_threads: usize,

    /// Flag if the repository content should be loaded after fetch is done.
    pub load_after_fetch: bool,
    /// Flag if submodule commits that are missing should be fetched.
    pub fetch_missing_commits: bool,

    repos_to_load: VecDeque<RepoName>,
    repos_to_fetch: VecDeque<RepoName>,

    /// A cache of parsed `.gitmodules` files.
    dot_gitmodules_cache: DotGitModulesCache,
}

impl<'a> CommitLoader<'a> {
    pub fn new(
        toprepo: gix::Repository,
        cached_repo_states: &'a mut RepoStates,
        config: &'a mut GitTopRepoConfig,
        progress: indicatif::MultiProgress,
        logger: Logger,
        error_observer: crate::log::ErrorObserver,
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
            "     {prefix:.cyan} [{bar:24}] {pos}/{len}{msg!}",
        )
        .unwrap()
        .progress_chars("=> ");
        let load_progress = ProgressStatus::new(
            progress.add(
                indicatif::ProgressBar::no_length()
                    .with_style(style.clone())
                    .with_prefix("Loading "),
            ),
        );
        let fetch_progress = ProgressStatus::new(
            progress.add(
                indicatif::ProgressBar::no_length()
                    .with_style(style)
                    .with_prefix("Fetching"),
            ),
        );
        Ok(Self {
            toprepo: toprepo.clone(),
            repos: HashMap::new(),
            cached_repo_states,
            config,
            tx,
            rx,
            event_queue: VecDeque::new(),
            progress,
            import_progress,
            load_progress,
            fetch_progress,
            logger,
            error_observer,
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
        self.error_observer.maybe_consume(&self.logger, result)
    }

    fn fetch_repo_impl(&mut self, repo_name: RepoName) -> Result<()> {
        let repo_fetcher = self.get_or_create_repo_fetcher(&repo_name)?;
        if !repo_fetcher.enabled {
            self.logger.warning(format!(
                "Repo {repo_name} is disabled in the configuration, will not fetch"
            ));
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
            self.logger.warning(format!(
                "Repo {repo_name} is disabled in the configuration, will not load commits"
            ));
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
                    Ok(pbs) => {
                        self.ongoing_jobs_in_threads += 1;
                        self.fetch_progress.start(repo_name.to_string(), pbs);
                    }
                    Err(err) => {
                        // Fail unless keep-going.
                        self.error_observer.maybe_consume(&self.logger, Err(err))?;
                    }
                }
                return Ok(true);
            }
            if let Some(repo_name) = self.repos_to_load.pop_front() {
                self.start_load_repo_job(repo_name.clone());
                self.ongoing_jobs_in_threads += 1;
                self.load_progress.start(repo_name.to_string(), Vec::new());
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
                match self.rx.recv() {
                    Ok(msg) => msg,
                    Err(std::sync::mpsc::RecvError) => {
                        // The sender has been dropped, no more events will come.
                        unreachable!();
                    }
                }
            }
        };

        // Process one messages.
        match msg {
            TaskResult::RepoFetchDone(repo_name, result) => {
                self.ongoing_jobs_in_threads -= 1;
                self.fetch_progress.finish(&repo_name.to_string());
                match result {
                    Ok(()) => {
                        self.finish_fetch_repo_job(&repo_name);
                    }
                    Err(err) => {
                        self.error_observer.maybe_consume(&self.logger, Err(err))?;
                    }
                }
            }
            TaskResult::LoadCachedCommits(repo_name, cached_tips_to_load, result_channel) => {
                let result = match self.load_cached_commits(&repo_name, cached_tips_to_load) {
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
            TaskResult::ImportCommit(repo_name, commit, tree_id) => {
                match self.import_commit(&repo_name, commit, tree_id) {
                    Ok(()) => self.import_progress.inc(1),
                    Err(InterruptedError::Interrupted) => {
                        // The caller can continue processing if they want.
                        return Ok(true);
                    }
                    Err(InterruptedError::Normal(err)) => {
                        self.error_observer.maybe_consume(&self.logger, Err(err))?;
                    }
                }
            }
            TaskResult::LoadRepoDone(repo_name, result) => {
                self.ongoing_jobs_in_threads -= 1;
                self.load_progress.finish(&repo_name.to_string());
                match result {
                    Ok(()) => {
                        let result = self.finish_load_repo_job(&repo_name);
                        self.error_observer.maybe_consume(&self.logger, result)?;
                    }
                    Err(InterruptedError::Interrupted) => {
                        // This doesn't mean there are no more events to process.
                    }
                    Err(InterruptedError::Normal(err)) => {
                        self.error_observer.maybe_consume(&self.logger, Err(err))?;
                    }
                }
            }
        }
        Ok(true)
    }

    fn start_fetch_repo_job(&mut self, repo_name: RepoName) -> Result<Vec<indicatif::ProgressBar>> {
        let repo_fetcher = self.repos.get_mut(&repo_name).unwrap();
        assert_eq!(repo_fetcher.fetch_state, RepoFetcherState::Queued);
        repo_fetcher.fetch_state = RepoFetcherState::InProgress;

        let mut fetcher = crate::fetch::RemoteFetcher::new(&self.toprepo);
        fetcher.set_remote_from_repo_name(&self.toprepo, &repo_name, self.config)?;

        let pb_url = self.progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::with_template("{elapsed:>4} {prefix:.cyan} {msg}")
                        .unwrap(),
                )
                .with_prefix("git fetch")
                .with_message(fetcher.remote.clone().unwrap_or_else(|| "<top>".to_owned())),
        );
        let pb_status = self.progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(indicatif::ProgressStyle::with_template("     {wide_msg}").unwrap()),
        );
        // Now when it is added, we can start calling tick to print. Don't print
        // before it is added as multiple ProgressBars will trash eachother.
        pb_status.enable_steady_tick(std::time::Duration::from_millis(1000));

        let tx = self.tx.clone();
        let pb_clone = pb_status.clone();
        self.thread_pool.execute(move || {
            let result = fetcher
                .fetch_with_progress_bar(&pb_clone)
                .with_context(|| format!("Fetching {repo_name}"));
            // Sending might fail on interrupt.
            let _ = tx.send(TaskResult::RepoFetchDone(repo_name, result));
        });
        Ok(vec![pb_url, pb_status])
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
        let logger = self.logger.with_context(&context);

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

        let toprepo = self.toprepo.clone();
        let tx = self.tx.clone();
        let error_observer = self.error_observer.clone();
        self.thread_pool.execute(move || {
            let single_repo_loader = SingleRepoLoader {
                toprepo: &toprepo,
                repo_name: &repo_name,
                logger: &logger,
                error_observer: &error_observer,
            };

            struct LoadRepoCallback {
                tx: std::sync::mpsc::Sender<TaskResult>,
                repo_name: Arc<RepoName>,
            }
            impl SingleLoadRepoCallback for LoadRepoCallback {
                fn load_cached_commits(
                    &self,
                    cached_tips_to_load: Vec<CommitId>,
                ) -> InterruptedResult<()> {
                    let (result_tx, result_rx) = oneshot::channel();
                    self.tx
                        .send(TaskResult::LoadCachedCommits(
                            (*self.repo_name).clone(),
                            cached_tips_to_load,
                            result_tx,
                        ))
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
                ) -> InterruptedResult<()> {
                    self.tx
                        .send(TaskResult::ImportCommit(
                            self.repo_name.clone(),
                            commit,
                            tree_id,
                        ))
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
            let _ = tx.send(TaskResult::LoadRepoDone(repo_name, result));
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
                            &self.logger,
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
        logger: &Logger,
    ) {
        // Already logged when loading the repositories.
        for commit_id in &missing_commits {
            logger.warning(format!("Missing commit in {repo_name}: {commit_id}"));
        }
        all_missing_commits.extend(missing_commits);
    }

    fn load_cached_commits(
        &mut self,
        repo_name: &RepoName,
        cached_tips_to_load: Vec<CommitId>,
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
        let mut todo = cached_tips_to_load
            .into_iter()
            .map(|commit_id| {
                cached_repo
                    .thin_commits
                    .get(&commit_id)
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
                Self::verify_cached_commit(
                    &repo_fetcher.repo_data,
                    thin_commit.as_ref(),
                    self.config,
                    &mut self.dot_gitmodules_cache,
                )
                .with_context(|| format!("Repo {repo_name} commit {}", thin_commit.commit_id))?;
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
        self.error_observer.maybe_consume(&self.logger, result)?;
        // Load all submodule commits that are needed so far.
        for needed_commit in needed_commits {
            let result = self.ensure_commit_available(needed_commit);
            self.error_observer.maybe_consume(&self.logger, result)?;
        }
        Ok(())
    }

    fn import_commit(
        &mut self,
        repo_name: &RepoName,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
    ) -> InterruptedResult<()> {
        let context = format!("Repo {} commit {}", repo_name, exported_commit.original_id);
        let hash_without_committer = exported_commit.hash_without_committer()?;
        let repo_data = &mut self.repos.get_mut(repo_name).unwrap().repo_data;
        let (thin_commit, updated_submodule_commits) = Self::export_thin_commit(
            repo_data,
            exported_commit,
            tree_id,
            self.config,
            &mut self.dot_gitmodules_cache,
            &self.logger.with_context(&context),
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
            self.error_observer.maybe_consume(&self.logger, result)?;
        }
        Ok(())
    }

    fn verify_cached_commit(
        repo_storage: &RepoData,
        commit: &ThinCommit,
        config: &mut GitTopRepoConfig,
        dot_gitmodules_cache: &mut DotGitModulesCache,
    ) -> Result<()> {
        let submodule_paths_to_check = if commit
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
                        config,
                        dot_gitmodules_cache,
                        None,
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
                        &self.logger,
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
        config: &mut GitTopRepoConfig,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        logger: &Logger,
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
        let dot_gitmodules = Self::get_dot_gitmodules_update(&exported_commit, logger)?
            .unwrap_or(old_dot_gitmodules); // None means no update of .gitmodules.

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
                        let submod_commit_id: CommitId = gix::ObjectId::from_hex(&hash)?;
                        let submod_repo_name = Self::get_submod_repo_name(
                            dot_gitmodules,
                            thin_parents.first(),
                            &path,
                            &repo_storage.url,
                            config,
                            dot_gitmodules_cache,
                            Some(logger),
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
        if dot_gitmodules != old_dot_gitmodules {
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
                            config,
                            dot_gitmodules_cache,
                            Some(logger),
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

    /// Returns `Ok(Some(...))` if a `.gitmodules` update was found.
    fn get_dot_gitmodules_update(
        exported_commit: &FastExportCommit,
        logger: &Logger,
    ) -> Result<Option<Option<gix::ObjectId>>> {
        // Assume just a single entry for .gitmodules.
        const GITMODULES_FILE_REMOVED: Option<Option<gix::ObjectId>> = Some(None);
        for fc in &exported_commit.file_changes {
            if fc.path == b".gitmodules" {
                match &fc.change {
                    FileChange::Modified { mode, hash } => {
                        let dot_gitmodules =
                            gix::ObjectId::from_hex(hash).context("Bad blob id for .gitmodules")?;
                        if mode != b"100644" && mode != b"100755" {
                            // Expecting regular file or executable file,
                            // not a symlink, directory, submodule, etc.
                            logger.warning(format!("Bad mode {mode} for .gitmodules"));
                            return Ok(GITMODULES_FILE_REMOVED);
                        }
                        return Ok(Some(Some(dot_gitmodules)));
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
        dot_gitmodules: Option<gix::ObjectId>,
        first_parent: Option<&Rc<ThinCommit>>,
        path: &GitPath,
        base_url: &gix::Url,
        config: &mut GitTopRepoConfig,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        logger: Option<&Logger>,
    ) -> Option<SubRepoName> {
        let get_logger = || {
            logger.filter(|_| {
                // Only log if .gitmodules has changed or the submodule was just added.
                let submodule_just_added = first_parent
                    .is_none_or(|first_parent| !first_parent.submodule_paths.contains(path));
                // A missing parent means that .gitmodules has changed.
                let dot_gitmodules_updated = first_parent
                    .is_none_or(|first_parent| dot_gitmodules != first_parent.dot_gitmodules);
                dot_gitmodules_updated || submodule_just_added
            })
        };

        // Parse .gitmodules.
        let Some(dot_gitmodules) = dot_gitmodules else {
            if let Some(logger) = get_logger() {
                logger.warning(format!(
                    "Cannot resolve submodule {path}, .gitmodules is missing"
                ));
            }
            return None;
        };
        let gitmodules_info = match dot_gitmodules_cache.get_from_blob_id(dot_gitmodules) {
            Ok(gitmodules_info) => gitmodules_info,
            Err(err) => {
                if let Some(logger) = get_logger() {
                    logger.warning(format!("{err:#}"));
                }
                return None;
            }
        };

        let submod_url = match gitmodules_info.submodules.get(path) {
            Some(Ok(url)) => url,
            Some(Err(err)) => {
                if let Some(logger) = get_logger() {
                    logger.warning(format!("{err:#}"));
                }
                return None;
            }
            None => {
                if let Some(logger) = get_logger() {
                    logger.warning(format!("Missing {path} in .gitmodules"));
                }
                return None;
            }
        };
        let full_url = base_url.join(submod_url);
        let name = match config
            .get_or_insert_from_url(&full_url)
            .map_err(|err| {
                if let Some(logger) = get_logger() {
                    logger.error(format!("{err:#}"));
                }
            })
            .ok()?
        {
            crate::config::GetOrInsertOk::Found((name, _)) => name,
            crate::config::GetOrInsertOk::Missing(_) => {
                if let Some(logger) = get_logger() {
                    logger.warning(format!(
                        "URL {full_url} is missing in the git-toprepo configuration"
                    ));
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
    logger: &'a Logger,
    error_observer: &'a crate::log::ErrorObserver,
}

trait SingleLoadRepoCallback {
    fn load_cached_commits(&self, cached_tips_to_load: Vec<CommitId>) -> InterruptedResult<()>;

    fn import_commit(
        &self,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
    ) -> InterruptedResult<()>;
}

impl SingleRepoLoader<'_> {
    pub fn load_repo(
        &self,
        existing_commits: &HashSet<CommitId>,
        cached_commits: &HashSet<CommitId>,
        callback: &impl SingleLoadRepoCallback,
    ) -> InterruptedResult<()> {
        let (mut refs_arg, mut cached_tips_to_load, mut unknown_commit_count) = self
            .get_refs_to_load_arg(existing_commits, cached_commits)
            .context("Failed to find refs to load")?;
        if !cached_tips_to_load.is_empty()
            && let Err(err) = callback.load_cached_commits(cached_tips_to_load)
        {
            let err = match err {
                InterruptedError::Interrupted => return Err(InterruptedError::Interrupted),
                InterruptedError::Normal(err) => err,
            };
            // Loading the cache failed, try again without the cache. Some of
            // the commits might have been loaded, but this kind of failure
            // should be rare so it is not worth updating existing_commits.
            self.logger
                .warning(format!("Discarding cache for {}: {err:#}", self.repo_name));
            (refs_arg, cached_tips_to_load, unknown_commit_count) = self
                .get_refs_to_load_arg(existing_commits, &HashSet::new())
                .context("Failed to find refs to load")
                .map_err(InterruptedError::Normal)?;
            assert!(cached_tips_to_load.is_empty());
        }
        if !refs_arg.is_empty() {
            match self.load_from_refs(refs_arg, unknown_commit_count, callback) {
                Ok(()) => Ok(()),
                Err(InterruptedError::Interrupted) => Err(InterruptedError::Interrupted),
                Err(InterruptedError::Normal(err)) => {
                    Err(err.context("Failed to load commits").into())
                }
            }?
        }
        Ok(())
    }

    /// Creates ref listing arguments to give to git, on the form of
    /// `<start_rev> ^<stop_rev>`. An empty list means that there is nothing to
    /// load.
    fn get_refs_to_load_arg(
        &self,
        existing_commits: &HashSet<CommitId>,
        cached_commits: &HashSet<CommitId>,
    ) -> Result<(Vec<String>, Vec<CommitId>, usize)> {
        // TODO: self.pb.set_message("Listing refs");

        let ref_prefix = self.repo_name.to_ref_prefix();
        let mut unknown_tips: Vec<CommitId> = Vec::new();
        let mut visited_cached_commits = Vec::new();
        for r in self
            .toprepo
            .references()?
            .prefixed(BStr::new(ref_prefix.as_bytes()))?
        {
            let mut r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            let commit_id = r
                .peel_to_commit()
                .with_context(|| format!("Failed to peel to commit: {r:?}"))?
                .id;
            if existing_commits.contains(&commit_id) {
                continue;
            }
            if cached_commits.contains(&commit_id) {
                visited_cached_commits.push(commit_id);
                continue;
            }
            unknown_tips.push(commit_id);
        }
        if unknown_tips.is_empty() {
            return Ok((vec![], visited_cached_commits, 0));
        }

        // TODO: self.pb.set_message("Walking the git history");

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

    fn load_from_refs(
        &self,
        refs_arg: Vec<String>,
        _unknown_commit_count: usize, // TODO: Remove.
        callback: &impl SingleLoadRepoCallback,
    ) -> InterruptedResult<()> {
        // TODO: self.pb.set_message(format!("Exporting commits"));

        // TODO: The super repository will get an empty URL, which is exactly
        // what is wanted. Does the rest of the code handle that?
        let toprepo_git_dir = self.toprepo.git_dir();
        for export_entry in
            FastExportRepo::load_from_path(toprepo_git_dir, Some(refs_arg), self.logger.clone())?
        {
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
                    callback.import_commit(exported_commit, tree_id)?;
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
struct DotGitModulesCache {
    repo: gix::Repository,
    cache: HashMap<gix::ObjectId, Result<GitModulesInfo>>,
}

impl DotGitModulesCache {
    /// Parse the `.gitmodules` file given by the `BlobId` and return the map
    /// from path to url.
    pub fn get_from_blob_id(&mut self, id: BlobId) -> &Result<GitModulesInfo> {
        self.cache
            .entry(id)
            .or_insert_with(|| Self::get_from_blob_id_impl(&self.repo, id))
    }

    fn get_from_blob_id_impl(repo: &gix::Repository, id: BlobId) -> Result<GitModulesInfo> {
        let bytes = repo
            .find_blob(id)
            .with_context(|| format!("Failed to read .gitmodules file, blob {id}"))?
            .take_data();
        GitModulesInfo::parse_dot_gitmodules_bytes(&bytes, PathBuf::from(id.to_hex().to_string()))
    }
}

struct ProgressStatus {
    pb: indicatif::ProgressBar,
    queue_size: usize,
    active: Vec<(String, Vec<indicatif::ProgressBar>)>,
    num_done: usize,
    num_cached_done: usize,
}

impl ProgressStatus {
    fn new(pb: indicatif::ProgressBar) -> Self {
        let ret = Self {
            pb,
            queue_size: 0,
            active: Vec::new(),
            num_done: 0,
            num_cached_done: 0,
        };
        ret.draw();
        ret
    }

    pub fn start(&mut self, name: String, pbs: Vec<indicatif::ProgressBar>) {
        if !self.active.is_empty() {
            for pb in &pbs {
                pb.set_draw_target(indicatif::ProgressDrawTarget::hidden());
            }
        }
        self.active.push((name, pbs));
        self.draw();
    }

    pub fn finish(&mut self, name: &str) {
        let (idx, _value) = self
            .active
            .iter()
            .find_position(|(n, _pb)| *n == name)
            .expect("name is active");
        // Remove the first occurrence of the name, in case of duplicates.
        self.active.remove(idx);
        if idx == 0
            && let Some((_name, item_pbs)) = self.active.first()
        {
            // Show the first active item, the oldest one.
            for item_pb in item_pbs {
                item_pb.set_draw_target(indicatif::ProgressDrawTarget::stderr());
            }
        }
        self.num_done += 1;
        self.pb.inc(1);
        self.draw();
    }

    pub fn set_queue_size(&mut self, queue_size: usize) {
        if queue_size != self.queue_size {
            self.draw();
        }
    }

    pub fn inc_num_cached_done(&mut self) {
        self.num_cached_done += 1;
        self.draw();
    }

    fn draw(&self) {
        self.pb
            .set_length((self.num_done + self.active.len() + self.queue_size) as u64);

        let mut msg = String::new();
        if !self.active.is_empty() {
            msg.push_str(": ");
            msg.push_str(&self.active.iter().map(|(name, _pb)| name).join(", "));
        }
        if self.num_cached_done > 0 {
            if msg.is_empty() {
                msg.push_str(": ");
            } else {
                msg.push(' ');
            }
            msg.push_str(&format!("({} cached)", self.num_cached_done));
        }
        self.pb.set_message(msg);
    }
}
