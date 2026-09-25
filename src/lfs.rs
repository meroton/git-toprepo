use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::CommandSpanExt as _;
use crate::log::ErrorObserver;
use crate::repo::ConfiguredTopRepo;
use crate::repo_name::RepoName;
use crate::ui::ProgressStatus;
use crate::util::CommandExtension as _;
use crate::util::EMPTY_GIX_URL;
use crate::util::argument_error_unless;
use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use clap::Args;
use itertools::Itertools;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsFetchTarget {
    pub include_path: GitPath,
    pub repo_name: RepoName,
    pub remote_url: gix::Url,
}

const GIT_LFS_REQUIRED: &str = "\
Git LFS is required for 'git toprepo lfs fetch'.
Install Git LFS and ensure 'git lfs version' works.";

pub fn ensure_git_lfs_available(repo_worktree: &Path) -> Result<()> {
    Command::new("git")
        .arg("lfs")
        .arg("version")
        .current_dir(repo_worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .trace_command(crate::command_span!("git lfs version"))
        .safe_status()
        .map_err(|err| anyhow::anyhow!("{GIT_LFS_REQUIRED}\nUnderlying error: {err}"))?
        .check_success()
        .map_err(|err| anyhow::anyhow!("{GIT_LFS_REQUIRED}\nUnderlying error: {err}"))?;

    Ok(())
}

/// Returns a list of equivialent patterns that only affects the given directory.
///
/// # Empirical observations
///
/// Running `GIT_CURL_VERBOSE=1 git lfs fetch -I <pattern>` gives the following observations:
///
/// 1. Git LFS doesn't support negation, i.e. `!pattern`.
/// 2. Paths are always relative to the repository root.
/// 3. `GIT_CURL_VERBOSE=1 git -c lfs.fetchinclude=/tests lfs fetch -I ''` shows that an empty pattern includes all files but
///    `GIT_CURL_VERBOSE=1 git -c lfs.fetchinclude=/tests lfs fetch -X ''` shows that an empty pattern does not exclude any files.
///    This function is only concerned with include patterns, not exclude patterns.
/// 4. The following patterns accept the path `sub/dir/file`.
///    * `dir`
///    * `f*e`
///    * `fi*e`
///    * `dir/`
///    * `sub`
///    * `/sub`
///    * `/**/file`
///    * `/**/dir/**/file`
/// 5. The following patterns reject the path `sub/dir/file`.
///    * `/`
///    * `/dir`
///    * `dir/file` -
///
/// # Examples
///
/// ```
/// # use git_toprepo::lfs::filter_include_pattern;
///
/// // Passthrough of the pattern for an empty subdir.
/// let subdir = "";
/// let examples: Vec<(&str, &[&str])> = vec![
///     ("pattern", &["pattern"]),
///     ("pat/tern", &["pat/tern"]),
///     ("pattern", &["pattern"]),
///     ("pattern/", &["pattern/"]),
/// ];
/// for (pattern, expected) in examples {
///     assert_eq!(
///         filter_include_pattern(pattern, subdir),
///         Vec::from(expected),
///         "actual != expected for pattern={pattern} subdir={subdir}",
///     );
/// }
///
/// // Try sub patterns.
/// let subdir = "sub";
/// let examples: Vec<(&str, &[&str])> = vec![
///     ("", &["/sub/"]),
///     ("/", &[]),
///     ("sub/", &["/sub/"]),
///     ("dir", &["/sub/**/dir"]),
///     ("dir/", &["/sub/**/dir/"]),
///     ("/dir", &[]),
///     ("sub/dir", &["/sub/dir"]),
///     ("/sub/dir/inner", &["/sub/dir/inner"]),
///     ("inner*", &["/sub/**/inner*"]),
///     ("*/dir/inner", &["/sub/dir/inner"]),
///     ("sub/*/inner", &["/sub/*/inner"]),
///     ("**/dir/inner", &["/sub/**/dir/inner"]),
///     ("**/*/inner", &["/sub/**/*/inner", "/sub/inner"]),
///     ("sub/**/inner", &["/sub/**/inner"]),
///     ("sub/**/dir/inner", &["/sub/**/dir/inner"]),
///     ("**/other/**/inner", &["/sub/**/other/**/inner"]),
///     ("**", &["/sub/"]),
///     ("sub/**", &["/sub/**"]),
///     ("sub/inner/**", &["/sub/inner/**"]),
///     ("other/**/inner", &[]),
/// ];
/// for (pattern, expected) in examples {
///     assert_eq!(
///         filter_include_pattern(pattern, subdir),
///         Vec::from(expected),
///         "actual != expected for pattern={pattern} subdir={subdir}",
///     );
/// }
///
/// // Try sub/dir patterns.
/// let subdir = "sub/dir";
/// let examples: Vec<(&str, &[&str])> = vec![
///     ("", &["/sub/dir/"]),
///     ("/", &[]),
///     ("sub/", &["/sub/dir/"]),
///     ("dir", &["/sub/dir/"]),
///     ("dir/", &["/sub/dir/"]),
///     ("/dir", &[]),
///     ("sub/dir", &["/sub/dir/"]),
///     ("/sub/dir/inner", &["/sub/dir/inner"]),
///     ("inner*", &["/sub/dir/**/inner*"]),
///     ("*/dir/inner", &["/sub/dir/inner"]),
///     ("sub/*/inner", &["/sub/dir/inner"]),
///     ("**/dir/inner", &["/sub/dir/**/dir/inner", "/sub/dir/inner"]),
///     ("**/*/inner", &["/sub/dir/**/*/inner", "/sub/dir/inner"]),
///     ("sub/**/inner", &["/sub/dir/**/inner"]),
///     (
///         "sub/**/dir/inner",
///         &["/sub/dir/**/dir/inner", "/sub/dir/inner"],
///     ),
///     ("**/other/**/inner", &["/sub/dir/**/other/**/inner"]),
///     ("**", &["/sub/dir/"]),
///     ("sub/**", &["/sub/dir/"]),
///     ("sub/dir/**", &["/sub/dir/**"]),
///     ("sub/dir/inner/**", &["/sub/dir/inner/**"]),
///     ("sub/other/**/inner", &[]),
/// ];
/// for (pattern, expected) in examples {
///     assert_eq!(
///         filter_include_pattern(pattern, subdir),
///         Vec::from(expected),
///         "actual != expected for pattern={pattern} subdir={subdir}",
///     );
/// }
/// ```
pub fn filter_include_pattern(pattern: &str, subdir: &str) -> Vec<String> {
    if pattern == "/" {
        return vec![];
    }
    let subdir = subdir.trim_suffix('/');
    if subdir.is_empty() {
        return vec![pattern.to_owned()];
    }
    let subdir_with_slashes = format!("/{}/", subdir);
    if pattern.is_empty() {
        return vec![subdir_with_slashes];
    }
    // gix-glob doesn't handle single entry patterns correctly.
    let pattern = if pattern.trim_suffix('/').contains('/') {
        &format!("/{}", pattern.trim_prefix('/'))
    } else {
        &format!("/**/{pattern}")
    };

    // Check if the pattern matches the subdir directly, allowing everything under it to be included.
    if gix::glob::Pattern::from_bytes_without_negation(pattern.trim_suffix('/').as_bytes())
        .expect("non-empty pattern")
        .matches(
            subdir.into(),
            gix::glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
        )
    {
        return vec![subdir_with_slashes];
    }
    if gix::glob::Pattern::from_bytes_without_negation(
        format!("{}/**", pattern.trim_suffix('/')).as_bytes(),
    )
    .expect("non-empty pattern")
    .matches(
        subdir.into(),
        gix::glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
    ) {
        return vec![subdir_with_slashes];
    }

    let mut result = Vec::new();
    let mut component_count_left = subdir_with_slashes.matches('/').count() - 1; // "/ab/cde/fg/" => 3 components
    debug_assert_eq!(pattern.match_indices('/').next(), Some((0, "/")));
    for (i, _) in pattern.match_indices('/').skip(1) {
        let partial_pattern = &pattern[..i];
        let partial_pattern_ends_with_double_star = partial_pattern.ends_with("/**");
        if gix::glob::Pattern::from_bytes_without_negation(partial_pattern.as_bytes())
            .expect("non-empty pattern")
            .matches(
                subdir.into(),
                gix::glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            )
        {
            if partial_pattern_ends_with_double_star {
                // The ** might match both part of the subdir and part of the rest of the path.
                result.push(format!("{subdir_with_slashes}**/{}", &pattern[i + 1..]));
            } else {
                result.push(format!("{subdir_with_slashes}{}", &pattern[i + 1..]));
            }
        }
        // Worth continuing? `**` can match zero components but otherwise one
        // component of subdir must have been consumed by the partial pattern.
        if !partial_pattern_ends_with_double_star {
            component_count_left -= 1;
        }
        if component_count_left == 0 {
            break;
        }
    }
    result
}

pub struct FetchArgs {
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub options: FetchOptions,
    pub remote: gix::Url,
    pub refs: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub struct FetchOptions {
    /// Download objects referenced by recent branches & commits in addition to
    /// those that would otherwise be downloaded.
    #[arg(long)]
    pub recent: bool,

    /// Unsupported in git-toprepo's LFS wrapper.
    #[arg(
        long, hide = true,
        value_parser = |s: &str| argument_error_unless(s, false, "unsupported for 'git toprepo lfs fetch'"),
    )]
    pub all: bool,

    /// Prune old and unreferenced LFS objects after fetching.
    #[arg(long, short = 'p')]
    pub prune: bool,

    /// Fetch objects even if they already exist locally.
    #[arg(long)]
    pub refetch: bool,

    /// Print what Git LFS would fetch, without downloading objects.
    // git-lfs has `-d`, the rest of git-toprepo `-n`. Skipping short flag here
    // to avoid confusion.
    #[arg(long)]
    pub dry_run: bool,

    /// Unsupported in git-toprepo's LFS wrapper.
    #[arg(
        long, hide = true,
        value_parser = |s: &str| argument_error_unless(s, false, "unsupported for 'git toprepo lfs fetch'"),
    )]
    pub json: bool,

    /// Unsupported in git-toprepo's LFS wrapper.
    #[arg(
        long, hide = true,
        value_parser = |s: &str| argument_error_unless(s, false, "unsupported for 'git toprepo lfs fetch'"),
    )]
    pub stdin: bool,
}

