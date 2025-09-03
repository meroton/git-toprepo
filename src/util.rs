use crate::git::CommitId;
use anyhow::Result;
use anyhow::bail;
use bstr::ByteSlice as _;
use bstr::ByteVec;
use gix::discover::upwards;
use itertools::Itertools;
use serde::Deserialize as _;
use serde::Serialize as _;
use serde_with::DeserializeAs;
use serde_with::SerializeAs;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::ops::Deref;
use std::ops::DerefMut;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitStatus;
use std::sync::atomic::AtomicBool;

pub type RawUrl = String;
pub type Url = String;

lazy_static::lazy_static! {
    /// A URL that serializes to an empty string.
    pub static ref EMPTY_GIX_URL: gix::Url = new_empty_gix_url();
}

pub fn find_current_worktree(relative_to: &Path) -> Result<PathBuf> {
    let path = upwards(relative_to)?
        .0
        .into_repository_and_work_tree_directories()
        .1
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Couldn't find git directory",
        ))?
        .to_owned();
    Ok(path)
}

pub fn find_main_worktree(relative_to: &Path) -> Result<PathBuf> {
    let dotgit = upwards(relative_to)?
        .0
        .into_repository_and_work_tree_directories()
        .0
        .to_path_buf();
    let dotgit_parent = dotgit
        .parent()
        .ok_or(anyhow::anyhow!("Git repository has no worktree"))?;
    let main_worktree = upwards(dotgit_parent)?
        .0
        .into_repository_and_work_tree_directories()
        .1
        .ok_or(anyhow::anyhow!("Git repository has no worktree"))?
        .to_path_buf();
    Ok(main_worktree)
}

/// Creates a `gix::Url` that serializes to an empty string.
fn new_empty_gix_url() -> gix::Url {
    let mut empty_url: gix::Url = Default::default();
    empty_url.scheme = gix::url::Scheme::File;
    empty_url = empty_url.serialize_alternate_form(true);

    debug_assert_eq!(empty_url.to_bstring(), b"");

    empty_url
}

pub(crate) struct SerdeGixUrl;

impl SerializeAs<gix::Url> for SerdeGixUrl {
    fn serialize_as<S>(source: &gix::Url, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if *source == *EMPTY_GIX_URL {
            let s: &str = "";
            s.serialize(serializer)
        } else {
            let bs = source.to_bstring();
            let s: &str = bs
                .to_str()
                .map_err(|err| serde::ser::Error::custom(format!("Invalid URL {source}: {err}")))?;
            s.serialize(serializer)
        }
    }
}

impl<'de> DeserializeAs<'de, gix::Url> for SerdeGixUrl {
    fn deserialize_as<D>(deserializer: D) -> Result<gix::Url, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.is_empty() {
            Ok(EMPTY_GIX_URL.clone())
        } else {
            let url = gix::Url::from_bytes(s.as_bytes().as_bstr())
                .map_err(|err| serde::de::Error::custom(format!("Invalid URL {s}: {err}")))?;
            Ok(url)
        }
    }
}

impl SerializeAs<Option<gix::Url>> for SerdeGixUrl {
    fn serialize_as<S>(source: &Option<gix::Url>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match source {
            Some(url) => {
                if url.to_bstring().is_empty() {
                    return Err(serde::ser::Error::custom("Empty optonal URL not allowed"));
                }
                SerdeGixUrl::serialize_as(url, serializer)
            }
            None => serializer.serialize_str(""),
        }
    }
}

impl<'de> DeserializeAs<'de, Option<gix::Url>> for SerdeGixUrl {
    fn deserialize_as<D>(deserializer: D) -> Result<Option<gix::Url>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let url: gix::Url = SerdeGixUrl::deserialize_as(deserializer)?;
        if url != *EMPTY_GIX_URL {
            Ok(Some(url))
        } else {
            Ok(None)
        }
    }
}

pub(crate) struct SerdeOctalNumber;

impl SerializeAs<u32> for SerdeOctalNumber {
    fn serialize_as<S>(source: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            let source_str = format!("{source:o}");
            source_str.serialize(serializer)
        } else {
            source.serialize(serializer)
        }
    }
}

