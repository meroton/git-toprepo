use anyhow::Context as _;
use anyhow::Result;
use bstr::ByteSlice as _;
use gix::refs::FullNameRef;
use std::ops::Deref;
use std::str::FromStr;

#[derive(
    Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum RepoName {
    Top,
    SubRepo(SubRepoName),
}

impl RepoName {
    pub fn new<T>(s: T) -> Self
    where
        T: Into<String> + AsRef<str>,
    {
        if s.as_ref() == "top" {
            RepoName::Top
        } else {
            RepoName::SubRepo(SubRepoName::new(s.into()))
        }
    }

    /// Converts `refs/namespaces/<name>/*` to `RepoName`.
    pub fn from_ref(fullname: &FullNameRef) -> Result<RepoName> {
        let fullname = fullname.as_bstr();
        let rest = fullname
            .strip_prefix(b"refs/namespaces/")
            .with_context(|| format!("Not a toprepo ref {fullname}"))?;
        let idx = rest
            .find_char('/')
            .with_context(|| format!("Too short toprepo ref {fullname}"))?;
        let name = rest[..idx]
            .to_str()
            .with_context(|| format!("Invalid encoding in ref {fullname}"))?;
        match name {
            "top" => Ok(RepoName::Top),
            _ => Ok(RepoName::SubRepo(SubRepoName::new(name.to_owned()))),
        }
    }

    pub fn to_ref_prefix(&self) -> String {
        // TODO: 2025-09-22 Start using gix::refs::Namespace?
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
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

#[derive(
    Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct SubRepoName(String);

impl SubRepoName {
    pub fn new(name: String) -> Self {
        SubRepoName(name)
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