struct FetchCommand {
    pub repo_name: RepoName,
    pub fetch_url: gix::Url,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

impl FetchCommand {
    pub fn create_command(&self, options: &FetchOptions, refs: &[String]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("lfs").arg("fetch");

        if options.recent {
            cmd.arg("--recent");
        }
        if options.all {
            cmd.arg("--all");
        }
        if options.prune {
            cmd.arg("--prune");
        }
        if options.refetch {
            cmd.arg("--refetch");
        }
        if options.dry_run {
            cmd.arg("--dry-run");
        }
        if options.json {
            unimplemented!("git lfs fetch --json'");
        }
        if options.stdin {
            unimplemented!("git lfs fetch --stdin'");
        }
        cmd.arg("--include");
        cmd.arg(self.include_patterns.join(","));
        cmd.arg("--exclude");
        cmd.arg(self.exclude_patterns.join(","));
        cmd.arg(self.fetch_url.to_string());
        cmd.args(refs);
        cmd
    }
}

fn revolve_lfs_fetches(
    worktree: &Path,
    ledger: &mut crate::loader::SubRepoLedger,
    remote: &gix::Url,
    global_include_patterns: &[String],
    global_exclude_patterns: &[String],
    error_observer: &ErrorObserver,
) -> Result<Vec<FetchCommand>> {
    let mut lfs_fetches = Vec::new();
    // TODO: Is utf-8 a too harsh requirement for submodule paths?
    let mut submodules_todo = vec![("".to_owned(), Ok(EMPTY_GIX_URL.clone()))];
    while let Some((submodule_dir, generic_url)) = submodules_todo.pop() {
        let submod_result = try {
            // Does any of the include filters apply to this submodule?
            let mut submodule_include_patterns = Vec::new();
            for global_pattern in global_include_patterns {
                submodule_include_patterns
                    .extend(filter_include_pattern(global_pattern, &submodule_dir));
            }
            if submodule_include_patterns.is_empty() {
                continue;
            }

            // Some include filter matched, process this submodule.
            let generic_url =
                generic_url.with_context(|| format!("URL for submodule in {submodule_dir}"))?;
            let (repo_name, fetch_url) = if generic_url.to_bstring().is_empty() {
                (RepoName::Top, remote.clone())
            } else {
                let sub_repo_name = match ledger.get_or_insert_from_url(&generic_url)? {
                    crate::config::GetOrInsertOk::Found((name, _)) => name,
                    crate::config::GetOrInsertOk::Missing(_)
                    | crate::config::GetOrInsertOk::MissingAgain(_) => {
                        anyhow::bail!("Missing URL {generic_url} in the Git Toprepo configuration");
                    }
                };
                let fetch_url = remote.join(
                    &ledger
                        .subrepos
                        .get(&sub_repo_name)
                        .expect("just inserted")
                        .url,
                );
                (RepoName::SubRepo(sub_repo_name), fetch_url)
            };
            // Use all the global exclude patterns as well, some might be unused
            // but no harm is done passing them all on.
            let mut submodule_exclude_patterns = Vec::from(global_exclude_patterns);

            // Traverse inner submodules, they might need `git lfs fetch` too.
            // Also add the inner submodules to the exclude patterns.
            let git_modules_info =
                GitModulesInfo::parse_dot_gitmodules_file_in_dir(&worktree.join(&submodule_dir))?;
            for (rel_dir, rel_url) in git_modules_info.submodules {
                let rel_dir = rel_dir.to_str().with_context(|| {
                    format!(
                        "Submodule path {} inside {submodule_dir}",
                        rel_dir.to_str_lossy()
                    )
                })?;
                let inner_dir = Path::new(&submodule_dir)
                    .join(rel_dir)
                    .into_string()
                    .expect("only utf-8 components");
                let inner_generic_url = rel_url.map(|u| generic_url.join(&u));
                submodule_exclude_patterns.push(inner_dir.to_owned());
                submodules_todo.push((inner_dir, inner_generic_url));
            }

            lfs_fetches.push(FetchCommand {
                repo_name,
                fetch_url,
                include_patterns: submodule_include_patterns,
                exclude_patterns: submodule_exclude_patterns,
            });
        }
        .with_context(|| format!("In {submodule_dir}"));
        error_observer.maybe_consume(submod_result)?;
    }
    Ok(lfs_fetches)
}

pub fn run_lfs_fetch(
    configured_repo: &mut ConfiguredTopRepo,
    args: FetchArgs,
    threadpool: &threadpool::ThreadPool,
    error_observer: &ErrorObserver,
    progress: &indicatif::MultiProgress,
) -> Result<()> {
    let worktree = configured_repo
        .gix_repo
        .workdir()
        .context("Worktree missing in git repository")?;

    // Resolve all `git lfs fetch` commands before starting to execute some of
    // them. It would be annoying to fail when resolving the second command and
    // then have the first command already running.
    let lfs_fetches = revolve_lfs_fetches(
        worktree,
        &mut configured_repo.ledger,
        &args.remote,
        &args.include_patterns,
        &args.exclude_patterns,
        error_observer,
    )?;

    let style = indicatif::ProgressStyle::with_template(
        "     {prefix:.cyan} [{bar:24}] {pos}/{len}{wide_msg}",
    )
    .unwrap()
    .progress_chars("=> ");
    let lfs_fetch_progress = ProgressStatus::new(
        progress.clone(),
        progress.add(
            indicatif::ProgressBar::no_length()
                .with_style(style.clone())
                .with_prefix("Fetching "),
        ),
    );
    lfs_fetch_progress.set_queue_size(lfs_fetches.len());

    let global_refs = Arc::new(args.refs.iter().cloned().collect_vec());
    let worktree = Arc::new(worktree.to_owned());
    for fetch_command in lfs_fetches {
        run_single_lfs_fetch_in_threadpool(
            fetch_command,
            error_observer.clone(),
            worktree.clone(),
            args.options.clone(),
            global_refs.clone(),
            threadpool,
            lfs_fetch_progress.clone(),
        );
    }
    threadpool.join();
    error_observer.get_result(())
}

fn run_single_lfs_fetch_in_threadpool(
    fetch_command: FetchCommand,
    error_observer: ErrorObserver,
    worktree: Arc<PathBuf>,
    options: FetchOptions,
    global_refs: Arc<Vec<String>>,
    threadpool: &threadpool::ThreadPool,
    lfs_fetch_progress: ProgressStatus,
) {
    threadpool.execute(move || {
        if error_observer.should_interrupt() {
            return;
        }
        error_observer.consume(run_single_lfs_fetch(
            fetch_command,
            &worktree,
            options,
            &global_refs,
            lfs_fetch_progress,
        ));
    })
}

fn run_single_lfs_fetch(
    fetch_command: FetchCommand,
    worktree: &Path,
    options: FetchOptions,
    global_refs: &[String],
    lfs_fetch_progress: ProgressStatus,
) -> Result<()> {
    let pb_url = indicatif::ProgressBar::hidden()
        .with_style(
            indicatif::ProgressStyle::with_template("{elapsed:>4} {prefix:.cyan} {msg}").unwrap(),
        )
        .with_prefix("git lfs fetch")
        .with_message(fetch_command.fetch_url.to_string());
    let pb_status = indicatif::ProgressBar::hidden()
        .with_style(indicatif::ProgressStyle::with_template("     {msg}").unwrap());
    // Make sure that the elapsed time is updated continuously.
    pb_url.enable_steady_tick(std::time::Duration::from_millis(1000));

    let _progress_task = lfs_fetch_progress.start(
        fetch_command.repo_name.to_string(),
        vec![pb_url, pb_status.clone()],
    );
    lfs_fetch_progress.inc_queue_size(-1);

    let mut cmd = fetch_command.create_command(&options, global_refs);
    let (mut proc, _span_guard) = cmd
        .current_dir(worktree)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .trace_command(crate::command_span!("git lfs fetch"))
        .spawn()
        .context("Failed to spawn git lfs fetch")?;
    let stderr_pipe = proc.stderr.take().expect("piping stderr");
    let permanent_stderr = crate::util::read_stderr_progress_status(stderr_pipe, |line| {
        tracing::trace!(name: "stderr", line = ?line);
        pb_status.set_message(line);
    });
    let exit_status = crate::util::SafeExitStatus::new(proc.wait().with_context(|| {
        format!(
            "Failed to wait for git-lfs-fetch {}",
            fetch_command.fetch_url
        )
    })?);
    if let Err(err) = exit_status.check_success() {
        bail!(
            "'git fetch{} {} failed: {err:#}{}{permanent_stderr}{}",
            if options.dry_run { "--dry-run" } else { "" },
            fetch_command.fetch_url,
            if permanent_stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            if options.dry_run {
                "\nYour installed Git LFS version may not support '--dry-run'."
            } else {
                ""
            },
        );
    }
    Ok(())
}