impl<'de> DeserializeAs<'de, u32> for SerdeOctalNumber {
    fn deserialize_as<D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = String::deserialize(deserializer)?;
            u32::from_str_radix(&s, 8)
                .map_err(|err| serde::de::Error::custom(format!("Invalid octal number {s}: {err}")))
        } else {
            u32::deserialize(deserializer)
        }
    }
}

/// A wrapper around `HashMap<K, V>` that serializes the keys in sorted order.
/// This is useful when comparing JSON serialized data.
pub(crate) struct OrderedHashMap<K, V> {
    phantom: std::marker::PhantomData<(K, V)>,
}

impl<K, KAs, V, VAs> SerializeAs<HashMap<K, V>> for OrderedHashMap<KAs, VAs>
where
    K: Hash + Ord,
    KAs: serde_with::SerializeAs<K>,
    VAs: serde_with::SerializeAs<V>,
{
    fn serialize_as<S>(source: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let vec = source
            .iter()
            .sorted_by(|(k_lhs, _), (k_rhs, _)| k_lhs.cmp(k_rhs))
            .collect_vec();
        <serde_with::Map<&KAs, &VAs> as SerializeAs<Vec<(&K, &V)>>>::serialize_as(&vec, serializer)
    }
}

impl<'de, K, KAs, V, VAs> DeserializeAs<'de, HashMap<K, V>> for OrderedHashMap<KAs, VAs>
where
    K: Hash + Eq,
    KAs: serde_with::DeserializeAs<'de, K>,
    VAs: serde_with::DeserializeAs<'de, V>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <HashMap<KAs, VAs> as DeserializeAs<'de, HashMap<K, V>>>::deserialize_as(deserializer)
    }
}

/// A wrapper around `HashSet<T>` that serializes the entries in sorted order.
/// This is useful when comparing JSON serialized data.
pub(crate) struct OrderedHashSet<T> {
    phantom: std::marker::PhantomData<T>,
}

impl<T, TAs> SerializeAs<HashSet<T>> for OrderedHashSet<TAs>
where
    T: Hash + Ord,
    TAs: serde_with::SerializeAs<T>,
{
    fn serialize_as<S>(source: &HashSet<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let vec = source
            .iter()
            .sorted_by(|lhs, rhs| lhs.cmp(rhs))
            .collect_vec();
        <Vec<&TAs> as SerializeAs<Vec<&T>>>::serialize_as(&vec, serializer)
    }
}

impl<'de, T, TAs> DeserializeAs<'de, HashSet<T>> for OrderedHashSet<TAs>
where
    T: Hash + Eq,
    TAs: serde_with::DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<HashSet<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <HashSet<TAs> as DeserializeAs<'de, HashSet<T>>>::deserialize_as(deserializer)
    }
}

/// Consumes an iterator to the end to check if it is non-empty and all the
/// elements are equal.
pub trait IterSingleUnique<T> {
    fn single_unique(self) -> Option<T>;
}

impl<I, T> IterSingleUnique<T> for I
where
    I: IntoIterator<Item = T>,
    T: PartialEq,
{
    /// Returns the first element of an iterator if all elements are equal.
    /// Otherwise, returns None.
    ///
    /// ```
    /// use git_toprepo::util::IterSingleUnique as _;
    ///
    /// assert_eq!(Vec::<i32>::new().single_unique(), None);
    /// assert_eq!(vec![1].single_unique(), Some(1));
    /// assert_eq!(vec![1,1,1].single_unique(), Some(1));
    /// assert_eq!(vec![1,2,1].single_unique(), None);
    /// ```
    fn single_unique(self) -> Option<T> {
        single_unique(self)
    }
}

