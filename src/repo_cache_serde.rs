use crate::git::CommitId;
use crate::git::GitPath;
use crate::git::TreeId;
use crate::git_fast_export_import::WithoutCommitterId;
use crate::git_fast_export_import_dedup::GitFastExportImportDedupCache;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExportedFileEntry;
use crate::repo::MonoRepoCommit;
use crate::repo::MonoRepoCommitId;
use crate::repo::MonoRepoParent;
use crate::repo::OriginalSubmodParent;
use crate::repo::RepoData;
use crate::repo::RepoStates;
use crate::repo::ThinCommit;
use crate::repo::ThinSubmodule;
use crate::repo::TopRepoCache;
use crate::repo::TopRepoCommitId;
use crate::repo_name::RepoName;
use crate::util::NewlineTrimmer as _;
use crate::util::OrderedHashMap;
use crate::util::RcKey;
use anyhow::Context;
use anyhow::Result;
use itertools::Itertools;
use serde_with::serde_as;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Read as _;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use tracing::instrument;

/// Serializeable version of `crate::repo::TopRepoCache`.
#[serde_as]
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct SerdeTopRepoCache {
    /// The checksum of the git-toprepo configuration used when writing.
    config_checksum: String,
    #[serde_as(
        serialize_as = "serde_with::IfIsHumanReadable<OrderedHashMap<serde_with::DisplayFromStr, _>>"
    )]
    repos: SerdeRepoStates,
    monorepo_commits: Vec<SerdeMonoRepoCommit>,
    #[serde_as(serialize_as = "serde_with::IfIsHumanReadable<OrderedHashMap<_, _>>")]
    top_to_mono_map: HashMap<TopRepoCommitId, MonoRepoCommitId>,
    dedup: GitFastExportImportDedupCache,
}

impl SerdeTopRepoCache {
    const TOPREPO_CACHE_PATH: &str = "toprepo/cache.bincode";
    const CACHE_VERSION_PRELUDE: &str = "#cache-format-v2\n";

    /// Constructs the path to the git repository information cache inside
    /// `.git/toprepo/`.
    pub fn get_cache_path(git_dir: &Path) -> PathBuf {
        git_dir.join(Self::TOPREPO_CACHE_PATH)
    }

