use crate::git::CommitId;
use crate::git::GitPath;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use anyhow::Result;
use anyhow::anyhow;
use bstr::BStr;
use bstr::ByteSlice;
use gix::prelude::ObjectIdExt as _;
use itertools::Itertools;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

/// The rewritten commit messages gets this additinal footer in the form
/// `Git-Toprepo-Ref: path commit-id`.
///
/// This footer is useful for users to find which the original commit ids were
/// by simply using `git-log`.
///
/// The footer is also used by `git-toprepo` to split a commit message into
/// multiple commit messages for different submodules, when pushing a
/// cherry-picked mono commit into multiple repositories.
const GIT_TOPREPO_FOOTER_PREFIX: &str = "Git-Toprepo-Ref:";

/// Instead of an empty path, use this in the commit message footer.
const TOPREPO_DISPLAY_PATH: &str = "<top>";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PushMessage {
    /// The commit message for the commit to push to a remote.
    pub message: String,
    /// Any potential topic for the commit to belong to.
    pub topic: Option<String>,
}

pub fn calculate_mono_commit_message_from_commits(
    repo: &gix::Repository,
    super_path: &GitPath,
    super_commit_id: &CommitId,
    super_commit: &gix::objs::CommitRef<'_>,
    submod_updates: &HashMap<GitPath, ExpandedOrRemovedSubmodule>,
) -> String {
    let sub_commit_infos = submod_updates
        .iter()
        .map(|(path, submod)| {
            let _scope_guard_path = crate::log::scope(format!(
                "Path {super_path}{}{path}",
                if super_path.is_empty() { "" } else { "/" }
            ));
            let (submod_message, status) = match submod {
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::Expanded(submod)) => {
                    let message = match get_and_decode_commit_message(repo, submod.orig_commit_id) {
                        Ok(submod_message) => Some(submod_message),
                        Err(err) => {
                            log::warn!(
                                "Failed to get commit message {} at {path}: {err:#}",
                                submod.orig_commit_id
                            );
                            None
                        }
                    };
                    (message, submod.orig_commit_id.to_string())
                }
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::KeptAsSubmodule(
                    submod_commit_id,
                )) => {
                    // Normal submodule bumps are not recorded, the subrepo has not
                    // even been resolved so cannot find the commit message.
                    // TODO: 2025-09-22 Should we try to load the commit message from the subrepo if available?
                    (None, format!("{submod_commit_id} (submodule)"))
                }
                ExpandedOrRemovedSubmodule::Expanded(
                    ExpandedSubmodule::CommitMissingInSubRepo(submod),
                ) => {
                    // Normal submodule bumps are not recorded and the commit message cannot be resolved anyway.
                    (None, format!("{} not found", submod.orig_commit_id))
                }
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::UnknownSubmodule(
                    submod_commit_id,
                )) => {
                    // Normal submodule bumps are not recorded. With an unknown subrepo, the commit message cannot be resolved anyway.
                    (None, format!("{submod_commit_id} unknown submodule"))
                }
                ExpandedOrRemovedSubmodule::Expanded(
                    ExpandedSubmodule::RegressedNotFullyImplemented(submod),
                ) => {
                    // A short commit id is enough as a subject line, the full id is in the footer.
                    let short_submod_commit_id = submod.orig_commit_id.attach(repo).shorten_or_id();
                    (
                        Some(format!(
                            "Regressed (not fully implemented) to {short_submod_commit_id}",
                        )),
                        format!("{} regressed", submod.orig_commit_id),
                    )
                }
                ExpandedOrRemovedSubmodule::Removed => {
                    // The submodule has been removed, so no commit message.
                    (None, "removed".to_owned())
                }
            };
            CommitMessageInfo {
                path: path.clone(),
                message: submod_message,
                status,
            }
        })
        .collect_vec();
    // Add the super repo commit message.
    let super_info = {
        let _scope_guard_path = crate::log::scope(format!("Path {super_path}/",));
        CommitMessageInfo {
            path: super_path.clone(),
            message: Some(decode_commit_message(super_commit)),
            status: super_commit_id.to_string(),
        }
    };
    calculate_mono_commit_message(super_info, sub_commit_infos)
}