pub fn single_unique<I, T>(items: I) -> Option<T>
where
    I: IntoIterator<Item = T>,
    T: PartialEq,
{
    let mut iter = items.into_iter();
    let first = iter.next()?;
    for item in iter {
        if item != first {
            return None;
        }
    }
    Some(first)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// A container that can hold either no entry or a single entry.
pub enum UniqueContainer<T> {
    /// No entry in the container.
    #[default]
    Empty,
    /// Only a single entry has been added.
    Single(T),
    /// Multiple different entries have been added, the last one is inserted.
    Multiple,
}

impl<T> UniqueContainer<T>
where
    T: PartialEq,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the first element of an iterator if all elements are equal.
    /// Otherwise, returns None.
    ///
    /// ```
    /// use git_toprepo::util::UniqueContainer;
    ///
    /// let mut container = UniqueContainer::Empty;
    /// container.insert(1);
    /// assert_eq!(container, UniqueContainer::Single(1));
    /// container.insert(1);
    /// assert_eq!(container, UniqueContainer::Single(1));
    /// container.insert(2);
    /// assert_eq!(container, UniqueContainer::Multiple);
    /// container.insert(1);
    /// assert_eq!(container, UniqueContainer::Multiple);
    /// container.insert(3);
    /// assert_eq!(container, UniqueContainer::Multiple);
    /// ```
    pub fn insert(&mut self, item: T) {
        match self {
            UniqueContainer::Empty => {
                *self = UniqueContainer::Single(item);
            }
            UniqueContainer::Single(first) => {
                if *first != item {
                    *self = UniqueContainer::Multiple;
                }
            }
            UniqueContainer::Multiple => {}
        }
    }
}

/// Normalize a path in the abstract, without filesystem accesses.
///
/// This is not guaranteed to give correct paths,
/// Notably, it will be incorrect in the presence of mounts or symlinks.
/// But if the paths are known to be free of links,
/// this is faster than `realpath(3)` et al.
///
/// ```
/// assert_eq!(git_toprepo::util::normalize("A/b/../C"), "A/C");
/// assert_eq!(git_toprepo::util::normalize("B/D"), "B/D");
/// assert_eq!(git_toprepo::util::normalize("E//./F"), "E/F");
/// ```
pub fn normalize(p: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    let parts = p.split("/");
    for p in parts {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." {
            stack.pop();
        } else {
            stack.push(p)
        }
    }

    stack.into_iter().map(|s| s.to_owned()).join("/")
}

pub trait CommandExtension {
    fn safe_output(&mut self) -> std::io::Result<SafeOutput>;
    fn safe_status(&mut self) -> std::io::Result<SafeExitStatus>;

    fn check_success_with_stderr(&mut self) -> anyhow::Result<SafeOutput> {
        let ret = self.safe_output()?;
        ret.check_success_with_stderr()?;
        Ok(ret)
    }
}

impl CommandExtension for Command {
    fn safe_output(&mut self) -> std::io::Result<SafeOutput> {
        self.output().map(|output| {
            let status = SafeExitStatus::new(output.status);
            SafeOutput { output, status }
        })
    }

    fn safe_status(&mut self) -> std::io::Result<SafeExitStatus> {
        self.status().map(SafeExitStatus::new)
    }
}

pub struct SafeOutput {
    output: std::process::Output,
    pub status: SafeExitStatus,
}

pub struct SafeExitStatus {
    status: ExitStatus,
    retreived: AtomicBool,
}

impl SafeExitStatus {
    pub fn new(status: ExitStatus) -> Self {
        SafeExitStatus {
            status,
            retreived: AtomicBool::new(false),
        }
    }

    pub fn check_success(&self) -> anyhow::Result<&Self> {
        if !self.success() {
            bail!("{self}");
        }
        Ok(self)
    }
}

impl Drop for SafeExitStatus {
    fn drop(&mut self) {
        if !self.retreived.load(std::sync::atomic::Ordering::Acquire) {
            panic!("SafeOutput dropped without status being retrieved");
        }
    }
}

impl Deref for SafeExitStatus {
    type Target = ExitStatus;

    fn deref(&self) -> &Self::Target {
        self.retreived
            .store(true, std::sync::atomic::Ordering::Release);
        &self.status
    }
}

impl std::fmt::Display for SafeExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.status.fmt(f)
    }
}

impl SafeOutput {
    /// Checks that the command was successful and otherwise returns an error
    /// with the exit status together with the stderr content.
    pub fn check_success_with_stderr(&self) -> anyhow::Result<&Self> {
        // TODO: Print the command line as well?
        if !self.status.success() {
            if self.stderr.is_empty() {
                bail!("{}", self.status);
            } else {
                bail!(
                    "{}:\n{}",
                    self.status,
                    String::from_utf8_lossy(&self.stderr)
                );
            }
        }
        Ok(self)
    }
}

