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
use crate::log::InterruptedError;
use crate::log::InterruptedResult;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use crate::repo::MonoRepoCommit;
use crate::repo::MonoRepoCommitId;
use crate::repo::MonoRepoParent;
use crate::repo::MonoRepoProcessor;
use crate::repo::SubmoduleContent;
use crate::repo::TopRepoCommitId;
use crate::repo_name::RepoName;
use crate::ui::ProgressStatus;
use crate::ui::ProgressTaskHandle;
use crate::util::EMPTY_GIX_URL;
use crate::util::SafeExitStatus;
use anyhow::Context;
use anyhow::Result;
use bstr::ByteSlice as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use itertools::Itertools;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;

/// Splits the mono-repo commits that needs to be pushed into submodule commits
/// and top-repo commits, so that each commit is pushed to its underlying
/// repositories.
pub fn split_for_push(
    processor: &mut MonoRepoProcessor,
    top_push_url: &gix::Url,
    local_rev_or_ref: &String,
) -> Result<Vec<PushMetadata>> {
    if processor.top_repo_cache.monorepo_commits.is_empty() {
        anyhow::bail!("No filtered mono commits exists, please run `git toprepo refilter` first");
    }

    let local_rev = processor
        .gix_repo
        .rev_parse_single(local_rev_or_ref.as_bytes())?;
    let local_rev_arg: std::ffi::OsString = local_rev.to_hex().to_string().into();
    let export_refs_args: Vec<std::ffi::OsString> = processor
        .gix_repo
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

    let mut dedup_cache = std::mem::take(&mut processor.top_repo_cache.dedup);
    let mut fast_importer = crate::git_fast_export_import_dedup::FastImportRepoDedup::new(
        crate::git_fast_export_import::FastImportRepo::new(processor.gix_repo.git_dir())?,
        &mut dedup_cache,
    );
    let to_push_metadata = split_for_push_impl(
        processor,
        &mut fast_importer,
        top_push_url,
        &export_refs_args,
    );
    // Make sure to gracefully shutdown the fast-importer before returning.
    fast_importer.wait()?;
    processor.top_repo_cache.dedup = dedup_cache;

    let mut to_push_metadata = to_push_metadata?;
    if to_push_metadata.is_empty() {
        // Everything exists upstream. Add a dummy entry of the toprepo to
        // actually push something, e.g. if creating a new branch.
        let top_commit_id = processor.top_repo_cache.monorepo_commits.get(&MonoRepoCommitId::new(local_rev.detach()))
        .and_then(|mono_commit| mono_commit.top_bump)
        .with_context(|| format!("All commits to push exist upstream, yet the mono commit {local_rev_or_ref} has not been assembled from upstream data. Please rerun `git toprepo refilter`"))?;

        to_push_metadata.push(PushMetadata {
            repo_name: RepoName::Top,
            push_url: top_push_url.clone(),
            topic: None,
            commit_id: top_commit_id.into_inner(),
            parents: Vec::new(), // Nothing else to push.
        });
    }
    Ok(to_push_metadata)
}

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
    // If the original message was "Subject\n\nTopic: something\n", the double
    // LF should be removed.
    while filtered_message.ends_with("\n\n") {
        filtered_message.pop();
    }
    (filtered_message, topic)
}

