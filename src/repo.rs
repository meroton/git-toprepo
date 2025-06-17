use crate::expander::BumpCache;
use crate::expander::TopRepoExpander;
use crate::expander::strip_ref_prefix;
use crate::git::BlobId;
use crate::git::CommitId;
use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git::git_command;
use crate::git::git_global_command;
use crate::git_fast_export_import::ChangedFile;
use crate::git_fast_export_import::FastImportCommit;
use crate::git_fast_export_import::ImportCommitRef;
use crate::git_fast_export_import::WithoutCommitterId;
use crate::git_fast_export_import_dedup::GitFastExportImportDedupCache;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::Logger;
use crate::repo_name::RepoName;
use crate::repo_name::SubRepoName;
use crate::util::CommandExtension as _;
use crate::util::EMPTY_GIX_URL;
use crate::util::RcKey;
use anyhow::Context;
use anyhow::Result;
use bstr::BStr;
use bstr::ByteSlice as _;
use gix::refs::FullName;
use gix::refs::FullNameRef;
use gix::refs::file::ReferenceExt;
use itertools::Itertools;
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

pub const COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY: &str =
    "Could not open toprepo, it must be a git repository";

#[derive(Debug)]
pub struct TopRepo {
    pub gix_repo: gix::ThreadSafeRepository,
}