    /// Load parsed git repository information from `.git/toprepo/`.
    ///
    /// If `config_checksum` is `None`, the stored config checksum will be
    /// ignored and the cache will be considered valid.
    pub fn load_from_git_dir(git_dir: &Path, config_checksum: Option<&str>) -> Result<Self> {
        let cache_path = Self::get_cache_path(git_dir);
        let _span_guard = tracing::info_span!(
            "load_cache",
            pach = %cache_path.display()
        )
        .entered();
        (|| -> anyhow::Result<_> {
            let now = std::time::Instant::now();
            let reader = match std::fs::File::open(&cache_path) {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // No cache file, skip reading.
                    return Ok(Self::default());
                }
                Err(err) => return Err(err.into()),
            };
            let mut reader = std::io::BufReader::new(reader);
            // Check the header.
            let mut version_prelude = [0; Self::CACHE_VERSION_PRELUDE.len()];
            reader.read_exact(&mut version_prelude)?;
            if version_prelude != Self::CACHE_VERSION_PRELUDE.as_bytes() {
                log::warn!(
                    "Discarding toprepo cache {} due to version mismatch, expected {:?}",
                    cache_path.display(),
                    Self::CACHE_VERSION_PRELUDE.trim_newline_suffix(),
                );
                return Ok(Self::default());
            }

            let loaded_cache: SerdeTopRepoCache =
                bincode::serde::decode_from_std_read(&mut reader, bincode::config::standard())?;
            let mut eof_buffer = [0; 1];
            if reader.read(&mut eof_buffer)? != 0 {
                anyhow::bail!("Expected EOF");
            }
            let file = reader.into_inner();
            drop(file);
            log::debug!(
                "Deserialized toprepo cache from {} in {:.2?}",
                &cache_path.display(),
                now.elapsed()
            );
            // If the checksum has changed, the imported and exported commits might be totally different.
            if let Some(config_checksum) = config_checksum
                && loaded_cache.config_checksum != config_checksum
            {
                log::warn!(
                    "The git-toprepo configuration has changed, discarding the toprepo cache",
                );
                log::debug!(
                    "Configuration checksum {config_checksum} does not match cached checksum {}",
                    loaded_cache.config_checksum
                );
                return Ok(Self::default());
            }
            Ok(loaded_cache)
        })()
        .with_context(|| {
            format!(
                "Failed to deserialize repo cache from {}",
                &cache_path.display()
            )
        })
    }

    /// Write parsed git repository information as JSON.
    #[tracing::instrument(name = "dump_as_json", skip_all)]
    pub fn dump_as_json<W>(self, writer: W) -> Result<()>
    where
        W: Write,
    {
        serde_json::to_writer_pretty(writer, &self).context("Failed to serialize repo states")
    }

    /// Store parsed git repository information to `.git/toprepo/`.
    pub fn store_to_git_dir(&self, git_dir: &Path) -> Result<()> {
        let cache_path = Self::get_cache_path(git_dir);
        self.store(&cache_path)
    }

    /// Store parsed git repository information.
    pub fn store(&self, cache_path: &Path) -> Result<()> {
        self.store_impl(cache_path)
            .with_context(|| format!("Failed to store repo cache to {}", cache_path.display()))
    }

    #[tracing::instrument(
        name = "store_cache",
        skip_all,
        fields(path = %cache_path.display())
    )]
    fn store_impl(&self, cache_path: &Path) -> Result<()> {
        let now = std::time::Instant::now();
        let cache_path_tmp = cache_path.with_extension(".tmp");
        if let Some(parent_dir) = cache_path.parent() {
            std::fs::create_dir_all(parent_dir).context("Failed to create parent directory")?;
        }
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&cache_path_tmp)?);
        writer.write_all(Self::CACHE_VERSION_PRELUDE.as_bytes())?;
        bincode::serde::encode_into_std_write(self, &mut writer, bincode::config::standard())
            .context("Failed to serialize repo states")?;
        let file = writer
            .into_inner()
            // .into_inner()
            .context("Failed to flush buffered writer")?;
        drop(file);
        std::fs::rename(cache_path_tmp, cache_path)?;
        log::debug!(
            "Serialized repo states to {} in {:.2?}",
            cache_path.display(),
            now.elapsed()
        );
        Ok(())
    }

    #[instrument(name = "unpack_cache", skip_all)]
    pub fn unpack(self) -> Result<TopRepoCache> {
        let now = std::time::Instant::now();

        let mut repos = HashMap::with_capacity(self.repos.len());
        for (repo_name, serde_repo_data) in self.repos {
            let repo_data = serde_repo_data.unpack()?;
            if repos.insert(repo_name, repo_data).is_some() {
                panic!("Duplicate repo name in map");
            }
        }
        let mut monorepo_commits = HashMap::with_capacity(self.monorepo_commits.len());
        for serde_commit in self.monorepo_commits {
            let commit_id = serde_commit.commit_id.clone();
            let mono_commit = serde_commit.unpack(&monorepo_commits)?;
            monorepo_commits.insert(commit_id, mono_commit);
        }
        let monorepo_commit_ids = monorepo_commits
            .iter()
            .map(|(commit_id, commit)| (RcKey::new(commit), commit_id.clone()))
            .collect();
        let top_to_mono_map = self
            .top_to_mono_map
            .into_iter()
            .map(|(top_commit_id, mono_commit_id)| {
                let mono_commit = monorepo_commits.get(&mono_commit_id).with_context(|| {
                    format!(
                        "Top commit {top_commit_id} refers to unknown monorepo commit {mono_commit_id}"
                    )
                })?;
                Ok((top_commit_id, mono_commit.clone()))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        log::debug!("Unpacked toprepo cache in {:.2?}", now.elapsed());
        Ok(TopRepoCache {
            repos,
            monorepo_commits,
            monorepo_commit_ids,
            top_to_mono_map,
            dedup: self.dedup,
        })
    }

    #[instrument(name = "pack_cache", skip_all, fields(checksum = %config_checksum))]
    pub fn pack(cache: &TopRepoCache, config_checksum: String) -> Self {
        let now = std::time::Instant::now();
        let repos = Self::pack_repo_states(&cache.repos);
        let monorepo_commits = cache
            .monorepo_commits
            .values()
            .sorted_by_key(|commit| commit.depth)
            .map(|commit| SerdeMonoRepoCommit::pack(&cache.monorepo_commit_ids, commit))
            .collect_vec();
        let top_to_mono_map = cache
            .top_to_mono_map
            .iter()
            .map(|(top_commit_id, mono_commit)| {
                (
                    top_commit_id.clone(),
                    cache
                        .monorepo_commit_ids
                        .get(&RcKey::new(mono_commit))
                        .unwrap()
                        .clone(),
                )
            })
            .collect();
        log::debug!(
            "Packed toprepo cache for serialization in {:.2?}",
            now.elapsed()
        );
        Self {
            config_checksum,
            repos,
            monorepo_commits,
            top_to_mono_map,
            dedup: cache.dedup.clone(),
        }
    }

    fn pack_repo_states(repo_states: &RepoStates) -> SerdeRepoStates {
        repo_states
            .iter()
            .map(|(repo_name, repo_data)| {
                let thin_commits = repo_data
                    .thin_commits
                    .values()
                    .sorted_by_key(|thin_commit| thin_commit.depth)
                    .map(|thin_commit| SerdeThinCommit::from(thin_commit.as_ref()))
                    .collect_vec();
                (
                    repo_name.clone(),
                    SerdeRepoData {
                        url: repo_data.url.clone(),
                        thin_commits,
                        dedup_cache: repo_data.dedup_cache.clone(),
                    },
                )
            })
            .collect()
    }
}

/// Serializeable version of `ThinCommit`.
#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeThinCommit {
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub commit_id: CommitId,
    #[serde_as(as = "serde_with::IfIsHumanReadable<serde_with::DisplayFromStr>")]
    pub tree_id: TreeId,
    #[serde_as(as = "serde_with::IfIsHumanReadable<Vec<serde_with::DisplayFromStr>>")]
    pub parents: Vec<CommitId>,
    pub dot_gitmodules: Option<ExportedFileEntry>,
    pub submodule_bumps: BTreeMap<GitPath, ThinSubmodule>,
}

