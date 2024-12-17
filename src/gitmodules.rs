use crate::config::Mapping;
use crate::util::{normalize, RawUrl, Url};
use anyhow::Result;
use bstr::{BStr, BString, ByteSlice};
use std::collections::HashSet;
use std::path::PathBuf;

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