impl TopRepo {
    pub fn create(directory: &Path, url: gix::url::Url) -> Result<TopRepo> {
        git_global_command()
            .arg("init")
            .arg("--quiet")
            .arg(directory.as_os_str())
            .safe_status()?
            .check_success()
            .context("Failed to initialize git repository")?;
        git_command(directory)
            .args([
                "config",
                "remote.origin.pushUrl",
                "https://ERROR.invalid/Please use 'git toprepo push ...' instead",
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.pushUrl")?;
        git_command(directory)
            .args(["config", "remote.origin.url", &url.to_string()])
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
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.fetch (HEAD)")?;
        git_command(directory)
            .args(["config", "remote.origin.tagOpt", "--no-tags"])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config remote.origin.tagOpt")?;
        git_command(directory)
            .args([
                "config",
                "toprepo.config",
                &format!("repo:{toprepo_ref_prefix}refs/remotes/origin/HEAD:.gittoprepo.toml"),
            ])
            .safe_status()?
            .check_success()
            .context("Failed to set git-config toprepo.config")?;

        let process = git_command(directory)
            .args(["hash-object", "-t", "blob", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let result = process.wait_with_output()?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_blob_hash = crate::util::trim_newline_suffix(result.stdout.to_str()?);

        let mut process = git_command(directory)
            .arg("mktree")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let mut stdin = process.stdin.take().expect("stdin is piped");
        stdin.write_all(
            format!("100644 blob {gittoprepotoml_blob_hash}\t.gittoprepo.toml\n").as_bytes(),
        )?;
        drop(stdin);
        let result = process.wait_with_output()?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_tree_hash =
            bstr::BStr::new(crate::util::trim_bytes_newline_suffix(&result.stdout));

        let mut process = git_command(directory)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
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
        let result = process.wait_with_output()?;
        if !result.status.success() {
            anyhow::bail!(
                "Failed to create tree for empty .gittoprepo.toml: {}",
                result.status
            );
        }
        let gittoprepotoml_commit_hash =
            bstr::BStr::new(crate::util::trim_bytes_newline_suffix(&result.stdout));

        let first_time_config_ref = toprepo_ref_prefix + "refs/remotes/origin/HEAD";
        git_command(directory)
            .arg("update-ref")
            .arg(&first_time_config_ref)
            .arg(gittoprepotoml_commit_hash.to_os_str()?)
            .safe_status()?
            .check_success()
            .with_context(|| format!("Failed to reset {first_time_config_ref}"))?;
        Self::open(directory)
    }

    pub fn open(directory: &Path) -> Result<TopRepo> {
        let gix_repo =
            gix::open(directory).context(COULD_NOT_OPEN_TOPREPO_MUST_BE_GIT_REPOSITORY)?;
        Ok(TopRepo {
            gix_repo: gix_repo.into_sync(),
        })
    }

    /// Get the main work tree path of the repository.
    pub fn work_tree(&self) -> Result<&Path> {
        self.gix_repo.work_dir().with_context(|| {
            format!(
                "Bare repository without worktree {}",
                self.gix_repo.git_dir().display()
            )
        })
    }
}

pub struct MonoRepoProcessor {
    pub gix_repo: gix::ThreadSafeRepository,
    pub config: crate::config::GitTopRepoConfig,
    pub top_repo_cache: crate::repo::TopRepoCache,
    pub error_observer: crate::log::ErrorObserver,
    pub progress: indicatif::MultiProgress,
}

impl MonoRepoProcessor {
    pub fn run<T, F>(directory: &Path, error_mode: crate::log::ErrorMode, f: F) -> Result<T>
    where
        F: FnOnce(&mut MonoRepoProcessor, &Logger) -> Result<T>,
    {
        let gix_repo = gix::open(directory)
            .context("Could not open directory for MonoRepoProcessor")?
            .into_sync();
        let config = crate::config::GitTopRepoConfig::load_config_from_repo(
            gix_repo.work_tree.as_deref().with_context(|| {
                format!(
                    "Bare repository without worktree {}",
                    gix_repo.git_dir().display()
                )
            })?,
        )?;
        let top_repo_cache = crate::repo_cache_serde::SerdeTopRepoCache::load_from_git_dir(
            gix_repo.git_dir(),
            Some(&config.checksum),
        )
        .with_context(|| format!("Loading cache from {}", gix_repo.git_dir().display()))?
        .unpack()?;
        let error_observer = crate::log::ErrorObserver::new(error_mode);
        let mut processor = MonoRepoProcessor {
            gix_repo,
            config,
            top_repo_cache,
            error_observer,
            progress: indicatif::MultiProgress::new(),
        };
        let mut result = crate::log::Logger::new_to_stderr(|logger| {
            logger.with_progress(|logger, progress| {
                let old_progress = std::mem::replace(&mut processor.progress, progress);
                let result = f(&mut processor, &logger);
                processor.progress = old_progress;
                result
            })
        });
        // Store some result files.
        if let Err(err) = crate::repo_cache_serde::SerdeTopRepoCache::pack(
            &processor.top_repo_cache,
            processor.config.checksum.clone(),
        )
        .store_to_git_dir(processor.gix_repo.git_dir())
            && result.is_ok()
        {
            result = Err(err);
        }
        const EFFECTIVE_TOPREPO_CONFIG: &str = "toprepo/last-effective-git-toprepo.toml";
        let config_path = processor.gix_repo.git_dir().join(EFFECTIVE_TOPREPO_CONFIG);
        if let Err(err) = processor.config.save(&config_path)
            && result.is_ok()
        {
            result = Err(err);
        }
        result
    }

    /// Reload the git-toprepo configuration in case anything has changed. Also
    /// check if the top repo cache is still valid given the new configuration.
    pub fn reload_config(&mut self) -> Result<()> {
        self.config = crate::config::GitTopRepoConfig::load_config_from_repo(
            self.gix_repo.work_tree.as_deref().with_context(|| {
                format!(
                    "Bare repository without worktree {}",
                    self.gix_repo.git_dir().display()
                )
            })?,
        )?;
        Ok(())
    }

    /// Reads the monorepo refs that was the result of the last refiltering.
    fn read_monorepo_refs_log(repo: &gix::Repository) -> Result<Vec<FullName>> {
        let refs_path = repo.git_dir().join("toprepo/mono-refs-ok-to-remove");
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

    /// Writes the monorepo refs that have resulted from a refilter.
    fn write_monorepo_refs_log(repo: &gix::Repository, monorepo_refs: &[FullName]) -> Result<()> {
        let refs_path = repo.git_dir().join("toprepo/mono-refs-ok-to-remove");
        if let Some(parent) = refs_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let refs_path_tmp = refs_path.with_extension("tmp");
        (|| -> Result<()> {
            let mut file = std::fs::File::create(&refs_path_tmp)?;
            for r in monorepo_refs {
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

    pub fn refilter_all_top_refs(&mut self, logger: &Logger) -> Result<()> {
        let repo = self.gix_repo.to_thread_local();
        let top_ref_prefix = format!("{}refs/", RepoName::Top.to_ref_prefix());

        let repo_refs = repo.references()?;
        let mut refs = Vec::new();
        for r in repo_refs.prefixed(BStr::new(top_ref_prefix.as_bytes()))? {
            let r = r.map_err(|err| anyhow::anyhow!("Failed while iterating refs: {err:#}"))?;
            refs.push(r);
        }
        self.refilter(refs, logger)
    }

    pub fn refilter_some_top_refspecs(
        &mut self,
        top_ref_names: impl IntoIterator<Item = impl AsRef<FullNameRef>>,
        logger: &Logger,
    ) -> Result<()> {
        let repo = self.gix_repo.to_thread_local();

        let mut refs = Vec::new();
        for top_ref in top_ref_names {
            let r = repo.find_reference(top_ref.as_ref())?;
            refs.push(r);
        }
        self.refilter(refs, logger)
    }

    pub fn refilter(&mut self, top_refs: Vec<gix::Reference<'_>>, logger: &Logger) -> Result<()> {
        let repo = self.gix_repo.to_thread_local();
        let old_monorepo_refs = Self::read_monorepo_refs_log(&repo)?;

        let top_ref_prefix = RepoName::Top.to_ref_prefix();

        let mut monorepo_symbolic_tips = Vec::new();
        let mut monorepo_object_tips = Vec::new();
        let mut todo_toprepo_object_tips = Vec::new();
        for r in top_refs {
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
            let monorepo_ref_name = strip_ref_prefix(&r.name, top_ref_prefix.as_str())
                .with_context(|| format!("Bad toprepo ref {}", r.name))?;
            match r.target {
                gix::refs::Target::Symbolic(target_name) => {
                    let monorepo_link_name = monorepo_ref_name;
                    let Ok(monorepo_target_name) =
                        strip_ref_prefix(&target_name, BStr::new(&top_ref_prefix))
                    else {
                        logger.warning(format!(
                            "Skipping symbolic ref {} that points outside the top repo, to {target_name}.",
                            r.name,
                        ));
                        continue;
                    };
                    monorepo_symbolic_tips.push((monorepo_link_name, monorepo_target_name));
                }
                gix::refs::Target::Object(object_id) => {
                    let commit_id = TopRepoCommitId(object_id);
                    if let Some(mono_commit) = self.top_repo_cache.top_to_mono_map.get(&commit_id)
                        && let Some(mono_commit_id) = self
                            .top_repo_cache
                            .monorepo_commit_ids
                            .get(&RcKey::new(mono_commit))
                    {
                        monorepo_object_tips.push((monorepo_ref_name, mono_commit_id.clone()));
                    } else {
                        todo_toprepo_object_tips.push((monorepo_ref_name, r.name, commit_id));
                    }
                }
            }
        }
        let mut final_monorefs = monorepo_object_tips
            .iter()
            .map(|(name, _)| name.clone())
            .collect_vec();
        final_monorefs.sort();
        // Mark all the old refs (already marked) and all the new refs (the user
        // have asked them to be overwritten) as okay to be removed if anything
        // fails.
        let old_and_new_monorefs = old_monorepo_refs
            .iter()
            .chain(&final_monorefs)
            .unique()
            .cloned()
            .sorted();
        Self::write_monorepo_refs_log(&repo, old_and_new_monorefs.as_slice())?;
        if !todo_toprepo_object_tips.is_empty() {
            let progress = self.progress.clone();
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
                todo_toprepo_object_tips
                    .iter()
                    .map(|(_, _, top_commit_id)| *top_commit_id.deref()),
                |commit_id| {
                    self.top_repo_cache
                        .top_to_mono_map
                        .contains_key(&TopRepoCommitId(commit_id))
                },
            )?;
            drop(pb);

            println!("Found {num_commits_to_export} commits to expand");
            let fast_importer = crate::git_fast_export_import::FastImportRepo::new(
                self.gix_repo.git_dir(),
                logger.clone(),
            )?;
            let mut expander = TopRepoExpander {
                gix_repo: &repo,
                storage: &mut self.top_repo_cache,
                config: &self.config,
                progress,
                logger: logger.clone(),
                fast_importer,
                imported_commits: HashMap::new(),
                bumps: crate::expander::BumpCache::default(),
                inject_at_oldest_super_commit: false,
            };
            expander.expand_toprepo_commits(
                &todo_toprepo_object_tips
                    .iter()
                    .map(|(_, toprepo_name, _)| toprepo_name.clone())
                    .collect_vec(),
                stop_commits,
                num_commits_to_export,
            )?;
            expander.wait()?;
            // Collect all new monorepo commits.
            for (monorepo_ref_name, _, top_commit_id) in todo_toprepo_object_tips {
                let mono_commit = self
                    .top_repo_cache
                    .top_to_mono_map
                    .get(&top_commit_id)
                    .expect("just filtered mono commit must exist");
                let mono_commit_id = self
                    .top_repo_cache
                    .monorepo_commit_ids
                    .get(&RcKey::new(mono_commit))
                    .expect("just filtered mono commit id must exist");
                monorepo_object_tips.push((monorepo_ref_name, mono_commit_id.clone()));
            }
        }
        // TODO: Don't update refs unless needed.
        // Refs just filtered should not need this update.
        Self::update_refs(
            &repo,
            logger,
            old_monorepo_refs,
            monorepo_object_tips,
            monorepo_symbolic_tips,
        )?;
        Self::write_monorepo_refs_log(&repo, final_monorefs.as_slice())?;
        Ok(())
    }

    fn update_refs(
        repo: &gix::Repository,
        logger: &Logger,
        old_refs: Vec<FullName>,
        object_tips: Vec<(FullName, MonoRepoCommitId)>,
        symbolic_tips: Vec<(FullName, FullName)>,
    ) -> Result<()> {
        let mut ref_edits = Vec::new();
        // Update object refs.
        for (mono_ref, commit_id) in &object_tips {
            ref_edits.push(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Update {
                    log: gix::refs::transaction::LogChange {
                        mode: gix::refs::transaction::RefLog::AndReference,
                        force_create_reflog: false,
                        message: b"git-toprepo filter".into(),
                    },
                    expected: gix::refs::transaction::PreviousValue::Any,
                    new: gix::refs::Target::Object(**commit_id),
                },
                name: mono_ref.clone(),
                deref: false,
            });
        }
        // Update symbolic refs.
        for (mono_link_name, mono_target_name) in &symbolic_tips {
            let new_target = gix::refs::Target::Symbolic(mono_target_name.clone());
            ref_edits.push(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Update {
                    log: gix::refs::transaction::LogChange {
                        mode: gix::refs::transaction::RefLog::AndReference,
                        force_create_reflog: false,
                        message: b"git-toprepo filter".into(),
                    },
                    expected: gix::refs::transaction::PreviousValue::Any,
                    new: new_target,
                },
                name: mono_link_name.clone(),
                deref: false,
            });
        }
        // Remove refs/remote/origin/* references that were removed in refs/namespaces/top/*.
        let all_new_tips = object_tips
            .iter()
            .map(|(name, _)| name)
            .chain(symbolic_tips.iter().map(|(name, _)| name))
            .collect::<HashSet<_>>();
        for old_ref in old_refs {
            if all_new_tips.contains(&old_ref) {
                continue;
            }
            logger.warning(format!("Deleting now removed ref {old_ref}",));
            ref_edits.push(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Delete {
                    // TODO: Is MustExistAndMatch possible? Should the previous
                    // filter result be stored in the log file?
                    expected: gix::refs::transaction::PreviousValue::Any,
                    log: gix::refs::transaction::RefLog::AndReference,
                },
                name: old_ref,
                deref: false,
            });
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
        &mut self,
        ref_to_inject: &FullNameRef,
        sub_repo_name: &SubRepoName,
        abs_sub_path: &GitPath,
        dest_ref: &FullNameRef,
        logger: &Logger,
    ) -> Result<Rc<MonoRepoCommit>> {
        let repo = self.gix_repo.to_thread_local();

        let mut ref_to_inject = repo.refs.find(ref_to_inject)?;
        let id_to_inject = ref_to_inject.peel_to_id_in_place(&repo.refs, &repo.objects)?;
        let thin_commit_to_inject = self
            .top_repo_cache
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
            .clone(); // Clone to avoid borrowing the `storage` object.

        let pb = self.progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{elapsed:>4} {msg} {pos}")
                        .unwrap(),
                )
                .with_message("Looking for mono commit to expand onto"),
        );
        // Hopefully, HEAD points to a commit.
        let head_id: gix::ObjectId = repo.head_id()?.detach();
        let mut possible_mono_parents = Vec::new();
        let (_possible_mono_parent_ids, _num_skipped_unknowns) =
            crate::git::get_first_known_commits(&repo, [head_id].into_iter(), |commit_id| {
                let Some(mono_parent) = self
                    .top_repo_cache
                    .monorepo_commits
                    .get(&MonoRepoCommitId(commit_id))
                else {
                    return false;
                };
                possible_mono_parents.push(mono_parent.clone());
                true
            })?;
        drop(pb);

        let fast_importer = crate::git_fast_export_import::FastImportRepo::new(
            self.gix_repo.git_dir(),
            logger.clone(),
        )?;
        let mut expander = TopRepoExpander {
            gix_repo: &repo,
            storage: &mut self.top_repo_cache,
            config: &mut self.config,
            progress: self.progress.clone(),
            logger: logger.clone(),
            fast_importer,
            imported_commits: HashMap::new(),
            bumps: crate::expander::BumpCache::default(),
            inject_at_oldest_super_commit: true,
        };
        let result = (|| {
            let Some(mono_commit) = expander.inject_submodule_commit(
                dest_ref,
                possible_mono_parents,
                abs_sub_path,
                sub_repo_name,
                &thin_commit_to_inject,
            )?
            else {
                anyhow::bail!(
                    "Failed to inject commit {}, to become {}, at {abs_sub_path}: No common history with HEAD",
                    ref_to_inject.name,
                    dest_ref.as_bstr()
                );
            };
            Ok(mono_commit)
        })();
        expander.wait()?;
        result
    }

    pub fn push(
        &mut self,
        top_push_url: &gix::Url,
        local_rev: &String,
        remote_ref: &FullName,
        dry_run: bool,
        logger: &Logger,
    ) -> Result<()> {
        if self.top_repo_cache.monorepo_commits.is_empty() {
            anyhow::bail!(
                "No filtered mono commits exists, please run `git toprepo refilter` first"
            );
        }
        let repo = self.gix_repo.to_thread_local();

        let local_rev_arg: std::ffi::OsString = local_rev.into();
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

        let mut dedup_cache = std::mem::take(&mut self.top_repo_cache.dedup);
        let mut fast_importer = crate::git_fast_export_import_dedup::FastImportRepoDedup::new(
            crate::git_fast_export_import::FastImportRepo::new(
                self.gix_repo.git_dir(),
                logger.clone(),
            )?,
            &mut dedup_cache,
        );
        let to_push_metadata =
            self.split_for_push(&mut fast_importer, top_push_url, &export_refs_args, logger);
        // Make sure to gracefully shutdown the fast-importer before returning.
        fast_importer.wait()?;
        self.top_repo_cache.dedup = dedup_cache;
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
            logger.info("Nothing to push".to_owned());
            return Ok(());
        }

        let info_label = if dry_run { "Would run" } else { "Running" };
        let mut failed_pushes = 0;
        for push_info in to_push_metadata {
            let topic_arg = match &push_info.topic {
                Some(topic) => format!(" -o topic={topic}"),
                None => String::new(),
            };
            logger.info(format!(
                "{info_label}: git push {}{topic_arg} {}:{remote_ref}",
                push_info.push_url,
                push_info.commit_id.to_hex()
            ));
            if dry_run {
                continue;
            }
            // Do the push.
            let mut cmd = git_command(self.gix_repo.git_dir());
            cmd.arg("push")
                .arg(push_info.push_url.to_bstring().to_os_str()?);
            if let Some(topic) = &push_info.topic {
                cmd.arg("-o").arg(format!("topic={topic}"));
            }
            cmd.arg(format!("{}:{remote_ref}", push_info.commit_id));
            if let Err(err) = cmd.safe_status()?.check_success() {
                logger.error(format!(
                    "Failed to git push {} {}:{remote_ref}: {err:#}",
                    push_info.push_url, push_info.commit_id
                ));
                failed_pushes += 1;
            }
        }
        if failed_pushes != 0 {
            let times_string = if failed_pushes == 1 { "time" } else { "times" };
            anyhow::bail!(format!("git-push failed {failed_pushes} {times_string}"));
        }
        Ok(())
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
        &mut self,
        fast_importer: &mut crate::git_fast_export_import_dedup::FastImportRepoDedup<'_>,
        top_push_url: &gix::Url,
        export_refs_args: &[std::ffi::OsString],
        logger: &Logger,
    ) -> Result<Vec<PushMetadata>> {
        let monorepo_commits = &self.top_repo_cache.monorepo_commits;
        let repo = self.gix_repo.to_thread_local();

        let pb = self.progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{elapsed:>4} {msg} {pos}")
                        .unwrap(),
                )
                .with_message("Splitting commits"),
        );
        let fast_exporter = crate::git_fast_export_import::FastExportRepo::load_from_path(
            self.gix_repo.git_dir(),
            Some(export_refs_args),
            logger.clone(),
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
                        anyhow::bail!(
                            "Pushing empty commits like {mono_commit_id} is not supported"
                        );
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
                        let (repo_name, submod_path, rel_path, push_url) = Self::resolve_push_repo(
                            &gix_mono_commit,
                            GitPath::new(fc.path),
                            top_push_url.clone(),
                            &mut self.config,
                        )?;
                        grouped_file_changes
                            .entry((submod_path, repo_name, push_url))
                            .or_default()
                            .push(ChangedFile {
                                path: (*rel_path).clone(),
                                change: fc.change,
                            });
                    }
                    let (message, topic) =
                        Self::rewrite_push_message(exported_mono_commit.message.to_str()?);
                    if grouped_file_changes.len() > 1 && topic.is_none() {
                        anyhow::bail!(
                            "Multiple submodules changed in commit {mono_commit_id}, but no topic was provided. \
                            Please amend the commit message to add a 'Topic: something-descriptive' line."
                        );
                    }
                    for ((abs_sub_path, repo_name, push_url), file_changes) in grouped_file_changes
                    {
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
                                    ExpandedOrRemovedSubmodule::Expanded(
                                        ExpandedSubmodule::Expanded(SubmoduleContent {
                                            repo_name: sub_repo_name,
                                            orig_commit_id: import_commit_id,
                                        }),
                                    ),
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
                    logger.warning(format!(
                        "Resetting {} to {} is unimplemented",
                        reset.branch, reset.from
                    ));
                }
            };
        }
        Ok(to_push_metadata)
    }
}

#[derive(Debug, Clone)]
struct PushMetadata {
    pub push_url: gix::Url,
    pub topic: Option<String>,
    pub commit_id: CommitId,
    pub parents: Vec<CommitId>,
}

#[serde_as]
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
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
    /// Mapping from top repo commit to mono repo commit.
    pub top_to_mono_map: HashMap<TopRepoCommitId, Rc<MonoRepoCommit>>,
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

/// While importing, the commit id might not yet be known and set to a dummy id.
#[serde_as]
#[derive(
    Debug, Clone, Eq, Hash, PartialEq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct MonoRepoCommitId(
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")] CommitId,
);

impl MonoRepoCommitId {
    pub fn new(commit_id: CommitId) -> Self {
        MonoRepoCommitId(commit_id)
    }

    pub fn dummy() -> Self {
        Self(gix::ObjectId::null(gix::hash::Kind::Sha1))
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
        dot_gitmodules: Option<BlobId>,
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

    /// Walks the first parent commit graph to the submodule entry.
    pub fn get_submodule<'a>(&'a self, path: &GitPath) -> Option<&'a ThinSubmodule> {
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