pub struct CommitMessageInfo {
    path: GitPath,
    message: Option<String>,
    status: String,
}

/// Construct a commit message from the submodule updates.
///
/// If the toprepo commit message starts with `Update git submodules`, the
/// message is assumed to be generated by Gerrit and optionally include the
/// commit messages of all updated submodules, for example:
/// ```text
/// Update git submodules
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
///     Change-Id: I0123456789abcdef0123456789abcdef01234567
///
///   - Parent commit to new algo
///
///     Change-Id: I89abcdef0123456789abcdef0123456789abcdef0
/// ```
///
/// First of all, the parent commit messages are not wanted. Secondly, when
/// cherry-picking the commit and pushing it to a different branch, the
/// submodule commit ids will be wrong and we don't want to push the whole
/// message to the toprepo change anyway. Therefore, compose a message from the
/// interesting messages only.
fn calculate_mono_commit_message(
    super_info: CommitMessageInfo,
    sub_infos: Vec<CommitMessageInfo>,
) -> String {
    let all_statuses_ordered = sub_infos
        .iter()
        .map(|info| (info.path.clone(), info.status.clone()))
        .chain(std::iter::once((
            super_info.path.clone(),
            super_info.status.clone(),
        )))
        .collect::<BTreeMap<_, _>>();

    let mut interesting_messages: BTreeMap<_, _> = sub_infos
        .into_iter()
        .filter_map(|info| {
            let message = info.message?;
            if !is_interesting_message(&message) {
                return None;
            }
            Some((info.path, message))
        })
        .collect();
    // Add the super repo commit message.
    if let Some(super_message) = super_info.message
        && (is_interesting_message(&super_message) || interesting_messages.is_empty())
    {
        // Even if the message is boring, if there are no submodule messages,
        // use the super repo message anyway.
        interesting_messages.insert(super_info.path.clone(), super_message);
    }
    if interesting_messages.is_empty() {
        // No interesting messages amon any submodule. Use a default boring message.
        interesting_messages.insert(
            super_info.path.clone(),
            "Update git submodules\n".to_owned(),
        );
    }

    // Group by identical messages, sort by how much that message is used.
    let mut message_to_paths = HashMap::new();
    for (path, msg) in &interesting_messages {
        message_to_paths
            .entry(msg.clone())
            .or_insert_with(Vec::new)
            .push(path.clone());
    }
    // In case of just one message, put all footers in the same paragraph in order.
    if message_to_paths.len() == 1 {
        *message_to_paths.values_mut().next().unwrap() =
            all_statuses_ordered.keys().cloned().collect();
    }
    let message_and_sorted_paths = message_to_paths
        .into_iter()
        .map(|(msg, mut paths)| {
            // The super_path is a prefix to all other paths and will therefore
            // be first.
            paths.sort();
            (msg, paths)
        })
        .sorted_by(|(_, lhs_paths), (_, rhs_paths)| {
            // Prioritise the super commit message.
            (lhs_paths.first() != Some(&super_info.path))
                .cmp(&(rhs_paths.first() != Some(&super_info.path)))
                // Then prioritise messages used by more submodules.
                .then_with(|| rhs_paths.len().cmp(&lhs_paths.len()))
                .then_with(|| lhs_paths.cmp(rhs_paths))
        })
        .collect_vec();

    let mut paths_done = HashSet::new();
    let mut mono_message = String::new();
    for (msg, ordered_paths) in message_and_sorted_paths {
        if !mono_message.is_empty() && !mono_message.ends_with("\n\n") {
            mono_message.push('\n');
        }
        mono_message.push_str(&msg);
        if !mono_message.ends_with('\n') {
            mono_message.push('\n');
        }
        if !commit_message_has_footer(mono_message.as_bytes().as_bstr()) {
            // Add an empty line before our new footer.
            mono_message.push('\n');
        }
        // Add some footers.
        for path in ordered_paths {
            let status = all_statuses_ordered
                .get(&path)
                .unwrap_or_else(|| unreachable!("Path {path} not in all_statuses"));
            let path_if_empty = if path.is_empty() { "<top>" } else { "" };
            mono_message.push_str(&format!(
                "{GIT_TOPREPO_FOOTER_PREFIX} {path}{path_if_empty} {status}\n"
            ));
            paths_done.insert(path.clone());
        }
    }
    // Print the remaining paths in order.
    let mut first_extra_path = true;
    for (path, status) in all_statuses_ordered.iter() {
        if !paths_done.contains(path) {
            if first_extra_path {
                first_extra_path = false;
                mono_message.push('\n');
            }
            let path_if_empty = if path.is_empty() { "<top>" } else { "" };
            mono_message.push_str(&format!(
                "{GIT_TOPREPO_FOOTER_PREFIX} {path}{path_if_empty} {status}\n"
            ));
        }
    }
    mono_message
}

