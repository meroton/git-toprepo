use crate::config::GitTopRepoConfig;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git_fast_export_import::ChangedFile;
use crate::git_fast_export_import::FastExportCommit;
use crate::git_fast_export_import::FastExportEntry;
use crate::git_fast_export_import::FastExportRepo;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::Logger;
use crate::repo::RepoData;
use crate::repo::RepoName;
use crate::repo::SubRepoName;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::ThinSubmoduleContent;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use std::borrow::Borrow as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

enum TaskResult {
    RepoFetchDone((RepoName, HashSet<Option<String>>)),
    ImportCommit((Arc<RepoName>, FastExportCommit, TreeId)),
    LoadRepoDone(RepoName),
}

#[derive(Debug)]
struct NeededCommit {
    pub repo_name: SubRepoName,
    pub commit_id: CommitId,
}

#[derive(Default)]
struct GitModulesInfo {
    pub submodules: BTreeMap<GitPath, gix::Url>,
}

pub struct CommitLoader {
    toprepo: gix::Repository,
    repos: HashMap<RepoName, RepoFetcher>,
    config: GitTopRepoConfig,

    tx: std::sync::mpsc::Sender<TaskResult>,
    rx: std::sync::mpsc::Receiver<TaskResult>,
    event_queue: VecDeque<TaskResult>,

    pub progress: indicatif::MultiProgress,
    fetch_progress: FetchProgress,
    logger: Logger,
    /// Signal to not start new work but to fail as fast as possible.
    interrupted: Arc<std::sync::atomic::AtomicBool>,

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

impl CommitLoader {
    pub fn new(
        toprepo: gix::Repository,
        config: GitTopRepoConfig,
        progress: indicatif::MultiProgress,
        logger: Logger,
        interrupted: Arc<std::sync::atomic::AtomicBool>,
        thread_pool: threadpool::ThreadPool,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<TaskResult>();
        let pb_fetch_queue = progress
            .add(indicatif::ProgressBar::no_length().with_style(
                indicatif::ProgressStyle::with_template("{elapsed:>4} {msg}").unwrap(),
            ));
        // Make sure that the elapsed time is updated continuously.
        pb_fetch_queue.enable_steady_tick(std::time::Duration::from_millis(1000));
        Self {
            toprepo,
            repos: HashMap::new(),
            config,
            tx,
            rx,
            event_queue: VecDeque::new(),
            progress,
            fetch_progress: FetchProgress::new(pb_fetch_queue),
            logger,
            interrupted,
            thread_pool,
            ongoing_jobs_in_threads: 0,
            load_after_fetch: true,
            fetch_missing_commits: true,
            repos_to_fetch: VecDeque::new(),
            repos_to_load: VecDeque::new(),
            dot_gitmodules_cache: DotGitModulesCache::default(),
        }
    }

    /// Enqueue fetching of a repo with specific refspecs. Using `None` as
    /// refspec will fetch all heads and tags.
    pub fn fetch_repo(&mut self, repo_name: RepoName, mut refspecs: Vec<Option<String>>) {
        let repo_fetcher = self.repos.entry(repo_name.clone()).or_default();
        refspecs.retain(|refspec| !repo_fetcher.refspecs_done.contains(refspec));
        if refspecs.is_empty() {
            return;
        }
        if repo_fetcher.refspecs_to_fetch.is_empty() {
            self.repos_to_fetch.push_back(repo_name);
        }
        repo_fetcher.refspecs_to_fetch.extend(refspecs);
    }

    /// Enqueue loading commits from a repo.
    pub fn load_repo(&mut self, repo_name: RepoName) {
        let repo_fetcher = self.repos.entry(repo_name.clone()).or_default();
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet | LoadRepoState::Done => {
                repo_fetcher.loading = LoadRepoState::LoadingThenDone;
                self.repos_to_load.push_back(repo_name);
            }
            LoadRepoState::LoadingThenQueueAgain => (),
            // If loading and this call is because a fetch has finished, or some
            // other external update of the repo, all refs might not have been
            // accounted for. Need to load again when done.
            LoadRepoState::LoadingThenDone => {
                repo_fetcher.loading = LoadRepoState::LoadingThenQueueAgain;
            }
        }
    }

