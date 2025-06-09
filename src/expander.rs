use crate::config::GitTopRepoConfig;
use crate::git::CommitId;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git_fast_export_import::ChangedFile;
use crate::git_fast_export_import::FastExportCommit;
use crate::git_fast_export_import::FastImportCommit;
use crate::git_fast_export_import::ImportCommitRef;
use crate::log::Logger;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use crate::repo::MonoRepoCommit;
use crate::repo::MonoRepoCommitId;
use crate::repo::MonoRepoParent;
use crate::repo::OriginalSubmodParent;
use crate::repo::RepoData;
use crate::repo::SubmoduleContent;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::ThinSubmoduleContent;
use crate::repo::TopRepoCache;
use crate::repo::TopRepoCommitId;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::RcKey;
use crate::util::UniqueContainer;
use anyhow::Context as _;
use anyhow::Result;
use bstr::B;
use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice as _;
use bstr::ByteVec;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools as _;
use lru::LruCache;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::io::Write;
use std::ops::Deref;
use std::rc::Rc;

pub struct TopRepoExpander<'a> {
    pub gix_repo: &'a gix::Repository,
    pub storage: &'a mut TopRepoCache,
    pub config: &'a GitTopRepoConfig,
    pub progress: indicatif::MultiProgress,
    pub logger: Logger,
    pub fast_importer: crate::git_fast_export_import::FastImportRepo,
    pub imported_commits: HashMap<RcKey<MonoRepoCommit>, (usize, Rc<MonoRepoCommit>)>,
    pub bumps: BumpCache,
    pub inject_at_oldest_super_commit: bool,
}