impl SerdeThinCommit {
    pub fn unpack(
        self,
        previous_commits: &HashMap<CommitId, Rc<ThinCommit>>,
    ) -> Result<Rc<ThinCommit>> {
        let commit_id: CommitId = self.commit_id;
        let thin_parents = self
            .parents
            .iter()
            .map(|parent_id| {
                previous_commits
                    .get(parent_id)
                    .with_context(|| format!("Parent {parent_id} of {commit_id} not yet parsed"))
                    .cloned()
            })
            .collect::<Result<Vec<_>>>()?;

        let thin_commit = ThinCommit::new_rc(
            commit_id,
            self.tree_id,
            thin_parents,
            self.dot_gitmodules,
            self.submodule_bumps,
        );
        Ok(thin_commit)
    }
}

impl From<&ThinCommit> for SerdeThinCommit {
    fn from(thin_commit: &ThinCommit) -> Self {
        Self {
            commit_id: thin_commit.commit_id,
            tree_id: thin_commit.tree_id,
            parents: thin_commit.parents.iter().map(|p| p.commit_id).collect(),
            dot_gitmodules: thin_commit.dot_gitmodules,
            submodule_bumps: thin_commit.submodule_bumps.clone(),
        }
    }
}

#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeRepoData {
    #[serde_as(as = "crate::util::SerdeGixUrl")]
    pub url: gix::Url,
    pub thin_commits: Vec<SerdeThinCommit>,
    #[serde_as(
        as = "serde_with::IfIsHumanReadable<OrderedHashMap<WithoutCommitterId, serde_with::DisplayFromStr>>"
    )]
    pub dedup_cache: HashMap<WithoutCommitterId, CommitId>,
}

