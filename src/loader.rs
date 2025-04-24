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
use serde_with::serde_as;
use std::borrow::Borrow as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::io::Read;
use std::io::Write;
use std::ops::Deref as _;
use std::path::Path;
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
    pub submodules: HashMap<GitPath, gix::Url>,
}

pub type SerdeRepoStates = HashMap<RepoName, Vec<SerdeThinCommit>>;

pub struct CommitLoader {
    toprepo: gix::Repository,
    repos: HashMap<RepoName, RepoFetcher>,
    /// Repositories that have been loaded from the cache.
    repos_loaded_from_cache: SerdeRepoStates,
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
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<TaskResult>();
        let pb_fetch_queue = progress
            .add(indicatif::ProgressBar::no_length().with_style(
                indicatif::ProgressStyle::with_template("{elapsed:>4} {msg}").unwrap(),
            ));
        // Make sure that the elapsed time is updated continuously.
        pb_fetch_queue.enable_steady_tick(std::time::Duration::from_millis(1000));
        Ok(Self {
            toprepo,
            repos: HashMap::new(),
            repos_loaded_from_cache: HashMap::new(),
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
        })
    }

    const TOPREPO_IMPORT_CACHE_PATH: &str = "toprepo/import_cache.bincode";
    const CACHE_VERSION_PRELUDE: &str = "#import-cache-format-v1\n";

    /// Constructs the path to the git repository information cache inside
    /// `.git/toprepo/`.
    pub fn cache_path(git_dir: &Path) -> PathBuf {
        git_dir.join(Self::TOPREPO_IMPORT_CACHE_PATH)
    }

    /// Load parsed git repository information from `.git/toprepo/`.
    pub fn load_cache(&mut self) -> Result<()> {
        self.repos_loaded_from_cache =
            Self::load_cache_from_dir(self.toprepo.git_dir(), &self.logger)?;
        Ok(())
    }

    /// Load parsed git repository information from `.git/toprepo/`.
    pub fn load_cache_from_dir(git_dir: &Path, logger: &Logger) -> Result<SerdeRepoStates> {
        let cache_path = Self::cache_path(git_dir);
        (|| -> anyhow::Result<_> {
            let now = std::time::Instant::now();
            let reader = match std::fs::File::open(&cache_path) {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // No cache file, skip reading.
                    return Ok(SerdeRepoStates::new());
                }
                Err(err) => return Err(err.into()),
            };
            let mut reader = std::io::BufReader::new(reader);
            let mut version_prelude = [0; Self::CACHE_VERSION_PRELUDE.len()];
            reader.read_exact(&mut version_prelude)?;
            if version_prelude != Self::CACHE_VERSION_PRELUDE.as_bytes() {
                logger.warning(format!(
                    "Discarding import cache {} due to version mismatch, expected {:?}",
                    cache_path.display(),
                    Self::CACHE_VERSION_PRELUDE
                ));
                return Ok(SerdeRepoStates::new());
            }

            let cached_repo_states: SerdeRepoStates =
                bincode::serde::decode_from_std_read(&mut reader, bincode::config::standard())?;
            let mut eof_buffer = [0; 1];
            if reader.read(&mut eof_buffer)? != 0 {
                anyhow::bail!("Expected EOF");
            }
            let file = reader.into_inner();
            drop(file);
            eprintln!(
                "DEBUG: Deserialized repo states from {} in {:.2?}",
                &cache_path.display(),
                now.elapsed()
            );
            Ok(cached_repo_states)
        })()
        .with_context(|| {
            format!(
                "Failed to deserialize repo states from {}",
                &cache_path.display()
            )
        })
    }

    fn prepare_repo_states_for_serialization(&self) -> SerdeRepoStates {
        let now = std::time::Instant::now();
        let ret = self
            .repos
            .iter()
            .map(|(repo_name, fetcher)| {
                let thin_commits = fetcher
                    .repo_data
                    .thin_commits
                    .values()
                    .sorted_by_key(|thin_commit| thin_commit.depth)
                    .map(|thin_commit| SerdeThinCommit::from(thin_commit.as_ref()))
                    .collect_vec();
                (repo_name.clone(), thin_commits)
            })
            .collect();
        eprintln!(
            "DEBUG: Prepared repo states for serialization in {:.2?}",
            now.elapsed()
        );
        ret
    }

    /// Write parsed git repository information as JSON.
    pub fn dump_cache_as_json<W>(writer: W, repo_states: &SerdeRepoStates) -> Result<()>
    where
        W: Write,
    {
        serde_json::to_writer_pretty(writer, &repo_states)
            .context("Failed to serialize repo states")
    }

    /// Store parsed git repository information from `.git/toprepo/`.
    pub fn store_cache(&self) -> Result<()> {
        let now = std::time::Instant::now();
        let cache_path = Self::cache_path(self.toprepo.git_dir());
        let cache_path_tmp = cache_path.with_extension(".tmp");
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&cache_path_tmp)?);
        writer.write_all(Self::CACHE_VERSION_PRELUDE.as_bytes())?;
        bincode::serde::encode_into_std_write(
            self.prepare_repo_states_for_serialization(),
            &mut writer,
            bincode::config::standard(),
        )
        .context("Failed to serialize repo states")?;
        let file = writer
            .into_inner()
            // .into_inner()
            .context("Failed to flush buffered writer")?;
        drop(file);
        std::fs::rename(cache_path_tmp, &cache_path)?;
        eprintln!(
            "DEBUG: Serialized repo states to {} in {:.2?}",
            cache_path.display(),
            now.elapsed()
        );
        Ok(())
    }

    /// Enqueue fetching of a repo with specific refspecs. Using `None` as
    /// refspec will fetch all heads and tags.
    pub fn fetch_repo(&mut self, repo_name: RepoName, refspecs: Vec<Option<String>>) {
        if let Err(err) = self.fetch_repo_impl(repo_name, refspecs) {
            self.logger.error(format!("{err:#}"));
        }
    }

    fn fetch_repo_impl(
        &mut self,
        repo_name: RepoName,
        mut refspecs: Vec<Option<String>>,
    ) -> Result<()> {
        let repo_fetcher = self.get_or_create_repo_fetcher(&repo_name)?;
        refspecs.retain(|refspec| !repo_fetcher.refspecs_done.contains(refspec));
        if refspecs.is_empty() {
            return Ok(());
        }
        let was_empty = repo_fetcher.refspecs_to_fetch.is_empty();
        repo_fetcher.refspecs_to_fetch.extend(refspecs);
        if was_empty {
            self.repos_to_fetch.push_back(repo_name);
        }
        Ok(())
    }

    /// Enqueue loading commits from a repo.
    pub fn load_repo(&mut self, repo_name: RepoName) -> Result<()> {
        let repo_fetcher = self.get_or_create_repo_fetcher(&repo_name)?;
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

    pub fn into_result(self) -> (RepoStates, GitTopRepoConfig) {
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
                self.fetch_progress.inc_num_loads_done();
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
            self.load_repo(repo_name)
                .expect("configuration exists for repo");
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
            .or_insert(thin_commit);

        // Any of the submodule updates that need to be fetched?
        for needed_commit in updated_submodule_commits {
            self.assure_commit_available(needed_commit);
        }
        Ok(())
    }

    fn import_cached_commit(
        repo: &gix::Repository,
        repo_name: &RepoName,
        repo_storage: &RepoData,
        cached_commit: SerdeThinCommit,
        config: &mut GitTopRepoConfig,
        dot_gitmodules_cache: &mut DotGitModulesCache,
        logger: &Logger,
    ) -> Result<Rc<ThinCommit>> {
        let commit_id: CommitId = cached_commit.commit_id;
        let thin_parents = cached_commit
            .parents
            .iter()
            .map(|parent_id| {
                repo_storage
                    .thin_commits
                    .get(parent_id)
                    .with_context(|| {
                        format!("Parent {} of {} not yet parsed", parent_id, commit_id)
                    })
                    .cloned()
            })
            .collect::<Result<Vec<_>>>()?;

        // Check for an updated .gitmodules file.
        let gitmodules_info = match &cached_commit.dot_gitmodules {
            Some(dot_gitmodules_oid) => match dot_gitmodules_cache
                .get_from_blob_id(repo, *dot_gitmodules_oid)
                .with_context(|| format!("Failed to parse .gitmodules in commit {commit_id}"))
            {
                Ok(gitmodules_info) => gitmodules_info,
                Err(err) => {
                    logger.warning(format!("{err:#}"));
                    &GitModulesInfo::default()
                }
            },
            None => &GitModulesInfo::default(),
        };
        let mut submodule_bumps = BTreeMap::new();
        for (path, cached_bump) in cached_commit.submodule_bumps {
            let thin_bump = match cached_bump {
                Some(submod_commit_id) => {
                    let submod_url = gitmodules_info.submodules.get(&path);
                    let context = format!("Repo {repo_name} commit {commit_id}");
                    let submod_repo_name = Self::get_submod_repo_name(
                        config,
                        &path,
                        submod_url,
                        &repo_storage.url,
                        &logger.with_context(&context),
                    )
                    .unwrap_or_else(|err| {
                        logger.error(format!("{}: {err:#}", context));
                        None
                    });
                    ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
                        repo_name: submod_repo_name,
                        commit_id: submod_commit_id,
                    })
                }
                None => ThinSubmodule::Removed,
            };
            submodule_bumps.insert(path, thin_bump);
        }

        let thin_commit = ThinCommit::new_rc(
            commit_id,
            cached_commit.tree_id,
            thin_parents,
            cached_commit.dot_gitmodules,
            submodule_bumps,
        );
        Ok(thin_commit)
    }

    fn get_or_create_repo_fetcher(
        &'_ mut self,
        repo_name: &RepoName,
    ) -> Result<&'_ mut RepoFetcher> {
        if !self.repos.contains_key(repo_name) {
            self.create_repo_fetcher(repo_name)?;
        }
        Ok(self.repos.get_mut(repo_name).expect("just added repos"))
    }

    /// Creates a `RepoFetcher` for the given repository name. The repository is
    /// populated from the cache if possible. If `repo_name` doesn't exist in
    /// the configuration, the creation will fail.
    fn create_repo_fetcher(&mut self, repo_name: &RepoName) -> Result<()> {
        let url = match &repo_name {
            RepoName::Top => crate::util::EMPTY_GIX_URL.clone(),
            RepoName::SubRepo(submod_repo_name) => {
                // Check if the submodule is configured.
                self.config
                    .subrepos
                    .get(submod_repo_name.deref())
                    .with_context(|| format!("Repo {repo_name} not found in config"))?
                    .resolve_fetch_url()
                    .clone()
            }
        };
        let mut repo_fetcher = RepoFetcher::new(url);
        // Load from cache, should be quick.
        let cached_commits = self
            .repos_loaded_from_cache
            .remove(repo_name)
            .unwrap_or_default();
        let mut needed_commits = Vec::new();
        for cached_commit in cached_commits {
            match Self::import_cached_commit(
                &self.toprepo,
                repo_name,
                &repo_fetcher.repo_data,
                cached_commit,
                &mut self.config,
                &mut self.dot_gitmodules_cache,
                &self.logger,
            ) {
                Ok(thin_commit) => {
                    // Load all submodule commits as well as that would be
                    // done if not using the cache.
                    for bump in thin_commit.submodule_bumps.values() {
                        if let ThinSubmodule::AddedOrModified(bump) = bump {
                            if let Some(submod_repo_name) = &bump.repo_name {
                                needed_commits.push(NeededCommit {
                                    repo_name: submod_repo_name.clone(),
                                    commit_id: bump.commit_id,
                                });
                            }
                        }
                    }
                    repo_fetcher
                        .repo_data
                        .thin_commits
                        .insert(thin_commit.commit_id, thin_commit);
                }
                Err(err) => {
                    // Failed to load the commit.
                    self.logger.error(format!("{err:#}"));
                    continue;
                }
            };
        }
        self.repos.insert(repo_name.clone(), repo_fetcher);
        for needed_commit in needed_commits {
            self.assure_commit_available(needed_commit);
        }
        Ok(())
    }

    fn assure_commit_available(&mut self, needed_commit: NeededCommit) {
        let repo_name: RepoName = RepoName::SubRepo(needed_commit.repo_name);
        let commit_id = needed_commit.commit_id;
        // Already loaded?
        let repo_fetcher = self
            .get_or_create_repo_fetcher(&repo_name)
            .expect("repo_name has been found in the configuration");
        if repo_fetcher.repo_data.thin_commits.contains_key(&commit_id)
            || repo_fetcher.missing_commits.contains(&commit_id)
        {
            return;
        }
        match repo_fetcher.loading {
            LoadRepoState::NotLoadedYet => {
                repo_fetcher.needed_commits.insert(commit_id);
                self.load_repo(repo_name)
                    .expect("configuration exists for repo");
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
                        format!("BUG: Parent {} of {} not yet parsed", parent_id, commit_id)
                    })
                    .cloned()
            })
            .collect::<Result<Vec<_>>>()?;
        let mut submodule_bumps = BTreeMap::new();

        // Check for an updated .gitmodules file.
        let mut dot_gitmodules = thin_parents
            .first()
            .and_then(|first_parent| first_parent.dot_gitmodules);
        let old_dot_gitmodules = dot_gitmodules;
        {
            let get_dot_gitmodules_logger =
                logger.with_context(&format!(".gitmodules in commit {commit_id}"));
            match Self::get_dot_gitmodules_update(&exported_commit, &get_dot_gitmodules_logger) {
                Ok(Some(new_dot_gitmodules)) => dot_gitmodules = new_dot_gitmodules,
                Ok(None) => (), // No update of .gitmodules.
                Err(err) => {
                    get_dot_gitmodules_logger.error(format!("{err:#}"));
                    // Keep the old dot_gitmodules content as that will probably expand the repository the best way.
                }
            };
        }
        let (gitmodules_info, dot_gitmodules) = match dot_gitmodules {
            Some(dot_gitmodules_oid) => match dot_gitmodules_cache
                .get_from_blob_id(repo, dot_gitmodules_oid)
                .context("Failed to parse .gitmodules")
            {
                Ok(gitmodules_info) => (gitmodules_info, Some(dot_gitmodules_oid)),
                Err(err) => {
                    logger.warning(format!("{err:#}"));
                    // Reset dot_gitmodules to avoid logging the same error again.
                    (&GitModulesInfo::default(), None)
                }
            },
            None => (&GitModulesInfo::default(), None),
        };
        // Look for submodule updates.
        let parent_submodule_paths = match thin_parents.first() {
            Some(first_parent) => &first_parent.submodule_paths,
            None => &HashSet::new(),
        };
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
                        submodule_bumps.insert(
                            path,
                            ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
                                repo_name: subrepo_name,
                                commit_id: submod_commit_id,
                            }),
                        );
                    } else if parent_submodule_paths.contains(&path) {
                        // It might be a submodule that changed to another
                        // type, e.g. tree or file. Remove it.
                        submodule_bumps.insert(path, ThinSubmodule::Removed);
                    }
                }
                ChangedFile::Deleted(fc) => {
                    // TODO: Implement borrow between BStr and GitPath to delay
                    // construction of a GitPath.
                    let path = GitPath::new(fc.path);
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
            // Loop through all submodules to see if any have changed.
            for path in parent_submodule_paths {
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
                logger.warning(format!("Missing {path} in .gitmodules"));
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

/// Serializeable version of `ThinCommit`.
#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SerdeThinCommit {
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub commit_id: CommitId,
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub tree_id: TreeId,
    #[serde_as(as = "Vec<serde_with::DisplayFromStr>")]
    pub parents: Vec<CommitId>,
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub dot_gitmodules: Option<BlobId>,
    /// `None` if the submodule was removed.
    pub submodule_bumps: HashMap<GitPath, Option<CommitId>>,
}

impl From<&ThinCommit> for SerdeThinCommit {
    fn from(thin_commit: &ThinCommit) -> Self {
        let submodule_bumps = thin_commit
            .submodule_bumps
            .iter()
            .map(|(path, bump)| {
                let commit_id_or_none = match bump {
                    ThinSubmodule::AddedOrModified(submod) => Some(submod.commit_id),
                    ThinSubmodule::Removed => None,
                };
                (path.clone(), commit_id_or_none)
            })
            .collect();
        Self {
            commit_id: thin_commit.commit_id,
            tree_id: thin_commit.tree_id,
            parents: thin_commit.parents.iter().map(|p| p.commit_id).collect(),
            dot_gitmodules: thin_commit.dot_gitmodules,
            submodule_bumps,
        }
    }
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

impl RepoFetcher {
    pub fn new(url: gix::Url) -> Self {
        Self {
            repo_data: RepoData::new(url),
            needed_commits: HashSet::new(),
            loading: LoadRepoState::NotLoadedYet,
            missing_commits: HashSet::new(),
            refspecs_to_fetch: HashSet::new(),
            refspecs_done: HashSet::new(),
        }
    }

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
    num_loads_done: usize,
    event_queue_size: usize,
}

impl FetchProgress {
    fn new(pb: indicatif::ProgressBar) -> Self {
        let ret = Self {
            pb,
            fetch_queue_size: 0,
            num_fetches_done: 0,
            num_loads_done: 0,
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

    pub fn inc_num_loads_done(&mut self) {
        self.num_loads_done += 1;
        self.draw();
    }

    fn draw(&self) {
        let mut msg = String::new();
        if self.fetch_queue_size != 0 {
            msg.push_str(&format!(
                "{} {} in queue for fetching",
                self.fetch_queue_size,
                if self.fetch_queue_size == 1 {
                    "repository"
                } else {
                    "repositories"
                },
            ));
        }
        msg.push_str(&format!(
            ", {} {} done",
            self.num_fetches_done,
            if self.num_fetches_done == 1 {
                "fetch"
            } else {
                "fetches"
            },
        ));
        if self.fetch_queue_size > 0 {
            msg += &format!(" ({} in queue)", self.fetch_queue_size);
        }
        msg.push_str(&format!(
            ", {} {} done",
            self.num_loads_done,
            if self.num_loads_done == 1 {
                "load"
            } else {
                "loads"
            },
        ));
        self.pb.set_message(msg);
    }
}
