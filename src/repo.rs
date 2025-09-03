use crate::config::GitTopRepoConfig;
use crate::config::TOPREPO_CONFIG_FILE_KEY;
use crate::config::toprepo_git_config;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git::git_command;
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

pub fn gix_discover(directory: &Path) -> Result<gix::Repository> {
    let repo = gix::ThreadSafeRepository::discover_with_environment_overrides(directory)?;
    Ok(repo.to_thread_local())
}

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

// TODO: 2025-09-22 A specific type for the resolved subprojects?
pub fn resolve_subprojects(
    subs: &GitModulesInfo,
    main_project: String,
) -> Result<HashMap<GitPath, String>> {
    let mut resolved = HashMap::<GitPath, String>::default();

    for (path, url) in subs.submodules.iter() {
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

// TODO: 2025-09-22 This should be unified with the information about modules found
// through the ToprepoConfig and Processor data.
// #unified-git-config.
// NOTE: ConfiguredTopRepo::submodules() provides the unified implementation
// that uses config data instead of parsing .gitmodules each time.
pub fn get_submodules(gix_repo: &gix::Repository) -> Result<HashMap<GitPath, String>> {
    let modules = gix_repo.modules()?;
    if modules.is_none() {
        return Ok(HashMap::new());
    }
    let modules = modules.unwrap();
    let main_project = resolve_gerrit_project(gix_repo)?;

    let mut info = GitModulesInfo::default();
    for name in modules.names() {
        let path = modules.path(name)?;
        let url = modules.url(name)?;
        info.submodules
            .insert(GitPath::new(path.into_owned()), Ok(url));
    }

    resolve_subprojects(&info, main_project)
}

pub fn resolve_gerrit_project(gix_repo: &gix::Repository) -> Result<String> {
    let url = crate::git::get_default_remote_url(gix_repo)?;
    parse_gerrit_project(&url).with_context(|| format!("Parse gerrit project from {url}"))
}

/// A fully configured toprepo with all state loaded. This unifies access to
/// both raw git operations and toprepo configuration.
pub struct ConfiguredTopRepo {
    pub gix_repo: gix::Repository,
    pub config: GitTopRepoConfig,
    pub ledger: SubRepoLedger,
    pub top_repo_cache: TopRepoCache,
}

impl ConfiguredTopRepo {
    pub fn create(directory: &Path, url: gix::url::Url) -> Result<ConfiguredTopRepo> {
        std::process::Command::new("git")
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
        // TODO: 2025-09-22 Does HEAD always exist on the remote? Is `git ls-remote` needed
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
        Self::open_directory(directory)
    }

    /// Does not load any configuration, just creates an empty state.
    pub fn new_empty(gix_repo: gix::Repository) -> Self {
        Self {
            gix_repo,
            config: GitTopRepoConfig::default(),
            ledger: SubRepoLedger::default(),
            top_repo_cache: TopRepoCache::default(),
        }
    }

    /// Open the git repository in the `directory` or its parents, trying
    /// configured first, falling back to basic This is the main entry point for
    /// command operations.
    pub fn discover(directory: &Path) -> Result<ConfiguredTopRepo> {
        let gix_repo = gix_discover(directory)?;
        ConfiguredTopRepo::load_repo(gix_repo)
    }

    /// Open a toprepo with full configuration and state loaded. The `directory`
    /// must point to the repository root or the `.git` directory.
    pub fn open_directory(directory: &Path) -> Result<Self> {
        let gix_repo = gix::open(directory)?;
        Self::load_repo(gix_repo)
    }

    /// Loads the same repository from scratch using `load_repo`.
    pub fn reload_repo(&mut self) -> Result<()> {
        let config = GitTopRepoConfig::load_config_from_repo(&self.gix_repo)?;
        self.ledger.subrepos = config.subrepos.clone();
        self.config = config;
        Ok(())
    }

    pub fn load_repo(gix_repo: gix::Repository) -> Result<Self> {
        let config = GitTopRepoConfig::load_config_from_repo(&gix_repo)?;
        let ledger = SubRepoLedger {
            subrepos: config.subrepos.clone(),
            missing_subrepos: std::collections::HashSet::new(),
        };
        let top_repo_cache = crate::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
            gix_repo.git_dir(),
            Some(&config.checksum),
        )
        .with_context(|| format!("Loading cache from {}", gix_repo.git_dir().display()))?
        .unpack()?;

        Ok(Self {
            gix_repo,
            config,
            ledger,
            top_repo_cache,
        })
    }

    /// Save state (config + cache) back to disk
    pub fn save_state(&mut self) -> Result<()> {
        // Save cache
        crate::repo_cache_serde::SerdeTopRepoCache::pack(
            &self.top_repo_cache,
            self.config.checksum.clone(),
        )
        .store_to_git_dir(self.gix_repo.git_dir())?;

        // Save effective config with ledger mutations
        const EFFECTIVE_TOPREPO_CONFIG: &str = "toprepo/last-effective-git-toprepo.toml";
        let config_path = self.gix_repo.git_dir().join(EFFECTIVE_TOPREPO_CONFIG);
        let updated_config = GitTopRepoConfig {
            checksum: self.config.checksum.clone(),
            fetch: self.config.fetch.clone(),
            subrepos: self.ledger.subrepos.clone(),
        };
        updated_config.save(&config_path)?;

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

/// The parent is a commit in the original submodule.
///
/// Note that the path to the submodule is available to not used right now.
#[serde_as]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OriginalSubmodParent {
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
    /// The submodule was not expanded. The user has to run `git submodule
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
    // TODO: 2025-09-22 MovedAndBumped(MovedSubmodule),
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
    // TODO: 2025-09-22 Implement this in the
    // Expander::get_recursive_submodule_bumps() or extract the
    // information from Expander::expand_inner_submodules().
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
