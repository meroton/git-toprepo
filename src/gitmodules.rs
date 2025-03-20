use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice;
use bstr::ByteVec;
use std::collections::HashMap;
use std::collections::HashSet;

pub trait SubmoduleUrlExt {
    fn join(&self, other: &Self) -> Self;
}

impl SubmoduleUrlExt for gix::url::Url {
    /// Joins two URLs. If the other URL is a relative path, it is joined to the
    /// base URL path.
    ///
    /// # Examples
    /// ```
    /// use bstr::ByteSlice;
    /// use git_toprepo::gitmodules::SubmoduleUrlExt;
    /// use gix::url;
    ///
    /// /// Join two URLs as strings.
    /// fn join_url_str(base: &str, other: &str) -> String{
    ///     gix::url::parse(base.as_bytes().as_bstr()).unwrap()
    ///         .join(&gix::url::parse(other.as_bytes().as_bstr()).unwrap())
    ///         .to_string()
    /// }
    ///
    /// // Absolute path.
    /// assert_eq!(
    ///     join_url_str("parent", "https://github.com/org/bar"),
    ///     "https://github.com/org/bar",
    /// );
    ///
    /// // Relative paths.
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "."),
    ///     "ssh://github.com/org/repo",
    /// );
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "./"),
    ///     "ssh://github.com/org/repo",
    /// );
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "./foo"),
    ///     "ssh://github.com/org/repo/foo",
    /// );
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "../foo"),
    ///     "ssh://github.com/org/foo",
    /// );
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "../../foo"),
    ///     "ssh://github.com/foo",
    /// );
    ///
    /// // Ignore double slash.
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", ".//foo"),
    ///     "ssh://github.com/org/repo/foo",
    /// );
    ///
    /// // Handle too many '../'.
    /// assert_eq!(
    ///     join_url_str("ssh://github.com/org/repo", "../../../foo"),
    ///     "ssh://github.com/../foo",
    /// );
    /// assert_eq!(
    ///     join_url_str("file:///data/repo", "../../foo"),
    ///     "file:///foo",
    /// );
    /// assert_eq!(
    ///     join_url_str("file:///data/repo", "../../../foo"),
    ///     "file:///../foo",
    /// );
    /// // Without scheme.
    /// assert_eq!(
    ///     join_url_str("parent", "/data/repo"),
    ///     "/data/repo",
    /// );
    /// assert_eq!(
    ///     join_url_str("/data/repo", "../other"),
    ///     "/data/other",
    /// );
    /// ```
    fn join(&self, other: &Self) -> Self {
        if other.scheme == gix::url::Scheme::File
            && other.user().is_none()
            && other.password().is_none()
            && other.host().is_none()
            && (other.path == b"."
                || other.path == b".."
                || other.path.starts_with(b"./")
                || other.path.starts_with(b"../"))
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
/// assert_eq!(join_relative_url_paths("a/b", "./."), "a/b");
/// assert_eq!(join_relative_url_paths("a/b/c", "../.."), "a");
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
        if s.is_empty() {
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

    let mut base = base;
    let mut other = other;
    loop {
        if other.starts_with("/") {
            other = &other[1..];
        } else if other.starts_with("./") {
            other = &other[2..];
        } else if other == "." {
            other = "";
        } else if other.starts_with("../") || other == ".." {
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
            if other == ".." {
                other = "";
            } else {
                other = &other[3..];
            }
        } else {
            break;
        }
    }
    if other.is_empty() {
        let mut ret = base.trim_end_matches(is_path_sep);
        if base.len() > 2 && ret.chars().nth(1) == Some(':') && ret.chars().nth(2).is_none() {
            // Windows drive letter left, keep one (ASCII) separator.
            ret = &base[..3];
        }
        ret.to_owned()
    } else if base.is_empty() || base.ends_with(is_path_sep) {
        base.to_owned() + other
    } else {
        base.to_owned() + "/" + other
    }
}

/// Appends the inner submodule configuration to the outer submodule
/// configuration.
///
/// The name of the sections with the inner submodules are prefixed with
/// `{path}/`.
///
/// Fallbacks are used in case of conclicts in the configuration:
/// * If a section already exists in the outer configuration, it is assumed to
///   be manually handled and therefore skipped.
/// * If the `path` or `url` are missing in the inner configuration, the
///   submodule is skipped.
/// * If the `url` in the inner configuration cannot be parsed, it is assumed to
///   not be relative and used as is without joining onto `base_url`.
///
/// # Examples
/// ```
/// let outer_gitmodules = r#"
/// ## Comment that should be kept.
/// [submodule "submod-outer"]
///     path = submod/outer
///     url = ../../somewhere/org/submod.git
///     branch = .
/// "#;
/// println!("{}", outer_gitmodules);
/// let inner_gitmodules = r#"# First line.
/// [submodule "inner-name"]
///     path = inner/path
///     url = ../../inner.git
///     branch = .
/// ## Another comment that should be kept.
/// "#;
/// let mut outer_config = gix::config::File::from_bytes_owned(
///     &mut outer_gitmodules.as_bytes().to_vec(),
///     gix::config::file::Metadata::default(),
///     Default::default(),
/// ).unwrap();
/// let inner_config = gix::config::File::from_bytes_owned(
///     &mut inner_gitmodules.as_bytes().to_vec(),
///     gix::config::file::Metadata::default(),
///     Default::default(),
/// ).unwrap();
/// git_toprepo::gitmodules::append_inner_submodule_config(
///     &mut outer_config,
///     &gix::url::parse(bstr::BStr::new(b"ssh://server.com/example/outer.git")).unwrap(),
///     bstr::BStr::new(b"submod/dir"),
///     inner_config,
/// );
/// let mut buf: Vec<u8> = Vec::new();
/// outer_config.write_to(&mut buf).unwrap();
/// assert_eq!(
///    String::from_utf8(buf).unwrap(),
///   r#"
/// ## Comment that should be kept.
/// [submodule "submod-outer"]
///     path = submod/outer
///     url = ../../somewhere/org/submod.git
///     branch = .
///
///
/// ## *** Import of submod/dir/.gitmodules ***
///
/// ## First line.
/// [submodule "submod/dir/inner-name"]
///     path = submod/dir/inner/path
///     url = ssh://server.com/inner.git
///     branch = .
/// ## Another comment that should be kept.
/// "#);
/// ```
pub fn append_inner_submodule_config(
    config: &mut gix::config::File<'static>,
    base_url: &gix::url::Url,
    path: &BStr,
    mut inner_config: gix::config::File<'static>,
) {
    // NOTE: If multiple sections point to the same path, simply pick any.
    let existing_path_to_names: HashMap<_, _> = config
        .sections_by_name("submodule")
        .into_iter()
        .flatten()
        .filter_map(|s| {
            s.header().subsection_name().and_then(|n| {
                s.value("path")
                    .map(|path| (path.into_owned(), n.to_owned()))
            })
        })
        .collect();
    let existing_names: HashSet<_> = existing_path_to_names.values().collect();

    let newline = &config.detect_newline_style().to_owned();
    let mut description = Vec::new();
    description.push_str(newline);
    description.push_str(newline);
    description.push_str(b"# *** Import of ");
    description.push_str(path);
    description.push_str(b"/.gitmodules ***");
    description.push_str(newline);

    let mut section_removals = Vec::new();
    let mut section_renames = Vec::new();

    // Avoid borrowing `inner_config` multiple times.
    let section_ids: Vec<_> = inner_config.sections_and_ids().map(|(_, id)| id).collect();
    let inner_name_to_path: HashMap<_, _> = inner_config
        .sections_by_name("submodule")
        .into_iter()
        .flatten()
        .filter_map(|s| {
            s.header().subsection_name().and_then(|n| {
                s.value("path")
                    .map(|path| (n.to_owned(), path.into_owned()))
            })
        })
        .collect();

    for id in section_ids {
        let mut s = inner_config.section_mut_by_id(id).expect("id in loop");
        match s.header().subsection_name() {
            Some(inner_name) if s.header().name() == b"submodule" => {
                let full_name = BString::new(bstr::concat([path, b"/".into(), inner_name]));
                if existing_names.contains(&full_name) {
                    // Skip sections that already exist in the outer configuration.
                    // The user has probably added the information manually.
                    description.push_str(b"# Section already exists: ");
                    description.push_str(full_name);
                    description.push_str(newline);
                    description.push_str(b"# Skipping section: ");
                    description.push_str(inner_name);
                    description.push_str(newline);
                    section_removals.push(id);
                    continue;
                }

                // Note that it is not enough to check if inner_section has a `path`
                // entry because there might be multiple sections with the same path.
                let inner_path = match inner_name_to_path.get(inner_name) {
                    Some(p) => p,
                    None => {
                        // Skip sections without a `path` field.
                        description
                            .push_str(b"# Submodule path missing in inner .gitmodules file.");
                        description.push_str(newline);
                        description.push_str(b"# Skipping section: ");
                        description.push_str(inner_name);
                        description.push_str(newline);
                        section_removals.push(id);
                        continue;
                    }
                };
                let full_path =
                    BString::new(bstr::concat([path, "/".into(), inner_path.as_bstr()]));
                if existing_path_to_names.contains_key(&full_path) {
                    // Skip sections that already exist in the outer configuration.
                    // The user has probably added the information manually.
                    description.push_str(b"# Submodule path already described: ");
                    description.push_str(full_path);
                    description.push_str(newline);
                    description.push_str(b"# Skipping section: ");
                    description.push_str(inner_name);
                    description.push_str(newline);
                    section_removals.push(id);
                    continue;
                }

                section_renames.push((inner_name.to_owned(), full_name.to_owned()));

                // Prefix path.
                if let Some(inner_path) = s.value("path") {
                    let full_path = bstr::concat([path, b"/".into(), &inner_path]);
                    s.set(
                        "path".try_into().expect("known valid key"),
                        full_path.as_bstr(),
                    );
                }
                // Prefix url.
                if let Some(inner_url) = s.value("url") {
                    let full_url = match gix::url::parse(inner_url.as_bstr()) {
                        Ok(u) => base_url.join(&u).to_bstring(),
                        Err(_) => inner_url.into_owned(),
                    };
                    s.set(
                        "url".try_into().expect("known valid key"),
                        full_url.as_bstr(),
                    );
                }
            }
            subname => {
                // Section not related to submodules.
                let name = s.header().name();
                description.push_str(b"# Skipping unused section: ");
                description.push_str(name);
                if let Some(subname) = subname {
                    description.push_str(b".");
                    description.push_str(subname);
                }
                description.push_str(newline);
                section_removals.push(id);
            }
        };
    }
    description.push_str(newline);

    for id in section_removals {
        inner_config.remove_section_by_id(id);
    }
    for (inner_name, full_name) in section_renames {
        inner_config
            .rename_section(
                "submodule",
                inner_name.as_bstr(),
                "submodule",
                Some(std::borrow::Cow::Owned(full_name)),
            )
            .expect("known valid section name and subsection name");
    }

    let description_config = gix::config::File::from_bytes_owned(
        &mut description,
        gix::config::file::Metadata::default(),
        Default::default(),
    )
    .expect("known comment only git-config");
    config.append(description_config);
    config.append(inner_config);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_configs_and_append(
        outer_gitmodules: &str,
        base_url: &[u8],
        path: &[u8],
        inner_gitmodules: &str,
    ) -> (gix::config::File<'static>, gix::config::File<'static>) {
        let mut outer_config = gix::config::File::from_bytes_owned(
            &mut outer_gitmodules.as_bytes().to_vec(),
            gix::config::file::Metadata::default(),
            Default::default(),
        )
        .unwrap();
        let inner_config = gix::config::File::from_bytes_owned(
            &mut inner_gitmodules.as_bytes().to_vec(),
            gix::config::file::Metadata::default(),
            Default::default(),
        )
        .unwrap();
        append_inner_submodule_config(
            &mut outer_config,
            &gix::url::parse(bstr::BStr::new(base_url)).unwrap(),
            bstr::BStr::new(path),
            inner_config.clone(),
        );

        (outer_config, inner_config)
    }

    // Key-value pairs (e.g. `path` and `branch`) are tested in the doc test for `append_inner_submodule_config`.

    #[test]
    fn test_urls() {
        let outer_gitmodules_relative = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = ../../outer.git
    branch = .
"#;
        let outer_gitmodules_absolute = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = https://server/abs/path/outer.git
    branch = .
"#;
        let inner_gitmodules = r#"
[submodule "inner-name-relative"]
    path = inner/path/relative
    url = ../../inner.git
    branch = .
[submodule "inner-name-absolute"]
    path = inner/path/absolute
    url = https://server/abs/path/inner.git
    branch = .
"#;

        let (outer_config_relative, _) = create_configs_and_append(
            outer_gitmodules_relative,
            b"ssh://server.com/example/outer.git",
            b"submod/dir",
            inner_gitmodules,
        );
        let (outer_config_absolute, _) = create_configs_and_append(
            outer_gitmodules_absolute,
            b"ssh://server.com/example/outer.git",
            b"submod/dir",
            inner_gitmodules,
        );

        assert_eq!(
            outer_config_relative
                .section("submodule", Some("submod-outer".into()))
                .unwrap()
                .value("url")
                .unwrap()
                .to_string(),
            "../../outer.git"
        );
        assert_eq!(
            outer_config_absolute
                .section("submodule", Some("submod-outer".into()))
                .unwrap()
                .value("url")
                .unwrap()
                .to_string(),
            "https://server/abs/path/outer.git"
        );
        assert_eq!(
            outer_config_relative
                .section("submodule", Some("submod/dir/inner-name-relative".into()))
                .unwrap()
                .value("url")
                .unwrap()
                .to_string(),
            "ssh://server.com/inner.git"
        );
        assert_eq!(
            // `outer_config_relative` and `outer_config_absolute` have identical inner submodules.
            outer_config_relative
                .section("submodule", Some("submod/dir/inner-name-absolute".into()))
                .unwrap()
                .value("url")
                .unwrap()
                .to_string(),
            "https://server/abs/path/inner.git"
        );
    }

    #[test]
    fn test_append_existing_section() {
        let outer_gitmodules = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = ../../somewhere/org/submod.git
    branch = .
"#;
        let inner_gitmodules = r#"
[submodule "inner"]
    path = inner
    url = ../../inner.git
    branch = .
"#;

        let (mut outer_config, inner_config) = create_configs_and_append(
            outer_gitmodules,
            b"ssh://server.com/example/outer.git",
            b"submod/dir",
            inner_gitmodules,
        );
        // Append `inner_gitmodules` twice (first appended in `create_configs_and_append`)
        append_inner_submodule_config(
            &mut outer_config,
            &gix::url::parse(bstr::BStr::new(b"ssh://server.com/example/outer.git")).unwrap(),
            bstr::BStr::new(b"submod/dir"),
            inner_config,
        );

        assert!(
            outer_config
                .to_string()
                .contains("\n# Section already exists: submod/dir/inner\n")
        );
    }

    #[test]
    fn test_append_missing_submodule_path() {
        let outer_gitmodules = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = ../../somewhere/org/submod.git
    branch = .
"#;
        // Missing `path` variable in `inner_gitmodules`
        let inner_gitmodules = r#"
[submodule "inner"]
    url = ../../inner.git
    branch = .
"#;

        let (outer_config, _) = create_configs_and_append(
            outer_gitmodules,
            b"ssh://server.com/example/outer.git",
            b"submod/dir",
            inner_gitmodules,
        );

        assert!(
            outer_config
                .to_string()
                .contains("\n# Submodule path missing in inner .gitmodules file.\n")
        );
    }

    #[test]
    fn test_append_existing_path() {
        let outer_gitmodules = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = ../../somewhere/org/submod.git
    branch = .
"#;
        // Variable `path` in `inner_gitmodules` will be evaluated to `submod/outer`, which already exists in `outer_gitmodules`
        let inner_gitmodules = r#"
[submodule "inner"]
    path = outer
    url = ../../inner.git
    branch = .
"#;

        let (outer_config, _) = create_configs_and_append(
            outer_gitmodules,
            b"ssh://server.com/example/outer.git",
            b"submod",
            inner_gitmodules,
        );

        assert!(
            outer_config
                .to_string()
                .contains("\n# Submodule path already described: submod/outer\n")
        );
    }

    #[test]
    fn test_append_unused_section() {
        let outer_gitmodules = r#"
[submodule "submod-outer"]
    path = submod/outer
    url = ../../somewhere/org/submod.git
    branch = .
"#;
        // Non-submodule header in `inner_gitmodules`
        let inner_gitmodules = r#"
[foo "bar"]
    path = inner
    url = ../../inner.git
    branch = .
"#;

        let (outer_config, _) = create_configs_and_append(
            outer_gitmodules,
            b"ssh://server.com/example/outer.git",
            b"submod",
            inner_gitmodules,
        );

        assert!(
            outer_config
                .to_string()
                .contains("\n# Skipping unused section: foo.bar\n")
        );
    }
}