    /// Calls `load_repo` for all repositories represented among the refs in the
    /// git repository.
    pub fn load_all_repos(&mut self) -> Result<()> {
        let refs = self
            .toprepo
            .references()
            .context("Error getting references")?;

        let mut repo_names = HashSet::new();
        for r in refs.all()? {
            let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            if let Ok(repo_name) = RepoName::from_ref(r.name()) {
                repo_names.insert(repo_name);
            }
        }
        for repo_name in repo_names {
            self.load_repo(repo_name);
        }
        Ok(())
    }

    /// Waits for all ongoing tasks to finish.
    pub fn join(&mut self) {
        while !self.interrupted.load(std::sync::atomic::Ordering::Relaxed) {
            if !self.process_one_event() {
                break;
            }
        }
        self.thread_pool.join();
    }

    pub fn into_result(self) -> (HashMap<RepoName, RepoData>, GitTopRepoConfig) {
        let repo_states = self
            .repos
            .into_iter()
            .map(|(repo_name, repo_fetcher)| (repo_name, repo_fetcher.repo_data))
            .collect::<HashMap<_, _>>();
        (repo_states, self.config)
    }

    /// Receives one event and processes it. Returns true if there are more
    /// events to be expected.
    pub fn process_one_event(&mut self) -> bool {
        self.fetch_progress
            .set_queue_sizes(self.repos_to_fetch.len(), self.event_queue.len());

        // Start work if possible.
        if self.ongoing_jobs_in_threads < self.thread_pool.max_count() {
            if let Some(repo_name) = self.repos_to_fetch.pop_front() {
                match self.start_fetch_repo_job(repo_name) {
                    Ok(()) => self.ongoing_jobs_in_threads += 1,
                    Err(err) => self.logger.error(format!("{err:#}")),
                }
                return true;
            }
            if let Some(repo_name) = self.repos_to_load.pop_front() {
                match self.start_load_repo_job(repo_name) {
                    Ok(()) => self.ongoing_jobs_in_threads += 1,
                    Err(err) => self.logger.error(format!("{err:#}")),
                }
                return true;
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
                    return false;
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
            TaskResult::RepoFetchDone((repo_name, refspecs)) => {
                self.finish_fetch_repo_job(repo_name, refspecs);
                self.ongoing_jobs_in_threads -= 1;
                self.fetch_progress.inc_num_fetches_done();
            }
            TaskResult::ImportCommit((repo_name, commit, tree_id)) => {
                if let Err(err) = self.import_commit(&repo_name, commit, tree_id) {
                    self.logger.error(format!("{err:#}"));
                }
            }
            TaskResult::LoadRepoDone(repo_name) => {
                self.finish_load_repo_job(repo_name);
                self.ongoing_jobs_in_threads -= 1;
            }
        }
        true
    }

    fn start_fetch_repo_job(&mut self, repo_name: RepoName) -> Result<()> {
        let repo_fetcher = self.repos.get_mut(&repo_name).unwrap();
        let refspecs = repo_fetcher.refspecs_to_fetch.clone();

        let mut fetcher = crate::fetch::RemoteFetcher::new(&self.toprepo);
        fetcher
            .set_remote_from_repo_name(&self.toprepo, &repo_name, &self.config)
            .expect("repo_name is valid");
        if !refspecs.contains(&None) {
            fetcher.refspecs.clear();
        }
        fetcher.refspecs.extend(refspecs.iter().flatten().cloned());

        let pb = self.add_progress_bar();
        pb.enable_steady_tick(std::time::Duration::from_millis(1000));
        let logger = self.logger.with_context(&format!("Fetching {repo_name}"));
        let tx = self.tx.clone();
        self.thread_pool.execute(move || {
            if let Err(err) = fetcher.fetch(&pb) {
                logger.error(format!("{err:#}"));
            }
            tx.send(TaskResult::RepoFetchDone((repo_name, refspecs)))
                .expect("receiver never close");
        });
        Ok(())
    }

    fn finish_fetch_repo_job(&mut self, repo_name: RepoName, refspecs: HashSet<Option<String>>) {
        let repo_fetcher = self.repos.get_mut(&repo_name).unwrap();
        for refspec in &refspecs {
            repo_fetcher.refspecs_to_fetch.remove(refspec);
        }
        repo_fetcher.refspecs_done.extend(refspecs);
        if !repo_fetcher.refspecs_to_fetch.is_empty() {
            // Enqueue again.
            self.repos_to_fetch.push_back(repo_name.clone());
        }
        // Load the fetched data.
        if self.load_after_fetch {
            self.load_repo(repo_name);
        }
    }

    /// Loads basic information, i.e. `ThinCommit` information, about all
    /// reachable commits, from `refs/namespaces/{repo_name}/*`, and their
    /// referenced submodules and stores them in `storage`. Commits that are
    /// already in `storage` are skipped.
    fn start_load_repo_job(&self, repo_name: RepoName) -> Result<()> {
        let context = format!("Loading commits in {repo_name}");
        let logger = self.logger.with_context(&context);

        let pb = self.add_progress_bar().with_prefix(repo_name.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(1000));

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

        let toprepo = self.toprepo.clone();
        let tx = self.tx.clone();
        let interrupted = self.interrupted.clone();
        self.thread_pool.execute(move || {
            match Self::get_refs_to_load_arg(&toprepo, &repo_name, &existing_commits, &pb) {
                Ok((refs_arg, unknown_commit_count)) => {
                    if !refs_arg.is_empty() {
                        if let Err(err) = Self::load_repo_commits(
                            &toprepo,
                            &repo_name,
                            refs_arg,
                            unknown_commit_count,
                            &pb,
                            &logger,
                            &interrupted,
                            &tx,
                        ) {
                            logger.error(format!("Loading repo failed: {err:#}"));
                        }
                    }
                }
                Err(err) => {
                    logger.error(format!("Finding refs to load failed: {err:#}"));
                }
            }
            tx.send(TaskResult::LoadRepoDone(repo_name))
                .expect("receiver never close");
        });
        Ok(())
    }

    fn finish_load_repo_job(&mut self, repo_name: RepoName) {
        let repo_fetcher = self.repos.get_mut(&repo_name).unwrap();
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
                        Self::add_missing_commits(&repo_name, missing_commits, repo_fetcher);
                    } else {
                        self.fetch_repo(repo_name, vec![None]);
                    }
                }
            }
            LoadRepoState::Done => unreachable!(),
        }
    }

    fn add_missing_commits(
        _repo_name: &RepoName,
        missing_commits: HashSet<CommitId>,
        repo_fetcher: &mut RepoFetcher,
    ) {
        // FRME
        // for commit_id in &missing_commits {
        //     self.logger
        //         .warning(format!("Missing commit in {repo_name}: {commit_id}"));
        // }
        repo_fetcher.missing_commits.extend(missing_commits);
    }

    /// Creates ref listing arguments to give to git, on the form of
    /// `<start_rev> ^<stop_rev>`. An empty list means that there is nothing to
    /// load.
    fn get_refs_to_load_arg(
        toprepo: &gix::Repository,
        repo_name: &RepoName,
        existing_commits: &HashSet<CommitId>,
        pb: &indicatif::ProgressBar,
    ) -> Result<(Vec<String>, usize)> {
        pb.set_message("Listing refs");

        let ref_prefix = repo_name.to_ref_prefix();
        let mut unknown_tips: Vec<CommitId> = Vec::new();
        for r in toprepo
            .references()?
            .prefixed(BStr::new(ref_prefix.as_bytes()))?
        {
            let mut r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            let commit_id = r
                .peel_to_commit()
                .with_context(|| format!("Failed to peel to commit: {r:?}"))?
                .id;
            if !existing_commits.contains(&commit_id) {
                unknown_tips.push(commit_id);
            }
        }
        if unknown_tips.is_empty() {
            return Ok((vec![], 0));
        }

        pb.set_style(indicatif::ProgressStyle::with_template("{elapsed:>4} {msg} {pos}").unwrap());
        pb.set_message("Walking the git history");

        let start_refs = unknown_tips
            .iter()
            .map(|id| id.to_hex().to_string())
            .collect::<Vec<_>>();
        let (stop_commit_ids, unknown_commit_count) = crate::git::get_first_known_commits(
            toprepo,
            unknown_tips.into_iter(),
            |commit_id| existing_commits.contains(&commit_id),
            pb,
        )?;
        let stop_refs = stop_commit_ids.iter().map(|id| format!("^{}", id.to_hex()));

        let refs_arg = start_refs.into_iter().chain(stop_refs).collect();
        Ok((refs_arg, unknown_commit_count))
    }

    fn load_repo_commits(
        toprepo: &gix::Repository,
        repo_name: &RepoName,
        refs_arg: Vec<String>,
        unknown_commit_count: usize,
        pb: &indicatif::ProgressBar,
        logger: &Logger,
        interrupted: &std::sync::atomic::AtomicBool,
        tx: &std::sync::mpsc::Sender<TaskResult>,
    ) -> Result<()> {
        pb.set_message(format!("Exporting commits in {repo_name}"));
        if unknown_commit_count > 0 {
            pb.set_style(
                indicatif::ProgressStyle::with_template("{elapsed:>4} {msg} {pos}/{len}").unwrap(),
            );
            pb.set_length(unknown_commit_count as u64);
        } else {
            pb.set_style(
                indicatif::ProgressStyle::with_template("{elapsed:>4} {msg} {pos}").unwrap(),
            );
        }

        // TODO: The super repository will get an empty URL, which is exactly
        // what is wanted. Does the rest of the code handle that?
        let arc_repo_name = Arc::new(repo_name.clone());
        let toprepo_git_dir = toprepo.git_dir();
        for export_entry in
            FastExportRepo::load_from_path(toprepo_git_dir, Some(refs_arg), logger.clone())?
        {
            if interrupted.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match export_entry? {
                FastExportEntry::Commit(exported_commit) => {
                    let tree_id = toprepo
                        .find_commit(exported_commit.original_id)
                        .with_context(|| {
                            format!("Exported commit {} not found", exported_commit.original_id)
                        })?
                        .tree_id()
                        .with_context(|| {
                            format!("Missing tree id in commit {}", exported_commit.original_id)
                        })?
                        .detach();
                    tx.send(TaskResult::ImportCommit((
                        arc_repo_name.clone(),
                        exported_commit,
                        tree_id,
                    )))
                    .expect("receiver never close");
                    pb.inc(1);
                }
                FastExportEntry::Reset(_exported_reset) => {
                    // Not used.
                }
            }
        }
        Ok(())
    }

    fn import_commit(
        &mut self,
        repo_name: &RepoName,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
    ) -> Result<()> {
        let context = format!("Repo {}, commit {}", repo_name, exported_commit.original_id);
        let repo_data = &mut self.repos.get_mut(repo_name).unwrap().repo_data;
        let (thin_commit, updated_submodule_commits) = Self::export_thin_commit(
            &self.toprepo,
            repo_data,
            exported_commit,
            tree_id,
            &mut self.config,
            &mut self.dot_gitmodules_cache,
            &self.logger.with_context(&context),
        )?;
        // Insert it into the storage.
        repo_data
            .thin_commits
            .entry(thin_commit.commit_id)
            .or_insert_with(|| Rc::new(thin_commit));

        // Any of the submodule updates that need to be fetched?
        for needed_commit in updated_submodule_commits {
            self.assure_commit_available(needed_commit);
        }
        Ok(())
    }

    fn assure_commit_available(&mut self, needed_commit: NeededCommit) {
        let repo_name = RepoName::SubRepo(needed_commit.repo_name);
        let commit_id = needed_commit.commit_id;
        // Already loaded?
        let repo_fetcher = self.repos.entry(repo_name.clone()).or_default();
        if repo_fetcher.repo_data.thin_commits.contains_key(&commit_id)
            || repo_fetcher.missing_commits.contains(&commit_id)
        {
            return;
        }
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet => {
                repo_fetcher.needed_commits.insert(commit_id);
                self.load_repo(repo_name);
            }
            LoadRepoState::LoadingThenQueueAgain => (),
            LoadRepoState::LoadingThenDone => (),
            LoadRepoState::Done => {
                if repo_fetcher.fetching_default_refspec_done() {
                    let mut missing_commits = HashSet::new();
                    missing_commits.insert(commit_id);
                    Self::add_missing_commits(&repo_name, missing_commits, repo_fetcher);
                } else {
                    repo_fetcher.needed_commits.insert(commit_id);
                    if self.fetch_missing_commits {
                        self.fetch_repo(repo_name, vec![None]);
                    }
                }
            }
        }
    }

    fn resolve_reference(r: gix::Reference, logger: &Logger) -> Option<(RepoName, gix::ObjectId)> {
        let name = r.name().as_bstr();
        if !name.starts_with(b"refs/namespaces/") {
            // Not a toprepo ref.
            return None;
        }
        match r.id().header() {
            Ok(header) if header.kind().is_commit() => (),
            Ok(_) => {
                logger.warning(format!("Ref {} is not a commit", r.name().as_bstr()));
                return None;
            }
            Err(err) => {
                logger.warning(format!("{err:#}: Missing header in {}", r.name().as_bstr()));
                return None;
            }
        }
        let r = r.detach();
        let commit_id = match r.peeled {
            Some(commit_id) => commit_id,
            None => {
                logger.warning(format!("Could not peel commit ref {}", r.name.as_bstr()));
                return None;
            }
        };
        let repo_name = match RepoName::from_ref(r.name.borrow()) {
            Ok(repo_name) => repo_name,
            Err(err) => {
                logger.warning(format!("{err:#}"));
                return None;
            }
        };
        Some((repo_name, commit_id))
    }

    /// Converts a `FastExportCommit` to a `ThinCommit`.
    fn export_thin_commit(
        repo: &gix::Repository,
        repo_storage: &RepoData,
        exported_commit: FastExportCommit,
        tree_id: TreeId,
        config: &mut GitTopRepoConfig,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        logger: &Logger,
    ) -> Result<(ThinCommit, Vec<NeededCommit>)> {
        let commit_id: CommitId = exported_commit.original_id;
        let thin_parents = exported_commit
            .parents
            .iter()
            .map(|parent_id| {
                repo_storage
                    .thin_commits
                    .get(parent_id)
                    .with_context(|| {
                        format!("BUG: Parent {} of {} not yet parsed", parent_id, commit_id)
                    })
                    .cloned()
            })
            .collect::<Result<Vec<_>>>()?;
        let dot_gitmodules = match thin_parents.first() {
            Some(first_thin_parent) => first_thin_parent.dot_gitmodules,
            None => None,
        };
        let mut submodule_bumps = BTreeMap::new();

        let mut thin_commit = ThinCommit {
            commit_id,
            tree_id,
            depth: thin_parents.iter().map(|p| p.depth + 1).max().unwrap_or(0),
            dot_gitmodules,
            submodule_bumps: BTreeMap::new(),
            submodule_paths: thin_parents.first().map_or_else(
                || Rc::new(HashSet::new()),
                |first_parent| first_parent.submodule_paths.clone(),
            ),
            parents: thin_parents,
        };
        // Check for an updated .gitmodules file.
        let old_dot_gitmodules = thin_commit.dot_gitmodules;
        {
            let get_dot_gitmodules_logger =
                logger.with_context(&format!(".gitmodules in commit {commit_id}"));
            match Self::get_dot_gitmodules_update(&exported_commit, &get_dot_gitmodules_logger) {
                Ok(Some(new_dot_gitmodules)) => thin_commit.dot_gitmodules = new_dot_gitmodules,
                Ok(None) => (), // No update of .gitmodules.
                Err(err) => {
                    get_dot_gitmodules_logger.error(format!("{err:#}"));
                    // Keep the old dot_gitmodules content as that will probably expand the repository the best way.
                }
            };
        }
        let gitmodules_info = match thin_commit.dot_gitmodules {
            Some(dot_gitmodules) => match dot_gitmodules_cache
                .get_from_blob_id(repo, dot_gitmodules)
                .context("Failed to parse .gitmodules")
            {
                Ok(gitmodules_info) => gitmodules_info,
                Err(err) => {
                    logger.warning(format!("{err:#}"));
                    // Reset thin_commit.dot_gitmodules to avoid logging the same error again.
                    thin_commit.dot_gitmodules = None;
                    &GitModulesInfo::default()
                }
            },
            None => &GitModulesInfo::default(),
        };
        // Look for submodule updates.
        // Adding and removing more than one submodule at a time is so rare that
        // it is not worth optimizing for it. Let's copy the HashSet every time.
        let mut new_submodule_commits = Vec::new();
        for fc in exported_commit.file_changes {
            match fc {
                ChangedFile::Modified(fc) => {
                    let path = GitPath::new(fc.path);
                    if fc.mode == b"160000" {
                        // 160000 means submodule
                        let submod_commit_id: CommitId = gix::ObjectId::from_hex(&fc.hash)?;
                        let submod_url = gitmodules_info.submodules.get(&path);
                        let subrepo_name = Self::get_submod_repo_name(
                            config,
                            &path,
                            submod_url,
                            &repo_storage.url,
                            logger,
                        )
                        .unwrap_or_else(|err| {
                            logger.error(format!("{err:#}"));
                            None
                        });
                        if let Some(subrepo_name) = &subrepo_name {
                            new_submodule_commits.push(NeededCommit {
                                repo_name: subrepo_name.clone(),
                                commit_id: submod_commit_id,
                            });
                        }
                        thin_commit.submodule_paths = Rc::new({
                            let mut paths = thin_commit.submodule_paths.as_ref().clone();
                            paths.insert(path.clone());
                            paths
                        });
                        submodule_bumps.insert(
                            path,
                            ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
                                repo_name: subrepo_name,
                                commit_id: submod_commit_id,
                            }),
                        );
                    } else if thin_commit.submodule_paths.contains(&path) {
                        // It might be a submodule that changed to another
                        // type, e.g. tree or file. Remove it.
                        thin_commit.submodule_paths = Rc::new({
                            let mut paths = thin_commit.submodule_paths.as_ref().clone();
                            paths.remove(&path);
                            paths
                        });
                        submodule_bumps.insert(path, ThinSubmodule::Removed);
                    }
                }
                ChangedFile::Deleted(fc) => {
                    // TODO: Implement borrow between BStr and GitPath to delay
                    // construction of a GitPath.
                    let path = GitPath::new(fc.path);
                    if thin_commit.submodule_paths.contains(&path) {
                        thin_commit.submodule_paths = Rc::new({
                            let mut paths = thin_commit.submodule_paths.as_ref().clone();
                            paths.remove(&path);
                            paths
                        });
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
        if thin_commit.dot_gitmodules != old_dot_gitmodules {
            // Loop through all submodules to see if any have changed.
            for (path, thin_submod) in thin_commit.get_all_submodules() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    submodule_bumps.entry(path.clone())
                {
                    match thin_submod {
                        ThinSubmodule::AddedOrModified(thin_submod) => {
                            let submod_url = gitmodules_info.submodules.get(path);
                            let new_repo_name = Self::get_submod_repo_name(
                                config,
                                path,
                                submod_url,
                                &repo_storage.url,
                                logger,
                            )
                            .unwrap_or_else(|err| {
                                logger.error(format!("{err:#}"));
                                None
                            });
                            if new_repo_name != thin_submod.repo_name {
                                // Insert an entry that this submodule has been updated.
                                entry.insert(ThinSubmodule::AddedOrModified(
                                    ThinSubmoduleContent {
                                        repo_name: new_repo_name,
                                        commit_id: thin_submod.commit_id,
                                    },
                                ));
                            }
                        }
                        ThinSubmodule::Removed => {}
                    }
                }
            }
        }
        // Could not fill in thin_commit.submodule_bumps before during the call
        // to thin_commit.get_all_submodules().
        assert!(thin_commit.submodule_bumps.is_empty());
        thin_commit.submodule_bumps = submodule_bumps;
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
            match fc {
                ChangedFile::Modified(fc) => {
                    if fc.path == b".gitmodules" {
                        let dot_gitmodules = gix::ObjectId::from_hex(&fc.hash)
                            .context("Bad blob id for .gitmodules")?;
                        if fc.mode != b"100644" && fc.mode != b"100755" {
                            // Expecting regular file or executable file,
                            // not a symlink, directory, submodule, etc.
                            logger.warning(format!("Bad mode {} for .gitmodules", fc.mode));
                            return Ok(GITMODULES_FILE_REMOVED);
                        }
                        return Ok(Some(Some(dot_gitmodules)));
                    }
                }
                ChangedFile::Deleted(fc) => {
                    if fc.path == b".gitmodules" {
                        return Ok(GITMODULES_FILE_REMOVED);
                    }
                }
            }
        }
        // No .gitmodules update found.
        Ok(None)
    }

    /// Updates the `thin_commit.submodules.repo_name` based on a potentially new `gitmodules_info`.
    fn get_submod_repo_name(
        config: &mut GitTopRepoConfig,
        path: &GitPath,
        submod_url: Option<&gix::Url>,
        base_url: &gix::Url,
        logger: &Logger,
    ) -> Result<Option<SubRepoName>> {
        let name = match submod_url {
            Some(submod_url) => {
                let full_url = base_url.join(submod_url);
                let (name, _subrepo_config) = config.get_or_insert_from_url(&full_url)?;
                Some(SubRepoName::new(name))
            }
            None => {
                // If the submodule is removed in this commit, it will
                // already be gone from thin_commit.submodules.
                logger.warning(format!("Missing submodule {path} in .gitmodules"));
                None
            }
        };
        Ok(name)
    }

    fn add_progress_bar(&self) -> indicatif::ProgressBar {
        let pb =
            self.progress.add(
                indicatif::ProgressBar::no_length().with_style(
                    indicatif::ProgressStyle::with_template("{elapsed:>4} {prefix} {wide_msg}")
                        .unwrap(),
                ),
            );
        // Now when it is added, we can start calling tick to print. Don't print
        // before it is added as multiple ProgressBars will trash eachother.
        pb.enable_steady_tick(std::time::Duration::from_millis(1000));
        pb
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

struct RepoFetcher {
    repo_data: RepoData,
    /// Commits that need to be loaded or even fetched.
    needed_commits: HashSet<CommitId>,

    loading: LoadRepoState,
    /// Confirmed missing commits.
    missing_commits: HashSet<CommitId>,

    /// The refspecs that should be fetched or None for the default refspec.
    /// If non-empty, the repo is in queue or currently being fetched.
    refspecs_to_fetch: HashSet<Option<String>>,
    refspecs_done: HashSet<Option<String>>,
}

impl Default for RepoFetcher {
    fn default() -> Self {
        RepoFetcher {
            repo_data: RepoData::default(),
            needed_commits: HashSet::new(),
            loading: LoadRepoState::NotLoadedYet,
            missing_commits: HashSet::new(),
            refspecs_to_fetch: HashSet::new(),
            refspecs_done: HashSet::new(),
        }
    }
}

impl RepoFetcher {
    fn fetching_default_refspec_done(&self) -> bool {
        self.refspecs_done.contains(&None)
    }
}

/// `DotGitModulesCache` is a caching storage of parsed `.gitmodules` content
/// that is read directly from blobs in a git repository. file by given a blob `id`.
#[derive(Default)]
pub struct DotGitModulesCache {
    cache: HashMap<gix::ObjectId, GitModulesInfo>,
}

impl DotGitModulesCache {
    /// Parse the `.gitmodules` file given by the `BlobId` and return the map
    /// from path to url.
    // TODO: Handle parsing error, duplicated paths, missing path, missing url, bad url syntax etc.
    fn get_from_blob_id(
        &mut self,
        gix_repo: &gix::Repository,
        id: BlobId,
    ) -> Result<&GitModulesInfo> {
        match self.cache.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let bytes = gix_repo.find_blob(id)?.take_data();
                let config = gix::submodule::File::from_bytes(
                    &bytes,
                    PathBuf::from(id.to_hex().to_string()),
                    &Default::default(),
                )?;
                let mut info = GitModulesInfo::default();
                for name in config.names() {
                    let path = config.path(name)?;
                    let url = config.url(name)?;
                    info.submodules.insert(GitPath::new(path.into_owned()), url);
                }
                Ok(entry.insert(info))
            }
        }
    }
}

