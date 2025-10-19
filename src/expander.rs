use crate::commit_message::calculate_mono_commit_message_from_commits;
use crate::git::CommitId;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git_fast_export_import::ChangedFile;
use crate::git_fast_export_import::FastExportCommit;
use crate::git_fast_export_import::FastImportCommit;
use crate::git_fast_export_import::ImportCommitRef;
use crate::loader::SubRepoLedger;
use crate::repo::ConfiguredTopRepo;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use crate::repo::ImportCache;
use crate::repo::MonoRepoCommit;
use crate::repo::MonoRepoCommitId;
use crate::repo::MonoRepoParent;
use crate::repo::OriginalSubmodParent;
use crate::repo::RepoData;
use crate::repo::SubmoduleReference;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::ThinSubmoduleReference;
use crate::repo::TopRepoCommitId;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::RcKey;
use crate::util::UniqueContainer;
use anyhow::Context as _;
use anyhow::Result;
use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice as _;
use bstr::ByteVec;
use gix::prelude::ObjectIdExt as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use gix::refs::file::ReferenceExt as _;
use itertools::Itertools as _;
use lru::LruCache;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::io::Write;
use std::ops::Deref;
use std::rc::Rc;
use tracing::instrument;

pub struct Expander<'a> {
    pub(crate) gix_repo: &'a gix::Repository,
    pub(crate) ledger: &'a mut SubRepoLedger,
    pub(crate) import_cache: &'a mut ImportCache,
    pub(crate) progress: indicatif::MultiProgress,
    pub(crate) fast_importer: crate::git_fast_export_import::FastImportRepo,
    pub(crate) imported_commits:
        HashMap<RcKey<MonoRepoCommit>, (usize, Rc<MonoRepoCommit>, Option<TopRepoCommitId>)>,
    /// Commits that have been expanded, but not yet resolved the mono commit id
    /// and stored in `self.import_cache`.
    pub(crate) expanded_top_commits: HashMap<TopRepoCommitId, Rc<MonoRepoCommit>>,
    pub(crate) bumps: BumpCache,
    pub(crate) inject_at_oldest_super_commit: bool,
}