impl TopRepoExpander<'_> {
    /// Creates a list of not yet expanded top repo commits needed to expand the
    /// given tips. The returned list is sorted in the order to be expanded.
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

        let toprepo_commits = &self.storage.top_to_mono_map;
        let walk = self.gix_repo.rev_walk(toprepo_tips);
        let mut commits_to_expand: Vec<_> = Vec::new();
        walk.selected(|commit_id| {
            let commit_id = TopRepoCommitId::new(commit_id.to_owned());
            if toprepo_commits.contains_key(&commit_id) {
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

    /// `top_refs_to_mono_ref` maps top refs like
    /// `refs/namespaces/top/refs/heads/branch` to monorepo refs like
    /// `refs/remotes/origin/branch`.
    pub fn expand_toprepo_commits(
        &mut self,
        top_refs: &[gix::refs::FullName],
        stop_commit_ids: Vec<CommitId>,
        c: usize,
    ) -> Result<()> {
        self.progress
            .suspend(|| eprintln!("Expanding the toprepo to a monorepo..."));
        self.progress
            .set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));
        let pb = self.progress.add(
            indicatif::ProgressBar::new(c as u64)
                .with_style(
                    indicatif::ProgressStyle::with_template(
                        "{elapsed:>4} {prefix:.cyan} [{bar:24}] {pos}/{len} ({eta})",
                    )
                    .unwrap()
                    .progress_chars("=> "),
                )
                .with_prefix("Expanding commits"),
        );

        let start_refs = top_refs;
        let fast_exporter = crate::git_fast_export_import::FastExportRepo::load_from_path(
            self.gix_repo.git_dir(),
            Some(
                start_refs
                    .iter()
                    .map(|name| name.as_bstr().to_os_str().unwrap().to_owned())
                    .chain(stop_commit_ids.iter().map(|id| {
                        format!("^{}", id.to_hex())
                            .as_bytes()
                            .to_os_str()
                            .unwrap()
                            .to_owned()
                    })),
            ),
            self.logger.clone(),
        )?;

        (|| {
            let top_ref_prefix = RepoName::Top.to_ref_prefix();
            for entry in fast_exporter {
                let entry = entry?; // TODO: error handling
                match entry {
                    crate::git_fast_export_import::FastExportEntry::Commit(commit) => {
                        let input_branch = commit.branch.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("Top repo commit {} has no branch", commit.original_id)
                        })?;
                        let output_branch = strip_ref_prefix(input_branch, top_ref_prefix.as_str()).with_context(|| {
                            format!(
                                "Bad git-fast-export branch {input_branch} which was not requested"
                            )
                        })?;
                        let commit_id = TopRepoCommitId::new(commit.original_id);
                        let now = std::time::Instant::now();
                        self.expand_toprepo_commit(output_branch.as_ref(), commit)?;
                        let ms = now.elapsed().as_millis();
                        if ms > 100 {
                            // TODO: Remove this debug print.
                            pb.suspend(|| eprintln!("DEBUG: Commit {commit_id} took {ms} ms"));
                        }
                        pb.inc(1);
                    }
                    crate::git_fast_export_import::FastExportEntry::Reset(reset) => {
                        let input_branch = &reset.branch;
                        let output_branch = strip_ref_prefix(input_branch, top_ref_prefix.as_str()).with_context(|| {
                            format!(
                                "Bad git-fast-export branch {input_branch} which was not requested"
                            )
                        })?;
                        let mono_commit = self
                            .storage
                            .top_to_mono_map
                            .get(&TopRepoCommitId::new(reset.from))
                            .with_context(|| {
                                format!(
                                    "Failed to reset {} to not yet expanded toprepo revision {}",
                                    reset.branch, reset.from
                                )
                            })?;
                        self.fast_importer.write_reset(
                            output_branch.as_ref(),
                            &self.get_import_commit_ref(mono_commit),
                        )?;
                    }
                }
            }
            Ok(())
        })()
    }

    pub fn wait(self) -> Result<()> {
        // Record the new mono commit ids.
        let commit_ids = self.fast_importer.wait()?;
        for (mark, mono_commit) in self.imported_commits.values() {
            let mono_commit_id = MonoRepoCommitId::new(commit_ids[*mark - 1]);
            self.storage
                .monorepo_commits
                .insert(mono_commit_id.clone(), mono_commit.clone());
            self.storage
                .monorepo_commit_ids
                .insert(RcKey::new(mono_commit), mono_commit_id);
        }
        Ok(())
    }

    /// Gets a `:mark` marker reference or the full commit id for a commit.
    fn get_import_commit_ref(&self, mono_commit: &Rc<MonoRepoCommit>) -> ImportCommitRef {
        let key = RcKey::new(mono_commit);
        if let Some((mark, _)) = self.imported_commits.get(&key) {
            ImportCommitRef::Mark(*mark)
        } else {
            let commit_id = self
                .storage
                .monorepo_commit_ids
                .get(&key)
                .expect("existing mono commits have commit id");
            ImportCommitRef::CommitId(*commit_id.deref())
        }
    }

    fn expand_toprepo_commit(
        &mut self,
        branch: &FullNameRef,
        commit: FastExportCommit,
    ) -> Result<()> {
        let commit_id = TopRepoCommitId::new(commit.original_id);
        let top_storage = self.storage.repos.get(&RepoName::Top).unwrap();
        let top_commit = top_storage
            .thin_commits
            .get(commit_id.deref())
            .unwrap()
            .clone();
        let mut mono_parents_of_top = top_commit
            .parents
            .iter()
            .map(|parent| {
                self.storage
                    .top_to_mono_map
                    .get(&TopRepoCommitId::new(parent.commit_id))
                    .unwrap()
                    .clone()
            })
            .collect_vec();
        const TOP_PATH: GitPath = GitPath::new(BString::new(vec![]));
        let parents_for_submodules =
            self.expand_inner_submodules(branch, &mono_parents_of_top, &TOP_PATH, &top_commit)?;
        if mono_parents_of_top.is_empty() && !parents_for_submodules.is_empty() {
            // There should be a first parent that is not a submodule.
            // Add an initial empty commit.
            mono_parents_of_top.push(self.emit_mono_commit_with_tree_updates(
                branch,
                &top_commit,
                vec![],
                vec![],
                None,
                HashMap::new(),
                Some(BString::from(b"Initial empty commit")),
            )?);
        }
        let mono_parents = mono_parents_of_top
            .into_iter()
            .map(MonoRepoParent::Mono)
            .chain(parents_for_submodules)
            .collect_vec();
        let mono_commit = self.emit_mono_commit(
            branch,
            &TOP_PATH,
            &RepoName::Top,
            &top_commit,
            mono_parents,
            commit.file_changes,
            None,
        )?;
        self.storage
            .top_to_mono_map
            .insert(commit_id, mono_commit.clone());
        Ok(())
    }

    fn get_recursive_submodule_bumps(
        &self,
        path: &GitPath,
        commit: &ThinCommit,
        submod_updates: &mut HashMap<GitPath, ExpandedOrRemovedSubmodule>,
        tree_updates: &mut Vec<(GitPath, TreeId)>,
    ) {
        for (rel_sub_path, bump) in commit.submodule_bumps.iter() {
            let abs_sub_path = path.join(rel_sub_path);
            let submod_update = match bump {
                ThinSubmodule::AddedOrModified(bump) => {
                    let expanded_submod = if let Some(submod_repo_name) = &bump.repo_name {
                        let repo_name = RepoName::SubRepo(submod_repo_name.clone());
                        if self.config.is_enabled(&repo_name) {
                            let subconfig = self
                                .config
                                .subrepos
                                .get(submod_repo_name)
                                .expect("submod name exists");
                            if subconfig.skip_expanding.contains(&bump.commit_id) {
                                ExpandedSubmodule::KeptAsSubmodule(bump.commit_id)
                            } else {
                                let submod_storage = self.storage.repos.get(&repo_name).unwrap();
                                if let Some(submod_commit) =
                                    submod_storage.thin_commits.get(&bump.commit_id)
                                {
                                    tree_updates
                                        .push((abs_sub_path.clone(), submod_commit.tree_id));
                                    self.get_recursive_submodule_bumps(
                                        &abs_sub_path,
                                        submod_commit,
                                        submod_updates,
                                        tree_updates,
                                    );
                                    // TODO: This might be a regression, but the caller
                                    // is not interested in that information anyway.
                                    ExpandedSubmodule::Expanded(SubmoduleContent {
                                        repo_name: submod_repo_name.clone(),
                                        orig_commit_id: bump.commit_id,
                                    })
                                } else {
                                    ExpandedSubmodule::CommitMissingInSubRepo(SubmoduleContent {
                                        repo_name: submod_repo_name.clone(),
                                        orig_commit_id: bump.commit_id,
                                    })
                                }
                            }
                        } else {
                            // Repository disabled by config, keep the submodule.
                            ExpandedSubmodule::KeptAsSubmodule(bump.commit_id)
                        }
                    } else {
                        ExpandedSubmodule::UnknownSubmodule(bump.commit_id)
                    };
                    ExpandedOrRemovedSubmodule::Expanded(expanded_submod)
                }
                ThinSubmodule::Removed => ExpandedOrRemovedSubmodule::Removed,
            };
            submod_updates.insert(abs_sub_path.clone(), submod_update);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_mono_commit(
        &mut self,
        branch: &FullNameRef,
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
                        SubmoduleContent {
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
            &mut submodule_bumps,
            &mut tree_updates,
        );

        // tree_updates need to be ordered to get the inner submodules replaced
        // inside the outer submodules.
        tree_updates.sort_by(|(lhs_path, _), (rhs_path, _)| lhs_path.cmp(rhs_path));
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
            branch,
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
        branch: &FullNameRef,
        source_commit: &ThinCommit,
        parents: Vec<MonoRepoParent>,
        file_changes: Vec<ChangedFile>,
        top_bump: Option<TopRepoCommitId>,
        submodule_bumps: HashMap<GitPath, ExpandedOrRemovedSubmodule>,
        message: Option<BString>,
    ) -> Result<Rc<MonoRepoCommit>> {
        // TODO: Use references instead of cloning.
        let source_gix_commit = self.gix_repo.find_commit(source_commit.commit_id)?;
        let source_gix_commit = source_gix_commit.decode()?;
        let mut author = Vec::new();
        source_gix_commit.author.write_to(&mut author)?;
        let mut committer = Vec::new();
        source_gix_commit.committer.write_to(&mut committer)?;
        let message = message.unwrap_or_else(|| {
            calculate_mono_commit_message(source_gix_commit.message, &submodule_bumps)
        });
        let importer_mark = self.fast_importer.write_commit(&FastImportCommit {
            branch,
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
            (importer_mark, mono_commit.clone()),
        );
        Ok(mono_commit)
    }

    fn expand_inner_submodules(
        &mut self,
        branch: &FullNameRef,
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
                                ThinSubmodule::AddedOrModified(ThinSubmoduleContent {
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
                    let expanded_submod = if let Some(submod_repo_name) = &submod.repo_name {
                        let submod_commit_id = submod.commit_id;
                        let submod_content = SubmoduleContent {
                            repo_name: submod_repo_name.clone(),
                            orig_commit_id: submod.commit_id,
                        };
                        // The submodule is known.
                        if !self
                            .config
                            .subrepos
                            .get(submod_repo_name)
                            .is_none_or(|repo_config| repo_config.enabled)
                        {
                            // Repository disabled by config, skipping to keep the submodule.
                            // No need to log a warning because it is part of the user configuration.
                            ExpandedSubmodule::KeptAsSubmodule(submod_commit_id)
                        } else if let Some(submod_storage) = self
                            .storage
                            .repos
                            .get(&RepoName::SubRepo(submod_repo_name.clone()))
                        {
                            if let Some(submod_commit) =
                                submod_storage.thin_commits.get(&submod_commit_id)
                            {
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
                                if non_descendants.is_empty() {
                                    extra_parents_due_to_submods.extend(
                                        self.expand_parents_of_submodule(
                                            branch,
                                            mono_parents,
                                            abs_super_path,
                                            rel_sub_path,
                                            submod_repo_name,
                                            &submod_commit,
                                        )?,
                                    );
                                    ExpandedSubmodule::Expanded(submod_content)
                                } else {
                                    // Not descendant.
                                    let regressing_parent_vec = regressing_commit
                                        .take()
                                        .map(|c: Rc<MonoRepoCommit>| vec![c.clone()]);
                                    let mono_commit = self
                                        .expand_parent_for_regressing_submodule_bump(
                                            branch,
                                            regressing_parent_vec.as_ref().unwrap_or(mono_parents),
                                            &RepoName::SubRepo(submod_repo_name.clone()),
                                            &abs_sub_path,
                                            &submod_commit,
                                            non_descendants,
                                        )?;
                                    regressing_commit.replace(mono_commit);
                                    ExpandedSubmodule::RegressedNotFullyImplemented(submod_content)
                                }
                            } else {
                                // TODO: Save the log of missing commits for exporting to an autogenerated git-toprepo config.
                                // TODO: Should this print be added or will that just be duplicated information from the loading phase?
                                // self.logger.warning(format!(
                                //     "Commit {submod_commit_id} is missing in {}",
                                //     submod_repo_name
                                // ));
                                ExpandedSubmodule::CommitMissingInSubRepo(submod_content)
                            }
                        } else {
                            // No commits loaded for the submodule.
                            ExpandedSubmodule::CommitMissingInSubRepo(submod_content)
                        }
                    } else {
                        // A warning has already been logged when loading the
                        // super commit.
                        ExpandedSubmodule::UnknownSubmodule(submod.commit_id)
                    };
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

    fn expand_parents_of_submodule(
        &mut self,
        branch: &FullNameRef,
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
                branch,
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
                path: abs_sub_path.clone(),
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
        branch: &FullNameRef,
        mono_parents: &[Rc<MonoRepoCommit>],
        repo_name: &RepoName,
        abs_sub_path: &GitPath,
        submod_commit: &ThinCommit,
        mut non_descendants: Vec<CommitId>,
    ) -> Result<Rc<MonoRepoCommit>> {
        // Going backwards in history, add he original submod_commit as parent instead.
        // TODO: Make it configurable per commit what to do.
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
            branch,
            abs_sub_path,
            repo_name,
            submod_commit,
            regressing_mono_parents,
            file_changes,
            Some(commit_message),
        )?;
        // TODO: What to record in extra_mono_commit.submodule_bumps[abs_sub_path]?
        Ok(extra_mono_commit)
    }

    pub fn inject_submodule_commit(
        &mut self,
        branch: &FullNameRef,
        possible_mono_parents: Vec<Rc<MonoRepoCommit>>,
        abs_sub_path: &GitPath,
        wanted_sub_repo_name: &SubRepoName,
        wanted_sub_commit: &Rc<ThinCommit>,
    ) -> Result<Option<Rc<MonoRepoCommit>>> {
        let mut sub_to_mono_commit = HashMap::new();
        self.inject_submodule_commit_impl(
            branch,
            possible_mono_parents,
            abs_sub_path,
            wanted_sub_repo_name,
            wanted_sub_commit,
            &mut sub_to_mono_commit,
        )
    }

    fn inject_submodule_commit_memo(
        &mut self,
        branch: &FullNameRef,
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
            branch,
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
        branch: &FullNameRef,
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
                    .config
                    .subrepos
                    .get(&submod.repo_name)
                    .is_none_or(|repo_config| repo_config.enabled)
                {
                    // Repository disabled by config, skipping to keep the submodule.
                    // No need to log a warning because it is part of the user configuration.
                    continue;
                }
                if let Some(submod_storage) = self
                    .storage
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
                branch,
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
                    path: abs_sub_path.clone(),
                    commit_id: wanted_sub_parent.commit_id,
                }));
            }
        }
        if !some_parent_found {
            return Ok(None);
        }

        let parents_for_submodules = self.expand_inner_submodules(
            branch,
            &expanded_parents,
            abs_sub_path,
            wanted_sub_commit,
        )?;
        all_parents.extend(parents_for_submodules);
        // TODO: Can this code be cleaner in some way?
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
            branch,
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
            if let Some(submod_content) = submod.get_known_submod()
                && submod_content.repo_name == *submod_repo_name
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
        // TODO: Is caching needed?
        loop {
            if let Some(top_bump) = &mono_commit.top_bump {
                return Some(top_bump.clone());
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
            last_bumps: LruCache::new(std::num::NonZeroUsize::new(10000).unwrap()),
        }
    }
}