/// Decoding problems are logged as debug message because the user cannot repair
/// the history anyway.
fn get_and_decode_commit_message(repo: &gix::Repository, commit_id: CommitId) -> Result<String> {
    let commit = repo
        .find_commit(commit_id)
        .map_err(|err| anyhow!("Failed to find original commit {commit_id}: {err:#}"))?;
    let commit = commit.decode()?;
    let _scope_guard = crate::log::scope(format!("Commit {commit_id}"));
    Ok(decode_commit_message(&commit))
}

/// Best effort decoding the commit message, logging any error.
// TODO: 2025-09-22 How to only log at tips? Fix that later if an issue arises.
fn decode_commit_message(commit: &gix::objs::CommitRef<'_>) -> String {
    let encoding = if let Some(encoding_name) = commit.encoding {
        encoding_rs::Encoding::for_label_no_replacement(encoding_name).unwrap_or_else(|| {
            log::debug!("Unknown commit message encoding {encoding_name:?}, assuming UTF-8");
            encoding_rs::UTF_8
        })
    } else {
        encoding_rs::UTF_8
    };
    let (message, had_errors) = encoding.decode_without_bom_handling(commit.message);
    if had_errors {
        log::debug!("Commit message decoding errors");
    }
    message.into_owned()
}

/// Split a commit message into multiple messages for different submodules
/// according to toprepo footers.
///
/// Returns a mapping from submodule path to commit message and any potential
/// last message not containing a footer.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::PushMessage;
/// use git_toprepo::commit_message::split_commit_message;
/// use git_toprepo::git::GitPath;
///
/// let full_message = "\
/// Subject line
///
/// Topic: not-the-footer-yet
///
/// Body line 1
/// Body line 2
///
/// Footer-Key: value
/// Git-Toprepo-Ref: sub1 0123456789abcdef0123456789abcdef01234567
/// Git-Toprepo-Ref: sub2 89abcdef0123456789abcdef0123456789abcdef0
/// Topic: my-topic
///
/// Subject 2
///
/// Another-Footer: another value
/// Git-Toprepo-Ref: <top> fedcba9876543210fedcba9876543210fedcba98
///
/// Residual message
///
/// Topic: with-topic
/// ";
/// let (messages, residual) = split_commit_message(full_message.to_owned()).unwrap();
/// let expected_sub_push_message = PushMessage {
///     message: "Subject line
///
/// Topic: not-the-footer-yet
///
/// Body line 1
/// Body line 2
///
/// Footer-Key: value
/// "
///     .to_owned(),
///     topic: Some("my-topic".to_owned()),
/// };
/// assert_eq!(
///     messages,
///     std::collections::HashMap::from_iter(vec![
///         (GitPath::from("sub1"), expected_sub_push_message.clone()),
///         (GitPath::from("sub2"), expected_sub_push_message),
///         (
///             GitPath::from(""),
///             PushMessage {
///                 message: "Subject 2
///
/// Another-Footer: another value
/// "
///                 .to_owned(),
///                 topic: None,
///             }
///         ),
///     ])
/// );
/// assert_eq!(
///     residual,
///     Some(PushMessage {
///         message: "Residual message\n".to_owned(),
///         topic: Some("with-topic".to_owned()),
///     })
/// );
/// ```
pub fn split_commit_message(
    full_message: String,
) -> Result<(HashMap<GitPath, PushMessage>, Option<PushMessage>)> {
    #[derive(Debug)]
    enum SplitState {
        /// Empty line before the subject.
        BeforeSubject,
        /// The subject line (the first paragraph) of the commit message.
        Subject { message: String },
        /// The body of the commit message.
        Body { message: String },
        /// So far conforming to be a footer of a commit message. At the end of
        /// a paragraph, continue with the commit message to find the first
        /// footer with a git-toprepo key.
        MaybeFooter {
            /// The commit message so far, with some footers removed.
            tidy_message: String,
            /// The full commit message so far, including all footers.
            full_message: String,
            /// Any topic footer found so far.
            topic: Option<String>,
        },
        /// So far conforming to be a footer of a commit message, containing at
        /// least one TopRepo footer line. If it turns out to be a valid footer
        /// paragraph, this is the end of the commit message and the next
        /// paragraph starts a message for a different submodule.
        ToprepoFooter {
            /// The commit message so far, with some footers removed.
            tidy_message: String,
            /// The full commit message so far, including all footers.
            full_message: String,
            /// Any topic footer found so far.
            topic: Option<String>,
            /// The submodule paths found so far.
            paths: Vec<GitPath>,
        },
    }
    impl SplitState {
        fn add_toprepo_footer_path(&mut self, path: GitPath) {
            match self {
                SplitState::MaybeFooter {
                    tidy_message,
                    full_message,
                    topic,
                } => {
                    *self = SplitState::ToprepoFooter {
                        tidy_message: std::mem::take(tidy_message),
                        full_message: std::mem::take(full_message),
                        topic: topic.take(),
                        paths: vec![path],
                    }
                }
                SplitState::ToprepoFooter { paths, .. } => paths.push(path),
                _ => unreachable!(
                    "TopRepo footer path can only be added in MaybeFooter or ToprepoFooter state"
                ),
            }
        }
    }

    let mut state = SplitState::BeforeSubject;
    let mut all_messages = HashMap::new();
    for line in (full_message + "\n\n").lines() {
        if line.is_empty() {
            // End of the paragraph.
            match &mut state {
                SplitState::BeforeSubject => {}
                SplitState::Subject { message } | SplitState::Body { message } => {
                    let new_message = std::mem::take(message) + "\n";
                    state = SplitState::MaybeFooter {
                        tidy_message: new_message.clone(),
                        full_message: new_message,
                        topic: None,
                    };
                }
                SplitState::MaybeFooter {
                    tidy_message,
                    full_message,
                    topic: _,
                } => {
                    // No TopRepo footer found, maybe the next paragraph is the
                    // actualy footer and this was just a footer pattern in the body
                    // of the commit message.
                    *tidy_message += "\n";
                    *full_message += "\n";
                }
                SplitState::ToprepoFooter {
                    tidy_message,
                    full_message: _,
                    topic,
                    paths,
                } => {
                    let mut final_message = tidy_message.as_str();
                    while final_message.ends_with("\n\n") {
                        final_message = &final_message[..final_message.len() - 1];
                    }
                    for path in paths {
                        if all_messages
                            .insert(
                                path.clone(),
                                PushMessage {
                                    message: final_message.to_owned(),
                                    topic: topic.clone(),
                                },
                            )
                            .is_some()
                        {
                            anyhow::bail!("Multiple commit messages for submodule {path}");
                        }
                    }
                    state = SplitState::BeforeSubject;
                }
            }
        } else {
            // Next line in the paragraph.
            match &mut state {
                SplitState::BeforeSubject => {
                    state = SplitState::Subject {
                        message: format!("{line}\n"),
                    };
                }
                SplitState::Subject { message } | SplitState::Body { message } => {
                    message.push_str(line);
                    message.push('\n');
                }
                SplitState::MaybeFooter {
                    tidy_message,
                    full_message,
                    topic,
                }
                | SplitState::ToprepoFooter {
                    tidy_message,
                    full_message,
                    topic,
                    paths: _,
                } => {
                    full_message.push_str(line);
                    full_message.push('\n');
                    if let Some(new_topic) = line.strip_prefix("Topic:") {
                        let new_topic = new_topic.trim();
                        if let Some(old_topic) = topic {
                            anyhow::bail!("Multiple topic footers: {new_topic} {old_topic}");
                        }
                        topic.replace(new_topic.to_owned());
                        // Skip this line from the commit message.
                        continue;
                    }
                    match get_toprepo_footer_subrepo_path(line)? {
                        Some(path) => {
                            state.add_toprepo_footer_path(path);
                        }
                        None => {
                            if is_footer_line(line.as_bytes().as_bstr()) {
                                // Continue in the footer.
                                tidy_message.push_str(line);
                                tidy_message.push('\n');
                            } else {
                                // This paragraph is not a footer at all.
                                state = SplitState::Body {
                                    message: std::mem::take(full_message),
                                };
                            }
                        }
                    }
                }
            }
        }
    }
    let residual_message = match (state, None) {
        (SplitState::BeforeSubject, _) => None,
        (SplitState::Subject { mut message }, topic)
        | (SplitState::Body { mut message }, topic)
        | (
            SplitState::MaybeFooter {
                tidy_message: mut message,
                full_message: _,
                topic,
            },
            _,
        ) => {
            // All lines in the message include a newline, so at lease one character and one newline exists.
            debug_assert!(message.len() >= 2);
            while message.ends_with("\n\n") {
                message.pop();
            }
            Some(PushMessage { message, topic })
        }
        (SplitState::ToprepoFooter { .. }, _) => {
            unreachable!("Toprepo footer has been followed by an empty line")
        }
    };
    Ok((all_messages, residual_message))
}