impl Expander<'_> {
    const TOPREPO_IMPORT_REF: &'static str = "refs/toprepo/import";

    /// Creates a list of not yet expanded top repo commits needed to expand the
    /// given tips. The returned list is sorted in the order to be expanded.
    #[instrument(
        name = "get_commits_to_expand",
        skip_all,
        fields(
            top_tip_count = toprepo_tips.len(),
        )
    )]
    pub fn get_toprepo_commits_to_expand(
        &self,
        toprepo_tips: Vec<gix::ObjectId>,
    ) -> Result<Vec<TopRepoCommitId>> {
        let pb: indicatif::ProgressBar = self.progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{elapsed:>4} {msg} {pos}")
                        .unwrap(),
                )
                .with_message("Looking for new commits to expand"),
        );

        let walk = self.gix_repo.rev_walk(toprepo_tips);
        let mut commits_to_expand: Vec<_> = Vec::new();
        walk.selected(|commit_id| {
            let commit_id = TopRepoCommitId::new(commit_id.to_owned());
            if self
                .import_cache
                .top_to_mono_commit_map
                .contains_key(&commit_id)
            {
                // Stop iterating.
                false
            } else {
                commits_to_expand.push(commit_id);
                pb.set_position(commits_to_expand.len() as u64);
                // Continue the search.
                true
            }
        })?
        // Discard the output, check for errors.
        .filter_map_ok(|_| None)
        .collect::<std::result::Result<Vec<()>, _>>()
        .context("Looking for new commits to expand")?;
        commits_to_expand.reverse();
        Ok(commits_to_expand)
    }

    /// Expands the listed `start_commit_ids` and their ancestors. Note that
    /// `start_commit_ids` must be commits as git-fast-export does not support
    /// annotated tags pointing to filtered out commits. Tags are instead
    /// handled manually with Gitoxide after commit expansion.
    #[instrument(
        name = "expand_commits",
        skip_all,
        fields(
            start_commit_count = start_commit_ids.len(),
            stop_commit_count = stop_commit_ids.len(),
            count = c,
        )
    )]
    pub fn expand_toprepo_commits(
        &mut self,
        start_commit_ids: &[TopRepoCommitId],
        stop_commit_ids: &[TopRepoCommitId],
        c: usize,
    ) -> Result<()> {
        log::info!("Expanding the toprepo to a monorepo...");
        self.progress
            .set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));
        let pb = self.progress.add(
            indicatif::ProgressBar::new(c as u64)
                .with_style(
                    indicatif::ProgressStyle::with_template(
                        "{elapsed:>4} {prefix:.cyan} [{bar:24}] {pos}/{len}",
                    )
                    .unwrap()
                    .progress_chars("=> "),
                )
                .with_prefix("Expanding commits"),
        );

        let fast_exporter = crate::git_fast_export_import::FastExportRepo::load_from_path(
            self.gix_repo.git_dir(),
            Some(
                start_commit_ids
                    .iter()
                    .map(|id| id.deref().to_hex().to_string())
                    .chain(
                        stop_commit_ids
                            .iter()
                            .map(|id| format!("^{}", id.deref().to_hex())),
                    ),
            ),
        )?;

        for entry in fast_exporter {
            let entry = entry?; // TODO: 2025-09-22 error handling
            match entry {
                crate::git_fast_export_import::FastExportEntry::Commit(commit) => {
                    let commit_id = TopRepoCommitId::new(commit.original_id);
                    let now = std::time::Instant::now();
                    self.expand_toprepo_commit(commit)?;
                    let ms = now.elapsed().as_millis();
                    if ms > 100 {
                        log::debug!("Commit {commit_id} took {ms} ms");
                    }
                    pb.inc(1);
                }
                crate::git_fast_export_import::FastExportEntry::Reset(_reset) => {
                    // We only write to one branch, so no point of translating 'reset' messages.
                }
            }
        }
        log::info!("Finished expanding commits in {:.2?}", pb.elapsed());
        Ok(())
    }

    /// Not just the cache contains the top-to-mono mapping, also
    /// `self.expanded_top_commits`. This method checks both.
    fn get_mono_commit_from_toprepo_commit_id(
        &self,
        top_commit_id: &TopRepoCommitId,
    ) -> Option<&Rc<MonoRepoCommit>> {
        if let Some((_mono_commit_id, mono_commit)) =
            self.import_cache.top_to_mono_commit_map.get(top_commit_id)
        {
            return Some(mono_commit);
        }
        self.expanded_top_commits.get(top_commit_id)
    }

    #[instrument(name = "final_wait", skip_all)]
    pub fn wait(self) -> Result<()> {
        // Record the new mono commit ids.
        let commit_ids = self.fast_importer.wait()?;
        for (mark, mono_commit, top_commit_id) in self.imported_commits.into_values() {
            let mono_commit_id = MonoRepoCommitId::new(commit_ids[mark - 1]);
            self.import_cache
                .monorepo_commits
                .insert(mono_commit_id, mono_commit.clone());
            self.import_cache
                .monorepo_commit_ids
                .insert(RcKey::new(&mono_commit), mono_commit_id);
            if let Some(top_commit_id) = top_commit_id {
                self.import_cache
                    .top_to_mono_commit_map
                    .insert(top_commit_id, (mono_commit_id, mono_commit));
            }
        }
        Ok(())
    }

    /// Gets a `:mark` marker reference or the full commit id for a commit.
    fn get_import_commit_ref(&self, mono_commit: &Rc<MonoRepoCommit>) -> ImportCommitRef {
        let key = RcKey::new(mono_commit);
        if let Some((mark, _, _)) = self.imported_commits.get(&key) {
            ImportCommitRef::Mark(*mark)
        } else {
            let commit_id = self
                .import_cache
                .monorepo_commit_ids
                .get(&key)
                .expect("existing mono commits have commit id");
            ImportCommitRef::CommitId(*commit_id.deref())
        }
    }

    fn expand_toprepo_commit(&mut self, commit: FastExportCommit) -> Result<()> {
        let commit_id = TopRepoCommitId::new(commit.original_id);
        let top_storage = self.import_cache.repos.get(&RepoName::Top).unwrap();
        let top_commit = top_storage
            .thin_commits
            .get(commit_id.deref())
            .unwrap()
            .clone();
        let mut mono_parents_of_top = top_commit
            .parents
            .iter()
            .map(|parent| {
                self.get_mono_commit_from_toprepo_commit_id(&TopRepoCommitId::new(parent.commit_id))
                    .expect("all parents must already have been expanded")
                    .clone()
            })
            .collect_vec();
        const TOP_PATH: GitPath = GitPath::new(BString::new(vec![]));
        let parents_for_submodules =
            self.expand_inner_submodules(&mono_parents_of_top, &TOP_PATH, &top_commit)?;
        if mono_parents_of_top.is_empty() && !parents_for_submodules.is_empty() {
            // There should be a first parent that is not a submodule.
            // Add an initial empty commit.
            mono_parents_of_top.push(self.emit_mono_commit_with_tree_updates(
                &GitPath::new("".into()),
                &top_commit,
                vec![],
                vec![],
                None,
                HashMap::new(),
                Some(BString::from(b"Initial empty commit\n")),
            )?);
        }
        let mono_parents = mono_parents_of_top
            .into_iter()
            .map(MonoRepoParent::Mono)
            .chain(parents_for_submodules)
            .collect_vec();
        let mono_commit = self.emit_mono_commit(
            &TOP_PATH,
            &RepoName::Top,
            &top_commit,
            mono_parents,
            commit.file_changes,
            None,
        )?;
        // Overwrite, but include the top commit id.
        self.imported_commits
            .get_mut(&RcKey::new(&mono_commit))
            .expect("just returned from emit_mono_commit")
            .2 = Some(commit_id);
        self.expanded_top_commits.insert(commit_id, mono_commit);
        Ok(())
    }

    /// If a super repository has been bumped, all the inner trees need to be
    /// rewritten to override the submodule entries. In that case,
    /// `force_add_tree_ids` should be set.
    ///
    /// Example:
    /// ```text
    /// top-1 -> sub-1 -> inner-sub-1
    /// top-2 -> sub-2 -> inner-sub-1
    /// ```
    ///
    /// In commit `top-2`, the path `sub` is replaced with `sub-2^{tree}` which
    /// contains a submodule at `sub/inner`, so `sub/inner` also need to be
    /// replaced with `inner-sub-1^{tree}`.
    fn get_recursive_submodule_bumps(
        &self,
        path: &GitPath,
        commit: &ThinCommit,
        submod_updates: Option<&mut HashMap<GitPath, ExpandedOrRemovedSubmodule>>,
        force_add_tree_ids: bool,
        tree_updates: &mut Vec<(GitPath, TreeId)>,
    ) {
        if force_add_tree_ids {
            for rel_sub_path in commit.submodule_paths.iter() {
                if submod_updates.is_some() && commit.submodule_bumps.contains_key(rel_sub_path) {
                    // Will be done later anyway.
                    continue;
                }
                let abs_sub_path = path.join(rel_sub_path);
                let submod = commit
                    .get_submodule(rel_sub_path)
                    .expect("submodule exists as path exists");
                match submod {
                    ThinSubmodule::AddedOrModified(bump) => {
                        self.get_recursive_expanded_submodule_bump(
                            &abs_sub_path,
                            bump,
                            None,
                            force_add_tree_ids,
                            tree_updates,
                        );
                    }
                    ThinSubmodule::Removed => unreachable!("path to removed submodule exists"),
                };
            }
        }
        if let Some(submod_updates) = submod_updates {
            for (rel_sub_path, bump) in commit.submodule_bumps.iter() {
                let abs_sub_path = path.join(rel_sub_path);
                let submod_update = match bump {
                    ThinSubmodule::AddedOrModified(bump) => ExpandedOrRemovedSubmodule::Expanded(
                        self.get_recursive_expanded_submodule_bump(
                            &abs_sub_path,
                            bump,
                            Some(submod_updates),
                            /* force_add_tree_ids */ true,
                            tree_updates,
                        ),
                    ),
                    ThinSubmodule::Removed => ExpandedOrRemovedSubmodule::Removed,
                };
                submod_updates.insert(abs_sub_path.clone(), submod_update);
            }
        }
    }

    fn get_recursive_expanded_submodule_bump(
        &self,
        abs_sub_path: &GitPath,
        bump: &ThinSubmoduleReference,
        submod_updates: Option<&mut HashMap<GitPath, ExpandedOrRemovedSubmodule>>,
        force_add_tree_ids: bool,
        tree_updates: &mut Vec<(GitPath, TreeId)>,
    ) -> ExpandedSubmodule {
        let Some(submod_repo_name) = &bump.repo_name else {
            return ExpandedSubmodule::UnknownSubmodule(bump.commit_id);
        };
        let repo_name = RepoName::SubRepo(submod_repo_name.clone());
        if !self.ledger.is_enabled(&repo_name) {
            // Repository disabled by config, keep the submodule.
            return ExpandedSubmodule::KeptAsSubmodule(bump.commit_id);
        }
        if self
            .ledger
            .is_missing_commit(submod_repo_name, &bump.commit_id)
        {
            return ExpandedSubmodule::KeptAsSubmodule(bump.commit_id);
        }
        let submod_storage = self.import_cache.repos.get(&repo_name).unwrap();
        let Some(submod_commit) = submod_storage.thin_commits.get(&bump.commit_id) else {
            return ExpandedSubmodule::CommitMissingInSubRepo(SubmoduleReference {
                repo_name: submod_repo_name.clone(),
                orig_commit_id: bump.commit_id,
            });
        };
        tree_updates.push((abs_sub_path.clone(), submod_commit.tree_id));
        self.get_recursive_submodule_bumps(
            abs_sub_path,
            submod_commit,
            submod_updates,
            force_add_tree_ids,
            tree_updates,
        );
        // TODO: 2025-09-22 This might be a regression, but the caller is not
        // interested in that information anyway.
        ExpandedSubmodule::Expanded(SubmoduleReference {
            repo_name: submod_repo_name.clone(),
            orig_commit_id: bump.commit_id,
        })
    }

    fn emit_mono_commit(
        &mut self,
        path: &GitPath,
        repo_name: &RepoName,
        source_commit: &ThinCommit,
        parents: Vec<MonoRepoParent>,
        initial_file_changes: Vec<ChangedFile>,
        message: Option<BString>,
    ) -> Result<Rc<MonoRepoCommit>> {
        let mut submodule_bumps = HashMap::new();
        let mut tree_updates = Vec::new();
        let top_bump = match repo_name {
            RepoName::Top => Some(TopRepoCommitId::new(source_commit.commit_id)),
            RepoName::SubRepo(sub_repo_name) => {
                submodule_bumps.insert(
                    path.clone(),
                    ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::Expanded(
                        SubmoduleReference {
                            repo_name: sub_repo_name.clone(),
                            orig_commit_id: source_commit.commit_id,
                        },
                    )),
                );
                tree_updates.push((path.clone(), source_commit.tree_id));
                None
            }
        };
        self.get_recursive_submodule_bumps(
            path,
            source_commit,
            Some(&mut submodule_bumps),
            /* force_add_tree_ids */ false,
            &mut tree_updates,
        );

        // tree_updates need to be ordered to get the inner submodules replaced
        // inside the outer submodules.
        tree_updates.sort();
        let tree_file_changes = tree_updates
            .into_iter()
            .map(|(tree_path, tree_id)| {
                const TREE_MODE: &[u8] = b"040000";
                let mut tree_id_hex = gix::hash::Kind::hex_buf();
                let _len = tree_id.hex_to_buf(&mut tree_id_hex);
                ChangedFile {
                    path: tree_path.deref().clone(),
                    change: crate::git_fast_export_import::FileChange::Modified {
                        mode: TREE_MODE.into(),
                        hash: tree_id_hex.into(),
                    },
                }
            })
            .collect::<Vec<_>>();
        let mut file_changes = initial_file_changes;
        file_changes.extend(tree_file_changes);
        let mono_commit = self.emit_mono_commit_with_tree_updates(
            path,
            source_commit,
            parents,
            file_changes,
            top_bump,
            submodule_bumps,
            message,
        )?;
        Ok(mono_commit)
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_mono_commit_with_tree_updates(
        &mut self,
        source_path: &GitPath,
        source_commit: &ThinCommit,
        parents: Vec<MonoRepoParent>,
        file_changes: Vec<ChangedFile>,
        top_bump: Option<TopRepoCommitId>,
        submodule_bumps: HashMap<GitPath, ExpandedOrRemovedSubmodule>,
        message: Option<BString>,
    ) -> Result<Rc<MonoRepoCommit>> {
        let source_gix_commit = self.gix_repo.find_commit(source_commit.commit_id)?;
        let source_gix_commit = source_gix_commit.decode()?;
        let mut author = Vec::new();
        source_gix_commit.author.write_to(&mut author)?;
        let mut committer = Vec::new();
        source_gix_commit.committer.write_to(&mut committer)?;
        let message = message.unwrap_or_else(|| {
            calculate_mono_commit_message_from_commits(
                self.gix_repo,
                source_path,
                &source_commit.commit_id,
                &source_gix_commit,
                &submodule_bumps,
            )
            .into()
        });
        let importer_mark = self.fast_importer.write_commit(&FastImportCommit {
            // TODO: 2025-09-22 FullNameRef::try_from() doesn't work for some reason.
            branch: FullName::try_from(Self::TOPREPO_IMPORT_REF)
                .unwrap()
                .as_ref(),
            author_info: BString::new(author),
            committer_info: BString::new(committer),
            encoding: source_gix_commit.encoding.map(|enc| enc.to_owned()),
            message,
            file_changes,
            parents: parents
                .iter()
                .map(|p| match p {
                    MonoRepoParent::Mono(mono_parent) => self.get_import_commit_ref(mono_parent),
                    MonoRepoParent::OriginalSubmod(submod_parent) => {
                        ImportCommitRef::CommitId(submod_parent.commit_id)
                    }
                })
                .collect(),
            original_id: None,
        })?;
        let mono_commit = MonoRepoCommit::new_rc(parents, top_bump, submodule_bumps);
        self.imported_commits.insert(
            RcKey::new(&mono_commit),
            (importer_mark, mono_commit.clone(), None),
        );
        Ok(mono_commit)
    }

    fn expand_inner_submodules(
        &mut self,
        mono_parents: &Vec<Rc<MonoRepoCommit>>,
        abs_super_path: &GitPath,
        super_commit: &ThinCommit,
    ) -> Result<Vec<MonoRepoParent>> {
        let mut extra_parents_due_to_submods = Vec::new();
        let mut submodule_bumps = super_commit.submodule_bumps.clone();
        if let Some(first_mono_parent) = mono_parents.first() {
            for abs_sub_path in first_mono_parent.submodule_paths.iter() {
                // Only consider submodules within abs_super_path.
                let rel_sub_path = match abs_sub_path.relative_to(abs_super_path) {
                    Some(rel_sub_path) if !rel_sub_path.is_empty() => rel_sub_path,
                    _ => continue,
                };
                if submodule_bumps.contains_key(&rel_sub_path) {
                    // The submodule is already added or removed, no need to add it again.
                    continue;
                }
                let first_parent_bump = self
                    .bumps
                    .get_submodule(first_mono_parent, abs_sub_path)
                    .expect("submodule path exists")
                    .clone(); // Clone to allow mut-borrowing self.bumps again.
                if let Some(submod) = first_parent_bump.get_known_submod() {
                    for mono_parent in &mono_parents[1..] {
                        if let Some(other_parent_submod) = self.bumps.get_some_submodule(
                            mono_parent,
                            abs_sub_path,
                            &submod.repo_name,
                        ) && other_parent_submod.get_orig_commit_id() != &submod.orig_commit_id
                        {
                            // Even if not bumped compared to the first parent,
                            // a check for unrelated parents must be performed.
                            submodule_bumps.insert(
                                rel_sub_path.clone(),
                                ThinSubmodule::AddedOrModified(ThinSubmoduleReference {
                                    repo_name: Some(submod.repo_name.clone()),
                                    commit_id: submod.orig_commit_id,
                                }),
                            );
                        }
                    }
                }
            }
        }
        let mut regressing_commit = None;
        for (rel_sub_path, submod) in submodule_bumps.iter() {
            let _expanded_bump = match submod {
                ThinSubmodule::AddedOrModified(submod) => {
                    let (expanded_submod, extra_parents) = self.expand_inner_submodule(
                        mono_parents,
                        submod,
                        abs_super_path,
                        rel_sub_path,
                        &mut regressing_commit,
                    )?;
                    extra_parents_due_to_submods.extend(extra_parents);
                    ExpandedOrRemovedSubmodule::Expanded(expanded_submod)
                }
                ThinSubmodule::Removed => ExpandedOrRemovedSubmodule::Removed,
            };
            // expanded_bumps.insert(rel_sub_path.clone(), expanded_bump);
        }
        if let Some(regressing_commit) = regressing_commit.take() {
            // The commit is not a submodule bump, but a commit that is not a descendant of the mono parents.
            // Add the commit as a parent.
            extra_parents_due_to_submods.push(MonoRepoParent::Mono(regressing_commit));
        }
        Ok(extra_parents_due_to_submods)
    }

    fn expand_inner_submodule(
        &mut self,
        mono_parents: &Vec<Rc<MonoRepoCommit>>,
        submod: &ThinSubmoduleReference,
        abs_super_path: &GitPath,
        rel_sub_path: &GitPath,
        regressing_commit: &mut Option<Rc<MonoRepoCommit>>,
    ) -> Result<(ExpandedSubmodule, Vec<MonoRepoParent>)> {
        let Some(submod_repo_name) = &submod.repo_name else {
            // A warning has already been logged when loading the
            // super commit.
            return Ok((
                ExpandedSubmodule::UnknownSubmodule(submod.commit_id),
                vec![],
            ));
        };
        let submod_commit_id = submod.commit_id;
        let submod_reference = SubmoduleReference {
            repo_name: submod_repo_name.clone(),
            orig_commit_id: submod.commit_id,
        };
        // The submodule is known.
        if !self
            .ledger
            .subrepos
            .get(submod_repo_name)
            .is_none_or(|repo_config| repo_config.enabled)
        {
            // Repository disabled by config, skipping to keep the submodule.
            // No need to log a warning because it is part of the user configuration.
            return Ok((ExpandedSubmodule::KeptAsSubmodule(submod_commit_id), vec![]));
        }
        let Some(submod_storage) = self
            .import_cache
            .repos
            .get(&RepoName::SubRepo(submod_repo_name.clone()))
        else {
            // No commits loaded for the submodule.
            return Ok((
                ExpandedSubmodule::CommitMissingInSubRepo(submod_reference),
                vec![],
            ));
        };
        let Some(submod_commit) = submod_storage.thin_commits.get(&submod_commit_id) else {
            return Ok((
                ExpandedSubmodule::CommitMissingInSubRepo(submod_reference),
                vec![],
            ));
        };
        // Drop the borrow of self.
        let submod_commit = submod_commit.clone();

        let abs_sub_path = abs_super_path.join(rel_sub_path);
        // Check for a regressing or unrelated submodule bump.
        let non_descendants = self.bumps.non_descendants_for_all_parents(
            mono_parents,
            &abs_sub_path,
            submod_repo_name,
            &submod_commit,
            submod_storage,
        );
        if !non_descendants.is_empty() {
            // Not descendant.
            let regressing_parent_vec = regressing_commit
                .take()
                .map(|c: Rc<MonoRepoCommit>| vec![c.clone()]);
            let mono_commit = self.expand_parent_for_regressing_submodule_bump(
                regressing_parent_vec.as_ref().unwrap_or(mono_parents),
                &RepoName::SubRepo(submod_repo_name.clone()),
                &abs_sub_path,
                &submod_commit,
                non_descendants,
            )?;
            regressing_commit.replace(mono_commit);
            return Ok((
                ExpandedSubmodule::RegressedNotFullyImplemented(submod_reference),
                vec![],
            ));
        }

        // The normal case: Expand the parents of the submodule.
        let extra_parents = self.expand_parents_of_submodule(
            mono_parents,
            abs_super_path,
            rel_sub_path,
            submod_repo_name,
            &submod_commit,
        )?;
        Ok((ExpandedSubmodule::Expanded(submod_reference), extra_parents))
    }

    fn expand_parents_of_submodule(
        &mut self,
        possible_mono_parents: &Vec<Rc<MonoRepoCommit>>,
        abs_super_path: &GitPath,
        rel_sub_path: &GitPath,
        submod_repo_name: &SubRepoName,
        submod_commit: &ThinCommit,
    ) -> Result<Vec<MonoRepoParent>> {
        let abs_sub_path = abs_super_path.join(rel_sub_path);
        let mut extra_parents = Vec::new();
        if self.uptodate_for_any_parent(
            possible_mono_parents,
            &abs_sub_path,
            submod_repo_name,
            submod_commit.commit_id,
        ) {
            // The submodule was already pointing on the same submod.commit_id in at least one of the parents.
            // No need to add any extra parent relation.
            return Ok(extra_parents);
        }

        // Add links to all parents of submod_commit.
        let mut submodule_exists_in_some_parent = false;
        for submod_parent in &submod_commit.parents {
            if self.uptodate_for_any_parent(
                possible_mono_parents,
                &abs_sub_path,
                submod_repo_name,
                submod_parent.commit_id,
            ) {
                // This submodule parent does not need to be expanded.
                submodule_exists_in_some_parent = true;
                continue;
            }
            // Expand this submod_parent for one of the possible_mono_parents.
            if let Some(extra_parent) = self.inject_submodule_commit(
                possible_mono_parents.clone(),
                &abs_sub_path,
                submod_repo_name,
                submod_parent,
            )? {
                extra_parents.push(MonoRepoParent::Mono(extra_parent));
            }
        }
        if !submodule_exists_in_some_parent && extra_parents.is_empty() {
            // The submodule was not present in any of the parents.
            // Add a single parent relation to the original commit.
            // The git history will show a rename of all the files.
            //
            // NOTE: The alternative is to move all the files in the
            // history into abs_sub_path. Then the user doesn't need
            // to specify `--follow` in git-log. If the submodule is
            // then moved and a commit is missing, or there is
            // another path pointing to the same submodule, there
            // will be another import of the same repository but
            // with files moved into a different subfolder and there
            // will be no relation between those multiple imports.
            // The decision is to keep the original commits to get a
            // relation between submodules existing in multiple
            // places.
            extra_parents.push(MonoRepoParent::OriginalSubmod(OriginalSubmodParent {
                commit_id: submod_commit.commit_id,
            }));
        }
        Ok(extra_parents)
    }

    fn uptodate_for_any_parent(
        &mut self,
        mono_parents: &Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        submod_repo_name: &SubRepoName,
        submod_commit_id: CommitId,
    ) -> bool {
        for mono_parent in mono_parents {
            if let Some(parent_submod) =
                self.bumps
                    .get_some_submodule(mono_parent, abs_sub_path, submod_repo_name)
            {
                if parent_submod.get_orig_commit_id() == &submod_commit_id {
                    return true;
                } else {
                    // The submodule is not well defined in this parent.
                }
            }
        }
        false
    }

    fn expand_parent_for_regressing_submodule_bump(
        &mut self,
        mono_parents: &[Rc<MonoRepoCommit>],
        repo_name: &RepoName,
        abs_sub_path: &GitPath,
        submod_commit: &ThinCommit,
        mut non_descendants: Vec<CommitId>,
    ) -> Result<Rc<MonoRepoCommit>> {
        // Going backwards in history, add he original submod_commit as parent instead.
        // TODO: 2025-09-22 Make it configurable per commit what to do.
        // 1. Use no history = noop
        // 2. Use submodule history.
        // extra_parents_due_to_submods.push(MonoRepoParent::OriginalSubmod(
        //     OriginalSubmodParent {
        //         path: abs_sub_path.clone(),
        //         commit_id: submod_commit.commit_id,
        //     },
        // ));
        // 3. Create a chain of reverts back to submod_commit.
        // 4. Insert one extra branch from first parent to update.
        const TREE_MODE: &[u8] = b"040000";
        let mut tree_id_hex = gix::hash::Kind::hex_buf();
        let _len = submod_commit.tree_id.hex_to_buf(&mut tree_id_hex);
        let file_changes = vec![ChangedFile {
            path: abs_sub_path.deref().clone(),
            change: crate::git_fast_export_import::FileChange::Modified {
                mode: TREE_MODE.into(),
                hash: tree_id_hex.into(),
            },
        }];

        let mut commit_message = BString::new(vec![]);
        let commit_id_str = submod_commit.commit_id.to_string();
        let source_gix_commit = self.gix_repo.find_commit(submod_commit.commit_id)?;
        write!(
            commit_message,
            "Resetting submodule {} to {}\n\n",
            abs_sub_path,
            &commit_id_str[..12]
        )?;
        non_descendants.sort();
        non_descendants.dedup();
        writeln!(
            commit_message,
            "The gitlinks of the parents to this commit references the commit{}:",
            if non_descendants.len() == 1 { "" } else { "s" },
        )?;
        for non_descendant in non_descendants {
            writeln!(commit_message, "- {non_descendant}")?;
        }
        write!(
            commit_message,
            "Regress the gitlink to the earlier commit\n{commit_id_str}:\n\n"
        )?;
        commit_message.push_str(source_gix_commit.message_raw().unwrap_or_default());
        drop(source_gix_commit);
        let regressing_mono_parents = mono_parents
            .iter()
            .filter_map(|p| {
                if let Some(expanded_submod) = self.bumps.get_submodule(p, abs_sub_path) {
                    if expanded_submod.get_orig_commit_id() != &submod_commit.commit_id {
                        Some(MonoRepoParent::Mono(p.clone()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        let extra_mono_commit = self.emit_mono_commit(
            abs_sub_path,
            repo_name,
            submod_commit,
            regressing_mono_parents,
            file_changes,
            Some(commit_message),
        )?;
        // TODO: 2025-09-22 What to record in extra_mono_commit.submodule_bumps[abs_sub_path]?
        Ok(extra_mono_commit)
    }

    pub fn inject_submodule_commit(
        &mut self,
        possible_mono_parents: Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        wanted_sub_repo_name: &SubRepoName,
        wanted_sub_commit: &Rc<ThinCommit>,
    ) -> Result<Option<Rc<MonoRepoCommit>>> {
        let mut sub_to_mono_commit = HashMap::new();
        self.inject_submodule_commit_impl(
            possible_mono_parents,
            abs_sub_path,
            wanted_sub_repo_name,
            wanted_sub_commit,
            &mut sub_to_mono_commit,
        )
    }

    fn inject_submodule_commit_memo(
        &mut self,
        possible_mono_parents: Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        wanted_sub_repo_name: &SubRepoName,
        wanted_sub_commit: &Rc<ThinCommit>,
        sub_to_mono_commit: &mut HashMap<RcKey<ThinCommit>, Option<Rc<MonoRepoCommit>>>,
    ) -> Result<Option<Rc<MonoRepoCommit>>> {
        if let Some(ret) = sub_to_mono_commit.get(&RcKey::new(wanted_sub_commit)) {
            return Ok(ret.clone());
        }
        let ret = self.inject_submodule_commit_impl(
            possible_mono_parents,
            abs_sub_path,
            wanted_sub_repo_name,
            wanted_sub_commit,
            sub_to_mono_commit,
        )?;
        sub_to_mono_commit.insert(RcKey::new(wanted_sub_commit), ret.clone());
        Ok(ret)
    }

    fn inject_submodule_commit_impl(
        &mut self,
        possible_mono_parents: Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        wanted_sub_repo_name: &SubRepoName,
        wanted_sub_commit: &Rc<ThinCommit>,
        sub_to_mono_commit: &mut HashMap<RcKey<ThinCommit>, Option<Rc<MonoRepoCommit>>>,
    ) -> Result<Option<Rc<MonoRepoCommit>>> {
        // Depth first search, therefore use a stack and reverse the initial
        // possible_mono_parents to start taking the first suggested mono
        // parent.
        let mut todo_next = Vec::new();
        let mut todo = possible_mono_parents;
        todo.reverse();
        let mut visited = HashSet::new();
        while let Some(mono_commit) = todo.pop() {
            if !visited.insert(RcKey::new(&mono_commit)) {
                // Already checked.
                continue;
            }
            let Some(submod) =
                self.bumps
                    .get_some_submodule(&mono_commit, abs_sub_path, wanted_sub_repo_name)
            else {
                // The submodule does not exist. Don't traverse the parents.
                continue;
            };
            if let Some(submod) = submod.get_known_submod() {
                // The submodule is known.
                if !self
                    .ledger
                    .subrepos
                    .get(&submod.repo_name)
                    .is_none_or(|repo_config| repo_config.enabled)
                {
                    // Repository disabled by config, skipping to keep the submodule.
                    // No need to log a warning because it is part of the user configuration.
                    continue;
                }
                if let Some(submod_storage) = self
                    .import_cache
                    .repos
                    .get(&RepoName::SubRepo(submod.repo_name.clone()))
                {
                    // Make sure that the submodule commit is known before
                    // checking if it is the correct one. If the submodule
                    // commit is not known inside the given submodule name,
                    // it should not be accepted.
                    if let Some(submod_commit) =
                        submod_storage.thin_commits.get(&submod.orig_commit_id)
                    {
                        if submod.orig_commit_id == wanted_sub_commit.commit_id {
                            // The submodule commit is already in the mono
                            // commit.
                            if self.inject_at_oldest_super_commit {
                                // Instead of using the latest super commit, use
                                // the oldest commit to simplify rebasing and to
                                // get a deterministic result even if more mono
                                // commits are added which do not update the
                                // submodule.
                                return Ok(Some(
                                    self.bumps
                                        .get_bumps(&mono_commit, abs_sub_path)
                                        .first()
                                        .expect("The submodule commit should be in the mono commit")
                                        .clone(),
                                ));
                            }
                            return Ok(Some(mono_commit));
                        }
                        if submod_commit.depth < wanted_sub_commit.depth {
                            // Pause with this branch and dig into the submodule
                            // history.
                            todo_next.push(mono_commit.clone());
                            continue;
                        } else {
                            // Check if the parents are at level.
                        }
                    } else {
                        // The submodule commit is not known. Try the parents
                        // for better luck.
                    }
                } else {
                    // No commits loaded for the submodule.
                }
            } else {
                // Unknown which submodule was referred to. Try the parents for better luck.
            }

            // Depth first applied to a last-in-first-out stack means reversing.
            todo.extend(
                self.bumps
                    .get_parents_of_last_bumps(&mono_commit, abs_sub_path)
                    .into_iter()
                    .rev(),
            );
        }
        if todo_next.is_empty() {
            // wanted_sub_commit was not found in the history.
            return Ok(None);
        }
        let mono_ancestors = todo_next
            .into_iter()
            .unique_by(|c| Rc::as_ptr(c).addr())
            .collect_vec();

        // Dig deeper into the submodule history.
        let mut all_parents = Vec::new();
        let mut expanded_parents = Vec::new();
        let mut some_parent_found = false;
        for wanted_sub_parent in wanted_sub_commit.parents.iter() {
            if let Some(expanded_parent) = self.inject_submodule_commit_memo(
                mono_ancestors.clone(),
                abs_sub_path,
                wanted_sub_repo_name,
                wanted_sub_parent,
                sub_to_mono_commit,
            )? {
                some_parent_found = true;
                expanded_parents.push(expanded_parent.clone());
                all_parents.push(MonoRepoParent::Mono(expanded_parent));
            } else {
                // Use the original submodule commit as parent if some of the
                // parents are found but not others.
                all_parents.push(MonoRepoParent::OriginalSubmod(OriginalSubmodParent {
                    commit_id: wanted_sub_parent.commit_id,
                }));
            }
        }
        if !some_parent_found {
            return Ok(None);
        }

        let parents_for_submodules =
            self.expand_inner_submodules(&expanded_parents, abs_sub_path, wanted_sub_commit)?;
        all_parents.extend(parents_for_submodules);
        // TODO: 2025-09-22 Can this code be cleaner in some way?
        const TREE_MODE: &[u8] = b"040000";
        let mut tree_id_hex = gix::hash::Kind::hex_buf();
        let _len = wanted_sub_commit.tree_id.hex_to_buf(&mut tree_id_hex);
        let file_changes = vec![ChangedFile {
            path: abs_sub_path.deref().clone(),
            change: crate::git_fast_export_import::FileChange::Modified {
                mode: TREE_MODE.into(),
                hash: tree_id_hex.into(),
            },
        }];
        let mono_commit = self.emit_mono_commit(
            abs_sub_path,
            &RepoName::SubRepo(wanted_sub_repo_name.clone()),
            wanted_sub_commit,
            all_parents,
            file_changes,
            None,
        )?;
        Ok(Some(mono_commit))
    }
}

pub struct BumpCache {
    submodules: LruCache<BumpCacheKey, Rc<ExpandedSubmodule>>,
    last_bumps: LruCache<BumpCacheKey, Vec<Rc<MonoRepoCommit>>>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct BumpCacheKey {
    mono_commit: RcKey<MonoRepoCommit>,
    abs_sub_path: GitPath,
}

impl BumpCache {
    /// Find the submodule content in for `abs_sub_path` in `mono_commit`. Note
    /// that the same submodule might exist multiple times in a recursive
    /// submodule graph, which should be supported.
    ///
    /// In case a submodule has moved, it should be found as removed + added. If
    /// there are multiple removed and multiple added, the resolution is limited
    /// to only support that all removed gitlinks point to the same submodule
    /// commit id.
    ///
    /// If a submodule is added but also exists since before, a new history is
    /// not merged in if all existing gitlinks point to the same commit id.
    pub fn get_some_submodule(
        &mut self,
        mono_commit: &Rc<MonoRepoCommit>,
        abs_sub_path: &GitPath,
        submod_repo_name: &SubRepoName,
    ) -> Option<Rc<ExpandedSubmodule>> {
        if let Some(submod) = self.get_submodule(mono_commit, abs_sub_path) {
            return Some(submod);
        }
        // abs_sub_path does not exist in mono_commit. Try to resolve a
        // potential move of the submodule among all the paths that will not
        // exist in the resulting expanded commit.
        //
        // If the submodule wasn't moved, check if it is simply duplicated.
        let mut moved_orig_commit_ids = UniqueContainer::Empty;
        let mut duplicated_orig_commit_ids = UniqueContainer::Empty;
        for path in mono_commit.submodule_paths.iter() {
            let submod = self
                .get_submodule(mono_commit, path)
                .expect("submodule path exists");
            if let Some(submod_reference) = submod.get_known_submod()
                && submod_reference.repo_name == *submod_repo_name
            {
                if mono_commit.submodule_bumps.get(path)
                    == Some(&ExpandedOrRemovedSubmodule::Removed)
                {
                    // It is the same submodule and it has been moved or removed.
                    moved_orig_commit_ids.insert(submod.clone());
                }
                // It is the same submodule, so copied, moved or removed.
                duplicated_orig_commit_ids.insert(submod);
            }
        }
        // If there are multiple submodule commit ids to choose from, then
        // it cannot be determined which path was moved.
        match moved_orig_commit_ids {
            UniqueContainer::Empty => {}
            UniqueContainer::Single(submod) => {
                // Exactly one gitlink commit id to the same submodule repository
                // was removed, it must be ours that is moved.
                return Some(submod);
            }
            UniqueContainer::Multiple => {
                // Multiple submodule commit ids to choose from, cannot
                // determine which path was moved.
                return None;
            }
        }
        match duplicated_orig_commit_ids {
            UniqueContainer::Empty => {}
            UniqueContainer::Single(submod) => {
                // Exactly one gitlink commit id to the same submodule repository
                // existed previously, it must be ours that is duplicated.
                return Some(submod);
            }
            UniqueContainer::Multiple => {
                // Multiple submodule commit ids to choose from, cannot
                // determine which commit id was duplicated.
                return None;
            }
        }
        // Not found.
        None
    }

    pub fn get_submodule(
        &mut self,
        mono_commit: &Rc<MonoRepoCommit>,
        abs_sub_path: &GitPath,
    ) -> Option<Rc<ExpandedSubmodule>> {
        if !mono_commit.submodule_paths.contains(abs_sub_path) {
            // The submodule doesn't exist in this commit.
            return None;
        }

        let mut key = BumpCacheKey {
            mono_commit: RcKey::new(mono_commit),
            abs_sub_path: abs_sub_path.clone(),
        };

        let mut depth: usize = 0;
        let mut current_commit = mono_commit;
        let ret = loop {
            key.mono_commit = RcKey::new(current_commit);
            // Cached?
            if let Some(submod) = self.submodules.get(&key) {
                break submod.clone();
            }
            // Was the submodule updated in this commit?
            match current_commit.submodule_bumps.get(abs_sub_path) {
                Some(ExpandedOrRemovedSubmodule::Expanded(submod)) => {
                    break Rc::new(submod.clone());
                }
                Some(ExpandedOrRemovedSubmodule::Removed) => {
                    // The submodule was removed in this commit.
                    unreachable!("removed submodule exists");
                }
                None => {
                    // The submodule was not updated in this commit.
                }
            }
            // Recursively find the submodule through the first parent chain.
            let Some(MonoRepoParent::Mono(ancestor_commit)) = current_commit.parents.first() else {
                unreachable!("the submodule was never added");
            };

            current_commit = ancestor_commit;
            depth += 1;
        };

        // Insert the result into the cache.
        current_commit = mono_commit;
        for i in 0..depth {
            // Don't cache every commit in the history of all submodules, that fills
            // up the caches too much.
            if i.is_power_of_two() {
                let key = BumpCacheKey {
                    mono_commit: RcKey::new(current_commit),
                    abs_sub_path: abs_sub_path.clone(),
                };
                self.submodules.put(key, ret.clone());
            }
            let Some(MonoRepoParent::Mono(ancestor_commit)) = current_commit.parents.first() else {
                unreachable!("ancestors existed last time");
            };
            current_commit = ancestor_commit;
        }
        Some(ret)
    }

    pub fn get_top_bump(&self, mut mono_commit: &Rc<MonoRepoCommit>) -> Option<TopRepoCommitId> {
        // TODO: 2025-09-22 Is caching needed?
        loop {
            if let Some(top_bump) = &mono_commit.top_bump {
                return Some(*top_bump);
            }
            let Some(MonoRepoParent::Mono(first_parent)) = mono_commit.parents.first() else {
                return None;
            };
            mono_commit = first_parent;
        }
    }
    fn non_descendants_for_all_parents(
        &mut self,
        mono_parents: &Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        submod_repo_name: &SubRepoName,
        submod_commit: &ThinCommit,
        submod_storage: &RepoData,
    ) -> Vec<CommitId> {
        let mut non_descendants = Vec::new();
        for parent in mono_parents {
            if let Some(submod) = self.get_some_submodule(parent, abs_sub_path, submod_repo_name) {
                let submod_commit_id = submod.get_orig_commit_id();
                if let Some(parent_submod_commit) =
                    submod_storage.thin_commits.get(submod_commit_id)
                {
                    if !submod_commit.is_descendant_of(parent_submod_commit) {
                        non_descendants.push(*submod_commit_id);
                    }
                } else {
                    // Unknown submodule commit, just assume it is an ancestor.
                }
            } else {
                // The submodule is missing in the parent.
            }
        }
        non_descendants
    }

    pub fn get_parents_of_last_bumps(
        &mut self,
        mono_commit: &Rc<MonoRepoCommit>,
        abs_sub_path: &GitPath,
    ) -> Vec<Rc<MonoRepoCommit>> {
        let bumps = self.get_bumps(mono_commit, abs_sub_path);
        bumps
            .iter()
            .flat_map(|bump_commit| &bump_commit.parents)
            .filter_map(|parent| match parent {
                MonoRepoParent::Mono(parent) => Some(parent),
                MonoRepoParent::OriginalSubmod(_) => None,
            })
            .unique_by(|c| Rc::as_ptr(c).addr())
            .cloned()
            .collect_vec()
    }

    // TODO: 2025-10-18 Rewrite to a generator to be able to reduce the
    // last_bumps cache size and speed up.
    pub fn get_bumps<'a>(
        &'a mut self,
        mono_commit: &Rc<MonoRepoCommit>,
        abs_sub_path: &GitPath,
    ) -> &'a Vec<Rc<MonoRepoCommit>> {
        // This function is prone to stack overflow if implemented recursively.
        //
        // Caching information far back in the history is probably less useful.
        // Cache more sparsely the deeper we go.
        let mut key = BumpCacheKey {
            mono_commit: RcKey::new(mono_commit),
            abs_sub_path: abs_sub_path.clone(),
        };
        if self.last_bumps.contains(&key) {
            return self.last_bumps.get(&key).unwrap();
        }

        struct StackEntry {
            mono_commit: Rc<MonoRepoCommit>,
            commit_key: RcKey<MonoRepoCommit>,
            next_parent_idx: usize,
            ret: Vec<Rc<MonoRepoCommit>>,
        }
        // Start with a fake entry that will make mono_commit be computed and
        // cached.
        let mut stack = vec![StackEntry {
            mono_commit: MonoRepoCommit::new_rc(
                vec![MonoRepoParent::Mono(mono_commit.clone())],
                None,
                HashMap::new(),
            ),
            commit_key: key.mono_commit,
            next_parent_idx: 0,
            ret: Vec::new(),
        }];

        loop {
            let entry = stack.last_mut().unwrap();
            match entry.mono_commit.parents.get(entry.next_parent_idx) {
                Some(MonoRepoParent::Mono(parent)) => {
                    // Process one more parent of entry.mono_commit and add the
                    // result to entry.ret.
                    entry.next_parent_idx += 1;
                    key.mono_commit = RcKey::new(parent);
                    if let Some(parent_ret) = self.last_bumps.get(&key) {
                        // Cache hit.
                        entry.ret.extend(parent_ret.iter().cloned());
                    } else if parent.submodule_bumps.contains_key(abs_sub_path) {
                        // The submodule was added, updated (compared to first
                        // parent), moved or copied to abs_sub_path.
                        entry.ret.push(parent.clone());
                    } else if parent.submodule_paths.contains(abs_sub_path) {
                        // The submodule exists here, so push the parent to the
                        // stack.
                        let parent = parent.clone();
                        stack.push(StackEntry {
                            mono_commit: parent,
                            commit_key: key.mono_commit,
                            next_parent_idx: 0,
                            ret: Vec::new(),
                        });
                    } else {
                        // The submodule does not exist at abs_sub_path in this
                        // parent.
                    }
                }
                Some(MonoRepoParent::OriginalSubmod(_)) => {
                    // The parent is pointing to the original submodule commit,
                    // not an expanded mono commit. Nothing to do.
                    entry.next_parent_idx += 1;
                }
                None => {
                    // No more parents. Extend the child's entry.ret with this
                    // parent's entry.ret.
                    let entry = stack.pop().expect("processing stack entry");
                    key.mono_commit = entry.commit_key;
                    let ret = entry.ret.into_iter().unique_by(RcKey::new).collect_vec();
                    if stack.is_empty() {
                        self.last_bumps.put(key.clone(), ret);
                        return self.last_bumps.get(&key).expect("just inserted");
                    }
                    // Add the result to the child.
                    stack.last_mut().unwrap().ret.extend(ret.iter().cloned());
                    self.last_bumps.put(key.clone(), ret);
                }
            }
        }
    }
}

impl Default for BumpCache {
    fn default() -> Self {
        Self {
            submodules: LruCache::new(std::num::NonZeroUsize::new(100000).unwrap()),
            last_bumps: LruCache::new(std::num::NonZeroUsize::new(100000).unwrap()),
        }
    }
}

/// Removes a given prefix from a reference name. The prefix often ends with a
/// slash.
///
/// # Examples
/// ```
/// # use git_toprepo::expander::strip_ref_prefix;
/// use gix::refs::FullName;
/// use std::borrow::Borrow as _;
///
/// fn make_ref(name: &str) -> FullName {
///     FullName::try_from(name).unwrap()
/// }
///
/// assert_eq!(
///     strip_ref_prefix(
///         &make_ref("refs/namespaces/top/refs/remotes/origin/foo"),
///         "refs/namespaces/top/"
///     )
///     .unwrap(),
///     make_ref("refs/remotes/origin/foo"),
/// );
/// assert_eq!(
///     strip_ref_prefix(
///         &make_ref("refs/namespaces/top/HEAD"),
///         "refs/namespaces/top/"
///     )
///     .unwrap(),
///     make_ref("HEAD"),
/// );
///
/// assert_eq!(
///     strip_ref_prefix(&make_ref("refs/namespaces/top/HEAD"), "refs/namespaces/top")
///         .unwrap_err()
///         .to_string(),
///     "A reference must be a valid tag name as well",
/// );
/// assert_eq!(
///     strip_ref_prefix(&make_ref("refs/namespaces/top/HEAD"), "refs/other")
///         .unwrap_err()
///         .to_string(),
///     "Expected refs/namespaces/top/HEAD to start with refs/other",
/// );
/// ```
///
/// This function is public only to allow doc-testing and is not intended for
/// general use.
pub fn strip_ref_prefix<'a>(
    ref_name: impl AsRef<FullNameRef>,
    prefix: impl Into<&'a BStr>,
) -> Result<FullName> {
    let ref_name = ref_name.as_ref().as_bstr();
    let prefix = prefix.into();
    if let Some(new_ref_name) = ref_name.strip_prefix(prefix.as_bytes()) {
        FullName::try_from(new_ref_name.as_bstr()).map_err(|err| anyhow::anyhow!("{err:#}"))
    } else {
        anyhow::bail!("Expected {ref_name} to start with {prefix}");
    }
}

// TODO: 2025-09-22 Let Gitoxide implement try_find_full_reference().
fn try_find_full_reference(
    repo: &gix::Repository,
    name: &FullNameRef,
) -> Result<Option<gix::refs::Reference>> {
    let refs_platform = repo.references()?;
    let mut repo_refs = refs_platform.prefixed(name.as_bstr())?;
    if let Some(r) = repo_refs.next() {
        let r = r.map_err(|err| anyhow::anyhow!("Failed to get first matching ref: {err:#}"))?;
        if r.name() == name {
            return Ok(Some(r.detach()));
        }
    }
    #[cfg(debug_assertions)]
    for r in repo_refs {
        let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
        assert_ne!(r.name(), name);
    }
    Ok(None)
}

/// Recombine all `refs/namespaces/top/refs/*` into `refs/*`.
pub fn recombine_all_top_refs(
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
) -> Result<()> {
    let top_ref_prefix = format!("{}refs/", RepoName::Top.to_ref_prefix());

    let repo_refs = configured_repo.gix_repo.references()?;
    let mut refs = Vec::new();
    for r in repo_refs.prefixed(BStr::new(top_ref_prefix.as_bytes()))? {
        let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
        refs.push(r.detach());
    }
    recombine(configured_repo, progress, refs, true)
}

pub fn recombine_some_top_refspecs(
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
    top_ref_names: impl IntoIterator<Item = impl AsRef<FullNameRef>>,
) -> Result<()> {
    let mut refs = Vec::new();
    for top_ref in top_ref_names {
        let r = try_find_full_reference(&configured_repo.gix_repo, top_ref.as_ref())?
            .with_context(|| format!("Reference {} does not exist", top_ref.as_ref().as_bstr()))?;
        refs.push(r);
    }
    recombine(configured_repo, progress, refs, false)
}

fn recombine(
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
    top_refs: Vec<gix::refs::Reference>,
    remove_refs: bool,
) -> Result<()> {
    let old_monorepo_refs_names = read_monorepo_refs_log(&configured_repo.gix_repo)?;
    let mut old_monorepo_refs = HashMap::with_capacity(old_monorepo_refs_names.len());
    for old_name in old_monorepo_refs_names {
        if let Some(old_ref) = try_find_full_reference(&configured_repo.gix_repo, old_name.as_ref())
            .with_context(|| format!("Failed to resolve reference {old_name}"))?
        {
            // Doesn't matter if it is a mono-repo commit or not, we are just updating the normal git-refs.
            old_monorepo_refs.insert(old_name, old_ref.target);
        } else {
            // The old ref is missing, so the user must have removed it manually.
            log::debug!("Previously written ref {old_name} is missing");
        }
    }

    let top_ref_prefix = RepoName::Top.to_ref_prefix();

    let mut update_actions = BTreeMap::new();
    let mut final_monorefs = Vec::with_capacity(top_refs.len());
    let mut todo_commit_tips = Vec::with_capacity(top_refs.len());
    let mut todo_annotated_tags = Vec::with_capacity(top_refs.len());
    let mut toprepo_commit_ids_to_filter = Vec::with_capacity(top_refs.len());
    for r in top_refs {
        let monorepo_ref_name = strip_ref_prefix(&r.name, top_ref_prefix.as_str())
            .with_context(|| format!("Bad toprepo ref {}", r.name.as_bstr()))?;
        let old_target = old_monorepo_refs.get(&monorepo_ref_name);
        match r.target {
            gix::refs::Target::Symbolic(top_target_name) => {
                // Simply update the symbolic references after the combine
                // process.
                let Ok(monorepo_target_name) =
                    strip_ref_prefix(&top_target_name, BStr::new(&top_ref_prefix))
                else {
                    log::warn!(
                        "Skipping symbolic ref {} that points outside the top repo, to {}",
                        r.name.as_bstr(),
                        top_target_name.as_bstr(),
                    );
                    continue;
                };
                update_actions.insert(
                    monorepo_ref_name.clone(),
                    MonoRefUpdateAction::new(
                        old_target.cloned(),
                        Some(gix::refs::Target::Symbolic(monorepo_target_name)),
                    )
                    .unwrap(),
                );
            }
            gix::refs::Target::Object(object_id) => {
                let target_kind = configured_repo
                    .gix_repo
                    .find_header(object_id)
                    .with_context(|| {
                        format!("Failed to resolve non-symbolic ref {}", r.name.as_bstr())
                    })?
                    .kind();
                match target_kind {
                    gix::object::Kind::Tree => {
                        log::warn!("Skipping ref {} that points to a tree", r.name.as_bstr());
                        continue;
                    }
                    gix::object::Kind::Blob => {
                        log::warn!("Skipping ref {} that points to a blob", r.name.as_bstr());
                        continue;
                    }
                    gix::object::Kind::Tag => {
                        let tag_target = configured_repo
                            .gix_repo
                            .find_object(object_id)
                            .with_context(|| {
                                format!("Failed to read tag object {}", r.name.as_bstr())
                            })?;
                        let ultimate_target = tag_target.peel_tags_to_end().with_context(|| {
                            format!(
                                "Failed to peel tag {} to the ultimate target",
                                r.name.as_bstr()
                            )
                        })?;
                        match ultimate_target.kind {
                            gix::object::Kind::Blob => {
                                log::warn!(
                                    "Skipping tag {} that (ultimately) points to a blob",
                                    r.name.as_bstr()
                                );
                            }
                            gix::object::Kind::Tree => {
                                log::warn!(
                                    "Skipping tag {} that (ultimately) points to a tree",
                                    r.name.as_bstr()
                                );
                            }
                            gix::object::Kind::Tag => unreachable!("peeled to non-tag target"),
                            gix::object::Kind::Commit => {
                                todo_annotated_tags.push((
                                    monorepo_ref_name.clone(),
                                    object_id.to_owned(),
                                    old_target,
                                ));
                                toprepo_commit_ids_to_filter
                                    .push(TopRepoCommitId::new(ultimate_target.id));
                            }
                        }
                    }
                    gix::object::Kind::Commit => {
                        let commit_id = TopRepoCommitId::new(object_id.to_owned());
                        todo_commit_tips.push((monorepo_ref_name.clone(), commit_id, old_target));
                        toprepo_commit_ids_to_filter.push(commit_id);
                    }
                }
            }
        }
        final_monorefs.push(monorepo_ref_name);
    }
    final_monorefs.sort();

    // Mark all the old refs (already in the file) and all the new refs (the
    // user has asked them to be overwritten) as okay to be removed if anything
    // fails.
    let old_and_new_monorefs = old_monorepo_refs
        .keys()
        .chain(final_monorefs.iter())
        .unique()
        .sorted();
    write_monorepo_refs_log(&configured_repo.gix_repo, old_and_new_monorefs.as_slice())?;

    toprepo_commit_ids_to_filter.retain(|commit_id| {
        !configured_repo
            .import_cache
            .top_to_mono_commit_map
            .contains_key(commit_id)
    });
    if !toprepo_commit_ids_to_filter.is_empty() {
        let pb = progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{elapsed:>4} {msg} {pos}")
                        .unwrap(),
                )
                .with_message("Looking for new commits to expand"),
        );
        let (stop_commit_ids, num_commits_to_export) = crate::git::get_first_known_commits(
            &configured_repo.gix_repo,
            toprepo_commit_ids_to_filter
                .iter()
                .map(|top_commit_id| *top_commit_id.deref()),
            |object_id| {
                configured_repo
                    .import_cache
                    .top_to_mono_commit_map
                    .contains_key(&TopRepoCommitId::new(object_id))
            },
        )?;
        let stop_commit_ids = stop_commit_ids
            .into_iter()
            .map(TopRepoCommitId::new)
            .collect_vec();
        drop(pb);

        log::info!("Found {num_commits_to_export} commits to expand");
        let fast_importer =
            crate::git_fast_export_import::FastImportRepo::new(configured_repo.gix_repo.git_dir())?;
        let mut expander = Expander {
            gix_repo: &configured_repo.gix_repo,
            ledger: &mut configured_repo.ledger,
            import_cache: &mut configured_repo.import_cache,
            progress: progress.clone(),
            fast_importer,
            imported_commits: HashMap::new(),
            expanded_top_commits: HashMap::new(),
            bumps: crate::expander::BumpCache::default(),
            inject_at_oldest_super_commit: false,
        };
        expander.expand_toprepo_commits(
            &toprepo_commit_ids_to_filter,
            &stop_commit_ids,
            num_commits_to_export,
        )?;
        expander.wait()?;
    }
    // Map the refs to the expanded mono commit ids.
    // Collect all new monorepo commits.
    for (monorepo_ref_name, top_commit_id, old_target) in todo_commit_tips {
        let (mono_commit_id, _mono_commit) = configured_repo
            .import_cache
            .top_to_mono_commit_map
            .get(&top_commit_id)
            .expect("mono commit was cached or just filtered and must therefore exist");
        update_actions.insert(
            monorepo_ref_name,
            MonoRefUpdateAction::new(
                old_target.cloned(),
                Some(gix::refs::Target::Object(*mono_commit_id.deref())),
            )
            .unwrap(),
        );
    }
    // Convert all the tags.
    for (monorepo_ref_name, annotated_tag_id, old_target) in todo_annotated_tags {
        let _log_scope_guard = crate::log::scope(format!("Writing mono tag {monorepo_ref_name:?}"));
        let target_mono_id = match write_mono_tag_chain(configured_repo, annotated_tag_id) {
            Ok(id) => id,
            Err(err) => {
                log::warn!("{err:#}");
                continue;
            }
        };
        update_actions.insert(
            monorepo_ref_name,
            MonoRefUpdateAction::new(
                old_target.cloned(),
                Some(gix::refs::Target::Object(target_mono_id)),
            )
            .unwrap(),
        );
    }
    // Find out which old refs are deleted.
    if remove_refs {
        for (old_name, old_target) in old_monorepo_refs {
            update_actions.entry(old_name).or_insert_with(|| {
                MonoRefUpdateAction::new(Some(old_target.clone()), None).unwrap()
            });
        }
    }

    update_refs(&configured_repo.gix_repo, &update_actions)?;
    print_updated_refs(configured_repo, progress, &update_actions);
    if remove_refs {
        let remaining_monorefs = update_actions
            .iter()
            .filter_map(|(name, action)| match action {
                MonoRefUpdateAction::Delete { .. } => None,
                _ => Some(name),
            })
            .collect_vec();
        // No refs were removed, so nothing to clean up in the log file.
        write_monorepo_refs_log(&configured_repo.gix_repo, remaining_monorefs.as_slice())?;
    }
    Ok(())
}

/// Translates a chain of top-repo tags, starting at `annotated_tag_id`, into
/// mono-repo tags where the ultimate target must be a commit that has already
/// been translated to a corresponding mono-repo commit.
///
/// In case of a badly encoded intermediate tag, a warning is logged and the tag
/// is skipped.
///
/// The id of the last written mono tag in the the chain, or potentially the
/// target mono commit, will be returned.
fn write_mono_tag_chain(
    configured_repo: &ConfiguredTopRepo,
    annotated_tag_id: gix::ObjectId,
) -> Result<gix::ObjectId> {
    let mut nested_top_tags = Vec::with_capacity(1);
    let mut top_tag_id = annotated_tag_id;
    let mut top_tag = configured_repo
        .gix_repo
        .find_object(annotated_tag_id)?
        .into_tag();
    let mut target_mono_id = loop {
        let target_object_id = top_tag
            .target_id()
            .with_context(|| format!("Failed to peel previously peeled tag {top_tag_id}"))?;
        let target_object = target_object_id.object().with_context(|| {
            format!("Failed to resolve previously resolved tag target object {target_object_id}")
        })?;
        nested_top_tags.push((top_tag_id, top_tag));
        match target_object.kind {
            gix::objs::Kind::Blob => {
                unreachable!(
                    "Already skipped tag {} that (ultimately) points to a blob",
                    top_tag_id.to_hex()
                );
            }
            gix::objs::Kind::Tree => {
                unreachable!(
                    "Already skipped tag {} that (ultimately) points to a tree",
                    top_tag_id.to_hex()
                );
            }
            gix::objs::Kind::Tag => {
                // Nested tag, continue.
                top_tag_id = target_object.id;
                top_tag = target_object.into_tag();
            }
            gix::objs::Kind::Commit => {
                // Found the commit, done.
                let target_top_id = target_object.id;
                let (target_mono_id, _target_mono_commit) = configured_repo
                    .import_cache
                    .top_to_mono_commit_map
                    .get(&TopRepoCommitId::new(target_top_id))
                    .expect("mono commit was cached or just filtered and must therefore exist");
                break *target_mono_id.deref();
            }
        }
    };
    // Recreate all the nested tags, but pointing to the mono commits.
    let mut target_kind = gix::objs::Kind::Commit;
    for (top_tag_id, top_tag) in nested_top_tags.into_iter().rev() {
        let decoded_top_tag = match top_tag.decode() {
            Ok(tag) => tag,
            Err(err) => {
                log::warn!(
                    "Ignoring intermediate tag {top_tag_id} that cannot be decoded: {err:#}"
                );
                continue;
            }
        };
        let tagger = match decoded_top_tag
            .tagger
            .map(|tagger| tagger.to_owned())
            .transpose()
        {
            Ok(tagger) => tagger,
            Err(err) => {
                log::warn!("Ignoring intermediate tag {top_tag_id} with bad tagger: {err:#}");
                continue;
            }
        };
        // Create an identical tag object but pointing to the mono commit instead.
        let mono_tag = gix::objs::Tag {
            target: target_mono_id,
            target_kind,
            name: decoded_top_tag.name.to_owned(),
            tagger,
            message: decoded_top_tag.message.to_owned(),
            // The PGP signature is impossible to recreate as the target has changed to a mono target.
            pgp_signature: None,
        };
        // Repository::write_object() ensures that existing objects are not written again.
        target_mono_id = configured_repo.gix_repo.write_object(&mono_tag)?.detach();
        target_kind = gix::objs::Kind::Tag;
    }
    Ok(target_mono_id)
}

/// Prints the resulting ref updates, in the same fashion like git-fetch.
fn print_updated_refs(
    configured_repo: &ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
    update_actions: &BTreeMap<FullName, MonoRefUpdateAction>,
) {
    let update_results = update_actions
        .iter()
        .filter_map(|(name, action)| {
            let sort_order = match action {
                MonoRefUpdateAction::Create { .. } => 1,
                MonoRefUpdateAction::Update { .. } => 2,
                MonoRefUpdateAction::Unchanged { .. } => {
                    if crate::log::get_global_logger().get_stderr_log_level() < log::Level::Debug {
                        // Don't print "up-to-date" in the default info log level.
                        return None;
                    }
                    3
                }
                MonoRefUpdateAction::Delete { .. } => 4,
            };
            let (description, short_name) = action.describe(
                name,
                &configured_repo.gix_repo,
                &configured_repo.import_cache,
            );
            Some((sort_order, short_name, description))
        })
        .sorted();
    let update_results = update_results.as_slice();
    // git-fetch use 20 as minimum length when using 7 hex digits for object ids.
    let min_description_len = 20;
    let max_description_len = update_results
        .iter()
        .map(|(_, _, desc)| desc.len())
        .chain([min_description_len])
        .max()
        .unwrap();
    progress.suspend(|| {
        for (_, short_name, description) in update_results {
            println!(" {description:<max_description_len$} -> {short_name}");
        }
    });
}

/// Return a short description of the target of the reference.
fn short_ref_target_description(repo: &gix::Repository, target: &gix::refs::Target) -> String {
    match target {
        gix::refs::Target::Object(object_id) => object_id.attach(repo).shorten_or_id().to_string(),
        gix::refs::Target::Symbolic(target) => format!("link:{target}"),
    }
}
/// An action to perform on a mono-repo reference.
enum MonoRefUpdateAction {
    Create {
        new_target: gix::refs::Target,
    },
    Update {
        old_target: gix::refs::Target,
        new_target: gix::refs::Target,
    },
    Unchanged {
        target: gix::refs::Target,
    },
    Delete {
        old_target: gix::refs::Target,
    },
}

impl MonoRefUpdateAction {
    pub fn new(
        old_target: Option<gix::refs::Target>,
        new_target: Option<gix::refs::Target>,
    ) -> Option<MonoRefUpdateAction> {
        match (old_target, new_target) {
            (None, None) => None,
            (None, Some(new_target)) => Some(MonoRefUpdateAction::Create { new_target }),
            (Some(old_target), None) => Some(MonoRefUpdateAction::Delete { old_target }),
            (Some(old_target), Some(new_target)) => Some(if old_target == new_target {
                MonoRefUpdateAction::Unchanged { target: old_target }
            } else {
                MonoRefUpdateAction::Update {
                    old_target,
                    new_target,
                }
            }),
        }
    }

    /// Describe the update action in a short human-readable form, similar to git-fetch.
    pub fn describe(
        &self,
        name: &FullName,
        repo: &gix::Repository,
        import_cache: &ImportCache,
    ) -> (String, String) {
        let (is_tag, short_name) = match name.category_and_short_name() {
            Some((gix::refs::Category::LocalBranch, short_name))
            | Some((gix::refs::Category::RemoteBranch, short_name)) => {
                // git-fetch also has the category "branch", but that clutters
                // the output so it is not used.
                (false, short_name.to_str_lossy().into_owned())
            }
            Some((gix::refs::Category::Tag, short_name)) => {
                (true, short_name.to_str_lossy().into_owned())
            }
            Some((_, _)) | None => {
                // git-fetch also has the category "ref", but that clutters
                // the output so it is not used.
                (false, name.as_bstr().to_str_lossy().into_owned())
            }
        };
        let maybe_tag_space = if is_tag { "tag " } else { "" };
        let maybe_space_tag = if is_tag { " tag" } else { "" };
        match self {
            MonoRefUpdateAction::Create { new_target } => (
                format!(
                    "* [new{maybe_space_tag}] {}",
                    short_ref_target_description(repo, new_target)
                ),
                short_name,
            ),
            MonoRefUpdateAction::Update {
                old_target,
                new_target,
            } => {
                let forced = if let gix::refs::Target::Object(old_target) = old_target
                    && let gix::refs::Target::Object(new_target) = new_target
                    && let Some(old_mono_commit) = import_cache
                        .monorepo_commits
                        .get(&MonoRepoCommitId::new(*old_target))
                    && let Some(new_mono_commit) = import_cache
                        .monorepo_commits
                        .get(&MonoRepoCommitId::new(*new_target))
                    && !old_mono_commit.is_ancestor_of(new_mono_commit)
                {
                    true
                } else {
                    false
                };
                let prefix = if name.category() == Some(gix::refs::Category::Tag) {
                    if forced {
                        "T [force updated tag]"
                    } else {
                        "t [updated tag]"
                    }
                } else if forced {
                    "+ [forced update]"
                } else {
                    " "
                };
                (
                    format!(
                        "{prefix} {}{}{}",
                        short_ref_target_description(repo, old_target),
                        if forced { "..." } else { ".." },
                        short_ref_target_description(repo, new_target),
                    ),
                    short_name,
                )
            }
            MonoRefUpdateAction::Unchanged { target } => (
                format!(
                    "= [{maybe_tag_space}up to date] {}",
                    short_ref_target_description(repo, target)
                ),
                short_name,
            ),
            MonoRefUpdateAction::Delete { old_target } => (
                format!(
                    "- [deleted{maybe_space_tag}] {}",
                    short_ref_target_description(repo, old_target)
                ),
                short_name,
            ),
        }
    }
}

/// Update the references in the repository, i.e. creating, updating and deleting
/// as needed. Also log the updates to the user.
///
/// Using `BTreeMap` to get a deterministic order of the updates.
fn update_refs(
    repo: &gix::Repository,
    updates: &BTreeMap<FullName, MonoRefUpdateAction>,
) -> Result<()> {
    let mut ref_edits = Vec::new();
    // Always delete our special import ref.
    ref_edits.push(gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Delete {
            expected: gix::refs::transaction::PreviousValue::Any,
            log: gix::refs::transaction::RefLog::AndReference,
        },
        name: FullName::try_from(Expander::TOPREPO_IMPORT_REF).unwrap(),
        deref: false,
    });

    for (name, action) in updates {
        match action {
            MonoRefUpdateAction::Create { new_target }
            | MonoRefUpdateAction::Update {
                old_target: _,
                new_target,
            } => {
                ref_edits.push(gix::refs::transaction::RefEdit {
                    change: gix::refs::transaction::Change::Update {
                        log: gix::refs::transaction::LogChange {
                            mode: gix::refs::transaction::RefLog::AndReference,
                            force_create_reflog: false,
                            message: b"git-toprepo filter".into(),
                        },
                        expected: gix::refs::transaction::PreviousValue::Any,
                        new: new_target.clone(),
                    },
                    name: name.clone(),
                    deref: false,
                });
            }
            MonoRefUpdateAction::Unchanged { .. } => {}
            MonoRefUpdateAction::Delete { old_target: _ } => {
                ref_edits.push(gix::refs::transaction::RefEdit {
                    change: gix::refs::transaction::Change::Delete {
                        // TODO: 2025-09-22 Is MustExistAndMatch possible? Should the previous
                        // filter result be stored in the log file?
                        expected: gix::refs::transaction::PreviousValue::Any,
                        log: gix::refs::transaction::RefLog::AndReference,
                    },
                    name: name.clone(),
                    deref: false,
                });
            }
        }
    }
    // Apply the ref changes.
    if !ref_edits.is_empty() {
        let committer = gix::actor::SignatureRef {
            name: "git-toprepo".as_bytes().as_bstr(),
            email: BStr::new(""),
            time: &gix::date::Time::now_local_or_utc().format(gix::date::time::Format::Raw),
        };
        repo.edit_references_as(ref_edits, Some(committer))
            .context("Failed to update all the mono references")?;
    }
    Ok(())
}

