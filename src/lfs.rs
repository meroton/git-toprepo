use crate::git::GitModulesInfo;
use crate::git::GitPath;
use crate::gitmodules::SubmoduleUrlExt as _;
use crate::log::CommandSpanExt as _;
use crate::repo::ConfiguredTopRepo;
use crate::repo_name::RepoName;
use crate::util::CommandExtension as _;
use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsFetchTarget {
    pub include_path: GitPath,
    pub repo_name: RepoName,
    pub remote_url: gix::Url,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LfsFetchOptions {
    pub dry_run: bool,
    pub prune: bool,
    pub recent: bool,
    pub refetch: bool,
    pub exclude: Vec<String>,
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

pub fn resolve_lfs_fetch_targets(
    top_repo: &ConfiguredTopRepo,
    paths: &[PathBuf],
) -> Result<Vec<LfsFetchTarget>> {
    let worktree = top_repo
        .gix_repo
        .workdir()
        .context("Worktree missing in git repository")?;
    let worktree = normalize_path(worktree);
    let top_url = default_fetch_url(&top_repo.gix_repo)?;
    let gitmodules = GitModulesInfo::parse_dot_gitmodules_in_repo(&top_repo.gix_repo)?;

    paths
        .iter()
        .map(|path| {
            ensure_literal_path(path)?;
            let include_path = repo_relative_git_path(&worktree, path)?;
            resolve_lfs_target_for_path(top_repo, &gitmodules, &top_url, include_path)
        })
        .collect()
}

pub fn run_lfs_fetch(
    worktree: &Path,
    targets: &[LfsFetchTarget],
    options: &LfsFetchOptions,
) -> Result<()> {
    for target in targets {
        let remote_url = target.remote_url.to_bstring().to_str()?.to_owned();

        let mut cmd = Command::new("git");
        cmd.arg("lfs").arg("fetch").arg(&remote_url);

        if options.dry_run {
            cmd.arg("--dry-run");
        }
        if options.prune {
            cmd.arg("--prune");
        }
        if options.recent {
            cmd.arg("--recent");
        }
        if options.refetch {
            cmd.arg("--refetch");
        }

        for exclude in &options.exclude {
            cmd.arg(format!("--exclude={exclude}"));
        }

        cmd.arg(format!("--include={}", target.include_path));

        cmd.current_dir(worktree)
            .trace_command(crate::command_span!("git lfs fetch"))
            .safe_status()?
            .check_success()
            .with_context(|| {
                if options.dry_run {
                    format!(
                        "'git lfs fetch --dry-run' failed for '{}'. \
Your installed Git LFS version may not support '--dry-run'.",
                        target.include_path
                    )
                } else {
                    format!("'git lfs fetch' failed for '{}'", target.include_path)
                }
            })?;
    }

    Ok(())
}

fn default_fetch_url(repo: &gix::Repository) -> Result<gix::Url> {
    // NB: find_default_remote() returns Option<Result<Remote, Error>>;
    // the first context handles None, the second handles Err.
    Ok(repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .context("Default git-remote not found")?
        .context("Bad default git-remote")?
        .url(gix::remote::Direction::Fetch)
        .context("Missing fetch URL for the default git-remote")?
        .clone())
}

fn resolve_lfs_target_for_path(
    top_repo: &ConfiguredTopRepo,
    gitmodules: &GitModulesInfo,
    top_url: &gix::Url,
    include_path: GitPath,
) -> Result<LfsFetchTarget> {
    let Some((submodule_path, submodule_url)) =
        deepest_containing_submodule(gitmodules, &include_path)
    else {
        return Ok(LfsFetchTarget {
            include_path,
            repo_name: RepoName::Top,
            remote_url: top_url.clone(),
        });
    };

    let submodule_url = submodule_url
        .as_ref()
        .map_err(|err| anyhow::anyhow!("Bad URL for {submodule_path} in .gitmodules: {err}"))?
        .clone();
    let resolved_url = top_url.join(&submodule_url);
    let repo_name = match top_repo
        .ledger
        .get_name_from_similar_full_url(resolved_url.clone(), top_url)
    {
        Ok(RepoName::SubRepo(repo_name))
            if !top_repo.ledger.missing_subrepos.contains(&repo_name) =>
        {
            repo_name
        }
        Ok(_) | Err(_) => {
            bail!(
                "Cannot resolve LFS remote for '{include_path}'.\n\
The path belongs to submodule '{submodule_path}', but its .gitmodules URL is not configured in .gittoprepo.toml."
            )
        }
    };

    Ok(LfsFetchTarget {
        include_path,
        repo_name: RepoName::from(repo_name),
        remote_url: resolved_url,
    })
}

fn deepest_containing_submodule<'a>(
    gitmodules: &'a GitModulesInfo,
    path: &GitPath,
) -> Option<(&'a GitPath, &'a Result<gix::Url>)> {
    let mut best = None;
    let mut best_len = 0usize;
    for (submodule_path, url) in &gitmodules.submodules {
        if path.relative_to(submodule_path).is_some() && submodule_path.len() >= best_len {
            best = Some((submodule_path, url));
            best_len = submodule_path.len();
        }
    }
    best
}

fn ensure_literal_path(path: &Path) -> Result<()> {
    let path = path.to_string_lossy();
    if path.contains('*') || path.contains('?') || path.contains('[') || path.contains(']') {
        bail!(
            "'git toprepo lfs fetch' currently expects literal paths, not glob patterns: '{path}'"
        );
    }
    Ok(())
}

fn repo_relative_git_path(worktree: &Path, path: &Path) -> Result<GitPath> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let cwd = normalize_path(&cwd);
    let requested = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&cwd.join(path))
    };
    let rel = requested
        .strip_prefix(worktree)
        .with_context(|| format!("Path '{}' is outside the worktree", path.display()))?;
    Ok(path_to_git_path(rel))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(unix)]