/// Extracts the submodule path from a TopRepo footer line, if any.
///
/// # Examples
/// ```
/// use git_toprepo::git::GitPath;
/// use git_toprepo::commit_message::get_toprepo_footer_subrepo_path_for_tests_only as get_toprepo_footer_subrepo_path;
///
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: path/to/submodule 0123456789abcdef").unwrap(),
///     Some(GitPath::from("path/to/submodule")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref:   path 0123456789abcdef").unwrap(),
///     Some(GitPath::from("path")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: path").unwrap(),
///     Some(GitPath::from("path")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: <top>").unwrap(),
///     Some(GitPath::from("")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Other-Footer: foo bar").unwrap(),
///     None,
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Not a footer").unwrap(),
///     None,
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: ").unwrap_err().to_string(),
///     "Empty submodule path in TopRepo footer \"Git-Toprepo-Ref: \"",
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref:    ").unwrap_err().to_string(),
///     "Empty submodule path in TopRepo footer \"Git-Toprepo-Ref:    \"",
/// );
/// ```
#[doc(hidden)]
fn get_toprepo_footer_subrepo_path(line: &str) -> Result<Option<GitPath>> {
    let Some(value) = line.strip_prefix(GIT_TOPREPO_FOOTER_PREFIX) else {
        return Ok(None);
    };
    // Looking for the next whitespace will trim the end.
    let value = value.trim_start();
    let subrepo_path = if let Some(idx) = value.find(|c: char| c.is_whitespace()) {
        &value[..idx]
    } else {
        value
    };
    if subrepo_path.is_empty() {
        anyhow::bail!("Empty submodule path in TopRepo footer {line:?}");
    }
    Ok(Some(GitPath::new(
        if subrepo_path == TOPREPO_DISPLAY_PATH {
            "".into()
        } else {
            subrepo_path.as_bytes().into()
        },
    )))
}