impl Deref for SafeOutput {
    type Target = std::process::Output;

    fn deref(&self) -> &Self::Target {
        &self.output
    }
}

impl DerefMut for SafeOutput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.output
    }
}

pub struct ReadLossyCrOrLfLines<R> {
    reader: R,
    buf: Vec<u8>,
}

impl<R: std::io::BufRead> ReadLossyCrOrLfLines<R> {
    pub fn new(reader: R) -> Self {
        ReadLossyCrOrLfLines {
            reader,
            buf: Vec::new(),
        }
    }
}

impl<R: std::io::BufRead> Iterator for ReadLossyCrOrLfLines<R> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        while let Ok(bytes) = self.reader.fill_buf() {
            if bytes.is_empty() {
                // End of file.
                if self.buf.is_empty() {
                    return None;
                }
            } else if let Some(idx) = bytes.find_byteset(b"\r\n") {
                self.buf.push_str(bytes[..idx + 1].as_bstr());
                self.reader.consume(idx + 1);
            } else {
                self.buf.push_str(bytes.as_bstr());
                let bytes_len = bytes.len();
                self.reader.consume(bytes_len);
                // No CR or LF found, get some more bytes.
                continue;
            }
            let line_str = self.buf.to_str_lossy().to_string();
            self.buf.clear();
            return Some(line_str);
        }
        None
    }
}

/// Reads for example the stderr of a process and sends each line to a callback,
/// with CR or LF stripped. All text that was not erased with CR will be
/// returned.
pub fn read_stderr_progress_status<R, F>(input: R, status_callback: F) -> String
where
    R: std::io::Read,
    F: Fn(String),
{
    let stderr_reader = std::io::BufReader::new(input);
    let mut permanent_text = String::new();
    for mut line in crate::util::ReadLossyCrOrLfLines::new(stderr_reader) {
        if line.ends_with('\r') {
            if let Some(eol_idx) = permanent_text.rfind('\n') {
                permanent_text.truncate(eol_idx + 1);
            } else {
                permanent_text.clear();
            }
            line.pop();
        } else {
            permanent_text += &line;
            if line.ends_with('\n') {
                line.pop();
            }
        }
        status_callback(line);
    }
    permanent_text
}

/// Returns true if the given value is the default value for the type.
///
/// # Examples
/// ```
/// use git_toprepo::util::is_default;
///
/// use serde::Serialize;
/// #[derive(Serialize)]
/// pub struct Config {
///     #[serde(skip_serializing_if = "is_default")]
///     value: i32,
/// }
/// ```
pub fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

/// Removes trailing LF or CRLF from a string.
///
/// # Examples
/// ```
/// use git_toprepo::util::trim_newline_suffix;
///
/// assert_eq!(trim_newline_suffix("foo"), "foo");
/// assert_eq!(trim_newline_suffix("foo\n"), "foo");
/// assert_eq!(trim_newline_suffix("foo\r\n"), "foo");
/// assert_eq!(trim_newline_suffix("foo\nbar\n"), "foo\nbar");
/// assert_eq!(trim_newline_suffix("foo\r\nbar\r\n"), "foo\r\nbar");
///
/// assert_eq!(trim_newline_suffix("foo\n\r"), "foo\n\r");
/// ```
pub fn trim_newline_suffix(line: &str) -> &str {
    let Some(line) = line.strip_suffix('\n') else {
        return line;
    };
    let Some(line) = line.strip_suffix('\r') else {
        return line;
    };
    line
}

