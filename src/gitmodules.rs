use bstr::ByteSlice;

pub trait SubmoduleUrlExt {
    fn join(&self, other: &Self) -> Self;
}

impl SubmoduleUrlExt for gix_url::Url {
    /// Joins two URLs. If the other URL is a relative path, it is joined to the
    /// base URL path.
    ///
    /// # Examples
    /// ```
    /// use bstr::ByteSlice;
    /// use git_toprepo::gitmodules::SubmoduleUrlExt;
    /// use gix_url;
    ///
    /// /// Join two URLs as strings.
    /// fn join_url_str(base: &str, other: &str) -> String{
    ///     gix_url::parse(base.as_bytes().as_bstr()).unwrap()
    ///         .join(&gix_url::parse(other.as_bytes().as_bstr()).unwrap())
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
        if other.scheme == gix_url::Scheme::File
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
