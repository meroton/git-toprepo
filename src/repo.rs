use crate::config::TOPREPO_CONFIG_FILE_KEY;
use crate::config::toprepo_git_config;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git::git_command;
use crate::git::git_global_command;
use crate::git_fast_export_import::WithoutCommitterId;
use crate::git_fast_export_import_dedup::GitFastExportImportDedupCache;
use crate::loader::SubRepoLedger;
use crate::log::CommandSpanExt as _;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::CommandExtension as _;
use crate::util::NewlineTrimmer as _;
use crate::util::RcKey;
use crate::util::normalize;
use crate::error::NotAMonorepo;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use bstr::ByteSlice as _;
use serde_with::serde_as;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::hash::Hash;
use std::io::Write as _;
use std::ops::Deref;
use std::path::Path;
use std::rc::Rc;

// TODO(terminology): see if the important part is the git repository itself and
// the toprepo, or whether the important error is that it is not a monorepo.
pub const COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY: &str =
    "Could not open toprepo";
pub const LOADING_THE_MAIN_PROJECT_CONTEXT: &str = "Loading the main repo Gerrit project";

pub fn parse_gerrit_project(url: &gix::url::Url) -> Result<String> {
    // TODO use `url.scheme`
    let tail = url
        .path_argument_safe()
        .ok_or(anyhow!("Could not parse url to string."))?
        .to_owned()
        .to_string();
    let sans_slash = tail.strip_prefix("/").get_or_insert(&tail).to_string();
    Ok(sans_slash)
}

// TODO: A specific type for the resolved subprojects?
pub fn resolve_subprojects(
    subs: &GitModulesInfo,
    main_project: String,
) -> Result<HashMap<GitPath, String>> {
    let mut resolved = HashMap::<GitPath, String>::default();

    for (path, url) in subs.submodules.iter() {
        // TODO: Nightly `as_str`: https://docs.rs/bstr/latest/bstr/struct.BString.html#deref-methods-%5BT%5D-1
        let relative = parse_gerrit_project(url.as_ref().unwrap())?;
        let relative = match relative.strip_prefix("/") {
            None => relative,
            Some(r) => r.to_owned(),
        };

        let project = normalize(&format!("{}/{}", &main_project, relative));
        resolved.insert(path.clone(), project);
    }

    Ok(resolved)
}

#[derive(Debug)]
pub struct TopRepo {
    pub gix_repo: gix::Repository,
}

/// Handle for either a basic or fully configured toprepo
/// This provides a unified interface for commands that may or may not need full config
// TODO(terminology pr#172): emulated monorepo as opposed to a toprepo.
pub enum RepoHandle {
    /// Basic git repository access (for discovery/bootstrap operations)
    /// Includes an error to explain why it could not be opened as a Toprepo.
    Basic(TopRepo, Option<anyhow::Error>),
    /// Fully configured toprepo with all state loaded
    Configured(ConfiguredTopRepo),
}

impl<E> TryFrom<Result<RepoHandle, E>> for ConfiguredTopRepo
where
    E: Into<anyhow::Error> + Send + Sync + 'static {
    type Error = NotAMonorepo;

    fn try_from(value: Result<RepoHandle, E>) -> std::result::Result<Self, Self::Error> {
        match value {
            Ok(RepoHandle::Basic(_, Some(err))) => Err(NotAMonorepo::new(err)),
            Ok(RepoHandle::Basic(_, None)) => Err(NotAMonorepo::default()),
            Ok(RepoHandle::Configured(toprepo)) => Ok(toprepo),
            Err(e) => Err(NotAMonorepo::new(e.into())),
        }
    }
}

impl TryFrom<RepoHandle> for ConfiguredTopRepo {
    type Error = NotAMonorepo;

    fn try_from(value: RepoHandle) -> std::result::Result<Self, Self::Error> {
        match value {
            RepoHandle::Basic(_, Some(err)) => Err(NotAMonorepo::new(err)),
            RepoHandle::Basic(_, None) => Err(NotAMonorepo::default()),
            RepoHandle::Configured(toprepo) => Ok(toprepo),
        }
    }
}