/// Removes trailing LF or CRLF from a byte string.
///
/// # Examples
/// ```
/// use git_toprepo::util::trim_bytes_newline_suffix;
///
/// assert_eq!(trim_bytes_newline_suffix(b"foo"), b"foo");
/// assert_eq!(trim_bytes_newline_suffix(b"foo\n"), b"foo");
/// assert_eq!(trim_bytes_newline_suffix(b"foo\r\n"), b"foo");
/// assert_eq!(trim_bytes_newline_suffix(b"foo\nbar\n"), b"foo\nbar");
/// assert_eq!(trim_bytes_newline_suffix(b"foo\r\nbar\r\n"), b"foo\r\nbar");
///
/// assert_eq!(trim_bytes_newline_suffix(b"foo\n\r"), b"foo\n\r");
/// ```
pub fn trim_bytes_newline_suffix(s: &[u8]) -> &[u8] {
    // Even if the string ends with LF, the character before needs to be ASCII
    // or the LF is a continuation of a multi-byte UTF-8 character.
    if s.ends_with(b"\r\n") && (s.len() == 2 || s[s.len() - 3].is_ascii()) {
        &s[..s.len() - 2]
    } else if s.ends_with(b"\n") && (s.len() == 1 || s[s.len() - 2].is_ascii()) {
        &s[..s.len() - 1]
    } else {
        s
    }
}

pub fn strip_suffix<'a>(string: &'a str, suffix: &str) -> &'a str {
    if string.ends_with(suffix) {
        string.strip_suffix(suffix).unwrap()
    } else {
        string
    }
}

pub fn annotate_message(message: &str, subdir: &str, orig_commit_hash: &CommitId) -> String {
    let mut res = message.trim_end_matches("\n").to_string() + "\n";
    if !res.contains("\n\n") {
        // Single-line message. Only a subject.
        res.push('\n');
    }

    format!("{res}^-- {subdir} {orig_commit_hash}\n")
}

pub fn iter_to_string<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    items.into_iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone)]
pub struct PtrKey<T> {
    phantom: std::marker::PhantomData<T>,
    key: usize,
}

impl<T> Eq for PtrKey<T> {}

impl<T> PartialEq for PtrKey<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<T> Hash for PtrKey<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

pub type ArcKey<T> = PtrKey<std::sync::Arc<T>>;
pub type RcKey<T> = PtrKey<std::rc::Rc<T>>;

impl<T> ArcKey<T> {
    pub fn new(value: &std::sync::Arc<T>) -> Self {
        ArcKey {
            phantom: std::marker::PhantomData,
            key: std::sync::Arc::as_ptr(value).addr(),
        }
    }
}

impl<T> RcKey<T> {
    pub fn new(value: &std::rc::Rc<T>) -> Self {
        RcKey {
            phantom: std::marker::PhantomData,
            key: std::rc::Rc::as_ptr(value).addr(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitId;
    use anyhow::Result;

    #[test]
    fn test_annotate_message() -> Result<()> {
        let commit_id = CommitId::from_hex(b"0123456789abcdef0123456789abcdef01234567")?;
        // Don't fold the footer into the subject line, leave an empty line.
        assert_eq!(
            annotate_message("Subject line\n", "sub/dir", &commit_id),
            "\
Subject line

^-- sub/dir 0123456789abcdef0123456789abcdef01234567
"
        );

        assert_eq!(
            annotate_message("Subject line, no LF", "sub/dir", &commit_id),
            "\
Subject line, no LF

^-- sub/dir 0123456789abcdef0123456789abcdef01234567
"
        );

        assert_eq!(
            annotate_message("Double subject line\n", "sub/dir", &commit_id),
            "\
Double subject line

^-- sub/dir 0123456789abcdef0123456789abcdef01234567
"
        );

        assert_eq!(
            annotate_message("Subject line, extra LFs\n\n\n", "sub/dir", &commit_id),
            "\
Subject line, extra LFs

^-- sub/dir 0123456789abcdef0123456789abcdef01234567
",
        );

        assert_eq!(
            annotate_message("Multi line\n\nmessage\n", "sub/dir", &commit_id),
            "\
Multi line

message
^-- sub/dir 0123456789abcdef0123456789abcdef01234567
",
        );

        assert_eq!(
            annotate_message("Multi line\n\nmessage, no LF", "sub/dir", &commit_id),
            "\
Multi line

message, no LF
^-- sub/dir 0123456789abcdef0123456789abcdef01234567
",
        );

        assert_eq!(
            annotate_message(
                "Multi line\n\nmessage, extra LFs\n\n\n",
                "sub/dir",
                &commit_id,
            ),
            "\
Multi line

message, extra LFs
^-- sub/dir 0123456789abcdef0123456789abcdef01234567
",
        );
        Ok(())
    }
}