pub fn get_toprepo_footer_subrepo_path_for_tests_only(line: &str) -> Result<Option<GitPath>> {
    get_toprepo_footer_subrepo_path(line)
}

fn is_interesting_message(message: &str) -> bool {
    if message.starts_with("Update git submodules\n") {
        // A boring message generated by Gerrit.
        return false;
    }
    true
}

/// Check if the commit message has a footer section.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::extract_commit_message_footer;
///
/// let verify_no_footer = |msg: &str| {
///     assert_eq!(
///         extract_commit_message_footer(msg.into()),
///         (msg.into(), None)
///     );
/// };
///
/// verify_no_footer("Subject line\nmore subject");
/// verify_no_footer("Subject line\n\nBody (invalid footer line)\n");
/// verify_no_footer("Subject line\n\nInvalid_Key: value\nValid-Key: value");
///
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nFooter-Key: value".into()),
///     ("Subject line\n\n".into(), Some("Footer-Key: value".into())),
/// );
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nFooter-Key: value".into()),
///     ("Subject line\n\n".into(), Some("Footer-Key: value".into()))
/// );
/// verify_no_footer("Subject line\n\nFooter Key: value");
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nBody\n\nFooter-Key: value".into()),
///     (
///         "Subject line\n\nBody\n\n".into(),
///         Some("Footer-Key: value".into())
///     )
/// );
/// verify_no_footer("Subject line\n\nBody\n\nFooter Key: value");
/// assert_eq!(
///     extract_commit_message_footer(
///         "Subject line\n\nFooter-Key: value\nAnother-Footer: another value".into()
///     ),
///     (
///         "Subject line\n\n".into(),
///         Some("Footer-Key: value\nAnother-Footer: another value".into())
///     )
/// );
/// verify_no_footer("Subject line\n\nFooter Key: value\nAnother-Footer: value\n");
///
/// verify_no_footer("Subject line\n\nBad^Key: value");
/// verify_no_footer("Subject line\n\nBad_Key: value");
///
/// assert_eq!(
///     extract_commit_message_footer(
///         "With CRLF, spaces\nand extra newlines\n\r\n\r\nFooter-Key: value\r\n   \n".into()
///     ),
///     (
///         "With CRLF, spaces\nand extra newlines\n\r\n\r\n".into(),
///         Some("Footer-Key: value\r\n   \n".into())
///     )
/// );
/// ```
pub fn extract_commit_message_footer(message: &BStr) -> (&BStr, Option<&BStr>) {
    let mut footer_start = None;
    let mut line_start = message.len() - message.trim_start().len();
    let mut between_paragraphs = false;
    while line_start < message.len() {
        let line_end = line_start
            + message[line_start..]
                .find_byte(b'\n')
                .unwrap_or_else(|| message.len() - line_start - 1)
            + 1;
        let line = &message[line_start..line_end];
        if line.trim_start().is_empty() {
            between_paragraphs = true;
        } else {
            if is_footer_line(line) {
                if between_paragraphs {
                    footer_start = Some(line_start);
                }
            } else {
                // Non-empty line that is not a footer, then the whole paragraph
                // is discarded.
                footer_start = None;
            }
            between_paragraphs = false;
        }
        line_start = line_end;
    }
    (
        &message[0..footer_start.unwrap_or_else(|| message.len())],
        footer_start.map(|idx| &message[idx..message.len()]),
    )
}

