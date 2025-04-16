use crate::expander::TopRepoExpander;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git::git_command;
use crate::git::git_global_command;
use crate::git_fast_export_import::ImportCommitRef;
use crate::log::Logger;
use crate::util::CommandExtension as _;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use bstr::ByteSlice as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use gix::remote::Direction;
use itertools::Itertools;
use std::borrow::Borrow as _;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

#[derive(Debug)]
pub struct TopRepo {
    pub directory: PathBuf,
    pub gix_repo: gix::ThreadSafeRepository,
    pub url: gix::url::Url,
}

impl TopRepo {
    pub fn create(directory: PathBuf, url: gix::url::Url) -> Result<TopRepo> {
        git_global_command()
            .arg("init")
            .arg("--quiet")
            .arg(directory.as_os_str())
            .safe_status()?
            .check_success()
            .context("Failed to initialize git repository")?;
        git_command(&directory)
            .args([
                "config",
                "remote.origin.pushUrl",
                "https://ERROR.invalid/Please use 'git toprepo push ...' instead",
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.pushUrl")?;
        git_command(&directory)
            .args(["config", "remote.origin.url", &url.to_string()])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.url")?;
        let toprepo_ref_prefix: String = RepoName::Top.to_ref_prefix();
        git_command(&directory)
            .args([
                "config",
                "--replace-all",
                "remote.origin.fetch",
                &format!("+refs/heads/*:{toprepo_ref_prefix}refs/heads/*"),
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (heads)")?;
        git_command(&directory)
            .args([
                "config",
                "--add",
                "remote.origin.fetch",
                &format!("+refs/tags/*:{toprepo_ref_prefix}refs/tags/*"),
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (tags)")?;
        git_command(&directory)
            .args([
                "config",
                "--add",
                "remote.origin.fetch",
                &format!("+HEAD:{toprepo_ref_prefix}HEAD"),
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (HEAD)")?;
        git_command(&directory)
            .args(["config", "remote.origin.tagOpt", "--no-tags"])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.tagOpt")?;
        git_command(&directory)
            .args(["symbolic-ref", "HEAD", "refs/remotes/origin/HEAD"])
            .safe_status()?
            .check_success()
            .context("Failed to reset HEAD")?;
        Self::open(directory)
    }

    pub fn open(directory: PathBuf) -> Result<TopRepo> {
        let gix_repo = gix::open(&directory)?;
        let url = gix_repo
            .find_default_remote(Direction::Fetch)
            .context("Missing default git-remote")?
            .context("Error getting default git-remote")?
            .url(Direction::Fetch)
            .context("Missing default git-remote fetch url")?
            .to_owned();

        Ok(TopRepo {
            directory,
            gix_repo: gix_repo.into_sync(),
            url,
        })
    }

    pub fn fetch_toprepo(&self) -> Result<()> {
        git_command(&self.directory)
            .arg("fetch")
            .arg("--recurse-submodules=false")
            .safe_status()?
            .check_success()?;
        Ok(())
    }

    pub fn fetch_toprepo_quiet(&self) -> Result<()> {
        git_command(&self.directory)
            .arg("fetch")
            .arg("--recurse-submodules=false")
            .arg("--quiet")
            .safe_status()?
            .check_success()?;
        Ok(())
    }

    pub fn refilter(
        &self,
        mut storage: TopRepoCache,
        config: &crate::config::GitTopRepoConfig,
        logger: Logger,
        progress: indicatif::MultiProgress,
    ) -> Result<()> {
        let repo = self.gix_repo.to_thread_local();

        let old_origin_refs = repo
            .references()?
            .prefixed(b"refs/remotes/origin/".as_bstr())?
            .map_ok(|r| {
                let r = r.detach();
                (r.name.clone(), r)
            })
            .collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(|err| {
                anyhow::anyhow!("Failed while iterating refs/remotes/origin/: {err:#}")
            })?;

        let ref_prefix = RepoName::Top.to_ref_prefix();
        let mut new_origin_ref_names = HashSet::new();
        let mut toprepo_symbolic_tips = Vec::new();
        let mut toprepo_object_tip_names = Vec::new();
        let mut toprepo_object_tip_ids = Vec::new();
        for r in repo
            .references()?
            .prefixed(BStr::new(ref_prefix.as_bytes()))?
        {
            let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            let r_target = r.clone().follow_to_object().with_context(|| {
                format!("Failed to resolve symbolic ref {}", r.name().as_bstr())
            })?;
            match r_target.object()?.kind {
                gix::object::Kind::Commit => {}
                gix::object::Kind::Tag => {}
                gix::object::Kind::Tree => {
                    logger.warning(format!(
                        "Skipping ref {} that points to a tree",
                        r.name().as_bstr()
                    ));
                    continue;
                }
                gix::object::Kind::Blob => {
                    logger.warning(format!(
                        "Skipping ref {} that points to a blob",
                        r.name().as_bstr()
                    ));
                    continue;
                }
            }
            let r = r.detach();
            // TODO: FRME Remove this debug print.
            eprintln!(
                "DEBUG: Initial top commits: {:?} {:?}",
                r.name, // c.name_without_namespace(&top_namespace).with_context(|| format!("ref {}", c.name)).unwrap().as_bstr(),
                r.target.kind(),
            );
            new_origin_ref_names.insert(TopRepoExpander::input_ref_to_output_ref(r.name.borrow())?);
            match r.target {
                gix::refs::Target::Symbolic(target_name) => {
                    toprepo_symbolic_tips.push((r.name, target_name));
                }
                gix::refs::Target::Object(object_id) => {
                    toprepo_object_tip_names.push(r.name);
                    toprepo_object_tip_ids.push(TopRepoCommitId(object_id));
                }
            }
        }
        let mut unknown_toprepo_tips = toprepo_object_tip_ids
            .into_iter()
            .filter(|commit_id| !storage.expanded_commits.contains_key(commit_id))
            .peekable();
        if unknown_toprepo_tips.peek().is_some() {
            let progress = progress.clone();
            let pb = progress.add(
                indicatif::ProgressBar::no_length()
                    .with_style(
                        indicatif::ProgressStyle::default_spinner()
                            .template("{elapsed:>4} {msg} {pos}")
                            .unwrap(),
                    )
                    .with_message("Looking for new commits to expand"),
            );
            let (stop_commits, num_commits_to_export) = crate::git::get_first_known_commits(
                &repo,
                unknown_toprepo_tips.map(|commit_id| commit_id.into_inner()),
                |commit_id| {
                    storage
                        .expanded_commits
                        .contains_key(&TopRepoCommitId(commit_id))
                },
                &pb,
            )?;
            drop(pb);

            println!("Found {} commits to expand", num_commits_to_export);
            // TODO: FRME Remove this debug print.
            for c in &stop_commits {
                eprintln!("DEBUG: Stop commit: {}", c.to_hex());
            }

            let fast_importer = crate::git_fast_export_import::FastImportRepo::new(
                self.gix_repo.git_dir(),
                logger.clone(),
            )?;
            let mut expander = TopRepoExpander {
                gix_repo: &repo,
                storage: &mut storage,
                config,
                progress,
                logger: logger.clone(),
                fast_importer,
                bumps: crate::expander::BumpCache::default(),
            };

            // let commits_to_expand = expander.get_toprepo_commits_to_expand(toprepo_tips)?;
            expander.expand_toprepo_commits(
                toprepo_object_tip_names,
                stop_commits,
                num_commits_to_export,
            )?;
            let _commit = expander.fast_importer.wait()?;

            Self::update_refs(
                &repo,
                &logger,
                toprepo_symbolic_tips,
                old_origin_refs,
                new_origin_ref_names,
            )?;
        }
        Ok(())
    }

    fn update_refs(
        repo: &gix::Repository,
        logger: &Logger,
        toprepo_symbolic_tips: Vec<(FullName, FullName)>,
        old_origin_refs: HashMap<FullName, gix::refs::Reference>,
        new_origin_ref_names: HashSet<FullName>,
    ) -> Result<()> {
        let mut ref_edits = Vec::new();
        // Update symbolic refs/remotes/origin/* if needed.
        for (top_link_name, top_target_name) in &toprepo_symbolic_tips {
            let origin_link_name =
                TopRepoExpander::input_ref_to_output_ref(top_link_name.borrow())?;
            let Ok(origin_target_name) =
                TopRepoExpander::input_ref_to_output_ref(top_target_name.borrow())
            else {
                logger.warning(format!(
                    "Skipping symbolic ref {} that points outside the top repo, to {}.",
                    top_link_name.as_bstr(),
                    top_target_name.as_bstr(),
                ));
                continue;
            };
            let new_target = gix::refs::Target::Symbolic(origin_target_name);
            let old_target = old_origin_refs.get(&origin_link_name).map(|r| &r.target);
            if old_target != Some(&new_target) {
                ref_edits.push(gix::refs::transaction::RefEdit {
                    change: gix::refs::transaction::Change::Update {
                        log: gix::refs::transaction::LogChange {
                            mode: gix::refs::transaction::RefLog::AndReference,
                            force_create_reflog: false,
                            message: b"git-toprepo filter".into(),
                        },
                        expected: old_target.cloned().map_or(
                            gix::refs::transaction::PreviousValue::MustNotExist,
                            gix::refs::transaction::PreviousValue::MustExistAndMatch,
                        ),
                        new: new_target,
                    },
                    name: origin_link_name,
                    deref: false,
                });
            }
        }
        // Remove refs/remote/origin/* references that were removed in refs/namespaces/top/*.
        for old_ref in old_origin_refs.into_values() {
            if new_origin_ref_names.contains(&old_ref.name) {
                continue;
            }
            logger.warning(format!(
                "Deleting now removed ref {}",
                old_ref.name.as_bstr()
            ));
            ref_edits.push(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Delete {
                    expected: gix::refs::transaction::PreviousValue::MustExistAndMatch(
                        old_ref.target,
                    ),
                    log: gix::refs::transaction::RefLog::AndReference,
                },
                name: old_ref.name,
                deref: false,
            });
        }
        // Apply the ref changes.
        if !ref_edits.is_empty() {
            repo.edit_references(ref_edits)
                .context("Failed to update all the refs/remotes/origin/* references")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RepoName {
    Top,
    SubRepo(SubRepoName),
}

impl RepoName {
    /// Converts `refs/namespaces/<name>/*` to `RepoName`.
    pub fn from_ref(fullname: &FullNameRef) -> Result<RepoName> {
        let fullname = fullname.as_bstr();
        let rest = fullname
            .strip_prefix(b"refs/namespaces/")
            .with_context(|| format!("Not a toprepo ref {}", fullname))?;
        let idx = rest
            .find_char('/')
            .with_context(|| format!("Too short toprepo ref {}", fullname))?;
        let name = rest[..idx]
            .to_str()
            .with_context(|| format!("Invalid encoding in ref {}", fullname))?;
        match name {
            "top" => Ok(RepoName::Top),
            _ => Ok(RepoName::SubRepo(SubRepoName::new(name.to_owned()))),
        }
    }

    pub fn to_ref_prefix(&self) -> String {
        // TODO: Start using gix::refs::Namespace.
        format!("refs/namespaces/{self}/")
    }

    fn name(&self) -> &str {
        match self {
            RepoName::Top => "top",
            RepoName::SubRepo(name) => name.deref(),
        }
    }
}

impl std::fmt::Display for RepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name().fmt(f)
    }
}

impl From<SubRepoName> for RepoName {
    fn from(name: SubRepoName) -> Self {
        RepoName::SubRepo(name)
    }
}

impl FromStr for RepoName {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s == "top" {
            Ok(RepoName::Top)
        } else {
            Ok(RepoName::SubRepo(SubRepoName::new(s.to_owned())))
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct SubRepoName(String);

impl SubRepoName {
    pub fn new(name: String) -> Self {
        SubRepoName(name.to_owned())
    }
}

impl Deref for SubRepoName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for SubRepoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopRepoCommitId(CommitId);

impl TopRepoCommitId {
    pub fn new(commit_id: CommitId) -> Self {
        TopRepoCommitId(commit_id)
    }

    fn into_inner(self) -> CommitId {
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
    // The same commit can have been expanded under multiple branches in the top repo.
    // pub expanded_commits: HashMap<CommitId, Vec<TopRepoCommitId>>,
    pub monorepo_commits: HashMap<MonoRepoCommitId, Rc<MonoRepoCommit>>,
    // TODO: #[serde(skip)]
    pub expanded_commits: HashMap<TopRepoCommitId, Rc<MonoRepoCommit>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OriginalSubmodParent {
    // TODO: Unused?
    pub path: GitPath,
    pub commit_id: CommitId,
}

#[derive(Clone)]
pub enum MonoRepoParent {
    OriginalSubmod(OriginalSubmodParent),
    Mono(Rc<MonoRepoCommit>),
}

/// While importing, the commit id might not yet be known. As soon as it is
/// known, the `FastImportMark` is not needed.
#[derive(Clone, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub enum MonoRepoCommitId {
    CommitId(CommitId),
    /// Marker given to git-fast-import.
    FastImportMark(usize),
}

impl MonoRepoCommitId {
    pub fn into_fast_import_id(self) -> ImportCommitRef {
        match self {
            MonoRepoCommitId::CommitId(commit_id) => ImportCommitRef::CommitId(commit_id),
            MonoRepoCommitId::FastImportMark(mark) => ImportCommitRef::Mark(mark),
        }
    }
}

impl Display for MonoRepoCommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MonoRepoCommitId::CommitId(commit_id) => commit_id.fmt(f),
            MonoRepoCommitId::FastImportMark(mark) => write!(f, ":{}", mark),
        }
    }
}

pub struct MonoRepoCommit {
    pub commit_id: MonoRepoCommitId,
    pub parents: Vec<MonoRepoParent>,
    /// The depth in the mono repo, i.e. the number of commits in the longest
    /// history path.
    depth: usize,
    /// The original commits that were updated in this mono repo commit, recursively.
    pub submodule_updates: BTreeMap<GitPath, ExpandedOrRemovedSubmodule>,
    /// The expanded submodule paths in this mono repo commit, recursively.
    pub submodule_paths: Rc<HashSet<GitPath>>,
}

impl MonoRepoCommit {
    pub fn new(
        commit_id: MonoRepoCommitId,
        parents: Vec<MonoRepoParent>,
        submodule_updates: BTreeMap<GitPath, ExpandedOrRemovedSubmodule>,
        submodule_paths: Rc<HashSet<GitPath>>,
    ) -> MonoRepoCommit {
        let depth = parents
            .iter()
            .filter_map(|p| match p {
                MonoRepoParent::Mono(parent) => Some(parent.depth + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        MonoRepoCommit {
            commit_id,
            parents,
            depth,
            submodule_updates,
            submodule_paths,
        }
    }

    pub fn depth(&self) -> usize {
        self.depth
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpandedSubmodule {
    /// Known submodule and known commit.
    Expanded(SubmoduleContent),
    /// The submodule was not expanded. The used has to run `git submodule
    /// update --init` to get its content.
    KeptAsSubmodule(CommitId),
    /// The commit does not exist (any more) in the referred sub repository.
    CommitMissingInSubRepo(SubmoduleContent),
    /// It is unknown which sub repo it should be loaded from.
    UnknownSubmodule(CommitId),
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

#[derive(Clone, Debug, PartialEq)]
pub enum ExpandedOrRemovedSubmodule {
    Expanded(Rc<ExpandedSubmodule>),
    Removed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubmoduleContent {
    /// `None` if the submodule is unknown.
    pub repo_name: SubRepoName,
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

#[derive(Debug)]
pub struct RepoData {
    pub url: gix::Url,
    pub thin_commits: HashMap<CommitId, Rc<ThinCommit>>,
}

impl Default for RepoData {
    fn default() -> Self {
        let mut empty_url: gix::Url = Default::default();
        empty_url.scheme = gix::url::Scheme::File;
        empty_url = empty_url.serialize_alternate_form(true);
        assert_eq!(empty_url.to_bstring(), b"");

        RepoData {
            url: empty_url,
            thin_commits: Default::default(),
            // fat_commits: Default::default(),
            // expanded_commits: Default::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThinSubmodule {
    AddedOrModified(ThinSubmoduleContent),
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThinSubmoduleContent {
    /// `None` is the submodule could not be resolved from the .gitmodules file.
    pub repo_name: Option<SubRepoName>,
    pub commit_id: CommitId,
}

#[derive(Debug)]
pub struct ThinCommit {
    pub commit_id: CommitId,
    pub tree_id: TreeId,
    /// Number of parents in the longest path to the root commit. This number is
    /// strictly decreasing when following the parents.
    pub depth: u32,
    pub parents: Vec<Rc<ThinCommit>>,
    pub dot_gitmodules: Option<BlobId>,
    /// Submodule updates in this commit compared to first parent. Added
    /// submodules are included.
    pub submodule_bumps: BTreeMap<GitPath, ThinSubmodule>,
    /// Paths to all the submodules in the commit, not just the updated ones.
    pub submodule_paths: Rc<HashSet<GitPath>>,
}

impl ThinCommit {
    pub fn is_descendant_of(&self, ancestor: &ThinCommit) -> bool {
        // Doesn't matter which order we iterate.
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

    /// Walks the first parent commit graph to find all submodule entries.
    ///
    /// TODO: Cache results to avoid looping down to the root?
    pub fn get_all_submodules(&self) -> BTreeMap<&GitPath, &ThinSubmodule> {
        let mut ret: BTreeMap<&GitPath, &ThinSubmodule> = self.submodule_bumps.iter().collect();
        let mut node = self;
        while !node.parents.is_empty() {
            node = &node.parents[0];
            for (path, submod) in &node.submodule_bumps {
                ret.entry(path).or_insert(submod);
            }
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_fetch() -> Result<()> {
        use tempfile::tempdir;

        let from_dir = tempdir().unwrap();
        let from_path = from_dir.path();

        let to_dir = tempdir().unwrap();
        let to_path = to_dir.path();
        let env = HashMap::from([
            ("GIT_AUTHOR_NAME", "A Name"),
            ("GIT_AUTHOR_EMAIL", "a@no.domain"),
            ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
            ("GIT_COMMITTER_NAME", "C Name"),
            ("GIT_COMMITTER_EMAIL", "c@no.domain"),
            ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
        ]);

        git_command(from_path)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .safe_status()?
            .check_success()?;
        git_command(from_path)
            .args(["commit", "--allow-empty", "--quiet"])
            .args(["-m", "Initial commit"])
            .envs(&env)
            .safe_status()?
            .check_success()?;
        git_command(from_path)
            .args(["tag", "mytag"])
            .envs(&env)
            .safe_status()?
            .check_success()?;

        let toprepo = TopRepo::create(
            to_path.to_path_buf(),
            gix::url::Url::try_from(from_path).unwrap(),
        )
        .unwrap();

        toprepo.fetch_toprepo_quiet().unwrap();

        let ref_pairs = vec![
            ("HEAD", "refs/namespaces/top/HEAD"),
            ("main", "refs/namespaces/top/refs/heads/main"),
            ("mytag", "refs/namespaces/top/refs/tags/mytag"),
        ];
        for (orig_ref, top_ref) in ref_pairs {
            let orig_rev = git_command(from_path)
                .args(["rev-parse", "--verify", orig_ref])
                .output_stdout_only()?
                .check_success_with_stderr()
                .with_context(|| format!("orig {}", orig_ref))?
                .stdout
                .to_owned();
            let top_rev = git_command(&toprepo.directory)
                .args(["rev-parse", "--verify", top_ref])
                .output_stdout_only()?
                .check_success_with_stderr()
                .with_context(|| format!("top {}", top_ref))?
                .stdout
                .to_owned();
            assert_eq!(
                orig_rev.to_str().unwrap(),
                top_rev.to_str().unwrap(),
                "ref {orig_ref} mismatch",
            );
        }
        Ok(())
    }
}
