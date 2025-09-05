use crate::expander::BumpCache;
use crate::git::CommitId;
use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::git::git_command;
use crate::git_fast_export_import::ChangedFile;
use crate::git_fast_export_import::FastImportCommit;
use crate::git_fast_export_import::ImportCommitRef;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::CommandSpanExt as _;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use crate::repo::MonoRepoCommit;
use crate::repo::MonoRepoCommitId;
use crate::repo::MonoRepoParent;
use crate::repo::MonoRepoProcessor;
use crate::repo::SubmoduleContent;
use crate::repo::TopRepoCommitId;
use crate::repo_name::RepoName;
use crate::util::CommandExtension as _;
use crate::util::EMPTY_GIX_URL;
use anyhow::Context;
use anyhow::Result;
use bstr::ByteSlice as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools;
use std::collections::BTreeMap;
use std::collections::HashMap;

fn rewrite_push_message(message: &str) -> (String, Option<String>) {
    let mut filtered_message = String::with_capacity(message.len());
    let mut topic = None;
    for line in message.lines() {
        if let Some(topic_name) = line.strip_prefix("Topic: ") {
            topic = Some(topic_name.to_owned());
        } else if line.starts_with("^-- ") {
            // Ignore '^-- path/to/submod 0123...'
        } else {
            filtered_message.push_str(line);
            filtered_message.push('\n');
        }
    }
    (filtered_message, topic)
}

/// Resolves which repository to push to. Note that the push URL might not be part of the git-toprepo configuration, so `url` is used when resolving the that.
fn resolve_push_repo(
    mono_commit: &gix::Commit,
    path: GitPath,
    mut push_url: gix::Url,
    config: &mut crate::config::GitTopRepoConfig,
) -> Result<(RepoName, GitPath, GitPath, gix::Url)> {
    let mut repo_name = RepoName::Top;
    let mut repo_path = GitPath::new(b"".into());
    let mut rel_path = path;
    let mut generic_url = EMPTY_GIX_URL.clone();
    loop {
        let dot_gitmodules_path = repo_path.join(&GitPath::new(b".gitmodules".into()));
        let dot_gitmodules_bytes = match mono_commit
            .tree()?
            .lookup_entry_by_path(dot_gitmodules_path.to_path()?)?
        {
            Some(dot_gitmodules_entry) => {
                let dot_gitmodules_object = dot_gitmodules_entry.object()?;
                dot_gitmodules_object
                    .try_into_blob()
                    .with_context(|| format!("Failed to read {dot_gitmodules_path} file"))?
                    .take_data()
            }
            None => Vec::new(),
        };
        let git_modules_info = GitModulesInfo::parse_dot_gitmodules_bytes(
            &dot_gitmodules_bytes,
            dot_gitmodules_path.to_path()?.to_owned(),
        )
        .with_context(|| format!("Failed to parse {dot_gitmodules_path} file"))?;
        let Some((submod_path, sub_url)) = git_modules_info.get_containing_submodule(&rel_path)
        else {
            return Ok((repo_name, repo_path, rel_path.clone(), push_url));
        };
        // Apply one submodule level.
        rel_path = GitPath::new(
            rel_path
                .strip_prefix(submod_path.as_bytes())
                .expect("part of the submodule")
                .strip_prefix(b"/")
                .expect("part of the submodule")
                .into(),
        );
        repo_path = repo_path.join(submod_path);
        let sub_url = match sub_url {
            Ok(sub_url) => sub_url,
            Err(err) => anyhow::bail!("{err:#}"),
        };
        generic_url = generic_url.join(sub_url);
        push_url = push_url.join(sub_url);
        // Update the return value.
        let sub_repo_name = match config.get_or_insert_from_url(&generic_url)? {
            crate::config::GetOrInsertOk::Found((name, _)) => name,
            crate::config::GetOrInsertOk::Missing(_)
            | crate::config::GetOrInsertOk::MissingAgain(_) => {
                anyhow::bail!("Missing URL {generic_url} in the git-toprepo configuration");
            }
        };
        repo_name = RepoName::SubRepo(sub_repo_name);
    }
}

