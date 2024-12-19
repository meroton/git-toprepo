use crate::config::Mapping;
use crate::util::{normalize, RawUrl, Url};
use anyhow::Result;
use bstr::{BStr, BString, ByteSlice};
use gix::remote::Direction;
use gix::validate::path::component;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GitModuleInfo {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub url: Url,
    pub raw_url: RawUrl,
}

/// Headers and keys for parsing `.gitmodule`
const SUBMODULE_HEADER: &str = "submodule";
const GIT_MODULE_PATH: &str = "path";
const GIT_MODULE_URL: &str = "url";
const GIT_MODULE_BRANCH: &str = "branch";

#[derive(Debug)]
struct RelativeGerritProject(BString);

#[derive(Debug)]
/// NB: `gix-config` parses to `Bstr` for us.
/// This is described well here: https://blog.burntsushi.net/bstr/#motivation-based-on-concepts
pub struct Submodule {
    pub path: BString,
    pub project: BString,
    branch: Option<BString>,
}

pub struct RawSubmodule {
    pub path: BString,
    project: RelativeGerritProject,
    branch: Option<BString>,
}

pub fn resolve_submodules(subs: Vec<RawSubmodule>, main_project: String) -> Result<Vec<Submodule>> {
    let mut resolved: Vec<Submodule> = Vec::new();

    for module in subs {
        // TODO: Nightly `as_str`: https://docs.rs/bstr/latest/bstr/struct.BString.html#deref-methods-%5BT%5D-1
        let burrow: &BStr = module.project.0.as_ref();
        let burrow: &str = burrow.to_str()?;
        let project = normalize(&format!("{}/{}", &main_project, burrow));

        resolved.push(Submodule {
            path: module.path,
            project: project.into(),
            branch: module.branch,
        });
    }

    Ok(resolved)
}

pub trait SubmoduleUrlExt {
    fn join(&self, other: &Self) -> Self;
}

impl SubmoduleUrlExt for gix_url::Url {
    /// Joins two URLs. If the other URL is a relative path, it is joined to the
    /// base URL path.
    ///
    /// # Examples
    /// ```
    /// use git_toprepo::gitmodules::SubmoduleUrlExt;
    /// use gix_url;
    ///
    /// let base = gix_url::parse(b"ssh://github.com/org/repo".into()).unwrap();
    /// let other = gix_url::parse(b"../foo".into()).unwrap();
    /// assert_eq!(base.join(&other), gix_url::parse(b"ssh://github.com/org/foo".into()).unwrap());
    ///
    /// let base = gix_url::parse(b"ssh://github.com/org/foo".into()).unwrap();
    /// let other = gix_url::parse(b"https://github.com/org/bar".into()).unwrap();
    /// assert_eq!(base.join(&other), gix_url::parse(b"https://github.com/org/bar".into()).unwrap());
    /// ```
    fn join(&self, other: &Self) -> Self {
        if other.scheme == gix_url::Scheme::File
            && other.user().is_none()
            && other.password().is_none()
            && other.host().is_none()
            && (other.path.starts_with(b"./") || other.path.starts_with(b"../"))
        {
            if let Ok(self_path_str) = self.path.to_str() {
                if let Ok(other_path_str) = other.path.to_str() {
                    let mut ret = self.clone();
                    ret.path = join_relative_url_paths(self_path_str, other_path_str).into();
                    return ret;
                }
            }
        }
        other.clone()
    }
}