fn path_to_git_path(path: &Path) -> GitPath {
    GitPath::from(path.as_os_str().as_encoded_bytes())
}

#[cfg(windows)]
fn path_to_git_path(path: &Path) -> GitPath {
    GitPath::from(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SubRepoConfig;
    use crate::repo_name::SubRepoName;
    use bstr::BStr;

    #[test]
    fn deepest_prefix_wins() {
        let mut gitmodules = GitModulesInfo::default();
        gitmodules.submodules.insert(
            GitPath::from("libs/a"),
            Ok(gix::Url::from_bytes(b"../a.git".as_bstr()).unwrap()),
        );
        gitmodules.submodules.insert(
            GitPath::from("libs/a/vendor/b"),
            Ok(gix::Url::from_bytes(b"../b.git".as_bstr()).unwrap()),
        );
        let hit =
            deepest_containing_submodule(&gitmodules, &GitPath::from("libs/a/vendor/b/model.bin"))
                .map(|(path, _)| path.clone())
                .unwrap();
        assert_eq!(hit, GitPath::from("libs/a/vendor/b"));
    }

    #[test]
    fn repo_relative_path_rejects_outside_worktree() {
        let worktree = normalize_path(Path::new("/tmp/worktree"));
        let err = repo_relative_git_path(&worktree, Path::new("../outside"))
            .expect_err("path should be rejected");
        assert!(err.to_string().contains("outside the worktree"));
    }

    #[test]
    fn resolves_top_level_path_to_top_repo() {
        let repo = init_repo();
        let target =
            resolve_lfs_fetch_targets(&repo, &[repo.gix_repo.workdir().unwrap().join("video.mov")])
                .unwrap();
        assert_eq!(target[0].repo_name, RepoName::Top);
        assert_eq!(target[0].include_path, GitPath::from("video.mov"));
    }

    fn init_repo() -> ConfiguredTopRepo {
        let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
        git_toprepo_testtools::test_util::git_command_for_testing(&temp_dir)
            .args(["init"])
            .assert()
            .success();
        git_toprepo_testtools::test_util::git_command_for_testing(&temp_dir)
            .args([
                "config",
                "remote.origin.url",
                "ssh://example.com/toprepo.git",
            ])
            .assert()
            .success();
        std::fs::write(
            temp_dir.join(".gitmodules"),
            "[submodule \"service-a\"]\n\tpath = service-a\n\turl = ../service-a.git\n",
        )
        .unwrap();
        let mut repo = ConfiguredTopRepo::new_empty(gix::open(temp_dir.path()).unwrap());
        let subrepo_url =
            gix::Url::from_bytes(BStr::new("ssh://example.com/service-a.git".as_bytes())).unwrap();
        repo.config.subrepos.insert(
            SubRepoName::new("service-a".to_owned()),
            SubRepoConfig::new_disabled(subrepo_url.clone()),
        );
        repo.ledger.subrepos = repo.config.subrepos.clone();
        repo
    }

    #[test]
    fn resolves_submodule_path_to_subrepo_url() {
        let repo = init_repo();
        let mut gitmodules = GitModulesInfo::default();
        gitmodules.submodules.insert(
            GitPath::from("service-a"),
            Ok(gix::Url::from_bytes(b"../service-a.git".as_bstr()).unwrap()),
        );
        assert_eq!(
            deepest_containing_submodule(
                &gitmodules,
                &GitPath::from("service-a/assets/model.bin"),
            )
            .map(|(path, _)| path.clone()),
            Some(GitPath::from("service-a"))
        );
        let top_url = default_fetch_url(&repo.gix_repo).unwrap();
        let target = resolve_lfs_target_for_path(
            &repo,
            &gitmodules,
            &top_url,
            GitPath::from("service-a/assets/model.bin"),
        )
        .unwrap();
        assert_eq!(target.repo_name.to_string(), "service-a");
        assert_eq!(
            target.include_path,
            GitPath::from("service-a/assets/model.bin")
        );
    }
}