fn split_for_push(
    gix_repo: &gix::ThreadSafeRepository,
    progress: &indicatif::MultiProgress,
    top_repo_cache: &crate::repo::TopRepoCache,
    config: &mut crate::config::GitTopRepoConfig,
    fast_importer: &mut crate::git_fast_export_import_dedup::FastImportRepoDedup<'_>,
    top_push_url: &gix::Url,
    export_refs_args: &[std::ffi::OsString],
) -> Result<Vec<PushMetadata>> {
    let monorepo_commits = &top_repo_cache.monorepo_commits;
    let repo = gix_repo.to_thread_local();

    let pb = progress.add(
        indicatif::ProgressBar::no_length()
            .with_style(
                indicatif::ProgressStyle::default_spinner()
                    .template("{elapsed:>4} {msg} {pos}")
                    .unwrap(),
            )
            .with_message("Splitting commits"),
    );
    let fast_exporter = crate::git_fast_export_import::FastExportRepo::load_from_path(
        gix_repo.git_dir(),
        Some(export_refs_args),
    )?;

    let mut to_push_metadata = Vec::new();
    let mut bumps = BumpCache::default();
    let mut imported_mono_commits = HashMap::new();
    let mut imported_submod_commits = HashMap::new();
    for entry in fast_exporter {
        let entry = entry?; // TODO: error handling
        match entry {
            crate::git_fast_export_import::FastExportEntry::Commit(exported_mono_commit) => {
                // TODO: Should we check if exported_mono_commit.original_id exists in the top_repo_cache?
                let mono_commit_id = MonoRepoCommitId::new(exported_mono_commit.original_id);
                let gix_mono_commit = repo.find_commit(*mono_commit_id)?;
                let mono_parents = exported_mono_commit
                    .parents
                    .iter()
                    .map(|parent_id| {
                        let mono_parent = monorepo_commits
                            .get(&MonoRepoCommitId::new(*parent_id))
                            // Fallback to the newly imported commits.
                            .or_else(|| imported_mono_commits.get(parent_id))
                            .cloned()
                            .with_context(|| {
                                format!("Unknown mono commit parent {}", parent_id.to_hex())
                            })?;
                        Ok(mono_parent)
                    })
                    .collect::<Result<Vec<_>>>()?;
                if exported_mono_commit.file_changes.is_empty() {
                    // Unknown which repository to push to if there are no file changes at all.
                    anyhow::bail!("Pushing empty commits like {mono_commit_id} is not supported");
                }
                // The user should make sure that the .gitmodules is
                // correct. Note that inner submodules might be
                // mentioned, but there should not be any submodule
                // mentioned that is a valid path in the repository.
                // TODO: Handle updated URLs in the .gitmodules file.
                // TODO: How to handle added and removed submodules from the .gitmodules file?
                let mut grouped_file_changes: BTreeMap<(GitPath, RepoName, gix::Url), Vec<_>> =
                    BTreeMap::new();
                for fc in exported_mono_commit.file_changes {
                    let (repo_name, submod_path, rel_path, push_url) = resolve_push_repo(
                        &gix_mono_commit,
                        GitPath::new(fc.path),
                        top_push_url.clone(),
                        config,
                    )?;
                    grouped_file_changes
                        .entry((submod_path, repo_name, push_url))
                        .or_default()
                        .push(ChangedFile {
                            path: (*rel_path).clone(),
                            change: fc.change,
                        });
                }
                let (message, topic) = rewrite_push_message(exported_mono_commit.message.to_str()?);
                if grouped_file_changes.len() > 1 && topic.is_none() {
                    anyhow::bail!(
                        "Multiple submodules changed in commit {mono_commit_id}, but no topic was provided. \
                            Please amend the commit message to add a 'Topic: something-descriptive' line."
                    );
                }
                for ((abs_sub_path, repo_name, push_url), file_changes) in grouped_file_changes {
                    let push_branch = format!("{}push", repo_name.to_ref_prefix());
                    let parents_commit_ids = mono_parents
                        .iter()
                        .filter_map(|mono_parent| match &repo_name {
                            RepoName::Top => {
                                bumps.get_top_bump(mono_parent).map(|top_bump| *top_bump)
                            }
                            RepoName::SubRepo(sub_repo_name) => bumps
                                .get_some_submodule(mono_parent, &abs_sub_path, sub_repo_name)
                                .map(|parent_submod| *parent_submod.get_orig_commit_id()),
                        })
                        .unique()
                        .collect_vec();
                    let parents = parents_commit_ids
                        .iter()
                        .map(|parent_submod_id| {
                            imported_submod_commits
                                .get(parent_submod_id)
                                .cloned()
                                .unwrap_or(ImportCommitRef::CommitId(*parent_submod_id))
                        })
                        .collect_vec();
                    if parents.is_empty() {
                        match repo_name {
                            RepoName::Top => anyhow::bail!(
                                "Mono commit {mono_commit_id} has no parents with content outside of the submodules, which is impossible"
                            ),
                            RepoName::SubRepo(sub_repo_name) => anyhow::bail!(
                                "Submodule {sub_repo_name} at {abs_sub_path} does not exist as a git-link in any parent of {mono_commit_id}"
                            ),
                        }
                    }
                    let import_ref = fast_importer.write_commit(&FastImportCommit {
                        branch: <&FullNameRef as TryFrom<_>>::try_from(&push_branch)
                            .expect("valid ref name"),
                        author_info: exported_mono_commit.author_info.clone(),
                        committer_info: exported_mono_commit.committer_info.clone(),
                        encoding: exported_mono_commit.encoding.clone(),
                        message: bstr::BString::from(message.clone()),
                        file_changes,
                        parents,
                        original_id: None,
                    })?;
                    let import_commit_id = fast_importer.get_object_id(&import_ref)?;
                    imported_submod_commits.insert(import_commit_id, import_ref);

                    let (top_bump, submodule_bumps) = match repo_name {
                        RepoName::Top => {
                            (Some(TopRepoCommitId::new(import_commit_id)), HashMap::new())
                        }
                        RepoName::SubRepo(sub_repo_name) => (
                            None,
                            HashMap::from([(
                                abs_sub_path,
                                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::Expanded(
                                    SubmoduleContent {
                                        repo_name: sub_repo_name,
                                        orig_commit_id: import_commit_id,
                                    },
                                )),
                            )]),
                        ),
                    };
                    let mono_commit = MonoRepoCommit::new_rc(
                        mono_parents
                            .iter()
                            .map(|mono_parent| MonoRepoParent::Mono(mono_parent.clone()))
                            .collect(),
                        top_bump,
                        submodule_bumps,
                    );
                    imported_mono_commits
                        .insert(exported_mono_commit.original_id, mono_commit.clone());
                    to_push_metadata.push(PushMetadata {
                        push_url,
                        topic: topic.clone(),
                        commit_id: import_commit_id,
                        parents: parents_commit_ids,
                    });
                }
                pb.inc(1);
            }
            crate::git_fast_export_import::FastExportEntry::Reset(reset) => {
                log::warn!(
                    "Resetting {} to {} is unimplemented",
                    reset.branch,
                    reset.from
                );
            }
        };
    }
    Ok(to_push_metadata)
}