impl SerdeRepoData {
    pub fn unpack(self) -> Result<RepoData> {
        let mut thin_commits = HashMap::with_capacity(self.thin_commits.len());
        for serde_commit in self.thin_commits {
            let thin_commit = serde_commit.unpack(&thin_commits)?;
            if let Some(existing_commit) = thin_commits.insert(thin_commit.commit_id, thin_commit) {
                anyhow::bail!(
                    "Duplicate commit id in cache: {}",
                    &existing_commit.commit_id
                );
            }
        }
        Ok(RepoData {
            url: self.url,
            thin_commits,
            dedup_cache: self.dedup_cache,
        })
    }
}

impl From<&RepoData> for SerdeRepoData {
    fn from(repo_data: &RepoData) -> Self {
        let thin_commits = repo_data
            .thin_commits
            .values()
            .sorted_by_key(|thin_commit| thin_commit.depth)
            .map(|thin_commit| SerdeThinCommit::from(thin_commit.as_ref()))
            .collect_vec();
        Self {
            url: repo_data.url.clone(),
            thin_commits,
            dedup_cache: repo_data.dedup_cache.clone(),
        }
    }
}

type SerdeRepoStates = HashMap<RepoName, SerdeRepoData>;

/// Serializeable version of `crate::repo::MonoRepoParent`.
#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
enum SerdeMonoRepoParent {
    OriginalSubmod(OriginalSubmodParent),
    Mono(MonoRepoCommitId),
}

/// Serializeable version of `crate::repo::MonoRepoCommit`.
#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
struct SerdeMonoRepoCommit {
    pub commit_id: MonoRepoCommitId,
    pub parents: Vec<SerdeMonoRepoParent>,
    pub top_bump: Option<TopRepoCommitId>,
    pub submodule_bumps: HashMap<GitPath, ExpandedOrRemovedSubmodule>,
}

impl SerdeMonoRepoCommit {
    pub fn unpack(
        self,
        monorepo_commits: &HashMap<MonoRepoCommitId, Rc<MonoRepoCommit>>,
    ) -> Result<Rc<MonoRepoCommit>> {
        let parents = self
            .parents
            .into_iter()
            .map(|parent| match parent {
                SerdeMonoRepoParent::OriginalSubmod(original_submod) => {
                    Ok(MonoRepoParent::OriginalSubmod(original_submod))
                }
                SerdeMonoRepoParent::Mono(monorepo_commit_id) => Ok(MonoRepoParent::Mono(
                    monorepo_commits
                        .get(&monorepo_commit_id)
                        .with_context(|| {
                            format!("Parent monorepo commit {monorepo_commit_id} not yet parsed")
                        })?
                        .clone(),
                )),
            })
            .collect::<Result<_>>()?;
        let commit = MonoRepoCommit::new_rc(parents, self.top_bump, self.submodule_bumps);
        Ok(commit)
    }

    pub fn pack(
        commit_ids: &HashMap<RcKey<MonoRepoCommit>, MonoRepoCommitId>,
        commit: &Rc<MonoRepoCommit>,
    ) -> Self {
        let parents = commit
            .parents
            .iter()
            .map(|parent| match parent {
                MonoRepoParent::OriginalSubmod(original_submod) => {
                    SerdeMonoRepoParent::OriginalSubmod(original_submod.clone())
                }
                MonoRepoParent::Mono(monorepo_commit) => SerdeMonoRepoParent::Mono(
                    commit_ids
                        .get(&RcKey::new(monorepo_commit))
                        .expect("mono commit parents have commit ids")
                        .clone(),
                ),
            })
            .collect();
        Self {
            commit_id: commit_ids
                .get(&RcKey::new(commit))
                .expect("mono commits have commit ids")
                .clone(),
            parents,
            top_bump: commit.top_bump.clone(),
            submodule_bumps: commit.submodule_bumps.clone(),
        }
    }
}
