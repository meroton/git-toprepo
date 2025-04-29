use crate::git::CommitId;
use crate::git_fast_export_import::FastExportCommit;
use crate::git_fast_export_import::FastExportEntry;
use crate::git_fast_export_import::FastExportRepo;
use crate::git_fast_export_import::FastImportCommit;
use crate::git_fast_export_import::FastImportRepo;
use crate::git_fast_export_import::ImportCommitRef;
use anyhow::Result;
use bstr::BString;
use gix::ObjectId;
use itertools::Itertools;
use serde_with::serde_as;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct DedupCacheKey(ObjectId);

impl std::fmt::Display for DedupCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for DedupCacheKey {
    type Err = <ObjectId as std::str::FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let oid = ObjectId::from_str(s)?;
        Ok(Self(oid))
    }
}

/// `GitFastExportImportDedupCache` deduplicates entries that would be exported that
/// have different committer but otherwise are exactly the same.
#[serde_as]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GitFastExportImportDedupCache {
    /// Maps from a hash of a commit, apart from the committer, to the latest
    /// imported or exported commit id.
    #[serde_as(
        serialize_as = "serde_with::IfIsHumanReadable<HashMap<serde_with::DisplayFromStr, serde_with::DisplayFromStr>>"
    )]
    commits: HashMap<DedupCacheKey, CommitId>,
}

/// Commits that are considered fresh and will trigger updates in the dedup cache.
pub struct FastExportRepoDedup<'a> {
    inner: FastExportRepo,
    cache: &'a mut GitFastExportImportDedupCache,
}

impl<'a> FastExportRepoDedup<'a> {
    pub fn new(inner: FastExportRepo, cache: &'a mut GitFastExportImportDedupCache) -> Self {
        Self { inner, cache }
    }

    fn hash_export_entry(entry: &FastExportCommit) -> Result<DedupCacheKey> {
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update_bstring(&entry.author_info);
        // Self::hash_bstring(&entry.committer_info);
        hasher.update_option_bstring(&entry.encoding);
        hasher.update_bstring(&entry.message);
        hasher.update_serde(&entry.file_changes)?;
        hasher.update_serde(&entry.parents)?;
        Ok(DedupCacheKey(hasher.try_finalize()?))
    }
}

impl Iterator for FastExportRepoDedup<'_> {
    type Item = Result<FastExportEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let export_entry = self.inner.next()?;
        Some((|| {
            let export_entry = export_entry?;
            if let FastExportEntry::Commit(commit) = &export_entry {
                let key = Self::hash_export_entry(commit)?;
                // Always overwrite the cache entry with the newest commit with
                // the same key. It makes most sense to reuse the newest commit
                // available.
                self.cache.commits.insert(key, commit.original_id);
            }
            Ok(export_entry)
        })())
    }
}

pub struct FastImportRepoDedup<'a> {
    inner: FastImportRepo,
    cache: &'a mut GitFastExportImportDedupCache,
    written_commits: HashMap<usize, CommitId>,
}

impl<'a> FastImportRepoDedup<'a> {
    pub fn new(inner: FastImportRepo, cache: &'a mut GitFastExportImportDedupCache) -> Self {
        Self {
            inner,
            cache,
            written_commits: HashMap::new(),
        }
    }

    pub fn write_commit(&mut self, commit: &FastImportCommit<'_>) -> Result<ImportCommitRef> {
        let key = self.hash_import_entry(commit)?;
        match self.cache.commits.entry(key) {
            std::collections::hash_map::Entry::Occupied(existing_commit) => {
                // Return a commit id instead of an import marker to refer to
                // the existing commit.
                Ok(ImportCommitRef::CommitId(*existing_commit.get()))
            }
            std::collections::hash_map::Entry::Vacant(dedup_entry) => {
                // This is a new commit, write it and record the resulting
                // commit id.
                let mark = self.inner.write_commit(commit)?;
                // NOTE: Calling get_object_id() synchronously after
                // write_commit() is a bit slow due to the communication between
                // this process and git-fast-import, but just a few imported
                // commits are expected.
                let commit_id = self.inner.get_object_id(mark)?;
                dedup_entry.insert(commit_id);
                self.written_commits.insert(mark, commit_id);
                Ok(ImportCommitRef::Mark(mark))
            }
        }
    }

    fn hash_import_entry(&self, entry: &FastImportCommit<'_>) -> Result<DedupCacheKey> {
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update_bstring(&entry.author_info);
        // Self::hash_bstring(&entry.committer_info);
        hasher.update_option_bstring(&entry.encoding);
        hasher.update_bstring(&entry.message);
        hasher.update_serde(&entry.file_changes)?;
        hasher.update_usize(entry.parents.len());
        let parents = entry
            .parents
            .iter()
            .map(|parent| match parent {
                ImportCommitRef::Mark(mark) => self
                    .written_commits
                    .get(mark)
                    .expect("parent marker marks an already imported commit"),
                ImportCommitRef::CommitId(commit_id) => commit_id,
            })
            .collect_vec();
        hasher.update_serde(&parents)?;
        Ok(DedupCacheKey(hasher.try_finalize()?))
    }
}

trait HasherExt {
    fn update_bstring(&mut self, value: &BString);
    fn update_usize(&mut self, value: usize);
    fn update_option_bstring(&mut self, value: &Option<BString>);
    fn update_serde<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize;
}

impl HasherExt for gix::hash::Hasher {
    fn update_bstring(&mut self, value: &BString) {
        self.update_usize(value.len());
        self.update(value);
    }

    fn update_usize(&mut self, value: usize) {
        // Native endianess is enough as the cache is not meant to be cross-platform.
        self.update(&value.to_ne_bytes());
    }

    fn update_option_bstring(&mut self, value: &Option<BString>) {
        match value {
            Some(inner) => {
                self.update(&[1]);
                self.update_bstring(inner);
            }
            None => self.update(&[0]),
        }
    }

    fn update_serde<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let encoded_value = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
        self.update(&encoded_value);
        Ok(())
    }
}