pub struct PushProcessor<'a>(&'a mut MonoRepoProcessor);

impl<'a> PushProcessor<'a> {
    pub fn new(processor: &'a mut MonoRepoProcessor) -> Self {
        Self(processor)
    }

    pub fn push(
        &mut self,
        top_push_url: &gix::Url,
        local_rev_or_ref: &String,
        remote_ref: &FullName,
        dry_run: bool,
    ) -> Result<()> {
        if self.0.top_repo_cache.monorepo_commits.is_empty() {
            anyhow::bail!(
                "No filtered mono commits exists, please run `git toprepo refilter` first"
            );
        }
        let repo = self.0.gix_repo.to_thread_local();

        let local_rev = repo.rev_parse_single(local_rev_or_ref.as_bytes())?;
        let local_rev_arg: std::ffi::OsString = local_rev.to_hex().to_string().into();
        let export_refs_args: Vec<std::ffi::OsString> = repo
            .references()?
            .prefixed(b"refs/remotes/origin/".as_bstr())?
            .map(|r| {
                let r = match r {
                    Ok(r) => r,
                    Err(err) => anyhow::bail!("{err:#}"),
                }
                .detach();
                match bstr::concat([b"^".as_bstr(), r.name.as_bstr()]).to_os_str() {
                    Ok(arg) => Ok(arg.to_owned()),
                    Err(err) => anyhow::bail!("{err:#}"),
                }
            })
            .chain([Ok(local_rev_arg)])
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| "Failed while iterating refs/remotes/origin/")?;