/// Joins a base URL with a relative URL path.
///
/// Note that path separators for the base can be either `/`, `\` or `:` for the
/// Windows drive letter. All these are checked on all operating systems because
/// the URL resolution is done in the context of the users host.
///
/// The other path is always assumed to use `/` as separator.
///
/// # Examples
/// ```
/// use git_toprepo::gitmodules::join_relative_url_paths;
///
/// assert_eq!(join_relative_url_paths("a/b", "c/d"), "a/b/c/d");
/// assert_eq!(join_relative_url_paths("a/b", "./c/d"), "a/b/c/d");
/// assert_eq!(join_relative_url_paths("a/b", "../c/d"), "a/c/d");
/// assert_eq!(join_relative_url_paths("a/b", "../../c/d"), "c/d");
/// assert_eq!(join_relative_url_paths("a/b", "../c/../d"), "a/c/../d");
/// assert_eq!(join_relative_url_paths("a/b/", "c/d"), "a/b/c/d");
/// assert_eq!(join_relative_url_paths("a", "../c/d"), "c/d");
/// assert_eq!(join_relative_url_paths("a", "../../c/d"), "../c/d");
/// assert_eq!(join_relative_url_paths("./a", "../../c/d"), "./../c/d");
/// assert_eq!(join_relative_url_paths("/../a", "../../c/d"), "/../../c/d");
/// assert_eq!(join_relative_url_paths("/a/b", "../../../c/d"), "/../c/d");
/// assert_eq!(join_relative_url_paths(r"a\b\c", "../d/e"), r"a\b\d/e");
/// assert_eq!(join_relative_url_paths(r"C:\a", "../../c/d"), r"C:\../c/d");
/// ```
pub fn join_relative_url_paths(base: &str, other: &str) -> String {
    fn is_path_sep(c: char) -> bool {
        c == '/' || c == '\\'
    }

    /// Find the index of the last path separator in the string.
    fn strip_last_component(s: &str) -> Option<(&str, &str)> {
        let s = s.trim_end_matches(is_path_sep);
        if s.len() == 0 {
            // No component to remove.
            return None;
        }
        match s.rfind(is_path_sep) {
            Some(i) => {
                let component = &s[i + 1..];
                let parent = &s[..=i];
                Some((parent, component))
            }
            None => {
                match (s.chars().nth(1), s.chars().nth(2)) {
                    // Windows drive letter left.
                    (Some(':'), None) => None,
                    // One component to strip.
                    _ => Some(("", s)),
                }
            }
        }
    }

    let full_base = base;
    let mut base = base;
    let mut other = other;
    loop {
        if other.starts_with("/") {
            other = &other[1..];
        } else if other.starts_with("./") {
            other = &other[2..];
        } else if other.starts_with("../") {
            match strip_last_component(base) {
                Some((parent, component)) => {
                    if component == ".." || component == "." {
                        // ../ and ./ are not handled.
                        break;
                    }
                    base = parent;
                }
                None => break,
            }
            other = &other[3..];
        } else {
            break;
        }
    }
    if base.is_empty() || other.is_empty() || base.ends_with(is_path_sep) {
        base.to_owned() + other
    } else {
        base.to_owned() + "/" + other
    }
}

pub fn doit(repo_dir: &Path) -> Result<()> {
    println!("url: {:?}", gix_url::parse(b"../foo/bar.git".into()));
    println!("url: {:?}", gix_url::parse(b"./foo/bar.git".into()));
    println!("url: {:?}", gix_url::parse(b"/foo/bar.git".into()));
    println!("url: {:?}", gix_url::parse(b"..\\foo\\bar.git".into()));
    println!("url: {:?}", gix_url::parse(b".\\foo\\bar.git".into()));
    println!("url: {:?}", gix_url::parse(b"C:\\foo\\bar.git".into()));
    let repo = gix::open(repo_dir)?;
    let default_remote = repo.find_default_remote(Direction::Fetch).unwrap()?;
    let toprepo_url = default_remote.url(Direction::Fetch).unwrap();

    let dot_gitmodules_path = repo_dir.join(".gitmodules");
    let dot_gitmodules_bytes = std::fs::read(&dot_gitmodules_path).or_else(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Ok(Vec::new()),
        _ => Err(e),
    })?;
    let submod = gix_submodule::File::from_bytes(
        &dot_gitmodules_bytes,
        dot_gitmodules_path,
        &Default::default(),
    )?;
    // println!("{:?}", submod);
    submod.names().for_each(|name| {
        let path = submod.path(name).unwrap();
        let url = submod.url(name).unwrap();
        let branch = submod.branch(name);
        println!("{}: {} {} {:?}", name, path, toprepo_url.join(&url), branch);
    });
    Ok(())
}