pub fn expand_submodule_ref_onto_head(
    configured_repo: &mut ConfiguredTopRepo,
    progress: &indicatif::MultiProgress,
    ref_to_inject: &FullNameRef,
    sub_repo_name: &SubRepoName,
    abs_sub_path: &GitPath,
    dest_ref: &FullNameRef,
) -> Result<Rc<MonoRepoCommit>> {
    let mut ref_to_inject = configured_repo.gix_repo.refs.find(ref_to_inject)?;
    let id_to_inject = ref_to_inject.peel_to_id_in_place(
        &configured_repo.gix_repo.refs,
        &configured_repo.gix_repo.objects,
    )?;
    let thin_commit_to_inject = configured_repo
        .import_cache
        .repos
        .get(&RepoName::SubRepo(sub_repo_name.clone()))
        .and_then(|repo_data| repo_data.thin_commits.get(&id_to_inject))
        .with_context(|| {
            format!(
                "Failed to find {}, commit {}",
                ref_to_inject.name,
                id_to_inject.to_hex()
            )
        })?
        .clone(); // Clone to avoid borrowing the `import_cache` object.

    let pb = progress.add(
        indicatif::ProgressBar::no_length()
            .with_style(
                indicatif::ProgressStyle::default_spinner()
                    .template("{elapsed:>4} {msg} {pos}")
                    .unwrap(),
            )
            .with_message("Looking for mono commit to expand onto"),
    );
    // Hopefully, HEAD points to a commit.
    let head_id: CommitId = configured_repo
        .gix_repo
        .head_commit()
        .context("Could not resolve HEAD as a commit")?
        .id;
    let mut possible_mono_parents = Vec::new();
    let (_possible_mono_parent_ids, _num_skipped_unknowns) = crate::git::get_first_known_commits(
        &configured_repo.gix_repo,
        [head_id].into_iter(),
        |commit_id| {
            let Some(mono_parent) = configured_repo
                .import_cache
                .monorepo_commits
                .get(&MonoRepoCommitId::new(commit_id))
            else {
                return false;
            };
            possible_mono_parents.push(mono_parent.clone());
            true
        },
    )?;
    drop(pb);

    let fast_importer =
        crate::git_fast_export_import::FastImportRepo::new(configured_repo.gix_repo.git_dir())?;
    let mut expander = Expander {
        gix_repo: &configured_repo.gix_repo,
        ledger: &mut configured_repo.ledger,
        import_cache: &mut configured_repo.import_cache,
        progress: progress.clone(),
        fast_importer,
        imported_commits: HashMap::new(),
        expanded_top_commits: HashMap::new(),
        bumps: crate::expander::BumpCache::default(),
        inject_at_oldest_super_commit: true,
    };
    let expand_result = (|| {
        let Some(mono_commit) = expander.inject_submodule_commit(
            possible_mono_parents,
            abs_sub_path,
            sub_repo_name,
            &thin_commit_to_inject,
        )?
        else {
            anyhow::bail!(
                "Failed to expand commit {}, to become {}, at {abs_sub_path}: \
                No common history with HEAD, running 'git toprepo recombine' may help",
                ref_to_inject.name,
                dest_ref.as_bstr()
            );
        };
        Ok(mono_commit)
    })();
    expander.wait()?;
    let expanded_mono_commit = expand_result?;
    let expanded_mono_commit_id = configured_repo
        .import_cache
        .monorepo_commit_ids
        .get(&RcKey::new(&expanded_mono_commit))
        .expect("just expanded commit must be known");

    // Update the ref as well.
    let old_ref = try_find_full_reference(&configured_repo.gix_repo, dest_ref)
        .with_context(|| format!("Failed to resolve reference {}", dest_ref.as_bstr()))?;
    let ref_updates = BTreeMap::from([(
        dest_ref.to_owned(),
        MonoRefUpdateAction::new(
            old_ref.map(|r| r.target),
            Some(gix::refs::Target::Object(*expanded_mono_commit_id.deref())),
        )
        .unwrap(),
    )]);
    update_refs(&configured_repo.gix_repo, &ref_updates)?;
    print_updated_refs(configured_repo, progress, &ref_updates);
    Ok(expanded_mono_commit)
}