/// Construct a commit message from the submodule updates.
///
/// # Examples
/// ```
/// use git_toprepo::expander::calculate_mono_commit_message;
/// use git_toprepo::git::CommitId;
/// use git_toprepo::git::GitPath;
/// use git_toprepo::repo::ExpandedOrRemovedSubmodule;
/// use git_toprepo::repo::ExpandedSubmodule;
///
/// use bstr::B;
/// use bstr::ByteSlice;
/// use std::collections::HashMap;
/// use std::rc::Rc;
///
/// let mut submod_updates = HashMap::new();
/// let subx_commit_id: CommitId = gix::ObjectId::from_hex(b"1234567890abcdef1234567890abcdef12345678").unwrap();
/// submod_updates.insert(
///     GitPath::new(B("subx").into()),
///     ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::KeptAsSubmodule(subx_commit_id)),
/// );
/// submod_updates.insert(
///     GitPath::new(B("suby").into()),
///     ExpandedOrRemovedSubmodule::Removed,
/// );
///
/// let toprepo_message = br#"Update git submodules
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
///   - Parent commit
///
/// * Update suby from branch 'main'
///   to def456
///   - New algo
///
///   - Another parent commit
/// "#.as_bstr();
/// let expected_message = br#"New algo
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
///   - Parent commit
///
/// * Update suby from branch 'main'
///   to def456
///   - New algo
///
///   - Another parent commit
/// ^-- subx 1234567890abcdef1234567890abcdef12345678
/// ^-- suby removed
/// "#.as_bstr();
/// assert_eq!(calculate_mono_commit_message(toprepo_message.as_bstr(), &submod_updates), expected_message);
///
/// let toprepo_message = br#"Something
/// "#.as_bstr();
/// let expected_message = br#"Something
///
/// ^-- subx 1234567890abcdef1234567890abcdef12345678
/// ^-- suby removed
/// "#.as_bstr();
/// assert_eq!(calculate_mono_commit_message(toprepo_message.as_bstr(), &submod_updates), expected_message);
///
/// let toprepo_message = b"Something".as_bstr();
/// let expected_message = br#"Something
///
/// ^-- subx 1234567890abcdef1234567890abcdef12345678
/// ^-- suby removed
/// "#.as_bstr();
/// assert_eq!(calculate_mono_commit_message(toprepo_message.as_bstr(), &submod_updates), expected_message);
///
/// let toprepo_message = br#"Update git modules
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
/// * Update suby from branch 'main'
///   to def456
///   - Other algo
/// "#.as_bstr();
/// let expected_message = br#"Update git modules
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
/// * Update suby from branch 'main'
///   to def456
///   - Other algo
/// ^-- subx 1234567890abcdef1234567890abcdef12345678
/// ^-- suby removed
/// "#.as_bstr();
/// assert_eq!(calculate_mono_commit_message(toprepo_message.as_bstr(), &submod_updates), expected_message);
/// ```
pub fn calculate_mono_commit_message(
    toprepo_message: &BStr,
    submod_updates: &HashMap<GitPath, ExpandedOrRemovedSubmodule>,
) -> BString {
    let mut message =
        if let Some(alt_message) = toprepo_message.strip_prefix(b"Update git submodules\n\n") {
            let alt_message = alt_message.as_bstr();
            let mut alt_subject = UniqueContainer::Empty;
            let mut line_idx_after_submod: usize = 0;
            for line in alt_message.lines() {
                if line.starts_with(b"* Update ") {
                    line_idx_after_submod = 0;
                } else if line_idx_after_submod == 2
                    && let Some(subject) = line.strip_prefix(B("  - "))
                {
                    alt_subject.insert(subject);
                }
                line_idx_after_submod += 1;
            }
            if let UniqueContainer::Single(alt_subject) = alt_subject {
                let mut message = BString::new(vec![]);
                message.push_str(alt_subject);
                message.push_str("\n\n");
                message.push_str(alt_message);
                message
            } else {
                toprepo_message.to_owned()
            }
        } else {
            toprepo_message.to_owned()
        };

    // Add lines referencing the original commit ids.
    if !submod_updates.is_empty() {
        if !message.ends_with(b"\n") {
            message.push(b'\n');
        }
        if message.find_byte(b'\n').unwrap() == message.len() - 1 {
            // If the message is just a single subject line, add an empty line
            // before the body.
            message.push(b'\n');
        }
        for (path, submod) in submod_updates.iter().sorted_by_key(|(path, _)| *path) {
            let status = match submod {
                ExpandedOrRemovedSubmodule::Expanded(submod) => {
                    &submod.get_orig_commit_id().to_string()
                }
                ExpandedOrRemovedSubmodule::Removed => "removed",
            };
            message
                .write_fmt(format_args!("^-- {path} {status}\n"))
                .unwrap();
        }
    }
    message
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
///     strip_ref_prefix(&make_ref("refs/namespaces/top/refs/remotes/origin/foo"), "refs/namespaces/top/").unwrap(),
///     make_ref("refs/remotes/origin/foo"),
/// );
/// assert_eq!(
///     strip_ref_prefix(&make_ref("refs/namespaces/top/HEAD"), "refs/namespaces/top/").unwrap(),
///     make_ref("HEAD"),
/// );
///
/// assert_eq!(
///     strip_ref_prefix(&make_ref("refs/namespaces/top/HEAD"), "refs/namespaces/top").unwrap_err().to_string(),
///     "A reference must be a valid tag name as well",
/// );
/// assert_eq!(
///     strip_ref_prefix(&make_ref("refs/namespaces/top/HEAD"), "refs/other").unwrap_err().to_string(),
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