struct FetchProgress {
    pb: indicatif::ProgressBar,
    fetch_queue_size: usize,
    num_fetches_done: usize,
    event_queue_size: usize,
}

impl FetchProgress {
    fn new(pb: indicatif::ProgressBar) -> Self {
        let ret = Self {
            pb,
            fetch_queue_size: 0,
            num_fetches_done: 0,
            event_queue_size: 0,
        };
        ret.draw();
        ret
    }

    pub fn set_queue_sizes(&mut self, fetch_queue_size: usize, event_queue_size: usize) {
        if fetch_queue_size != self.fetch_queue_size || event_queue_size != self.event_queue_size {
            self.fetch_queue_size = fetch_queue_size;
            self.event_queue_size = event_queue_size;
            self.draw();
        }
    }

    pub fn inc_num_fetches_done(&mut self) {
        self.num_fetches_done += 1;
        self.draw();
    }

    fn draw(&self) {
        let mut msg = format!(
            "{} {} in queue for fetching, {} done",
            self.fetch_queue_size,
            if self.fetch_queue_size == 1 {
                "repository"
            } else {
                "repositories"
            },
            self.num_fetches_done,
        );
        if self.fetch_queue_size > 0 {
            msg += &format!(" ({} in queue)", self.fetch_queue_size);
        }
        self.pb.set_message(msg);
    }
}