        let mut dedup_cache = std::mem::take(&mut self.0.top_repo_cache.dedup);
        let mut fast_importer = crate::git_fast_export_import_dedup::FastImportRepoDedup::new(
            crate::git_fast_export_import::FastImportRepo::new(self.0.gix_repo.git_dir())?,
            &mut dedup_cache,
        );
        let to_push_metadata = split_for_push(
            &self.0.gix_repo,
            &self.0.progress,
            &self.0.top_repo_cache,
            &mut self.0.config,
            &mut fast_importer,
            top_push_url,
            &export_refs_args,
        );
        // Make sure to gracefully shutdown the fast-importer before returning.
        fast_importer.wait()?;
        self.0.top_repo_cache.dedup = dedup_cache;
        let mut to_push_metadata = to_push_metadata?;

        // Group the pushes together to run fewer git-push commands.
        to_push_metadata.reverse();
        let mut redundant_pushes = HashMap::new();
        to_push_metadata.retain(|push_info| {
            let is_needed = redundant_pushes
                .remove(&(push_info.push_url.clone(), push_info.commit_id))
                .as_ref()
                != Some(&push_info.topic);
            for parent in &push_info.parents {
                // Even if the entry exists, it should be replaced to show that
                // the first push after `*parent` will be with `topic`. Later
                // pushes will not affect anything anyway.
                redundant_pushes.insert(
                    (push_info.push_url.clone(), *parent),
                    push_info.topic.clone(),
                );
            }
            is_needed
        });
        to_push_metadata.reverse();
        if to_push_metadata.is_empty() {
            log::info!("Nothing to push");
            return Ok(());
        }

        let info_label = if dry_run { "Would run" } else { "Running" };
        let mut failed_pushes = 0;
        for push_info in to_push_metadata {
            let topic_arg = match &push_info.topic {
                Some(topic) => format!(" -o topic={topic}"),
                None => String::new(),
            };
            log::info!(
                "{info_label}: git push {}{topic_arg} {}:{remote_ref}",
                push_info.push_url,
                push_info.commit_id.to_hex()
            );
            if dry_run {
                continue;
            }
            // Do the push.
            let mut cmd = git_command(self.0.gix_repo.git_dir());
            cmd.arg("push")
                .arg(push_info.push_url.to_bstring().to_os_str()?);
            if let Some(topic) = &push_info.topic {
                cmd.arg("-o").arg(format!("topic={topic}"));
            }
            cmd.arg(format!("{}:{remote_ref}", push_info.commit_id));
            if let Err(err) = cmd
                .trace_command(crate::command_span!("git push"))
                .safe_status()?
                .check_success()
            {
                log::info!(
                    "Failed to git push {} {}:{remote_ref}: {err:#}",
                    push_info.push_url,
                    push_info.commit_id
                );
                failed_pushes += 1;
            }
        }
        if failed_pushes != 0 {
            let times_string = if failed_pushes == 1 { "time" } else { "times" };
            anyhow::bail!(format!("git-push failed {failed_pushes} {times_string}"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PushMetadata {
    pub push_url: gix::Url,
    pub topic: Option<String>,
    pub commit_id: CommitId,
    pub parents: Vec<CommitId>,
}