/*
 * // TODO: TopRepo is decidedly a bad term form this!
 * // It is a plain not-monorepo git repo.
 * impl<E> TryFrom<Result<RepoHandle, E>> for TopRepo
 * where
 *     E: Display + Send + Sync + 'static {
 *     type Error = NotAMonorepo;
 *
 *     fn try_from(value: Result<RepoHandle, E>) -> std::result::Result<Self, Self::Error> {
 *         match value {
 *             Ok(RepoHandle::Basic(repo)) => Ok(repo),
 *             Ok(RepoHandle::Configured(toprepo)) => Err(AlreadyAMonorepo),
 *             // TODO: I want to contextualize the other error in here.
 *             // Err(e) => Err(NotAMonorepo).context(e),
 *             Err(e) => Err(e),
 *         }
 *     }
 * }
 */

/// A fully configured toprepo with all state loaded
/// This unifies access to both raw git operations and toprepo configuration
// TODO(terminology pr#172): emulated monorepo as opposed to a toprepo.
pub struct ConfiguredTopRepo {
    pub gix_repo: gix::Repository,
    pub config: crate::config::GitTopRepoConfig,
    // TODO: Use interior mutability (RefCell/Mutex) for ledger to avoid requiring
    // mutable references to the entire ConfiguredTopRepo struct during operations
    pub ledger: crate::loader::SubRepoLedger,
    pub top_repo_cache: TopRepoCache,
    pub progress: indicatif::MultiProgress,
    // TODO: Revisit whether caching gerrit project name is worth the complexity
    // vs just parsing it on-demand from the git remote URL
    cached_gerrit_project: String,
}