/// Resolves which repository to push to. Note that the push URL might not be
/// part of the git-toprepo configuration, so `push_url` is used as base when
/// resolving the destinations.
fn resolve_push_repo(
    mono_commit: &gix::Commit,
    path: GitPath,
    mut push_url: gix::Url,
    config: &mut crate::config::GitTopRepoConfig,
) -> Result<(RepoName, GitPath, GitPath, gix::Url)> {
    let mut repo_name = RepoName::Top;
    let mut repo_path = GitPath::from("");
    let mut rel_path = path;
    let mut generic_url = EMPTY_GIX_URL.clone();
    loop {
        let dot_gitmodules_path = repo_path.join(&GitPath::from(".gitmodules"));
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
        rel_path = GitPath::from(
            rel_path
                .strip_prefix(submod_path.as_bytes())
                .expect("part of the submodule")
                .strip_prefix(b"/")
                .expect("part of the submodule"),
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

fn split_for_push_impl(
    processor: &mut MonoRepoProcessor,
    fast_importer: &mut crate::git_fast_export_import_dedup::FastImportRepoDedup<'_>,
    top_push_url: &gix::Url,
    export_refs_args: &[std::ffi::OsString],
) -> Result<Vec<PushMetadata>> {
    let monorepo_commits = &processor.top_repo_cache.monorepo_commits;

    let pb = processor.progress.add(
        indicatif::ProgressBar::no_length()
            .with_style(
                indicatif::ProgressStyle::default_spinner()
                    .template("{elapsed:>4} {msg} {pos}")
                    .unwrap(),
            )
            .with_message("Splitting commits"),
    );
    let fast_exporter = crate::git_fast_export_import::FastExportRepo::load_from_path(
        processor.gix_repo.git_dir(),
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
                let gix_mono_commit = processor.gix_repo.find_commit(*mono_commit_id)?;
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
                        processor.config,
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

                    let (top_bump, submodule_bumps) = match &repo_name {
                        RepoName::Top => {
                            (Some(TopRepoCommitId::new(import_commit_id)), HashMap::new())
                        }
                        RepoName::SubRepo(sub_repo_name) => (
                            None,
                            HashMap::from([(
                                abs_sub_path,
                                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::Expanded(
                                    SubmoduleContent {
                                        repo_name: sub_repo_name.clone(),
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
                        repo_name,
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

#[derive(Debug, Clone)]
pub struct PushMetadata {
    pub repo_name: RepoName,
    pub push_url: gix::Url,
    pub topic: Option<String>,
    pub commit_id: CommitId,
    pub parents: Vec<CommitId>,
}

impl PushMetadata {
    /// Returns extra parameters for the git-push command.
    pub fn extra_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(topic) = &self.topic {
            args.push("-o".to_owned());
            args.push(format!("topic={topic}"));
        }
        args
    }
}

pub struct CommitPusher {
    context: PushContext,
    thread_count: NonZeroUsize,
}

impl CommitPusher {
    pub fn new(
        toprepo: gix::Repository,
        progress: indicatif::MultiProgress,
        error_observer: crate::log::ErrorObserver,
        thread_count: NonZeroUsize,
    ) -> Self {
        let summary_pb = indicatif::ProgressBar::no_length()
            .with_style(
                indicatif::ProgressStyle::with_template(
                    "{elapsed:>4} {prefix:.cyan} [{bar:24}] {pos}/{len}{wide_msg}",
                )
                .unwrap()
                .progress_chars("=> "),
            )
            .with_prefix("Pushing");
        // Make sure that the elapsed time is updated continuously.
        summary_pb.enable_steady_tick(std::time::Duration::from_millis(1000));
        let push_progress = ProgressStatus::new(progress.clone(), progress.add(summary_pb));
        Self {
            context: PushContext {
                toprepo,
                push_progress,
                error_observer: Arc::new(Mutex::new(error_observer)),
            },
            thread_count,
        }
    }

    pub fn push(
        &self,
        push_metadata: Vec<PushMetadata>,
        remote_ref: &FullName,
        extra_args: &[String],
        dry_run: bool,
    ) -> Result<()> {
        if push_metadata.is_empty() {
            log::info!("Nothing to push");
            return Ok(());
        }
        self.push_to_remote_parallel(push_metadata, remote_ref, extra_args, dry_run);
        self.context
            .error_observer
            .lock()
            .unwrap()
            .get_result(())
            .context("Some git-push commands failed, see the logs for details")
    }

    fn push_to_remote_parallel(
        &self,
        push_metadata: Vec<PushMetadata>,
        remote_ref: &FullName,
        extra_args: &[String],
        dry_run: bool,
    ) {
        let splitted_metadata = push_metadata
            .into_iter()
            .into_group_map_by(|info| info.push_url.clone());
        let thread_pool = threadpool::ThreadPool::new(std::cmp::min(
            self.thread_count.get(),
            splitted_metadata.len(),
        ));
        // Make push order deterministic.
        let mut sorted_metadata = splitted_metadata.into_iter().collect::<Vec<_>>();
        sorted_metadata.sort_by_key(|(push_url, _)| push_url.clone());
        for (_push_url, url_push_metadata) in sorted_metadata {
            let mut task = PushTask::new(self.context.clone(), url_push_metadata);
            let remote_ref = remote_ref.clone();
            let extra_args = extra_args.to_vec();
            let error_observer = self.context.error_observer.clone();
            thread_pool.execute(move || {
                let result = task.push_all(&remote_ref, extra_args, dry_run);
                error_observer.lock().unwrap().consume_interrupted(result);
            });
        }
        thread_pool.join();
    }
}

#[derive(Clone)]
struct PushContext {
    toprepo: gix::Repository,

    push_progress: ProgressStatus,
    /// Signal to not start new work but to fail as fast as possible.
    error_observer: Arc<Mutex<crate::log::ErrorObserver>>,
}

struct PushTask {
    context: PushContext,
    /// Stuff to push in reverse order, i.e. the last item is pushed first by
    /// using `reversed_push_metadata.pop()`.
    reversed_push_metadata: Vec<PushMetadata>,

    /// The currently active PushMetadata, if any.
    current_push_item: Option<CurrentPushItem>,
}

struct CurrentPushItem {
    repo_name: RepoName,
    pb_url: indicatif::ProgressBar,
    pb_status: indicatif::ProgressBar,

    /// Keep a handle to the progress task to keep it visible in the UI.
    #[allow(unused)]
    progress_task: ProgressTaskHandle,
}

impl PushTask {
    pub fn new(context: PushContext, mut push_metadata: Vec<PushMetadata>) -> Self {
        context
            .push_progress
            .inc_queue_size(push_metadata.len() as isize);
        push_metadata.reverse();
        Self {
            context,
            reversed_push_metadata: push_metadata,
            current_push_item: None,
        }
    }

    pub fn push_all(
        &mut self,
        remote_ref: &FullName,
        extra_args: Vec<String>,
        dry_run: bool,
    ) -> InterruptedResult<()> {
        while let Some(push_info) = self.reversed_push_metadata.pop() {
            self.context.push_progress.inc_queue_size(-1);
            if self
                .context
                .error_observer
                .lock()
                .unwrap()
                .should_interrupt()
            {
                return Err(InterruptedError::Interrupted);
            }
            self.push_one(push_info, remote_ref, extra_args.clone(), dry_run)?;
        }
        Ok(())
    }

    fn push_one(
        &mut self,
        push_info: PushMetadata,
        remote_ref: &FullName,
        mut extra_args: Vec<String>,
        dry_run: bool,
    ) -> Result<()> {
        // Update progress bars.
        if self
            .current_push_item
            .as_ref()
            .is_none_or(|item| item.repo_name != push_info.repo_name)
        {
            // Recreate the progress bar as this push is to a different repository.
            self.current_push_item = None;
            let pb_url = indicatif::ProgressBar::hidden()
                .with_style(
                    indicatif::ProgressStyle::with_template("     {prefix:.cyan} {msg}").unwrap(),
                )
                .with_prefix("git push");
            let pb_status = indicatif::ProgressBar::hidden()
                .with_style(indicatif::ProgressStyle::with_template("     {msg}").unwrap());
            let progress_task = self.context.push_progress.start(
                push_info.repo_name.to_string(),
                vec![pb_url.clone(), pb_status.clone()],
            );
            self.current_push_item = Some(CurrentPushItem {
                repo_name: push_info.repo_name.clone(),
                pb_url,
                pb_status,
                progress_task,
            });
        }
        let current_push_item = self.current_push_item.as_ref().expect("just set above");
        current_push_item
            .pb_url
            .set_message(push_info.push_url.to_string());
        // Log.
        extra_args.extend(push_info.extra_args());
        let log_command = format!(
            "git push {}{} {}:{remote_ref}",
            push_info.push_url,
            extra_args.iter().map(|arg| format!(" {arg}")).join(""),
            push_info.commit_id.to_hex()
        );
        log::info!(
            "{} {log_command}",
            if dry_run { "Would run" } else { "Running" },
        );
        // Run the command.
        if !dry_run {
            let (mut proc, _span_guard) = git_command(self.context.toprepo.git_dir())
                .arg("push")
                .arg(push_info.push_url.to_bstring().to_os_str()?)
                .args(&extra_args)
                .arg(format!("{}:{remote_ref}", push_info.commit_id))
                // TODO: Collect stdout (use a thread to avoid backpressure deadlock).
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .trace_command(crate::command_span!("git push"))
                .spawn()
                .with_context(|| "Failed to spawn git-push".to_string())?;
            let permanent_stderr = crate::util::read_stderr_progress_status(
                proc.stderr.take().expect("piping stderr"),
                |line| current_push_item.pb_status.set_message(line),
            );
            let trimmed_stderr = permanent_stderr.trim_end();
            let push_result = (|| {
                let status =
                    SafeExitStatus::new(proc.wait().context("Failed to wait for git-push")?);
                if status.success() {
                    return Ok(());
                }
                if status.code() == Some(1) {
                    // Check for Gerrit 'no new changes' rejection which is not any problem.
                    for line in permanent_stderr.lines() {
                        if line.starts_with(" ! [remote rejected] ")
                            && line.ends_with(" (no new changes)")
                        {
                            // Nothing to worry about.
                            log::debug!(
                                "During {log_command}: Ignoring 'no new changes' rejection"
                            );
                            return Ok(());
                        }
                    }
                }
                // Print stderr in the error message as well.
                let maybe_newline = if trimmed_stderr.is_empty() { "" } else { "\n" };
                anyhow::bail!("Failed to {log_command}: {status:#}{maybe_newline}{trimmed_stderr}");
            })()
            .map(|_| {
                if !trimmed_stderr.is_empty() {
                    log::info!("Stderr from {log_command}\n{trimmed_stderr}");
                }
            });
            self.context
                .error_observer
                .lock()
                .unwrap()
                .maybe_consume(push_result)?;
        }
        Ok(())
    }
}

impl Drop for PushTask {
    fn drop(&mut self) {
        self.context
            .push_progress
            .inc_queue_size(-(self.reversed_push_metadata.len() as isize));
        drop(self.current_push_item.take());
    }
}