/// Check if the commit message has a footer section.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::commit_message_has_footer;
///
/// assert!(!commit_message_has_footer("Subject line".into()));
/// assert!(commit_message_has_footer(
///     "Subject line\n\nFooter-Key: value".into()
/// ));
pub fn commit_message_has_footer(message: &BStr) -> bool {
    extract_commit_message_footer(message).1.is_some()
}

/// Check if the line is a proper footer line.
///
/// # Examples
/// ```
/// # use git_toprepo::commit_message::is_footer_line_for_tests_only as is_footer_line;
///
/// assert!(is_footer_line("Valid-Key: value".into()));
/// assert!(is_footer_line("Valid-Key1:value".into()));
/// assert!(!is_footer_line("Normal line".into()));
/// assert!(!is_footer_line("Invalid_Key: value".into()));
/// assert!(!is_footer_line("Invalid^Key: value".into()));
/// assert!(!is_footer_line(":Something".into()));
/// assert!(!is_footer_line("".into()));
/// ```
#[doc(hidden)]
fn is_footer_line(line: &BStr) -> bool {
    let Some(idx) = line.find_byte(b':') else {
        return false;
    };
    let key = line[..idx].as_bstr();
    if key.is_empty() {
        return false;
    }
    for c in key.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    true
}

pub fn is_footer_line_for_tests_only(line: &BStr) -> bool {
    is_footer_line(line)
}