impl TopRepo {
    pub fn create(directory: &Path, url: gix::url::Url) -> Result<TopRepo> {
        git_global_command()
            .arg("init")
            .arg("--quiet")
            .arg(directory.as_os_str())
            .trace_command(crate::command_span!("git init"))
            .safe_status()?
            .check_success()
            .context("Failed to initialize git repository")?;
        git_command(directory)
            .args([
                "config",
                "remote.origin.pushUrl",
                "https://ERROR.invalid/Please use 'git toprepo push ...' instead",
            ])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.pushUrl")?;
        git_command(directory)
            .args(["config", "remote.origin.url", &url.to_string()])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.url")?;
        let toprepo_ref_prefix: String = RepoName::Top.to_ref_prefix();
        git_command(directory)
            .args([
                "config",
                "--replace-all",
                "remote.origin.fetch",
                &format!("+refs/heads/*:{toprepo_ref_prefix}refs/remotes/origin/*"),
            ])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (heads)")?;
        git_command(directory)
            .args([
                "config",
                "--add",
                "remote.origin.fetch",
                &format!("+refs/tags/*:{toprepo_ref_prefix}refs/tags/*"),
            ])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (tags)")?;
        // TODO: Does HEAD always exist on the remote? Is `git ls-remote` needed
        // to prioritize HEAD, main, master, etc.
        git_command(directory)
            .args([
                "config",
                "--add",
                "remote.origin.fetch",
                &format!("+HEAD:{toprepo_ref_prefix}refs/remotes/origin/HEAD"),
            ])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (HEAD)")?;
        git_command(directory)
            .args(["config", "remote.origin.tagOpt", "--no-tags"])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.tagOpt")?;
        let key = &toprepo_git_config(TOPREPO_CONFIG_FILE_KEY);
        git_command(directory)
            .args([
                "config",
                key,
                &format!("repo:{toprepo_ref_prefix}refs/remotes/origin/HEAD:.gittoprepo.toml"),
            ])
            .trace_command(crate::command_span!("git config"))
            .safe_status()?
            .check_success()
            .context("Failed to set git-config {key}")?;

        let result = {
            let (process, _span_guard) = git_command(directory)
                .args(["hash-object", "-t", "blob", "-w", "--stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .trace_command(crate::command_span!("git hash-object"))
                .spawn()?;
            process.wait_with_output()
        }?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_blob_hash = result.stdout.to_str()?.trim_newline_suffix();

        let result = {
            let (mut process, _span_guard) = git_command(directory)
                .arg("mktree")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .trace_command(crate::command_span!("git mktree"))
                .spawn()?;
            let mut stdin = process.stdin.take().expect("stdin is piped");
            stdin.write_all(
                format!("100644 blob {gittoprepotoml_blob_hash}\t.gittoprepo.toml\n").as_bytes(),
            )?;
            drop(stdin);
            process.wait_with_output()
        }?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_tree_hash = bstr::BStr::new(result.stdout.trim_newline_suffix());

        let result = {
            let (mut process, _span_guard) = git_command(directory)
                .args(["hash-object", "-t", "commit", "-w", "--stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .trace_command(crate::command_span!("git hash-object"))
                .spawn()?;
            let mut stdin = process.stdin.take().expect("stdin is piped");
            stdin.write_all(
                format!(
                    "\
tree {gittoprepotoml_tree_hash}
author Git Toprepo <noname@example.com> 946684800 +0000
committer Git Toprepo <noname@example.com> 946684800 +0000

Initial empty git-toprepo configuration
"
                )
                .as_bytes(),
            )?;
            drop(stdin);
            process.wait_with_output()
        }?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_commit_hash = bstr::BStr::new(result.stdout.trim_newline_suffix());

        let first_time_config_ref = toprepo_ref_prefix + "refs/remotes/origin/HEAD";
        git_command(directory)
            .arg("update-ref")
            .arg(&first_time_config_ref)
            .arg(gittoprepotoml_commit_hash.to_os_str()?)
            .trace_command(crate::command_span!("git update-ref"))
            .safe_status()?
            .check_success()
            .with_context(|| format!("Failed to reset {first_time_config_ref}"))?;
        Self::open(directory)
    }

    pub fn open(directory: &Path) -> Result<TopRepo> {
        let gix_repo =
            gix::open(directory).context(COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY)?;
        Ok(TopRepo {
            gix_repo: gix_repo,
        })
    }

    /// Open a repository, trying configured first, falling back to basic
    /// This is the main entry point for command operations
    pub fn open_for_commands(directory: &Path) -> Result<RepoHandle> {
        let gix_repo = gix::open(directory)
            .context(COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY)?;
        Ok(match Self::open_configured(directory) {
            Ok(monorepo) => RepoHandle::Configured(monorepo),
            Err(err) => RepoHandle::Basic(TopRepo{gix_repo: gix_repo.into()}, Some(err)),
        })
    }

    /// Open a toprepo with full configuration and state loaded
    /// This centralizes the state loading that's currently scattered in MonoRepoProcessor::run
    pub fn open_configured(directory: &Path) -> Result<ConfiguredTopRepo> {
        let gix_repo = gix::open(directory)
            .context(COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY)?;

        let config = crate::config::GitTopRepoConfig::load_config_from_repo(
            gix_repo
                .worktree()
                .with_context(|| {
                    format!(
                        "Bare repository without worktree {}",
                        gix_repo.git_dir().display()
                    )
                })?
                .base(),
        )?;

        let ledger = crate::loader::SubRepoLedger {
            subrepos: config.subrepos.clone(),
            missing_subrepos: std::collections::HashSet::new(),
        };

        let top_repo_cache = crate::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
            gix_repo.git_dir(),
            Some(&config.checksum),
        )
        .with_context(|| format!("Loading cache from {}", gix_repo.git_dir().display()))?
        .unpack()?;

        let progress = indicatif::MultiProgress::new();

        // Cache gerrit project name during initialization
        let cached_gerrit_project = {
            let url = crate::git::get_default_remote_url(&gix_repo)?;
            parse_gerrit_project(&url).with_context(|| format!("Parse gerrit project from {url}"))?
        };

        Ok(ConfiguredTopRepo {
            gix_repo,
            config,
            ledger,
            top_repo_cache,
            progress,
            cached_gerrit_project,
        })
    }

    /// Get the main worktree path of the repository.
    /// If the user has multiple worktrees
    /// this may not be the current working directory.
    pub fn main_worktree(&self) -> Result<&Path> {
        self.gix_repo.workdir().with_context(|| {
            format!(
                "Bare repository without worktree {}",
                self.gix_repo.git_dir().display()
            )
        })
    }

    // TODO: This should be unified with the information about modules found
    // through the ToprepoConfig and Processor data.
    // #unified-git-config.
    // NOTE: ConfiguredTopRepo::submodules() provides the unified implementation
    // that uses config data instead of parsing .gitmodules each time.
    pub fn submodules(&self) -> Result<HashMap<GitPath, String>> {
        let modules = self.gix_repo.modules()?;
        if modules.is_none() {
            return Ok(HashMap::new());
        }
        let modules = modules.unwrap();
        let main_project = self.gerrit_project()?;

        let mut info = GitModulesInfo::default();
        for name in modules.names() {
            let path = modules.path(name)?;
            let url = modules.url(name)?;
            info.submodules
                .insert(GitPath::new(path.into_owned()), Ok(url));
        }

        resolve_subprojects(&info, main_project)
    }

    pub fn gerrit_project(&self) -> Result<String> {
        let url = crate::git::get_default_remote_url(&self.gix_repo)?;
        parse_gerrit_project(&url).with_context(|| format!("Parse gerrit project from {url}"))
    }
}

impl ConfiguredTopRepo {
    /// Get submodules using the authoritative config data (more efficient & consistent)
    /// This uses the loaded configuration instead of parsing .gitmodules each time
    pub fn submodules(&self) -> Result<HashMap<GitPath, String>> {
        // Convert from the ledger's subrepo data to the GitPath -> project string format
        // This ensures we use the same data that operations actually work with
        let mut result = HashMap::new();

        for (sub_repo_name, sub_config) in &self.ledger.subrepos {
            // We need to find the GitPath for each SubRepoName
            // For now, we'll use the URL to derive the path (this could be enhanced)
            // TODO: Consider storing the GitPath -> SubRepoName mapping in the ledger
            let url = sub_config.resolve_fetch_url();
            if let Ok(project) = parse_gerrit_project(url) {
                // Derive path from repo name - this is a simplification
                // In a full implementation, we'd want the actual .gitmodules path mapping
                let path = GitPath::new(sub_repo_name.to_string().as_bytes().into());
                result.insert(path, format!("{}/{}", &self.cached_gerrit_project, project));
            }
        }

        Ok(result)
    }

    /// Get gerrit project using cached value (more efficient)
    pub fn gerrit_project(&self) -> &str {
        &self.cached_gerrit_project
    }

    /// Reload configuration (preserving ledger state)
    pub fn reload_config(&mut self) -> Result<()> {
        let new_config = crate::config::GitTopRepoConfig::load_config_from_repo(
            self.gix_repo
                .worktree()
                .with_context(|| {
                    format!(
                        "Bare repository without worktree {}",
                        self.gix_repo.git_dir().display()
                    )
                })?
                .base(),
        )?;

        // Preserve any missing_subrepos from current ledger state
        let preserved_missing_subrepos = std::mem::take(&mut self.ledger.missing_subrepos);
        self.ledger.subrepos = new_config.subrepos.clone();
        self.ledger.missing_subrepos = preserved_missing_subrepos;

        self.config = crate::config::GitTopRepoConfig {
            checksum: new_config.checksum,
            fetch: new_config.fetch,
            subrepos: self.ledger.subrepos.clone(),
        };

        Ok(())
    }

    /// Save state (config + cache) back to disk
    pub fn save_state(&mut self) -> Result<()> {
        // Save cache
        if let Err(err) = crate::repo_cache_serde::SerdeTopRepoCache::pack(
            &self.top_repo_cache,
            self.config.checksum.clone(),
        ).store_to_git_dir(self.gix_repo.git_dir()) {
            return Err(err);
        }

        // Save effective config with ledger mutations
        const EFFECTIVE_TOPREPO_CONFIG: &str = "toprepo/last-effective-git-toprepo.toml";
        let config_path = self.gix_repo.git_dir().join(EFFECTIVE_TOPREPO_CONFIG);
        let updated_config = crate::config::GitTopRepoConfig {
            checksum: self.config.checksum.clone(),
            fetch: self.config.fetch.clone(),
            subrepos: self.ledger.subrepos.clone(),
        };
        updated_config.save(&config_path)?;

        Ok(())
    }
}

impl RepoHandle {
    /// Save state if this is a configured repo (no-op for basic repos)
    pub fn save_state_if_configured(&mut self) -> Result<()> {
        if let RepoHandle::Configured(configured) = self {
            configured.save_state()?;
        }
        Ok(())
    }
}

pub struct MonoRepoProcessor<'a> {
    pub gix_repo: &'a gix::Repository,
    // NB: To keep it in a mutable reference would save some time and space
    // but pollutes the code with all the `muts` that are only meant to happen
    // through the reload function.
    pub config: crate::config::GitTopRepoConfig,
    pub ledger: &'a mut crate::loader::SubRepoLedger,
    pub top_repo_cache: &'a mut crate::repo::TopRepoCache,
    pub progress: &'a mut indicatif::MultiProgress,
}

impl MonoRepoProcessor<'_> {
    pub fn run<T, F>(directory: &Path, f: F) -> Result<T>
    where
        F: FnOnce(&mut MonoRepoProcessor) -> Result<T>,
    {
        let gix_repo =
            gix::open(directory).context("Could not open directory for MonoRepoProcessor")?;
        let config = crate::config::GitTopRepoConfig::load_config_from_repo(
            gix_repo
                .worktree()
                .with_context(|| {
                    format!(
                        "Bare repository without worktree {}",
                        gix_repo.git_dir().display()
                    )
                })?
                .base(),
        )?;
        let mut ledger = SubRepoLedger{
            subrepos: config.subrepos.clone(),
            missing_subrepos: HashSet::new(),
        };
        let mut top_repo_cache = crate::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
            gix_repo.git_dir(),
            Some(&config.checksum),
        )
        .with_context(|| format!("Loading cache from {}", gix_repo.git_dir().display()))?
        .unpack()?;
        let mut progress = indicatif::MultiProgress::new();
        let mut processor = MonoRepoProcessor {
            gix_repo: &gix_repo,
            config: config,
            ledger: &mut ledger,
            top_repo_cache: &mut top_repo_cache,
            progress: &mut progress,
        };
        processor
            .progress
            .set_draw_target(indicatif::ProgressDrawTarget::hidden());
        let mut result = crate::log::get_global_logger().with_progress(|progress| {
            let old_progress = std::mem::replace(processor.progress, progress);
            let result = f(&mut processor);
            *processor.progress = old_progress;
            result
        });
        // Store some result files.
        if let Err(err) = crate::repo_cache_serde::SerdeTopRepoCache::pack(
            processor.top_repo_cache,
            processor.config.checksum.clone(),
        )
        .store_to_git_dir(processor.gix_repo.git_dir())
            && result.is_ok()
        {
            result = Err(err);
        }
        const EFFECTIVE_TOPREPO_CONFIG: &str = "toprepo/last-effective-git-toprepo.toml";
        let config_path = processor.gix_repo.git_dir().join(EFFECTIVE_TOPREPO_CONFIG);
        // Create updated config with ledger mutations
        let updated_config = crate::config::GitTopRepoConfig {
            checksum: processor.config.checksum.clone(),
            fetch: processor.config.fetch.clone(),
            subrepos: processor.ledger.subrepos.clone(),
        };
        if let Err(err) = updated_config.save(&config_path)
            && result.is_ok()
        {
            result = Err(err);
        }
        result
    }