#[allow(unused)]
pub fn parse_submodules(gitmodules: PathBuf) -> Result<Vec<RawSubmodule>> {
    let modules = gix_config::File::from_path_no_includes(gitmodules, gix_config::Source::Worktree);

    let mut res: Vec<RawSubmodule> = Vec::new();

    for section in modules?.sections() {
        let header = section.header().name(); // Category
        let subheader = section.header().subsection_name();

        if header == SUBMODULE_HEADER {
            let module_path = subheader.expect("Could not unpack submodule info");
            let body = section.body();

            let path = body.value(GIT_MODULE_PATH).unwrap().into_owned();
            // TODO: We tend to use relative Gerrit project urls.
            // This parsing must be updated to also support the regular formats.
            let url = body.value(GIT_MODULE_URL).unwrap().into_owned();
            let branch = body.value(GIT_MODULE_BRANCH).map(|b| b.into_owned());

            let project = match url.strip_suffix(b".git") {
                None => url,
                Some(p) => p.into(),
            };

            res.push(RawSubmodule {
                branch,
                path,
                project: RelativeGerritProject(project),
            });
        }
    }

    Ok(res)
}

pub fn get_gitmodules_info(
    submod_config_mapping: Mapping,
    parent_url: &str,
) -> Result<Vec<GitModuleInfo>> {
    let mut configs = Vec::new();
    let mut used = HashSet::new();

    for (name, configmap) in submod_config_mapping {
        let raw_url = configmap.get_singleton("url").unwrap().to_string();
        let resolved_url = join_submodule_url(parent_url, &raw_url);
        let path = configmap.get_singleton("path").unwrap();

        let submod_info = GitModuleInfo {
            name,
            path: PathBuf::from(path),
            branch: configmap.get_singleton("branch").map(|s| s.to_string()),
            url: resolved_url,
            raw_url,
        };

        if used.insert(path.to_owned()) {
            panic!("Duplicate submodule configs for '{}'", path);
        }
        configs.push(submod_info);
    }

    Ok(configs)
}

// TODO: check and improve + add doc tests
pub fn join_submodule_url(parent: &str, mut other: &str) -> String {
    if other.starts_with("./") || other.starts_with("../") || other == "." {
        let scheme_end = match parent.find("://") {
            Some(i) => i + 3,
            None => 0,
        };
        let (scheme, parent) = parent.split_at(scheme_end);
        let mut parent = parent.trim_end_matches("/").to_string();

        loop {
            if other.starts_with("/") {
                (_, other) = other.split_at(1);
            } else if other.starts_with("./") {
                (_, other) = other.split_at(2);
            } else if other.starts_with("../") {
                match parent.rfind("/") {
                    Some(i) => {
                        parent.drain(i..);
                    }

                    //Too many "../", move it from other to parent.
                    None => parent += "/..",
                }

                (_, other) = other.split_at(3);
            } else {
                break;
            }
        }

        return if other == "." || other.is_empty() {
            format!("{}{}", scheme, parent)
        } else {
            format!("{}{}/{}", scheme, parent, other)
        };
    }

    other.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_submodule_url() {
        // Relative.
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "."),
            "https://github.com/org/repo"
        );
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "./"),
            "https://github.com/org/repo"
        );
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "./foo"),
            "https://github.com/org/repo/foo"
        );
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "../foo"),
            "https://github.com/org/foo"
        );
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "../../foo"),
            "https://github.com/foo"
        );

        // Ignore double slash.
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", ".//foo"),
            "https://github.com/org/repo/foo"
        );

        // Handle too many '../'.
        assert_eq!(
            join_submodule_url("https://github.com/org/repo", "../../../foo"),
            "https://github.com/../foo"
        );
        assert_eq!(
            join_submodule_url("file:///data/repo", "../../foo"),
            "file:///foo"
        );
        assert_eq!(
            join_submodule_url("file:///data/repo", "../../../foo"),
            "file:///../foo"
        );

        // Absolute.
        assert_eq!(
            join_submodule_url("parent", "ssh://github.com/org/repo"),
            "ssh://github.com/org/repo"
        );

        // Without scheme.
        assert_eq!(join_submodule_url("parent", "/data/repo"), "/data/repo");
        assert_eq!(join_submodule_url("/data/repo", "../other"), "/data/other");
    }
}