/// Reads the monorepo refs that was the result of the last submodule expansion.
fn read_monorepo_refs_log(repo: &gix::Repository) -> Result<Vec<FullName>> {
    let refs_path = repo.common_dir().join("toprepo/mono-refs-ok-to-remove");
    if !refs_path.exists() {
        return Ok(Vec::new());
    }
    std::fs::read(&refs_path)
        .with_context(|| format!("Failed to read monorepo ref log at {}", refs_path.display()))?
        .lines()
        .map(|line| {
            FullName::try_from(line.as_bstr()).with_context(|| {
                format!(
                    "Bad ref {:?} in {}",
                    line.to_str_lossy(),
                    refs_path.display()
                )
            })
        })
        .collect::<Result<Vec<_>>>()
}

/// Writes the monorepo refs that have resulted from the submodule expansion.
fn write_monorepo_refs_log(repo: &gix::Repository, monorepo_refs: &[&FullName]) -> Result<()> {
    let refs_path = repo.common_dir().join("toprepo/mono-refs-ok-to-remove");
    if let Some(parent) = refs_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let refs_path_tmp = refs_path.with_extension("tmp");
    (|| -> Result<()> {
        let mut file = std::fs::File::create(&refs_path_tmp)?;
        for &r in monorepo_refs {
            writeln!(file, "{}", r.as_bstr())?;
        }
        Ok(())
    })()
    .with_context(|| format!("Failed to write {}", refs_path_tmp.display()))?;
    std::fs::rename(&refs_path_tmp, &refs_path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            refs_path_tmp.display(),
            refs_path.display()
        )
    })?;
    Ok(())
}