    /// Reload the git-toprepo configuration in case anything has changed. Also
    /// check if the top repo cache is still valid given the new configuration.
    pub fn reload_config(&mut self) -> Result<()> {
        let new_config = crate::config::GitTopRepoConfig::load_config_from_repo(
            self.gix_repo
                .worktree()
                .with_context(|| {
                    format!(
                        "Bare repository without worktree {}",
                        self.gix_repo.git_dir().display()
                    )
                })?
                .base(),
        )?;

        // Preserve any missing_subrepos from current ledger state
        let preserved_missing_subrepos = std::mem::take(&mut self.ledger.missing_subrepos);

        self.ledger.subrepos = new_config.subrepos.clone();
        self.ledger.missing_subrepos = preserved_missing_subrepos;

        self.config = crate::config::GitTopRepoConfig {
            checksum: new_config.checksum,
            fetch: new_config.fetch,
            subrepos: self.ledger.subrepos.clone(),
        };

        Ok(())
    }
}

#[serde_as]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TopRepoCommitId(
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")] CommitId,
);

impl TopRepoCommitId {
    pub fn new(commit_id: CommitId) -> Self {
        TopRepoCommitId(commit_id)
    }

    pub fn into_inner(self) -> CommitId {
        self.0
    }
}

impl Deref for TopRepoCommitId {
    type Target = CommitId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for TopRepoCommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub type RepoStates = HashMap<RepoName, RepoData>;

// TODO: Use `Rc` to all the `GitPath`s and `ObjectId`s to avoid memory duplication.
// Is it really more efficient to use `Rc`?
#[derive(Default)]
pub struct TopRepoCache {
    pub repos: RepoStates,
    pub monorepo_commits: HashMap<MonoRepoCommitId, Rc<MonoRepoCommit>>,
    pub monorepo_commit_ids: HashMap<RcKey<MonoRepoCommit>, MonoRepoCommitId>,
    /// Mapping from top repo commit to mono repo commit. To avoid confusion,
    /// entries are only allowed when a `MonoRepoCommitId` is known.
    pub top_to_mono_commit_map: HashMap<TopRepoCommitId, (MonoRepoCommitId, Rc<MonoRepoCommit>)>,
    pub dedup: GitFastExportImportDedupCache,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OriginalSubmodParent {
    // TODO: Unused?
    pub path: GitPath,
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub commit_id: CommitId,
}

#[derive(Clone)]
pub enum MonoRepoParent {
    OriginalSubmod(OriginalSubmodParent),
    Mono(Rc<MonoRepoCommit>),
}

#[serde_as]
#[derive(
    Debug, Clone, Copy, Eq, Hash, PartialEq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct MonoRepoCommitId(
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")] CommitId,
);

impl MonoRepoCommitId {
    pub fn new(commit_id: CommitId) -> Self {
        MonoRepoCommitId(commit_id)
    }
}

impl Display for MonoRepoCommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Deref for MonoRepoCommitId {
    type Target = CommitId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct MonoRepoCommit {
    pub parents: Vec<MonoRepoParent>,
    /// The depth in the mono repo, i.e. the number of commits in the longest
    /// history path.
    pub depth: usize,
    /// Potential update of the top repo content in this mono repo commit.
    pub top_bump: Option<TopRepoCommitId>,
    /// The original commits that were updated in this mono repo commit, recursively.
    pub submodule_bumps: HashMap<GitPath, ExpandedOrRemovedSubmodule>,
    /// The expanded submodule paths in this mono repo commit, recursively.
    pub submodule_paths: Rc<HashSet<GitPath>>,
}

impl MonoRepoCommit {
    pub fn new_rc(
        parents: Vec<MonoRepoParent>,
        top_bump: Option<TopRepoCommitId>,
        submodule_bumps: HashMap<GitPath, ExpandedOrRemovedSubmodule>,
    ) -> Rc<MonoRepoCommit> {
        let depth = parents
            .iter()
            .filter_map(|p| match p {
                MonoRepoParent::Mono(parent) => Some(parent.depth + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        // Adding and removing more than one submodule at a time is so rare that
        // it is not worth optimizing for it. Let's copy the HashSet every time.
        let mut submodule_paths = match parents.first() {
            Some(MonoRepoParent::Mono(first_parent)) => first_parent.submodule_paths.clone(),
            Some(MonoRepoParent::OriginalSubmod(_)) | None => Rc::new(HashSet::new()),
        };
        for (path, bump) in submodule_bumps.iter() {
            match bump {
                ExpandedOrRemovedSubmodule::Expanded(_) => {
                    submodule_paths = Rc::new({
                        let mut paths = submodule_paths.as_ref().clone();
                        paths.insert(path.clone());
                        paths
                    });
                }
                ExpandedOrRemovedSubmodule::Removed => {
                    submodule_paths = Rc::new({
                        let mut paths = submodule_paths.as_ref().clone();
                        paths.remove(path);
                        paths
                    });
                }
            }
        }
        Rc::new(MonoRepoCommit {
            parents,
            depth,
            top_bump,
            submodule_bumps,
            submodule_paths,
        })
    }

    pub fn is_ancestor_of(&self, descendant: &Rc<MonoRepoCommit>) -> bool {
        // Doesn't matter in which order we iterate.
        let mut visited = HashSet::new();
        let mut queue = Vec::new();
        visited.insert(RcKey::new(descendant));
        queue.push(descendant);

        while let Some(descendant) = queue.pop() {
            if std::ptr::addr_eq(Rc::as_ptr(descendant), self) {
                return true;
            }
            for descendant_parent in &descendant.parents {
                match descendant_parent {
                    MonoRepoParent::OriginalSubmod(_) => {}
                    MonoRepoParent::Mono(descendant_parent) => {
                        if descendant_parent.depth >= self.depth
                            && visited.insert(RcKey::new(descendant_parent))
                        {
                            queue.push(descendant_parent);
                        }
                    }
                }
            }
        }
        false
    }
}

#[serde_as]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExpandedSubmodule {
    /// Known submodule and known commit.
    Expanded(SubmoduleContent),
    /// The submodule was not expanded. The used has to run `git submodule
    /// update --init` to get its content.
    KeptAsSubmodule(
        #[serde_as(serialize_as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
        CommitId,
    ),
    /// The commit does not exist (any more) in the referred sub repository.
    CommitMissingInSubRepo(SubmoduleContent),
    /// It is unknown which sub repo it should be loaded from.
    UnknownSubmodule(
        #[serde_as(serialize_as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
        CommitId,
    ),
    // TODO: MovedAndBumped(MovedSubmodule),
    /// If a submodule has regressed to an earlier or unrelated commit, it
    /// should be expanded with a different set of parents submodules. The
    /// reason is that there should not be merge lines over a revert point as
    /// those merges makes no sense.
    ///
    /// Consider the following example:
    /// ```txt
    /// Submodule:
    /// * z
    /// * y
    /// * x
    ///
    /// Top repo:
    /// * C with z
    /// * B with x
    /// * A with y
    ///
    /// Mono repo (not acceptable):
    /// * C with z
    /// |\
    /// * |  B with x
    /// |/
    /// * A with y
    /// ```
    /// This mono repo version includes a merge line from A to C after the
    /// submodule was reverted in B. The merge line does no bring any new
    /// information and is simply redundant. This means that we are missing `y`
    /// in the history between `x` in B and `z` in C. Instead, the following
    /// history is wanted:
    /// ```txt
    /// Mono repo (acceptable):
    /// * C with z
    /// |\
    /// | * B with y
    /// |/
    /// * B with x
    /// |\
    /// | * Resetting to x
    /// |/
    /// * A with y
    /// ```
    // TODO: Implement this in the
    // TopRepoExpander::get_recursive_submodule_bumps() or extract the
    // information from TopRepoExpander::expand_inner_submodules().
    RegressedNotFullyImplemented(SubmoduleContent),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExpandedOrRemovedSubmodule {
    Expanded(ExpandedSubmodule),
    Removed,
}

#[serde_as]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubmoduleContent {
    pub repo_name: SubRepoName,
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub orig_commit_id: CommitId,
}

impl ExpandedSubmodule {
    /// Returns the submodule content if the submodule could be resolved, i.e.
    /// .gitmodules information was accurate.
    pub fn get_known_submod(&self) -> Option<&SubmoduleContent> {
        match self {
            ExpandedSubmodule::Expanded(submod) => Some(submod),
            ExpandedSubmodule::KeptAsSubmodule(_commit_id) => None,
            ExpandedSubmodule::CommitMissingInSubRepo(submod) => Some(submod),
            ExpandedSubmodule::UnknownSubmodule(_commit_id) => None,
            ExpandedSubmodule::RegressedNotFullyImplemented(submod) => Some(submod),
        }
    }

    pub fn get_orig_commit_id(&self) -> &CommitId {
        match self {
            ExpandedSubmodule::Expanded(submod) => &submod.orig_commit_id,
            ExpandedSubmodule::KeptAsSubmodule(commit_id) => commit_id,
            ExpandedSubmodule::CommitMissingInSubRepo(submod) => &submod.orig_commit_id,
            ExpandedSubmodule::UnknownSubmodule(commit_id) => commit_id,
            ExpandedSubmodule::RegressedNotFullyImplemented(submod) => &submod.orig_commit_id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RepoData {
    pub url: gix::Url,
    pub thin_commits: HashMap<CommitId, Rc<ThinCommit>>,
    /// A map for git-fast-import commit deduplicating, where the exported
    /// commit have different committer but otherwise are exactly the same.
    /// The values represent the latest imported or exported commit id.
    pub dedup_cache: HashMap<WithoutCommitterId, CommitId>,
}

impl RepoData {
    pub fn new(url: gix::Url) -> Self {
        Self {
            url,
            thin_commits: HashMap::new(),
            dedup_cache: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThinSubmodule {
    AddedOrModified(ThinSubmoduleContent),
    Removed,
}

#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThinSubmoduleContent {
    /// `None` is the submodule could not be resolved from the .gitmodules file.
    pub repo_name: Option<SubRepoName>,
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub commit_id: CommitId,
}

/// A file entry received from git-fast-export pointing to a specific blob.
#[serde_as]
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExportedFileEntry {
    /// The mode reported by git-fast-export.
    #[serde_as(as = "crate::util::SerdeOctalNumber")]
    pub mode: u32,
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub id: BlobId,
}

#[derive(Debug)]
pub struct ThinCommit {
    pub commit_id: CommitId,
    pub tree_id: TreeId,
    /// Number of parents in the longest path to the root commit. This number is
    /// strictly decreasing when following the parents.
    pub depth: u32,
    pub parents: Vec<Rc<ThinCommit>>,
    pub dot_gitmodules: Option<ExportedFileEntry>,
    /// Submodule updates in this commit compared to first parent. Added
    /// submodules are included. `BTreeMap` is used for deterministic ordering.
    pub submodule_bumps: BTreeMap<GitPath, ThinSubmodule>,
    /// Paths to all the submodules in the commit, not just the updated ones.
    pub submodule_paths: Rc<HashSet<GitPath>>,
}

impl ThinCommit {
    /// Creates a new `ThinCommit` which is effectively read only due to the
    /// reference counting.
    ///
    /// It is an error to try to update the contents of the `ThinCommit` after
    /// it has been created.
    pub fn new_rc(
        commit_id: CommitId,
        tree_id: TreeId,
        parents: Vec<Rc<ThinCommit>>,
        dot_gitmodules: Option<ExportedFileEntry>,
        submodule_bumps: BTreeMap<GitPath, ThinSubmodule>,
    ) -> Rc<Self> {
        // Adding and removing more than one submodule at a time is so rare that
        // it is not worth optimizing for it. Let's copy the HashSet every time.
        let mut submodule_paths = match parents.first() {
            Some(first_parent) => first_parent.submodule_paths.clone(),
            None => Rc::new(HashSet::new()),
        };
        for (path, bump) in submodule_bumps.iter() {
            match bump {
                ThinSubmodule::AddedOrModified(_) => {
                    submodule_paths = Rc::new({
                        let mut paths = submodule_paths.as_ref().clone();
                        paths.insert(path.clone());
                        paths
                    });
                }
                ThinSubmodule::Removed => {
                    submodule_paths = Rc::new({
                        let mut paths = submodule_paths.as_ref().clone();
                        paths.remove(path);
                        paths
                    });
                }
            }
        }
        Rc::new(Self {
            commit_id,
            tree_id,
            depth: parents.iter().map(|p| p.depth + 1).max().unwrap_or(0),
            parents,
            dot_gitmodules,
            submodule_bumps,
            submodule_paths,
        })
    }

    pub fn is_descendant_of(&self, ancestor: &ThinCommit) -> bool {
        // Doesn't matter in which order we iterate.
        let mut visited = HashSet::new();
        let mut queue = Vec::new();
        visited.insert(self.commit_id);
        queue.push(self);

        while let Some(descendant) = queue.pop() {
            if descendant.commit_id == ancestor.commit_id {
                return true;
            }
            for descendant_parent in &descendant.parents {
                if descendant_parent.depth >= ancestor.depth
                    && visited.insert(descendant_parent.commit_id)
                {
                    queue.push(descendant_parent);
                }
            }
        }
        false
    }

    /// Walks the first parent commit graph to the submodule entry.
    pub fn get_submodule(&'_ self, path: &GitPath) -> Option<&'_ ThinSubmodule> {
        let mut node = self;
        loop {
            if let Some(submod) = node.submodule_bumps.get(path) {
                return Some(submod);
            }
            let Some(parent) = node.parents.first() else {
                break;
            };
            node = parent;
        }
        None
    }
}
